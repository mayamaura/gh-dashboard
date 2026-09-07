// gh-dashboard docs - shared sidebar navigation.
// ページを増やしたら NAV に 1 行足すだけでよいように、ナビはここに一本化する。

const NAV = [
  {
    title: '入口',
    items: [
      ['index.html', 'ドキュメント一覧'],
      ['agent-guide.html', 'AI エージェント作業ガイド'],
      ['glossary.html', '用語集'],
    ],
  },
  {
    title: '何を作るか',
    items: [
      ['requirements.html', '要求仕様書 v1.0'],
      ['open-questions.html', '未決事項 (OQ)'],
      ['ui-spec.html', 'UI 仕様'],
    ],
  },
  {
    title: 'どう作るか',
    items: [
      ['architecture.html', 'アーキテクチャ'],
      ['data-model.html', 'データモデル'],
      ['api-spec.html', 'IPC API 仕様'],
      ['decisions.html', '設計判断 (ADR)'],
      ['coding-standards.html', 'コーディング規約'],
    ],
  },
  {
    title: '進め方',
    items: [
      ['implementation-plan.html', '実装計画'],
      ['subagents.html', 'サブエージェント構成'],
      ['test-strategy.html', 'テスト戦略'],
      ['traceability.html', '要求トレーサビリティ'],
    ],
  },
];

function currentFile() {
  const path = window.location.pathname;
  const name = path.substring(path.lastIndexOf('/') + 1);
  return name === '' ? 'index.html' : name;
}

function renderSidebar() {
  const host = document.querySelector('[data-sidebar]');
  if (!host) return;
  const here = currentFile();

  const parts = [
    '<a class="brand" href="index.html">gh-dashboard</a>',
    '<div class="brand-sub">Copilot 稼働 / プロジェクト ダッシュボード</div>',
  ];

  for (const group of NAV) {
    parts.push('<nav class="nav-group">');
    parts.push('<h4>' + group.title + '</h4>');
    for (const [href, label] of group.items) {
      const cls = href === here ? ' class="current"' : '';
      parts.push('<a href="' + href + '"' + cls + '>' + label + '</a>');
    }
    parts.push('</nav>');
  }

  parts.push('<nav class="nav-group">');
  parts.push('<h4>リポジトリ</h4>');
  parts.push('<a href="../README.md">README.md</a>');
  parts.push('<a href="../CLAUDE.md">CLAUDE.md</a>');
  parts.push('<a href="source/">Markdown 原本 (docs/source/)</a>');
  parts.push('</nav>');

  host.innerHTML = parts.join('');
}

// 見出しにアンカーリンクを付ける (要求 ID を URL で共有できるようにするため)
function linkHeadings() {
  const heads = document.querySelectorAll('.main h2[id], .main h3[id]');
  for (const h of heads) {
    const a = document.createElement('a');
    a.href = '#' + h.id;
    a.textContent = '#';
    a.style.cssText = 'margin-left:.5em;color:var(--fg-faint);text-decoration:none;font-weight:400;opacity:0;transition:opacity .12s';
    h.appendChild(a);
    h.addEventListener('mouseenter', () => { a.style.opacity = '1'; });
    h.addEventListener('mouseleave', () => { a.style.opacity = '0'; });
  }
}

renderSidebar();
linkHeadings();
