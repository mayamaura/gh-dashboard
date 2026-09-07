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
  IndexProgress,
  ProjectsSnapshot,
  QuotaGauge,
  UsageToday,
} from '../types/dto'

export type TabId = 'copilot' | 'projects'

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
  usageToday: UsageToday | null
  projects: ProjectsSnapshot | null

  animation: AnimationPref
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
  usageToday: null,
  projects: null,
  animation: 'auto',
}

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
