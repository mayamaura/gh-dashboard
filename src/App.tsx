// 2 タブのシェル。
//
// ★ タブ状態は appStore に置く (FR-C-161 / 164)。ページのローカル state に
// 置くと、切り替えたときに進行中の処理が「消えて」見える。

import { useRef, useState } from 'react'

import { CopilotPage } from './pages/CopilotPage'
import { ProjectsPage } from './pages/ProjectsPage'
import { appStore, useAppStore } from './store/appStore'
import { useWindowVisibilityBridge } from './hooks/useWindowVisible'
import { useProjectsSnapshotBridge } from './hooks/useProjectsSnapshotBridge'
import { useDevStatusBridge } from './hooks/useDevStatusBridge'
import { useIndexBridge } from './hooks/useIndexBridge'
import { relativeTime } from './lib/format'
import { scanProjects, stopAllDevServers } from './lib/projectsActions'

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
    projects,
    projectsScanning,
  } = useAppStore()
  const now = Date.now()

  // 「すべて停止」の二段階確認 (FR-P-67)
  const [confirmingStopAll, setConfirmingStopAll] = useState(false)
  const confirmTimer = useRef<number | undefined>(undefined)

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
    <div className="app">
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
              <span className="muted">利用枠: {relativeTime(quotaFetchedAt, now)}</span>
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
        </div>
      </header>

      <main className="app-main">
        {tab === 'copilot' ? <CopilotPage /> : <ProjectsPage />}
      </main>
    </div>
  )
}
