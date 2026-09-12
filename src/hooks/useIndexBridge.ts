// `index-progress` / `snapshot` イベントの橋渡し (IR-43 / IR-44)。
//
// ★ `listen` は `src/ipc/events.ts` だけに書く (IR-31)。ここではラッパーの
//   `onIndexProgress` / `onSnapshot` を呼ぶだけ。
// ★ App で 1 回だけ購読する。購読解除を必ず返す — 積み上がると多重発火する。
//
// **「インデックス中」バッジは手動更新のときだけ出す** (FR-C-60)。ここでは
// `indexingManual` を true にはしない。true にするのは手動更新の起動元だけ
// (自動起動 (T-5.12) では触らない)。完了時は常に false に戻す。

import { useEffect } from 'react'

import { onIndexProgress, onSnapshot } from '../ipc/events'
import { appStore } from '../store/appStore'

export function useIndexBridge(): void {
  useEffect(() => {
    let cancelled = false
    let unlistenProgress: (() => void) | undefined
    let unlistenSnapshot: (() => void) | undefined

    void onIndexProgress((progress) => {
      appStore.set({ indexing: true, indexProgress: progress })
    }).then((fn) => {
      if (cancelled) fn()
      else unlistenProgress = fn
    })

    void onSnapshot((snapshot) => {
      appStore.set({
        indexing: false,
        indexingManual: false,
        indexProgress: null,
        snapshot,
        lastIndexedAt: snapshot.last_indexed_at,
      })
    }).then((fn) => {
      if (cancelled) fn()
      else unlistenSnapshot = fn
    })

    return () => {
      cancelled = true
      unlistenProgress?.()
      unlistenSnapshot?.()
    }
  }, [])
}
