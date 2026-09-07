// プロジェクトタブ。
//
// ★ **定期ポーリングを持たない。**ユーザー操作駆動のみ (要求 4.2)。
// ★ 絞り込み・並べ替えはすべてフロントで完結させる (FR-P-81)。
//
// 実装状況: 一覧の骨格と絞り込みの配線だけ。詳細パネルと dev サーバー操作は
// 段階 1〜3 で実装する。

import { useEffect, useMemo, useState } from 'react'

import { projectsScan } from '../ipc/commands'
import { appStore, useAppStore } from '../store/appStore'
import {
  defaultFilter,
  filterProjects,
  launchState,
  sortProjects,
  type ProjectFilter,
  type SortKey,
} from '../lib/projectList'
import { matchedByLabel, relativeTime } from '../lib/format'
import { useIsViewing } from '../hooks/useWindowVisible'

export function ProjectsPage() {
  const viewing = useIsViewing('projects')
  const { projects } = useAppStore()
  const [filter, setFilter] = useState<ProjectFilter>(defaultFilter)
  const [sortKey, setSortKey] = useState<SortKey>('last_used')
  const [scanning, setScanning] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const now = Date.now()

  const scan = async () => {
    setScanning(true)
    setError(null)
    try {
      const snap = await projectsScan()
      appStore.set({ projects: snap })
    } catch (e) {
      setError(describeError(e))
    } finally {
      setScanning(false)
    }
  }

  // タブを開いたときだけ。**定期ポーリングは張らない**
  useEffect(() => {
    if (viewing) void scan()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [viewing])

  const visible = useMemo(
    () => sortProjects(filterProjects(projects?.projects ?? [], filter), sortKey),
    [projects, filter, sortKey]
  )

  return (
    <div className="page">
      <section className="panel">
        <div className="toolbar">
          <input
            className="search"
            placeholder="名前で絞り込む"
            value={filter.text}
            onChange={(e) => setFilter({ ...filter, text: e.target.value })}
          />
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

          <span className="spacer" />
          <span className="muted">
            最終スキャン: {relativeTime(projects?.scanned_at ?? null, now)}
          </span>
          <button onClick={() => void scan()} disabled={scanning}>
            {scanning ? 'スキャン中…' : '更新'}
          </button>
        </div>

        {/* FR-P-03 / NFR-43: 読めなかったフォルダを無言で欠落させない */}
        {projects?.warnings.map((w) => (
          <p key={w} className="note-inline">
            {w}
          </p>
        ))}
        {projects?.using_default_folder && (
          <p className="note-inline">
            スキャン対象フォルダが未登録のため、既定フォルダを一時的に使っています (保存はしていません)。
          </p>
        )}
        {error && <p className="note-inline error">{error}</p>}
      </section>

      <section className="panel">
        {visible.length === 0 ? (
          <p className="muted">
            {projects === null ? '読み込み中…' : '表示できるプロジェクトがありません。'}
          </p>
        ) : (
          <ul className="project-list">
            {visible.map((p) => {
              const launch = launchState(p)
              const fallback = p.copilot ? matchedByLabel(p.copilot.matched_by) : null
              return (
                <li key={p.path_key} className="project-row">
                  <span className={`dot dev-${p.dev.state}`} aria-hidden />
                  <span className="project-name">{p.display_name}</span>
                  {p.hidden && <span className="chip">非表示</span>}
                  {p.archived && <span className="chip">アーカイブ</span>}
                  <span className="chip">{p.kind_label}</span>
                  {p.git && (
                    <span className="muted">
                      {p.git.branch ?? 'detached'} {p.git.dirty && '●'}
                    </span>
                  )}
                  <span className="muted">
                    {relativeTime(p.copilot?.last_used_at ?? null, now)}
                  </span>
                  {/* FR-P-53 / NFR-41: 推測による紐付けを明示する */}
                  {fallback && <span className="chip warn">{fallback}</span>}
                  <span className="spacer" />
                  {/* FR-P-22: 起動不可でも隠さず、理由を出す */}
                  {launch.canLaunch ? (
                    <button disabled title="段階 3 (T-3.2) で実装">
                      起動
                    </button>
                  ) : (
                    <span className="muted" title={launch.reason ?? undefined}>
                      起動不可
                    </span>
                  )}
                </li>
              )
            })}
          </ul>
        )}
      </section>
    </div>
  )
}

/** AppError を人が読める文にする。**「何をすればよいか」があるなら添える** */
function describeError(e: unknown): string {
  if (typeof e === 'object' && e !== null && 'kind' in e) {
    const err = e as { kind: string; reason?: string; message?: string; how_to_fix?: string | null }
    const head = err.reason ?? err.message ?? err.kind
    return err.how_to_fix ? `${head} — ${err.how_to_fix}` : head
  }
  return String(e)
}
