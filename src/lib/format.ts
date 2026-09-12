// 表示の整形 (純粋)。
//
// ★ **取れない値を「0」や「—」だけで埋めない。**取得不可は取得不可として
// 表現する (NFR-40 / 43 / 44)。この方針をここの関数群で守る。

import type { AppError, Entrypoint, QuotaGauge, QuotaSource } from '../types/dto'

/** 時刻を絶対時刻の短い文字列にする (タイムライン・ガント表示用)。 */
export function absoluteTime(ms: number | null): string {
  if (ms === null) return '不明'
  return new Date(ms).toLocaleString('ja-JP')
}

/** 相対時刻。`null` は「一度も無い」であって「0 秒前」ではない。 */
export function relativeTime(ms: number | null, now: number): string {
  if (ms === null) return 'なし'
  const diff = now - ms
  if (diff < 0) return 'たった今'
  const sec = Math.floor(diff / 1000)
  if (sec < 60) return `${sec} 秒前`
  const min = Math.floor(sec / 60)
  if (min < 60) return `${min} 分前`
  const hour = Math.floor(min / 60)
  if (hour < 24) return `${hour} 時間前`
  const day = Math.floor(hour / 24)
  if (day < 30) return `${day} 日前`
  const month = Math.floor(day / 30)
  if (month < 12) return `${month} か月前`
  return `${Math.floor(month / 12)} 年前`
}

/** 稼働時間。 */
export function duration(fromMs: number | null, toMs: number): string {
  if (fromMs === null) return '不明'
  const sec = Math.max(0, Math.floor((toMs - fromMs) / 1000))
  const h = Math.floor(sec / 3600)
  const m = Math.floor((sec % 3600) / 60)
  const s = sec % 60
  if (h > 0) return `${h}時間${m}分`
  if (m > 0) return `${m}分${s}秒`
  return `${s}秒`
}

/** トークン数の短縮表記。 */
export function compactNumber(n: number): string {
  if (n < 1000) return String(n)
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`
  return `${(n / 1_000_000).toFixed(1)}M`
}

/** nano-AIU から AI Credit へ (用語定義)。 */
export const NANO_AIU_PER_CREDIT = 1_000_000_000

export function creditsFromNanoAiu(nanoAiu: number | null): number | null {
  return nanoAiu === null ? null : nanoAiu / NANO_AIU_PER_CREDIT
}

/**
 * ゲージの本文。
 *
 * **率だけでなく消費量も併記する** (FR-C-89)。ただし単価 (1 AI Credit が何ドルか)
 * は実行時にも取得不可であることが実測で確定しているため、**`$` への換算は一切
 * 表示しない** (ADR-0010)。`used` / `entitlement` は AI Credits 単位の数値であり
 * ドルではないので、単位を明示するラベルを付ける (FR-C-100 の用語に合わせる)。
 * 取れていない値は数字を作らない。
 */
export function gaugeText(g: QuotaGauge): string {
  if (g.origin.source === 'unavailable') return '取得できませんでした'
  if (g.unlimited) return '無制限'
  if (g.used === null || g.entitlement === null) {
    return g.used_pct === null ? '不明' : `${g.used_pct.toFixed(0)}%`
  }
  const pct = g.used_pct === null ? '' : ` (${g.used_pct.toFixed(0)}%)`
  if (g.kind === 'monthly_credits') {
    return `${g.used.toFixed(2)} / ${g.entitlement.toFixed(2)} AI Credits${pct}`
  }
  return `${g.used.toLocaleString()} / ${g.entitlement.toLocaleString()}${pct}`
}

/**
 * 「適用外」(FR-C-131 / ADR-0015)。**「無制限」とは別状態。**
 * 無制限 = 上限が無い。適用外 = そもそも対象プランにこの枠が無い。
 * 率も危険色も出さない。
 */
export function quotaNotApplicableText(): string {
  return '対象プランでは利用できません'
}

/** 出所ラベル。**推定を実測であるかのように見せない** (NFR-40)。 */
export function sourceLabel(origin: QuotaSource): string {
  switch (origin.source) {
    case 'actual':
      return origin.via === 'sdk' ? '実値 (SDK)' : '実値 (API)'
    case 'estimated':
      return '推定'
    case 'unavailable':
      return '取得不可'
  }
}

/**
 * ヘッダーに出す「利用枠の取得経路」(FR-C-160)。
 *
 * 個々のゲージは枠ごとに出所が独立している (FR-C-84) が、ヘッダーは要約として
 * 1 つのラベルを出す。実際に率を持つ枠 (`has_quota`) を優先し、無ければ先頭を使う。
 */
export function quotaRouteLabel(quota: QuotaGauge[] | null): string {
  if (quota === null || quota.length === 0) return '取得不可'
  const applicable = quota.find((g) => g.has_quota) ?? quota[0]
  if (!applicable) return '取得不可'
  return sourceLabel(applicable.origin)
}

/** 「推定」の但し書き。ラベルだけでは足りない (FR-C-82)。 */
export function sourceNote(origin: QuotaSource): string | null {
  switch (origin.source) {
    case 'estimated':
      return `過去実績との相対値です (${origin.basis})。提供元が課金している実際の消費率ではありません。`
    case 'unavailable':
      return origin.how_to_fix
    case 'actual':
      return null
  }
}

/** しきい値による色分け (FR-C-88)。取れていない枠は「安全」ではなく「通常」。 */
export type Severity = 'normal' | 'warning' | 'danger'

export function severity(usedPct: number | null): Severity {
  if (usedPct === null) return 'normal'
  if (usedPct >= 90) return 'danger'
  if (usedPct >= 70) return 'warning'
  return 'normal'
}

/** 観測が古いか (FR-C-87)。目安 15 分。 */
export const STALE_THRESHOLD_MS = 15 * 60 * 1000

export function isStale(origin: QuotaSource, now: number): boolean {
  if (origin.source === 'unavailable') return false
  return now - origin.observed_at > STALE_THRESHOLD_MS
}

/** 活動状態の文言。**区別できないものを断定しない** (FR-C-46 / NFR-42)。 */
export function activityLabel(
  activity: 'generating' | 'tool_running' | 'waiting_input' | 'subagent_running' | 'unknown'
): string {
  switch (activity) {
    case 'generating':
      return '生成中'
    case 'tool_running':
      // 「許可待ち」と断定しない。補足はツールチップへ
      return 'ツール実行中'
    case 'waiting_input':
      return '入力待ち'
    case 'subagent_running':
      return 'サブエージェント実行中'
    case 'unknown':
      return '不明'
  }
}

/** エントリポイントの短いラベル (FR-C-53)。 */
export function entrypointLabel(entrypoint: Entrypoint): string {
  switch (entrypoint) {
    case 'cli_interactive':
      return 'CLI'
    case 'cli_background':
      return 'CLI (バックグラウンド)'
    case 'vscode':
      return 'VS Code'
    case 'coding_agent':
      return 'Coding Agent'
    case 'unknown':
      return '不明'
  }
}

/**
 * コンテキストウィンドウ使用率。**どちらかが無ければ null** (取得不可)。
 * 0 で埋めない (NFR-40 / 43)。
 */
export function contextUsagePct(used: number | null, limit: number | null): number | null {
  if (used === null || limit === null || limit <= 0) return null
  return (used / limit) * 100
}

/** 30 分アイドルのしきい値。バックエンドの `activity::IDLE_THRESHOLD_MS` と同じ値 (FR-C-55)。 */
export const IDLE_THRESHOLD_MS = 30 * 60 * 1000

/** アイドル判定。**状態を持たず、毎回この時刻差だけで決める。** 最終活動が無ければ稼働中扱い。 */
export function isSessionIdle(lastActivityAt: number | null, now: number): boolean {
  if (lastActivityAt === null) return false
  return now - lastActivityAt > IDLE_THRESHOLD_MS
}

export const ACTIVITY_TOOLTIP: Record<string, string> = {
  tool_running:
    'ツールを実行中か、ツールの許可を待っている状態です。ログ上ではこの 2 つを区別できません。',
}

/** 履歴の照合方法。**推測による紐付けを事実として提示しない** (FR-P-53 / NFR-41)。 */
export function matchedByLabel(matchedBy: 'exact' | 'folder_name_fallback'): string | null {
  return matchedBy === 'folder_name_fallback' ? '旧パスの履歴 (フォルダ名で照合)' : null
}

/**
 * IPC の `AppError` を人が読める文にする。
 *
 * **「何をすればよいか」がある (`how_to_fix` / `hint`) なら必ず添える**
 * (FR-C-83 / FR-P-73)。未知の形の値は文字列化して落とさない。
 */
export function describeError(e: unknown): string {
  if (typeof e !== 'object' || e === null) {
    return String(e)
  }
  if (!('kind' in e)) {
    // 未知の形。断定せずそのまま文字列化する
    return JSON.stringify(e)
  }
  const err = e as AppError
  switch (err.kind) {
    case 'not_found':
      return `見つかりません: ${err.what}`
    case 'invalid_input':
      return err.message
    case 'io':
      return err.message
    case 'db':
      return err.message
    case 'external':
      return err.hint ? `${err.message} — ${err.hint}` : err.message
    case 'unavailable':
      return err.how_to_fix ? `${err.reason} — ${err.how_to_fix}` : err.reason
    default:
      // 未知の形。断定せずそのまま文字列化する
      return JSON.stringify(err)
  }
}
