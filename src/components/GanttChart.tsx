// ガントチャート (T-7.7、FR-C-118)。
//
// ★ 外部チャートライブラリを追加せず自前 SVG で描く (ADR-0005 / FR-C-163)。
// ★ 系統図と同じ `subagents` 配列・同じ表示順を使う (FR-C-114)。
// ★ アニメーションは無し。データに応じた静的なレイアウトなので
//   transform/opacity/stroke-dashoffset/filter の制約 (FR-C-163) の対象外。

import type { GanttWindow, QuotaEventMark, SubagentNode } from '../types/dto'
import { ganttBounds, ganttPosition, nodeEndAt, visibleNodes, visibleQuotaMarks } from '../lib/lineage'
import { absoluteTime } from '../lib/format'

const ROW_HEIGHT = 22
const VIEW_WIDTH = 640

export function GanttChart({
  subagents,
  gantt,
  quotaEvents,
  now,
  selectedRunKey,
  hoveredRunKey,
  onSelect,
  onHover,
}: {
  subagents: SubagentNode[]
  gantt: GanttWindow
  quotaEvents: QuotaEventMark[]
  now: number
  selectedRunKey: string | null
  hoveredRunKey: string | null
  onSelect: (runKey: string | null) => void
  onHover: (runKey: string | null) => void
}) {
  const rows = visibleNodes(subagents)
  const bounds = ganttBounds(gantt, subagents, now)

  if (bounds === null || rows.length === 0) {
    return <p className="muted">表示できる稼働期間のデータがありません。</p>
  }

  const marks = visibleQuotaMarks(quotaEvents, bounds)
  const height = rows.length * ROW_HEIGHT + 4

  return (
    <div className="gantt">
      <svg
        viewBox={`0 0 ${VIEW_WIDTH} ${height}`}
        width="100%"
        height={height}
        role="img"
        aria-label="サブエージェントの稼働ガントチャート"
      >
        {rows.map((n, i) => {
          const x1 = ganttPosition(n.started_at ?? bounds.start, bounds) * VIEW_WIDTH
          const x2 = ganttPosition(nodeEndAt(n, now), bounds) * VIEW_WIDTH
          const y = i * ROW_HEIGHT + 3
          const selected = n.run_key === selectedRunKey
          const hovered = n.run_key === hoveredRunKey
          return (
            <rect
              key={n.run_key}
              x={x1}
              y={y}
              width={Math.max(2, x2 - x1)}
              height={ROW_HEIGHT - 6}
              rx={3}
              className={`gantt-bar${selected ? ' selected' : ''}${hovered ? ' hovered' : ''}`}
              onClick={() => onSelect(selected ? null : n.run_key)}
              onMouseEnter={() => onHover(n.run_key)}
              onMouseLeave={() => onHover(null)}
            >
              <title>
                {(n.agent_type ?? n.agent_id ?? n.run_key) +
                  ' — ' +
                  absoluteTime(n.started_at) +
                  ' 〜 ' +
                  (n.ended_at === null ? '稼働中' : absoluteTime(n.ended_at))}
              </title>
            </rect>
          )
        })}
        {/* FR-C-118: 利用枠到達イベントを縦マーカーで重畳。0 件なら何も描かない */}
        {marks.map((m, i) => {
          const x = ganttPosition(m.occurred_at, bounds) * VIEW_WIDTH
          return (
            <line
              key={i}
              x1={x}
              x2={x}
              y1={0}
              y2={height}
              className="gantt-quota-mark"
              strokeDasharray="3,3"
            >
              <title>
                {m.kind}
                {m.reset_text ? ` — ${m.reset_text}` : ''} ({absoluteTime(m.occurred_at)})
              </title>
            </line>
          )
        })}
      </svg>
      <div className="gantt-axis">
        <span className="faint">{absoluteTime(bounds.start)}</span>
        <span className="faint">{absoluteTime(bounds.end)}</span>
      </div>
    </div>
  )
}
