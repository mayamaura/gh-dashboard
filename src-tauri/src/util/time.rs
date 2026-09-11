//! ISO 8601 日時文字列のパース (純粋)。
//!
//! Copilot のレコードは固定書式 (`YYYY-MM-DDTHH:MM:SS[.mmm]Z`) でしか
//! タイムスタンプを書かない。それ以外のオフセット表記や壊れた文字列を
//! 推測で補正せず `None` を返す (FR-P-56 / NFR-43)。
//!
//! 対応要求: FR-P-55 / FR-P-56 / NFR-50

/// "2026-09-07T17:14:22.666Z" / "...T17:14:22Z" → epoch ミリ秒。
///
/// `Z` 以外のオフセット、桁数違い、範囲外の値はすべて `None`。
pub fn parse_iso8601_ms(s: &str) -> Option<i64> {
    let s = s.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;

    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }
    if !(1..=9999).contains(&year) || !(1..=12).contains(&month) {
        return None;
    }
    let days_in_month = days_in_month(year, month);
    if !(1..=days_in_month).contains(&day) {
        return None;
    }

    // 秒とミリ秒を分離する (ミリ秒は任意)
    let (hms, millis) = match time.split_once('.') {
        Some((hms, frac)) => {
            if frac.len() != 3 || !frac.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            (hms, frac.parse::<i64>().ok()?)
        }
        None => (time, 0),
    };

    let mut hms_parts = hms.split(':');
    let hour: i64 = hms_parts.next()?.parse().ok()?;
    let minute: i64 = hms_parts.next()?.parse().ok()?;
    let second: i64 = hms_parts.next()?.parse().ok()?;
    if hms_parts.next().is_some() {
        return None;
    }
    if !(0..24).contains(&hour) || !(0..60).contains(&minute) || !(0..60).contains(&second) {
        return None;
    }

    let days = civil_days_from_epoch(year, month, day);
    let ms_of_day = ((hour * 60 + minute) * 60 + second) * 1000 + millis;
    Some(days * 86_400_000 + ms_of_day)
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Howard Hinnant の civil-days アルゴリズム。1970-01-01 を 0 とする通算日数。
fn civil_days_from_epoch(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m as i64 + 9) % 12; // [0, 11], 3月始まり
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_millis_and_matches_known_epoch() {
        // 2026-09-07T17:14:22.666Z の既知の epoch ms
        assert_eq!(
            parse_iso8601_ms("2026-09-07T17:14:22.666Z"),
            Some(1_788_801_262_666)
        );
    }

    #[test]
    fn parses_without_millis() {
        assert_eq!(
            parse_iso8601_ms("2026-09-07T17:14:22Z"),
            Some(1_788_801_262_000)
        );
    }

    #[test]
    fn non_utc_offset_is_rejected() {
        // Z 以外のオフセットは対応しない。推測で補正しない
        assert_eq!(parse_iso8601_ms("2026-09-07T17:14:22+09:00"), None);
    }

    #[test]
    fn leap_day_is_valid() {
        assert!(parse_iso8601_ms("2024-02-29T00:00:00Z").is_some());
        assert_eq!(parse_iso8601_ms("2023-02-29T00:00:00Z"), None);
    }

    #[test]
    fn empty_and_truncated_strings_are_rejected() {
        assert_eq!(parse_iso8601_ms(""), None);
        assert_eq!(parse_iso8601_ms("2026-09-07T17:14"), None);
        assert_eq!(parse_iso8601_ms("not a date"), None);
    }
}
