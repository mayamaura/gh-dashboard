#!/usr/bin/env node
// T-0.3 / OQ-01・OQ-07 — events.jsonl のレコード種別・フィールド出現率・
// サブエージェント/委任らしき構造を分類集計する (読み取り専用プローブ)。
//
// ★ 読み取りしかしない (INV-1)。~/.copilot/** に一切書き込まない。
// ★ 認証情報らしきキー (headers/authorization/token/secret/password/api_key/cookie) は
//   **キー名だけ報告し、値は絶対に出さない** (INV-2)。config.json と mcp-secrets/ は開かない。
// ★ 合否判定はしない。数字だけ出す (NFR-52)。
//
//   node tools/probe/record-kinds.mjs
//   node tools/probe/record-kinds.mjs --json
//
// COPILOT_HOME で ~/.copilot の場所を上書きできる。

import { readdirSync, statSync, existsSync, mkdirSync, writeFileSync, createReadStream } from 'node:fs';
import { join, dirname } from 'node:path';
import { homedir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { createInterface } from 'node:readline';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const OUT_DIR = join(ROOT, 'tools', 'probe-out');
const JSON_ONLY = process.argv.includes('--json');

// 完全一致で伏せるキー名
const SECRET_KEY_EXACT = /^(headers?|authorization|token|secret|password|passwd|cookie|credentials?)$/i;
// 部分一致で伏せるキー名。`authToken` / `accessToken` / `authInfo` のような複合名を拾う。
// **`tokenCount` / `totalTokens` / `token_based_billing` のようなトークン「数」のキーは伏せない** —
// これらは利用量表示の中核であり、秘密ではない (FR-C-132)。
const SECRET_KEY_PART =
  /(auth[-_]?(token|info|header)|access[-_]?token|refresh[-_]?token|id[-_]?token|session[-_]?token|bearer[-_]?token|(github|gh|copilot|oauth|pat)[-_]?token|api[-_]?key|apikey|client[-_]?secret|private[-_]?key|password|passphrase|credential)/i;

/** 値を絶対に出さないフィールド名か (INV-2 / FR-C-72) */
const SECRET_KEYS = { test: (k) => SECRET_KEY_EXACT.test(k) || SECRET_KEY_PART.test(k) };
// 種別を担いうるキーの候補。**値をそのまま出力する**ので、列挙値になりうるものだけを並べる。
// `name` は外してある — 実データの `workspace.yaml` の `name` は初回プロンプトの文面がそのまま入っており、
// 種別ではなく本文だった。ここに残すとセッション本文を出力してしまう (INV-6 / FR-C-02)。
const KIND_KEY_CANDIDATES = ['type', 'kind', 'event', 'eventType', 'event_type', 'record_type', 'recordType'];
const TIMESTAMP_KEY_HINT = /(time|timestamp|_at$|At$|date)/i;
const USAGE_KEY_HINT = /(token|usage|cost|credit|aiu|nano)/i;
const AGENT_KEY_HINT = /(parent|child|agent|subagent|delegat|task)/i;
const TOOL_CALL_HINT = /(tool_call|toolcall|tool_use|toolUse|function_call|functionCall)/i;

const copilotHome = process.env.COPILOT_HOME || join(homedir(), '.copilot');
const stateDir = join(copilotHome, 'session-state');

function findJsonlFiles(dir) {
  const found = [];
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return found;
  }
  for (const e of entries) {
    const full = join(dir, e.name);
    if (e.isDirectory()) found.push(...findJsonlFiles(full));
    else if (e.name === 'events.jsonl') found.push(full);
  }
  return found;
}

function redactKey(k) {
  return SECRET_KEYS.test(k) ? `${k} (伏せた)` : k;
}

function classifyTimestampFormat(value) {
  if (typeof value === 'string') {
    if (/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}/.test(value)) return 'ISO8601らしき文字列';
    if (/^\d{13}$/.test(value)) return 'epoch_ms らしき数字文字列';
    if (/^\d{10}$/.test(value)) return 'epoch_s らしき数字文字列';
    return '文字列 (形式不明)';
  }
  if (typeof value === 'number') {
    if (value > 1_000_000_000_000) return 'epoch_ms らしき数値';
    if (value > 1_000_000_00 && value < 10_000_000_000) return 'epoch_s らしき数値';
    return '数値 (形式不明)';
  }
  return typeof value;
}

function typeOf(v) {
  if (v === null) return 'null';
  if (Array.isArray(v)) return 'array';
  return typeof v;
}

/**
 * 1 つのオブジェクトのキーを、タイムスタンプ / usage / エージェント / ツールの
 * 候補として分類する。`prefix` は報告名に付ける階層 (例 `data.`)。
 * **値は出さない。** 列挙値になりうるエージェント系だけユニーク値を数える。
 */
function classifyKeys(state, obj, prefix) {
  for (const k of Object.keys(obj)) {
    const rk = prefix + redactKey(k);

    if (TIMESTAMP_KEY_HINT.test(k)) {
      const tsKey = `${rk} (${classifyTimestampFormat(obj[k])})`;
      state.timestampCandidates.set(tsKey, (state.timestampCandidates.get(tsKey) || 0) + 1);
    }

    if (USAGE_KEY_HINT.test(k)) {
      state.usageKeyCandidates.set(rk, (state.usageKeyCandidates.get(rk) || 0) + 1);
    }

    if (AGENT_KEY_HINT.test(k)) {
      if (!state.agentKeyCandidates.has(rk)) {
        state.agentKeyCandidates.set(rk, { count: 0, uniqueValues: new Set() });
      }
      const e = state.agentKeyCandidates.get(rk);
      e.count++;
      const v = obj[k];
      if (typeof v === 'string' || typeof v === 'number' || typeof v === 'boolean') {
        e.uniqueValues.add(String(v));
      } else {
        e.uniqueValues.add(`(${typeOf(v)})`);
      }
    }

    if (TOOL_CALL_HINT.test(k)) {
      state.toolCallKeyHitCount++;
    }
  }
}

/** レコード種別を担うキーの候補ごとに、値の出現を集計する。 */
class KindTally {
  constructor() {
    this.byCandidateKey = new Map(); // candidateKey -> Map(value -> {count, bytes, firstPos, lastPos})
  }
  add(candidateKey, value, byteLen, pos) {
    if (!this.byCandidateKey.has(candidateKey)) this.byCandidateKey.set(candidateKey, new Map());
    const m = this.byCandidateKey.get(candidateKey);
    const key = typeof value === 'string' || typeof value === 'number' ? String(value) : `(${typeOf(value)})`;
    if (!m.has(key)) m.set(key, { count: 0, bytes: 0, firstPos: pos, lastPos: pos });
    const e = m.get(key);
    e.count++;
    e.bytes += byteLen;
    e.lastPos = pos;
  }
}

async function scanFile(filePath, state) {
  const st = statSync(filePath);
  let pos = 0; // 行の開始バイト位置 (概算: UTF-8 バイト長を都度加算)
  const rl = createInterface({ input: createReadStream(filePath, { encoding: 'utf8' }), crlfDelay: Infinity });

  for await (const line of rl) {
    const byteLen = Buffer.byteLength(line, 'utf8') + 1; // + 改行 1 バイト概算 (CRLF 環境では誤差あり)
    const startPos = pos;
    pos += byteLen;
    if (line.length === 0) continue;

    state.totalLines++;
    let obj;
    try {
      obj = JSON.parse(line);
    } catch {
      state.parseFailCount++;
      continue;
    }
    if (typeof obj !== 'object' || obj === null || Array.isArray(obj)) {
      state.nonObjectRecordCount++;
      continue;
    }
    state.recordCount++;

    // 種別キー候補ごとに集計
    for (const cand of KIND_KEY_CANDIDATES) {
      if (cand in obj) {
        state.kindTally.add(cand, obj[cand], byteLen, startPos);
      }
    }

    // トップレベルフィールド出現率
    for (const k of Object.keys(obj)) {
      const rk = redactKey(k);
      state.topFieldCount.set(rk, (state.topFieldCount.get(rk) || 0) + 1);
      const t = typeOf(obj[k]);
      const tkey = `${rk}:${t}`;
      state.topFieldTypeCount.set(tkey, (state.topFieldTypeCount.get(tkey) || 0) + 1);

      // 2 階層目
      if (t === 'object') {
        for (const k2 of Object.keys(obj[k])) {
          const rk2 = `${rk}.${redactKey(k2)}`;
          state.nestedFieldCount.set(rk2, (state.nestedFieldCount.get(rk2) || 0) + 1);
        }
      }
    }

    // ヒント判定は 2 階層目まで見る。
    // 実データはレコード本体を data の下に入れるため、トップレベルだけを見ると
    // data.totalNanoAiu / data.parentToolCallId のような肝心のキーを取りこぼす。
    classifyKeys(state, obj, '');
    for (const k of Object.keys(obj)) {
      if (typeOf(obj[k]) === 'object') classifyKeys(state, obj[k], `${redactKey(k)}.`);
    }

    // type/kind の値自体が tool_call 系を指しているか (列挙値なので値を出してよい)
    for (const cand of ['type', 'kind', 'event', 'eventType', 'event_type']) {
      if (cand in obj && typeof obj[cand] === 'string' && /tool/i.test(obj[cand])) {
        state.toolCallRecordHitCount++;
        break;
      }
    }
  }

  return { path: filePath, bytes: st.size };
}

const report = {
  probedAt: new Date().toISOString(),
  copilotHome,
  stateDirExists: existsSync(stateDir),
  filesScanned: 0,
  files: [],
  totalLines: 0,
  parseFailCount: 0,
  nonObjectRecordCount: 0,
  recordCount: 0,
  kindKeyCandidates: {},
  topLevelFieldOccurrence: [],
  topLevelFieldTypeDistribution: [],
  nestedFieldOccurrence: [],
  timestampKeyCandidates: [],
  usageKeyCandidates: [],
  agentKeyCandidates: [],
  toolCallKeyHitCount: 0,
  toolCallRecordHitCount: 0,
  notes: [],
};

async function main() {
  if (!report.stateDirExists) {
    report.notes.push(
      'session-state/ が存在しません。対象データが 0 件のため、レコード種別・フィールド出現率などの数字は出せません。'
    );
  } else {
    const files = findJsonlFiles(stateDir);
    report.filesScanned = files.length;
    if (files.length === 0) {
      report.notes.push('session-state/ は存在しますが events.jsonl が 0 件です。');
    }

    const state = {
      totalLines: 0,
      parseFailCount: 0,
      nonObjectRecordCount: 0,
      recordCount: 0,
      kindTally: new KindTally(),
      topFieldCount: new Map(),
      topFieldTypeCount: new Map(),
      nestedFieldCount: new Map(),
      timestampCandidates: new Map(),
      usageKeyCandidates: new Map(),
      agentKeyCandidates: new Map(),
      toolCallKeyHitCount: 0,
      toolCallRecordHitCount: 0,
    };

    for (const f of files) {
      try {
        const r = await scanFile(f, state);
        report.files.push(r);
      } catch (e) {
        report.notes.push(`読み取り失敗のためスキップ: ${f} (${e.message})`);
      }
    }

    report.totalLines = state.totalLines;
    report.parseFailCount = state.parseFailCount;
    report.nonObjectRecordCount = state.nonObjectRecordCount;
    report.recordCount = state.recordCount;
    report.toolCallKeyHitCount = state.toolCallKeyHitCount;
    report.toolCallRecordHitCount = state.toolCallRecordHitCount;

    const total = state.recordCount || 1;

    for (const [cand, valueMap] of state.kindTally.byCandidateKey.entries()) {
      const values = [...valueMap.entries()].map(([v, e]) => ({
        value: v,
        count: e.count,
        totalBytes: e.bytes,
        avgBytes: Math.round(e.bytes / e.count),
        firstPos: e.firstPos,
        lastPos: e.lastPos,
      }));
      report.kindKeyCandidates[cand] = {
        recordsWithThisKey: values.reduce((a, v) => a + v.count, 0),
        uniqueValues: values.length,
        values: values.sort((a, b) => b.count - a.count),
      };
    }

    report.topLevelFieldOccurrence = [...state.topFieldCount.entries()]
      .map(([k, c]) => ({ key: k, count: c, rate: +(c / total).toFixed(4) }))
      .sort((a, b) => b.count - a.count);

    report.topLevelFieldTypeDistribution = [...state.topFieldTypeCount.entries()]
      .map(([k, c]) => ({ keyType: k, count: c }))
      .sort((a, b) => b.count - a.count);

    report.nestedFieldOccurrence = [...state.nestedFieldCount.entries()]
      .map(([k, c]) => ({ key: k, count: c, rate: +(c / total).toFixed(4) }))
      .sort((a, b) => b.count - a.count);

    report.timestampKeyCandidates = [...state.timestampCandidates.entries()]
      .map(([k, c]) => ({ key: k, count: c }))
      .sort((a, b) => b.count - a.count);

    report.usageKeyCandidates = [...state.usageKeyCandidates.entries()]
      .map(([k, c]) => ({ key: k, count: c, rate: +(c / total).toFixed(4) }))
      .sort((a, b) => b.count - a.count);

    report.agentKeyCandidates = [...state.agentKeyCandidates.entries()]
      .map(([k, e]) => ({ key: k, count: e.count, rate: +(e.count / total).toFixed(4), uniqueValueCount: e.uniqueValues.size }))
      .sort((a, b) => b.count - a.count);
  }

  if (JSON_ONLY) {
    console.log(JSON.stringify(report, null, 2));
  } else {
    console.log('=== レコード種別・フィールド出現率プローブ (OQ-01 / OQ-07) ===\n');
    console.log('COPILOT_HOME       :', report.copilotHome);
    console.log('session-state/ 存在:', report.stateDirExists);
    console.log('走査ファイル数     :', report.filesScanned);
    console.log('総行数             :', report.totalLines);
    console.log('オブジェクトレコード数:', report.recordCount);
    console.log('parse 失敗         :', report.parseFailCount);
    console.log('非オブジェクト行   :', report.nonObjectRecordCount);

    console.log('\n--- 種別キー候補ごとの件数 ---');
    for (const [cand, info] of Object.entries(report.kindKeyCandidates)) {
      console.log(`キー候補 "${cand}": ${info.recordsWithThisKey} 件中の内訳 (ユニーク値 ${info.uniqueValues})`);
      for (const v of info.values) {
        console.log(`  値="${v.value}" count=${v.count} totalBytes=${v.totalBytes} avgBytes=${v.avgBytes} firstPos=${v.firstPos} lastPos=${v.lastPos}`);
      }
    }

    console.log('\n--- トップレベルフィールド出現率 ---');
    for (const f of report.topLevelFieldOccurrence) {
      console.log(`  ${f.key}: ${f.count} 件 (${(f.rate * 100).toFixed(1)}%)`);
    }

    console.log('\n--- フィールドの型分布 (同一キーが複数型を持つか) ---');
    for (const f of report.topLevelFieldTypeDistribution) {
      console.log(`  ${f.keyType}: ${f.count} 件`);
    }

    console.log('\n--- 2 階層目フィールド出現率 ---');
    for (const f of report.nestedFieldOccurrence) {
      console.log(`  ${f.key}: ${f.count} 件 (${(f.rate * 100).toFixed(1)}%)`);
    }

    console.log('\n--- タイムスタンプ候補キー ---');
    for (const t of report.timestampKeyCandidates) {
      console.log(`  ${t.key}: ${t.count} 件`);
    }

    console.log('\n--- usage/cost/credit 候補キー ---');
    for (const u of report.usageKeyCandidates) {
      console.log(`  ${u.key}: ${u.count} 件 (${(u.rate * 100).toFixed(1)}%)`);
    }

    console.log('\n--- ツール呼び出しらしきキー/レコード ---');
    console.log('  キー名ヒット数   :', report.toolCallKeyHitCount);
    console.log('  レコード値ヒット数:', report.toolCallRecordHitCount);

    console.log('\n--- サブエージェント/委任らしき候補キー (OQ-07) ---');
    for (const a of report.agentKeyCandidates) {
      console.log(`  ${a.key}: ${a.count} 件 (${(a.rate * 100).toFixed(1)}%) ユニーク値数=${a.uniqueValueCount}`);
    }

    if (report.notes.length) {
      console.log('\n--- 注記 ---');
      for (const n of report.notes) console.log('*', n);
    }
  }

  mkdirSync(OUT_DIR, { recursive: true });
  const outPath = join(OUT_DIR, 'record-kinds.json');
  writeFileSync(outPath, JSON.stringify(report, null, 2), 'utf8');
  if (!JSON_ONLY) {
    console.log('\n結果を書き出しました:', outPath, '(コミットしないこと)');
  }
}

main();
