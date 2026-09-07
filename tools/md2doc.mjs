#!/usr/bin/env node
// docs/source/*.md -> docs/*.html
//
// ドキュメント一式は Markdown を原本として書き、この変換器で HTML を生成する。
// 手書きの HTML を十数枚維持するのは現実的でないため、原本 1 本 + 生成にした (ADR-0008)。
//
//   node tools/md2doc.mjs          全ページを再生成
//   node tools/md2doc.mjs --check  生成物が最新かだけを検査する (CI 用、書き込まない)
//
// 対応記法: 見出し / GFM テーブル / 箇条書き / 番号付き / 引用 / 水平線 /
//           フェンスコード / 段落 / インライン (code, **bold**, *em*, link)
// 対応しない記法は「原本で使わない」で運用する。

import { readFileSync, writeFileSync, readdirSync, existsSync } from 'node:fs';
import { join, dirname, basename } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const SRC_DIR = join(ROOT, 'docs', 'source');
const OUT_DIR = join(ROOT, 'docs');

const CHECK_ONLY = process.argv.includes('--check');

// ---------------------------------------------------------------- inline

const escapeHtml = (s) =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

/** 要求 ID (FR-P-01 / NFR-23 / OQ-05 / ADR-0003 / T-4.2 など) */
const REQ_ID =
  /^(?:FR-[PC]-\d+|DR-\d+|IR-\d+|NFR-\d+|OQ-\d+|ADR-\d{4}|INV-\d+|S-\d+|N-\d+|T-\d+(?:\.\d+)?)$/;

const PRIORITY_CLASS = { 必須: 'must', 推奨: 'should', 任意: 'may' };

function inline(src) {
  // コードスパンを先に退避する (中の記号を装飾解釈させないため)。
  // 目印は本文に現れない ASCII トークンにする。
  const spans = [];
  let s = src.replace(/`([^`]+)`/g, (_, code) => {
    spans.push('<code>' + escapeHtml(code) + '</code>');
    return '@@__CODE' + (spans.length - 1) + '__@@';
  });

  s = escapeHtml(s);
  s = s.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (_, text, href) => {
    return '<a href="' + href + '">' + text + '</a>';
  });
  s = s.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  s = s.replace(/(^|[^\w*])\*([^*\n]+)\*(?![\w*])/g, '$1<em>$2</em>');
  s = s.replace(/@@__CODE(\d+)__@@/g, (_, i) => spans[Number(i)]);
  return s;
}

/** テーブルセル: 要求 ID と優先度をチップに変換する */
function cell(raw, isHeader) {
  const text = raw.trim();
  if (!isHeader) {
    const bare = text.replace(/^\*\*/, '').replace(/\*\*$/, '').trim();
    if (REQ_ID.test(bare)) {
      return '<span class="chip id">' + escapeHtml(bare) + '</span>';
    }
    if (PRIORITY_CLASS[bare]) {
      return '<span class="chip ' + PRIORITY_CLASS[bare] + '">' + bare + '</span>';
    }
  }
  return inline(text);
}

function splitRow(line) {
  const trimmed = line.trim().replace(/^\|/, '').replace(/\|$/, '');
  const out = [];
  let cur = '';
  let escaped = false;
  for (const ch of trimmed) {
    if (escaped) {
      cur += ch;
      escaped = false;
      continue;
    }
    if (ch === '\\') {
      escaped = true;
      continue;
    }
    if (ch === '|') {
      out.push(cur);
      cur = '';
      continue;
    }
    cur += ch;
  }
  out.push(cur);
  return out;
}

const isTableDelimiter = (line) =>
  /^\s*\|?[\s:|-]+\|[\s:|-]*$/.test(line) && line.includes('-');

// ---------------------------------------------------------------- slugs

function slugify(text, used) {
  let base = text
    .replace(/<[^>]+>/g, '')
    .replace(/[`*]/g, '')
    .trim()
    .toLowerCase()
    .replace(/[^\w぀-ヿ一-鿿.-]+/g, '-')
    .replace(/^-+/, '')
    .replace(/-+$/, '');
  if (!base) base = 'section';
  let slug = base;
  let n = 2;
  while (used.has(slug)) slug = base + '-' + n++;
  used.add(slug);
  return slug;
}

// ---------------------------------------------------------------- blocks

function renderBlocks(lines, used) {
  const out = [];
  let i = 0;

  const flushList = (ordered) => {
    const tag = ordered ? 'ol' : 'ul';
    const items = [];
    const marker = ordered ? /^(\s*)\d+[.)]\s+(.*)$/ : /^(\s*)[-*+]\s+(.*)$/;
    while (i < lines.length) {
      const m = lines[i].match(marker);
      if (!m) {
        // インデントされた継続行は直前の項目に連結する
        if (items.length && /^\s{2,}\S/.test(lines[i])) {
          items[items.length - 1] += ' ' + lines[i].trim();
          i++;
          continue;
        }
        break;
      }
      items.push(m[2]);
      i++;
    }
    out.push('<' + tag + '>');
    for (const it of items) out.push('<li>' + inline(it) + '</li>');
    out.push('</' + tag + '>');
  };

  while (i < lines.length) {
    const line = lines[i];

    if (!line.trim()) {
      i++;
      continue;
    }

    // フェンスコード
    if (/^```/.test(line)) {
      const lang = line.slice(3).trim();
      const body = [];
      i++;
      while (i < lines.length && !/^```/.test(lines[i])) body.push(lines[i++]);
      i++; // 閉じフェンス
      // 罫線素片を含むブロックは「図」として扱う
      const looksLikeDiagram = body.some((l) => /[─-╿]/.test(l));
      if (looksLikeDiagram && !lang) {
        out.push('<div class="ascii">' + escapeHtml(body.join('\n')) + '</div>');
      } else {
        const cls = lang ? ' class="language-' + lang + '"' : '';
        out.push('<pre><code' + cls + '>' + escapeHtml(body.join('\n')) + '</code></pre>');
      }
      continue;
    }

    // 見出し
    const h = line.match(/^(#{1,6})\s+(.*)$/);
    if (h) {
      const level = h[1].length;
      const text = h[2].trim();
      if (level === 1) {
        out.push('<h1>' + inline(text) + '</h1>');
      } else {
        const id = slugify(text, used);
        out.push('<h' + level + ' id="' + id + '">' + inline(text) + '</h' + level + '>');
      }
      i++;
      continue;
    }

    // 水平線
    if (/^\s*(-{3,}|\*{3,}|_{3,})\s*$/.test(line)) {
      out.push('<hr />');
      i++;
      continue;
    }

    // テーブル
    if (line.includes('|') && i + 1 < lines.length && isTableDelimiter(lines[i + 1])) {
      const header = splitRow(line);
      i += 2;
      const rows = [];
      while (i < lines.length && lines[i].includes('|') && lines[i].trim()) {
        rows.push(splitRow(lines[i]));
        i++;
      }
      out.push('<div class="table-wrap"><table>');
      out.push(
        '<thead><tr>' + header.map((c) => '<th>' + cell(c, true) + '</th>').join('') + '</tr></thead>'
      );
      out.push('<tbody>');
      for (const r of rows) {
        const tds = [];
        for (let c = 0; c < header.length; c++) {
          tds.push('<td>' + cell(r[c] === undefined ? '' : r[c], false) + '</td>');
        }
        out.push('<tr>' + tds.join('') + '</tr>');
      }
      out.push('</tbody></table></div>');
      continue;
    }

    // 引用 -> コールアウト
    //   > [!注意] ... のように 1 行目に印を書くと種類を明示できる。
    //   印が無いときは本文のキーワードから推測する (原本を書き換えずに済ませるため)。
    if (/^>\s?/.test(line)) {
      const body = [];
      while (i < lines.length && /^>/.test(lines[i])) {
        body.push(lines[i].replace(/^>\s?/, ''));
        i++;
      }
      let joined = body.join('\n');
      let cls = 'rationale';
      let title = '補足';

      const marker = joined.match(/^\s*\[!([^\]]+)\]\s*/);
      if (marker) {
        joined = joined.slice(marker[0].length);
        title = marker[1].trim();
        if (/注意|警告|落とし穴/.test(title)) cls = 'warning';
        else if (/理由|根拠|なぜ/.test(title)) cls = 'rationale';
        else cls = 'note';
      } else if (/が要る理由|なぜ/.test(joined)) {
        cls = 'rationale';
        title = 'なぜこの要求があるか';
      } else if (/壊れる|事故|注意|できない|してはいけない|重大/.test(joined)) {
        cls = 'warning';
        title = '注意';
      }

      out.push('<div class="' + cls + '"><span class="note-title">' + title + '</span>');
      out.push(renderBlocks(joined.split('\n'), used).join('\n'));
      out.push('</div>');
      continue;
    }

    // リスト
    if (/^\s*[-*+]\s+/.test(line)) {
      flushList(false);
      continue;
    }
    if (/^\s*\d+[.)]\s+/.test(line)) {
      flushList(true);
      continue;
    }

    // 段落
    const para = [];
    while (
      i < lines.length &&
      lines[i].trim() &&
      !/^(#{1,6}\s|>|```|\s*[-*+]\s|\s*\d+[.)]\s)/.test(lines[i]) &&
      !(lines[i].includes('|') && i + 1 < lines.length && isTableDelimiter(lines[i + 1]))
    ) {
      para.push(lines[i]);
      i++;
    }
    if (para.length) out.push('<p>' + inline(para.join(' ')) + '</p>');
    else i++;
  }

  return out;
}

// ---------------------------------------------------------------- page

function parseFrontMatter(text) {
  const m = text.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?/);
  if (!m) return { meta: {}, body: text };
  const meta = {};
  for (const line of m[1].split(/\r?\n/)) {
    const kv = line.match(/^([\w-]+):\s*(.*)$/);
    if (kv) meta[kv[1]] = kv[2].trim();
  }
  return { meta, body: text.slice(m[0].length) };
}

function page(meta, contentHtml) {
  const title = meta.title || 'ドキュメント';
  const bits = [];
  if (meta.status) bits.push('<span>' + escapeHtml(meta.status) + '</span>');
  if (meta.version) bits.push('<span>版 ' + escapeHtml(meta.version) + '</span>');
  if (meta.updated) bits.push('<span>更新 ' + escapeHtml(meta.updated) + '</span>');
  bits.push('<span>原本 <code>docs/source/' + escapeHtml(meta.sourceFile) + '</code></span>');

  const head =
    meta.lede === undefined
      ? ''
      : '<h1>' + escapeHtml(title) + '</h1>\n<p class="lede">' + inline(meta.lede) + '</p>\n';

  return (
    '<!doctype html>\n' +
    '<html lang="ja">\n' +
    '<head>\n' +
    '<meta charset="utf-8" />\n' +
    '<meta name="viewport" content="width=device-width, initial-scale=1" />\n' +
    '<title>' + escapeHtml(title) + ' | gh-dashboard docs</title>\n' +
    '<link rel="stylesheet" href="assets/docs.css" />\n' +
    '</head>\n' +
    '<body>\n' +
    '<div class="layout">\n' +
    '  <aside class="sidebar" data-sidebar></aside>\n' +
    '  <main class="main">\n' +
    head +
    '<div class="doc-meta">' + bits.join('') + '</div>\n' +
    contentHtml +
    '\n    <div class="footer-nav">\n' +
    '      <span><a href="index.html">&larr; ドキュメント一覧</a></span>\n' +
    '      <span>gh-dashboard</span>\n' +
    '    </div>\n' +
    '  </main>\n' +
    '</div>\n' +
    '<script src="assets/docs.js"></script>\n' +
    '</body>\n' +
    '</html>\n'
  );
}

// ---------------------------------------------------------------- main

function convert(file) {
  const raw = readFileSync(join(SRC_DIR, file), 'utf8').replace(/\r\n/g, '\n');
  const { meta, body } = parseFrontMatter(raw);
  meta.sourceFile = file;
  const lines = body.split('\n');
  // front matter で見出しを出すときは、本文先頭の H1 を落として二重見出しを避ける
  if (meta.title && meta.lede !== undefined) {
    while (lines.length && !lines[0].trim()) lines.shift();
    if (lines.length && /^#\s+/.test(lines[0])) lines.shift();
  }
  const used = new Set();
  return page(meta, renderBlocks(lines, used).join('\n'));
}

if (!existsSync(SRC_DIR)) {
  console.error('docs/source が見つかりません: ' + SRC_DIR);
  process.exit(1);
}

const files = readdirSync(SRC_DIR)
  .filter((f) => f.endsWith('.md'))
  .sort();
let stale = 0;

for (const file of files) {
  const html = convert(file);
  const outPath = join(OUT_DIR, basename(file, '.md') + '.html');
  const prev = existsSync(outPath) ? readFileSync(outPath, 'utf8') : null;
  if (CHECK_ONLY) {
    if (prev !== html) {
      stale++;
      console.error('stale: docs/' + basename(outPath));
    }
  } else if (prev !== html) {
    writeFileSync(outPath, html, 'utf8');
    console.log('generated: docs/' + basename(outPath));
  }
}

if (CHECK_ONLY) {
  if (stale) {
    console.error('\n' + stale + ' 件の HTML が原本と一致しません。npm run docs:build を実行してください。');
    process.exit(1);
  }
  console.log('docs: 全 ' + files.length + ' ページが最新です');
} else {
  console.log('docs: ' + files.length + ' ページを処理しました');
}
