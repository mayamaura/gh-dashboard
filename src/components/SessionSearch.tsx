// セッション検索 (T-7.4、FR-C-110/111)。
//
// ★ テキスト入力は 250ms デバウンスしてから `sessions_list_get` を呼ぶ。
//   バックエンドへの問い合わせ自体はここだけが行う (絞り込みロジックは
//   バックエンド側 IR-13 の実装に委ねる — フロントでの再絞り込みはしない)。

import { useEffect, useState } from 'react'

import { sessionsListGet } from '../ipc/commands'
import { relativeTime } from '../lib/format'
import type { SessionSummary } from '../types/dto'

const DEBOUNCE_MS = 250

export function SessionSearch({
  onSelect,
  selectedSessionId,
}: {
  onSelect: (sessionId: string) => void
  selectedSessionId: string | null
}) {
  const [text, setText] = useState('')
  const [subagentsOnly, setSubagentsOnly] = useState(false)
  const [results, setResults] = useState<SessionSummary[]>([])
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    const id = window.setTimeout(() => {
      void sessionsListGet({ text: text.trim() === '' ? null : text.trim(), with_subagents_only: subagentsOnly })
        .then((r) => {
          if (!cancelled) setResults(r)
        })
        .finally(() => {
          if (!cancelled) setLoading(false)
        })
    }, DEBOUNCE_MS)
    return () => {
      cancelled = true
      window.clearTimeout(id)
    }
  }, [text, subagentsOnly])

  const now = Date.now()

  return (
    <div className="session-search">
      <div className="toolbar">
        <input
          className="search"
          placeholder="検索 (フォルダ名・タイトル)"
          value={text}
          onChange={(e) => setText(e.target.value)}
        />
        <label>
          <input
            type="checkbox"
            checked={subagentsOnly}
            onChange={(e) => setSubagentsOnly(e.target.checked)}
          />
          サブエージェントを使ったもののみ
        </label>
        {loading && <span className="faint">検索中…</span>}
      </div>
      {results.length === 0 ? (
        <p className="muted">
          {text.trim() === '' && !subagentsOnly ? '保持されているセッションがありません。' : '一致するセッションがありません。'}
        </p>
      ) : (
        <ul className="session-list">
          {results.map((s) => (
            <li
              key={s.session_id}
              className={`session-card session-search-row${s.session_id === selectedSessionId ? ' selected' : ''}`}
              onClick={() => onSelect(s.session_id)}
            >
              <div className="session-card-main">
                <span className="session-folder">{s.folder_name ?? '不明なフォルダ'}</span>
                <span className="session-title">{s.title ?? '(タイトルなし)'}</span>
                <span className="faint">{relativeTime(s.last_activity_at, now)}</span>
                {s.agent_count > 0 && <span className="badge">サブエージェント {s.agent_count}</span>}
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
