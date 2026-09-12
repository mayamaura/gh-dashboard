// セッション詳細パネル (T-7.6〜7.11)。
//
// ★ 選択中の session_id は appStore に置く (T-7.13 / FR-C-161)。ここでは
//   フェッチしてきた詳細データ・タイムライン蓄積・選択/ホバーは、パネルの
//   表示に閉じたエフェメラルな UI 状態なのでローカル state で構わない —
//   長時間処理の進行状態ではなく、選択が変われば即座に取り直せば足りる。

import { useEffect, useRef, useState } from 'react'

import type { SessionDetail } from '../types/dto'
import { indexRefresh, sessionDetailGet } from '../ipc/commands'
import { LineageGraph } from './LineageGraph'
import { GanttChart } from './GanttChart'
import { TurnTimeline } from './TurnTimeline'
import { duration, relativeTime } from '../lib/format'

export function SessionDetailPanel({
  sessionId,
  onClose,
}: {
  sessionId: string
  onClose: () => void
}) {
  const [detail, setDetail] = useState<SessionDetail | null | 'loading'>('loading')
  const [loadingMore, setLoadingMore] = useState(false)
  const [selectedRunKey, setSelectedRunKey] = useState<string | null>(null)
  const [hoveredRunKey, setHoveredRunKey] = useState<string | null>(null)
  // FR-C-57 系: 未インデックスは 1 回だけ indexRefresh を試みる。抑止が無いと
  // 「取得→未インデックス→再取得」のループになる
  const attemptedRef = useRef(false)

  useEffect(() => {
    setDetail('loading')
    setSelectedRunKey(null)
    setHoveredRunKey(null)
    attemptedRef.current = false
    let cancelled = false
    void sessionDetailGet(sessionId).then((d) => {
      if (!cancelled) setDetail(d)
    })
    return () => {
      cancelled = true
    }
  }, [sessionId])

  useEffect(() => {
    if (detail !== null || attemptedRef.current) return
    attemptedRef.current = true
    void indexRefresh().then(() => sessionDetailGet(sessionId)).then((d) => setDetail(d))
  }, [detail, sessionId])

  const loadMore = async () => {
    if (detail === 'loading' || detail === null || detail.timeline_next_offset === null) return
    setLoadingMore(true)
    try {
      const next = await sessionDetailGet(sessionId, detail.timeline_next_offset)
      if (next) {
        setDetail({ ...next, timeline: [...detail.timeline, ...next.timeline] })
      }
    } finally {
      setLoadingMore(false)
    }
  }

  const now = Date.now()

  return (
    <section className="panel detail-panel">
      <div className="panel-head">
        <h2>セッション詳細</h2>
        <button onClick={onClose}>閉じる</button>
      </div>

      {detail === 'loading' ? (
        <p className="muted">読み込み中…</p>
      ) : detail === null ? (
        // FR-C-57: エラーではなく穏やかな文言
        <p className="muted">まだ記録がありません。</p>
      ) : (
        <>
          <dl className="detail-grid">
            <dt>フォルダ</dt>
            <dd>{detail.session.folder_name ?? '不明なフォルダ'}</dd>
            <dt>タイトル</dt>
            <dd>{detail.session.title ?? '(タイトルなし)'}</dd>
            <dt>稼働時間</dt>
            <dd>{duration(detail.session.started_at, detail.session.last_activity_at ?? now)}</dd>
            <dt>最終活動</dt>
            <dd>{relativeTime(detail.session.last_activity_at, now)}</dd>
            {/* FR-C-89: 金額は概算であることを添える。ここは AI Credits 単位でドル換算はしない */}
            <dt>消費クレジット</dt>
            <dd>{detail.credits.toFixed(3)} クレジット</dd>
          </dl>

          <section className="detail-section">
            <h3>モデル別内訳</h3>
            {detail.models.length === 0 ? (
              // NFR-43: 空配列 = 取れなかった。0 消費と見せない
              <p className="muted">モデル別の内訳は取得できませんでした。</p>
            ) : (
              <table className="model-table">
                <thead>
                  <tr>
                    <th>モデル</th>
                    <th>入力</th>
                    <th>出力</th>
                    <th>キャッシュ読込</th>
                    <th>キャッシュ書込</th>
                    <th>クレジット</th>
                    <th>件数</th>
                  </tr>
                </thead>
                <tbody>
                  {detail.models.map((m) => (
                    <tr key={m.model}>
                      <td>{m.model}</td>
                      {/* turn_index 由来は出力・件数以外 null。「—」と 0 を描き分ける */}
                      <td>{m.input_tokens === null ? '—' : m.input_tokens.toLocaleString()}</td>
                      <td>{m.output_tokens === null ? '—' : m.output_tokens.toLocaleString()}</td>
                      <td>{m.cache_read_tokens === null ? '—' : m.cache_read_tokens.toLocaleString()}</td>
                      <td>{m.cache_write_tokens === null ? '—' : m.cache_write_tokens.toLocaleString()}</td>
                      <td>{m.credits === null ? '—' : m.credits.toFixed(3)}</td>
                      <td>{m.record_count === null ? '—' : m.record_count.toLocaleString()}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
            <p className="fineprint">
              「—」は取得不可を表します (出所: shutdown_metrics は実値 4 種+クレジット、turn_index
              は出力トークンと件数のみ)。
            </p>
          </section>

          <section className="detail-section">
            <h3>系統図</h3>
            <LineageGraph
              subagents={detail.subagents}
              isLive={detail.is_live}
              now={now}
              selectedRunKey={selectedRunKey}
              hoveredRunKey={hoveredRunKey}
              onSelect={setSelectedRunKey}
              onHover={setHoveredRunKey}
            />
          </section>

          <section className="detail-section">
            <h3>ガント</h3>
            <GanttChart
              subagents={detail.subagents}
              gantt={detail.gantt}
              quotaEvents={detail.quota_events}
              now={now}
              selectedRunKey={selectedRunKey}
              hoveredRunKey={hoveredRunKey}
              onSelect={setSelectedRunKey}
              onHover={setHoveredRunKey}
            />
          </section>

          <section className="detail-section">
            <h3>本文タイムライン</h3>
            <TurnTimeline
              timeline={detail.timeline}
              timelineTotal={detail.timeline_total}
              timelineNextOffset={detail.timeline_next_offset}
              onLoadMore={() => void loadMore()}
              loadingMore={loadingMore}
              selectedRunKey={selectedRunKey}
              onClearSelection={() => setSelectedRunKey(null)}
            />
          </section>
        </>
      )}
    </section>
  )
}
