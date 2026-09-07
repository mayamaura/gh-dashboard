// 2 秒ポーリング (FR-C-41 / 42 / 43)。
//
// ★ タイマーは**画面 (タブ) 側に置く。**バックエンドに常駐ポーリングループを
// 作らない (ADR-0004 / NFR-11)。コンポーネントのマウント / アンマウントが
// 「タブを見ているか」と厳密に一致するため、「離れたら確実に止まる」保証が
// コードから読み取れる。
//
// ★ この経路で呼んでよいのは `liveStatusGet` **だけ**。ネットワークアクセスも
// 外部プロセス起動も含めない (INV-4 / NFR-03)。

import { useEffect, useRef, useState } from 'react'

import { liveStatusGet } from '../ipc/commands'
import type { LiveStatus } from '../types/dto'

export const LIVE_POLL_INTERVAL_MS = 2000

export interface LivePollResult {
  live: LiveStatus | null
  /** 直近の取得に失敗したか。失敗しても直前の値は保持する (NFR-07) */
  failed: boolean
}

/**
 * `enabled` が true の間だけ 2 秒ごとに `live_status_get` を呼ぶ。
 *
 * `enabled` は `useIsViewing()` の戻り値を渡すこと。タブ離脱・最小化で
 * false になり、タイマーが確実に止まる。
 */
export function useLivePoll(enabled: boolean): LivePollResult {
  const [live, setLive] = useState<LiveStatus | null>(null)
  const [failed, setFailed] = useState(false)
  // 前回の応答が返る前に次を撃たないためのガード
  const inFlight = useRef(false)

  useEffect(() => {
    if (!enabled) return

    let cancelled = false

    const tick = async () => {
      if (cancelled || inFlight.current) return
      inFlight.current = true
      try {
        const next = await liveStatusGet()
        if (!cancelled) {
          setLive(next)
          setFailed(false)
        }
      } catch {
        // 1 回の失敗で表示を捨てない。直前の値を保持したまま失敗を示す
        if (!cancelled) setFailed(true)
      } finally {
        inFlight.current = false
      }
    }

    void tick()
    const id = window.setInterval(() => void tick(), LIVE_POLL_INTERVAL_MS)

    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [enabled])

  return { live, failed }
}

/**
 * 自動インデックスの発火判定 (FR-C-58 / 59)。
 *
 * ★ 判定に使ってよいのは**ログの mtime と直近インデックス完了時刻の比較だけ**。
 * 定期リフレッシュ由来のタイムスタンプ (取得時刻など) を混ぜると、アイドル時も
 * 常に「未取り込みあり」と誤判定して際限なく回り続ける。
 *
 * 下限間隔 10 秒。これが無いと生成中のセッションがある間ポーリングのたびに走る。
 */
export const AUTO_INDEX_MIN_INTERVAL_MS = 10_000

export function shouldAutoIndex(
  logMtimeMs: number | null,
  lastIndexedAt: number | null,
  lastAttemptAt: number | null,
  now: number
): boolean {
  if (logMtimeMs === null) return false
  // 未取り込みの追記が実際にあるときだけ
  if (lastIndexedAt !== null && logMtimeMs <= lastIndexedAt) return false
  // 下限間隔
  if (lastAttemptAt !== null && now - lastAttemptAt < AUTO_INDEX_MIN_INTERVAL_MS) return false
  return true
}
