#!/usr/bin/env node
// T-0.4 / OQ-04 — ~/.copilot/ide/*.lock と logs/process-*.log から、読み取り専用で
// 「稼働中セッション」の判定ができるかを測るためのプローブ。
//
// ★ このスクリプトは **読み取りしかしない** (INV-1)。lock / log ファイルを一切変更しない。
// ★ `headers` など認証情報に相当するフィールドは**値を出さない** (INV-2)。
// ★ 合否判定はしない。数字を出すだけ (NFR-52)。
// ★ PID の生存確認は `tasklist` の出力テキストを見て判定する。**終了コードには依存しない**
//   (FR-P-72 と同じ考え方: 外部ツールの終了コードを判定材料にしない)。
//
//   node tools/probe/live-detect.mjs
//   node tools/probe/live-detect.mjs --json    JSON だけを標準出力に出す

import { readdirSync, statSync, readFileSync, existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { homedir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const OUT_DIR = join(ROOT, 'tools', 'probe-out');
const JSON_ONLY = process.argv.includes('--json');

/** 値を絶対に出さないフィールド名 (INV-2 / FR-C-72) */
// 完全一致で伏せるキー名
const SECRET_KEY_EXACT = /^(headers?|authorization|token|secret|password|passwd|cookie|credentials?)$/i;
// 部分一致で伏せるキー名。`authToken` / `accessToken` / `authInfo` のような複合名を拾う。
// **`tokenCount` / `totalTokens` / `token_based_billing` のようなトークン「数」のキーは伏せない** —
// これらは利用量表示の中核であり、秘密ではない (FR-C-132)。
const SECRET_KEY_PART =
  /(auth[-_]?(token|info|header)|access[-_]?token|refresh[-_]?token|id[-_]?token|session[-_]?token|bearer[-_]?token|(github|gh|copilot|oauth|pat)[-_]?token|api[-_]?key|apikey|client[-_]?secret|private[-_]?key|password|passphrase|credential)/i;

/** 値を絶対に出さないフィールド名か (INV-2 / FR-C-72) */
const SECRET_KEYS = { test: (k) => SECRET_KEY_EXACT.test(k) || SECRET_KEY_PART.test(k) };

const copilotHome = process.env.COPILOT_HOME || join(homedir(), '.copilot');

/**
 * 指定 PID が生存しているかを PowerShell Get-Process の出力から判定する。
 * tasklist は日本語ロケールだと「見つからない」メッセージがシステムのコードページで
 * 出力され UTF-8 デコードで文字化けするため使わない。Get-Process -ErrorAction SilentlyContinue
 * は見つからない場合 stdout に何も出さない (エラーは抑制する) ので、空文字かどうかだけで判定できる。
 * **終了コードは見ない** (FR-P-72 と同じ考え方)。読めなければ「取得不可」を返す。
 */
function checkPid(pid) {
  // PowerShell は Get-Process が非終了エラーを出すだけでも、SilentlyContinue で
  // 出力は抑制されるのにプロセス自体の終了コードが 0 以外になることがある。
  // **終了コードには依存しない** (FR-P-72 と同じ考え方)。execFileSync が例外を投げても
  // 標準出力 (err.stdout) が空文字かどうかだけで生存を判定する。
  let stdout;
  try {
    stdout = execFileSync(
      'powershell',
      [
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        `Get-Process -Id ${Number(pid)} -ErrorAction SilentlyContinue | Select-Object -ExpandProperty ProcessName`,
      ],
      { encoding: 'utf8', windowsHide: true }
    );
  } catch (err) {
    if (typeof err.stdout === 'string') {
      stdout = err.stdout;
    } else {
      // stdout すら取れない (powershell が無い環境等) — 取得不可
      return { alive: null, processName: null, error: String(err.message || err) };
    }
  }
  const trimmed = stdout.trim();
  if (!trimmed) {
    return { alive: false, processName: null };
  }
  return { alive: true, processName: trimmed };
}

function describeLockJsonKeys(obj) {
  const keys = [];
  const redacted = [];
  for (const k of Object.keys(obj)) {
    if (SECRET_KEYS.test(k)) redacted.push(k);
    else keys.push(k);
  }
  return { keys, redacted };
}

const report = {
  probedAt: new Date().toISOString(),
  copilotHome,
  exists: existsSync(copilotHome),
  locks: [],
  logs: [],
  lockPidSet: [],
  logPidSet: [],
  pidIntersection: [],
  logLevelCounts: {},
  logMessageFirstWordCounts: {},
  patternHits: {
    sessionIdLike: { count: 0, samples: [] },
    workspaceLike: { count: 0, samples: [] },
    cwdLike: { count: 0, samples: [] },
  },
  notes: [],
};

if (!report.exists) {
  report.notes.push('~/.copilot が存在しません。数字が出せません。');
} else {
  // --- ide/*.lock ---
  const ideDir = join(copilotHome, 'ide');
  if (existsSync(ideDir)) {
    const lockFiles = readdirSync(ideDir).filter((f) => f.endsWith('.lock'));
    for (const f of lockFiles) {
      const full = join(ideDir, f);
      const entry = { fileName: f };
      try {
        const st = statSync(full);
        entry.bytes = st.size;
        entry.mtime = st.mtime.toISOString();
      } catch (err) {
        entry.statError = String(err.message || err);
        report.locks.push(entry);
        continue;
      }
      let raw;
      try {
        raw = readFileSync(full, 'utf8');
      } catch (err) {
        entry.readError = String(err.message || err);
        report.locks.push(entry);
        continue;
      }
      let obj;
      try {
        obj = JSON.parse(raw);
      } catch (err) {
        entry.parseError = String(err.message || err);
        report.locks.push(entry);
        continue;
      }
      const { keys, redacted } = describeLockJsonKeys(obj);
      entry.keys = keys;
      entry.redactedKeys = redacted; // ★ headers 等はキー名だけ (INV-2)
      entry.pid = typeof obj.pid === 'number' ? obj.pid : null;
      entry.ideName = typeof obj.ideName === 'string' ? obj.ideName : null;
      entry.workspaceFolders = Array.isArray(obj.workspaceFolders) ? obj.workspaceFolders : null;
      entry.scheme = typeof obj.scheme === 'string' ? obj.scheme : null;
      entry.isTrusted = typeof obj.isTrusted === 'boolean' ? obj.isTrusted : null;
      entry.timestampRaw = obj.timestamp ?? null;
      if (typeof obj.timestamp === 'number') {
        entry.timestampIso = new Date(obj.timestamp).toISOString();
        entry.timestampVsMtimeMs =
          entry.mtime != null ? new Date(entry.mtime).getTime() - obj.timestamp : null;
        entry.ageMsFromNow = Date.now() - obj.timestamp;
      }
      if (entry.pid != null) {
        const check = checkPid(entry.pid);
        entry.pidAlive = check.alive;
        entry.pidProcessName = check.processName;
        if (check.error) entry.pidCheckError = check.error;
      }
      report.locks.push(entry);
    }
  } else {
    report.notes.push('ide/ ディレクトリが存在しません。');
  }

  // --- logs/process-*.log ---
  const logsDir = join(copilotHome, 'logs');
  const LINE_RE = /^(\S+)\s+\[(\w+)\]\s?(.*)$/;
  const SESSION_ID_RE = /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/i;
  const WORKSPACE_RE = /workspace/i;
  const CWD_RE = /\b(?:[a-zA-Z]:\\|\/)[^\s"']{2,}/; // Windows/Unix っぽいパス

  if (existsSync(logsDir)) {
    const files = readdirSync(logsDir).filter((f) => f.endsWith('.log'));
    for (const f of files) {
      const full = join(logsDir, f);
      const entry = { fileName: f };
      const m = f.match(/^process-(\d+)-(\d+)\.log$/);
      entry.parsedTimestampMs = m ? Number(m[1]) : null;
      entry.parsedPid = m ? Number(m[2]) : null;
      let st;
      try {
        st = statSync(full);
        entry.bytes = st.size;
        entry.mtime = st.mtime.toISOString();
      } catch (err) {
        entry.statError = String(err.message || err);
        report.logs.push(entry);
        continue;
      }
      let raw;
      try {
        raw = readFileSync(full, 'utf8');
      } catch (err) {
        entry.readError = String(err.message || err);
        report.logs.push(entry);
        continue;
      }
      // 末尾の空行を除いた行だけ数える
      const lines = raw.split(/\r?\n/).filter((l) => l.length > 0);
      entry.lineCount = lines.length;
      if (lines.length > 0) {
        const firstMatch = lines[0].match(LINE_RE);
        const lastMatch = lines[lines.length - 1].match(LINE_RE);
        entry.firstLineTimestamp = firstMatch ? firstMatch[1] : null;
        entry.lastLineTimestamp = lastMatch ? lastMatch[1] : null;
        entry.lastLineMessage = lastMatch ? lastMatch[3] : lines[lines.length - 1].slice(0, 200);
      }
      for (const line of lines) {
        const lm = line.match(LINE_RE);
        if (lm) {
          const level = lm[2];
          report.logLevelCounts[level] = (report.logLevelCounts[level] || 0) + 1;
          const msg = lm[3] || '';
          const firstWord = msg.trim().split(/\s+/)[0] || '(空)';
          report.logMessageFirstWordCounts[firstWord] =
            (report.logMessageFirstWordCounts[firstWord] || 0) + 1;
        }
        const sid = line.match(SESSION_ID_RE);
        if (sid) {
          report.patternHits.sessionIdLike.count++;
          if (report.patternHits.sessionIdLike.samples.length < 5) {
            report.patternHits.sessionIdLike.samples.push(sid[0]);
          }
        }
        if (WORKSPACE_RE.test(line)) {
          report.patternHits.workspaceLike.count++;
          if (report.patternHits.workspaceLike.samples.length < 5) {
            report.patternHits.workspaceLike.samples.push(line.slice(0, 200));
          }
        }
        const cwd = line.match(CWD_RE);
        if (cwd) {
          report.patternHits.cwdLike.count++;
          if (report.patternHits.cwdLike.samples.length < 5) {
            report.patternHits.cwdLike.samples.push(cwd[0]);
          }
        }
      }
      if (entry.parsedPid != null) {
        const check = checkPid(entry.parsedPid);
        entry.pidAlive = check.alive;
        entry.pidProcessName = check.processName;
        if (check.error) entry.pidCheckError = check.error;
      }
      report.logs.push(entry);
    }
  } else {
    report.notes.push('logs/ ディレクトリが存在しません。');
  }

  // --- PID 集合の交差 (件数と集合を別に算出しない) ---
  const lockPidSet = new Set(report.locks.map((l) => l.pid).filter((p) => p != null));
  const logPidSet = new Set(report.logs.map((l) => l.parsedPid).filter((p) => p != null));
  report.lockPidSet = [...lockPidSet];
  report.logPidSet = [...logPidSet];
  report.pidIntersection = [...lockPidSet].filter((p) => logPidSet.has(p));

  // 上位 20 語に絞る (メッセージ本文を大量に出さない)
  const topWords = Object.entries(report.logMessageFirstWordCounts)
    .sort((a, b) => b[1] - a[1])
    .slice(0, 20);
  report.logMessageFirstWordCounts = Object.fromEntries(topWords);
}

if (JSON_ONLY) {
  console.log(JSON.stringify(report, null, 2));
} else {
  console.log('=== live-detect プローブ (読み取り専用, OQ-04) ===\n');
  console.log('COPILOT_HOME :', report.copilotHome, report.exists ? '' : '(存在しません)');

  console.log('\n--- ide/*.lock ---');
  console.log('lock 件数    :', report.locks.length);
  for (const l of report.locks) {
    console.log(
      `  ${l.fileName} bytes=${l.bytes} pid=${l.pid} alive=${l.pidAlive} proc=${l.pidProcessName}` +
        ` ideName=${l.ideName} ws=${JSON.stringify(l.workspaceFolders)}` +
        ` ts=${l.timestampIso} mtime=${l.mtime} (ts-mtime差ms=${l.timestampVsMtimeMs}, ageMs=${l.ageMsFromNow})` +
        ` redacted=${JSON.stringify(l.redactedKeys)}`
    );
  }

  console.log('\n--- logs/process-*.log ---');
  console.log('log 件数     :', report.logs.length);
  for (const l of report.logs) {
    console.log(
      `  ${l.fileName} pid=${l.parsedPid} alive=${l.pidAlive} proc=${l.pidProcessName}` +
        ` bytes=${l.bytes} lines=${l.lineCount} first=${l.firstLineTimestamp} last=${l.lastLineTimestamp}` +
        ` lastMsg="${l.lastLineMessage}"`
    );
  }

  console.log('\n--- PID 集合の交差 ---');
  console.log('lock 側 PID  :', report.lockPidSet.join(', '));
  console.log('log 側 PID   :', report.logPidSet.join(', '));
  console.log('交差         :', report.pidIntersection.length, '件', report.pidIntersection);

  console.log('\n--- ログレベル件数 ---');
  console.log(JSON.stringify(report.logLevelCounts));

  console.log('\n--- メッセージ先頭語 上位20 ---');
  console.log(JSON.stringify(report.logMessageFirstWordCounts));

  console.log('\n--- パターン出現件数 ---');
  console.log('session id 風:', report.patternHits.sessionIdLike.count, report.patternHits.sessionIdLike.samples);
  console.log('workspace 語 :', report.patternHits.workspaceLike.count);
  console.log('cwd/path 風  :', report.patternHits.cwdLike.count, report.patternHits.cwdLike.samples);

  if (report.notes.length) {
    console.log('\n--- 注記 ---');
    for (const n of report.notes) console.log('*', n);
  }
}

mkdirSync(OUT_DIR, { recursive: true });
const outPath = join(OUT_DIR, 'live-detect.json');
writeFileSync(outPath, JSON.stringify(report, null, 2), 'utf8');
if (!JSON_ONLY) {
  console.log('\n結果を書き出しました:', outPath, '(コミットしないこと)');
}
