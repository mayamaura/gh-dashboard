// プロジェクト一覧の 1 行 (FR-P-83)。
//
// 出す順序: dev 状態ドット / Copilot 稼働中バッジ / 表示名 (+ 非表示・アーカイブ
// バッジ) / git ブランチと未コミット変更ドット / 種別 / 最終利用日時 (相対) /
// 稼働中の URL または起動コマンド / 起動・停止ボタン / 外部ツールメニュー。

import type { Project } from '../types/dto'
import { launchState } from '../lib/projectList'
import { matchedByLabel, relativeTime } from '../lib/format'

export function ProjectRow({
  project: p,
  selected,
  now,
  onSelect,
  onMenuOpen,
}: {
  project: Project
  selected: boolean
  now: number
  onSelect: () => void
  onMenuOpen: (x: number, y: number) => void
}) {
  const launch = launchState(p)
  const fallback = p.copilot ? matchedByLabel(p.copilot.matched_by) : null

  // 稼働中の URL があればそれを、無ければ解決済みコマンドを出す (FR-P-83)。
  // どちらも無ければこの列は空にする — 「起動不可」はボタン列が出すので、
  // ここにも出すと同じ文言が 2 回並ぶ。理由はボタン側の title に出す (FR-P-22)。
  const runningUrl = p.dev.state === 'running' ? p.dev.url : null
  const commandOrUrl = runningUrl ?? p.resolved_command

  return (
    <li
      className={selected ? 'project-row selected' : 'project-row'}
      onClick={onSelect}
      role="button"
      tabIndex={0}
    >
      <span className={`dot dev-${p.dev.state}`} aria-hidden />
      {/* FR-P-83: Copilot 稼働中バッジ */}
      {p.copilot?.is_active && <span className="badge">稼働中</span>}
      <span className="project-name">{p.display_name}</span>
      {p.hidden && <span className="chip">非表示</span>}
      {p.archived && <span className="chip">アーカイブ</span>}
      {p.git && (
        <span className="muted">
          {p.git.branch ?? 'detached HEAD'} {p.git.dirty && <span className="dot dirty" aria-hidden />}
        </span>
      )}
      <span className="chip">{p.kind_label}</span>
      <span className="muted">{relativeTime(p.copilot?.last_used_at ?? null, now)}</span>
      {/* FR-P-53 / NFR-41: 推測による紐付けを明示する */}
      {fallback && <span className="chip warn">{fallback}</span>}
      <span className="spacer" />
      {commandOrUrl !== null && (
        <span className="muted project-command" title={commandOrUrl}>
          {commandOrUrl}
        </span>
      )}

      {/* 起動・停止は段階 3 (T-3.2) まで実装しない */}
      {launch.canLaunch ? (
        <button disabled title="段階 3 (T-3.2) で実装" onClick={(e) => e.stopPropagation()}>
          起動
        </button>
      ) : (
        <span className="muted" title={launch.reason ?? undefined}>
          起動不可
        </span>
      )}

      <button
        className="row-menu-trigger"
        title="外部ツール"
        onClick={(e) => {
          e.stopPropagation()
          const rect = (e.target as HTMLElement).getBoundingClientRect()
          onMenuOpen(rect.left, rect.bottom)
        }}
      >
        ⋮
      </button>
    </li>
  )
}
