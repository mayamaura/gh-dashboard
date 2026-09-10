//! 利用枠ビューの組み立てと降格判定。
//!
//! この節が満たすことは 1 つ: 「**契約している利用枠に対して、今どこまで
//! 使っているか**」が一目で分かること。取得手段は問わない。
//!
//! 純粋部分 (降格判定・率と色の算出・鮮度判定) だけをここに置く。
//! SDK / REST の呼び出しは `fetch.rs` に分ける (未実装。OQ-06 待ち)。
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
}

/// 観測を取り込む前の生の値。
#[derive(Debug, Clone, PartialEq)]
pub struct RawObservation {
    pub used: f64,
    pub entitlement: f64,
    pub reset_at: Option<i64>,
    pub observed_at: i64,
}

/// 率と超過の算出 (FR-C-90 / 131)。
///
/// - `entitlement < 0` は無制限。率を計算しない (0 除算もマイナス率も出さない)
/// - `used > entitlement` でも 100% でクランプしない。超過額を別に返す
pub fn compute_usage(used: f64, entitlement: f64) -> (Option<f64>, Option<f64>, bool) {
    if entitlement < 0.0 {
        // 無制限
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
                let (pct, overage, unlimited) = compute_usage(obs.used, obs.entitlement);
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
    }
}

/// nano-AIU から AI Credit へ。10^9 で割る (用語定義)。
pub const NANO_AIU_PER_CREDIT: f64 = 1_000_000_000.0;

pub fn credits_from_nano_aiu(nano_aiu: i64) -> f64 {
    nano_aiu as f64 / NANO_AIU_PER_CREDIT
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn obs(used: f64, ent: f64) -> RawObservation {
        RawObservation {
            used,
            entitlement: ent,
            reset_at: Some(NOW + 86_400_000),
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
        let (pct, overage, unlimited) = compute_usage(500.0, -1.0);
        assert_eq!(pct, None);
        assert_eq!(overage, None);
        assert!(unlimited);
    }

    #[test]
    fn zero_entitlement_does_not_divide_by_zero() {
        let (pct, overage, unlimited) = compute_usage(3.0, 0.0);
        assert_eq!(pct, None);
        assert_eq!(overage, Some(3.0));
        assert!(!unlimited);
    }

    /// FR-C-90: 超過を 100% でクランプしない。超過分を別建てで返す
    #[test]
    fn overage_is_not_clamped() {
        let (pct, overage, _) = compute_usage(12.5, 10.0);
        assert_eq!(pct, Some(125.0));
        assert_eq!(overage, Some(2.5));
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
