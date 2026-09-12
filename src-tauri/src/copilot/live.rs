//! ライブ監視 (IO)。読み取り専用 (INV-1)。
//!
//! **2 秒ごとに呼ばれる** (IR-10 / FR-C-42)。この経路に入れてはいけないもの:
//!
//! | 禁止 | 根拠 |
//! |---|---|
//! | ネットワークアクセス | INV-4 / NFR-03 |
//! | 外部プロセス起動 | INV-4 / FR-C-42 (PID の**生存確認**は可) |
//! | DB (`app.db` / Copilot 側の SQLite) の読み書き | ADR-0013 / DR-02 |
//! | 全件パース | FR-C-08 / FR-C-47 |
//!
//! **段階 4 の差分インデックスに依存しない。** ライブ経路は DB を読まないし
//! 書かない。索引が一度も走っていなくても稼働中セッションは出る。
//!
//! キャッシュはプロセス内メモリ (`AppState::live_cache`) にだけ置く。
//! 導出データを永続化しない (INV-5 / DR-02)。
//!
//! **同期関数。** 呼び出し側 (`copilot::commands`) が `spawn_blocking` で包む
//! (INV-10 / NFR-20)。1 セッションの失敗で全体を落とさない (NFR-24)。
//!
//! 対応要求: FR-C-40 / 42 / 44〜54 / 58 / 61 / 70〜72 / ADR-0014

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::copilot::sessions::{copilot_home, read_tail, TAIL_READ_BYTES};
use crate::copilot::{activity, delta, parser, store, IdeWorkspace, LiveSession, LiveStatus};

/// 本文レコードが 64KB に入らなかったときの読み直し幅。**1 回だけ** (FR-C-47)。
pub const TAIL_READ_BYTES_WIDE: u64 = 512 * 1024;

/// `(パス, サイズ, mtime)` が変わらない限り使い回す末尾の解釈結果 (FR-C-48)。
///
/// これが無いと 2 秒ごとに全セッションの末尾を読むことになる。
#[derive(Debug, Clone)]
pub struct CachedTail {
    path: PathBuf,
    size: u64,
    mtime_ms: i64,
    facts: parser::LiveTailFacts,
}

/// セッション ID → 直近の末尾解釈。**DB に持たない** (INV-5)。
pub type LiveCache = HashMap<String, CachedTail>;

/// `events.jsonl` の stat 結果。どちらも取れなければセッションを稼働と見なさない。
fn stat_events(path: &Path) -> Option<(u64, i64)> {
    let md = std::fs::metadata(path).ok()?;
    let mtime = md
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    Some((md.len(), mtime))
}

/// 末尾ウィンドウを読んで分類する (FR-C-47 / 49)。
///
/// 64KB で本文レコードが見つからなければ 512KB で**1 回だけ**読み直す。
/// ファイル全体は読まない。
fn read_and_classify(path: &Path) -> parser::LiveTailFacts {
    let classify = |max_bytes: u64| -> Option<(parser::LiveTailFacts, bool)> {
        let (buf, from_start) = read_tail(path, max_bytes).ok()?;
        let dropped = delta::drop_leading_partial(&buf, from_start);
        // 追記中の末尾断片を確定扱いしない (FR-C-05)
        let committed = delta::split_committed(dropped);
        Some((parser::classify_tail(&committed.lines), from_start))
    };

    let Some((facts, from_start)) = classify(TAIL_READ_BYTES) else {
        // 読めないセッションは「不明」のまま。全体は落とさない (NFR-24)
        return parser::LiveTailFacts::default();
    };
    if facts.found_body || from_start {
        // from_start ならファイル全体を見ているので、広げても増えない
        return facts;
    }
    classify(TAIL_READ_BYTES_WIDE)
        .map(|(f, _)| f)
        .unwrap_or(facts)
}

/// ライブ状況を 1 回ぶん集める (IR-10)。
///
/// `cache` は呼び出しをまたいで保持されるプロセス内メモリ。
pub fn collect(now_ms: i64, cache: &mut LiveCache) -> LiveStatus {
    let Some(home) = copilot_home() else {
        return LiveStatus::new(vec![], vec![], None, now_ms);
    };

    let mut sessions: Vec<LiveSession> = Vec::new();
    let mut newest_log_mtime_ms: Option<i64> = None;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let entries = std::fs::read_dir(home.join("session-state"))
        .ok()
        .into_iter()
        .flatten()
        .flatten();

    for entry in entries {
        let session_dir = entry.path();
        if !session_dir.is_dir() {
            continue;
        }
        let session_id = entry.file_name().to_string_lossy().to_string();
        let events_path = session_dir.join("events.jsonl");

        let Some((size, mtime_ms)) = stat_events(&events_path) else {
            // events.jsonl が無いセッションは稼働と見なさない (ADR-0014)
            continue;
        };
        seen.insert(session_id.clone());

        // T-5.12 (FR-C-58 / 59): 自動インデックスの発火判定に渡す値。
        // **稼働/非稼働を問わず全セッションの最大 mtime**。定期更新される値は混ぜない
        newest_log_mtime_ms = Some(newest_log_mtime_ms.map_or(mtime_ms, |m: i64| m.max(mtime_ms)));

        // 安いほうから順に切る。ここで落ちたセッションは末尾も workspace.yaml も
        // 読まない (46 セッションで stat 1 回ずつだけになる)
        let mtime_age_ms = now_ms - mtime_ms;
        if !activity::is_session_active(Some(mtime_age_ms), false) {
            continue;
        }

        // FR-C-48: (パス, サイズ, mtime) が同じならディスクを読み直さない
        let facts = match cache.get(&session_id) {
            Some(c) if c.path == events_path && c.size == size && c.mtime_ms == mtime_ms => {
                c.facts.clone()
            }
            _ => {
                let facts = read_and_classify(&events_path);
                cache.insert(
                    session_id.clone(),
                    CachedTail {
                        path: events_path.clone(),
                        size,
                        mtime_ms,
                        facts: facts.clone(),
                    },
                );
                facts
            }
        };

        // shutdown で終わっているセッションは除外する (ADR-0014)
        if !activity::is_session_active(Some(mtime_age_ms), facts.ended_by_shutdown) {
            continue;
        }

        let meta = std::fs::read_to_string(session_dir.join("workspace.yaml"))
            .map(|t| parser::parse_workspace_yaml(&t))
            .unwrap_or_default();

        // **集合が先。件数はその長さ** (FR-C-51)
        let running_subagent_ids = facts.running_subagent_ids.clone();

        // ① のシグナルは CLI セッションに存在しない (ADR-0014 / ADR-0012)。
        // ② + ③ だけで成立する設計になっている (FR-C-45)
        let activity_state = activity::synthesize(None, facts.tail, running_subagent_ids.len());

        // **中身のタイムスタンプ**を使う。mtime で並べない (FR-P-56 / ADR-0014)。
        // 本文レコードが窓に無ければ workspace.yaml の updated_at に落とす
        let last_activity_at = facts.last_body_at.or(meta.updated_at);

        sessions.push(LiveSession {
            session_id,
            folder_name: meta
                .cwd
                .as_deref()
                .and_then(|c| Path::new(c).file_name())
                .map(|n| n.to_string_lossy().to_string()),
            entrypoint: store::entrypoint_from_client_name(meta.client_name.as_deref()),
            // FR-C-54: 索引済みタイトルへのフォールバックは UI 側が行う。
            // ここは DB を読まないので workspace.yaml の name まで (INV-6 の 140 字)
            title: meta
                .name
                .as_deref()
                .and_then(|n| parser::preview(n, parser::PREVIEW_MAX_CHARS)),
            activity: activity_state,
            model: facts.model.clone(),
            started_at: meta.created_at,
            last_activity_at,
            // 実データに稼働中セッションのコンテキスト使用量は無い。
            // `currentTokens` は `session.shutdown` にしか載らない = 終了後の値。
            // **0 で埋めない** (NFR-40 / NFR-43)。OQ-01 に記録
            context_used: None,
            context_limit: None,
            session_nano_aiu: facts.session_nano_aiu,
            running_subagent_ids,
        });
    }

    // FR-C-48: 一覧から消えたセッションのキャッシュ行は毎回剪定する
    cache.retain(|id, _| seen.contains(id));

    // read_dir の順序は保証されない。2 秒ごとに並びが変わると読めないので固定する
    sessions.sort_by(|a, b| {
        b.last_activity_at
            .cmp(&a.last_activity_at)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });

    LiveStatus::new(sessions, ide_workspaces(&home), newest_log_mtime_ms, now_ms)
}

/// `ide/*.lock` の読み取り先 (FR-C-70〜72 / T-5.13)。
///
/// **`headers` (認証情報) をここに書かない** (INV-2 / FR-C-72)。serde は
/// 未知のフィールドを既定で捨てるので、**宣言しないことがそのまま読まない**
/// ことになる。`skip` では「読んでから捨てる」形になり、値が一度メモリに載る。
#[derive(serde::Deserialize)]
struct IdeLock {
    pid: Option<u32>,
    #[serde(rename = "ideName")]
    ide_name: Option<String>,
    #[serde(rename = "workspaceFolders")]
    workspace_folders: Option<Vec<String>>,
}

/// 接続中 / 切断済みの IDE ワークスペース一覧 (FR-C-70〜72)。
///
/// **PID が死んでいても行を捨てない** (FR-C-71)。stale な lock が実在する。
pub fn ide_workspaces(home: &Path) -> Vec<IdeWorkspace> {
    let mut out: Vec<IdeWorkspace> = std::fs::read_dir(home.join("ide"))
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "lock"))
        .filter_map(|e| {
            // 1 ファイルの失敗で全体を落とさない (NFR-24)
            let text = std::fs::read_to_string(e.path()).ok()?;
            let lock: IdeLock = serde_json::from_str(&text).ok()?;
            Some(IdeWorkspace {
                ide_name: lock.ide_name,
                folders: lock.workspace_folders.unwrap_or_default(),
                // PID の生存確認。プロセスは起動しない (FR-C-42 / INV-4)
                connected: lock
                    .pid
                    .is_some_and(crate::platform::win_job::is_process_alive),
            })
        })
        .collect();

    // read_dir 順は保証されない。接続中を先に、あとはフォルダ名で固定する
    out.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| a.folders.cmp(&b.folders))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copilot::sessions::COPILOT_HOME_ENV_LOCK as ENV_LOCK;
    use crate::copilot::ActivityState;

    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        dir: PathBuf,
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("COPILOT_HOME");
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// **実 `~/.copilot` は絶対に触らない。** 隔離した COPILOT_HOME を使う (INV-1)
    fn setup(name: &str) -> EnvGuard {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "gh-dashboard-live-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("COPILOT_HOME", &dir);
        EnvGuard { _lock: lock, dir }
    }

    fn write_session(home: &Path, id: &str, yaml: &str, events: &str) {
        let dir = home.join("session-state").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("workspace.yaml"), yaml).unwrap();
        std::fs::write(dir.join("events.jsonl"), events).unwrap();
    }

    const YAML: &str = "cwd: D:\\Projects\\Foo\nclient_name: github/cli\nname: \"直してください\"\ncreated_at: 2026-09-07T17:00:00.000Z\nupdated_at: 2026-09-07T17:14:22.666Z\n";

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    #[test]
    fn missing_home_returns_empty_status() {
        let guard = setup("missing-home");
        let mut cache = LiveCache::new();
        // session-state/ も ide/ も作っていない
        let s = collect(now(), &mut cache);
        assert!(s.sessions.is_empty());
        assert!(s.ide_workspaces.is_empty());
        assert_eq!(s.running_session_count, 0);
        assert_eq!(s.newest_log_mtime_ms, None, "0 で埋めない (NFR-43)");
        drop(guard);
    }

    #[test]
    fn fresh_session_with_open_tool_call_is_running() {
        let guard = setup("fresh-tool");
        write_session(
            &guard.dir,
            "s-1",
            YAML,
            "{\"type\":\"user.message\",\"timestamp\":\"2026-09-07T17:14:00.000Z\",\"data\":{\"content\":\"直して\"}}\n\
             {\"type\":\"assistant.message\",\"timestamp\":\"2026-09-07T17:14:10.000Z\",\"data\":{\"content\":\"やります\",\"model\":\"gpt-5\"}}\n\
             {\"type\":\"tool.execution_start\",\"data\":{\"toolCallId\":\"c1\",\"toolName\":\"bash\"}}\n",
        );

        let mut cache = LiveCache::new();
        let s = collect(now(), &mut cache);

        assert_eq!(s.running_session_count, 1);
        let sess = &s.sessions[0];
        assert_eq!(sess.activity, ActivityState::ToolRunning);
        assert_eq!(sess.model.as_deref(), Some("gpt-5"));
        assert_eq!(sess.folder_name.as_deref(), Some("Foo"));
        assert_eq!(sess.title.as_deref(), Some("直してください"));
        // 中身のタイムスタンプ。mtime ではない (FR-P-56)
        assert_eq!(sess.last_activity_at, Some(1_788_801_250_000));
        assert!(sess.started_at.is_some());
        // 取れない値は埋めない (NFR-40)
        assert_eq!(sess.context_used, None);
        assert!(s.newest_log_mtime_ms.is_some());
        drop(guard);
    }

    /// ADR-0014: 末尾が `session.shutdown` なら mtime が新しくても稼働ではない
    #[test]
    fn session_ended_by_shutdown_is_excluded() {
        let guard = setup("shutdown");
        write_session(
            &guard.dir,
            "s-1",
            YAML,
            "{\"type\":\"user.message\",\"data\":{\"content\":\"hi\"}}\n\
             {\"type\":\"session.shutdown\",\"data\":{\"totalNanoAiu\":5}}\n",
        );

        let mut cache = LiveCache::new();
        let s = collect(now(), &mut cache);
        assert!(s.sessions.is_empty());
        // 除外しても mtime のフィードには載る (自動インデックスは追記を取り込む)
        assert!(s.newest_log_mtime_ms.is_some());
        drop(guard);
    }

    /// FR-C-40: 古いセッションは一覧から除外する。**末尾も読まない**
    #[test]
    fn stale_session_is_excluded_without_reading_the_tail() {
        let guard = setup("stale");
        write_session(
            &guard.dir,
            "s-1",
            YAML,
            "{\"type\":\"user.message\",\"data\":{\"content\":\"hi\"}}\n",
        );

        let mut cache = LiveCache::new();
        // now を 1 時間先に進めると mtime 窓 (120 秒) の外になる
        let s = collect(now() + 3_600_000, &mut cache);
        assert!(s.sessions.is_empty());
        assert!(cache.is_empty(), "窓の外なら末尾読みごと飛ばす (FR-C-48)");
        drop(guard);
    }

    /// FR-C-51: 集合が先。件数はその長さ
    #[test]
    fn running_subagents_produce_the_set_and_the_count_together() {
        let guard = setup("subagents");
        write_session(
            &guard.dir,
            "s-1",
            YAML,
            "{\"type\":\"user.message\",\"data\":{\"content\":\"hi\"}}\n\
             {\"type\":\"assistant.turn_end\",\"data\":{\"turnId\":\"0\"}}\n\
             {\"type\":\"subagent.started\",\"data\":{\"toolCallId\":\"a1\",\"agentName\":\"a\"}}\n\
             {\"type\":\"subagent.started\",\"data\":{\"toolCallId\":\"b2\",\"agentName\":\"b\"}}\n\
             {\"type\":\"subagent.completed\",\"data\":{\"toolCallId\":\"b2\"}}\n",
        );

        let mut cache = LiveCache::new();
        let s = collect(now(), &mut cache);
        assert_eq!(s.sessions[0].running_subagent_ids, vec!["a1".to_string()]);
        assert_eq!(s.running_subagent_count, 1);
        // ターン終了 + 稼働中サブエージェントは「サブエージェント実行中」(FR-C-45③)
        assert_eq!(s.sessions[0].activity, ActivityState::SubagentRunning);
        drop(guard);
    }

    /// FR-C-48: 2 回目は (パス, サイズ, mtime) が同じなのでディスクを読み直さない。
    ///
    /// キャッシュに**ファイルには存在しない印**を入れてから 2 回目を回す。
    /// 印が残っていれば、読み直していない証拠になる。
    #[test]
    fn unchanged_file_reuses_the_cached_tail_without_touching_the_disk() {
        let guard = setup("cache-hit");
        write_session(
            &guard.dir,
            "s-1",
            YAML,
            "{\"type\":\"user.message\",\"data\":{\"content\":\"hi\"}}\n",
        );

        let mut cache = LiveCache::new();
        let first = collect(now(), &mut cache);
        assert_eq!(first.sessions[0].activity, ActivityState::Generating);
        assert_eq!(cache.len(), 1);

        // ファイルのどこにも無い値を仕込む
        cache.get_mut("s-1").unwrap().facts.model = Some("印".into());

        let second = collect(now(), &mut cache);
        assert_eq!(
            second.sessions[0].model.as_deref(),
            Some("印"),
            "同じ (サイズ, mtime) で末尾を読み直してはいけない (FR-C-48)"
        );
        drop(guard);
    }

    /// 追記があれば (サイズが変わるので) 読み直す
    #[test]
    fn appended_file_is_re_read() {
        let guard = setup("cache-miss");
        write_session(
            &guard.dir,
            "s-1",
            YAML,
            "{\"type\":\"user.message\",\"data\":{\"content\":\"hi\"}}\n",
        );
        let mut cache = LiveCache::new();
        collect(now(), &mut cache);
        cache.get_mut("s-1").unwrap().facts.model = Some("印".into());

        let events = guard
            .dir
            .join("session-state")
            .join("s-1")
            .join("events.jsonl");
        let mut text = std::fs::read_to_string(&events).unwrap();
        text.push_str("{\"type\":\"assistant.turn_end\",\"data\":{\"turnId\":\"0\"}}\n");
        std::fs::write(&events, text).unwrap();

        let after = collect(now(), &mut cache);
        assert_eq!(after.sessions[0].model, None, "追記があれば読み直す");
        assert_eq!(after.sessions[0].activity, ActivityState::WaitingInput);
        drop(guard);
    }

    /// FR-C-48: 消えたセッションのキャッシュ行は毎回剪定する
    #[test]
    fn cache_rows_for_vanished_sessions_are_pruned() {
        let guard = setup("prune");
        write_session(
            &guard.dir,
            "s-1",
            YAML,
            "{\"type\":\"user.message\",\"data\":{\"content\":\"hi\"}}\n",
        );
        let mut cache = LiveCache::new();
        collect(now(), &mut cache);
        assert_eq!(cache.len(), 1);

        std::fs::remove_dir_all(guard.dir.join("session-state").join("s-1")).unwrap();
        collect(now(), &mut cache);
        assert!(cache.is_empty());
        drop(guard);
    }

    /// FR-C-47: 64KB に本文が無ければ 512KB で 1 回だけ読み直す
    #[test]
    fn widens_the_window_when_no_body_record_is_in_64kb() {
        let guard = setup("wide-window");
        // 本文レコードのあと、64KB を超える量の checkpoint で埋める
        let mut events =
            String::from("{\"type\":\"user.message\",\"timestamp\":\"2026-09-07T17:14:00.000Z\",\"data\":{\"content\":\"hi\"}}\n");
        let filler = "{\"type\":\"session.usage_checkpoint\",\"data\":{\"totalNanoAiu\":1}}\n";
        while events.len() < (TAIL_READ_BYTES as usize) + 2048 {
            events.push_str(filler);
        }
        write_session(&guard.dir, "s-1", YAML, &events);

        let mut cache = LiveCache::new();
        let s = collect(now(), &mut cache);
        let sess = &s.sessions[0];
        // 64KB では user.message に届かない。512KB で読み直して初めて届く
        assert_eq!(sess.activity, ActivityState::Generating);
        assert_eq!(sess.last_activity_at, Some(1_788_801_240_000));
        drop(guard);
    }

    /// 読めないセッションで全体を落とさない (NFR-24)
    #[test]
    fn broken_session_does_not_break_the_others() {
        let guard = setup("broken");
        write_session(
            &guard.dir,
            "ok",
            YAML,
            "{\"type\":\"user.message\",\"data\":{}}\n",
        );
        // workspace.yaml が無く、events.jsonl も壊れている
        let bad = guard.dir.join("session-state").join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("events.jsonl"), "not json at all\n").unwrap();

        let mut cache = LiveCache::new();
        let s = collect(now(), &mut cache);
        // 壊れたほうも「不明」として残る (空欄にしない / FR-C-44)
        assert_eq!(s.running_session_count, 2);
        let broken = s.sessions.iter().find(|x| x.session_id == "bad").unwrap();
        assert_eq!(broken.activity, ActivityState::Unknown);
        assert_eq!(broken.title, None);
        drop(guard);
    }

    // ---- IDE ワークスペース (FR-C-70〜72) ----

    /// FR-C-71: 死んだ PID でも行を残し、切断済みにする
    #[test]
    fn dead_pid_is_kept_as_disconnected() {
        let guard = setup("ide-dead");
        let ide = guard.dir.join("ide");
        std::fs::create_dir_all(&ide).unwrap();
        std::fs::write(
            ide.join("a.lock"),
            r#"{"socketPath":"\\\\.\\pipe\\x","scheme":"pipe","headers":{"Authorization":"Bearer SECRET"},"pid":4294967280,"ideName":"Visual Studio Code","workspaceFolders":["d:\\proj"],"isTrusted":true}"#,
        )
        .unwrap();

        let ws = ide_workspaces(&guard.dir);
        assert_eq!(ws.len(), 1, "行を捨てない (FR-C-71)");
        assert!(!ws[0].connected);
        assert_eq!(ws[0].ide_name.as_deref(), Some("Visual Studio Code"));
        assert_eq!(ws[0].folders, vec!["d:\\proj".to_string()]);

        // INV-2 / FR-C-72: 認証情報がどこにも漏れない
        let json = serde_json::to_string(&ws[0]).unwrap();
        assert!(!json.contains("SECRET"));
        assert!(!json.to_lowercase().contains("authorization"));
        drop(guard);
    }

    #[test]
    fn live_process_lock_is_connected() {
        let guard = setup("ide-live");
        let ide = guard.dir.join("ide");
        std::fs::create_dir_all(&ide).unwrap();
        // 自分自身の PID を書く = 確実に生きている
        std::fs::write(
            ide.join("b.lock"),
            format!(
                r#"{{"pid":{},"ideName":"Visual Studio Code","workspaceFolders":["d:\\proj"]}}"#,
                std::process::id()
            ),
        )
        .unwrap();

        let ws = ide_workspaces(&guard.dir);
        assert_eq!(ws.len(), 1);
        #[cfg(windows)]
        assert!(ws[0].connected);
        drop(guard);
    }

    /// 実 `~/.copilot` に対する**読み取り専用**の目視確認 (NFR-53 / ADR-0014 の宿題)。
    ///
    /// 既定では走らない。Copilot CLI を実際に動かしながら
    /// `cargo test --lib probe_real_home_readonly -- --ignored --nocapture`
    /// を回すと、「稼働中に見える / 終了後に消える」を実機で確かめられる。
    /// **何も書かない** (INV-1)。
    #[test]
    #[ignore]
    fn probe_real_home_readonly() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("COPILOT_HOME");
        let mut cache = LiveCache::new();
        let t = std::time::Instant::now();
        let s = collect(now(), &mut cache);
        println!("elapsed={:?}", t.elapsed());
        println!(
            "sessions={} subagents={} ide={} newest_mtime={:?} cache_rows={}",
            s.running_session_count,
            s.running_subagent_count,
            s.ide_workspaces.len(),
            s.newest_log_mtime_ms,
            cache.len()
        );
        for w in &s.ide_workspaces {
            println!(
                "  IDE {:?} connected={} {:?}",
                w.ide_name, w.connected, w.folders
            );
        }
        for x in &s.sessions {
            println!("  S {} {:?} {:?}", x.session_id, x.activity, x.folder_name);
        }
        let t2 = std::time::Instant::now();
        let _ = collect(now(), &mut cache);
        println!("2nd elapsed={:?}", t2.elapsed());
    }

    #[test]
    fn broken_lock_file_is_skipped_without_killing_the_list() {
        let guard = setup("ide-broken");
        let ide = guard.dir.join("ide");
        std::fs::create_dir_all(&ide).unwrap();
        std::fs::write(ide.join("a.lock"), "not json").unwrap();
        std::fs::write(ide.join("b.lock"), r#"{"ideName":"X"}"#).unwrap();
        std::fs::write(ide.join("notes.txt"), r#"{"ideName":"Y"}"#).unwrap();

        let ws = ide_workspaces(&guard.dir);
        assert_eq!(ws.len(), 1, ".lock だけ / 壊れた 1 件は読み飛ばす");
        assert_eq!(ws[0].ide_name.as_deref(), Some("X"));
        // pid が欠けていても行は残り、接続状態は false (NFR-23)
        assert!(!ws[0].connected);
        assert!(ws[0].folders.is_empty());
        drop(guard);
    }
}
