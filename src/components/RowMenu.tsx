// 行内メニュー (FR-P-88)。
//
// ★ **クリック位置から座標計算して `position: fixed` で固定配置する。**
//   一覧が横スクロールコンテナに入っていても overflow クリッピングされない。
// ★ 外側クリックと Esc で閉じる。

import { useEffect, useLayoutEffect, useRef, useState } from 'react'

export interface RowMenuItem {
  label: string
  onSelect: () => void
  disabled?: boolean
  title?: string
}

export function RowMenu({
  x,
  y,
  items,
  onClose,
}: {
  x: number
  y: number
  items: RowMenuItem[]
  onClose: () => void
}) {
  const ref = useRef<HTMLDivElement>(null)

  // 画面端で切れないように、描画後に実サイズを測って位置を補正する。
  // ⋮ は行の右端にあるので、右に開くと必ずウィンドウ外に出る (実機で確認)。
  // 右に収まらなければ左へ、下に収まらなければ上へ寄せる。
  const [pos, setPos] = useState({ top: y, left: x })
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const { width, height } = el.getBoundingClientRect()
    const margin = 8
    let left = x
    let top = y
    if (left + width + margin > window.innerWidth) left = Math.max(margin, x - width)
    if (top + height + margin > window.innerHeight) top = Math.max(margin, y - height)
    setPos({ top, left })
  }, [x, y, items.length])

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose()
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [onClose])

  return (
    <div className="row-menu" style={{ position: 'fixed', top: pos.top, left: pos.left }} ref={ref}>
      {items.map((item) => (
        <button
          key={item.label}
          className="row-menu-item"
          disabled={item.disabled}
          title={item.title}
          onClick={() => {
            item.onSelect()
            onClose()
          }}
        >
          {item.label}
        </button>
      ))}
    </div>
  )
}
