// Copilot タブ。

import { useEffect, useRef, useState } from 'react'

import { QuotaGaugeRow } from '../components/QuotaGaugeRow'
import { SessionSearch } from '../components/SessionSearch'
import { SessionDetailPanel } from '../components/SessionDetailPanel'
import { useIsViewing } from '../hooks/useWindowVisible'
import { shouldAutoIndex, useLivePoll } from '../hooks/useLivePoll'
import { useQuotaPoll } from '../hooks/useQuotaPoll'
import { indexRefresh, quotaGet, snapshotGet, usageTodayGet } from '../ipc/commands'
import { appStore, pushNotice, useAppStore } from '../store/appStore'
import {
  activityLabel,
  ACTIVITY_TOOLTIP,
  contextUsagePct,
  creditsFromNanoAiu,
  duration,
  entrypointLabel,
  isSessionIdle,
  relativeTime,
} from '../lib/format'
import { deriveCardExpanded } from '../lib/sessionCard'
import type { LiveSession } from '../types/dto'

export function CopilotPage() {
  // タブを見ていて、かつ最小化されていないときだけ true (FR-C-41 / 43)
  const viewing = useIsViewing('copilot')
  const { live, failed } = useLivePoll(viewing)
  // T-6.10: 長周期タイマー (目安 5 分)。2 秒ポーリングには絶対に混ぜない (INV-4)
  const { refresh: refreshQuota } = useQuotaPoll(viewing)
  const {
    snapshot,
    quota,
    usageToday,
    lastIndexedAt,
    indexing,
    indexingManual,
    copilotSelectedSessionId,
  } = useAppStore()
  const now = Date.now()

  // T-5.9: アイドルセッションを隠すトグル。タブ切替で消えても実害の無い UI 好み設定
  const [showIdle, setShowIdle] = useState(false)
  // T-5.10: 手動で開閉したセッションの上書き。無ければ自動判定に従う
  const [expandOverrides, setExpandOverrides] = useState<Map<string, boolean>>(new Map())
  // T-5.11: 未インデックスのセッションについて、indexRefresh を起動済みかどうか。
  // アプリ再起動で忘れてよい程度の状態なのでローカル ref で足りる
  const attemptedIndexRef = useRef<Set<string>>(new Set())
  // T-5.12: 自動インデックスの下限間隔判定用。消えてよい一時値なのでローカル ref
  const lastAutoIndexAttemptRef = useRef<number | null>(null)
  // 2 秒ポーリング中は failed が続けて true になりうる。「失敗し始めた」瞬間だけ通知する
  const wasFailedRef = useRef(false)
  useEffect(() => {
    if (failed && !wasFailedRef.current) {
      pushNotice('warning', '直近の取得に失敗しました。表示は直前の値です。')
    }
    wasFailedRef.current = failed
  }, [failed])

  // タブを開いたときの一括取得。
  // ★ ここで取るものを 2 秒ポーリングに混ぜない (FR-C-103 / INV-4)
  useEffect(() => {
    if (!viewing) return
    let cancelled = false
    void (async () => {
      const [snap, gauges, usage] = await Promise.allSettled([
        snapshotGet(),
        quotaGet(false),
        usageTodayGet(),
      ])
      if (cancelled) return
      // 1 つの失敗で他を落とさない (NFR-24)
      if (snap.status === 'fulfilled') {
        appStore.set({ snapshot: snap.value, lastIndexedAt: snap.value.last_indexed_at })
      }
      if (gauges.status === 'fulfilled') {
        appStore.set({ quota: gauges.value, quotaFetchedAt: Date.now() })
      }
      if (usage.status === 'fulfilled') {
        appStore.set({ usageToday: usage.value })
      }
    })()
    return () => {
      cancelled = true
    }
  }, [viewing])

  // T-5.12: 未取り込みの追記があれば自動でインデックスを起動する (FR-C-58〜60)。
  // ★ 手動更新ではないので `indexingManual` には触らない
  useEffect(() => {
    if (!viewing || live === null) return
    if (
      shouldAutoIndex(
        live.newest_log_mtime_ms,
        lastIndexedAt,
        lastAutoIndexAttemptRef.current,
        Date.now()
      )
    ) {
      lastAutoIndexAttemptRef.current = Date.now()
      void indexRefresh()
    }
  }, [viewing, live, lastIndexedAt])

  // T-5.11: ライブには出ているがまだインデックスされていないセッションを、
  // セッションごとに 1 回だけ indexRefresh の対象にする
  useEffect(() => {
    if (live === null || snapshot === null) return
    const indexedIds = new Set(snapshot.recent_sessions.map((s) => s.session_id))
    for (const s of live.sessions) {
      if (indexedIds.has(s.session_id)) continue
      if (attemptedIndexRef.current.has(s.session_id)) continue
      attemptedIndexRef.current.add(s.session_id)
      void indexRefresh()
    }
  }, [live, snapshot])

  const indexedIds = new Set((snapshot?.recent_sessions ?? []).map((s) => s.session_id))

  const setOverride = (sessionId: string, open: boolean) => {
    setExpandOverrides((prev) => {
      const next = new Map(prev)
      next.set(sessionId, open)
      return next
    })
  }

  const visibleSessions =
    live === null
      ? []
      : live.sessions.filter((s) => showIdle || !isSessionIdle(s.last_activity_at, now))
  const hiddenIdleCount = (live?.sessions.length ?? 0) - visibleSessions.length

  return (
    <div className="page">
      {/* 稼働サマリー (FR-C-61) */}
      <section className="panel">
        <h2>稼働サマリー</h2>
        <div className="summary">
          <Stat label="稼働中セッション" value={live?.running_session_count ?? null} />
          {/* 件数は集合の length。別カウントを持たない (FR-C-51) */}
          <Stat label="稼働中サブエージェント" value={live?.running_subagent_count ?? null} />
          <Stat label="IDE ワークスペース" value={live?.ide_workspaces.length ?? null} />
        </div>
      </section>

      {/* 利用枠 (FR-C-80〜95) */}
      <section className="panel">
        <div className="panel-head">
          <h2>利用枠</h2>
          {/* T-6.10: 手動更新。長周期タイマー (5 分) とは別に即時取得できる */}
          <button onClick={() => void refreshQuota()}>今すぐ更新</button>
        </div>
        {quota === null || quota.length === 0 ? (
          <p className="muted">まだ取得していません。</p>
        ) : (
          <div className="gauges">
            {quota.map((g) => (
              <QuotaGaugeRow key={g.kind} gauge={g} now={now} />
            ))}
          </div>
        )}
        {/* NFR-45: 金額は概算であることを必ず添える (該当するのは $ 換算を出す場合のみ。
            現状 monthly_credits は $ を表示しないため、単位について誤解させない注記にする) */}
        <p className="fineprint">
          消費量は AI Credits 単位です (金額換算は表示していません)。
        </p>
      </section>

      {/* 本日の使用状況 (FR-C-100〜105) */}
      <section className="panel">
        <h2>本日の使用状況</h2>
        {usageToday === null ? (
          <p className="muted">まだ取得していません。</p>
        ) : (
          <>
            <div className="summary">
              <Stat label="入力トークン" value={usageToday.input_tokens} />
              <Stat label="出力トークン" value={usageToday.output_tokens} />
              <Stat label="ターン" value={usageToday.turn_count} />
            </div>
            {/* NFR-43: 除外が起きたら無言で欠落させない */}
            {usageToday.excluded_records > 0 && (
              <p className="note-inline">
                {usageToday.excluded_records} 件を集計から除外しました。
              </p>
            )}
          </>
        )}
      </section>

      {/* 稼働中セッション (FR-C-53〜60) */}
      <section className="panel">
        <div className="panel-head">
          <h2>稼働中セッション</h2>
          <label className="idle-toggle">
            <input type="checkbox" checked={showIdle} onChange={(e) => setShowIdle(e.target.checked)} />
            アイドルなセッションも表示 ({hiddenIdleCount} 件隠れています)
          </label>
        </div>
        {live === null ? (
          <p className="muted">読み込み中…</p>
        ) : visibleSessions.length === 0 ? (
          // 0 件は正常系。エラーとして見せない
          <p className="muted">
            {live.sessions.length === 0 ? '稼働中のセッションはありません。' : 'アイドルなセッションのみです。'}
          </p>
        ) : (
          <ul className="session-list">
            {visibleSessions.map((s) => (
              <SessionCard
                key={s.session_id}
                session={s}
                now={now}
                failed={failed}
                indexed={indexedIds.has(s.session_id)}
                override={expandOverrides.get(s.session_id)}
                onToggle={(open) => setOverride(s.session_id, open)}
              />
            ))}
          </ul>
        )}
      </section>

      {/* IDE ワークスペース (FR-C-70〜72) */}
      <section className="panel">
        <h2>IDE ワークスペース</h2>
        {live === null ? (
          <p className="muted">読み込み中…</p>
        ) : live.ide_workspaces.length === 0 ? (
          <p className="muted">接続されている IDE はありません。</p>
        ) : (
          <ul className="session-list">
            {live.ide_workspaces.map((w, i) => (
              <li key={i} className="session-card">
                <span className={w.connected ? 'badge' : 'badge muted-badge'}>
                  {w.connected ? '接続中' : '切断'}
                </span>
                <span className="session-folder">{w.ide_name ?? '不明な IDE'}</span>
                <span className="session-title">{w.folders.join(', ') || '(フォルダなし)'}</span>
              </li>
            ))}
          </ul>
        )}
      </section>

      {/* セッション履歴 (FR-C-110〜121) */}
      <section className="panel">
        <div className="panel-head">
          <h2>セッション履歴</h2>
          <button
            disabled={indexing}
            onClick={() => {
              appStore.set({ indexingManual: true })
              void indexRefresh()
            }}
          >
            {indexing && indexingManual ? 'インデックス中…' : '今すぐ更新'}
          </button>
        </div>
        {snapshot === null ? (
          <p className="muted">読み込み中…</p>
        ) : (
          <p className="muted">
            {/* NFR-44: 「累計」ではなく「保持されている」 */}
            保持されているセッション {snapshot.retained_session_count} 件 / サブエージェント実行{' '}
            {snapshot.subagent_run_count} 件
          </p>
        )}
        <SessionSearch
          selectedSessionId={copilotSelectedSessionId}
          onSelect={(id) => appStore.set({ copilotSelectedSessionId: id })}
        />
      </section>

      {copilotSelectedSessionId !== null && (
        <SessionDetailPanel
          sessionId={copilotSelectedSessionId}
          onClose={() => appStore.set({ copilotSelectedSessionId: null })}
        />
      )}
    </div>
  )
}

function SessionCard({
  session: s,
  now,
  failed,
  indexed,
  override,
  onToggle,
}: {
  session: LiveSession
  now: number
  failed: boolean
  indexed: boolean
  override: boolean | undefined
  onToggle: (open: boolean) => void
}) {
  const subagentCount = s.running_subagent_ids.length
  const expanded = deriveCardExpanded(subagentCount, failed, override)
  const pct = contextUsagePct(s.context_used, s.context_limit)
  const credits = creditsFromNanoAiu(s.session_nano_aiu)

  return (
    <li className="session-card">
      <div className="session-card-main">
        <span className={`dot ${s.activity}`} aria-hidden />
        <span className="session-folder">{s.folder_name ?? '不明なフォルダ'}</span>
        <span className="chip">{entrypointLabel(s.entrypoint)}</span>
        <span className="session-title">{s.title ?? '(タイトルなし)'}</span>
        <span
          className="session-activity"
          title={ACTIVITY_TOOLTIP[s.activity] ?? undefined}
        >
          {activityLabel(s.activity)}
        </span>
        <span className="muted">{s.model ?? '不明なモデル'}</span>
        <span className="muted">稼働 {duration(s.started_at, now)}</span>
        <span className="muted">最終活動 {relativeTime(s.last_activity_at, now)}</span>
        {/* NFR-40 / 43: コンテキスト使用率・消費クレジットは取得不可を数字で埋めない */}
        <span className="muted">コンテキスト {pct === null ? '取得できませんでした' : `${pct.toFixed(0)}%`}</span>
        <span className="muted">{credits === null ? 'クレジット: 取得できませんでした' : `${credits.toFixed(3)} クレジット`}</span>
        {subagentCount > 0 && (
          <button className="badge badge-button" onClick={() => onToggle(!expanded)}>
            サブエージェント {subagentCount} {expanded ? '▲' : '▼'}
          </button>
        )}
      </div>
      {!indexed && (
        // FR-C-57: エラーではなく穏やかな文言。indexRefresh の起動は effect 側で 1 回だけ行う
        <p className="note-inline">まだ記録がありません。</p>
      )}
      {expanded && subagentCount > 0 && (
        // T-5.10: 暫定実装。`running_subagent_ids` のフラット一覧のみ。
        // 完全な入れ子構造は段階 7 で `session_detail_get` / `tree::build` が
        // 実装され次第、差し替える
        <ul className="session-lineage">
          {s.running_subagent_ids.map((id) => (
            <li key={id} className="muted">
              {id}
            </li>
          ))}
        </ul>
      )}
    </li>
  )
}

function Stat({ label, value }: { label: string; value: number | null }) {
  return (
    <div className="stat">
      {/* 取れていない値を 0 と見せない (NFR-40) */}
      <div className="stat-value">{value === null ? '—' : value.toLocaleString()}</div>
      <div className="stat-label">{label}</div>
    </div>
  )
}
