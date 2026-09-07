#!/usr/bin/env node
// T-0.1 / OQ-10 — ローカル Copilot データの構成と規模を出す (読み取り専用プローブ)。
//
// ★ このスクリプトは **読み取りしかしない** (INV-1)。書き込み・設定変更を一切行わない。
// ★ 認証情報に相当するフィールドは**値を出さない** (INV-2)。キー名だけを報告する。
// ★ 出力は tools/probe-out/ に置く。**コミットしない** (個人のセッション内容を含みうる)。
//
//   node tools/probe/copilot-layout.mjs
//   node tools/probe/copilot-layout.mjs --json    JSON だけを標準出力に出す
//
// 合否判定はしない。数字を出すだけ (NFR-52)。

import { readdirSync, statSync, readFileSync, existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { join, dirname, extname } from 'node:path';
import { homedir } from 'node:os';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const OUT_DIR = join(ROOT, 'tools', 'probe-out');
const JSON_ONLY = process.argv.includes('--json');

/** 値を絶対に出さないフィールド名 (INV-2 / FR-C-72) */
const SECRET_KEYS = /^(headers?|authorization|token|access_token|refresh_token|secret|password|api_?key|cookie)$/i;

const copilotHome = process.env.COPILOT_HOME || join(homedir(), '.copilot');

function walk(dir, depth = 0, maxDepth = 3) {
  const out = { dirs: 0, files: 0, bytes: 0, maxFile: null, byExt: {} };
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return out; // 読めないものはスキップする (全体を失敗させない)
  }
  for (const e of entries) {
    const full = join(dir, e.name);
    if (e.isDirectory()) {
      out.dirs++;
      if (depth < maxDepth) {
        const sub = walk(full, depth + 1, maxDepth);
        out.dirs += sub.dirs;
        out.files += sub.files;
        out.bytes += sub.bytes;
        for (const [k, v] of Object.entries(sub.byExt)) {
          out.byExt[k] = (out.byExt[k] || 0) + v;
        }
        if (sub.maxFile && (!out.maxFile || sub.maxFile.size > out.maxFile.size)) {
          out.maxFile = sub.maxFile;
        }
      }
      continue;
    }
    let st;
    try {
      st = statSync(full);
    } catch {
      continue;
    }
    out.files++;
    out.bytes += st.size;
    const ext = extname(e.name) || '(なし)';
    out.byExt[ext] = (out.byExt[ext] || 0) + 1;
    if (!out.maxFile || st.size > out.maxFile.size) {
      out.maxFile = { path: full, size: st.size };
    }
  }
  return out;
}

/** JSON のキー構造だけを報告する。**値は出さない**。 */
function describeJsonKeys(path) {
  try {
    const obj = JSON.parse(readFileSync(path, 'utf8'));
    if (typeof obj !== 'object' || obj === null) return { keys: [], redacted: [] };
    const keys = [];
    const redacted = [];
    for (const k of Object.keys(obj)) {
      if (SECRET_KEYS.test(k)) redacted.push(k);
      else keys.push(k);
    }
    return { keys, redacted };
  } catch {
    return { keys: [], redacted: [], error: 'JSON として読めません' };
  }
}

const report = {
  probedAt: new Date().toISOString(),
  copilotHome,
  exists: existsSync(copilotHome),
  topLevel: [],
  totals: null,
  ide: null,
  logs: null,
  sessionState: null,
  sessionStoreDb: null,
  notes: [],
};

if (!report.exists) {
  report.notes.push('~/.copilot が存在しません。Copilot CLI をまだ使っていない可能性があります。');
} else {
  report.topLevel = readdirSync(copilotHome, { withFileTypes: true })
    .map((e) => (e.isDirectory() ? e.name + '/' : e.name))
    .sort();

  report.totals = walk(copilotHome);

  // ide/*.lock — FR-C-70 に使えるか (OQ-04)
  const ideDir = join(copilotHome, 'ide');
  if (existsSync(ideDir)) {
    const locks = readdirSync(ideDir).filter((f) => f.endsWith('.lock'));
    const sample = locks[0] ? describeJsonKeys(join(ideDir, locks[0])) : null;
    report.ide = {
      lockCount: locks.length,
      sampleKeys: sample ? sample.keys : [],
      // ★ 値は出さない。存在することだけを報告する
      redactedKeys: sample ? sample.redacted : [],
    };
    if (report.ide.redactedKeys.length > 0) {
      report.notes.push(
        `ide/*.lock に認証情報を含みうるキーがあります: ${report.ide.redactedKeys.join(', ')}。DTO に定義しないこと (INV-2 / FR-C-72)。`
      );
    }
  }

  // logs/process-{timestamp}-{pid}.log — 稼働検出の手掛かり (OQ-04)
  const logsDir = join(copilotHome, 'logs');
  if (existsSync(logsDir)) {
    const files = readdirSync(logsDir).filter((f) => f.endsWith('.log'));
    const parsed = files
      .map((f) => f.match(/^process-(\d+)-(\d+)\.log$/))
      .filter(Boolean)
      .map((m) => ({ timestamp: Number(m[1]), pid: Number(m[2]) }));
    report.logs = {
      count: files.length,
      parsedNameCount: parsed.length,
      pidSample: parsed.slice(0, 5).map((p) => p.pid),
      timestampLooksLikeEpochMs: parsed.every(
        (p) => p.timestamp > 1_500_000_000_000 && p.timestamp < 4_000_000_000_000
      ),
    };
  }

  // session-state/ — OQ-01 / OQ-02 / OQ-10 の本体
  const stateDir = join(copilotHome, 'session-state');
  if (existsSync(stateDir)) {
    const sessions = readdirSync(stateDir, { withFileTypes: true }).filter((e) => e.isDirectory());
    let jsonlFiles = 0;
    let jsonlBytes = 0;
    let maxJsonl = 0;
    for (const s of sessions) {
      const events = join(stateDir, s.name, 'events.jsonl');
      try {
        const st = statSync(events);
        jsonlFiles++;
        jsonlBytes += st.size;
        maxJsonl = Math.max(maxJsonl, st.size);
      } catch {
        // events.jsonl が無いセッションもありうる
      }
    }
    report.sessionState = {
      sessionCount: sessions.length,
      jsonlFiles,
      jsonlBytes,
      maxJsonlBytes: maxJsonl,
    };
  } else {
    report.notes.push(
      'session-state/ が存在しません。OQ-01 / OQ-02 の調査には、先に Copilot CLI でセッションを作る必要があります。'
    );
  }

  const storeDb = join(copilotHome, 'session-store.db');
  report.sessionStoreDb = existsSync(storeDb)
    ? { exists: true, bytes: statSync(storeDb).size }
    : { exists: false };
}

// VS Code 側 (OQ-03)
const wsStorage = process.env.APPDATA
  ? join(process.env.APPDATA, 'Code', 'User', 'workspaceStorage')
  : null;
report.vscode = { path: wsStorage, exists: false, workspaceCount: 0, withChatSessions: 0 };
if (wsStorage && existsSync(wsStorage)) {
  const dirs = readdirSync(wsStorage, { withFileTypes: true }).filter((e) => e.isDirectory());
  report.vscode.exists = true;
  report.vscode.workspaceCount = dirs.length;
  report.vscode.withChatSessions = dirs.filter((d) =>
    existsSync(join(wsStorage, d.name, 'chatSessions'))
  ).length;
}

const mb = (n) => (n / 1024 / 1024).toFixed(1) + ' MB';

if (JSON_ONLY) {
  console.log(JSON.stringify(report, null, 2));
} else {
  console.log('=== Copilot ローカルデータ プローブ (読み取り専用) ===\n');
  console.log('COPILOT_HOME     :', report.copilotHome, report.exists ? '' : '(存在しません)');
  if (report.totals) {
    console.log('直下の項目       :', report.topLevel.join(', '));
    console.log(
      '総量             :',
      `${report.totals.files} ファイル / ${mb(report.totals.bytes)}`
    );
    if (report.totals.maxFile) {
      console.log('最大ファイル     :', mb(report.totals.maxFile.size));
    }
    console.log('拡張子別         :', JSON.stringify(report.totals.byExt));
  }
  if (report.ide) {
    console.log('\n--- ide/ (OQ-04) ---');
    console.log('lock ファイル数  :', report.ide.lockCount);
    console.log('キー             :', report.ide.sampleKeys.join(', '));
    console.log('伏せたキー       :', report.ide.redactedKeys.join(', ') || '(なし)');
  }
  if (report.logs) {
    console.log('\n--- logs/ (OQ-04) ---');
    console.log('ログ数           :', report.logs.count);
    console.log('名前から PID 取得:', report.logs.parsedNameCount, '件');
    console.log('timestamp は ms  :', report.logs.timestampLooksLikeEpochMs);
  }
  console.log('\n--- session-state/ (OQ-01 / 02 / 10) ---');
  if (report.sessionState) {
    console.log('セッション数     :', report.sessionState.sessionCount);
    console.log('events.jsonl     :', report.sessionState.jsonlFiles, '件');
    console.log('合計             :', mb(report.sessionState.jsonlBytes));
    console.log('最大             :', mb(report.sessionState.maxJsonlBytes));
  } else {
    console.log('(存在しません)');
  }
  console.log('\n--- session-store.db (OQ-05) ---');
  console.log(
    report.sessionStoreDb?.exists ? mb(report.sessionStoreDb.bytes) : '(存在しません)'
  );
  console.log('\n--- VS Code workspaceStorage (OQ-03) ---');
  console.log('ワークスペース数 :', report.vscode.workspaceCount);
  console.log('chatSessions あり:', report.vscode.withChatSessions);

  if (report.notes.length) {
    console.log('\n--- 注記 ---');
    for (const n of report.notes) console.log('*', n);
  }
}

mkdirSync(OUT_DIR, { recursive: true });
const outPath = join(OUT_DIR, 'copilot-layout.json');
writeFileSync(outPath, JSON.stringify(report, null, 2), 'utf8');
if (!JSON_ONLY) {
  console.log('\n結果を書き出しました:', outPath, '(コミットしないこと)');
}
