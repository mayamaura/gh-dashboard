// 系統図 (T-7.6/7.8/7.9、FR-C-112〜117)。
//
// ★ 折りたたみ判定は自前で組まない。`SubagentNode.visible` を見るだけ
//   (バックエンドが祖先保護込みで計算済み)。
// ★ 系統図とガントは同じ `subagents` 配列・同じ表示順を使う (FR-C-114)。

import type { SubagentNode } from '../types/dto'
import { hiddenCollapsedCount, noRunningAgents, visibleNodes } from '../lib/lineage'
import { duration, relativeTime } from '../lib/format'

export function LineageGraph({
  subagents,
  isLive,
  now,
  selectedRunKey,
  hoveredRunKey,
  onSelect,
  onHover,
}: {
  subagents: SubagentNode[]
  isLive: boolean
  now: number
  selectedRunKey: string | null
  hoveredRunKey: string | null
  onSelect: (runKey: string | null) => void
  onHover: (runKey: string | null) => void
}) {
  if (subagents.length === 0) {
    return <p className="muted">サブエージェントの実行はありません。</p>
  }

  if (noRunningAgents(isLive, subagents)) {
    // FR-C-116: 折りたたみは維持し、断定せず「ありません」と明示する
    return <p className="muted">稼働中のエージェントはありません。</p>
  }

  const rows = visibleNodes(subagents)
  const hidden = hiddenCollapsedCount(subagents)

  return (
    <div className="lineage">
      {/* FR-C-115/116: 隠れている行の存在を無言にしない */}
      {isLive && hidden > 0 && (
        <p className="fineprint">完了済み {hidden} 件を隠しています。</p>
      )}
      <ul className="lineage-list">
        {rows.map((n) => {
          const selected = n.run_key === selectedRunKey
          const hovered = n.run_key === hoveredRunKey
          return (
            <li key={n.run_key} style={{ paddingLeft: n.depth * 18 }}>
              <button
                type="button"
                className={`lineage-node${selected ? ' selected' : ''}${hovered ? ' hovered' : ''}`}
                onClick={() => onSelect(selected ? null : n.run_key)}
                onMouseEnter={() => onHover(n.run_key)}
                onMouseLeave={() => onHover(null)}
              >
                <span className={`dot ${n.running ? 'subagent_running' : 'waiting_input'}`} aria-hidden />
                <span className="lineage-agent">{n.agent_type ?? n.agent_id ?? n.run_key}</span>
                {n.orphaned && (
                  // FR-C-113: 推測で紐付けた孤児は明示する
                  <span className="chip warn" title="親エージェントが見つからず、ルート直下に置いています">
                    親不明
                  </span>
                )}
                {n.description && <span className="muted lineage-desc">{n.description}</span>}
                <span className="faint">{n.model ?? '不明なモデル'}</span>
                <span className="faint">ツール {n.tool_call_count} 回</span>
                <span className="faint">稼働 {duration(n.started_at, n.ended_at ?? now)}</span>
                <span className="faint">最終活動 {relativeTime(n.last_activity_at, now)}</span>
              </button>
            </li>
          )
        })}
      </ul>
    </div>
  )
}
