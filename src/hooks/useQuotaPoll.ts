// 利用枠の長周期タイマー (FR-C-135 / NFR-12)。
//
// ★ `quota_get` はネットワークを伴うため、**2 秒ポーリング (useLivePoll) には
// 絶対に混ぜない** (INV-4)。取得契機は「タブを開いたとき」「更新ボタン」
// 「この長周期タイマー」の 3 つだけに限る。
//
// ★ タブを開いた瞬間の初回取得は `CopilotPage` 側の effect が既に行っている。
// ここでは繰り返しタイマーだけを持ち、マウント直後には撃たない (二重取得を防ぐ)。
//
// ★ タイマーは画面側に置く。`enabled` には `useIsViewing()` の戻り値を渡すこと。
// タブ離脱・最小化で確実に止まる。

import { useCallback, useEffect, useRef } from 'react'

import { quotaGet } from '../ipc/commands'
import { appStore } from '../store/appStore'

export const QUOTA_POLL_INTERVAL_MS = 5 * 60_000

export interface QuotaPollResult {
  /** 「今すぐ更新」ボタンから呼ぶ。強制取得 (force=true)。 */
  refresh: () => Promise<void>
}

export function useQuotaPoll(enabled: boolean): QuotaPollResult {
  // 前回の応答が返る前に次を撃たないためのガード (useLivePoll と同じ設計)
  const inFlight = useRef(false)

  const fetchQuota = useCallback(async (force: boolean) => {
    if (inFlight.current) return
    inFlight.current = true
    try {
      const next = await quotaGet(force)
      appStore.set({ quota: next, quotaFetchedAt: Date.now() })
    } finally {
      inFlight.current = false
    }
  }, [])

  useEffect(() => {
    if (!enabled) return
    // 初回は撃たない (CopilotPage の effect が既に取得済み)。繰り返しのみ
    const id = window.setInterval(() => void fetchQuota(false), QUOTA_POLL_INTERVAL_MS)
    return () => window.clearInterval(id)
  }, [enabled, fetchQuota])

  const refresh = useCallback(() => fetchQuota(true), [fetchQuota])

  return { refresh }
}
