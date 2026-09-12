import { describe, expect, it } from 'vitest'

import { deriveCardExpanded } from '../sessionCard'

describe('deriveCardExpanded (FR-C-56)', () => {
  it('サブエージェント稼働中は自動展開する', () => {
    expect(deriveCardExpanded(1, false, undefined)).toBe(true)
  })

  it('サブエージェントが居なければ自動展開しない', () => {
    expect(deriveCardExpanded(0, false, undefined)).toBe(false)
  })

  it('直近のポーリングが失敗していれば自動展開を抑止する', () => {
    expect(deriveCardExpanded(1, true, undefined)).toBe(false)
  })

  it('ユーザーが手動で閉じたら、再びサブエージェントが動いていても閉じたままにする', () => {
    expect(deriveCardExpanded(1, false, false)).toBe(false)
  })

  it('ユーザーが手動で開いたら、サブエージェントが居なくても開いたままにする', () => {
    expect(deriveCardExpanded(0, false, true)).toBe(true)
  })
})
