//! 本日の使用状況とモデル別内訳の判定 (純粋)。
//!
//! **このモジュールは DB もファイルも触らない。** 呼び出し側 (`commands`) が
//! 行を渡す。判定・解釈を純粋関数に切り出してテストで固める (NFR-50)。
//!
//! 対応要求: FR-C-100〜105 / NFR-43 / NFR-51

/// 合成モデル (課金されない、ルーティング用の仮想モデル名) の一覧 (FR-C-104)。
///
/// # なぜ `auto` だけなのか (実データ 1,041 レコード / 51 セッション、2026-09-12)
///
/// | 観測 | 値 |
/// |---|---|
/// | `session.model_change.data.newModel` | **`"auto"` のみ** (ユーザーが選ぶ値) |
/// | `session.auto_mode_resolved.data.chosenModel` | `gpt-5-mini` / `claude-haiku-4.5` (実モデル) |
/// | `assistant.message` / `tool.*` / `subagent.*` の `data.model` | `gpt-5-mini` 146 件 / `claude-haiku-4.5` 125 件。**`auto` は 0 件** |
/// | `session.shutdown.data.modelMetrics` のキー | `gpt-5-mini` / `claude-haiku-4.5` のみ |
///
/// つまり `auto` は「ルーターに任せる」という指定であって、課金されるモデルでは
/// ない (ADR-0010 で `models.list()` が `auto` 1 件だけを返したのと同じもの)。
/// 課金の実体は `auto_mode_resolved` が選んだ実モデル側に載る。
///
/// **他の名前を推測で足さない。** 観測していない名前を除外リストに入れると、
/// 実在するモデルの消費が黙って消える (NFR-40 / NFR-43)。
pub const SYNTHETIC_MODELS: &[&str] = &["auto"];

/// 合成モデルか (FR-C-104)。判定は大文字小文字と前後空白を無視する。
pub fn is_synthetic_model(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    SYNTHETIC_MODELS.contains(&normalized.as_str())
}

/// ローカル日 0:00 起点の時間帯インデックス (FR-C-101)。
///
/// 日の範囲外は `None`。**0 に丸めない** — 丸めると前日の分が 0 時に積み上がる。
///
// ponytail: 夏時間で 1 日が 25 時間になる地域では 24 時台が落ちる。
// 24 要素の配列という DTO の形が先にあるので、そこは変えずに切り捨てる。
// 必要になったら `hourly_tokens` を可変長にして日の長さを返す
pub fn hour_bucket(timestamp_ms: i64, day_start_ms: i64) -> Option<usize> {
    if timestamp_ms < day_start_ms {
        return None;
    }
    let hour = (timestamp_ms - day_start_ms) / 3_600_000;
    (0..24).contains(&hour).then_some(hour as usize)
}

/// `turn_index` 1 行のうち、時間帯別集計に要るものだけ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnTokenRow {
    pub timestamp_ms: Option<i64>,
    pub model: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// 時間帯別集計の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HourlyAgg {
    /// 24 要素。ローカル日 0:00 起点
    pub hourly_tokens: Vec<i64>,
    /// 集計に入ったレコード件数
    pub turn_count: i64,
    /// 合成モデルとして除外した件数 (FR-C-104 / NFR-43)
    pub excluded_records: i64,
}

impl Default for HourlyAgg {
    fn default() -> Self {
        Self {
            hourly_tokens: vec![0; 24],
            turn_count: 0,
            excluded_records: 0,
        }
    }
}

/// レコード列を時間帯別に畳む (FR-C-101 / FR-C-104)。
///
/// - 合成モデルのレコードは**トークンも件数も足さず**、`excluded_records` に数える
/// - タイムスタンプが無い / 日の範囲外のレコードはトークンを足さない。
///   ただし `turn_count` には入れる (本日分として渡された行だから)
/// - **`input + output` を足す。キャッシュは含めない** (FR-C-101)。
///   実データではレコード単位の入力トークンが常に 0 なので実質は出力のみだが、
///   CLI 側が入力を載せるようになればそのまま効く
pub fn fold_hourly(rows: impl IntoIterator<Item = TurnTokenRow>, day_start_ms: i64) -> HourlyAgg {
    let mut agg = HourlyAgg::default();
    for row in rows {
        if row.model.as_deref().is_some_and(is_synthetic_model) {
            agg.excluded_records += 1;
            continue;
        }
        agg.turn_count += 1;
        if let Some(hour) = row.timestamp_ms.and_then(|t| hour_bucket(t, day_start_ms)) {
            agg.hourly_tokens[hour] += row.input_tokens + row.output_tokens;
        }
    }
    agg
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 1_788_800_000_000;

    #[test]
    fn auto_is_the_only_synthetic_model_we_observed() {
        assert!(is_synthetic_model("auto"));
        assert!(is_synthetic_model(" AUTO "));
        // 実在するモデルを巻き込まない
        assert!(!is_synthetic_model("gpt-5-mini"));
        assert!(!is_synthetic_model("claude-haiku-4.5"));
        // 観測していない名前を推測で除外しない (NFR-40)
        assert!(!is_synthetic_model("default"));
        assert!(!is_synthetic_model(""));
    }

    #[test]
    fn hour_bucket_covers_the_local_day_only() {
        assert_eq!(hour_bucket(DAY, DAY), Some(0));
        assert_eq!(hour_bucket(DAY + 3_599_999, DAY), Some(0));
        assert_eq!(hour_bucket(DAY + 3_600_000, DAY), Some(1));
        assert_eq!(hour_bucket(DAY + 23 * 3_600_000, DAY), Some(23));
        // 翌日・前日は落とす。0 に丸めない
        assert_eq!(hour_bucket(DAY + 24 * 3_600_000, DAY), None);
        assert_eq!(hour_bucket(DAY - 1, DAY), None);
    }

    fn row(ts: Option<i64>, model: Option<&str>, input: i64, output: i64) -> TurnTokenRow {
        TurnTokenRow {
            timestamp_ms: ts,
            model: model.map(|s| s.to_string()),
            input_tokens: input,
            output_tokens: output,
        }
    }

    #[test]
    fn sums_input_and_output_into_the_local_hour() {
        let agg = fold_hourly(
            [
                row(Some(DAY + 60_000), Some("gpt-5-mini"), 0, 10),
                row(Some(DAY + 120_000), Some("gpt-5-mini"), 5, 20),
                row(Some(DAY + 5 * 3_600_000), Some("claude-haiku-4.5"), 0, 7),
            ],
            DAY,
        );
        assert_eq!(agg.hourly_tokens[0], 35);
        assert_eq!(agg.hourly_tokens[5], 7);
        assert_eq!(agg.turn_count, 3);
        assert_eq!(agg.excluded_records, 0);
        assert_eq!(agg.hourly_tokens.len(), 24);
    }

    /// FR-C-104: 合成モデルはトークンも件数も足さず、除外件数として出す
    #[test]
    fn synthetic_model_records_are_excluded_and_counted() {
        let agg = fold_hourly(
            [
                row(Some(DAY + 1000), Some("auto"), 0, 999),
                row(Some(DAY + 1000), Some("gpt-5-mini"), 0, 1),
            ],
            DAY,
        );
        assert_eq!(agg.hourly_tokens[0], 1, "合成モデルの分を足さない");
        assert_eq!(agg.turn_count, 1);
        assert_eq!(agg.excluded_records, 1, "無言で落とさない (NFR-43)");
    }

    /// モデルが無いレコード (user.message 等) は除外対象ではない
    #[test]
    fn records_without_a_model_are_kept() {
        let agg = fold_hourly([row(Some(DAY + 1000), None, 0, 3)], DAY);
        assert_eq!(agg.hourly_tokens[0], 3);
        assert_eq!(agg.excluded_records, 0);
    }

    /// タイムスタンプが欠けても panic せず、件数には入る (NFR-23)
    #[test]
    fn record_without_timestamp_counts_but_lands_in_no_bucket() {
        let agg = fold_hourly([row(None, Some("gpt-5-mini"), 0, 42)], DAY);
        assert_eq!(agg.turn_count, 1);
        assert_eq!(agg.hourly_tokens.iter().sum::<i64>(), 0);
    }

    #[test]
    fn empty_input_yields_24_zero_buckets_not_an_empty_array() {
        let agg = fold_hourly(std::iter::empty(), DAY);
        assert_eq!(agg.hourly_tokens, vec![0; 24]);
        assert_eq!(agg.turn_count, 0);
    }
}
