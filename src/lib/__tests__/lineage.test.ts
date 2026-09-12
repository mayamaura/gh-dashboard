import { describe, expect, it } from 'vitest'

import {
  filterTimelineByRunKey,
  ganttBounds,
  ganttPosition,
  hiddenCollapsedCount,
  noRunningAgents,
  nodeEndAt,
  visibleNodes,
  visibleQuotaMarks,
} from '../lineage'
import type { SubagentNode, TurnMeta } from '../../types/dto'

function node(overrides: Partial<SubagentNode>): SubagentNode {
  return {
    run_key: 'a',
    parent_key: null,
    depth: 0,
    orphaned: false,
    child_keys: [],
    agent_id: 'a',
    agent_type: null,
    description: null,
    model: null,
    status: 'running',
    started_at: 0,
    last_activity_at: null,
    ended_at: null,
    tool_call_count: 0,
    running: false,
    visible: true,
    ...overrides,
  }
}

describe('visibleNodes / hiddenCollapsedCount', () => {
  it('visible=false の行を除外する', () => {
    const nodes = [node({ run_key: 'a', visible: true }), node({ run_key: 'b', visible: false })]
    expect(visibleNodes(nodes).map((n) => n.run_key)).toEqual(['a'])
    expect(hiddenCollapsedCount(nodes)).toBe(1)
  })

  it('全件表示なら隠れている行は 0', () => {
    const nodes = [node({ visible: true }), node({ visible: true })]
    expect(hiddenCollapsedCount(nodes)).toBe(0)
  })
})

describe('noRunningAgents', () => {
  it('ライブかつ visible が 0 件なら true (FR-C-116)', () => {
    expect(noRunningAgents(true, [node({ visible: false })])).toBe(true)
  })

  it('振り返り (is_live=false) では常に false (FR-C-117)', () => {
    expect(noRunningAgents(false, [])).toBe(false)
  })

  it('稼働中が 1 件以上あれば false', () => {
    expect(noRunningAgents(true, [node({ visible: true })])).toBe(false)
  })
})

describe('nodeEndAt', () => {
  it('ended_at が null なら now を使う (FR-C-118)', () => {
    expect(nodeEndAt(node({ ended_at: null }), 999)).toBe(999)
  })
  it('ended_at があればそれを使う', () => {
    expect(nodeEndAt(node({ ended_at: 500 }), 999)).toBe(500)
  })
})

describe('ganttBounds', () => {
  it('gantt に値があればそのまま使う', () => {
    expect(ganttBounds({ start_at: 100, end_at: 200 }, [], 999)).toEqual({ start: 100, end: 200 })
  })

  it('end_at が null (稼働中) なら now を使う', () => {
    expect(ganttBounds({ start_at: 100, end_at: null }, [], 999)).toEqual({ start: 100, end: 999 })
  })

  it('gantt が無ければ subagents の実測範囲から求める', () => {
    const nodes = [
      node({ started_at: 100, ended_at: 300 }),
      node({ started_at: 50, ended_at: null }),
    ]
    expect(ganttBounds({ start_at: null, end_at: null }, nodes, 999)).toEqual({
      start: 50,
      end: 999,
    })
  })

  it('データが無ければ null', () => {
    expect(ganttBounds({ start_at: null, end_at: null }, [], 999)).toBeNull()
  })
})

describe('ganttPosition', () => {
  it('範囲内は 0〜1 に正規化する', () => {
    expect(ganttPosition(150, { start: 100, end: 200 })).toBe(0.5)
  })
  it('範囲外はクランプする', () => {
    expect(ganttPosition(50, { start: 100, end: 200 })).toBe(0)
    expect(ganttPosition(250, { start: 100, end: 200 })).toBe(1)
  })
})

describe('visibleQuotaMarks', () => {
  it('範囲外のマーカーを除外する', () => {
    const marks = [
      { occurred_at: 50, kind: 'credit_exhausted', reset_text: null },
      { occurred_at: 150, kind: 'rate_limit', reset_text: null },
    ]
    expect(visibleQuotaMarks(marks, { start: 100, end: 200 })).toEqual([marks[1]])
  })

  it('0 件は「無かった」として空配列のまま', () => {
    expect(visibleQuotaMarks([], { start: 100, end: 200 })).toEqual([])
  })
})

describe('filterTimelineByRunKey', () => {
  const timeline: TurnMeta[] = [
    {
      turn_id: 1,
      timestamp_ms: null,
      record_type: null,
      role: null,
      model: null,
      agent_id: 'x',
      is_sidechain: true,
      preview: null,
      output_tokens: 0,
      byte_length: 0,
    },
    {
      turn_id: 2,
      timestamp_ms: null,
      record_type: null,
      role: null,
      model: null,
      agent_id: null,
      is_sidechain: false,
      preview: null,
      output_tokens: 0,
      byte_length: 0,
    },
  ]

  it('runKey が null なら絞り込まない', () => {
    expect(filterTimelineByRunKey(timeline, null)).toEqual(timeline)
  })

  it('agent_id が一致する行だけに絞り込む (FR-C-121)', () => {
    expect(filterTimelineByRunKey(timeline, 'x').map((t) => t.turn_id)).toEqual([1])
  })
})
