//! 差分判定と末尾断片の切り捨て (純粋)。
//!
//! **mtime を入力に取らない。** PC 移行やバックアップ復元で複数ファイルが同一
//! mtime になることが実際にあり、mtime を信頼した判定は壊れる (FR-P-56)。
//! 判定材料は記録済みオフセット・記録済みサイズ・現在サイズの 3 つだけ。
//!
//! 対応要求: FR-C-03 / FR-C-05 / FR-C-06 / FR-C-15 / NFR-50

/// 差分判定の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delta {
    /// 記録済みオフセットから追記分だけを読む
    Continue { from: u64 },
    /// truncate / 入れ替わり。先頭からやり直す (FR-C-06)
    Reparse,
    /// 追記なし
    UpToDate,
}

/// 差分判定 (FR-C-03 / 06)。
///
/// - 現在サイズが記録済みサイズより **小さい** → 全再パース
/// - 現在サイズが記録済みオフセットより大きい → 継続
/// - それ以外 → 更新なし
pub fn decide(recorded_offset: u64, recorded_size: u64, current_size: u64) -> Delta {
    if current_size < recorded_size || current_size < recorded_offset {
        // ファイルが縮んだ = 別物になった可能性がある。オフセットは意味を失う。
        return Delta::Reparse;
    }
    if current_size > recorded_offset {
        return Delta::Continue {
            from: recorded_offset,
        };
    }
    Delta::UpToDate
}

/// 確定した行と、進めてよいバイト数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedLines<'a> {
    /// 改行で終わっていた行だけ (末尾の改行は含まない)
    pub lines: Vec<&'a [u8]>,
    /// この読み取りでオフセットを進めてよいバイト数
    pub consumed: usize,
}

/// 読み込んだバイト列から、**改行で終わる行だけ**を確定分として取り出す (FR-C-05)。
///
/// 追記中のファイルを読むと最終行が途中で切れている。切れた行を確定扱いすると
/// オフセットが進みすぎ、次回に本来の行を丸ごと飛ばす。
pub fn split_committed(buf: &[u8]) -> CommittedLines<'_> {
    let mut lines = Vec::new();
    let mut consumed = 0usize;
    let mut start = 0usize;

    for (i, &b) in buf.iter().enumerate() {
        if b != b'\n' {
            continue;
        }
        let mut end = i;
        // CRLF は CR を落として行にする (オフセットは改行まで進める)
        if end > start && buf[end - 1] == b'\r' {
            end -= 1;
        }
        lines.push(&buf[start..end]);
        consumed = i + 1;
        start = i + 1;
    }

    CommittedLines { lines, consumed }
}

/// 末尾ウィンドウ読みの先頭側の切れた行を落とす (FR-P-55)。
///
/// `split_committed` の鏡像。ファイルの途中 (末尾から一定バイト) から読むと、
/// 先頭の行が途中から始まっている可能性がある。その断片を解釈すると壊れた
/// レコードを拾ってしまうので、最初の改行までを切り捨てる。
///
/// `from_file_start` が `true` (読み取りがオフセット 0 から始まった) なら、
/// 先頭は正しい行頭なので何も落とさない。
pub fn drop_leading_partial(buf: &[u8], from_file_start: bool) -> &[u8] {
    if from_file_start {
        return buf;
    }
    match buf.iter().position(|&b| b == b'\n') {
        Some(i) => &buf[i + 1..],
        None => &[],
    }
}

/// 空行を落として、実際にパース対象になる行だけを返す。
pub fn non_empty(lines: &[&[u8]]) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.is_empty())
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_read_starts_from_zero() {
        assert_eq!(decide(0, 0, 1000), Delta::Continue { from: 0 });
    }

    #[test]
    fn appended_file_continues_from_offset() {
        assert_eq!(decide(500, 500, 1200), Delta::Continue { from: 500 });
    }

    #[test]
    fn unchanged_file_is_up_to_date() {
        assert_eq!(decide(500, 500, 500), Delta::UpToDate);
    }

    /// FR-C-06: サイズが記録より小さくなったら truncate / 入れ替わりとみなす
    #[test]
    fn shrunk_file_triggers_full_reparse() {
        assert_eq!(decide(500, 800, 300), Delta::Reparse);
        assert_eq!(decide(500, 500, 499), Delta::Reparse);
    }

    #[test]
    fn offset_beyond_current_size_triggers_reparse() {
        // 記録済みサイズが欠けていても、オフセットより小さければ再パース
        assert_eq!(decide(900, 0, 500), Delta::Reparse);
    }

    // ---- 末尾断片 (FR-C-05) ----

    #[test]
    fn complete_lines_are_all_committed() {
        let buf = b"{\"a\":1}\n{\"b\":2}\n";
        let r = split_committed(buf);
        assert_eq!(r.lines.len(), 2);
        assert_eq!(r.consumed, buf.len());
    }

    #[test]
    fn trailing_fragment_is_excluded() {
        let buf = b"{\"a\":1}\n{\"b\":2";
        let r = split_committed(buf);
        assert_eq!(r.lines.len(), 1);
        assert_eq!(r.lines[0], b"{\"a\":1}");
        // 書きかけの 2 行目の分はオフセットに含めない
        assert_eq!(r.consumed, 8);
    }

    #[test]
    fn line_without_newline_commits_nothing() {
        let r = split_committed(b"{\"a\":1}");
        assert!(r.lines.is_empty());
        assert_eq!(r.consumed, 0);
    }

    #[test]
    fn bare_newline_is_an_empty_line() {
        let r = split_committed(b"\n");
        assert_eq!(r.lines.len(), 1);
        assert_eq!(r.lines[0], b"");
        assert_eq!(r.consumed, 1);
        // 空行はパース対象から外れる
        assert!(non_empty(&r.lines).is_empty());
    }

    #[test]
    fn crlf_is_committed_without_cr() {
        let buf = b"{\"a\":1}\r\n{\"b\":2}\r\n";
        let r = split_committed(buf);
        assert_eq!(r.lines.len(), 2);
        assert_eq!(r.lines[0], b"{\"a\":1}");
        assert_eq!(r.consumed, buf.len());
    }

    #[test]
    fn empty_buffer_is_safe() {
        let r = split_committed(b"");
        assert!(r.lines.is_empty());
        assert_eq!(r.consumed, 0);
    }

    /// 冪等性の根拠: 確定分だけを消費すれば、次回は必ず行頭から読み始められる
    #[test]
    fn consumed_always_lands_on_a_line_boundary() {
        let buf = b"aa\nbb\ncc";
        let r = split_committed(buf);
        assert_eq!(&buf[r.consumed..], b"cc");
    }

    // ---- drop_leading_partial (FR-P-55) ----

    #[test]
    fn leading_fragment_before_first_newline_is_dropped() {
        let buf = b"artial}\n{\"b\":2}\n";
        assert_eq!(drop_leading_partial(buf, false), b"{\"b\":2}\n".as_slice());
    }

    #[test]
    fn leading_newline_exactly_at_start_drops_nothing_after_it() {
        let buf = b"\n{\"b\":2}\n";
        assert_eq!(drop_leading_partial(buf, false), b"{\"b\":2}\n".as_slice());
    }

    #[test]
    fn from_file_start_keeps_everything() {
        let buf = b"{\"a\":1}\n{\"b\":2}\n";
        assert_eq!(drop_leading_partial(buf, true), buf.as_slice());
    }

    #[test]
    fn no_newline_at_all_yields_empty_slice() {
        let buf = b"not a complete line";
        assert_eq!(drop_leading_partial(buf, false), b"".as_slice());
    }
}
