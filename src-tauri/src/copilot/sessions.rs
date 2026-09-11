//! セッション履歴の部分読み (IO)。読み取り専用 (INV-1)。
//!
//! `<COPILOT_HOME>/session-state/<uuid>/` の `workspace.yaml` (全文) と
//! `events.jsonl` (末尾ウィンドウのみ) を読み、`SessionCandidate` を作る。
//! **同期関数。** 呼び出し側 (`projects::commands`) が `spawn_blocking` で包む
//! (INV-10 / NFR-20)。1 セッションの失敗で全体を落とさない (NFR-24)。
//!
//! 対応要求: FR-P-50〜58 / NFR-20 / NFR-24 / NFR-43 / INV-1 / INV-10

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::copilot::{parser, SessionCandidate, TimeSource};
use crate::util::path_key;

/// 末尾読みの窓幅 (FR-P-55)。
/// OQ-01 の実測でレコード長は p95 17,361 / max 36,428 バイト。
/// 数KBだと1レコードにも足りないため、max の約1.8倍を取る。
pub const TAIL_READ_BYTES: u64 = 64 * 1024;

/// `COPILOT_HOME` 環境変数 → 無ければ `~/.copilot`。
/// `COPILOT_HOME` は Copilot CLI 公式の上書き手段 (付録A.1)。テストや実機確認で
/// 実データを一切触らずに検証できる。
pub fn copilot_home() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("COPILOT_HOME") {
        return Some(PathBuf::from(v));
    }
    dirs::home_dir().map(|h| h.join(".copilot"))
}

#[derive(Debug, Default)]
pub struct CollectResult {
    pub candidates: Vec<SessionCandidate>,
    pub unreadable_sessions: usize,
    pub skipped_lines: u64,
}

/// ファイル末尾から最大 `max_bytes` を読む。
/// 戻り値の bool は「ファイル先頭から読めたか」= `drop_leading_partial` の引数。
pub fn read_tail(path: &Path, max_bytes: u64) -> std::io::Result<(Vec<u8>, bool)> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len <= max_bytes {
        let mut buf = Vec::with_capacity(len as usize);
        file.read_to_end(&mut buf)?;
        Ok((buf, true))
    } else {
        file.seek(SeekFrom::Start(len - max_bytes))?;
        let mut buf = Vec::with_capacity(max_bytes as usize);
        file.read_to_end(&mut buf)?;
        Ok((buf, false))
    }
}

/// `session-state/*` を列挙して候補を作る。同期関数。
/// 呼び出し側が `spawn_blocking` で包む (INV-10 / NFR-20)。
pub fn collect_candidates(now_ms: i64) -> CollectResult {
    let mut result = CollectResult::default();

    let Some(home) = copilot_home() else {
        return result;
    };
    let session_state_dir = home.join("session-state");
    let Ok(entries) = std::fs::read_dir(&session_state_dir) else {
        // session-state/ が無い = 未使用と区別できないので警告は出さない
        return result;
    };

    // 同一 cwd の is_dir() 呼び出しを重複させない (path_key ごとに 1 回だけ stat する)
    let mut exists_cache: HashMap<String, bool> = HashMap::new();

    for entry in entries.flatten() {
        let session_dir = entry.path();
        if !session_dir.is_dir() {
            continue;
        }
        let session_id = entry.file_name().to_string_lossy().to_string();

        let Ok(yaml_text) = std::fs::read_to_string(session_dir.join("workspace.yaml")) else {
            result.unreadable_sessions += 1;
            continue;
        };
        let meta = parser::parse_workspace_yaml(&yaml_text);

        let cwd_raw = meta.cwd.clone();
        let key = cwd_raw.as_deref().and_then(path_key::path_key);
        let folder_name = cwd_raw
            .as_deref()
            .and_then(|c| Path::new(c).file_name())
            .map(|n| n.to_string_lossy().to_string());

        let cwd_exists = match (&key, &cwd_raw) {
            (Some(k), Some(raw)) => *exists_cache
                .entry(k.clone())
                .or_insert_with(|| Path::new(raw).is_dir()),
            _ => false,
        };

        let events_path = session_dir.join("events.jsonl");
        let tail = std::fs::metadata(&events_path).ok().map(|md| {
            let mtime_ms = md.modified().ok().and_then(|m| {
                m.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_millis() as i64)
            });
            (md.len(), mtime_ms)
        });

        let mut tail_facts = parser::TailFacts::default();
        let mut mtime_ms: Option<i64> = None;
        if let Some((_len, mt)) = tail {
            mtime_ms = mt;
            if let Ok((buf, from_start)) = read_tail(&events_path, TAIL_READ_BYTES) {
                let dropped = crate::copilot::delta::drop_leading_partial(&buf, from_start);
                let committed = crate::copilot::delta::split_committed(dropped);
                tail_facts = parser::scan_tail(&committed.lines);
                result.skipped_lines += tail_facts.skipped_lines as u64;
            }
        }

        let (last_used_at, last_used_source) = if let Some(ts) = tail_facts.last_timestamp {
            (Some(ts), TimeSource::EventsTail)
        } else if let Some(ts) = meta.updated_at {
            (Some(ts), TimeSource::WorkspaceYaml)
        } else {
            (None, TimeSource::None)
        };

        let mtime_age_ms = mtime_ms.map(|mt| now_ms - mt);
        let is_active =
            crate::copilot::activity::is_session_active(mtime_age_ms, tail_facts.ended_by_shutdown);

        let title = meta
            .name
            .as_deref()
            .and_then(|n| parser::preview(n, parser::PREVIEW_MAX_CHARS));

        result.candidates.push(SessionCandidate {
            session_id,
            cwd_raw,
            path_key: key,
            folder_name,
            cwd_exists,
            client_name: meta.client_name,
            title,
            last_used_at,
            last_used_source,
            total_nano_aiu: tail_facts.total_nano_aiu,
            lines_added: tail_facts.lines_added,
            lines_removed: tail_facts.lines_removed,
            is_active,
        });
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `COPILOT_HOME` はプロセス全体の環境変数。テストは並列実行されるので
    // このモジュール内のテストだけを直列化する。
    // ponytail: グローバルロック。他モジュールが COPILOT_HOME を触るようになったら見直す。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

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

    fn setup(name: &str) -> EnvGuard {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "gh-dashboard-copilot-sessions-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("COPILOT_HOME", &dir);
        EnvGuard { _lock: lock, dir }
    }

    fn write_session(home: &Path, id: &str, yaml: &str, events: Option<&str>) {
        let dir = home.join("session-state").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("workspace.yaml"), yaml).unwrap();
        if let Some(events) = events {
            std::fs::write(dir.join("events.jsonl"), events).unwrap();
        }
    }

    #[test]
    fn no_session_state_dir_returns_empty_result() {
        let guard = setup("no-session-state");
        let result = collect_candidates(0);
        assert!(result.candidates.is_empty());
        assert_eq!(result.unreadable_sessions, 0);
        drop(guard);
    }

    #[test]
    fn valid_session_produces_one_candidate() {
        let guard = setup("valid-session");
        let cwd = guard.dir.join("work-b");
        std::fs::create_dir_all(&cwd).unwrap();
        let yaml = format!(
            "id: abc-123\ncwd: {}\nclient_name: github/cli\nname: \"Fix the bug\"\nupdated_at: 2026-09-07T17:14:22.666Z\n",
            cwd.display()
        );
        let events = r#"{"type":"session.shutdown","timestamp":"2026-09-07T17:14:22.666Z","data":{"totalNanoAiu":5,"codeChanges":{"linesAdded":1,"linesRemoved":0}}}
"#;
        write_session(&guard.dir, "abc-123", &yaml, Some(events));

        let result = collect_candidates(0);
        assert_eq!(result.unreadable_sessions, 0);
        assert_eq!(result.candidates.len(), 1);
        let c = &result.candidates[0];
        assert_eq!(c.session_id, "abc-123");
        assert!(c.cwd_exists);
        assert_eq!(c.title.as_deref(), Some("Fix the bug"));
        assert_eq!(c.last_used_source, TimeSource::EventsTail);
        assert_eq!(c.total_nano_aiu, Some(5));
    }

    #[test]
    fn missing_workspace_yaml_is_skipped_and_counted() {
        let guard = setup("missing-yaml");
        let dir = guard.dir.join("session-state").join("broken-1");
        std::fs::create_dir_all(&dir).unwrap();
        // workspace.yaml を書かない

        let result = collect_candidates(0);
        assert_eq!(result.unreadable_sessions, 1);
        assert!(result.candidates.is_empty());
    }

    #[test]
    fn read_tail_reads_only_the_tail_of_a_large_file() {
        let guard = setup("large-events");
        let path = guard.dir.join("big.jsonl");
        let line = "{\"type\":\"session.usage_checkpoint\"}\n";
        let repeat = (TAIL_READ_BYTES as usize / line.len()) + 10;
        let content = line.repeat(repeat);
        std::fs::write(&path, &content).unwrap();

        let (buf, from_start) = read_tail(&path, TAIL_READ_BYTES).unwrap();
        assert!(!from_start);
        assert_eq!(buf.len() as u64, TAIL_READ_BYTES);
        assert_eq!(&buf[buf.len() - line.len()..], line.as_bytes());
        drop(guard);
    }
}
