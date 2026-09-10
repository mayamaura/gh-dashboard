import { describe, expect, it } from 'vitest'

import type { Project } from '../../types/dto'
import { defaultFilter, filterProjects, launchState, sortProjects } from '../projectList'

const project = (patch: Partial<Project>): Project => ({
  path_key: 'd:\\projects\\x',
  root_path: 'D:\\Projects\\x',
  working_dir: 'D:\\Projects\\x',
  display_name: 'x',
  kind: 'other',
  kind_label: 'その他',
  command_candidates: [],
  resolved_command: 'npm run dev',
  launch_blocked_reason: null,
  hidden: false,
  archived: false,
  sort_order: null,
  override_values: null,
  git: null,
  copilot: null,
  dev: { state: 'stopped' },
  ...patch,
})

describe('filterProjects', () => {
  it('既定では非表示を隠し、アーカイブも隠す', () => {
    const list = [
      project({ display_name: 'normal' }),
      project({ display_name: 'hidden', hidden: true }),
      project({ display_name: 'archived', archived: true }),
    ]
    expect(filterProjects(list, defaultFilter).map((p) => p.display_name)).toEqual(['normal'])
  })

  it('showHidden で非表示も出す', () => {
    const list = [project({ display_name: 'hidden', hidden: true })]
    expect(filterProjects(list, { ...defaultFilter, showHidden: true })).toHaveLength(1)
  })

  it('名前の部分一致は大文字小文字を無視する', () => {
    const list = [project({ display_name: 'GH-Dashboard' }), project({ display_name: 'notes' })]
    const got = filterProjects(list, { ...defaultFilter, text: 'dash' })
    expect(got.map((p) => p.display_name)).toEqual(['GH-Dashboard'])
  })

  it('種別フィルタが空なら全種別を通す', () => {
    const list = [project({ kind: 'tauri' }), project({ kind: 'nextjs' })]
    expect(filterProjects(list, defaultFilter)).toHaveLength(2)
    expect(filterProjects(list, { ...defaultFilter, kinds: ['tauri'] })).toHaveLength(1)
  })

  it('起動中のみで絞れる', () => {
    const list = [
      project({ display_name: 'run', dev: { state: 'running', url: null, pid: 1, started_at: 0 } }),
      project({ display_name: 'stop' }),
    ]
    const got = filterProjects(list, { ...defaultFilter, runningOnly: true })
    expect(got.map((p) => p.display_name)).toEqual(['run'])
  })

  // FR-P-13: 「その他」も一覧から隠さない
  it('種別が other でも隠さない', () => {
    const list = [project({ kind: 'other', display_name: 'misc' })]
    expect(filterProjects(list, defaultFilter)).toHaveLength(1)
  })
})

describe('sortProjects', () => {
  const withUsage = (name: string, lastUsedAt: number | null) =>
    project({
      display_name: name,
      copilot:
        lastUsedAt === null
          ? null
          : {
              session_count: 1,
              last_used_at: lastUsedAt,
              last_title: null,
              is_active: false,
              last_nano_aiu: null,
              lines_added: null,
              lines_removed: null,
              matched_by: 'exact',
            },
    })

  // FR-P-82: 履歴なしは末尾に沈める
  it('最終利用日時の降順、履歴なしは末尾', () => {
    const list = [withUsage('none', null), withUsage('old', 100), withUsage('new', 200)]
    expect(sortProjects(list, 'last_used').map((p) => p.display_name)).toEqual([
      'new',
      'old',
      'none',
    ])
  })

  it('同率は表示名昇順', () => {
    const list = [withUsage('b', 100), withUsage('a', 100)]
    expect(sortProjects(list, 'last_used').map((p) => p.display_name)).toEqual(['a', 'b'])
  })

  it('履歴なし同士も表示名昇順', () => {
    const list = [withUsage('z', null), withUsage('a', null)]
    expect(sortProjects(list, 'last_used').map((p) => p.display_name)).toEqual(['a', 'z'])
  })

  it('名前順と種別順', () => {
    const list = [
      project({ display_name: 'b', kind: 'nextjs' }),
      project({ display_name: 'a', kind: 'tauri' }),
    ]
    expect(sortProjects(list, 'name').map((p) => p.display_name)).toEqual(['a', 'b'])
    expect(sortProjects(list, 'kind').map((p) => p.kind)).toEqual(['nextjs', 'tauri'])
  })

  it('入力配列を破壊しない', () => {
    const list = [withUsage('b', 100), withUsage('a', 200)]
    const before = list.map((p) => p.display_name)
    sortProjects(list, 'last_used')
    expect(list.map((p) => p.display_name)).toEqual(before)
  })
})

describe('launchState', () => {
  // FR-P-22: 起動不可でも隠さず理由を出す
  it('起動不可の理由をそのまま返す', () => {
    const p = project({ launch_blocked_reason: 'このアプリ自身のソースです' })
    expect(launchState(p)).toEqual({
      canLaunch: false,
      reason: 'このアプリ自身のソースです',
    })
  })

  it('コマンドが解決できないときも理由を出す', () => {
    const p = project({ resolved_command: null })
    expect(launchState(p).canLaunch).toBe(false)
    expect(launchState(p).reason).toBeTruthy()
  })

  it('起動できるときは理由なし', () => {
    expect(launchState(project({}))).toEqual({ canLaunch: true, reason: null })
  })
})
