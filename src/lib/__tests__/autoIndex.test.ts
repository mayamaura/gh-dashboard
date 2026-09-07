import { describe, expect, it } from 'vitest'

import { AUTO_INDEX_MIN_INTERVAL_MS, shouldAutoIndex } from '../../hooks/useLivePoll'

const NOW = 1_800_000_000_000

describe('shouldAutoIndex (FR-C-58 / 59)', () => {
  it('未取り込みの追記があるときだけ発火する', () => {
    // ログの mtime がインデックス完了時刻より新しい = 未取り込みあり
    expect(shouldAutoIndex(NOW - 1_000, NOW - 60_000, null, NOW)).toBe(true)
  })

  it('追記が無ければ発火しない', () => {
    expect(shouldAutoIndex(NOW - 60_000, NOW - 1_000, null, NOW)).toBe(false)
  })

  it('mtime とインデックス完了時刻が同じなら発火しない', () => {
    expect(shouldAutoIndex(NOW - 1_000, NOW - 1_000, null, NOW)).toBe(false)
  })

  it('下限間隔 10 秒を守る', () => {
    // 未取り込みはあるが、直前に試したばかり
    expect(shouldAutoIndex(NOW - 1_000, NOW - 60_000, NOW - 5_000, NOW)).toBe(false)
    expect(
      shouldAutoIndex(NOW - 1_000, NOW - 60_000, NOW - AUTO_INDEX_MIN_INTERVAL_MS - 1, NOW)
    ).toBe(true)
  })

  it('mtime が取れないときは発火しない', () => {
    // 取れないものを「更新あり」と決めつけない
    expect(shouldAutoIndex(null, NOW - 60_000, null, NOW)).toBe(false)
  })

  it('一度もインデックスしていなければ発火する', () => {
    expect(shouldAutoIndex(NOW - 1_000, null, null, NOW)).toBe(true)
  })
})
