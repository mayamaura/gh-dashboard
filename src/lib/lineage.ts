// 系統図・ガント・タイムラインの純粋ロジック (FR-C-112〜121)。
//
// ★ 系統図とガントは**同じ `SubagentNode[]` をそのまま使う** (FR-C-114)。
//   ここでは折りたたみ判定を自前で組まない — `visible` フラグはバックエンドが
//   計算済み。フロントは `visible === true` の行だけを描画対象にする。

import type { QuotaEventMark, SubagentNode, TurnMeta } from '../types/dto'

/** 描画対象の行 (折りたたみで隠れていない行) だけを表示順で返す。 */
export function visibleNodes(subagents: SubagentNode[]): SubagentNode[] {
  return subagents.filter((n) => n.visible)
}

/** 折りたたみで隠れている行数 (FR-C-115/116 の「隠しています」表示用)。 */
export function hiddenCollapsedCount(subagents: SubagentNode[]): number {
  return subagents.length - visibleNodes(subagents).length
}

/**
 * ライブ側で「稼働中のエージェントはありません」を出すべきか (FR-C-116)。
 * 振り返り側 (`isLive === false`) では常に false — 全件表示なので空表示にはならない。
 */
export function noRunningAgents(isLive: boolean, subagents: SubagentNode[]): boolean {
  return isLive && visibleNodes(subagents).length === 0
}

/** ノードの終了時刻。稼働中/未確定は `now` を使う (FR-C-118)。 */
export function nodeEndAt(node: SubagentNode, now: number): number {
  return node.ended_at ?? now
}

export interface TimeBounds {
  start: number
  end: number
}

/**
 * ガントの横軸 (FR-C-118)。
 *
 * `gantt` に値があればそれを使う。無ければ `subagents` の実測範囲から求める。
 * どちらも取れなければ `null` (描画するデータが無い)。
 */
export function ganttBounds(
  gantt: { start_at: number | null; end_at: number | null },
  subagents: SubagentNode[],
  now: number
): TimeBounds | null {
  let start = gantt.start_at
  let end = gantt.end_at ?? (start !== null ? now : null)

  if (start === null) {
    const starts = subagents.map((n) => n.started_at).filter((v): v is number => v !== null)
    if (starts.length === 0) return null
    start = Math.min(...starts)
    end = Math.max(...subagents.map((n) => nodeEndAt(n, now)))
  }
  if (end === null || end <= start) end = start + 1
  return { start, end }
}

/** 時刻 → 0〜1 の横軸位置。範囲外はクランプする。 */
export function ganttPosition(t: number, bounds: TimeBounds): number {
  const span = bounds.end - bounds.start
  if (span <= 0) return 0
  return Math.min(1, Math.max(0, (t - bounds.start) / span))
}

/** 利用枠到達マーカーのうち、表示範囲内にあるものだけ。0 件は「無かった」(FR-C-118) */
export function visibleQuotaMarks(marks: QuotaEventMark[], bounds: TimeBounds): QuotaEventMark[] {
  return marks.filter((m) => m.occurred_at >= bounds.start && m.occurred_at <= bounds.end)
}

/**
 * 系統図・ガント・タイムラインの選択/ホバー連動 (FR-C-121)。
 * `run_key` (=`agent_id`) が一致する行だけに絞り込む。`runKey` が `null` なら絞り込まない。
 */
export function filterTimelineByRunKey(timeline: TurnMeta[], runKey: string | null): TurnMeta[] {
  if (runKey === null) return timeline
  return timeline.filter((t) => t.agent_id === runKey)
}
