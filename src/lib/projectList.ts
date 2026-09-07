// プロジェクト一覧の絞り込みと並べ替え (純粋)。
//
// ★ **すべてフロント側で完結させる。**バックエンドに再問い合わせしない (FR-P-81)。

import type { Project, ProjectKind } from '../types/dto'

export interface ProjectFilter {
  /** 名前の部分一致 */
  text: string
  /** 種別フィルタ。空なら全種別 */
  kinds: ProjectKind[]
  /** 非表示のものも表示する */
  showHidden: boolean
  /** アーカイブを隠す */
  hideArchived: boolean
  /** 起動中のものだけ */
  runningOnly: boolean
}

export const defaultFilter: ProjectFilter = {
  text: '',
  kinds: [],
  showHidden: false,
  hideArchived: true,
  runningOnly: false,
}

/** 並べ替えキー (FR-P-82) */
export type SortKey = 'last_used' | 'name' | 'kind'

export function filterProjects(projects: Project[], f: ProjectFilter): Project[] {
  const needle = f.text.trim().toLowerCase()
  return projects.filter((p) => {
    if (!f.showHidden && p.hidden) return false
    if (f.hideArchived && p.archived) return false
    if (f.runningOnly && p.dev.state !== 'running') return false
    if (f.kinds.length > 0 && !f.kinds.includes(p.kind)) return false
    if (needle && !p.display_name.toLowerCase().includes(needle)) return false
    return true
  })
}

/**
 * 並べ替え (FR-P-82)。
 *
 * 既定は最終利用日時。**履歴なしは末尾に沈める** — 「触ったことがない」ものを
 * 「最近触った」ものより上に出さない。同率は表示名昇順。
 */
export function sortProjects(projects: Project[], key: SortKey): Project[] {
  const byName = (a: Project, b: Project) => a.display_name.localeCompare(b.display_name, 'ja')

  const sorted = [...projects]
  switch (key) {
    case 'name':
      sorted.sort(byName)
      break
    case 'kind':
      sorted.sort((a, b) => a.kind.localeCompare(b.kind) || byName(a, b))
      break
    case 'last_used':
      sorted.sort((a, b) => {
        const av = a.copilot?.last_used_at ?? null
        const bv = b.copilot?.last_used_at ?? null
        // 履歴なしは常に末尾
        if (av === null && bv === null) return byName(a, b)
        if (av === null) return 1
        if (bv === null) return -1
        return bv - av || byName(a, b)
      })
      break
  }
  return sorted
}

/** 一覧行に出す起動可否。**起動不可でも隠さず、理由を出す** (FR-P-22)。 */
export function launchState(p: Project): { canLaunch: boolean; reason: string | null } {
  if (p.launch_blocked_reason) return { canLaunch: false, reason: p.launch_blocked_reason }
  if (!p.resolved_command) {
    return { canLaunch: false, reason: '起動コマンドが見つかりません' }
  }
  return { canLaunch: true, reason: null }
}
