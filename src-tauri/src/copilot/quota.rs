//! 利用枠ビューの組み立てと降格判定。
//!
//! この節が満たすことは 1 つ: 「**契約している利用枠に対して、今どこまで
//! 使っているか**」が一目で分かること。取得手段は問わない。
//!
//! 純粋部分 (降格判定・率と色の算出・鮮度判定・推定の算出) だけをここに置く。
//! SDK の呼び出しと DB 走査は `quota_fetch.rs` に分ける。
//!
//! 対応要求: FR-C-80〜95 / FR-C-130〜144 / NFR-40 / NFR-45 / NFR-50

use serde::{Deserialize, Serialize};

/// 値の出所。**3 状態を型で強制する** (INV-7 / FR-C-81)。
///
/// 取れない値を 0 で埋めた瞬間、そのゲージは嘘になる。`Unavailable` は
/// エラーではなく正常な状態のひとつ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum QuotaSource {
    /// SDK / API 由来の実値
    Actual {
        via: ActualVia,
        /// **値が変化したときだけ更新される観測時刻** (FR-C-86)。
        /// 取得しに行った時刻ではない
        observed_at: i64,
    },
    /// 過去実績との相対値。**提供元が課金している実際の消費率ではない** (FR-C-82)
    Estimated { basis: String, observed_at: i64 },
    /// 取得不可。**何をすれば取れるようになるかを添える** (FR-C-83)
    Unavailable {
        reason: String,
        how_to_fix: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActualVia {
    Sdk,
    Rest,
}

/// しきい値による色分け (FR-C-88)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Normal,
    Warning,
    Danger,
}

/// 1 つの枠。**枠ごとに独立して評価する** (FR-C-84)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaGauge {
    pub kind: String,
    pub label: String,
    pub used: Option<f64>,
    /// `-1` = 無制限 (FR-C-131)
    pub entitlement: Option<f64>,
    /// **100 でクランプしない** (FR-C-90)
    pub used_pct: Option<f64>,
    /// 付与超過分。別建てで明示する (FR-C-90)
    pub overage: Option<f64>,
    pub unlimited: bool,
    pub reset_at: Option<i64>,
    pub origin: QuotaSource,
    /// `false` = この枠はそもそもプランに存在しない (適用外)。
    ///
    /// **率も色も出さない。** 実測で `premium_interactions` が
    /// `percent_remaining: 0` + `has_quota: false` を返した — そのままゲージに
    /// 載せると FR-C-88 の「90% 以上 = 危険」に該当して適用外の枠が真っ赤になる
    /// (ADR-0015)。取れなかった枠と区別が付くよう、既定は `true`
    pub has_quota: bool,
}

/// 観測を取り込む前の生の値。
#[derive(Debug, Clone, PartialEq)]
pub struct RawObservation {
    /// **`entitlement − quota_remaining` (小数) から出す。**
    /// `remaining` / `usedRequests` / `credits_used` は整数丸めで率と整合しない (ADR-0015)
    pub used: f64,
    pub entitlement: f64,
    /// 出所が返す残率 (`percent_remaining`)。**再計算しない** (ADR-0015)。
    /// `None` なら `used / entitlement` から出す
    pub remaining_pct: Option<f64>,
    pub reset_at: Option<i64>,
    /// 適用外の第一判定 (ADR-0015)
    pub has_quota: bool,
    /// 出所が `unlimited: true` を返したか。
    /// **`entitlement <= 0` と独立に持つ** — 実測で `entitlement: 0` かつ
    /// `unlimited: true` の枠が存在した (2026-09-12, plan=individual)
    pub unlimited: bool,
    /// **値が変化したときだけ更新される観測時刻** (FR-C-86)。
    /// 出所側の `timestamp_utc` ではない (値が動かなくても進むため)
    pub observed_at: i64,
}

impl RawObservation {
    /// 前回観測と**値が同じか** (FR-C-86)。
    ///
    /// `observed_at` は比較に入れない。出所側の観測時刻も入れない —
    /// `quota_remaining` が 198.4 のまま `timestamp_utc` だけが 3 回動いた実例がある。
    /// 出所の時刻で判定すると「取得しに行った時刻」と同じものになる。
    pub fn same_value(&self, other: &Self) -> bool {
        self.used == other.used
            && self.entitlement == other.entitlement
            && self.remaining_pct == other.remaining_pct
            && self.reset_at == other.reset_at
            && self.has_quota == other.has_quota
            && self.unlimited == other.unlimited
    }
}

/// FR-C-86: 値が変わったときだけ観測時刻を進める。
///
/// **定期取得は値の更新と無関係に走る。**取得時刻をそのまま鮮度に使うと、
/// 期限切れの古い値がいつまでも「最新」に見える。
pub fn settle_observed_at(prev: Option<&RawObservation>, next: &RawObservation, now: i64) -> i64 {
    match prev {
        Some(p) if p.same_value(next) => p.observed_at,
        _ => now,
    }
}

/// 率と超過の算出 (FR-C-90 / 131 / ADR-0015)。
///
/// 判定順が要点。**`unlimited` を `entitlement == 0` のガードより先に見る** —
/// 実測に `entitlement: 0` かつ `unlimited: true` の枠があり、順序を逆にすると
/// 無制限の枠が「付与 0」として出る。
///
/// - `has_quota == false` は適用外。率も超過も出さない
/// - `unlimited` (または `entitlement < 0`) は無制限。率を計算しない
/// - `entitlement == 0` は 0 除算ガード
/// - `used > entitlement` でも 100% でクランプしない。超過額を別に返す
pub fn compute_usage(
    used: f64,
    entitlement: f64,
    has_quota: bool,
    unlimited: bool,
) -> (Option<f64>, Option<f64>, bool) {
    if !has_quota {
        // 適用外。0% とも 100% とも言わない
        return (None, None, false);
    }
    if unlimited || entitlement < 0.0 {
        return (None, None, true);
    }
    if entitlement == 0.0 {
        // 付与 0 で 0 除算を作らない。率は出せない
        return (None, if used > 0.0 { Some(used) } else { None }, false);
    }
    let pct = used / entitlement * 100.0;
    let overage = if used > entitlement {
        Some(used - entitlement)
    } else {
        None
    };
    (Some(pct), overage, false)
}

/// 使用率のしきい値 (FR-C-88)。90% 以上 = 危険 / 70% 以上 = 警告。
pub fn severity(used_pct: Option<f64>) -> Severity {
    match used_pct {
        Some(p) if p >= 90.0 => Severity::Danger,
        Some(p) if p >= 70.0 => Severity::Warning,
        _ => Severity::Normal,
    }
}

/// 観測の鮮度が警告域か (FR-C-87)。目安 15 分。
pub const STALE_THRESHOLD_MS: i64 = 15 * 60 * 1000;

pub fn is_stale(observed_at: i64, now: i64) -> bool {
    now.saturating_sub(observed_at) > STALE_THRESHOLD_MS
}

/// リセット日時を過ぎた観測を除外する (FR-C-85)。
///
/// 期間がロールオーバー済みなら値は原理的に無意味。恣意的な時間しきい値より
/// 正確な基準になる。
pub fn is_rolled_over(reset_at: Option<i64>, now: i64) -> bool {
    matches!(reset_at, Some(r) if r <= now)
}

/// 取得経路の試行結果。降格の入力になる。
#[derive(Debug, Clone, PartialEq)]
pub enum FetchOutcome {
    Ok(RawObservation),
    /// 使えなかった。理由と対処を添える
    Failed {
        reason: String,
        how_to_fix: Option<String>,
    },
}

/// 経路 A→B→C の降格 (FR-C-136 / 143 / 144)。
///
/// **1 つの枠の中で実値と推定を混ぜない。** 上位が使えたら下位は見ない。
pub fn degrade(
    kind: &str,
    label: &str,
    sdk: Option<FetchOutcome>,
    rest: Option<FetchOutcome>,
    estimate: Option<(f64, String, i64)>,
    now: i64,
) -> QuotaGauge {
    let mut reasons: Vec<String> = Vec::new();
    let mut hint: Option<String> = None;

    for (outcome, via) in [(sdk, ActualVia::Sdk), (rest, ActualVia::Rest)] {
        match outcome {
            Some(FetchOutcome::Ok(obs)) => {
                // 期間がロールオーバー済みの観測は使わない (FR-C-85)
                if is_rolled_over(obs.reset_at, now) {
                    reasons.push("観測がリセット日時を過ぎています".to_string());
                    continue;
                }
                let (mut pct, overage, unlimited) =
                    compute_usage(obs.used, obs.entitlement, obs.has_quota, obs.unlimited);
                // ADR-0015: 出所が返す残率を再計算しない。同じ消費が丸めの異なる
                // 3 つの値で返るため、率は出所の `percent_remaining` をそのまま使う
                if let (Some(_), Some(rp)) = (pct, obs.remaining_pct) {
                    pct = Some(100.0 - rp);
                }
                return QuotaGauge {
                    kind: kind.to_string(),
                    label: label.to_string(),
                    used: Some(obs.used),
                    entitlement: Some(obs.entitlement),
                    used_pct: pct,
                    overage,
                    unlimited,
                    reset_at: obs.reset_at,
                    origin: QuotaSource::Actual {
                        via,
                        observed_at: obs.observed_at,
                    },
                    has_quota: obs.has_quota,
                };
            }
            Some(FetchOutcome::Failed { reason, how_to_fix }) => {
                reasons.push(reason);
                if hint.is_none() {
                    hint = how_to_fix;
                }
            }
            None => {}
        }
    }

    // 経路 C: 推定。**必ず「推定」と明示する** (FR-C-143)
    if let Some((pct, basis, observed_at)) = estimate {
        return QuotaGauge {
            kind: kind.to_string(),
            label: label.to_string(),
            used: None,
            entitlement: None,
            used_pct: Some(pct.clamp(0.0, 100.0)),
            overage: None,
            unlimited: false,
            reset_at: None,
            origin: QuotaSource::Estimated { basis, observed_at },
            // 推定経路は「この枠がプランにあるか」を知らない。断定しない
            has_quota: true,
        };
    }

    QuotaGauge {
        kind: kind.to_string(),
        label: label.to_string(),
        used: None,
        entitlement: None,
        used_pct: None,
        overage: None,
        unlimited: false,
        reset_at: None,
        origin: QuotaSource::Unavailable {
            reason: if reasons.is_empty() {
                "取得経路がありません".to_string()
            } else {
                reasons.join(" / ")
            },
            how_to_fix: hint,
        },
        // 取れなかっただけ。「プランに無い」と断定しない
        has_quota: true,
    }
}

/// nano-AIU から AI Credit へ。10^9 で割る (用語定義)。
pub const NANO_AIU_PER_CREDIT: f64 = 1_000_000_000.0;

pub fn credits_from_nano_aiu(nano_aiu: i64) -> f64 {
    nano_aiu as f64 / NANO_AIU_PER_CREDIT
}

// ---------------------------------------------------------------- 経路 C: 推定

/// 推定の比較窓 (時間)。「直近の期間」の長さ (FR-C-92)。
///
/// 24 時間 = 「今日の使い方 vs 過去 30 日で一番使った 24 時間」。
/// **調整つまみとして関数引数にも出してある** — 窓を変えると意味が変わるので、
/// 定数を書き換えるのではなく呼び出し側で決められるようにしてある。
pub const ESTIMATE_WINDOW_HOURS: i64 = 24;

/// 推定の母集団 (日)。FR-C-92 の「過去 30 日」。
pub const ESTIMATE_BASELINE_DAYS: i64 = 30;

/// FR-C-92: 「直近の期間のトークン合計 ÷ 過去 30 日を 1 時間刻みでスライドさせた
/// 最大合計」を 0〜100% にクランプする。
///
/// `buckets` は `(1 時間バケット番号, トークン数)`。順不同・欠損可。
///
/// **母数が 0 なら `None`。** 0% を「使っていない」として断定しない (NFR-43) —
/// 索引が空なだけかもしれない。
pub fn estimate_usage_pct(
    buckets: &[(i64, i64)],
    now_bucket: i64,
    window_hours: i64,
) -> Option<f64> {
    if window_hours <= 0 || buckets.is_empty() {
        return None;
    }
    let by_bucket: std::collections::HashMap<i64, i64> = buckets.iter().copied().collect();
    let oldest = buckets.iter().map(|(b, _)| *b).min()?;

    let window_sum = |anchor: i64| -> i64 {
        (0..window_hours)
            .filter_map(|d| by_bucket.get(&(anchor - d)))
            .sum()
    };

    // 窓は母集団の先頭から現在まで 1 時間刻みでスライドさせる
    let max = (oldest..=now_bucket).map(window_sum).max().unwrap_or(0);
    if max <= 0 {
        return None;
    }
    let recent = window_sum(now_bucket);
    Some((recent as f64 / max as f64 * 100.0).clamp(0.0, 100.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn obs(used: f64, ent: f64) -> RawObservation {
        RawObservation {
            used,
            entitlement: ent,
            remaining_pct: None,
            reset_at: Some(NOW + 86_400_000),
            has_quota: true,
            unlimited: false,
            observed_at: NOW - 60_000,
        }
    }

    fn failed(reason: &str) -> FetchOutcome {
        FetchOutcome::Failed {
            reason: reason.to_string(),
            how_to_fix: Some("gh auth login を実行してください".to_string()),
        }
    }

    #[test]
    fn sdk_success_is_actual() {
        let g = degrade(
            "m",
            "月次",
            Some(FetchOutcome::Ok(obs(6.2, 10.0))),
            None,
            None,
            NOW,
        );
        assert!(matches!(
            g.origin,
            QuotaSource::Actual {
                via: ActualVia::Sdk,
                ..
            }
        ));
        assert_eq!(g.used_pct, Some(62.0));
    }

    #[test]
    fn falls_back_to_rest_then_estimate() {
        let g = degrade(
            "m",
            "月次",
            Some(failed("SDK 未導入")),
            Some(FetchOutcome::Ok(obs(3.0, 10.0))),
            None,
            NOW,
        );
        assert!(matches!(
            g.origin,
            QuotaSource::Actual {
                via: ActualVia::Rest,
                ..
            }
        ));

        let g = degrade(
            "m",
            "月次",
            Some(failed("SDK 未導入")),
            Some(failed("403")),
            Some((42.0, "過去 30 日比".to_string(), NOW)),
            NOW,
        );
        assert!(matches!(g.origin, QuotaSource::Estimated { .. }));
    }

    /// FR-C-83: 取得不可には「何をすれば取れるか」が入っている
    #[test]
    fn unavailable_carries_how_to_fix() {
        let g = degrade(
            "m",
            "月次",
            Some(failed("未認証")),
            Some(failed("403")),
            None,
            NOW,
        );
        match g.origin {
            QuotaSource::Unavailable { reason, how_to_fix } => {
                assert!(reason.contains("未認証"));
                assert!(how_to_fix.is_some(), "対処が空だと画面に出せない");
            }
            other => panic!("Unavailable のはず: {other:?}"),
        }
        assert_eq!(g.used_pct, None, "取れない値を 0 で埋めてはいけない");
    }

    /// FR-C-84: ある枠が実値でも、別の枠に推定を混ぜない
    #[test]
    fn gauges_are_evaluated_independently() {
        let a = degrade(
            "a",
            "枠A",
            Some(FetchOutcome::Ok(obs(1.0, 10.0))),
            None,
            None,
            NOW,
        );
        let b = degrade(
            "b",
            "枠B",
            Some(failed("対象プランでない")),
            None,
            None,
            NOW,
        );
        assert!(matches!(a.origin, QuotaSource::Actual { .. }));
        assert!(matches!(b.origin, QuotaSource::Unavailable { .. }));
        assert_eq!(b.used_pct, None);
    }

    /// FR-C-131: -1 は無制限。0 除算もマイナス率も出さない
    #[test]
    fn unlimited_entitlement_has_no_percentage() {
        let (pct, overage, unlimited) = compute_usage(500.0, -1.0, true, false);
        assert_eq!(pct, None);
        assert_eq!(overage, None);
        assert!(unlimited);
    }

    /// 2026-09-12 実測 (plan=individual): `entitlement: 0` + `unlimited: true` +
    /// `has_quota: true` の枠が実在した。**0 除算ガードより先に無制限を見る**こと。
    /// 順序を逆にすると無制限の枠が「付与 0」として出る
    #[test]
    fn unlimited_wins_over_zero_entitlement_guard() {
        let (pct, overage, unlimited) = compute_usage(0.0, 0.0, true, true);
        assert_eq!(pct, None);
        assert_eq!(overage, None);
        assert!(unlimited, "entitlement 0 でも unlimited なら無制限");
    }

    /// ADR-0015: 適用外は `has_quota == false` が第一基準。
    /// `percent_remaining: 0` をそのままゲージに載せると真っ赤になる
    #[test]
    fn not_applicable_quota_produces_no_percentage() {
        let (pct, overage, unlimited) = compute_usage(0.0, 0.0, false, false);
        assert_eq!(pct, None);
        assert_eq!(overage, None);
        assert!(!unlimited);
        assert_eq!(severity(pct), Severity::Normal, "適用外を危険色にしない");

        let mut o = obs(0.0, 0.0);
        o.has_quota = false;
        o.remaining_pct = Some(0.0);
        let g = degrade("premium_interactions", "premium_interactions", Some(FetchOutcome::Ok(o)), None, None, NOW);
        assert!(!g.has_quota, "適用外は UI に伝える");
        assert_eq!(g.used_pct, None, "適用外の枠に率を出さない");
    }

    #[test]
    fn zero_entitlement_does_not_divide_by_zero() {
        let (pct, overage, unlimited) = compute_usage(3.0, 0.0, true, false);
        assert_eq!(pct, None);
        assert_eq!(overage, Some(3.0));
        assert!(!unlimited);
    }

    /// FR-C-90: 超過を 100% でクランプしない。超過分を別建てで返す
    #[test]
    fn overage_is_not_clamped() {
        let (pct, overage, _) = compute_usage(12.5, 10.0, true, false);
        assert_eq!(pct, Some(125.0));
        assert_eq!(overage, Some(2.5));
    }

    /// ADR-0015: 出所が返す `percent_remaining` を再計算しない。
    /// T-0.8 の実測値 (entitlement 200 / quota_remaining 198.4 / percent_remaining 99.2)
    #[test]
    fn remaining_pct_from_source_wins_over_division() {
        let mut o = obs(1.6, 200.0);
        o.remaining_pct = Some(99.2);
        let g = degrade("chat", "chat", Some(FetchOutcome::Ok(o)), None, None, NOW);
        let pct = g.used_pct.expect("率が出る");
        assert!((pct - 0.8).abs() < 1e-9, "出所の残率から出す: {pct}");
        assert_eq!(g.used, Some(1.6), "消費は小数の quota_remaining 由来");
    }

    /// FR-C-86: 値が同じなら観測時刻を進めない。
    /// `timestamp_utc` が動いても値が動いていなければ「新しい観測」ではない
    #[test]
    fn observed_at_only_moves_when_the_value_moves() {
        let prev = obs(1.6, 200.0);
        let same = obs(1.6, 200.0);
        assert_eq!(
            settle_observed_at(Some(&prev), &same, NOW),
            prev.observed_at,
            "値が同じなら取得時刻で上書きしない"
        );

        let changed = obs(2.4, 200.0);
        assert_eq!(settle_observed_at(Some(&prev), &changed, NOW), NOW);
        assert_eq!(settle_observed_at(None, &changed, NOW), NOW, "初回は今");
    }

    /// FR-C-86 の壊れ方の再現防止: 据え置いた観測時刻はちゃんと古びる
    #[test]
    fn unchanged_value_eventually_becomes_stale() {
        let prev = RawObservation {
            observed_at: NOW - 20 * 60 * 1000,
            ..obs(1.6, 200.0)
        };
        let settled = settle_observed_at(Some(&prev), &obs(1.6, 200.0), NOW);
        assert!(
            is_stale(settled, NOW),
            "20 分前の値を取りに行っただけで「最新」にしない"
        );
    }

    /// FR-C-92: 直近窓 ÷ 過去 30 日の最大窓。母数が無ければ推定しない
    #[test]
    fn estimate_is_relative_to_the_busiest_window() {
        // バケット 0..3 に 100 ずつ、直近 (100..101) に 50 ずつ。窓 2 時間
        let buckets = [(0i64, 100i64), (1, 100), (100, 50), (101, 50)];
        let pct = estimate_usage_pct(&buckets, 101, 2).expect("推定できる");
        assert!((pct - 50.0).abs() < 1e-9, "100/200 = 50%: {pct}");
    }

    #[test]
    fn estimate_returns_none_without_data() {
        assert_eq!(estimate_usage_pct(&[], 100, 24), None);
        assert_eq!(
            estimate_usage_pct(&[(10, 0)], 100, 24),
            None,
            "母数 0 を 0% と断定しない"
        );
    }

    #[test]
    fn estimate_is_clamped_to_0_100_at_the_peak() {
        let buckets = [(0i64, 10i64), (100, 999)];
        let pct = estimate_usage_pct(&buckets, 100, 1).expect("推定できる");
        assert_eq!(pct, 100.0, "自分が最大なら 100%");
    }

    /// FR-C-85: リセット日時を過ぎた観測は使わない
    #[test]
    fn rolled_over_observation_is_excluded() {
        let mut stale = obs(9.0, 10.0);
        stale.reset_at = Some(NOW - 1);
        let g = degrade("m", "月次", Some(FetchOutcome::Ok(stale)), None, None, NOW);
        assert!(
            matches!(g.origin, QuotaSource::Unavailable { .. }),
            "ロールオーバー済みの値を最新として出してはいけない"
        );
    }

    /// FR-C-86: 鮮度は観測時刻で見る。取得時刻ではない
    #[test]
    fn staleness_uses_observed_at() {
        assert!(!is_stale(NOW - 14 * 60 * 1000, NOW));
        assert!(is_stale(NOW - 16 * 60 * 1000, NOW));
    }

    /// FR-C-88: 90 / 70 の 3 段階
    #[test]
    fn severity_thresholds() {
        assert_eq!(severity(Some(69.9)), Severity::Normal);
        assert_eq!(severity(Some(70.0)), Severity::Warning);
        assert_eq!(severity(Some(89.9)), Severity::Warning);
        assert_eq!(severity(Some(90.0)), Severity::Danger);
        // 取れていない枠は「通常」であって「安全」ではない。色で断定しない
        assert_eq!(severity(None), Severity::Normal);
    }

    #[test]
    fn estimate_is_clamped_to_0_100() {
        let g = degrade(
            "m",
            "月次",
            None,
            None,
            Some((140.0, "過去 30 日比".to_string(), NOW)),
            NOW,
        );
        assert_eq!(g.used_pct, Some(100.0));
    }

    #[test]
    fn nano_aiu_conversion() {
        assert!((credits_from_nano_aiu(2_500_000_000) - 2.5).abs() < 1e-9);
    }
}
