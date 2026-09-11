// 詳細パネル (FR-P-84)。
//
// パス / 種別 / 作業ディレクトリ / 起動コマンド候補 / 外部ツールボタン列 /
// dev サーバー状態と操作とログ / git 状態 / Copilot 利用状況 / 起動不可の理由 /
// 手動調整フォーム。

import { useEffect, useState } from 'react'

import type { Project, ProjectOverrideRequest } from '../types/dto'
import { projectsSettingsUpdate } from '../ipc/commands'
import { appStore } from '../store/appStore'
import { launchState } from '../lib/projectList'
import { describeError, matchedByLabel, relativeTime } from '../lib/format'
import { openAgent, openBrowser, openFolder, openTerminal, openVscode } from '../lib/projectsActions'

function devStateText(p: Project): string {
  const d = p.dev
  switch (d.state) {
    case 'stopped':
      return '停止中'
    case 'starting':
      return '起動中'
    case 'running': {
      const parts = [d.url ?? 'URL 不明', `PID ${d.pid}`, `開始 ${new Date(d.started_at).toLocaleTimeString('ja-JP')}`]
      return `稼働中 (${parts.join(', ')})`
    }
    case 'exited':
      return `終了 (終了コード ${d.code ?? '不明'})`
    case 'failed':
      return `失敗 (${d.reason})`
  }
}

export function ProjectDetail({ project: p, now }: { project: Project | null; now: number }) {
  if (p === null) {
    return (
      <div className="panel detail-panel">
        <p className="muted">プロジェクトを選択してください。</p>
      </div>
    )
  }
  return <ProjectDetailBody project={p} now={now} />
}

function ProjectDetailBody({ project: p, now }: { project: Project; now: number }) {
  const launch = launchState(p)
  const fallback = p.copilot ? matchedByLabel(p.copilot.matched_by) : null

  // 手動調整フォーム (FR-P-30)。
  //
  // ★ 初期値は **保存済みの上書き値そのもの** (`override_values`) から取る。
  //   `display_name` / `resolved_command` / `working_dir` は上書き適用後の
  //   表示用の値なので、それを初期値にすると「上書きしていないのに上書きとして
  //   保存される」「表示名だけ直したら他の上書きが消える」が起きる。
  //   保存は行全体の置き換えなので、空欄 = その項目の上書きを解除する。
  const ov = p.override_values
  const [displayName, setDisplayName] = useState(ov?.display_name ?? '')
  const [commandOverride, setCommandOverride] = useState(ov?.command_override ?? '')
  const [workingDirOverride, setWorkingDirOverride] = useState(ov?.working_dir_override ?? '')
  const [sortOrder, setSortOrder] = useState(
    ov?.sort_order === null || ov?.sort_order === undefined ? '' : String(ov.sort_order)
  )
  const [hidden, setHidden] = useState(ov?.hidden ?? false)
  const [archived, setArchived] = useState(ov?.archived ?? false)
  const [fieldError, setFieldError] = useState<{ field: string; message: string } | null>(null)
  const [saving, setSaving] = useState(false)

  // 選択が切り替わったら、または保存後に新しい値が届いたら、フォームを合わせる
  useEffect(() => {
    const o = p.override_values
    setDisplayName(o?.display_name ?? '')
    setCommandOverride(o?.command_override ?? '')
    setWorkingDirOverride(o?.working_dir_override ?? '')
    setSortOrder(o?.sort_order === null || o?.sort_order === undefined ? '' : String(o.sort_order))
    setHidden(o?.hidden ?? false)
    setArchived(o?.archived ?? false)
    setFieldError(null)
  }, [p.path_key, p.override_values])

  const save = async () => {
    setSaving(true)
    setFieldError(null)
    const req: ProjectOverrideRequest = {
      path_key: p.path_key,
      display_name: displayName.trim() === '' ? null : displayName,
      command_override: commandOverride.trim() === '' ? null : commandOverride,
      working_dir_override: workingDirOverride.trim() === '' ? null : workingDirOverride,
      sort_order: sortOrder.trim() === '' ? null : Number(sortOrder),
      hidden,
      archived,
    }
    try {
      // IR-32: 変更系は戻り値のスナップショットでも即時反映する (イベントとの二重取りは appStore.set が差分無視で吸収する)
      const snapshot = await projectsSettingsUpdate(req)
      appStore.set({ projects: snapshot, projectsError: null })
    } catch (e) {
      const err = e as { kind?: string; field?: string; message?: string }
      if (err && err.kind === 'invalid_input' && err.field && err.message) {
        setFieldError({ field: err.field, message: err.message })
      } else {
        appStore.set({ projectsError: describeError(e) })
      }
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="panel detail-panel">
      <h2>{p.display_name}</h2>

      <dl className="detail-grid">
        <dt>パス</dt>
        <dd>{p.root_path}</dd>
        <dt>種別</dt>
        <dd>{p.kind_label}</dd>
        <dt>作業ディレクトリ</dt>
        <dd>{p.working_dir}</dd>
        <dt>起動コマンド候補</dt>
        <dd>{p.command_candidates.length > 0 ? p.command_candidates.join(' / ') : '候補なし'}</dd>
        <dt>解決済みコマンド</dt>
        <dd>{p.resolved_command ?? '—'}</dd>
      </dl>

      {/* FR-P-22: 起動不可の理由は目立つ形で出す */}
      {launch.reason && <p className="note-inline">起動不可: {launch.reason}</p>}

      <div className="button-row">
        <button onClick={() => void openVscode(p.path_key)}>VS Code で開く</button>
        <button onClick={() => void openFolder(p.path_key)}>エクスプローラーで開く</button>
        <button onClick={() => void openTerminal(p.path_key)}>ターミナルで開く</button>
        <button onClick={() => void openAgent(p.path_key)}>Copilot CLI を起動</button>
        {p.dev.state === 'running' && p.dev.url && (
          <button onClick={() => openBrowser(p.dev.state === 'running' ? p.dev.url ?? '' : '')}>
            ブラウザで開く
          </button>
        )}
      </div>

      <section className="detail-section">
        <h3>dev サーバー</h3>
        <p>{devStateText(p)}</p>
        <div className="button-row">
          <button disabled title="段階 3 (T-3.2) で実装">
            起動
          </button>
          <button disabled title="段階 3 (T-3.2) で実装">
            停止
          </button>
        </div>
        <p className="muted">ログはありません。</p>
      </section>

      <section className="detail-section">
        <h3>git</h3>
        {/* FR-P-58 に類する扱い: 情報が無いセクションは空欄で埋めず明示する */}
        {p.git === null ? (
          <p className="muted">git 情報なし</p>
        ) : (
          <dl className="detail-grid">
            <dt>ブランチ</dt>
            <dd>{p.git.branch ?? 'detached HEAD'}</dd>
            <dt>未コミット変更</dt>
            <dd>{p.git.dirty ? 'あり' : 'なし'}</dd>
            <dt>リモート</dt>
            <dd>{p.git.has_remote ? 'あり' : 'なし'}</dd>
            <dt>最終コミット</dt>
            <dd>{relativeTime(p.git.last_commit_at, now)}</dd>
          </dl>
        )}
      </section>

      {/* FR-P-58: 履歴が一切ないプロジェクトはセクションごと出さない */}
      {p.copilot && (
        <section className="detail-section">
          <h3>Copilot 利用状況</h3>
          <dl className="detail-grid">
            <dt>保持されているセッション</dt>
            <dd>{p.copilot.session_count} 件</dd>
            <dt>最終利用</dt>
            <dd>{relativeTime(p.copilot.last_used_at, now)}</dd>
            <dt>最後のタイトル</dt>
            <dd>{p.copilot.last_title ?? '—'}</dd>
          </dl>
          {fallback && <p className="note-inline">{fallback}</p>}
          {/* ADR-0019: 集計対象は Copilot CLI の履歴のみ。VS Code 拡張は対象外 */}
          <p className="muted note-inline">集計対象は Copilot CLI の履歴のみです。</p>
        </section>
      )}

      <section className="detail-section">
        <h3>手動調整</h3>
        <form
          className="override-form"
          onSubmit={(e) => {
            e.preventDefault()
            void save()
          }}
        >
          {/* プレースホルダには「空欄のときに使われる値」を出す。
              上書きが無いときの表示名はフォルダ名、コマンドは自動解決、作業 dir は判定結果 */}
          <label>
            表示名
            <input
              value={displayName}
              placeholder={`(空欄なら ${folderName(p.root_path) ?? p.display_name})`}
              onChange={(e) => setDisplayName(e.target.value)}
            />
          </label>
          <label>
            起動コマンド上書き
            <input
              value={commandOverride}
              placeholder={
                ov?.command_override
                  ? '(空欄にすると上書きを解除します)'
                  : `(空欄なら自動解決: ${p.resolved_command ?? '候補なし'})`
              }
              onChange={(e) => setCommandOverride(e.target.value)}
            />
          </label>
          <label>
            作業ディレクトリ上書き
            <input
              value={workingDirOverride}
              placeholder={
                ov?.working_dir_override
                  ? '(空欄にすると上書きを解除します)'
                  : `(空欄なら判定結果: ${p.working_dir})`
              }
              onChange={(e) => setWorkingDirOverride(e.target.value)}
            />
            {/* FR-P-32: 実在しないディレクトリはバックエンドが拒否して DB に書かない。
                その理由をこのフィールドの直下に出す */}
            {fieldError?.field === 'working_dir_override' && (
              <p className="note-inline error">{fieldError.message}</p>
            )}
          </label>
          <label>
            並び順
            <input
              type="number"
              value={sortOrder}
              onChange={(e) => setSortOrder(e.target.value)}
            />
          </label>
          <label className="checkbox-label">
            <input type="checkbox" checked={hidden} onChange={(e) => setHidden(e.target.checked)} />
            非表示
          </label>
          <label className="checkbox-label">
            <input
              type="checkbox"
              checked={archived}
              onChange={(e) => setArchived(e.target.checked)}
            />
            アーカイブ
          </label>
          {fieldError && fieldError.field !== 'working_dir_override' && (
            <p className="note-inline error">{fieldError.message}</p>
          )}
          <button type="submit" disabled={saving}>
            {saving ? '保存中…' : '保存'}
          </button>
        </form>
      </section>
    </div>
  )
}

/** パスの末尾要素 (フォルダ名)。末尾の区切りは無視する */
function folderName(path: string): string | null {
  const parts = path.split(/[\\/]+/).filter((s) => s.length > 0)
  return parts[parts.length - 1] ?? null
}
