// `projects-dev-status` イベントの橋渡し (IR-41)。
//
// ★ `listen` は `src/ipc/events.ts` だけに書く (IR-31)。ここではラッパーの
//   `onDevStatus` を呼ぶだけ。
// ★ App で 1 回だけ購読する。購読解除を必ず返す — 積み上がると多重発火する。

import { useEffect } from 'react'

import { onDevStatus } from '../ipc/events'
import { patchProjectDev } from '../store/appStore'

export function useDevStatusBridge(): void {
  useEffect(() => {
    let unlisten: (() => void) | undefined
    let cancelled = false

    void onDevStatus(({ path_key, state }) => {
      patchProjectDev(path_key, state)
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
