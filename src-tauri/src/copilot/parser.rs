//! セッションレコードのパース (純粋)。
//!
//! `workspace.yaml` の簡易読み取り、一覧プレビューの生成、`events.jsonl` の
//! 末尾ウィンドウからの事実抽出を持つ。**このモジュールはファイルを読まない。**
//! 呼び出し側 (IO 層) がテキスト・行を渡す。
//!
//! 対応要求: FR-P-50〜58 / NFR-23 / NFR-43 / NFR-50

use crate::util::time::parse_iso8601_ms;

/// `workspace.yaml` の読み取り結果。全フィールドが欠落しうる前提 (NFR-23)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceMeta {
    pub cwd: Option<String>,
    pub client_name: Option<String>,
    /// 初回プロンプト本文。呼び出し側が必ず `preview()` を通すこと
    pub name: Option<String>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
}

/// `events.jsonl` の末尾ウィンドウから拾えた事実だけ。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TailFacts {
    pub last_timestamp: Option<i64>,
    /// 末尾から遡って最初にパースできたレコードの type が `session.shutdown` か (ADR-0014)
    pub ended_by_shutdown: bool,
    pub total_nano_aiu: Option<i64>,
    pub lines_added: Option<i64>,
    pub lines_removed: Option<i64>,
    /// パース失敗行。無言で落とさず数える (FR-C-12 / NFR-43)
    pub skipped_lines: usize,
    pub parsed_lines: usize,
}

/// `workspace.yaml` (実測 375 バイト、フラットなスカラー 9 キー) を読む。
///
/// YAML の完全実装をしない。列 0 の `キー: 値` だけを拾い、値の前後の
/// 引用符を 1 組だけ剥がす。ブロックスカラー (`|` / `>`)・複数行・入れ子は
/// 解釈せずそのキーを `None` にする (NFR-23)。
pub fn parse_workspace_yaml(text: &str) -> WorkspaceMeta {
    let mut meta = WorkspaceMeta::default();

    for line in text.lines() {
        // 列 0 (先頭が空白でない) だけを扱う。入れ子・継続行は無視する
        if line.starts_with(' ') || line.starts_with('\t') || line.trim().is_empty() {
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let raw_value = rest.trim();

        // ブロックスカラー (`|` / `>`) は解釈しない → そのキーは None のまま
        if raw_value == "|"
            || raw_value == ">"
            || raw_value.starts_with('|')
            || raw_value.starts_with('>')
        {
            continue;
        }

        let value = unquote(raw_value);
        let value = if value.is_empty() { None } else { Some(value) };

        match key {
            "cwd" => meta.cwd = value,
            "client_name" | "clientName" => meta.client_name = value,
            "name" => meta.name = value,
            "created_at" | "createdAt" => {
                meta.created_at = value.as_deref().and_then(parse_iso8601_ms)
            }
            "updated_at" | "updatedAt" => {
                meta.updated_at = value.as_deref().and_then(parse_iso8601_ms)
            }
            _ => {}
        }
    }

    meta
}

/// 値の前後についた一重の引用符 (`'...'` / `"..."`) を 1 組だけ剥がす。
fn unquote(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

/// 一覧行に出す 140 字プレビュー (FR-C-02 / INV-6)。
pub const PREVIEW_MAX_CHARS: usize = 140;

/// 改行・制御文字は空白に畳む。バイトではなく文字数で切る (UTF-8 を割らない)。
/// 空・空白のみは `None`。
pub fn preview(s: &str, max_chars: usize) -> Option<String> {
    let folded: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();

    // 連続する空白を 1 つに畳んでから前後をトリムする
    let mut out = String::with_capacity(folded.len());
    let mut prev_space = false;
    for c in folded.chars() {
        if c == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    let trimmed = out.trim();
    if trimmed.is_empty() {
        return None;
    }

    let truncated: String = trimmed.chars().take(max_chars).collect();
    Some(truncated)
}

/// 末尾ウィンドウの確定行列から事実を拾う。行は読み込み順 (古い→新しい) で渡す。
///
/// 1. 末尾から遡って最初にパースできたレコード → `last_timestamp` / `ended_by_shutdown`
/// 2. さらに遡って最初の `session.shutdown` → `total_nano_aiu` / codeChanges
/// 3. 窓内に `session.shutdown` が無ければそれらは `None`。窓を広げない
/// 4. パース失敗は数えて読み飛ばす
pub fn scan_tail(lines: &[&[u8]]) -> TailFacts {
    let mut facts = TailFacts::default();
    let mut found_last = false;
    let mut found_shutdown = false;

    for line in lines.iter().rev() {
        let text = match std::str::from_utf8(line) {
            Ok(t) if !t.trim().is_empty() => t,
            _ => continue,
        };
        let value: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => {
                facts.skipped_lines += 1;
                continue;
            }
        };
        facts.parsed_lines += 1;

        let record_type = value.get("type").and_then(|v| v.as_str());

        if !found_last {
            found_last = true;
            facts.last_timestamp = value
                .get("timestamp")
                .and_then(|v| v.as_str())
                .and_then(parse_iso8601_ms);
            facts.ended_by_shutdown = record_type == Some("session.shutdown");
        }

        if !found_shutdown && record_type == Some("session.shutdown") {
            found_shutdown = true;
            let data = value.get("data");
            facts.total_nano_aiu = data
                .and_then(|d| d.get("totalNanoAiu"))
                .and_then(|v| v.as_i64());
            let code_changes = data.and_then(|d| d.get("codeChanges"));
            facts.lines_added = code_changes
                .and_then(|c| c.get("linesAdded"))
                .and_then(|v| v.as_i64());
            facts.lines_removed = code_changes
                .and_then(|c| c.get("linesRemoved"))
                .and_then(|v| v.as_i64());
        }
    }

    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "\
cwd: D:\\Projects\\Foo
client_name: cli
name: \"Fix the bug\"
created_at: 2026-09-07T17:00:00.000Z
updated_at: 2026-09-07T17:14:22.666Z
model: gpt
session_id: abc-123
schema_version: 1
entrypoint: cli_interactive
";

    #[test]
    fn parses_flat_nine_keys() {
        let m = parse_workspace_yaml(FIXTURE);
        assert_eq!(m.cwd.as_deref(), Some("D:\\Projects\\Foo"));
        assert_eq!(m.client_name.as_deref(), Some("cli"));
        assert_eq!(m.name.as_deref(), Some("Fix the bug"));
        assert!(m.created_at.is_some());
        assert!(m.updated_at.is_some());
    }

    #[test]
    fn missing_key_stays_none() {
        let m = parse_workspace_yaml("cwd: D:\\Projects\\Foo\n");
        assert_eq!(m.cwd.as_deref(), Some("D:\\Projects\\Foo"));
        assert_eq!(m.name, None);
    }

    #[test]
    fn empty_value_stays_none() {
        let m = parse_workspace_yaml("name:\n");
        assert_eq!(m.name, None);
    }

    #[test]
    fn quoted_value_is_unquoted() {
        let m = parse_workspace_yaml("name: \"hello world\"\n");
        assert_eq!(m.name.as_deref(), Some("hello world"));
    }

    /// NFR-23: ブロックスカラーは解釈しない。そのキーだけ None にし、他は生かす
    #[test]
    fn block_scalar_name_becomes_none_others_survive() {
        let text = "cwd: D:\\Projects\\Foo\nname: |\n  line one\n  line two\nclient_name: cli\n";
        let m = parse_workspace_yaml(text);
        assert_eq!(m.name, None);
        assert_eq!(m.cwd.as_deref(), Some("D:\\Projects\\Foo"));
        assert_eq!(m.client_name.as_deref(), Some("cli"));
    }

    #[test]
    fn windows_path_with_backslashes_is_not_mangled() {
        let m = parse_workspace_yaml("cwd: C:\\Users\\foo\\bar\n");
        assert_eq!(m.cwd.as_deref(), Some("C:\\Users\\foo\\bar"));
    }

    // ---- preview ----

    #[test]
    fn preview_truncates_to_140_chars() {
        let s = "a".repeat(141);
        let p = preview(&s, PREVIEW_MAX_CHARS).unwrap();
        assert_eq!(p.chars().count(), 140);
    }

    #[test]
    fn preview_does_not_split_utf8_boundary() {
        let s = "あ".repeat(150);
        let p = preview(&s, PREVIEW_MAX_CHARS).unwrap();
        assert_eq!(p.chars().count(), 140);
        // 文字境界が壊れていなければ文字列として妥当
        assert_eq!(p, "あ".repeat(140));
    }

    #[test]
    fn preview_folds_newlines_into_one_line() {
        let p = preview("line one\nline two\r\nline three", 140).unwrap();
        assert!(!p.contains('\n'));
        assert!(!p.contains('\r'));
        assert_eq!(p, "line one line two line three");
    }

    #[test]
    fn preview_of_blank_is_none() {
        assert_eq!(preview("   ", 140), None);
        assert_eq!(preview("", 140), None);
    }

    // ---- scan_tail ----

    fn line(json: &str) -> Vec<u8> {
        json.as_bytes().to_vec()
    }

    #[test]
    fn last_line_usage_checkpoint_is_not_ended_by_shutdown() {
        let owned = [line(
            r#"{"type":"session.usage_checkpoint","timestamp":"2026-09-07T17:14:20.000Z","data":{"totalNanoAiu":380000000}}"#,
        )];
        let refs: Vec<&[u8]> = owned.iter().map(|v| v.as_slice()).collect();
        let facts = scan_tail(&refs);
        assert!(!facts.ended_by_shutdown);
        assert!(facts.last_timestamp.is_some());
        // 窓内に shutdown が無いので総量は None (0 ではない)
        assert_eq!(facts.total_nano_aiu, None);
    }

    #[test]
    fn last_line_shutdown_is_ended_by_shutdown() {
        let owned = [line(
            r#"{"type":"session.shutdown","timestamp":"2026-09-07T17:14:22.666Z","data":{"totalNanoAiu":382635000,"codeChanges":{"linesAdded":12,"linesRemoved":3}}}"#,
        )];
        let refs: Vec<&[u8]> = owned.iter().map(|v| v.as_slice()).collect();
        let facts = scan_tail(&refs);
        assert!(facts.ended_by_shutdown);
        assert_eq!(facts.total_nano_aiu, Some(382635000));
        assert_eq!(facts.lines_added, Some(12));
        assert_eq!(facts.lines_removed, Some(3));
    }

    #[test]
    fn last_shutdown_in_column_is_the_one_taken() {
        let owned = [
            line(
                r#"{"type":"session.shutdown","timestamp":"2026-09-07T10:00:00.000Z","data":{"totalNanoAiu":1}}"#,
            ),
            line(
                r#"{"type":"session.shutdown","timestamp":"2026-09-07T12:00:00.000Z","data":{"totalNanoAiu":999}}"#,
            ),
            line(
                r#"{"type":"session.usage_checkpoint","timestamp":"2026-09-07T13:00:00.000Z","data":{}}"#,
            ),
        ];
        let refs: Vec<&[u8]> = owned.iter().map(|v| v.as_slice()).collect();
        let facts = scan_tail(&refs);
        // 物理末尾は usage_checkpoint だが、遡って最初に見つかる shutdown (12:00) の値を採る
        assert_eq!(facts.total_nano_aiu, Some(999));
        assert!(!facts.ended_by_shutdown); // 末尾は usage_checkpoint
    }

    #[test]
    fn no_shutdown_in_window_leaves_totals_none() {
        let owned = [line(
            r#"{"type":"session.usage_checkpoint","timestamp":"2026-09-07T11:00:00.000Z","data":{"totalNanoAiu":1}}"#,
        )];
        let refs: Vec<&[u8]> = owned.iter().map(|v| v.as_slice()).collect();
        let facts = scan_tail(&refs);
        assert_eq!(facts.total_nano_aiu, None);
        assert_eq!(facts.lines_added, None);
        assert_eq!(facts.lines_removed, None);
    }

    #[test]
    fn broken_lines_are_counted_and_others_survive() {
        let owned = [
            line(r#"not json at all"#),
            line(
                r#"{"type":"session.shutdown","timestamp":"2026-09-07T12:00:00.000Z","data":{"totalNanoAiu":5}}"#,
            ),
        ];
        let refs: Vec<&[u8]> = owned.iter().map(|v| v.as_slice()).collect();
        let facts = scan_tail(&refs);
        assert_eq!(facts.skipped_lines, 1);
        assert_eq!(facts.parsed_lines, 1);
        assert_eq!(facts.total_nano_aiu, Some(5));
    }

    #[test]
    fn empty_input_does_not_panic() {
        let facts = scan_tail(&[]);
        assert_eq!(facts, TailFacts::default());
    }
}
