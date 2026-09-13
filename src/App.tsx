// 2 タブのシェル。
//
// ★ タブ状態は appStore に置く (FR-C-161 / 164)。ページのローカル state に
// 置くと、切り替えたときに進行中の処理が「消えて」見える。

import { useEffect, useRef, useState } from 'react'

import { CopilotPage } from './pages/CopilotPage'
import { ProjectsPage } from './pages/ProjectsPage'
import { appStore, useAppStore, useAppStoreSelector } from './store/appStore'
import { ToastHost } from './components/ToastHost'
import { NoticeLogModal } from './components/NoticeLogModal'
import { useWindowVisibilityBridge } from './hooks/useWindowVisible'
import { useProjectsSnapshotBridge } from './hooks/useProjectsSnapshotBridge'
import { useDevStatusBridge } from './hooks/useDevStatusBridge'
import { useIndexBridge } from './hooks/useIndexBridge'
import { quotaRouteLabel, relativeTime } from './lib/format'
import { scanProjects, stopAllDevServers } from './lib/projectsActions'
import { animationPrefGet, animationPrefSet } from './ipc/commands'
import type { AnimationPref } from './types/dto'

const TABS = [
  { id: 'copilot', label: 'Copilot' },
  { id: 'projects', label: 'プロジェクト' },
] as const

export function App() {
  // ネイティブの最小化を受け取る。ここで 1 回だけ購読する (IR-46 / FR-C-43)
  useWindowVisibilityBridge()
  // スキャン・設定変更のたびにバックエンドが発火する (IR-40)。ここで 1 回だけ購読する
  useProjectsSnapshotBridge()
  // dev サーバーの状態変化 (URL 検出・終了) を追いかける (IR-41)。ここで 1 回だけ購読する
  useDevStatusBridge()
  // 差分インデックスの進捗・完了 (IR-43 / 44)。ここで 1 回だけ購読する
  useIndexBridge()

  const {
    tab,
    indexing,
    indexingManual,
    lastIndexedAt,
    quotaFetchedAt,
    quota,
    projects,
    projectsScanning,
    animation,
  } = useAppStore()
  const now = Date.now()

  // T-7.14: 起動時に永続化済みのアニメーション設定を読み込む (FR-C-162)
  useEffect(() => {
    void animationPrefGet().then((pref) => appStore.set({ animation: pref }))
  }, [])

  const onAnimationChange = (pref: AnimationPref) => {
    appStore.set({ animation: pref })
    void animationPrefSet(pref)
  }

  // 「すべて停止」の二段階確認 (FR-P-67)
  const [confirmingStopAll, setConfirmingStopAll] = useState(false)
  const confirmTimer = useRef<number | undefined>(undefined)

  // エラー・警告ログ画面 (トーストが消えたあとに見返す先)
  const [showNoticeLog, setShowNoticeLog] = useState(false)
  const noticeCount = useAppStoreSelector((s) => s.notices.length)

  const onStopAllClick = () => {
    if (!confirmingStopAll) {
      setConfirmingStopAll(true)
      window.clearTimeout(confirmTimer.current)
      // 3 秒操作が無ければ元に戻す
      confirmTimer.current = window.setTimeout(() => setConfirmingStopAll(false), 3000)
      return
    }
    window.clearTimeout(confirmTimer.current)
    setConfirmingStopAll(false)
    void stopAllDevServers()
  }

  return (
    <div className="app" data-animation={animation === 'auto' ? undefined : animation}>
      <header className="app-header">
        <nav className="tabs" role="tablist">
          {TABS.map((t) => (
            <button
              key={t.id}
              role="tab"
              aria-selected={tab === t.id}
              className={tab === t.id ? 'tab current' : 'tab'}
              onClick={() => appStore.set({ tab: t.id })}
            >
              {t.label}
            </button>
          ))}
        </nav>

        <div className="header-status">
          {tab === 'copilot' ? (
            <>
              {/* 「インデックス中」は手動更新のときだけ出す (FR-C-60)。
                  自動実行では出さず、最終インデックスの相対時刻を鮮度の保証とする */}
              {indexing && indexingManual ? (
                <span className="badge">インデックス中…</span>
              ) : (
                <span className="muted">
                  最終インデックス: {relativeTime(lastIndexedAt, now)}
                </span>
              )}
              {/* FR-C-160: 取得経路と最終取得時刻の両方を出す */}
              <span className="muted">
                利用枠: {quotaRouteLabel(quota)} / {relativeTime(quotaFetchedAt, now)}
              </span>
            </>
          ) : (
            <>
              {/* FR-P-85: 最終スキャン (相対時刻) */}
              <span className="muted">
                最終スキャン: {relativeTime(projects?.scanned_at ?? null, now)}
              </span>
              <button onClick={onStopAllClick} className={confirmingStopAll ? 'danger' : undefined}>
                {confirmingStopAll ? '本当に停止しますか?' : 'すべて停止'}
              </button>
              {/* FR-P-87: スキャン中はボタンに明示する */}
              <button onClick={() => void scanProjects()} disabled={projectsScanning}>
                {projectsScanning ? 'スキャン中…' : '更新'}
              </button>
            </>
          )}
          {/* T-7.14: アニメーション設定 (FR-C-162)。`自動` は属性を付けず CSS の
              prefers-reduced-motion に任せる */}
          <label className="animation-pref">
            アニメーション
            <select
              value={animation}
              onChange={(e) => onAnimationChange(e.target.value as AnimationPref)}
            >
              <option value="auto">自動</option>
              <option value="on">入</option>
              <option value="off">切</option>
            </select>
          </label>

          {/* エラー・警告はトーストで一時表示するだけなので、履歴をここから見返せるようにする */}
          <button onClick={() => setShowNoticeLog(true)}>
            ログ{noticeCount > 0 ? ` (${noticeCount})` : ''}
          </button>
        </div>
      </header>

      <main className="app-main">
        {tab === 'copilot' ? <CopilotPage /> : <ProjectsPage />}
      </main>

      <ToastHost />
      {showNoticeLog && <NoticeLogModal onClose={() => setShowNoticeLog(false)} />}
    </div>
  )
}
