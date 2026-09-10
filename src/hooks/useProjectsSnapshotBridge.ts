// `projects-snapshot` イベントの橋渡し (IR-40)。
//
// ★ `listen` は `src/ipc/events.ts` だけに書く (IR-31)。ここではラッパーの
//   `onProjectsSnapshot` を呼ぶだけ。
// ★ App で 1 回だけ購読する。購読解除を必ず返す — 積み上がると多重発火する。

import { useEffect } from 'react'

import { onProjectsSnapshot } from '../ipc/events'
import { appStore } from '../store/appStore'

export function useProjectsSnapshotBridge(): void {
  useEffect(() => {
    let unlisten: (() => void) | undefined
    let cancelled = false

    void onProjectsSnapshot((snapshot) => {
      appStore.set({ projects: snapshot })
    }).then((fn) => {
      if (cancelled) fn()
      else unlisten = fn
    })

    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [])
}
