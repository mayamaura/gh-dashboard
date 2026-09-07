//! dev サーバーの出力からの URL 抽出 (純粋)。
//!
//! **ポートは自動採番されるため、設定値からは決められない** (FR-P-63)。
//! 出力を読むしかない。
//!
//! 対応要求: FR-P-63 / NFR-50

/// 1 行から dev サーバーの URL を取り出す。見つからなければ `None`。
///
/// - ANSI エスケープと装飾記号に強い
/// - `0.0.0.0` はブラウザで開けないので `localhost` に読み替える
pub fn extract_url(line: &str) -> Option<String> {
    let clean = strip_ansi(line);
    let bytes = clean.as_bytes();

    let start = find_scheme(&clean)?;
    let mut end = start;
    while end < bytes.len() && !is_url_terminator(bytes[end]) {
        end += 1;
    }

    let raw = clean[start..end].trim_end_matches(['.', ',', ')', ']', '"', '\'']);
    if raw.len() < "http://a".len() {
        return None;
    }

    Some(normalize_host(raw))
}

fn find_scheme(s: &str) -> Option<usize> {
    let http = s.find("http://");
    let https = s.find("https://");
    match (http, https) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn is_url_terminator(b: u8) -> bool {
    b.is_ascii_whitespace() || matches!(b, b'<' | b'>' | b'|' | b'`')
}

/// ブラウザで開けないバインドアドレスを開ける形に読み替える。
fn normalize_host(url: &str) -> String {
    for bad in ["0.0.0.0", "[::]", "[::1]"] {
        if let Some(idx) = url.find(bad) {
            let mut out = String::with_capacity(url.len());
            out.push_str(&url[..idx]);
            out.push_str("localhost");
            out.push_str(&url[idx + bad.len()..]);
            return out;
        }
    }
    url.to_string()
}

/// ANSI エスケープシーケンスを落とす。
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // ESC [ ... 終端英字
        if chars.peek() == Some(&'[') {
            chars.next();
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_vite_local_url() {
        assert_eq!(
            extract_url("  Local:   http://localhost:5173/").as_deref(),
            Some("http://localhost:5173/")
        );
    }

    #[test]
    fn tolerates_decoration_characters() {
        assert_eq!(
            extract_url("  \u{27a4}  Local:   http://127.0.0.1:3000/").as_deref(),
            Some("http://127.0.0.1:3000/")
        );
    }

    #[test]
    fn strips_ansi_escapes() {
        let line = "\u{1b}[32m  Local:\u{1b}[39m   \u{1b}[36mhttp://localhost:5173/\u{1b}[39m";
        assert_eq!(extract_url(line).as_deref(), Some("http://localhost:5173/"));
    }

    /// 0.0.0.0 はブラウザで開けないので localhost に読み替える
    #[test]
    fn rewrites_wildcard_bind_address() {
        assert_eq!(
            extract_url("Running on http://0.0.0.0:8000").as_deref(),
            Some("http://localhost:8000")
        );
        assert_eq!(
            extract_url("Listening on http://[::]:4000").as_deref(),
            Some("http://localhost:4000")
        );
    }

    #[test]
    fn keeps_https() {
        assert_eq!(
            extract_url("Serving at https://localhost:5173").as_deref(),
            Some("https://localhost:5173")
        );
    }

    #[test]
    fn returns_none_for_ordinary_output() {
        assert_eq!(extract_url("Compiled successfully"), None);
        assert_eq!(extract_url(""), None);
        assert_eq!(extract_url("see the http docs"), None);
    }

    #[test]
    fn trims_trailing_punctuation() {
        assert_eq!(
            extract_url("open http://localhost:5173/.").as_deref(),
            Some("http://localhost:5173/")
        );
        assert_eq!(
            extract_url("(http://localhost:3000)").as_deref(),
            Some("http://localhost:3000")
        );
    }

    #[test]
    fn picks_the_first_url_on_the_line() {
        assert_eq!(
            extract_url("Local: http://localhost:5173/  Network: http://192.168.1.2:5173/")
                .as_deref(),
            Some("http://localhost:5173/")
        );
    }
}
