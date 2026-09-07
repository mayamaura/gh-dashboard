// 「画面を見ているか」の判定 (FR-C-43 / NFR-12)。
//
// ★ ブラウザの `visibilitychange` **だけに頼らない。**
// ネイティブウィンドウの最小化は WebView の visibilitychange に伝播せず、
// 最小化しても `visibilityState` は `visible` のままになる。
// そのためネイティブ側から届く `window-visibility` イベント (IR-46) と合成する。

import { useEffect, useState } from 'react'

import { onWindowVisibility } from '../ipc/events'
import { appStore, useAppStoreSelector } from '../store/appStore'

/** ネイティブの最小化イベントを購読してストアに反映する。アプリ直下で 1 回だけ呼ぶ。 */
export function useWindowVisibilityBridge(): void {
  useEffect(() => {
    let unlisten: (() => void) | undefined
    let cancelled = false

    void onWindowVisibility(({ minimized }) => {
      appStore.set({ minimized })
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

/**
 * 「今この画面を見ているか」。
 *
 * `tab` が一致し、ドキュメントが可視で、ウィンドウが最小化されていないときだけ true。
 * ポーリングの起動条件はこれ 1 つに集約する。
 */
export function useIsViewing(tab: 'copilot' | 'projects'): boolean {
  const activeTab = useAppStoreSelector((s) => s.tab)
  const minimized = useAppStoreSelector((s) => s.minimized)
  const [documentVisible, setDocumentVisible] = useState(
    () => typeof document === 'undefined' || document.visibilityState === 'visible'
  )

  useEffect(() => {
    const handler = () => setDocumentVisible(document.visibilityState === 'visible')
    document.addEventListener('visibilitychange', handler)
    return () => document.removeEventListener('visibilitychange', handler)
  }, [])

  return activeTab === tab && documentVisible && !minimized
}
