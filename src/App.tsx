// 2 タブのシェル。
//
// ★ タブ状態は appStore に置く (FR-C-161 / 164)。ページのローカル state に
// 置くと、切り替えたときに進行中の処理が「消えて」見える。

import { CopilotPage } from './pages/CopilotPage'
import { ProjectsPage } from './pages/ProjectsPage'
import { appStore, useAppStore } from './store/appStore'
import { useWindowVisibilityBridge } from './hooks/useWindowVisible'
import { relativeTime } from './lib/format'

const TABS = [
  { id: 'copilot', label: 'Copilot' },
  { id: 'projects', label: 'プロジェクト' },
] as const

export function App() {
  // ネイティブの最小化を受け取る。ここで 1 回だけ購読する (IR-46 / FR-C-43)
  useWindowVisibilityBridge()

  const { tab, indexing, indexingManual, lastIndexedAt, quotaFetchedAt } = useAppStore()
  const now = Date.now()

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
            <span className="muted">プロジェクト</span>
          )}
        </div>
      </header>

      <main className="app-main">
        {tab === 'copilot' ? <CopilotPage /> : <ProjectsPage />}
      </main>
    </div>
  )
}
