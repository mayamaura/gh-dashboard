#!/usr/bin/env node
// T-0.5 / OQ-03 — VS Code workspaceStorage の chatSessions と <hash> 導出を調べる (読み取り専用プローブ)。
//
// ★ このスクリプトは **読み取りしかしない** (INV-1)。workspaceStorage/** に一切書き込まない。
// ★ 認証情報に相当するフィールドは**値を出さない** (INV-2)。キー名だけを報告する。
// ★ 出力は tools/probe-out/ に置く。**コミットしない** (個人のセッション内容を含みうる)。
//
//   node tools/probe/vscode-sessions.mjs
//   node tools/probe/vscode-sessions.mjs --json    JSON だけを標準出力に出す
//
// 合否判定はしない。数字を出すだけ (NFR-52)。

import { readdirSync, statSync, readFileSync, existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { join, dirname, extname } from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';

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

/** cwd / ワークスペースパスらしき文字列 (Windows ドライブパス or POSIX ホームパス) */
const PATH_LIKE = /^[A-Za-z]:[\\/](?:[^"'\n]{1,120})$|^\/(?:home|Users|mnt)\/[^"'\n]{1,120}$/;

const appData = process.env.APPDATA || null;
const wsStorage = appData ? join(appData, 'Code', 'User', 'workspaceStorage') : null;

const report = {
  probedAt: new Date().toISOString(),
  workspaceStorage: wsStorage,
  exists: wsStorage ? existsSync(wsStorage) : false,
  hashRecalculation: null,
  chatSessions: null,
  otherFilesUnderHash: null,
  notes: [],
};

if (!wsStorage) {
  report.notes.push('APPDATA が設定されていません。この環境では対象がありません。');
} else if (!report.exists) {
  report.notes.push(`${wsStorage} が存在しません。対象がありません。`);
} else {
  const hashDirs = readdirSync(wsStorage, { withFileTypes: true }).filter((e) => e.isDirectory());

  // ---------------------------------------------------------------
  // (a) hash の再計算と照合
  // ---------------------------------------------------------------
  function toFsPathFromFileUri(uriStr) {
    // file:///c%3A/Users/... -> c:/Users/... (簡易デコード。他スキームは変換しない)
    if (!uriStr.startsWith('file:///')) return null;
    try {
      const rest = uriStr.slice('file:///'.length);
      return decodeURIComponent(rest);
    } catch {
      return null;
    }
  }

  function candidatesFor(uriStr) {
    const c = {};
    c['uri そのまま'] = uriStr;
    c['uri 小文字化'] = uriStr.toLowerCase();
    c['uri 末尾スラッシュ追加'] = uriStr.endsWith('/') ? uriStr : uriStr + '/';
    c['uri 末尾スラッシュ除去'] = uriStr.endsWith('/') ? uriStr.slice(0, -1) : uriStr;
    c['uri %3a 小文字化'] = uriStr.replace(/%3A/gi, '%3a');
    c['uri コロン非エンコード化'] = uriStr.replace(/%3A/gi, ':');
    const fsPath = toFsPathFromFileUri(uriStr);
    if (fsPath) {
      c['fsPath (デコード後)'] = fsPath;
      c['fsPath 小文字化'] = fsPath.toLowerCase();
      c['fsPath バックスラッシュ化'] = fsPath.replace(/\//g, '\\');
      c['fsPath バックスラッシュ化+小文字'] = fsPath.replace(/\//g, '\\').toLowerCase();
    }
    return c;
  }

  const ALGOS = ['md5', 'sha1', 'sha256'];
  // candidate 名 x algo 名 -> 一致件数
  const matchTable = {};
  let withWorkspaceJson = 0;
  let withoutWorkspaceJson = 0;
  let matchedAny = 0;
  let unmatchedAny = 0;
  const unmatchedSample = [];

  for (const d of hashDirs) {
    const wj = join(wsStorage, d.name, 'workspace.json');
    if (!existsSync(wj)) {
      withoutWorkspaceJson++;
      continue;
    }
    withWorkspaceJson++;
    let obj;
    try {
      obj = JSON.parse(readFileSync(wj, 'utf8'));
    } catch {
      withoutWorkspaceJson++;
      withWorkspaceJson--;
      continue;
    }
    const uriStr = obj.folder ?? obj.workspace ?? null;
    if (typeof uriStr !== 'string') {
      continue;
    }
    const cands = candidatesFor(uriStr);
    let thisOneMatched = false;
    for (const [name, value] of Object.entries(cands)) {
      for (const algo of ALGOS) {
        const key = `${name} / ${algo}`;
        if (!(key in matchTable)) matchTable[key] = 0;
        let digest;
        try {
          digest = createHash(algo).update(value, 'utf8').digest('hex');
        } catch {
          continue;
        }
        if (digest === d.name) {
          matchTable[key]++;
          thisOneMatched = true;
        }
      }
    }
    if (thisOneMatched) matchedAny++;
    else {
      unmatchedAny++;
      if (unmatchedSample.length < 5) {
        unmatchedSample.push({ hash: d.name, uriScheme: uriStr.split(':')[0] });
      }
    }
  }

  report.hashRecalculation = {
    hashDirCount: hashDirs.length,
    withWorkspaceJson,
    withoutWorkspaceJson,
    matchedAtLeastOneCandidate: matchedAny,
    matchedNoCandidate: unmatchedAny,
    perCandidateMatchCount: matchTable,
    unmatchedSample,
  };

  // ---------------------------------------------------------------
  // (b) chatSessions の中身
  // ---------------------------------------------------------------
  let fileCount = 0;
  let totalBytes = 0;
  let maxBytes = 0;
  let maxFile = null;
  const extCounts = {};
  const kind0TopKeyCounts = {}; // トップレベル(v の下)キーの出現回数
  const kind0FileCount = { withAtLeastOne: 0 };
  const pathLikeHits = {}; // キーパス -> { count, samples: [] }
  const timestampFieldCounts = {}; // キー名 -> { count, typeCounts }
  let mtimeVsInternalSamples = [];
  let orphanCount = 0;
  let dirsWithChatSessions = 0;
  const redactedKeysSeen = new Set();
  // 参考: kind0 (先頭スナップショット) だけでなく、ファイル全行 (kind1/2 の patch も含む) を
  // 正規表現で走査した場合にパスらしき文字列が見つかるファイル数 (FR-P-55 の裏取り)
  const FULLFILE_PATH_RE = /([A-Za-z]:[\\/][^"']{3,120})|(\/(home|Users|mnt)\/[^"']{3,120})/;
  let fullFileFilesWithPathLike = 0;
  const fullFileHitsByKind = {};

  function walkForPathLike(node, keyPath, depth) {
    if (depth > 6 || node === null || node === undefined) return;
    if (typeof node === 'string') {
      if (PATH_LIKE.test(node)) {
        const kp = keyPath.join('.') || '(root)';
        if (!pathLikeHits[kp]) pathLikeHits[kp] = { count: 0, samples: [] };
        pathLikeHits[kp].count++;
        if (pathLikeHits[kp].samples.length < 5 && !pathLikeHits[kp].samples.includes(node)) {
          pathLikeHits[kp].samples.push(node);
        }
      }
      return;
    }
    if (Array.isArray(node)) {
      // 配列は要素をたどるが、キーパスは膨らませない (件数だけ影響)
      for (let i = 0; i < Math.min(node.length, 20); i++) {
        walkForPathLike(node[i], keyPath.concat('[]'), depth + 1);
      }
      return;
    }
    if (typeof node === 'object') {
      for (const [k, v] of Object.entries(node)) {
        if (SECRET_KEYS.test(k)) {
          redactedKeysSeen.add(k);
          continue; // 値は絶対にたどらない・出さない (INV-2)
        }
        walkForPathLike(v, keyPath.concat(k), depth + 1);
      }
    }
  }

  function collectTopKeys(obj, keyPath, counter) {
    if (typeof obj !== 'object' || obj === null || Array.isArray(obj)) return;
    for (const k of Object.keys(obj)) {
      const kp = keyPath ? `${keyPath}.${k}` : k;
      counter[kp] = (counter[kp] || 0) + 1;
    }
  }

  /** タイムスタンプらしきフィールドを探す (キー名に date/time を含む数値 or ISO 文字列) */
  const TIME_KEY = /(date|time|timestamp|createdAt|updatedAt)/i;
  function collectTimestampFields(obj, keyPath, depth) {
    if (depth > 4 || typeof obj !== 'object' || obj === null) return;
    if (Array.isArray(obj)) {
      for (let i = 0; i < Math.min(obj.length, 5); i++) collectTimestampFields(obj[i], keyPath.concat('[]'), depth + 1);
      return;
    }
    for (const [k, v] of Object.entries(obj)) {
      if (SECRET_KEYS.test(k)) continue;
      if (TIME_KEY.test(k) && (typeof v === 'number' || typeof v === 'string')) {
        const kp = keyPath.concat(k).join('.');
        if (!timestampFieldCounts[kp]) timestampFieldCounts[kp] = { count: 0, types: {} };
        timestampFieldCounts[kp].count++;
        const t = typeof v;
        timestampFieldCounts[kp].types[t] = (timestampFieldCounts[kp].types[t] || 0) + 1;
      }
      if (typeof v === 'object') collectTimestampFields(v, keyPath.concat(k), depth + 1);
    }
  }

  for (const d of hashDirs) {
    const csDir = join(wsStorage, d.name, 'chatSessions');
    if (!existsSync(csDir)) continue;
    dirsWithChatSessions++;

    // 孤児化判定: workspace.json の folder が file:// スキームで、そのパスが今ディスクに存在するか
    const wj = join(wsStorage, d.name, 'workspace.json');
    if (existsSync(wj)) {
      try {
        const obj = JSON.parse(readFileSync(wj, 'utf8'));
        const uriStr = obj.folder ?? obj.workspace ?? null;
        if (typeof uriStr === 'string') {
          const fsPath = toFsPathFromFileUri(uriStr);
          if (fsPath && !existsSync(fsPath)) orphanCount++;
        }
      } catch {
        // 読めなければ孤児判定はスキップ (NFR-24)
      }
    }

    let entries;
    try {
      entries = readdirSync(csDir, { withFileTypes: true }).filter((e) => e.isFile());
    } catch {
      continue;
    }
    for (const e of entries) {
      const full = join(csDir, e.name);
      let st;
      try {
        st = statSync(full);
      } catch {
        continue;
      }
      fileCount++;
      totalBytes += st.size;
      if (st.size > maxBytes) {
        maxBytes = st.size;
        maxFile = full;
      }
      const ext = extname(e.name) || '(なし)';
      extCounts[ext] = (extCounts[ext] || 0) + 1;

      let txt;
      try {
        txt = readFileSync(full, 'utf8');
      } catch {
        continue;
      }
      const lines = txt.split('\n').filter((l) => l.trim().length > 0);
      let kind0 = null;
      let fileHasFullFilePathLike = false;
      for (const line of lines) {
        if (FULLFILE_PATH_RE.test(line)) {
          fileHasFullFilePathLike = true;
          let k = 'other';
          try {
            const o = JSON.parse(line);
            k = String(o.kind);
          } catch {
            // JSON として読めない行はスキップ (NFR-24)
          }
          fullFileHitsByKind[k] = (fullFileHitsByKind[k] || 0) + 1;
        }
        let obj;
        try {
          obj = JSON.parse(line);
        } catch {
          continue;
        }
        if (obj && obj.kind === 0 && obj.v && typeof obj.v === 'object' && !kind0) {
          kind0 = obj.v;
          // ★ break しない — 全行を FULLFILE_PATH_RE で見るため最後まで読む
        }
      }
      if (fileHasFullFilePathLike) fullFileFilesWithPathLike++;
      if (kind0) {
        kind0FileCount.withAtLeastOne++;
        collectTopKeys(kind0, '', kind0TopKeyCounts);
        walkForPathLike(kind0, [], 0);
        collectTimestampFields(kind0, [], 0);

        // mtime vs 内部タイムスタンプ (creationDate があれば ms として解釈)
        if (typeof kind0.creationDate === 'number') {
          const internal = new Date(kind0.creationDate);
          const diffMs = st.mtime.getTime() - kind0.creationDate;
          if (mtimeVsInternalSamples.length < 10) {
            mtimeVsInternalSamples.push({
              file: e.name,
              mtimeIso: st.mtime.toISOString(),
              internalIso: internal.toISOString(),
              diffMs,
            });
          }
        }
      }
    }
  }

  report.chatSessions = {
    dirsWithChatSessions,
    fileCount,
    totalBytes,
    maxBytes,
    maxFile,
    extCounts,
    kind0SnapshotFound: kind0FileCount.withAtLeastOne,
    kind0TopKeyOccurrenceRate: Object.fromEntries(
      Object.entries(kind0TopKeyCounts).map(([k, v]) => [
        k,
        `${v}/${kind0FileCount.withAtLeastOne} (${((v / (kind0FileCount.withAtLeastOne || 1)) * 100).toFixed(0)}%)`,
      ])
    ),
    pathLikeStringsByKeyPath: pathLikeHits,
    timestampFieldCounts,
    mtimeVsInternalTimestampSamples: mtimeVsInternalSamples,
    orphanCount, // workspaceStorage にあるが、フォルダが今ディスク上に存在しない件数
    redactedKeysSeen: [...redactedKeysSeen],
    fullFilePathLike: {
      note: 'kind0 (先頭スナップショット) だけでなく全行を正規表現で見た場合の参考値。FR-P-55 は先頭/末尾の部分読みのみを想定するため、この値は「全文パースすれば見つかる」ことの裏取りであり、実装方針そのものではない。',
      filesWithHit: fullFileFilesWithPathLike,
      totalFiles: fileCount,
      hitsByKind: fullFileHitsByKind,
    },
  };
  if (redactedKeysSeen.size > 0) {
    report.notes.push(
      `chatSessions 内に認証情報を含みうるキー名がありました: ${[...redactedKeysSeen].join(', ')}。値は出していません (INV-2)。`
    );
  }

  // ---------------------------------------------------------------
  // (c) <hash> 直下の他ファイル/ディレクトリの集計
  // ---------------------------------------------------------------
  const nameCounts = {};
  for (const d of hashDirs) {
    let entries;
    try {
      entries = readdirSync(join(wsStorage, d.name), { withFileTypes: true });
    } catch {
      continue;
    }
    for (const e of entries) {
      const label = e.isDirectory() ? `${e.name}/` : e.name;
      nameCounts[label] = (nameCounts[label] || 0) + 1;
    }
  }
  report.otherFilesUnderHash = { hashDirCount: hashDirs.length, nameCounts };
}

if (JSON_ONLY) {
  console.log(JSON.stringify(report, null, 2));
} else {
  console.log('=== VS Code workspaceStorage / chatSessions プローブ (読み取り専用) ===\n');
  console.log('workspaceStorage :', report.workspaceStorage, report.exists ? '' : '(存在しません)');

  if (report.hashRecalculation) {
    const h = report.hashRecalculation;
    console.log('\n--- (a) hash 再計算 ---');
    console.log('hash ディレクトリ数        :', h.hashDirCount);
    console.log('workspace.json あり        :', h.withWorkspaceJson);
    console.log('workspace.json なし        :', h.withoutWorkspaceJson);
    console.log('いずれかの候補で一致       :', h.matchedAtLeastOneCandidate);
    console.log('どの候補でも不一致         :', h.matchedNoCandidate);
    console.log('候補別一致件数 (candidate / algo -> 件数):');
    for (const [k, v] of Object.entries(h.perCandidateMatchCount)) {
      if (v > 0) console.log('  ', k, '=>', v);
    }
    console.log('  (0 件だった候補は省略。JSON 出力には全候補が入っています)');
    if (h.unmatchedSample.length) {
      console.log('不一致サンプル (scheme のみ):', JSON.stringify(h.unmatchedSample));
    }
  }

  if (report.chatSessions) {
    const c = report.chatSessions;
    console.log('\n--- (b) chatSessions の中身 ---');
    console.log('chatSessions を持つ hash 数:', c.dirsWithChatSessions);
    console.log('ファイル数                 :', c.fileCount, JSON.stringify(c.extCounts));
    console.log('合計バイト数               :', c.totalBytes);
    console.log('最大ファイル               :', c.maxBytes, c.maxFile);
    console.log('先頭スナップショット取得   :', c.kind0SnapshotFound, '/', c.fileCount);
    console.log('トップレベルキー出現率     :', JSON.stringify(c.kind0TopKeyOccurrenceRate, null, 2));
    console.log('パスらしき文字列のキーパス :', JSON.stringify(c.pathLikeStringsByKeyPath, null, 2));
    console.log('タイムスタンプらしきフィールド:', JSON.stringify(c.timestampFieldCounts, null, 2));
    console.log('mtime vs 内部タイムスタンプ サンプル:', JSON.stringify(c.mtimeVsInternalTimestampSamples, null, 2));
    console.log('孤児化件数 (フォルダが現存しない):', c.orphanCount);
    console.log(
      '参考: 全行走査でのパスらしき文字列 (kind 別):',
      c.fullFilePathLike.filesWithHit, '/', c.fullFilePathLike.totalFiles, 'ファイル',
      JSON.stringify(c.fullFilePathLike.hitsByKind)
    );
    if (c.redactedKeysSeen.length) console.log('伏せたキー名:', c.redactedKeysSeen.join(', '));
  }

  if (report.otherFilesUnderHash) {
    console.log('\n--- (c) <hash> 直下の項目集計 ---');
    console.log(JSON.stringify(report.otherFilesUnderHash.nameCounts, null, 2));
  }

  if (report.notes.length) {
    console.log('\n--- 注記 ---');
    for (const n of report.notes) console.log('*', n);
  }
}

mkdirSync(OUT_DIR, { recursive: true });
const outPath = join(OUT_DIR, 'vscode-sessions.json');
writeFileSync(outPath, JSON.stringify(report, null, 2), 'utf8');
if (!JSON_ONLY) {
  console.log('\n結果を書き出しました:', outPath, '(コミットしないこと)');
}
