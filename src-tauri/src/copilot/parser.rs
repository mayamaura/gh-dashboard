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
    /// `tool.execution_start` / `tool.execution_complete` / `subagent.*` の
    /// `data.toolCallId`。**start と complete の突き合わせキー** (T-5.2 / FR-C-50)。
    /// 実データ 78 start / 75 complete で欠落 0 件、重複 0 件
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
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

    // ツール呼び出しの突き合わせキー。種別を問わず `data.toolCallId` を拾っておく
    // (`tool.*` と `subagent.*` の両方に同じ形で載る)
    facts.tool_call_id = str_of(get("toolCallId"));
    facts.tool_name = str_of(get("toolName"));

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

// ---------------------------------------------------------------- T-5.2 / T-5.4

/// ライブ監視が末尾ウィンドウ **1 回の逆走査**で拾う事実 (FR-C-45② / 47 / 49 / 50)。
///
/// 2 秒ごとに呼ばれるので、同じ行を複数回走査しない。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveTailFacts {
    /// FR-C-45 ② の入力。**物理的な最終行ではなく、遡って最初に該当したもの** (FR-C-49)
    pub tail: crate::copilot::activity::TailRecord,
    /// **集合が正。件数はこの長さとして導出する** (FR-C-51)。
    /// 窓内に現れたが `subagent.completed` が窓内に無い `toolCallId`。
    /// 古い順 (ファイル出現順)
    pub running_subagent_ids: Vec<String>,
    /// 末尾から遡って最初にパースできたレコードが `session.shutdown` か (ADR-0014)
    pub ended_by_shutdown: bool,
    /// **本文レコード** (`user.message` / `assistant.message`) を窓内で見つけたか。
    /// `false` なら呼び出し側が 512KB で 1 回だけ読み直す (FR-C-47)
    pub found_body: bool,
    /// 直近の本文レコードの `timestamp`。**mtime ではない** (FR-P-56)
    pub last_body_at: Option<i64>,
    /// 窓内で最も新しい `data.model`
    pub model: Option<String>,
    /// 窓内で最も新しい `data.totalNanoAiu` (累計値なので加算しない)
    pub session_nano_aiu: Option<i64>,
    /// パース失敗行。無言で落とさず数える (FR-C-12 / NFR-43)
    pub skipped_lines: usize,
}

/// 末尾ウィンドウの確定行を**新しい行から古い行へ**辿って分類する (純粋)。
///
/// 行は読み込み順 (古い→新しい) で渡すこと。`scan_tail` と違い、
/// **物理的な最終行では止まらない** (FR-C-49)。
///
/// 判定規則 (実データ 46 セッション / 1,041 レコードで確認):
/// 1. `tool.execution_start` があり、同じ `toolCallId` の `tool.execution_complete`
///    が窓内の**より新しい位置に無い** → [`TailRecord::ToolUse`]
/// 2. `assistant.turn_end` → [`TailRecord::TurnEnded`]
/// 3. `user.message` → [`TailRecord::UserMessage`]
/// 4. どれにも当たらない → [`TailRecord::Unrecognized`]
///
/// `assistant.message` は規則を持たない。ターン途中で止まっている場合は
/// 同じターンの先頭にある `user.message` まで遡って規則 3 に当たり、
/// 「生成中」になる — 実データのレコード順がそれを保証する。
///
/// 許可プロンプト待ちも規則 1 に落ちる。実データのレコード順が
/// `tool.execution_start` → `permission.requested` → `permission.completed` で、
/// 許可待ちの間は complete の無い start が最後に残るため。
/// **`permission.requested` は `toolCallId` を持たない**ので、そもそも
/// 独立した規則を書けない (FR-C-46 の「区別できない」と整合する)。
///
/// # 稼働中サブエージェント (FR-C-50 / 51)
///
/// 候補は「**窓内に現れた `agentId`**」と「窓内の `subagent.started` の
/// `toolCallId`」の和。窓内に同じ ID の `subagent.completed` があれば除く。
///
/// `subagent.started` **だけ**を候補にすると実データで拾えない。実測
/// (`2bb4517d…`、293,339 バイト) では `subagent.started` が**末尾から
/// 252,883 バイト**の位置にあり、64KB 窓にも 512KB 窓の意味のある位置にも
/// 入らない — サブエージェントが長く走るほど、その子レコードが `started` を
/// 窓の外へ押し出すからである。つまり「長く走っている = 表示したい」ものほど
/// 取り逃す。
///
/// `agentId` は**サブエージェントに属するレコードだけ**に付き、その値は
/// 子の `toolCallId` と**一致する** (実データ 2 件で 1:1、OQ-01 / OQ-07)。
/// 走っているサブエージェントは今まさに書いているので、その `agentId` は
/// 必ず末尾付近にある。
///
/// **残る近似** (FR-C-52 と同種の既知の限界): サブエージェントが窓ぶんの
/// 期間まったく書かずに黙っている場合は取り逃す。ただしその間は親の
/// `events.jsonl` も伸びないため、mtime 窓 (120 秒) でセッションごと
/// 非稼働に落ちる方が先に効く。
pub fn classify_tail(lines: &[&[u8]]) -> LiveTailFacts {
    use std::collections::HashSet;

    let mut facts = LiveTailFacts::default();
    // 逆走査なので、complete は対応する start より**先に**見つかる
    let mut completed_tools: HashSet<String> = HashSet::new();
    let mut completed_subagents: HashSet<String> = HashSet::new();
    let mut seen_subagents: HashSet<String> = HashSet::new();
    let mut running: Vec<String> = Vec::new();
    let mut first_record = true;
    let mut tail_decided = false;

    for line in lines.iter().rev() {
        let text = match std::str::from_utf8(line) {
            Ok(t) if !t.trim().is_empty() => t,
            _ => continue,
        };
        let Some(record) = parse_record(text) else {
            facts.skipped_lines += 1;
            continue;
        };

        if first_record {
            first_record = false;
            facts.ended_by_shutdown = record.record_type.as_deref() == Some("session.shutdown");
        }
        if facts.model.is_none() {
            facts.model.clone_from(&record.model);
        }
        if facts.session_nano_aiu.is_none() {
            facts.session_nano_aiu = record.session.as_ref().and_then(|s| s.total_nano_aiu);
        }

        // 稼働中サブエージェントの候補。**match より後で判定する** —
        // `subagent.completed` 自身も `agentId` を持つので、先に評価すると
        // 「完了したものを稼働中に入れてしまう」
        let agent_id = record.agent_id.clone();

        let record_type = record.record_type.clone().unwrap_or_default();
        match record_type.as_str() {
            "tool.execution_complete" => {
                if let Some(id) = record.tool_call_id {
                    completed_tools.insert(id);
                }
            }
            "tool.execution_start" => {
                // 完了が無い = まだ走っている (または中断された)
                let still_open = record
                    .tool_call_id
                    .as_ref()
                    .is_some_and(|id| !completed_tools.contains(id));
                if still_open && !tail_decided {
                    facts.tail = crate::copilot::activity::TailRecord::ToolUse;
                    tail_decided = true;
                }
            }
            "assistant.turn_end" => {
                if !tail_decided {
                    facts.tail = crate::copilot::activity::TailRecord::TurnEnded;
                    tail_decided = true;
                }
            }
            "subagent.completed" => {
                if let Some(id) = record.tool_call_id {
                    completed_subagents.insert(id);
                }
            }
            "subagent.started" => {
                // 主キーが取れないレコードからは行を作らない (FR-C-22)。
                // 実データの `subagent.selected` は `toolCallId` を持たない
                if let Some(id) = record.tool_call_id {
                    if !completed_subagents.contains(&id) && seen_subagents.insert(id.clone()) {
                        running.push(id);
                    }
                }
            }
            "user.message" | "assistant.message" => {
                if !facts.found_body {
                    facts.found_body = true;
                    facts.last_body_at = record.timestamp_ms;
                }
                if !tail_decided && record_type == "user.message" {
                    facts.tail = crate::copilot::activity::TailRecord::UserMessage;
                    tail_decided = true;
                }
            }
            _ => {}
        }

        // サブエージェントが**今まさに書いている**レコード。`agentId` は子の
        // `toolCallId` と同じ値なので、`subagent.started` が窓の外でも拾える
        if let Some(id) = agent_id {
            if !completed_subagents.contains(&id) && seen_subagents.insert(id.clone()) {
                running.push(id);
            }
        }
    }

    // 逆走査で集めたので、出現順 (古い→新しい) に戻す
    running.reverse();
    facts.running_subagent_ids = running;
    facts
}

// ---------------------------------------------------------------- T-7.3 (FR-C-105)

/// `session.shutdown.data.modelMetrics` をモデル別内訳に開く (FR-C-105 / FR-C-112)。
///
/// 引数は `session.shutdown` レコード 1 行の生 JSON。**呼び出し側が
/// `turn_index` のオフセットでシーク読みしたもの**を渡す — 本文も
/// `modelMetrics` も DB に複製しない (INV-6)。
///
/// # 二重加算を避ける根拠 (FR-C-104、実データ 45 セッションで検証)
///
/// 1 モデルぶんの実際の形:
///
/// ```text
/// "gpt-5-mini": {
///   "requests": {"count":2,"cost":0},
///   "usage":    {"inputTokens":22775,"outputTokens":1208,"cacheReadTokens":22016,
///                "cacheWriteTokens":0,"reasoningTokens":896},
///   "totalNanoAiu": 315615000,
///   "tokenDetails": {"input":{"tokenCount":759},"cache_read":{"tokenCount":22016},
///                    "cache_write":{"tokenCount":0},"output":{"tokenCount":1208}}
/// }
/// ```
///
/// **`usage.inputTokens` は `tokenDetails` のキャッシュ分を含んだ上位集計**である。
/// 上の例では `759 + 22016 = 22775` がちょうど `usage.inputTokens` に一致する
/// (キャッシュ書き込みがある場合は `input + cache_read + cache_write`)。
/// 45 セッション / 46 モデル行すべてでこの関係が成立した。
/// したがって **`usage.*` と `tokenDetails.*` を足すと入力が二重に乗る。**
/// ここでは互いに素な `tokenDetails` だけを採る。
///
/// `totalNanoAiu` の合計は `data.totalNanoAiu` (セッション総量) と 45/45 で一致した。
/// 差分実行で加算しないこと — レコード自体が累計値である。
pub fn parse_model_metrics(line: &str) -> Vec<crate::copilot::ModelUsage> {
    use crate::copilot::{ModelUsage, ModelUsageSource};

    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return Vec::new();
    };
    let Some(metrics) = value
        .get("data")
        .and_then(|d| d.get("modelMetrics"))
        .and_then(|m| m.as_object())
    else {
        // modelMetrics を持たない shutdown が実データに 5/50 件ある (NFR-23)
        return Vec::new();
    };

    let mut out: Vec<ModelUsage> = metrics
        .iter()
        // 合成モデルが混じっても除外する (FR-C-104)。実データでは 0 件だが、
        // ルーティングの実装が変われば混じりうる
        .filter(|(model, _)| !crate::copilot::usage::is_synthetic_model(model))
        .map(|(model, m)| {
            let details = m.get("tokenDetails");
            let nano_aiu = i64_of(m.get("totalNanoAiu"));
            ModelUsage {
                model: model.clone(),
                input_tokens: token_count(details, "input"),
                output_tokens: token_count(details, "output"),
                cache_read_tokens: token_count(details, "cache_read"),
                cache_write_tokens: token_count(details, "cache_write"),
                nano_aiu,
                credits: nano_aiu.map(crate::copilot::quota::credits_from_nano_aiu),
                record_count: None,
                source: ModelUsageSource::ShutdownMetrics,
            }
        })
        .collect();

    // 消費の大きい順。同額はモデル名で固定する (描画順が呼び出しごとに変わらない)
    out.sort_by(|a, b| {
        b.nano_aiu
            .unwrap_or(0)
            .cmp(&a.nano_aiu.unwrap_or(0))
            .then_with(|| a.model.cmp(&b.model))
    });
    out
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
        assert!(
            parse_record("{\"type\":\"a\"").is_none(),
            "途中で切れた JSON"
        );
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

    // ---- classify_tail (T-5.2 / T-5.4) -----------------------------------
    //
    // fixture の並びは実データ (46 セッション / 1,041 レコード) の出現順をそのまま
    // 縮めたもの。とくに `tool.execution_start → permission.requested →
    // permission.completed` の順序は実測に基づく (NFR-51)。

    use crate::copilot::activity::TailRecord;

    fn classify(jsonl: &[&str]) -> LiveTailFacts {
        let owned: Vec<Vec<u8>> = jsonl.iter().map(|s| s.as_bytes().to_vec()).collect();
        let refs: Vec<&[u8]> = owned.iter().map(|v| v.as_slice()).collect();
        classify_tail(&refs)
    }

    /// FR-C-49: 物理的な最終行 (shutdown / usage_checkpoint) で止まらない
    #[test]
    fn physical_last_line_is_not_the_tail_record() {
        let f = classify(&[
            r#"{"type":"user.message","timestamp":"2026-09-07T17:00:00.000Z","data":{"content":"hi"}}"#,
            r#"{"type":"assistant.turn_end","data":{"turnId":"0"}}"#,
            r#"{"type":"session.usage_checkpoint","data":{"totalNanoAiu":7}}"#,
            r#"{"type":"session.shutdown","data":{"totalNanoAiu":9}}"#,
        ]);
        assert_eq!(f.tail, TailRecord::TurnEnded);
        assert!(f.ended_by_shutdown);
        // 累計値は最も新しいものを 1 つだけ採る (加算しない)
        assert_eq!(f.session_nano_aiu, Some(9));
    }

    #[test]
    fn open_tool_call_is_tool_use() {
        let f = classify(&[
            r#"{"type":"user.message","data":{"content":"hi"}}"#,
            r#"{"type":"assistant.message","data":{"content":"やります","model":"gpt-5"}}"#,
            r#"{"type":"tool.execution_start","data":{"toolCallId":"c1","toolName":"bash"}}"#,
        ]);
        assert_eq!(f.tail, TailRecord::ToolUse);
        assert_eq!(f.model.as_deref(), Some("gpt-5"));
    }

    /// complete が来ていれば「ツール実行中」にしない。さらに遡る
    #[test]
    fn completed_tool_call_falls_through_to_the_older_record() {
        let f = classify(&[
            r#"{"type":"user.message","data":{"content":"hi"}}"#,
            r#"{"type":"tool.execution_start","data":{"toolCallId":"c1","toolName":"bash"}}"#,
            r#"{"type":"tool.execution_complete","data":{"toolCallId":"c1"}}"#,
        ]);
        // start は閉じているので規則 1 に当たらず、user.message まで遡る
        assert_eq!(f.tail, TailRecord::UserMessage);
    }

    /// 実測の順序: start → permission.requested → permission.completed。
    /// 許可待ちは complete の無い start として現れる (FR-C-46)
    #[test]
    fn pending_permission_prompt_looks_like_tool_use() {
        let f = classify(&[
            r#"{"type":"user.message","data":{"content":"hi"}}"#,
            r#"{"type":"assistant.message","data":{"content":"実行します"}}"#,
            r#"{"type":"tool.execution_start","data":{"toolCallId":"c9","toolName":"bash"}}"#,
            r#"{"type":"permission.requested","data":{"requestId":"r1"}}"#,
        ]);
        assert_eq!(f.tail, TailRecord::ToolUse);
    }

    /// ターン途中の `assistant.message` は規則を持たないが、同じターンの
    /// `user.message` まで遡って「生成中」に落ちる
    #[test]
    fn assistant_message_midturn_falls_back_to_user_message() {
        let f = classify(&[
            r#"{"type":"user.message","timestamp":"2026-09-07T17:00:00.000Z","data":{"content":"hi"}}"#,
            r#"{"type":"assistant.turn_start","data":{"turnId":"1"}}"#,
            r#"{"type":"assistant.message","timestamp":"2026-09-07T17:00:05.000Z","data":{"content":"考え中"}}"#,
        ]);
        assert_eq!(f.tail, TailRecord::UserMessage);
        // 本文レコードは新しい側から採る
        assert!(f.found_body);
        assert_eq!(f.last_body_at, Some(1_788_800_405_000));
    }

    #[test]
    fn window_without_any_body_record_asks_for_a_wider_read() {
        let f = classify(&[
            r#"{"type":"session.usage_checkpoint","data":{"totalNanoAiu":1}}"#,
            r#"{"type":"session.shutdown","data":{"totalNanoAiu":2}}"#,
        ]);
        assert!(!f.found_body, "512KB で読み直す合図 (FR-C-47)");
        assert_eq!(f.tail, TailRecord::Unrecognized, "埋めない (NFR-43)");
    }

    // ---- 稼働中サブエージェント集合 (FR-C-50 / 51) ----

    #[test]
    fn started_without_completed_is_running() {
        let f = classify(&[
            r#"{"type":"tool.execution_start","data":{"toolCallId":"toolu_1","toolName":"task","arguments":{"name":"a"}}}"#,
            r#"{"type":"subagent.started","data":{"toolCallId":"toolu_1","agentName":"a"}}"#,
        ]);
        assert_eq!(f.running_subagent_ids, vec!["toolu_1".to_string()]);
    }

    #[test]
    fn completed_subagent_is_not_running() {
        let f = classify(&[
            r#"{"type":"subagent.started","data":{"toolCallId":"toolu_1","agentName":"a"}}"#,
            r#"{"type":"subagent.completed","data":{"toolCallId":"toolu_1","totalToolCalls":3}}"#,
        ]);
        assert!(f.running_subagent_ids.is_empty());
    }

    #[test]
    fn several_running_subagents_keep_file_order_and_are_deduped() {
        let f = classify(&[
            r#"{"type":"subagent.started","data":{"toolCallId":"a1","agentName":"a"}}"#,
            r#"{"type":"subagent.started","data":{"toolCallId":"b2","agentName":"b"}}"#,
            r#"{"type":"subagent.started","data":{"toolCallId":"a1","agentName":"a"}}"#,
            r#"{"type":"subagent.completed","data":{"toolCallId":"b2"}}"#,
        ]);
        assert_eq!(f.running_subagent_ids, vec!["a1".to_string()]);
    }

    /// 実測の要点: `subagent.started` は窓の外に出る (末尾から 252,883 バイト)。
    /// 子が書いているレコードの `agentId` で拾えること
    #[test]
    fn running_subagent_is_found_from_agent_id_when_started_is_out_of_window() {
        // 窓に入っているのは子のレコードだけ。subagent.started は入っていない
        let f = classify(&[
            r#"{"type":"assistant.message","agentId":"toolu_1","data":{"content":"調査中"}}"#,
            r#"{"type":"tool.execution_start","agentId":"toolu_1","data":{"toolCallId":"t9","toolName":"bash"}}"#,
            r#"{"type":"tool.execution_complete","agentId":"toolu_1","data":{"toolCallId":"t9"}}"#,
        ]);
        assert_eq!(f.running_subagent_ids, vec!["toolu_1".to_string()]);
    }

    /// `subagent.completed` 自身も `agentId` を持つ。完了したものを稼働中に入れない
    #[test]
    fn completed_subagent_is_not_revived_by_its_own_agent_id() {
        let f = classify(&[
            r#"{"type":"subagent.started","agentId":"toolu_1","data":{"toolCallId":"toolu_1"}}"#,
            r#"{"type":"assistant.message","agentId":"toolu_1","data":{"content":"done"}}"#,
            r#"{"type":"subagent.completed","agentId":"toolu_1","data":{"toolCallId":"toolu_1"}}"#,
        ]);
        assert!(
            f.running_subagent_ids.is_empty(),
            "completed より古い子レコードで復活させない"
        );
    }

    /// FR-C-22: 主キーが取れないレコードから集合に入れない。
    /// 実データの `subagent.selected` は `toolCallId` を持たない (46 件)
    #[test]
    fn subagent_without_tool_call_id_is_not_counted() {
        let f = classify(&[
            r#"{"type":"subagent.selected","data":{"agentName":"a","agentDisplayName":"A"}}"#,
            r#"{"type":"subagent.started","data":{"agentName":"a"}}"#,
        ]);
        assert!(f.running_subagent_ids.is_empty());
    }

    #[test]
    fn broken_lines_are_counted_and_do_not_stop_the_scan() {
        let f = classify(&[
            r#"{"type":"user.message","data":{"content":"hi"}}"#,
            r#"garbage{{{"#,
            r#"{"type":"assistant.turn_end","data":{"turnId":"0"}}"#,
            r#"also not json"#,
        ]);
        assert_eq!(f.skipped_lines, 2);
        // 壊れた行の手前まで遡れている
        assert_eq!(f.tail, TailRecord::TurnEnded);
    }

    #[test]
    fn empty_window_is_safe() {
        assert_eq!(classify_tail(&[]), LiveTailFacts::default());
        assert_eq!(classify_tail(&[]).tail, TailRecord::Unrecognized);
    }

    /// T-4.2 の `parse_record` に足したキーが実データの形で取れること
    #[test]
    fn tool_call_id_and_name_are_extracted() {
        let f = parse_record(
            r#"{"type":"tool.execution_start","data":{"toolCallId":"c1","toolName":"powershell"}}"#,
        )
        .unwrap();
        assert_eq!(f.tool_call_id.as_deref(), Some("c1"));
        assert_eq!(f.tool_name.as_deref(), Some("powershell"));
        let c = parse_record(r#"{"type":"tool.execution_complete","data":{"toolCallId":"c1"}}"#)
            .unwrap();
        assert_eq!(c.tool_call_id.as_deref(), Some("c1"));
        assert_eq!(c.tool_name, None);
    }

    // ---- parse_model_metrics (T-7.3 / FR-C-105) --------------------------
    //
    // fixture は実データ (~/.copilot、51 セッション / 1,041 レコード) の
    // session.shutdown.data.modelMetrics をそのまま縮めたもの (NFR-51)。

    /// FR-C-104: `usage.*` ではなく互いに素な `tokenDetails.*` を採る
    #[test]
    fn model_metrics_take_token_details_not_the_overlapping_usage_block() {
        // 759 + 22016 = 22775 = usage.inputTokens (実データで成立した関係)
        let line = r#"{"type":"session.shutdown","data":{"totalNanoAiu":315615000,
            "modelMetrics":{"gpt-5-mini":{
                "requests":{"count":2,"cost":0},
                "usage":{"inputTokens":22775,"outputTokens":1208,"cacheReadTokens":22016,
                         "cacheWriteTokens":0,"reasoningTokens":896},
                "totalNanoAiu":315615000,
                "tokenDetails":{"input":{"tokenCount":759},"cache_read":{"tokenCount":22016},
                                "cache_write":{"tokenCount":0},"output":{"tokenCount":1208}}}}}}"#;
        let rows = parse_model_metrics(line);
        assert_eq!(rows.len(), 1);
        let m = &rows[0];
        assert_eq!(m.model, "gpt-5-mini");
        assert_eq!(
            m.input_tokens,
            Some(759),
            "usage.inputTokens (22775) を採ると cache_read が二重に乗る"
        );
        assert_eq!(m.cache_read_tokens, Some(22016));
        assert_eq!(m.output_tokens, Some(1208));
        assert_eq!(m.cache_write_tokens, Some(0));
        assert_eq!(m.nano_aiu, Some(315_615_000));
        assert!((m.credits.unwrap() - 0.315615).abs() < 1e-9);
        assert_eq!(m.record_count, None);
    }

    /// 実データ 5/50 件の shutdown は modelMetrics を持たない。
    /// 0 の行をでっち上げず、空で返す (NFR-43)
    #[test]
    fn shutdown_without_model_metrics_yields_no_rows() {
        assert!(parse_model_metrics(r#"{"type":"session.shutdown","data":{"totalNanoAiu":5}}"#).is_empty());
        assert!(parse_model_metrics(
            r#"{"type":"session.shutdown","data":{"modelMetrics":{}}}"#
        )
        .is_empty());
        assert!(parse_model_metrics("not json").is_empty());
    }

    /// 中身が欠けていても panic せず、その項目が None になるだけ (NFR-23)
    #[test]
    fn model_metrics_with_missing_fields_do_not_panic() {
        let rows = parse_model_metrics(
            r#"{"type":"session.shutdown","data":{"modelMetrics":{"gpt-5-mini":{}}}}"#,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].input_tokens, None);
        assert_eq!(rows[0].nano_aiu, None);
        assert_eq!(rows[0].credits, None, "0 で埋めない (NFR-43)");
    }

    /// FR-C-104: 合成モデルのキーが混じっても集計に入れない
    #[test]
    fn synthetic_model_key_is_excluded() {
        let rows = parse_model_metrics(
            r#"{"type":"session.shutdown","data":{"modelMetrics":{
                "auto":{"totalNanoAiu":999},
                "gpt-5-mini":{"totalNanoAiu":1}}}}"#,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].model, "gpt-5-mini");
    }

    /// 描画順が呼び出しごとに変わらないこと (消費降順 → モデル名)
    #[test]
    fn model_rows_are_sorted_by_consumption_deterministically() {
        let line = r#"{"type":"session.shutdown","data":{"modelMetrics":{
            "claude-haiku-4.5":{"totalNanoAiu":100},
            "gpt-5-mini":{"totalNanoAiu":900}}}}"#;
        let a: Vec<String> = parse_model_metrics(line).iter().map(|m| m.model.clone()).collect();
        assert_eq!(a, vec!["gpt-5-mini".to_string(), "claude-haiku-4.5".to_string()]);
        let b: Vec<String> = parse_model_metrics(line).iter().map(|m| m.model.clone()).collect();
        assert_eq!(a, b);
    }

    /// T-4.11: 実データでも利用枠到達レコードは観測できていない (OQ-01 のまま)
    #[test]
    fn quota_event_extraction_finds_nothing_until_the_record_kind_is_observed() {
        let f = parse_record(r#"{"type":"session.shutdown","data":{"totalNanoAiu":1}}"#).unwrap();
        assert_eq!(parse_quota_event(&f), None);
    }
}
