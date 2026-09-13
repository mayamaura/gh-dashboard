// プロジェクトタブ。
//
// ★ **定期ポーリングを持たない。**ユーザー操作駆動のみ (要求 4.2)。
// ★ 絞り込み・並べ替えはすべてフロントで完結させる (FR-P-81)。
// ★ 一覧 + 詳細パネルの 2 ペイン構成 (FR-P-80)。
// ★ スキャン中フラグ・エラー・選択キーは appStore に置く (FR-C-161)。

import { useEffect, useMemo, useRef, useState } from 'react'

import { appStore, pushNotice, useAppStore } from '../store/appStore'
import {
  defaultFilter,
  filterProjects,
  sortProjects,
  type ProjectFilter,
  type SortKey,
} from '../lib/projectList'
import { addScanFolder, openAgent, openFolder, openTerminal, openVscode, removeScanFolder, scanProjects } from '../lib/projectsActions'
import { useIsViewing } from '../hooks/useWindowVisible'
import { ProjectRow } from '../components/ProjectRow'
import { ProjectDetail } from '../components/ProjectDetail'
import { RowMenu, type RowMenuItem } from '../components/RowMenu'
import type { ProjectKind } from '../types/dto'

const KIND_OPTIONS: Array<{ value: ProjectKind; label: string }> = [
  { value: 'tauri', label: 'Tauri' },
  { value: 'nextjs', label: 'Next.js' },
  { value: 'sveltekit', label: 'SvelteKit' },
  { value: 'vite', label: 'Vite' },
  { value: 'python_package', label: 'Python パッケージ' },
  { value: 'rust', label: 'Rust' },
  { value: 'python', label: 'Python' },
  { value: 'notebook', label: 'Notebook' },
  { value: 'other', label: 'その他' },
]

export function ProjectsPage() {
  const viewing = useIsViewing('projects')
  const { projects, projectsSelectedKey } = useAppStore()
  const [filter, setFilter] = useState<ProjectFilter>(defaultFilter)
  const [sortKey, setSortKey] = useState<SortKey>('last_used')
  const [newFolderPath, setNewFolderPath] = useState('')
  const [menu, setMenu] = useState<{ pathKey: string; x: number; y: number } | null>(null)
  const now = Date.now()

  // タブを開いたときだけ。キャッシュ済みスナップショットを即描画し (appStore.projects
  // は既に反映済みなのでそのまま出る)、裏で最新化する (FR-P-86)。**定期ポーリングは張らない**
  useEffect(() => {
    if (viewing) void scanProjects()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [viewing])

  // FR-P-03 / NFR-43: 読めなかったフォルダ等の警告は無言で欠落させない。
  // ただしタブを行き来するたびに同じ内容を再通知しない (内容が変わったときだけ)
  const lastWarningKeyRef = useRef('')
  useEffect(() => {
    if (!projects) return
    const key = JSON.stringify([projects.warnings, projects.using_default_folder])
    if (key === lastWarningKeyRef.current) return
    lastWarningKeyRef.current = key
    for (const w of projects.warnings) pushNotice('warning', w)
    if (projects.using_default_folder) {
      pushNotice(
        'warning',
        'スキャン対象フォルダが未登録のため、既定フォルダを一時的に使っています (保存はしていません)。'
      )
    }
  }, [projects])

  const visible = useMemo(
    () => sortProjects(filterProjects(projects?.projects ?? [], filter), sortKey),
    [projects, filter, sortKey]
  )

  const selected = visible.find((p) => p.path_key === projectsSelectedKey) ?? null

  const toggleKind = (kind: ProjectKind) => {
    setFilter((f) => ({
      ...f,
      kinds: f.kinds.includes(kind) ? f.kinds.filter((k) => k !== kind) : [...f.kinds, kind],
    }))
  }

  const menuItems: RowMenuItem[] = menu
    ? (() => {
        const p = projects?.projects.find((x) => x.path_key === menu.pathKey)
        const items: RowMenuItem[] = [
          { label: 'VS Code で開く', onSelect: () => void openVscode(menu.pathKey) },
          { label: 'エクスプローラーで開く', onSelect: () => void openFolder(menu.pathKey) },
          { label: 'ターミナルで開く', onSelect: () => void openTerminal(menu.pathKey) },
          { label: 'Copilot CLI を起動', onSelect: () => void openAgent(menu.pathKey) },
        ]
        if (p && p.dev.state === 'running' && p.dev.url) {
          items.push({
            label: 'ブラウザで開く',
            onSelect: () =>
              pushNotice('error', '未実装です (T-3.x) — ブラウザで開く機能は段階 3 で実装します'),
          })
        }
        return items
      })()
    : []

  return (
    <div className="page projects-page">
      <section className="panel">
        <div className="toolbar">
          <input
            className="search"
            placeholder="名前で絞り込む"
            value={filter.text}
            onChange={(e) => setFilter({ ...filter, text: e.target.value })}
          />

          <details className="kind-filter">
            <summary>種別 {filter.kinds.length > 0 ? `(${filter.kinds.length})` : ''}</summary>
            <div className="kind-filter-body">
              {KIND_OPTIONS.map((opt) => (
                <label key={opt.value}>
                  <input
                    type="checkbox"
                    checked={filter.kinds.includes(opt.value)}
                    onChange={() => toggleKind(opt.value)}
                  />{' '}
                  {opt.label}
                </label>
              ))}
            </div>
          </details>

          <label>
            <input
              type="checkbox"
              checked={filter.showHidden}
              onChange={(e) => setFilter({ ...filter, showHidden: e.target.checked })}
            />{' '}
            非表示も表示
          </label>
          <label>
            <input
              type="checkbox"
              checked={filter.hideArchived}
              onChange={(e) => setFilter({ ...filter, hideArchived: e.target.checked })}
            />{' '}
            アーカイブを隠す
          </label>
          <label>
            <input
              type="checkbox"
              checked={filter.runningOnly}
              onChange={(e) => setFilter({ ...filter, runningOnly: e.target.checked })}
            />{' '}
            起動中のみ
          </label>
          <select value={sortKey} onChange={(e) => setSortKey(e.target.value as SortKey)}>
            <option value="last_used">最終利用日時</option>
            <option value="name">名前</option>
            <option value="kind">種別</option>
          </select>
        </div>

        <details className="scan-folders">
          <summary>スキャン対象フォルダ</summary>
          <ul className="scan-folder-list">
            {(projects?.scan_folders ?? []).map((f) => (
              <li key={f}>
                <span className="mono">{f}</span>
                <button onClick={() => void removeScanFolder(f)}>削除</button>
              </li>
            ))}
          </ul>
          <div className="scan-folder-add">
            <input
              className="search"
              placeholder="フォルダの絶対パス"
              value={newFolderPath}
              onChange={(e) => setNewFolderPath(e.target.value)}
            />
            <button
              disabled={newFolderPath.trim() === ''}
              onClick={() => {
                void addScanFolder(newFolderPath.trim())
                setNewFolderPath('')
              }}
            >
              追加
            </button>
          </div>
        </details>

      </section>

      <section className="panel list-panel">
        {visible.length === 0 ? (
          <p className="muted">
            {projects === null ? '読み込み中…' : '表示できるプロジェクトがありません。'}
          </p>
        ) : (
          <ul className="project-list">
            {visible.map((p) => (
              <ProjectRow
                key={p.path_key}
                project={p}
                now={now}
                selected={p.path_key === projectsSelectedKey}
                onSelect={() => appStore.set({ projectsSelectedKey: p.path_key })}
                onMenuOpen={(x, y) => setMenu({ pathKey: p.path_key, x, y })}
              />
            ))}
          </ul>
        )}
      </section>

      <ProjectDetail project={selected} now={now} />

      {menu && <RowMenu x={menu.x} y={menu.y} items={menuItems} onClose={() => setMenu(null)} />}
    </div>
  )
}
