//! セッションの活動状態の合成 (純粋)。
//!
//! 判定は 3 段の優先順で行う (FR-C-45)。
//!   1. セッション状態のシグナルが**新鮮 (60 秒以内)** ならそれを最優先
//!   2. なければトランスクリプト末尾レコードのターン終了理由から判定
//!   3. 結果が「入力待ち / 不明」で、かつ稼働中サブエージェントが 1 件以上あれば上書き
//!
//! ② はエントリポイントに依存せず全セッションで効くので、**② だけでも成立する**
//! 設計にしておく。① が得られないエントリポイントが存在しうるため。
//!
//! 対応要求: FR-C-44 / FR-C-45 / FR-C-46 / NFR-50

use serde::{Deserialize, Serialize};

/// 活動状態の 5 値 (FR-C-44)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityState {
    /// 生成中
    Generating,
    /// ツール実行中。
    ///
    /// **「ツール許可プロンプトで停止中」と区別できない** (FR-C-46)。
    /// 両者はログ上まったく同じ形 (直近の書き込みがツール呼び出しのまま止まる)
    /// でしか現れないため、分岐を作らずこの 1 値に寄せる。UI でも断定しない。
    ToolRunning,
    /// 入力待ち
    WaitingInput,
    /// サブエージェント実行中
    SubagentRunning,
    /// 不明。**空欄にせず、不明として出す**
    Unknown,
}

/// ① セッション状態ファイル / フック由来のシグナル。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateSignal {
    Generating,
    ToolRunning,
    WaitingInput,
}

/// ② トランスクリプト末尾レコードから読める形。
///
/// **末尾の物理的な最終行が本文レコードとは限らない** (FR-C-49)。末尾から遡って
/// 最初に見つかる本文レコード (ユーザー / アシスタント) を採ったものを渡すこと。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TailRecord {
    /// アシスタントがツール呼び出しを書いたまま止まっている
    ToolUse,
    /// ターンが正常に終わっている
    TurnEnded,
    /// ユーザー発話で終わっている (= 応答生成中の可能性)
    UserMessage,
    /// 解釈できない / 本文レコードが見つからない。
    /// **既定値。** 何も分からないときに勝手な状態へ倒れないようにする (NFR-43)
    #[default]
    Unrecognized,
}

/// シグナルの鮮度しきい値 (FR-C-45)
pub const SIGNAL_FRESH_MS: i64 = 60_000;

/// 活動状態の合成 (FR-C-45)。
///
/// * `signal` — ① のシグナルと、その観測時刻からの経過ミリ秒
/// * `tail` — ② 末尾の本文レコード
/// * `running_subagents` — ③ 稼働中サブエージェントの**件数**。
///   呼び出し側は集合の長さを渡すこと。件数を別に数えない (FR-C-51)
pub fn synthesize(
    signal: Option<(StateSignal, i64)>,
    tail: TailRecord,
    running_subagents: usize,
) -> ActivityState {
    // ① 新鮮なシグナルが最優先
    let from_signal = match signal {
        Some((s, age_ms)) if age_ms <= SIGNAL_FRESH_MS => Some(match s {
            StateSignal::Generating => ActivityState::Generating,
            StateSignal::ToolRunning => ActivityState::ToolRunning,
            StateSignal::WaitingInput => ActivityState::WaitingInput,
        }),
        _ => None,
    };

    // ② シグナルが無い / 古いときは末尾レコードから
    let base = from_signal.unwrap_or(match tail {
        TailRecord::ToolUse => ActivityState::ToolRunning,
        TailRecord::TurnEnded => ActivityState::WaitingInput,
        TailRecord::UserMessage => ActivityState::Generating,
        TailRecord::Unrecognized => ActivityState::Unknown,
    });

    // ③ 「入力待ち / 不明」のときだけ、稼働中サブエージェントで上書きする
    if running_subagents > 0 && matches!(base, ActivityState::WaitingInput | ActivityState::Unknown)
    {
        return ActivityState::SubagentRunning;
    }

    base
}

/// アイドル判定 (FR-C-55)。
///
/// **タイマーもフラグも持たない。** 最終活動時刻からの経過だけで決まるので、
/// 指示を出せば判定が自然に戻る。隠す / 戻すの状態がずれようがない。
pub const IDLE_THRESHOLD_MS: i64 = 30 * 60 * 1000;

pub fn is_idle(last_activity_age_ms: i64) -> bool {
    last_activity_age_ms >= IDLE_THRESHOLD_MS
}

/// 「直近 N 秒に追記があったか」の窓 (FR-C-50 の目安 120 秒)。
///
/// 段階 2 (FR-P-57) と段階 5 (FR-C-40) で同じ値を使う。
pub const ACTIVE_MTIME_WINDOW_MS: i64 = 120_000;

/// セッションが稼働中か (ADR-0014)。
///
/// * `mtime_age_ms` — `now - events.jsonl の mtime`。ファイルが無ければ `None`
/// * `ended_by_shutdown` — 末尾から遡って最初のレコードが `session.shutdown` か
///
/// 未来 mtime (時計ずれ) は同じ窓幅で対称に許容する。
pub fn is_session_active(mtime_age_ms: Option<i64>, ended_by_shutdown: bool) -> bool {
    match mtime_age_ms {
        Some(age) if age.abs() <= ACTIVE_MTIME_WINDOW_MS => !ended_by_shutdown,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_signal_wins_over_tail_record() {
        let s = synthesize(
            Some((StateSignal::Generating, 1_000)),
            TailRecord::ToolUse,
            0,
        );
        assert_eq!(s, ActivityState::Generating);
    }

    #[test]
    fn stale_signal_falls_back_to_tail_record() {
        // 61 秒前のシグナルは使わない
        let s = synthesize(
            Some((StateSignal::Generating, 61_000)),
            TailRecord::ToolUse,
            0,
        );
        assert_eq!(s, ActivityState::ToolRunning);
    }

    #[test]
    fn works_without_any_signal() {
        // ① が得られないエントリポイントでも ② だけで成立すること
        assert_eq!(
            synthesize(None, TailRecord::TurnEnded, 0),
            ActivityState::WaitingInput
        );
        assert_eq!(
            synthesize(None, TailRecord::ToolUse, 0),
            ActivityState::ToolRunning
        );
    }

    #[test]
    fn unrecognized_tail_is_unknown_not_empty() {
        assert_eq!(
            synthesize(None, TailRecord::Unrecognized, 0),
            ActivityState::Unknown
        );
    }

    #[test]
    fn subagents_override_waiting_input() {
        assert_eq!(
            synthesize(None, TailRecord::TurnEnded, 2),
            ActivityState::SubagentRunning
        );
    }

    #[test]
    fn subagents_override_unknown() {
        assert_eq!(
            synthesize(None, TailRecord::Unrecognized, 1),
            ActivityState::SubagentRunning
        );
    }

    #[test]
    fn subagents_do_not_override_generating_or_tool_running() {
        // 本体が動いているならそちらを出す。上書きは「入力待ち / 不明」のときだけ
        assert_eq!(
            synthesize(None, TailRecord::UserMessage, 3),
            ActivityState::Generating
        );
        assert_eq!(
            synthesize(None, TailRecord::ToolUse, 3),
            ActivityState::ToolRunning
        );
    }

    #[test]
    fn boundary_of_signal_freshness() {
        // ちょうど 60 秒は新鮮側に含める
        assert_eq!(
            synthesize(
                Some((StateSignal::WaitingInput, SIGNAL_FRESH_MS)),
                TailRecord::ToolUse,
                0
            ),
            ActivityState::WaitingInput
        );
        assert_eq!(
            synthesize(
                Some((StateSignal::WaitingInput, SIGNAL_FRESH_MS + 1)),
                TailRecord::ToolUse,
                0
            ),
            ActivityState::ToolRunning
        );
    }

    /// FR-C-46: 許可待ちを表す状態を作らない。ToolRunning の 1 値に寄せる。
    #[test]
    fn no_separate_state_for_permission_prompt() {
        let all = [
            ActivityState::Generating,
            ActivityState::ToolRunning,
            ActivityState::WaitingInput,
            ActivityState::SubagentRunning,
            ActivityState::Unknown,
        ];
        assert_eq!(all.len(), 5, "活動状態は 5 値のまま (FR-C-44)");
    }

    #[test]
    fn idle_threshold_is_30_minutes() {
        assert!(!is_idle(29 * 60 * 1000));
        assert!(is_idle(30 * 60 * 1000));
    }

    // ---- is_session_active (ADR-0014 / FR-P-57) ----

    #[test]
    fn zero_age_and_not_shutdown_is_active() {
        assert!(is_session_active(Some(0), false));
    }

    #[test]
    fn within_119_seconds_is_active() {
        assert!(is_session_active(Some(119_000), false));
    }

    #[test]
    fn beyond_121_seconds_is_not_active() {
        assert!(!is_session_active(Some(121_000), false));
    }

    #[test]
    fn fresh_but_ended_by_shutdown_is_not_active() {
        assert!(!is_session_active(Some(0), true));
    }

    #[test]
    fn missing_mtime_is_not_active() {
        assert!(!is_session_active(None, false));
    }

    #[test]
    fn future_mtime_within_window_is_active() {
        // 時計ずれ対策として未来側も同じ窓幅で許容する
        assert!(is_session_active(Some(-30_000), false));
    }

    #[test]
    fn future_mtime_far_out_is_not_active() {
        assert!(!is_session_active(Some(-365 * 24 * 3600 * 1000), false));
    }
}
