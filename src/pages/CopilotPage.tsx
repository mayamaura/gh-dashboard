// Copilot タブ。
//
// 実装状況: ポーリングの配線と「取れないものの見せ方」だけが入った足場。
// 中身は段階 5〜7 で実装する (docs/implementation-plan.html)。

import { useEffect } from 'react'

import { QuotaGaugeRow } from '../components/QuotaGaugeRow'
import { useIsViewing } from '../hooks/useWindowVisible'
import { useLivePoll } from '../hooks/useLivePoll'
import { quotaGet, snapshotGet, usageTodayGet } from '../ipc/commands'
import { appStore, useAppStore } from '../store/appStore'
import { activityLabel, ACTIVITY_TOOLTIP, relativeTime } from '../lib/format'

export function CopilotPage() {
  // タブを見ていて、かつ最小化されていないときだけ true (FR-C-41 / 43)
  const viewing = useIsViewing('copilot')
  const { live, failed } = useLivePoll(viewing)
  const { snapshot, quota, usageToday } = useAppStore()
  const now = Date.now()

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
        {failed && (
          <p className="note-inline">
            直近の取得に失敗しました。表示は直前の値です。
          </p>
        )}
      </section>

      {/* 利用枠 (FR-C-80〜95) */}
      <section className="panel">
        <h2>利用枠</h2>
        {quota === null || quota.length === 0 ? (
          <p className="muted">まだ取得していません。</p>
        ) : (
          <div className="gauges">
            {quota.map((g) => (
              <QuotaGaugeRow key={g.kind} gauge={g} now={now} />
            ))}
          </div>
        )}
        {/* NFR-45: 金額は概算であることを必ず添える */}
        <p className="fineprint">
          金額は概算です。請求額の正は GitHub の課金画面です。
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

      {/* 稼働中セッション (FR-C-53) */}
      <section className="panel">
        <h2>稼働中セッション</h2>
        {live === null ? (
          <p className="muted">読み込み中…</p>
        ) : live.sessions.length === 0 ? (
          // 0 件は正常系。エラーとして見せない
          <p className="muted">稼働中のセッションはありません。</p>
        ) : (
          <ul className="session-list">
            {live.sessions.map((s) => (
              <li key={s.session_id} className="session-card">
                <span className={`dot ${s.activity}`} aria-hidden />
                <span className="session-folder">{s.folder_name ?? '不明なフォルダ'}</span>
                <span className="session-title">{s.title ?? '(タイトルなし)'}</span>
                <span
                  className="session-activity"
                  title={ACTIVITY_TOOLTIP[s.activity] ?? undefined}
                >
                  {activityLabel(s.activity)}
                </span>
                <span className="muted">{relativeTime(s.last_activity_at, now)}</span>
                {s.running_subagent_ids.length > 0 && (
                  <span className="badge">サブエージェント {s.running_subagent_ids.length}</span>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>

      {/* セッション履歴 (FR-C-110〜121) */}
      <section className="panel">
        <h2>セッション履歴</h2>
        {snapshot === null ? (
          <p className="muted">読み込み中…</p>
        ) : (
          <p className="muted">
            {/* NFR-44: 「累計」ではなく「保持されている」 */}
            保持されているセッション {snapshot.retained_session_count} 件 / サブエージェント実行{' '}
            {snapshot.subagent_run_count} 件
          </p>
        )}
        <p className="fineprint">検索・系統図・ガント・本文ビューアは段階 7 (T-7.4〜7.11) で実装。</p>
      </section>
    </div>
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
