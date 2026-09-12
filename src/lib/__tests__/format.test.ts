import { describe, expect, it } from 'vitest'

import type { QuotaGauge, QuotaSource } from '../../types/dto'
import {
  activityLabel,
  compactNumber,
  contextUsagePct,
  creditsFromNanoAiu,
  describeError,
  duration,
  entrypointLabel,
  gaugeText,
  IDLE_THRESHOLD_MS,
  isSessionIdle,
  isStale,
  matchedByLabel,
  quotaNotApplicableText,
  relativeTime,
  severity,
  sourceLabel,
  sourceNote,
} from '../format'

const NOW = 1_800_000_000_000

const gauge = (patch: Partial<QuotaGauge>): QuotaGauge => ({
  kind: 'monthly_credits',
  label: '月次 AI Credits',
  used: null,
  entitlement: null,
  used_pct: null,
  overage: null,
  unlimited: false,
  reset_at: null,
  origin: { source: 'actual', via: 'sdk', observed_at: NOW },
  has_quota: true,
  ...patch,
})

describe('relativeTime', () => {
  it('null は「0 秒前」ではなく「なし」', () => {
    // 履歴が無いものを「たった今」と見せない (FR-P-58 / NFR-44)
    expect(relativeTime(null, NOW)).toBe('なし')
  })

  it('単位を繰り上げる', () => {
    expect(relativeTime(NOW - 5_000, NOW)).toBe('5 秒前')
    expect(relativeTime(NOW - 5 * 60_000, NOW)).toBe('5 分前')
    expect(relativeTime(NOW - 5 * 3_600_000, NOW)).toBe('5 時間前')
    expect(relativeTime(NOW - 5 * 86_400_000, NOW)).toBe('5 日前')
  })

  it('未来の時刻でも壊れない', () => {
    expect(relativeTime(NOW + 10_000, NOW)).toBe('たった今')
  })
})

describe('duration', () => {
  it('開始時刻が不明なら数字を作らない', () => {
    expect(duration(null, NOW)).toBe('不明')
  })

  it('時分秒を組み立てる', () => {
    expect(duration(NOW - 45_000, NOW)).toBe('45秒')
    expect(duration(NOW - 125_000, NOW)).toBe('2分5秒')
    expect(duration(NOW - 3_900_000, NOW)).toBe('1時間5分')
  })
})

describe('compactNumber', () => {
  it('桁で短縮する', () => {
    expect(compactNumber(999)).toBe('999')
    expect(compactNumber(1500)).toBe('1.5k')
    expect(compactNumber(2_100_000)).toBe('2.1M')
  })
})

describe('creditsFromNanoAiu', () => {
  it('10^9 で割る', () => {
    expect(creditsFromNanoAiu(2_500_000_000)).toBe(2.5)
  })

  it('null は null のまま。0 にしない', () => {
    expect(creditsFromNanoAiu(null)).toBeNull()
  })
})

describe('gaugeText', () => {
  // FR-C-89 / ADR-0010: 消費量と率を併記するが、単価が取得不可のため $ には換算しない
  it('AI Credits 枠は消費量と率を併記し、$ には換算しない', () => {
    const g = gauge({ used: 6.2, entitlement: 10, used_pct: 62 })
    expect(gaugeText(g)).toBe('6.20 / 10.00 AI Credits (62%)')
    expect(gaugeText(g)).not.toContain('$')
  })

  // FR-C-131: -1 は率ではなく「無制限」
  it('無制限は率を出さない', () => {
    expect(gaugeText(gauge({ unlimited: true, used: 500, entitlement: -1 }))).toBe('無制限')
  })

  // NFR-40: 取れていないものを 0% にしない
  it('取得不可は数字を作らない', () => {
    const g = gauge({
      origin: { source: 'unavailable', reason: '未認証', how_to_fix: 'gh auth login' },
    })
    expect(gaugeText(g)).toBe('取得できませんでした')
  })

  // FR-C-90: 100% でクランプしない
  it('超過を 100% に丸めない', () => {
    const g = gauge({ used: 12.5, entitlement: 10, used_pct: 125, overage: 2.5 })
    expect(gaugeText(g)).toContain('125%')
  })
})

describe('sourceLabel / sourceNote', () => {
  it('実値・推定・取得不可を区別する', () => {
    const actual: QuotaSource = { source: 'actual', via: 'sdk', observed_at: NOW }
    const est: QuotaSource = { source: 'estimated', basis: '過去 30 日比', observed_at: NOW }
    const un: QuotaSource = { source: 'unavailable', reason: '未認証', how_to_fix: 'gh auth login' }

    expect(sourceLabel(actual)).toBe('実値 (SDK)')
    expect(sourceLabel(est)).toBe('推定')
    expect(sourceLabel(un)).toBe('取得不可')
  })

  // FR-C-82: ラベルだけでなく但し書きも出す
  it('推定には「実際の消費率ではない」と添える', () => {
    const note = sourceNote({ source: 'estimated', basis: '過去 30 日比', observed_at: NOW })
    expect(note).toContain('実際の消費率ではありません')
  })

  // FR-C-83: 取得不可には対処を出す
  it('取得不可には対処を返す', () => {
    expect(
      sourceNote({ source: 'unavailable', reason: '未認証', how_to_fix: 'gh auth login' })
    ).toBe('gh auth login')
  })

  it('実値には但し書きを付けない', () => {
    expect(sourceNote({ source: 'actual', via: 'rest', observed_at: NOW })).toBeNull()
  })
})

describe('severity', () => {
  // FR-C-88: 90 / 70 の 3 段階
  it('しきい値どおりに分ける', () => {
    expect(severity(69.9)).toBe('normal')
    expect(severity(70)).toBe('warning')
    expect(severity(89.9)).toBe('warning')
    expect(severity(90)).toBe('danger')
  })

  it('取れていない枠を「安全」に見せない', () => {
    expect(severity(null)).toBe('normal')
  })
})

describe('isStale', () => {
  // FR-C-86: 鮮度は観測時刻で見る
  it('観測から 15 分を超えたら古い', () => {
    expect(isStale({ source: 'actual', via: 'sdk', observed_at: NOW - 14 * 60_000 }, NOW)).toBe(
      false
    )
    expect(isStale({ source: 'actual', via: 'sdk', observed_at: NOW - 16 * 60_000 }, NOW)).toBe(
      true
    )
  })

  it('取得不可には鮮度が無い', () => {
    expect(isStale({ source: 'unavailable', reason: 'x', how_to_fix: null }, NOW)).toBe(false)
  })
})

describe('activityLabel', () => {
  // FR-C-46 / NFR-42: 区別できないものを断定しない
  it('ツール実行中を「許可待ち」と断定しない', () => {
    const label = activityLabel('tool_running')
    expect(label).toBe('ツール実行中')
    expect(label).not.toContain('許可')
  })

  it('不明を空文字にしない', () => {
    expect(activityLabel('unknown')).toBe('不明')
  })
})

describe('describeError', () => {
  // FR-C-83 / FR-P-73: 「何をすればよいか」があれば必ず添える
  it('unavailable は reason と how_to_fix をつなげる', () => {
    expect(
      describeError({ kind: 'unavailable', reason: '未実装です (T-3.x)', how_to_fix: 'あとで再試行してください' })
    ).toBe('未実装です (T-3.x) — あとで再試行してください')
  })

  it('how_to_fix が無い unavailable は reason だけ', () => {
    expect(describeError({ kind: 'unavailable', reason: '未実装です', how_to_fix: null })).toBe(
      '未実装です'
    )
  })

  it('invalid_input は message を返す', () => {
    expect(
      describeError({ kind: 'invalid_input', field: 'working_dir_override', message: 'フォルダが存在しません' })
    ).toBe('フォルダが存在しません')
  })

  it('external は hint があればつなげる', () => {
    expect(
      describeError({ kind: 'external', tool: 'code', message: '起動に失敗しました', hint: 'PATH に code がありません' })
    ).toBe('起動に失敗しました — PATH に code がありません')
  })

  it('文字列はそのまま返す', () => {
    expect(describeError('何かのエラー')).toBe('何かのエラー')
  })

  it('未知の形のオブジェクトでも落ちずに文字列化する', () => {
    expect(describeError({ foo: 'bar' })).toBe('{"foo":"bar"}')
  })
})

describe('entrypointLabel', () => {
  it('種別ごとの短いラベルを返す', () => {
    expect(entrypointLabel('cli_interactive')).toBe('CLI')
    expect(entrypointLabel('vscode')).toBe('VS Code')
    expect(entrypointLabel('unknown')).toBe('不明')
  })
})

describe('contextUsagePct', () => {
  it('使用量と上限から率を出す', () => {
    expect(contextUsagePct(50_000, 200_000)).toBe(25)
  })

  // NFR-40 / 43: どちらかが無ければ数字を作らない
  it('どちらかが null なら null', () => {
    expect(contextUsagePct(null, 200_000)).toBeNull()
    expect(contextUsagePct(50_000, null)).toBeNull()
  })

  it('上限 0 は 0 除算にしない', () => {
    expect(contextUsagePct(0, 0)).toBeNull()
  })
})

describe('isSessionIdle (FR-C-55)', () => {
  it('30 分を超えたらアイドル', () => {
    expect(isSessionIdle(NOW - IDLE_THRESHOLD_MS - 1, NOW)).toBe(true)
    expect(isSessionIdle(NOW - IDLE_THRESHOLD_MS + 1, NOW)).toBe(false)
  })

  it('最終活動が無ければ稼働中扱い', () => {
    expect(isSessionIdle(null, NOW)).toBe(false)
  })
})

describe('quotaNotApplicableText (FR-C-131 / ADR-0015)', () => {
  it('率や無制限とは別の「適用外」文言を返す', () => {
    const text = quotaNotApplicableText()
    expect(text).not.toBe('無制限')
    expect(text).not.toMatch(/%/)
  })
})

describe('matchedByLabel', () => {
  // FR-P-53 / NFR-41: 推測による紐付けを事実として提示しない
  it('フォールバック照合には明示ラベルを返す', () => {
    expect(matchedByLabel('folder_name_fallback')).toBe('旧パスの履歴 (フォルダ名で照合)')
  })

  it('完全一致には余計なラベルを出さない', () => {
    expect(matchedByLabel('exact')).toBeNull()
  })
})
