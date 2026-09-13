// エラー・警告のトースト表示。右下にポップアップし、しばらくしたら自動で消える。
//
// ★ 通知そのものは appStore.notices が正 (タブ横断で共有)。ここではまだ画面に
//   出していない新着だけをローカルにキューして、時間で消す (表示状態はタブごとの
//   見た目に過ぎないのでローカル state でよい)。

import { useEffect, useRef, useState } from 'react'

import { useAppStoreSelector, type Notice } from '../store/appStore'

const DISPLAY_MS = 6000

export function ToastHost() {
  const notices = useAppStoreSelector((s) => s.notices)
  const [visible, setVisible] = useState<Notice[]>([])
  // マウント時点までの通知は「既読」扱いにして、後から追加された分だけ拾う
  const seenIdRef = useRef<number>(notices.at(-1)?.id ?? 0)

  useEffect(() => {
    const fresh = notices.filter((n) => n.id > seenIdRef.current)
    if (fresh.length === 0) return
    seenIdRef.current = fresh[fresh.length - 1]?.id ?? seenIdRef.current
    setVisible((prev) => [...prev, ...fresh])
    const timers = fresh.map((n) =>
      window.setTimeout(() => {
        setVisible((prev) => prev.filter((x) => x.id !== n.id))
      }, DISPLAY_MS)
    )
    return () => timers.forEach((t) => window.clearTimeout(t))
  }, [notices])

  if (visible.length === 0) return null

  return (
    <div className="toast-host">
      {visible.map((n) => (
        <div key={n.id} className={`toast toast-${n.level}`}>
          {n.message}
        </div>
      ))}
    </div>
  )
}
