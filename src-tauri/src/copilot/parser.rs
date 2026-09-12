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

// ---------------------------------------------------------------- T-4.2

/// `session.*` レコードから拾えたセッション集計の材料 (FR-C-20)。
///
/// **すべて `Option`。** 主ソースは `session.shutdown.data` だが、実データでは
/// 同じ shutdown でも `tokenDetails` を持たないものが 50 件中 5 件、`agentMetrics`
/// に至っては 50 件中 0 件だった。「観測できた種別なら全フィールドが揃う」という
/// 仮定は実データで既に崩れている (NFR-23 / FR-C-13)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionFacts {
    /// `session.shutdown` 由来か (`false` なら `session.start` / `usage_checkpoint`)。
    /// shutdown があればそちらを主ソースにする (ADR-0013)
    pub from_shutdown: bool,
    pub cwd: Option<String>,
    pub started_at: Option<i64>,
    pub model: Option<String>,
    pub total_nano_aiu: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
}

/// サブエージェント実行の材料 (FR-C-21〜23 / ADR-0016)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentFacts {
    /// **主キー。親側と子側の両方に存在する唯一の識別子** (FR-C-22 / ADR-0016)。
    /// これが取れないレコードからは `SubagentFacts` を作らない
    pub tool_call_id: String,
    /// 親側の `tool.execution_start` (`toolName == "task"`) か
    pub is_dispatch: bool,
    /// **完了ソースは `subagent.completed` のみ** (FR-C-25 / ADR-0016)。
    /// ただし状態遷移は OQ-11 が埋まるまで行わない (T-4.9 / T-4.10 保留)
    pub is_completion: bool,
    pub agent_type: Option<String>,
    pub description: Option<String>,
    pub model: Option<String>,
    pub tool_call_count: Option<i64>,
}

/// 1 レコード分の防御的パース結果 (T-4.2)。
///
/// `turn_index` の各カラムを埋めるのに必要なものだけを持つ。**生の本文は持たない** —
/// `preview` に 140 字まで畳んだものだけが載る (FR-C-02 / INV-6)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordFacts {
    pub record_type: Option<String>,
    pub role: Option<String>,
    pub model: Option<String>,
    pub timestamp_ms: Option<i64>,
    pub uuid: Option<String>,
    pub parent_uuid: Option<String>,
    /// `agentId`。**サブエージェント配下のレコードにだけ付く** (実測 23.4% / 4.2%)。
    /// `turn_index.is_sidechain` はこの有無で決める — ドキュメントに定義が無いため、
    /// 「サブエージェントに属する = 本流から分岐した記録」という対応づけを採る
    pub agent_id: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_write_tokens: i64,
    pub cache_read_tokens: i64,
    /// **常に 0。** レコード単位の AI Credits 消費はログに存在しない。
    /// `totalNanoAiu` は累計値なので、ここに入れると合計が多重計上になる。
    /// セッション単位の値は `SessionFacts::total_nano_aiu` が持つ
    pub nano_aiu: i64,
    pub preview: Option<String>,
    pub session: Option<SessionFacts>,
    pub subagent: Option<SubagentFacts>,
}

fn str_of(v: Option<&serde_json::Value>) -> Option<String> {
    v?.as_str().filter(|s| !s.is_empty()).map(|s| s.to_string())
}

fn i64_of(v: Option<&serde_json::Value>) -> Option<i64> {
    v?.as_i64()
}

/// `tokenDetails.<kind>.tokenCount` を引く。どの階層が欠けても `None`。
fn token_count(details: Option<&serde_json::Value>, kind: &str) -> Option<i64> {
    i64_of(details?.get(kind)?.get("tokenCount"))
}

/// `type` からロールを導出する。判断できない種別は `None` (埋めない / NFR-43)。
fn role_of(record_type: &str) -> Option<String> {
    let role = match record_type {
        "user.message" => "user",
        "assistant.message" => "assistant",
        "system.message" => "system",
        t if t.starts_with("tool.") => "tool",
        _ => return None,
    };
    Some(role.to_string())
}

/// JSONL の 1 行を解釈する (FR-C-12 / FR-C-13 / NFR-23)。
///
/// - JSON として読めなければ `None` — 呼び出し側が「壊れた行」として数える
/// - 読めたなら、**中身がどれだけ欠けていても `Some`**。欠けた項目が `None` になるだけ
pub fn parse_record(line: &str) -> Option<RecordFacts> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;

    let record_type = str_of(value.get("type"));
    let data = value.get("data");
    let get = |key: &str| data.and_then(|d| d.get(key));

    let mut facts = RecordFacts {
        role: record_type.as_deref().and_then(role_of),
        model: str_of(get("model")),
        timestamp_ms: value
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(parse_iso8601_ms),
        uuid: str_of(value.get("id")),
        parent_uuid: str_of(value.get("parentId")),
        agent_id: str_of(value.get("agentId")),
        // 実測: 出力トークンだけがレコード単位で載る (assistant.message の 100%)。
        // 入力・キャッシュはセッション単位の tokenDetails にしか無い
        output_tokens: i64_of(get("outputTokens")).unwrap_or(0),
        ..RecordFacts::default()
    };

    // 本文プレビュー。**本文そのものは保持しない** (INV-6)
    if matches!(
        record_type.as_deref(),
        Some("user.message" | "assistant.message" | "system.message")
    ) {
        facts.preview = get("content")
            .and_then(|v| v.as_str())
            .and_then(|s| preview(s, PREVIEW_MAX_CHARS));
    }

    match record_type.as_deref() {
        Some("session.start") => {
            facts.session = Some(SessionFacts {
                cwd: str_of(get("context").and_then(|c| c.get("cwd"))),
                started_at: get("startTime")
                    .and_then(|v| v.as_str())
                    .and_then(parse_iso8601_ms),
                ..SessionFacts::default()
            });
        }
        Some("session.usage_checkpoint") => {
            // shutdown が無いセッション (中断・進行中) のフォールバック
            facts.session = Some(SessionFacts {
                total_nano_aiu: i64_of(get("totalNanoAiu")),
                ..SessionFacts::default()
            });
        }
        Some("session.shutdown") => {
            let details = get("tokenDetails");
            facts.session = Some(SessionFacts {
                from_shutdown: true,
                cwd: None,
                // **epoch ミリ秒**。トップレベル timestamp の ISO8601 と混同しない
                started_at: i64_of(get("sessionStartTime")),
                model: str_of(get("currentModel")),
                total_nano_aiu: i64_of(get("totalNanoAiu")),
                input_tokens: token_count(details, "input"),
                output_tokens: token_count(details, "output"),
                cache_write_tokens: token_count(details, "cache_write"),
                cache_read_tokens: token_count(details, "cache_read"),
            });
        }
        Some("tool.execution_start") if str_of(get("toolName")).as_deref() == Some("task") => {
            // 親側の委任呼び出し。ここで行を作る (FR-C-23)
            let args = get("arguments");
            facts.subagent = str_of(get("toolCallId")).map(|tool_call_id| SubagentFacts {
                tool_call_id,
                is_dispatch: true,
                is_completion: false,
                agent_type: str_of(args.and_then(|a| a.get("agent_type")))
                    .or_else(|| str_of(args.and_then(|a| a.get("name")))),
                description: str_of(args.and_then(|a| a.get("description")))
                    .and_then(|d| preview(&d, PREVIEW_MAX_CHARS)),
                model: str_of(get("model")),
                tool_call_count: None,
            });
        }
        Some(t @ ("subagent.started" | "subagent.configured" | "subagent.completed")) => {
            // 子側。メタ情報は差分の有無に関わらず毎回読み直してマージする (FR-C-29)。
            // agentType は実データ (CLI 1.0.1xx) では消えており agentName しか残らない。
            // 片方しか無い前提で両方を見る (NFR-23)
            facts.subagent = str_of(get("toolCallId")).map(|tool_call_id| SubagentFacts {
                tool_call_id,
                is_dispatch: false,
                is_completion: t == "subagent.completed",
                agent_type: str_of(get("agentType")).or_else(|| str_of(get("agentName"))),
                description: str_of(get("agentDescription"))
                    .and_then(|d| preview(&d, PREVIEW_MAX_CHARS)),
                model: str_of(get("model")),
                tool_call_count: i64_of(get("totalToolCalls")),
            });
        }
        _ => {}
    }

    facts.record_type = record_type;
    Some(facts)
}

/// 利用枠到達イベントの抽出 (FR-C-28 / T-4.11)。**現状は常に `None`**。
///
/// レコード種別が実データでも確認できていない。ユーザーの実 `~/.copilot`
/// 1,041 レコード / 3.0 MB を走査して、`quota` / `rate limit` / `credit`
/// 系の `type` も本文も 1 件も観測できなかった (2026-09-12)。
///
/// **推測で種別名を埋めないこと。** 決め打ちした名前は実物と一致しなければ
/// 永久に 0 件のままで、しかも「実装済み」に見えてしまう。実物を観測できたら
/// ここに条件を足す。書き込み経路 (`store::insert_quota_event`) は既にある。
pub fn parse_quota_event(_facts: &RecordFacts) -> Option<(String, Option<String>, Option<String>)> {
    None
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

    // ---- parse_record (T-4.2) --------------------------------------------
    //
    // fixture は実データ (ユーザーの ~/.copilot、46 セッション / 1,041 レコード) で
    // 観測した形をそのまま縮めたもの。値だけ差し替えている (NFR-51)。

    #[test]
    fn broken_json_returns_none_so_caller_can_count_it() {
        assert!(parse_record("not json at all").is_none());
        assert!(parse_record("").is_none());
        assert!(parse_record("{\"type\":\"a\"").is_none(), "途中で切れた JSON");
    }

    /// FR-C-13 / NFR-23: 中身が空でも panic せず、全項目が None / 0 になるだけ
    #[test]
    fn empty_object_parses_into_all_empty_facts() {
        let f = parse_record("{}").unwrap();
        assert_eq!(f, RecordFacts::default());
    }

    #[test]
    fn record_without_data_does_not_panic() {
        let f = parse_record(r#"{"type":"session.shutdown","timestamp":"2026-09-07T17:00:00Z"}"#)
            .unwrap();
        assert_eq!(f.record_type.as_deref(), Some("session.shutdown"));
        // data ごと欠けていても「shutdown だが中身なし」になるだけ
        let s = f.session.unwrap();
        assert!(s.from_shutdown);
        assert_eq!(s.total_nano_aiu, None);
    }

    #[test]
    fn assistant_message_fills_role_model_tokens_and_preview() {
        let line = r#"{"type":"assistant.message","id":"rec-2","parentId":"rec-1",
            "timestamp":"2026-09-07T17:14:22.666Z",
            "data":{"messageId":"m1","model":"claude-sonnet-4.5","content":"はい、\n直します",
                    "turnId":"3","outputTokens":123,"toolRequests":[]}}"#;
        let f = parse_record(line).unwrap();
        assert_eq!(f.role.as_deref(), Some("assistant"));
        assert_eq!(f.model.as_deref(), Some("claude-sonnet-4.5"));
        assert_eq!(f.output_tokens, 123);
        assert_eq!(f.uuid.as_deref(), Some("rec-2"));
        assert_eq!(f.parent_uuid.as_deref(), Some("rec-1"));
        assert_eq!(f.timestamp_ms, Some(1_788_801_262_666));
        // 改行は畳まれ、本文そのものは残らない (INV-6)
        assert_eq!(f.preview.as_deref(), Some("はい、 直します"));
        // レコード単位の AI Credits はログに存在しない
        assert_eq!(f.nano_aiu, 0);
    }

    #[test]
    fn role_is_derived_from_type() {
        let role = |t: &str| {
            parse_record(&format!(r#"{{"type":"{t}","data":{{}}}}"#))
                .unwrap()
                .role
        };
        assert_eq!(role("user.message").as_deref(), Some("user"));
        assert_eq!(role("system.message").as_deref(), Some("system"));
        assert_eq!(role("tool.execution_start").as_deref(), Some("tool"));
        assert_eq!(role("tool.execution_complete").as_deref(), Some("tool"));
        // 判断できない種別は埋めない (NFR-43)
        assert_eq!(role("assistant.turn_end"), None);
        assert_eq!(role("permission.requested"), None);
        assert_eq!(role("abort"), None);
    }

    /// `is_sidechain` の材料。サブエージェント配下のレコードにだけ付く
    #[test]
    fn agent_id_is_present_only_on_subagent_records() {
        let main = parse_record(r#"{"type":"user.message","data":{"content":"hi"}}"#).unwrap();
        assert_eq!(main.agent_id, None);
        let child = parse_record(
            r#"{"type":"assistant.message","agentId":"toolu_01","data":{"content":"hi"}}"#,
        )
        .unwrap();
        assert_eq!(child.agent_id.as_deref(), Some("toolu_01"));
    }

    /// FR-C-20 の集計はほぼこの 1 レコードで揃う (ADR-0013)
    #[test]
    fn shutdown_is_the_primary_session_source() {
        let line = r#"{"type":"session.shutdown","timestamp":"2026-09-07T17:14:22.666Z",
            "data":{"shutdownType":"routine","totalNanoAiu":382635000,"currentModel":"gpt-5",
                    "sessionStartTime":1788800000000,
                    "tokenDetails":{"input":{"tokenCount":1000},"cache_read":{"tokenCount":2000},
                                    "cache_write":{"tokenCount":300},"output":{"tokenCount":40}},
                    "codeChanges":{"linesAdded":12,"linesRemoved":3}}}"#;
        let s = parse_record(line).unwrap().session.unwrap();
        assert!(s.from_shutdown);
        assert_eq!(s.total_nano_aiu, Some(382_635_000));
        assert_eq!(s.input_tokens, Some(1000));
        assert_eq!(s.cache_read_tokens, Some(2000));
        assert_eq!(s.cache_write_tokens, Some(300));
        assert_eq!(s.output_tokens, Some(40));
        assert_eq!(s.model.as_deref(), Some("gpt-5"));
        // sessionStartTime は epoch ミリ秒。ISO8601 として解釈してはいけない
        assert_eq!(s.started_at, Some(1_788_800_000_000));
    }

    /// 実データで 50 件中 5 件の shutdown が `tokenDetails` を持っていなかった。
    /// 「観測できた種別なら全フィールドが揃う」は成り立たない (NFR-51)
    #[test]
    fn shutdown_without_token_details_keeps_totals() {
        let line = r#"{"type":"session.shutdown","data":{"totalNanoAiu":5,"sessionStartTime":1}}"#;
        let s = parse_record(line).unwrap().session.unwrap();
        assert_eq!(s.total_nano_aiu, Some(5));
        // 取れなかったトークンは 0 ではなく None (NFR-43)
        assert_eq!(s.input_tokens, None);
        assert_eq!(s.output_tokens, None);
    }

    #[test]
    fn usage_checkpoint_is_the_fallback_for_nano_aiu() {
        let line = r#"{"type":"session.usage_checkpoint","data":{"totalNanoAiu":7,"totalPremiumRequests":1}}"#;
        let s = parse_record(line).unwrap().session.unwrap();
        assert!(!s.from_shutdown, "主ソースは shutdown のままにする");
        assert_eq!(s.total_nano_aiu, Some(7));
    }

    #[test]
    fn session_start_provides_cwd_and_start_time() {
        let line = r#"{"type":"session.start","data":{"sessionId":"s1","startTime":"2026-09-07T17:00:00.000Z","context":{"cwd":"D:\\Projects\\Foo"}}}"#;
        let s = parse_record(line).unwrap().session.unwrap();
        assert_eq!(s.cwd.as_deref(), Some("D:\\Projects\\Foo"));
        assert!(s.started_at.is_some());
    }

    // ---- サブエージェント (ADR-0016: 主キーは toolCallId) ----

    #[test]
    fn task_dispatch_creates_the_run_from_the_parent_side() {
        let line = r#"{"type":"tool.execution_start","timestamp":"2026-09-07T17:10:00Z",
            "data":{"toolCallId":"toolu_0179","toolName":"task","model":"gpt-5","turnId":"1",
                    "arguments":{"name":"news-agent","agent_type":"本日ニュース要約",
                                 "description":"RSS から記事取得・要約","mode":"sync","prompt":"..."}}}"#;
        let sa = parse_record(line).unwrap().subagent.unwrap();
        assert_eq!(sa.tool_call_id, "toolu_0179");
        assert!(sa.is_dispatch, "親側で見つけた時点で行を作る (FR-C-23)");
        assert!(!sa.is_completion);
        assert_eq!(sa.agent_type.as_deref(), Some("本日ニュース要約"));
        assert_eq!(sa.description.as_deref(), Some("RSS から記事取得・要約"));
    }

    /// task 以外のツール呼び出しでサブエージェント行を作らない (幽霊行の防止)
    #[test]
    fn non_task_tool_calls_do_not_create_subagent_runs() {
        let line = r#"{"type":"tool.execution_start","data":{"toolCallId":"c1","toolName":"powershell","arguments":{"command":"ls"}}}"#;
        assert_eq!(parse_record(line).unwrap().subagent, None);
    }

    #[test]
    fn subagent_started_and_completed_share_the_tool_call_id() {
        let started = r#"{"type":"subagent.started","agentId":"toolu_0179",
            "data":{"toolCallId":"toolu_0179","agentName":"helper","agentDisplayName":"helper",
                    "agentDescription":"短い挨拶文の作成担当","model":"gpt-5-mini"}}"#;
        let completed = r#"{"type":"subagent.completed","agentId":"toolu_0179",
            "data":{"toolCallId":"toolu_0179","agentName":"helper","model":"gpt-5-mini",
                    "totalToolCalls":12,"totalTokens":298818,"durationMs":291463}}"#;
        let a = parse_record(started).unwrap().subagent.unwrap();
        let b = parse_record(completed).unwrap().subagent.unwrap();
        assert_eq!(a.tool_call_id, b.tool_call_id, "親子を同じキーでマージする");
        // agentType が無い実データでは agentName にフォールバックする
        assert_eq!(a.agent_type.as_deref(), Some("helper"));
        assert_eq!(a.description.as_deref(), Some("短い挨拶文の作成担当"));
        assert!(b.is_completion);
        assert_eq!(b.tool_call_count, Some(12));
    }

    /// 主キーが取れないレコードから行を作らない (FR-C-22)。
    /// 実データの `subagent.selected` は `toolCallId` を持たない
    #[test]
    fn subagent_record_without_tool_call_id_creates_nothing() {
        let line = r#"{"type":"subagent.started","data":{"agentName":"helper"}}"#;
        assert_eq!(parse_record(line).unwrap().subagent, None);
    }

    /// T-4.11: 実データでも利用枠到達レコードは観測できていない (OQ-01 のまま)
    #[test]
    fn quota_event_extraction_finds_nothing_until_the_record_kind_is_observed() {
        let f = parse_record(r#"{"type":"session.shutdown","data":{"totalNanoAiu":1}}"#).unwrap();
        assert_eq!(parse_quota_event(&f), None);
    }
}
