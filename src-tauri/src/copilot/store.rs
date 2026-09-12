//! 差分インデックスの永続化 (SQL だけ)。
//!
//! すべて同期関数。呼び出し側 (`indexer`) が `spawn_blocking` の中で使う (INV-10)。
//! **本文は書かない。** 索引に載るのはパス + オフセット + 長さ + メタ + 140 字
//! プレビューまで (FR-C-02 / INV-6)。
//!
//! 対応要求: FR-C-02 / 04 / 07 / 14 / 20 / 21〜23 / 28 / DR-02 / DR-03

use rusqlite::{Connection, OptionalExtension, Transaction};

use crate::copilot::parser::{RecordFacts, SubagentFacts};
use crate::copilot::{DbSnapshot, Entrypoint, SessionQuery, SessionSummary};
use crate::error::AppResult;

/// `index_files` に記録済みの進捗。差分判定の入力はこの 2 つと現在サイズだけ (FR-C-06)。
#[derive(Debug, Clone, Copy, Default)]
pub struct FileProgress {
    pub last_parsed_offset: u64,
    pub file_size_at_parse: u64,
}

pub fn file_progress(conn: &Connection, file_path: &str) -> AppResult<FileProgress> {
    let row = conn
        .query_row(
            "SELECT last_parsed_offset, file_size_at_parse FROM index_files WHERE file_path = ?1",
            [file_path],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()?;
    Ok(match row {
        Some((offset, size)) => FileProgress {
            last_parsed_offset: offset.max(0) as u64,
            file_size_at_parse: size.max(0) as u64,
        },
        None => FileProgress::default(),
    })
}

/// 全再パース前の掃除 (FR-C-06)。
///
/// truncate / 入れ替わりが起きたファイルは、同じオフセットに**別の内容**が載る。
/// UNIQUE(file_path, byte_offset) は二重挿入を防ぐが、古い行を古いまま残してしまう。
pub fn clear_file(tx: &Transaction<'_>, file_path: &str) -> AppResult<()> {
    tx.execute("DELETE FROM turn_index WHERE file_path = ?1", [file_path])?;
    tx.execute(
        "UPDATE index_files SET last_parsed_offset = 0, file_size_at_parse = 0 WHERE file_path = ?1",
        [file_path],
    )?;
    Ok(())
}

/// `turn_index` の 1 行。本文は持たない (FR-C-02)。
pub struct TurnRow<'a> {
    pub session_id: &'a str,
    pub file_path: &'a str,
    pub byte_offset: u64,
    pub byte_length: u64,
    pub facts: &'a RecordFacts,
}

/// 1 レコードを索引に入れる。**`INSERT OR IGNORE`** — 二重適用の安全装置 (FR-C-07)。
///
/// 戻り値は「実際に挿入されたか」。同じ (file_path, byte_offset) が既にあれば `false`。
pub fn insert_turn(tx: &Transaction<'_>, row: &TurnRow<'_>) -> AppResult<bool> {
    let f = row.facts;
    let changed = tx.execute(
        "INSERT OR IGNORE INTO turn_index \
         (session_id, agent_id, file_path, byte_offset, byte_length, record_type, role, model, \
          timestamp_ms, uuid, parent_uuid, is_sidechain, is_quota_error, \
          input_tokens, output_tokens, cache_write_tokens, cache_read_tokens, nano_aiu, preview) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0, ?13, ?14, ?15, ?16, ?17, ?18)",
        rusqlite::params![
            row.session_id,
            f.agent_id,
            row.file_path,
            row.byte_offset as i64,
            row.byte_length as i64,
            f.record_type,
            f.role,
            f.model,
            f.timestamp_ms,
            f.uuid,
            f.parent_uuid,
            // agentId が付く = サブエージェント配下のレコード (parser.rs の定義を参照)
            f.agent_id.is_some(),
            f.input_tokens,
            f.output_tokens,
            f.cache_write_tokens,
            f.cache_read_tokens,
            f.nano_aiu,
            f.preview,
        ],
    )?;
    Ok(changed > 0)
}

/// **バッチの最後に呼ぶこと** (FR-C-04)。
///
/// レコード挿入と同一トランザクション内でオフセットを進めるから、クラッシュしても
/// 「最後にコミットが成功した位置」から再開できる。先に更新すると、挿入が落ちた分が
/// 二度と読まれない。
pub fn commit_offset(
    tx: &Transaction<'_>,
    file_path: &str,
    session_id: &str,
    offset: u64,
    file_size: u64,
    mtime_ms: Option<i64>,
    now_ms: i64,
) -> AppResult<()> {
    tx.execute(
        "INSERT INTO index_files \
           (file_path, kind, session_id, agent_id, last_parsed_offset, file_size_at_parse, mtime_ms, updated_at) \
         VALUES (?1, 'events', ?2, NULL, ?3, ?4, ?5, ?6) \
         ON CONFLICT(file_path) DO UPDATE SET \
           session_id = excluded.session_id, \
           last_parsed_offset = excluded.last_parsed_offset, \
           file_size_at_parse = excluded.file_size_at_parse, \
           mtime_ms = excluded.mtime_ms, \
           updated_at = excluded.updated_at",
        rusqlite::params![
            file_path,
            session_id,
            offset as i64,
            file_size as i64,
            mtime_ms,
            now_ms
        ],
    )?;
    Ok(())
}

/// セッション 1 行ぶんの集計 (FR-C-20)。**無期限保持** (FR-C-14)。
///
/// 件数系 (`turn_count` / `agent_count` / `quota_event_count`) はここに持たない。
/// 集合から導出する — 別々に数えると食い違う (FR-C-51)。
#[derive(Debug, Clone, Default)]
pub struct SessionRow {
    pub session_id: String,
    pub cwd: Option<String>,
    pub path_key: Option<String>,
    pub folder_name: Option<String>,
    /// `workspace.yaml` の `client_name` を**生のまま**入れる。
    /// 表示用の分類は読み出し時に `Entrypoint` へ写像する (未知の値は Unknown)
    pub entrypoint: Option<String>,
    pub title: Option<String>,
    pub model: Option<String>,
    pub started_at: Option<i64>,
    pub last_activity_at: Option<i64>,
    pub total_nano_aiu: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
}

/// セッション集計を書く。**`None` の項目は既存値を消さない** (COALESCE)。
///
/// 差分実行では「今回のバッチに shutdown が入っていない」ことが普通にある。
/// そこで NULL を上書きすると、前回取れていた集計が消える。
pub fn upsert_session(tx: &Transaction<'_>, row: &SessionRow, now_ms: i64) -> AppResult<()> {
    tx.execute(
        "INSERT INTO sessions \
           (session_id, cwd, path_key, folder_name, git_branch, entrypoint, title, model, \
            started_at, last_activity_at, turn_count, \
            input_tokens, output_tokens, cache_write_tokens, cache_read_tokens, \
            total_nano_aiu, agent_count, quota_event_count, updated_at) \
         VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7, ?8, ?9, 0, \
                 COALESCE(?10, 0), COALESCE(?11, 0), COALESCE(?12, 0), COALESCE(?13, 0), \
                 COALESCE(?14, 0), 0, 0, ?15) \
         ON CONFLICT(session_id) DO UPDATE SET \
           cwd = COALESCE(excluded.cwd, cwd), \
           path_key = COALESCE(excluded.path_key, path_key), \
           folder_name = COALESCE(excluded.folder_name, folder_name), \
           entrypoint = COALESCE(excluded.entrypoint, entrypoint), \
           title = COALESCE(excluded.title, title), \
           model = COALESCE(excluded.model, model), \
           started_at = COALESCE(excluded.started_at, started_at), \
           last_activity_at = MAX(COALESCE(excluded.last_activity_at, 0), COALESCE(last_activity_at, 0)), \
           input_tokens = COALESCE(?10, input_tokens), \
           output_tokens = COALESCE(?11, output_tokens), \
           cache_write_tokens = COALESCE(?12, cache_write_tokens), \
           cache_read_tokens = COALESCE(?13, cache_read_tokens), \
           total_nano_aiu = COALESCE(?14, total_nano_aiu), \
           updated_at = excluded.updated_at",
        rusqlite::params![
            row.session_id,
            row.cwd,
            row.path_key,
            row.folder_name,
            row.entrypoint,
            row.title,
            row.model,
            row.started_at,
            row.last_activity_at,
            row.input_tokens,
            row.output_tokens,
            row.cache_write_tokens,
            row.cache_read_tokens,
            row.total_nano_aiu,
            now_ms,
        ],
    )?;
    Ok(())
}

/// 件数列を**集合から数え直す** (FR-C-51)。
///
/// 差分実行のたびに加算すると、再パースや二重適用で必ずずれる。数え直せばずれない。
pub fn refresh_session_counts(tx: &Transaction<'_>, session_id: &str) -> AppResult<()> {
    tx.execute(
        "UPDATE sessions SET \
           turn_count = (SELECT COUNT(*) FROM turn_index WHERE session_id = ?1), \
           agent_count = (SELECT COUNT(*) FROM subagent_runs WHERE session_id = ?1), \
           quota_event_count = (SELECT COUNT(*) FROM quota_events WHERE session_id = ?1) \
         WHERE session_id = ?1",
        [session_id],
    )?;
    Ok(())
}

/// サブエージェント実行をマージする (FR-C-21〜23 / 29 / ADR-0016)。
///
/// **`status` は `'running'` のまま。** `'completed'` / `'declined'` へ遷移させる
/// 経路をここに作らない — 完了ゲート (FR-C-24〜26) と拒否判定 (FR-C-27) は
/// OQ-11 が未実測で、拒否も非同期委任も 1 件も観測できていない。推測でゲートを
/// 組むと直せない形で壊れる (ADR-0016 の注記 / T-4.9 / T-4.10 保留)。
/// `subagent.completed` が来ても統計だけ更新する。
pub fn merge_subagent(
    tx: &Transaction<'_>,
    session_id: &str,
    parent_file_path: &str,
    facts: &SubagentFacts,
    agent_id: Option<&str>,
    timestamp_ms: Option<i64>,
    now_ms: i64,
) -> AppResult<()> {
    // 起動 (親側の tool.execution_start) を見た時刻だけを started_at にする。
    // 完了レコードの時刻で開始時刻を作らない
    let started_at = facts.is_dispatch.then_some(timestamp_ms).flatten();
    tx.execute(
        "INSERT INTO subagent_runs \
           (run_key, agent_id, session_id, parent_file_path, parent_agent_id, spawn_depth, \
            agent_type, description, model, started_at, last_activity_at, ended_at, status, \
            input_tokens, output_tokens, tool_call_count, updated_at) \
         VALUES (?1, ?2, ?3, ?4, NULL, NULL, ?5, ?6, ?7, ?8, ?9, NULL, 'running', 0, 0, COALESCE(?10, 0), ?11) \
         ON CONFLICT(run_key) DO UPDATE SET \
           agent_id = COALESCE(excluded.agent_id, agent_id), \
           parent_file_path = COALESCE(excluded.parent_file_path, parent_file_path), \
           agent_type = COALESCE(excluded.agent_type, agent_type), \
           description = COALESCE(excluded.description, description), \
           model = COALESCE(excluded.model, model), \
           started_at = COALESCE(excluded.started_at, started_at), \
           last_activity_at = MAX(COALESCE(excluded.last_activity_at, 0), COALESCE(last_activity_at, 0)), \
           tool_call_count = COALESCE(?10, tool_call_count), \
           updated_at = excluded.updated_at",
        rusqlite::params![
            facts.tool_call_id,
            agent_id,
            session_id,
            parent_file_path,
            facts.agent_type,
            facts.description,
            facts.model,
            started_at,
            timestamp_ms,
            facts.tool_call_count,
            now_ms,
        ],
    )?;
    Ok(())
}

/// 利用枠到達イベント (FR-C-28 / T-4.11)。
///
/// **呼び出し口はあるが、現状どのレコードからも呼ばれない。**
/// 抽出条件が決まっていない理由は `parser::parse_quota_event` を参照 (OQ-01)。
pub fn insert_quota_event(
    tx: &Transaction<'_>,
    session_id: &str,
    occurred_at: i64,
    kind: &str,
    reset_text: Option<&str>,
    raw_message: Option<&str>,
) -> AppResult<()> {
    tx.execute(
        "INSERT OR IGNORE INTO quota_events (session_id, occurred_at, kind, reset_text, raw_message) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![session_id, occurred_at, kind, reset_text, raw_message],
    )?;
    Ok(())
}

/// `client_name` から起動経路を写像する (用語定義)。
///
/// **未知の値を推測で分類しない。** 実データでは `sdk` (45 件) / `github/cli` (1 件)
/// が観測されているが、`sdk` がどの経路かを示す根拠が無いため `Unknown` に落とす
/// (NFR-40 / NFR-43)。生の値は `sessions.entrypoint` にそのまま残っている。
pub fn entrypoint_from_client_name(client_name: Option<&str>) -> Entrypoint {
    match client_name.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("github/cli" | "cli" | "cli_interactive") => Entrypoint::CliInteractive,
        Some("cli_background") => Entrypoint::CliBackground,
        Some(s) if s.contains("vscode") || s.contains("vs code") => Entrypoint::Vscode,
        Some("coding_agent" | "coding-agent") => Entrypoint::CodingAgent,
        _ => Entrypoint::Unknown,
    }
}

/// 直近 20 件を返すときの件数 (IR-11)。
pub const RECENT_SESSION_LIMIT: usize = 20;

/// IR-11 のダイジェスト。**件数はすべて集合の長さとして数える** (FR-C-51 / NFR-44)。
pub fn snapshot(conn: &Connection) -> AppResult<DbSnapshot> {
    let count = |sql: &str| -> AppResult<i64> { Ok(conn.query_row(sql, [], |r| r.get(0))?) };

    let mut stmt = conn.prepare(
        "SELECT session_id, folder_name, title, cwd, entrypoint, started_at, last_activity_at, \
                turn_count, total_nano_aiu, agent_count \
         FROM sessions ORDER BY last_activity_at DESC NULLS LAST LIMIT ?1",
    )?;
    let rows = stmt.query_map([RECENT_SESSION_LIMIT as i64], |r| {
        let client_name: Option<String> = r.get(4)?;
        Ok(SessionSummary {
            session_id: r.get(0)?,
            folder_name: r.get(1)?,
            title: r.get(2)?,
            cwd: r.get(3)?,
            entrypoint: entrypoint_from_client_name(client_name.as_deref()),
            started_at: r.get(5)?,
            last_activity_at: r.get(6)?,
            turn_count: r.get(7)?,
            total_nano_aiu: r.get(8)?,
            agent_count: r.get(9)?,
        })
    })?;
    let mut recent_sessions = Vec::new();
    for row in rows {
        recent_sessions.push(row?);
    }
    drop(stmt);

    Ok(DbSnapshot {
        // 「取りに行った時刻」ではなく、索引が実際に進んだ時刻 (FR-C-86 と同じ理屈)
        last_indexed_at: conn
            .query_row("SELECT MAX(updated_at) FROM index_files", [], |r| r.get(0))
            .optional()?
            .flatten(),
        retained_session_count: count("SELECT COUNT(*) FROM sessions")?,
        subagent_run_count: count("SELECT COUNT(*) FROM subagent_runs")?,
        turn_count: count("SELECT COUNT(*) FROM turn_index")?,
        recent_sessions,
    })
}

/// IR-13: セッション検索 (FR-C-110 / 111)。
///
/// `text` は `folder_name` / `title` / `cwd` の部分一致 (大小文字を問わない)。
/// `LIKE` の `%` / `_` はユーザー入力にそのまま出ても壊れないよう、パターン内の
/// 特殊文字は `\` でエスケープしプレースホルダでバインドする (SQL 文字列連結はしない)。
pub fn search_sessions(conn: &Connection, query: &SessionQuery) -> AppResult<Vec<SessionSummary>> {
    let text_pattern = query
        .text
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(like_pattern);
    let subagents_only = query.with_subagents_only.unwrap_or(false);
    let limit = query.effective_limit() as i64;

    let sql = "SELECT session_id, folder_name, title, cwd, entrypoint, started_at, \
               last_activity_at, turn_count, total_nano_aiu, agent_count \
               FROM sessions \
               WHERE (?1 IS NULL OR folder_name LIKE ?1 ESCAPE '\\' \
                      OR title LIKE ?1 ESCAPE '\\' OR cwd LIKE ?1 ESCAPE '\\') \
                 AND (?2 = 0 OR agent_count > 0) \
               ORDER BY last_activity_at DESC NULLS LAST \
               LIMIT ?3";

    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(
        rusqlite::params![text_pattern, subagents_only, limit],
        |r| {
            let client_name: Option<String> = r.get(4)?;
            Ok(SessionSummary {
                session_id: r.get(0)?,
                folder_name: r.get(1)?,
                title: r.get(2)?,
                cwd: r.get(3)?,
                entrypoint: entrypoint_from_client_name(client_name.as_deref()),
                started_at: r.get(5)?,
                last_activity_at: r.get(6)?,
                turn_count: r.get(7)?,
                total_nano_aiu: r.get(8)?,
                agent_count: r.get(9)?,
            })
        },
    )?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// `LIKE` 用パターンを組み立てる。`%` / `_` / `\` をエスケープしてから前後に `%` を付ける。
fn like_pattern(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

/// IR-19 の設定キー (FR-C-162)。**このキー名を変えると既存の保存値が読めなくなる。**
const ANIMATION_PREF_KEY: &str = "animation_pref";

/// `settings` から `animation_pref` を読む。未設定は `None` (呼び出し側が既定値を当てる)。
pub fn animation_pref_get(conn: &Connection) -> AppResult<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [ANIMATION_PREF_KEY],
            |r| r.get(0),
        )
        .optional()?)
}

/// `settings` に `animation_pref` を書く (`INSERT OR REPLACE`)。
pub fn animation_pref_set(conn: &Connection, value: &str) -> AppResult<()> {
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        rusqlite::params![ANIMATION_PREF_KEY, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copilot::parser;
    use crate::db::open_in_memory;

    const NOW: i64 = 1_800_000_000_000;

    fn facts(json: &str) -> RecordFacts {
        parser::parse_record(json).unwrap()
    }

    #[test]
    fn insert_turn_is_idempotent_per_offset() {
        let mut conn = open_in_memory().unwrap();
        let f = facts(r#"{"type":"user.message","id":"a","data":{"content":"hi"}}"#);
        let row = TurnRow {
            session_id: "s1",
            file_path: "d:\\a\\events.jsonl",
            byte_offset: 0,
            byte_length: 40,
            facts: &f,
        };

        let tx = conn.transaction().unwrap();
        assert!(insert_turn(&tx, &row).unwrap(), "1 回目は入る");
        assert!(!insert_turn(&tx, &row).unwrap(), "同じ位置は無視される");
        tx.commit().unwrap();

        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM turn_index", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "UNIQUE(file_path, byte_offset) が二重適用を吸収する");
    }

    /// INV-6 / FR-C-02: 索引に本文が載っていないこと
    #[test]
    fn turn_index_stores_preview_only_not_the_body() {
        let mut conn = open_in_memory().unwrap();
        let body = "秘密の本文".repeat(80); // 400 字
        let json = format!(
            r#"{{"type":"assistant.message","data":{{"content":"{body}","outputTokens":5}}}}"#
        );
        let f = facts(&json);
        let tx = conn.transaction().unwrap();
        insert_turn(
            &tx,
            &TurnRow {
                session_id: "s1",
                file_path: "f",
                byte_offset: 0,
                byte_length: json.len() as u64,
                facts: &f,
            },
        )
        .unwrap();
        tx.commit().unwrap();

        let stored: String = conn
            .query_row("SELECT preview FROM turn_index", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored.chars().count(), parser::PREVIEW_MAX_CHARS);
        assert!(stored.len() < body.len(), "本文全体を複製していない");
    }

    #[test]
    fn sidechain_flag_follows_agent_id() {
        let mut conn = open_in_memory().unwrap();
        let main = facts(r#"{"type":"user.message","data":{}}"#);
        let child = facts(r#"{"type":"user.message","agentId":"toolu_01","data":{}}"#);
        let tx = conn.transaction().unwrap();
        for (i, f) in [&main, &child].iter().enumerate() {
            insert_turn(
                &tx,
                &TurnRow {
                    session_id: "s1",
                    file_path: "f",
                    byte_offset: i as u64 * 100,
                    byte_length: 10,
                    facts: f,
                },
            )
            .unwrap();
        }
        tx.commit().unwrap();

        let flags: Vec<i64> = conn
            .prepare("SELECT is_sidechain FROM turn_index ORDER BY byte_offset")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(flags, vec![0, 1]);
    }

    #[test]
    fn session_upsert_does_not_erase_known_values_with_none() {
        let mut conn = open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        upsert_session(
            &tx,
            &SessionRow {
                session_id: "s1".into(),
                title: Some("最初のプロンプト".into()),
                total_nano_aiu: Some(382_635_000),
                last_activity_at: Some(NOW),
                ..Default::default()
            },
            NOW,
        )
        .unwrap();
        // 2 回目のバッチには shutdown が入っていない (差分実行では普通に起きる)
        upsert_session(
            &tx,
            &SessionRow {
                session_id: "s1".into(),
                last_activity_at: Some(NOW + 1000),
                ..Default::default()
            },
            NOW + 1000,
        )
        .unwrap();
        tx.commit().unwrap();

        let (title, aiu, last): (Option<String>, i64, i64) = conn
            .query_row(
                "SELECT title, total_nano_aiu, last_activity_at FROM sessions",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(title.as_deref(), Some("最初のプロンプト"));
        assert_eq!(aiu, 382_635_000, "取れていた集計を None で消さない");
        assert_eq!(last, NOW + 1000, "最終活動時刻は進む");
    }

    #[test]
    fn counts_are_recomputed_from_the_sets() {
        let mut conn = open_in_memory().unwrap();
        let f = facts(r#"{"type":"user.message","data":{}}"#);
        let tx = conn.transaction().unwrap();
        upsert_session(
            &tx,
            &SessionRow {
                session_id: "s1".into(),
                ..Default::default()
            },
            NOW,
        )
        .unwrap();
        for i in 0..3 {
            insert_turn(
                &tx,
                &TurnRow {
                    session_id: "s1",
                    file_path: "f",
                    byte_offset: i * 10,
                    byte_length: 10,
                    facts: &f,
                },
            )
            .unwrap();
        }
        refresh_session_counts(&tx, "s1").unwrap();
        // 2 回呼んでも増えない (加算ではなく数え直しだから)
        refresh_session_counts(&tx, "s1").unwrap();
        tx.commit().unwrap();

        let n: i64 = conn
            .query_row("SELECT turn_count FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 3);
    }

    /// ADR-0016: 親側で作った行に、子側のメタが同じ `toolCallId` でマージされる
    #[test]
    fn subagent_merges_parent_and_child_into_one_row() {
        let mut conn = open_in_memory().unwrap();
        let dispatch = facts(
            r#"{"type":"tool.execution_start","data":{"toolCallId":"toolu_01","toolName":"task",
                "model":"gpt-5","arguments":{"agent_type":"helper","description":"挨拶を作る"}}}"#,
        );
        let started = facts(
            r#"{"type":"subagent.started","agentId":"toolu_01","data":{"toolCallId":"toolu_01",
                "agentName":"helper","agentDescription":"短い挨拶文の作成担当","model":"gpt-5-mini"}}"#,
        );
        let completed = facts(
            r#"{"type":"subagent.completed","agentId":"toolu_01","data":{"toolCallId":"toolu_01",
                "agentName":"helper","totalToolCalls":12,"durationMs":291463}}"#,
        );

        let tx = conn.transaction().unwrap();
        merge_subagent(
            &tx,
            "s1",
            "f",
            dispatch.subagent.as_ref().unwrap(),
            None,
            Some(NOW),
            NOW,
        )
        .unwrap();
        merge_subagent(
            &tx,
            "s1",
            "f",
            started.subagent.as_ref().unwrap(),
            started.agent_id.as_deref(),
            Some(NOW + 500),
            NOW,
        )
        .unwrap();
        merge_subagent(
            &tx,
            "s1",
            "f",
            completed.subagent.as_ref().unwrap(),
            completed.agent_id.as_deref(),
            Some(NOW + 9000),
            NOW,
        )
        .unwrap();
        tx.commit().unwrap();

        let (n, agent_id, started_at, last, status, calls): (
            i64,
            Option<String>,
            i64,
            i64,
            String,
            i64,
        ) = conn
            .query_row(
                "SELECT COUNT(*), agent_id, started_at, last_activity_at, status, tool_call_count \
                 FROM subagent_runs",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(n, 1, "3 レコードが 1 行にマージされる (FR-C-23)");
        assert_eq!(agent_id.as_deref(), Some("toolu_01"));
        assert_eq!(started_at, NOW, "開始時刻は親側の起動レコードのもの");
        assert_eq!(last, NOW + 9000);
        assert_eq!(calls, 12);
        assert_eq!(
            status, "running",
            "完了への遷移は OQ-11 待ち (T-4.9 / T-4.10 保留)"
        );
    }

    /// FR-C-24: 完了レコードが来ても終了時刻を確定させない (ゲート未実装のため)
    #[test]
    fn completion_record_does_not_set_ended_at_yet() {
        let mut conn = open_in_memory().unwrap();
        let completed = facts(
            r#"{"type":"subagent.completed","data":{"toolCallId":"toolu_01","totalToolCalls":1}}"#,
        );
        let tx = conn.transaction().unwrap();
        merge_subagent(
            &tx,
            "s1",
            "f",
            completed.subagent.as_ref().unwrap(),
            None,
            Some(NOW),
            NOW,
        )
        .unwrap();
        tx.commit().unwrap();

        let ended: Option<i64> = conn
            .query_row("SELECT ended_at FROM subagent_runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ended, None);
    }

    #[test]
    fn unknown_client_name_is_not_guessed() {
        assert_eq!(
            entrypoint_from_client_name(Some("github/cli")),
            Entrypoint::CliInteractive
        );
        assert_eq!(
            entrypoint_from_client_name(Some("vscode")),
            Entrypoint::Vscode
        );
        // 実データで 45 件を占める `sdk` は、どの経路か示す根拠が無い
        assert_eq!(entrypoint_from_client_name(Some("sdk")), Entrypoint::Unknown);
        assert_eq!(entrypoint_from_client_name(None), Entrypoint::Unknown);
    }

    #[test]
    fn snapshot_counts_come_from_the_tables() {
        let mut conn = open_in_memory().unwrap();
        let f = facts(r#"{"type":"user.message","data":{}}"#);
        let tx = conn.transaction().unwrap();
        upsert_session(
            &tx,
            &SessionRow {
                session_id: "s1".into(),
                entrypoint: Some("github/cli".into()),
                last_activity_at: Some(NOW),
                ..Default::default()
            },
            NOW,
        )
        .unwrap();
        insert_turn(
            &tx,
            &TurnRow {
                session_id: "s1",
                file_path: "f",
                byte_offset: 0,
                byte_length: 10,
                facts: &f,
            },
        )
        .unwrap();
        commit_offset(&tx, "f", "s1", 10, 10, Some(NOW), NOW).unwrap();
        tx.commit().unwrap();

        let snap = snapshot(&conn).unwrap();
        assert_eq!(snap.retained_session_count, 1);
        assert_eq!(snap.turn_count, 1);
        assert_eq!(snap.subagent_run_count, 0);
        assert_eq!(snap.last_indexed_at, Some(NOW));
        assert_eq!(snap.recent_sessions.len(), 1);
        assert_eq!(
            snap.recent_sessions[0].entrypoint,
            Entrypoint::CliInteractive
        );
    }

    /// FR-C-04: 落ちたバッチはオフセットを進めない。
    ///
    /// オフセットを先に (別トランザクションで) 更新していたら、この巻き戻しで
    /// 「行は無いのに読み終わったことになっている」区間ができ、二度と読まれない。
    #[test]
    fn rolled_back_batch_advances_nothing() {
        let mut conn = open_in_memory().unwrap();
        let f = facts(r#"{"type":"user.message","data":{}}"#);

        // 1 バッチ目は成功する
        let tx = conn.transaction().unwrap();
        insert_turn(
            &tx,
            &TurnRow {
                session_id: "s1",
                file_path: "f",
                byte_offset: 0,
                byte_length: 50,
                facts: &f,
            },
        )
        .unwrap();
        commit_offset(&tx, "f", "s1", 50, 200, None, NOW).unwrap();
        tx.commit().unwrap();

        // 2 バッチ目の途中で落ちる (= コミットせずに巻き戻る)
        let tx = conn.transaction().unwrap();
        insert_turn(
            &tx,
            &TurnRow {
                session_id: "s1",
                file_path: "f",
                byte_offset: 50,
                byte_length: 50,
                facts: &f,
            },
        )
        .unwrap();
        commit_offset(&tx, "f", "s1", 100, 200, None, NOW).unwrap();
        tx.rollback().unwrap();

        let progress = file_progress(&conn, "f").unwrap();
        assert_eq!(
            progress.last_parsed_offset, 50,
            "再開位置は最後に成功したコミットのまま"
        );
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM turn_index", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "落ちたバッチの行も残っていない");
    }

    #[test]
    fn clear_file_removes_only_that_files_rows() {
        let mut conn = open_in_memory().unwrap();
        let f = facts(r#"{"type":"user.message","data":{}}"#);
        let tx = conn.transaction().unwrap();
        for path in ["a", "b"] {
            insert_turn(
                &tx,
                &TurnRow {
                    session_id: "s1",
                    file_path: path,
                    byte_offset: 0,
                    byte_length: 10,
                    facts: &f,
                },
            )
            .unwrap();
            commit_offset(&tx, path, "s1", 10, 10, None, NOW).unwrap();
        }
        clear_file(&tx, "a").unwrap();
        tx.commit().unwrap();

        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM turn_index", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 1);
        assert_eq!(file_progress(&conn, "a").unwrap().last_parsed_offset, 0);
        assert_eq!(file_progress(&conn, "b").unwrap().last_parsed_offset, 10);
    }

    fn seed_session(
        conn: &Connection,
        session_id: &str,
        folder_name: &str,
        title: &str,
        cwd: &str,
        agent_count: i64,
        last_activity_at: i64,
    ) {
        conn.execute(
            "INSERT INTO sessions \
               (session_id, cwd, folder_name, title, agent_count, last_activity_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            rusqlite::params![session_id, cwd, folder_name, title, agent_count, last_activity_at],
        )
        .unwrap();
    }

    /// T-7.4: `text` はフォルダ名・タイトル・作業ディレクトリの部分一致 (大小文字を問わない)
    #[test]
    fn search_sessions_matches_folder_title_or_cwd_case_insensitively() {
        let conn = open_in_memory().unwrap();
        seed_session(&conn, "s1", "gh-dashboard", "初期設定", "d:\\proj\\gh-dashboard", 0, 1);
        seed_session(&conn, "s2", "other-repo", "OTHER TASK", "d:\\proj\\other-repo", 0, 2);

        let q = SessionQuery {
            text: Some("dashboard".into()),
            limit: None,
            with_subagents_only: None,
        };
        let hits = search_sessions(&conn, &q).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id, "s1");

        let q = SessionQuery {
            text: Some("OTHER".into()),
            limit: None,
            with_subagents_only: None,
        };
        let hits = search_sessions(&conn, &q).unwrap();
        assert_eq!(hits.len(), 1, "大小文字を問わず一致する");
        assert_eq!(hits[0].session_id, "s2");
    }

    /// FR-C-111: サブエージェントを使ったセッションのみに絞る
    #[test]
    fn search_sessions_can_filter_to_sessions_with_subagents_only() {
        let conn = open_in_memory().unwrap();
        seed_session(&conn, "s1", "a", "t", "c", 0, 1);
        seed_session(&conn, "s2", "b", "t", "c", 3, 2);

        let q = SessionQuery {
            text: None,
            limit: None,
            with_subagents_only: Some(true),
        };
        let hits = search_sessions(&conn, &q).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id, "s2");
    }

    /// FR-C-110: 既定 100 / 上限 1000。ここでは小さい limit で切り詰めを確認する
    #[test]
    fn search_sessions_respects_effective_limit() {
        let conn = open_in_memory().unwrap();
        for i in 0..5 {
            seed_session(&conn, &format!("s{i}"), "f", "t", "c", 0, i);
        }
        let q = SessionQuery {
            text: None,
            limit: Some(2),
            with_subagents_only: None,
        };
        let hits = search_sessions(&conn, &q).unwrap();
        assert_eq!(hits.len(), 2);
        // last_activity_at 降順: 一番大きい値が先頭 (s4, s3)
        assert_eq!(hits[0].session_id, "s4");
        assert_eq!(hits[1].session_id, "s3");
    }

    /// `%` / `_` を含む入力がそのまま渡ってもクラッシュせず、リテラルとして扱われる
    #[test]
    fn search_sessions_treats_like_special_chars_literally() {
        let conn = open_in_memory().unwrap();
        seed_session(&conn, "s1", "100%_done", "t", "c", 0, 1);
        seed_session(&conn, "s2", "100Xdone", "t", "c", 0, 2);

        let q = SessionQuery {
            text: Some("100%_done".into()),
            limit: None,
            with_subagents_only: None,
        };
        let hits = search_sessions(&conn, &q).unwrap();
        assert_eq!(hits.len(), 1, "%/_ をワイルドカードとして展開してはいけない");
        assert_eq!(hits[0].session_id, "s1");
    }

    /// T-7.14: 未設定は既定値 (`None` を呼び出し側で `Auto` に落とす)
    #[test]
    fn animation_pref_round_trips_through_settings_table() {
        let conn = open_in_memory().unwrap();
        assert_eq!(animation_pref_get(&conn).unwrap(), None);

        animation_pref_set(&conn, "on").unwrap();
        assert_eq!(animation_pref_get(&conn).unwrap(), Some("on".to_string()));

        // 2 回目の書き込みは上書きされる (INSERT OR REPLACE)
        animation_pref_set(&conn, "off").unwrap();
        assert_eq!(animation_pref_get(&conn).unwrap(), Some("off".to_string()));
    }
}
