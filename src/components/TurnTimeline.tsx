// 本文ビューア (T-7.10、FR-C-119/120)。
//
// ★ 行クリックでその 1 レコードだけをシーク読みする (`turnBodyGet`)。
//   タイムライン自体には本文を持たない (INV-6)。
// ★ 系統図・ガントの選択と連動して `agent_id` で絞り込む (FR-C-121)。

import { useState } from 'react'

import type { TurnBody, TurnMeta } from '../types/dto'
import { turnBodyGet } from '../ipc/commands'
import { absoluteTime } from '../lib/format'
import { filterTimelineByRunKey } from '../lib/lineage'

type BodyState = { status: 'loading' } | { status: 'done'; body: TurnBody } | { status: 'error'; message: string }

export function TurnTimeline({
  timeline,
  timelineTotal,
  timelineNextOffset,
  onLoadMore,
  loadingMore,
  selectedRunKey,
  onClearSelection,
}: {
  timeline: TurnMeta[]
  timelineTotal: number
  timelineNextOffset: number | null
  onLoadMore: () => void
  loadingMore: boolean
  selectedRunKey: string | null
  onClearSelection: () => void
}) {
  const [openTurnId, setOpenTurnId] = useState<number | null>(null)
  const [bodies, setBodies] = useState<Map<number, BodyState>>(new Map())

  const rows = filterTimelineByRunKey(timeline, selectedRunKey)

  const toggleRow = (turnId: number) => {
    if (openTurnId === turnId) {
      setOpenTurnId(null)
      return
    }
    setOpenTurnId(turnId)
    if (bodies.has(turnId)) return
    setBodies((prev) => new Map(prev).set(turnId, { status: 'loading' }))
    void turnBodyGet(turnId)
      .then((body) => {
        setBodies((prev) => new Map(prev).set(turnId, { status: 'done', body }))
      })
      .catch(() => {
        // FR-P-56 系: オフセットがファイル範囲外などは案内を出す
        setBodies((prev) =>
          new Map(prev).set(turnId, {
            status: 'error',
            message: '取得できませんでした — インデックスを再実行してください。',
          })
        )
      })
  }

  if (timeline.length === 0) {
    return <p className="muted">本文の記録がありません。</p>
  }

  return (
    <div className="turn-timeline">
      {selectedRunKey !== null && (
        <p className="fineprint">
          サブエージェント選択で絞り込み中 ({rows.length} / {timeline.length} 件)。{' '}
          <button className="link-button" onClick={onClearSelection}>
            絞り込み解除
          </button>
        </p>
      )}
      {rows.length === 0 ? (
        <p className="muted">この絞り込みに一致する行はありません。</p>
      ) : (
        <ul className="turn-list">
          {rows.map((t) => {
            const state = bodies.get(t.turn_id)
            const open = openTurnId === t.turn_id
            return (
              <li key={t.turn_id} className="turn-row">
                <button type="button" className="turn-row-main" onClick={() => toggleRow(t.turn_id)}>
                  <span className="faint">{absoluteTime(t.timestamp_ms)}</span>
                  <span className="chip">{t.record_type ?? '不明な種別'}</span>
                  <span className="muted">{t.role ?? '不明なロール'}</span>
                  <span className="muted">{t.model ?? '—'}</span>
                  <span className="session-title turn-preview">{t.preview ?? '(プレビューなし)'}</span>
                  <span className="faint">{open ? '▲' : '▼'}</span>
                </button>
                {open && (
                  <div className="turn-body">
                    {state === undefined || state.status === 'loading' ? (
                      <p className="muted">読み込み中…</p>
                    ) : state.status === 'error' ? (
                      <p className="note-inline error">{state.message}</p>
                    ) : (
                      <>
                        {state.body.truncated && (
                          <p className="note-inline">本文が長いため一部のみ表示しています。</p>
                        )}
                        <pre className="turn-body-pre">{state.body.body}</pre>
                      </>
                    )}
                  </div>
                )}
              </li>
            )
          })}
        </ul>
      )}
      <p className="fineprint">
        {/* FR-C-119: 上限 1000 件でキャップ済みの総件数 */}
        {timeline.length} / {timelineTotal} 件を表示しています。
      </p>
      {timelineNextOffset !== null && (
        <button onClick={onLoadMore} disabled={loadingMore}>
          {loadingMore ? '読み込み中…' : 'もっと見る'}
        </button>
      )}
    </div>
  )
}
