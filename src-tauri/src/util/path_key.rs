//! パス正規化 (純粋)。
//!
//! `path_key` = 小文字化 + 区切りを `\` に統一 + 末尾区切り除去。
//! プロジェクトの一意キーであり、手動調整 (`project_overrides`) の主キーでもある。
//!
//! 対応要求: 用語定義 / FR-P-30 / FR-P-51 / NFR-50

/// パスを `path_key` に正規化する。
///
/// 空文字や空白のみは `None` を返す (パニックしない)。
pub fn path_key(path: &str) -> Option<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }

    let unified: String = trimmed
        .chars()
        .map(|c| if c == '/' { '\\' } else { c })
        .collect();

    // UNC (`\\server\share`) の先頭 2 つの区切りは保つ
    let (prefix, rest) = if let Some(stripped) = unified.strip_prefix("\\\\") {
        ("\\\\", stripped)
    } else {
        ("", unified.as_str())
    };

    // 連続する区切りを 1 つに畳む
    let mut collapsed = String::with_capacity(rest.len());
    let mut prev_sep = false;
    for ch in rest.chars() {
        let is_sep = ch == '\\';
        if is_sep && prev_sep {
            continue;
        }
        collapsed.push(ch);
        prev_sep = is_sep;
    }

    // 末尾区切りを落とす。ただしドライブルート (`d:\`) は落とすと別物になるので残す
    let trimmed_tail = collapsed.trim_end_matches('\\');
    let body = if trimmed_tail.is_empty() || is_drive_root(&collapsed) {
        collapsed.as_str()
    } else {
        trimmed_tail
    };

    let key = format!("{prefix}{body}").to_lowercase();
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

/// `d:\` のようなドライブルートか。
fn is_drive_root(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}

/// `path_key` から末尾のフォルダ名を取り出す。
///
/// フォルダ名フォールバック (FR-P-52) の照合に使う。
pub fn folder_name(key: &str) -> Option<&str> {
    let name = key.rsplit('\\').find(|s| !s.is_empty())?;
    if name.ends_with(':') {
        None
    } else {
        Some(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_case_and_separators() {
        assert_eq!(
            path_key("D:\\Projects\\Foo").as_deref(),
            Some("d:\\projects\\foo")
        );
        assert_eq!(
            path_key("D:/Projects/Foo").as_deref(),
            Some("d:\\projects\\foo")
        );
        assert_eq!(
            path_key("d:/PROJECTS/foo").as_deref(),
            Some("d:\\projects\\foo")
        );
    }

    #[test]
    fn strips_trailing_separator() {
        assert_eq!(
            path_key("D:\\Projects\\Foo\\").as_deref(),
            Some("d:\\projects\\foo")
        );
        assert_eq!(
            path_key("D:/Projects/Foo//").as_deref(),
            Some("d:\\projects\\foo")
        );
    }

    #[test]
    fn keeps_separator_for_drive_root() {
        // `d:` と `d:\` は別物なので、ここだけは落とさない
        assert_eq!(path_key("D:\\").as_deref(), Some("d:\\"));
    }

    #[test]
    fn preserves_unc_prefix() {
        assert_eq!(
            path_key("\\\\server\\share\\proj").as_deref(),
            Some("\\\\server\\share\\proj")
        );
        assert_eq!(
            path_key("//server/share/proj/").as_deref(),
            Some("\\\\server\\share\\proj")
        );
    }

    #[test]
    fn empty_input_returns_none() {
        assert_eq!(path_key(""), None);
        assert_eq!(path_key("   "), None);
    }

    #[test]
    fn extracts_folder_name() {
        assert_eq!(folder_name("d:\\projects\\foo"), Some("foo"));
        assert_eq!(folder_name("\\\\server\\share\\proj"), Some("proj"));
        // ドライブルートにはフォルダ名がない
        assert_eq!(folder_name("d:\\"), None);
    }
}
