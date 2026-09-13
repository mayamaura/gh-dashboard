// エラー・警告の通知履歴 (ログ画面)。ヘッダーの「ログ」ボタンから開く。
//
// ★ トースト自体は数秒で消えるので、消えたあとに見返す先をここにする。

import { useEffect } from 'react'

import { useAppStoreSelector } from '../store/appStore'

export function NoticeLogModal({ onClose }: { onClose: () => void }) {
  const notices = useAppStoreSelector((s) => s.notices)

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [onClose])

  const ordered = [...notices].reverse()

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal notice-log-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>エラー・警告ログ</h2>
          <button onClick={onClose}>閉じる</button>
        </div>
        {ordered.length === 0 ? (
          <p className="muted">記録はありません。</p>
        ) : (
          <ul className="notice-log-list">
            {ordered.map((n) => (
              <li key={n.id} className={`notice-log-item notice-${n.level}`}>
                <span className="notice-log-time">
                  {new Date(n.at).toLocaleString('ja-JP')}
                </span>
                <span className="notice-log-message">{n.message}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  )
}
