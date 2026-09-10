#!/usr/bin/env node
// T-0.2 / OQ-02 — events.jsonl が「1 レコード = 1 行」で成立するかを実測する (読み取り専用プローブ)。
//
// ★ これは設計の分水嶺 (ADR-0003)。バイトオフセット方式が成立するかがここで決まる。
// ★ 読み取りしかしない (INV-1)。~/.copilot/** に一切書き込まない。
// ★ 認証情報らしきキーは値を出さない (INV-2)。ただしこのスクリプトは行の構造しか見ないため
//   通常は該当しない。万一トップレベルにそれらしいキーが出ても、キー名だけ報告する。
// ★ 合否判定はしない。数字だけ出す (NFR-52)。
//
//   node tools/probe/jsonl-shape.mjs
//   node tools/probe/jsonl-shape.mjs --json
//
// COPILOT_HOME で ~/.copilot の場所を上書きできる。

import { readdirSync, statSync, existsSync, mkdirSync, writeFileSync, openSync, readSync, closeSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { homedir } from 'node:os';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const OUT_DIR = join(ROOT, 'tools', 'probe-out');
const JSON_ONLY = process.argv.includes('--json');

const copilotHome = process.env.COPILOT_HOME || join(homedir(), '.copilot');
const stateDir = join(copilotHome, 'session-state');

const READ_BUF_SIZE = 256 * 1024; // 256KB 固定バッファでストリーム読みする。全体をメモリに載せない。

/** ディレクトリ配下の events.jsonl を再帰的に列挙する (存在しないものはスキップ)。 */
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
    if (e.isDirectory()) {
      found.push(...findJsonlFiles(full));
    } else if (e.name === 'events.jsonl') {
      found.push(full);
    }
  }
  return found;
}

/** 再帰的にオブジェクトを走査し、文字列値に含まれる生の改行 (\n) を持つキー名を集める。 */
function collectRawNewlineKeys(value, path, into) {
  if (typeof value === 'string') {
    if (value.includes('\n')) into.add(path || '(root)');
    return;
  }
  if (Array.isArray(value)) {
    for (const v of value) collectRawNewlineKeys(v, path, into);
    return;
  }
  if (value && typeof value === 'object') {
    for (const [k, v] of Object.entries(value)) {
      collectRawNewlineKeys(v, path ? `${path}.${k}` : k, into);
    }
  }
}

function percentile(sortedArr, p) {
  if (sortedArr.length === 0) return null;
  const idx = Math.min(sortedArr.length - 1, Math.floor((p / 100) * sortedArr.length));
  return sortedArr[idx];
}

/**
 * 1 ファイルを固定長バッファでストリーム読みし、行 (LF 区切り) を切り出す。
 * 各行について: バイト長、開始バイトオフセット、生バイト列を保持する (verify のため)。
 * 大きな行が続く可能性があるため、行バッファは都度破棄する。
 */
function scanFile(filePath) {
  const fd = openSync(filePath, 'r');
  const size = statSync(filePath).size;

  const result = {
    path: filePath,
    bytes: size,
    lineCount: 0, // \n の個数 (末尾が \n で終わらない断片は含めない=行として数えない)
    parsedRecordCount: 0,
    parseFailCount: 0,
    firstBytesHex: null,
    hasBom: false,
    crlfCount: 0,
    lfOnlyCount: 0,
    rawNewlineInStringLineCount: 0,
    rawNewlineKeys: new Set(),
    lineLengths: [], // バイト長 (改行を含まない、行の中身のみ)
    lastLineEndsWithNewline: true,
    offsetVerify: { checkedLines: 0, matched: 0, mismatched: 0 },
  };

  // 先頭 3 バイトを見て BOM を判定する
  {
    const head = Buffer.alloc(3);
    const n = readSync(fd, head, 0, 3, 0);
    result.firstBytesHex = head.slice(0, n).toString('hex').toUpperCase().replace(/(..)/g, '$1 ').trim();
    result.hasBom = n >= 3 && head[0] === 0xef && head[1] === 0xbb && head[2] === 0xbf;
  }

  const buf = Buffer.alloc(READ_BUF_SIZE);
  let filePos = 0; // 次に read する絶対オフセット
  let lineStartOffset = 0; // 現在組み立て中の行の開始バイトオフセット
  let pending = Buffer.alloc(0); // バッファ境界をまたいだ行の残り
  let pendingStartOffset = 0;
  let sawAnyByte = false;
  let lastByteWasNewline = true; // ファイルが空なら「断片なし」扱い

  while (true) {
    const bytesRead = readSync(fd, buf, 0, READ_BUF_SIZE, filePos);
    if (bytesRead === 0) break;
    sawAnyByte = true;
    const chunk = Buffer.concat([pending, buf.subarray(0, bytesRead)]);
    let searchStart = 0;
    let nlIdx;
    while ((nlIdx = chunk.indexOf(0x0a, searchStart)) !== -1) {
      // [searchStart, nlIdx) が 1 行の中身 (LF は含まない)。CRLF なら CR も除く。
      let lineEnd = nlIdx;
      let isCrlf = false;
      if (lineEnd > searchStart && chunk[lineEnd - 1] === 0x0d) {
        lineEnd -= 1;
        isCrlf = true;
      }
      const lineBuf = chunk.subarray(searchStart, lineEnd);
      const thisLineStartOffset = pendingStartOffset + searchStart;

      result.lineCount++;
      if (isCrlf) result.crlfCount++;
      else result.lfOnlyCount++;
      result.lineLengths.push(lineBuf.length);

      // JSON parse を試みる
      const text = lineBuf.toString('utf8');
      try {
        const obj = JSON.parse(text);
        result.parsedRecordCount++;
        const newlineKeys = new Set();
        collectRawNewlineKeys(obj, '', newlineKeys);
        if (newlineKeys.size > 0) {
          result.rawNewlineInStringLineCount++;
          for (const k of newlineKeys) result.rawNewlineKeys.add(k);
        }
      } catch {
        result.parseFailCount++;
      }

      // バイトオフセット往復検証: 記録した開始オフセットから、行の総バイト長 (改行含む) を
      // fs.read で読み直し、元の行 (改行含まない中身) と一致するか確認する。
      const totalLineBytesWithNewline = nlIdx + 1 - searchStart; // 中身 + (CR) + LF
      if (result.offsetVerify.checkedLines < 5000) {
        // 検証コストを抑えるため、先頭 5000 行まで検証する (全走査はファイルサイズに比例して重い)。
        const verifyBuf = Buffer.alloc(totalLineBytesWithNewline);
        const n = readSync(fd, verifyBuf, 0, totalLineBytesWithNewline, thisLineStartOffset);
        let verifyContent = verifyBuf.subarray(0, n);
        if (verifyContent.length > 0 && verifyContent[verifyContent.length - 1] === 0x0a) {
          verifyContent = verifyContent.subarray(0, verifyContent.length - 1);
        }
        if (verifyContent.length > 0 && verifyContent[verifyContent.length - 1] === 0x0d) {
          verifyContent = verifyContent.subarray(0, verifyContent.length - 1);
        }
        result.offsetVerify.checkedLines++;
        if (Buffer.compare(verifyContent, lineBuf) === 0) result.offsetVerify.matched++;
        else result.offsetVerify.mismatched++;
      }

      searchStart = nlIdx + 1;
      lastByteWasNewline = true;
    }
    pending = chunk.subarray(searchStart);
    pendingStartOffset = pendingStartOffset + searchStart;
    if (pending.length > 0) lastByteWasNewline = false;
    filePos += bytesRead;
  }

  result.lastLineEndsWithNewline = !sawAnyByte ? true : lastByteWasNewline;
  // 末尾の未確定断片 (改行なしで終わる最終行) はレコードとしてもオフセットとしても数えない (FR-C-05)。
  result.trailingFragmentBytes = pending.length;

  closeSync(fd);

  const sortedLens = [...result.lineLengths].sort((a, b) => a - b);
  const n = sortedLens.length;
  result.lineLengthStats = n === 0 ? null : {
    min: sortedLens[0],
    max: sortedLens[n - 1],
    mean: Math.round(sortedLens.reduce((a, b) => a + b, 0) / n),
    median: percentile(sortedLens, 50),
    p95: percentile(sortedLens, 95),
  };

  return {
    path: result.path,
    bytes: result.bytes,
    lineCount: result.lineCount,
    parsedRecordCount: result.parsedRecordCount,
    parseFailCount: result.parseFailCount,
    lineCountEqualsRecordCount: result.lineCount === (result.parsedRecordCount + result.parseFailCount),
    firstBytesHex: result.firstBytesHex,
    hasBom: result.hasBom,
    crlfCount: result.crlfCount,
    lfOnlyCount: result.lfOnlyCount,
    rawNewlineInStringLineCount: result.rawNewlineInStringLineCount,
    rawNewlineKeys: [...result.rawNewlineKeys].sort(),
    lineLengthStats: result.lineLengthStats,
    lastLineEndsWithNewline: result.lastLineEndsWithNewline,
    trailingFragmentBytes: result.trailingFragmentBytes,
    offsetVerify: result.offsetVerify,
  };
}

const report = {
  probedAt: new Date().toISOString(),
  copilotHome,
  stateDirExists: existsSync(stateDir),
  filesScanned: 0,
  totalBytes: 0,
  files: [],
  aggregate: null,
  notes: [],
};

if (!report.stateDirExists) {
  report.notes.push(
    'session-state/ が存在しません。対象データが 0 件のため、行数・レコード数などの数字は出せません。'
  );
} else {
  const files = findJsonlFiles(stateDir);
  report.filesScanned = files.length;
  if (files.length === 0) {
    report.notes.push('session-state/ は存在しますが events.jsonl が 0 件です。');
  }
  for (const f of files) {
    try {
      const r = scanFile(f);
      report.totalBytes += r.bytes;
      report.files.push(r);
    } catch (e) {
      report.notes.push(`読み取り失敗のためスキップ: ${f} (${e.message})`);
    }
  }

  if (report.files.length > 0) {
    const agg = {
      lineCount: 0,
      parsedRecordCount: 0,
      parseFailCount: 0,
      crlfCount: 0,
      lfOnlyCount: 0,
      rawNewlineInStringLineCount: 0,
      rawNewlineKeys: new Set(),
      offsetVerifyMatched: 0,
      offsetVerifyMismatched: 0,
      filesWithBom: 0,
      filesLastLineNoNewline: 0,
    };
    for (const r of report.files) {
      agg.lineCount += r.lineCount;
      agg.parsedRecordCount += r.parsedRecordCount;
      agg.parseFailCount += r.parseFailCount;
      agg.crlfCount += r.crlfCount;
      agg.lfOnlyCount += r.lfOnlyCount;
      agg.rawNewlineInStringLineCount += r.rawNewlineInStringLineCount;
      for (const k of r.rawNewlineKeys) agg.rawNewlineKeys.add(k);
      agg.offsetVerifyMatched += r.offsetVerify.matched;
      agg.offsetVerifyMismatched += r.offsetVerify.mismatched;
      if (r.hasBom) agg.filesWithBom++;
      if (!r.lastLineEndsWithNewline) agg.filesLastLineNoNewline++;
    }
    report.aggregate = {
      ...agg,
      rawNewlineKeys: [...agg.rawNewlineKeys].sort(),
      lineCountEqualsRecordCount: agg.lineCount === agg.parsedRecordCount + agg.parseFailCount,
    };
  }
}

if (JSON_ONLY) {
  console.log(JSON.stringify(report, null, 2));
} else {
  console.log('=== events.jsonl 形状プローブ (OQ-02 / ADR-0003 設計の分水嶺) ===\n');
  console.log('COPILOT_HOME       :', report.copilotHome);
  console.log('session-state/ 存在:', report.stateDirExists);
  console.log('走査ファイル数     :', report.filesScanned);
  console.log('合計バイト数       :', report.totalBytes);
  if (report.aggregate) {
    const a = report.aggregate;
    console.log('\n--- 集計 (全ファイル合算) ---');
    console.log('行数               :', a.lineCount);
    console.log('parse 成功レコード :', a.parsedRecordCount);
    console.log('parse 失敗         :', a.parseFailCount);
    console.log('行数 = レコード数+失敗:', a.lineCountEqualsRecordCount, `(${a.lineCount} 行 / ${a.parsedRecordCount + a.parseFailCount} 件)`);
    console.log('CRLF               :', a.crlfCount, '件');
    console.log('LF のみ             :', a.lfOnlyCount, '件');
    console.log('BOM ありファイル数 :', a.filesWithBom, '/', report.filesScanned);
    console.log('生改行を含む行     :', a.rawNewlineInStringLineCount, '件');
    console.log('該当キー           :', a.rawNewlineKeys.join(', ') || '(なし)');
    console.log('末尾が改行で終わらないファイル数:', a.filesLastLineNoNewline, '/', report.filesScanned);
    console.log('オフセット往復検証 :', `一致 ${a.offsetVerifyMatched} / 不一致 ${a.offsetVerifyMismatched}`);
  }
  console.log('\n--- ファイルごと ---');
  for (const f of report.files) {
    console.log(f.path);
    console.log(
      '  bytes=%d line=%d parsed=%d failed=%d bom=%s firstBytes=%s crlf=%d lf=%d rawNL=%d lastLineNL=%s trailFrag=%d',
      f.bytes, f.lineCount, f.parsedRecordCount, f.parseFailCount, f.hasBom, f.firstBytesHex,
      f.crlfCount, f.lfOnlyCount, f.rawNewlineInStringLineCount, f.lastLineEndsWithNewline, f.trailingFragmentBytes
    );
    if (f.lineLengthStats) {
      console.log('  行長: min=%d median=%d mean=%d p95=%d max=%d',
        f.lineLengthStats.min, f.lineLengthStats.median, f.lineLengthStats.mean, f.lineLengthStats.p95, f.lineLengthStats.max);
    }
    console.log('  offsetVerify: checked=%d matched=%d mismatched=%d',
      f.offsetVerify.checkedLines, f.offsetVerify.matched, f.offsetVerify.mismatched);
  }
  if (report.notes.length) {
    console.log('\n--- 注記 ---');
    for (const n of report.notes) console.log('*', n);
  }
}

mkdirSync(OUT_DIR, { recursive: true });
const outPath = join(OUT_DIR, 'jsonl-shape.json');
writeFileSync(outPath, JSON.stringify(report, null, 2), 'utf8');
if (!JSON_ONLY) {
  console.log('\n結果を書き出しました:', outPath, '(コミットしないこと)');
}
