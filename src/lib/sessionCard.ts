// セッションカードのインライン系統図の開閉判定 (FR-C-56)。
//
// ★ 完全な入れ子系統図は段階 7 (`session_detail_get` / `tree::build`) で
// 差し替える前提の暫定実装。ここでは `running_subagent_ids` のフラット一覧を
// 展開するかどうかだけを決める。
//
// サブエージェント稼働中のカードは自動展開するが、
// - ユーザーが手動で閉じた場合
// - 直近のポーリングが失敗した場合
// は自動展開を抑止する。抑止しないと「自動展開→失敗→閉じる→自動展開」の
// 2 秒ループになる。

/**
 * @param subagentCount `running_subagent_ids.length`
 * @param failed 直近のポーリングが失敗したか (`useLivePoll` の `failed`)
 * @param manualOverride ユーザーが明示的に開閉した状態。無ければ `undefined`
 */
export function deriveCardExpanded(
  subagentCount: number,
  failed: boolean,
  manualOverride: boolean | undefined
): boolean {
  if (manualOverride !== undefined) return manualOverride
  return subagentCount > 0 && !failed
}
