// タブ横断の共有状態 (FR-C-161 / FR-C-164)。
//
// ★ **長時間かかるバックエンド処理の進行状態を、ページのローカル state に置かない。**
// ページのローカル state はタブ切替でコンポーネントごと破棄されるため、
// 「処理は続いているのに表示だけ元に戻る」が起きる。
//
// 状態管理ライブラリは入れず、React 標準の `useSyncExternalStore` で足す (ADR-0007)。

import { useSyncExternalStore } from 'react'

import type {
  AnimationPref,
  DbSnapshot,
  DevState,
  IndexProgress,
  ProjectsSnapshot,
  QuotaGauge,
  QuotaSourceStatus,
  UsageToday,
} from '../types/dto'

export type TabId = 'copilot' | 'projects'

/** 一過性のエラー・警告通知。右下トーストで見せ、履歴はログ画面で確認する */
export interface Notice {
  id: number
  level: 'error' | 'warning'
  message: string
  at: number
}

/** 保持する通知履歴の上限。無限に溜めない */
const NOTICE_HISTORY_LIMIT = 200

export interface AppStoreState {
  tab: TabId

  /** ネイティブウィンドウが最小化されているか (IR-46 / FR-C-43) */
  minimized: boolean

  // --- タブ切替で消えてはいけない進行状態 (FR-C-161) ---
  /** インデックス実行中か。**手動更新のときだけバッジを出す** (FR-C-60) */
  indexing: boolean
  indexingManual: boolean
  indexProgress: IndexProgress | null
  lastIndexedAt: number | null

  /** 利用枠の最終取得時刻。鮮度そのものは各ゲージの observed_at で見る (FR-C-86) */
  quotaFetchedAt: number | null

  // --- タブ切替時に即描画するためのスナップショット (FR-C-164 / FR-P-86) ---
  snapshot: DbSnapshot | null
  quota: QuotaGauge[] | null
  /** 経路 A/B/C それぞれがなぜ使えないか (FR-C-83)。経路 C への降格自体には理由が残らないので、これで補う */
  quotaSourceStatus: QuotaSourceStatus | null
  usageToday: UsageToday | null
  projects: ProjectsSnapshot | null

  /** スキャン中か。タブを切り替えても「スキャン中…」が消えない (FR-C-161 / FR-P-87) */
  projectsScanning: boolean
  /** 詳細パネルで選択中のプロジェクト。タブを往復しても残る */
  projectsSelectedKey: string | null

  /** セッション詳細パネルで選択中の session_id。タブを往復しても残る (FR-C-161) */
  copilotSelectedSessionId: string | null

  animation: AnimationPref

  /** エラー・警告通知の履歴。新しい順ではなく発生順に並ぶ (末尾が最新) */
  notices: Notice[]
}

const initial: AppStoreState = {
  tab: 'copilot',
  minimized: false,
  indexing: false,
  indexingManual: false,
  indexProgress: null,
  lastIndexedAt: null,
  quotaFetchedAt: null,
  snapshot: null,
  quota: null,
  quotaSourceStatus: null,
  usageToday: null,
  projects: null,
  projectsScanning: false,
  projectsSelectedKey: null,
  copilotSelectedSessionId: null,
  animation: 'auto',
  notices: [],
}

let nextNoticeId = 1

let state: AppStoreState = initial
const listeners = new Set<() => void>()

const emit = () => {
  for (const l of listeners) l()
}

export const appStore = {
  getState: (): AppStoreState => state,

  subscribe(listener: () => void): () => void {
    listeners.add(listener)
    return () => {
      listeners.delete(listener)
    }
  },

  set(patch: Partial<AppStoreState>): void {
    // 変化が無いときは通知しない (2 秒ポーリングで無駄な再描画を起こさない)
    let changed = false
    for (const k of Object.keys(patch) as Array<keyof AppStoreState>) {
      if (!Object.is(state[k], patch[k])) {
        changed = true
        break
      }
    }
    if (!changed) return
    state = { ...state, ...patch }
    emit()
  },

  /** テスト用。本番コードから呼ばない */
  reset(): void {
    state = initial
    emit()
  },
}

/**
 * エラー・警告を通知する。呼び出し側はトースト表示やログ画面を気にせず、
 * 発生したことをここに投げるだけでよい (右下トースト + ログ画面は appStore.notices を購読する側の仕事)。
 */
export function pushNotice(level: Notice['level'], message: string): void {
  const notices = [
    ...state.notices,
    { id: nextNoticeId++, level, message, at: Date.now() },
  ].slice(-NOTICE_HISTORY_LIMIT)
  appStore.set({ notices })
}

/**
 * 1 プロジェクトの `dev` 状態だけを差し替える (IR-04 の戻り値 / IR-41 のイベント共通)。
 * 対象がキャッシュに無ければ何もしない (スキャン前や未知のプロジェクト)。
 */
export function patchProjectDev(pathKey: string, dev: DevState): void {
  const current = state.projects
  if (!current) return
  const projects = current.projects.map((p) => (p.path_key === pathKey ? { ...p, dev } : p))
  appStore.set({ projects: { ...current, projects } })
}

/** ストア全体を購読する。 */
export function useAppStore(): AppStoreState {
  return useSyncExternalStore(appStore.subscribe, appStore.getState, appStore.getState)
}

/**
 * 一部だけを購読する。
 *
 * `selector` は**同一参照を返す**こと (プリミティブか、ストアが持つ配列/オブジェクト
 * そのもの)。毎回新しいオブジェクトを作ると無限ループになる。
 */
export function useAppStoreSelector<T>(selector: (s: AppStoreState) => T): T {
  return useSyncExternalStore(
    appStore.subscribe,
    () => selector(state),
    () => selector(state)
  )
}
