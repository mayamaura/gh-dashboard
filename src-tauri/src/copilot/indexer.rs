//! 差分インデックス (IO)。**読み取り専用** — `~/.copilot/**` に書き込まない (INV-1)。
//!
//! `<COPILOT_HOME>/session-state/<uuid>/events.jsonl` を全期間ぶん差分パースし、
//! 索引と集計を DB に入れる (FR-C-01)。**本文は複製しない** (FR-C-02 / INV-6)。
//!
//! この経路は 2 秒ポーリングから呼ばれない。重い処理をここに置いてよいのは
//! そのため (INV-4 は `live` 側の制約)。
//!
//! すべて同期関数。呼び出し側が `spawn_blocking` で包む (INV-10 / NFR-20)。
//!
//! 対応要求: FR-C-01〜14 / 20〜23 / 28 / 29 / NFR-24

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use crate::copilot::parser::{self, RecordFacts, SubagentFacts};
use crate::copilot::{delta, sessions, store, IndexProgress};
use crate::util::path_key;

/// バッチの行数 (FR-C-09)。
pub const BATCH_LINES: usize = 3_000;

/// バッチ間で DB ロックを手放す時間 (FR-C-09)。
/// **他機能の DB 利用と競合させないための待ち**なので、削ると 2 秒ポーリング側が待たされる。
pub const BATCH_SLEEP: std::time::Duration = std::time::Duration::from_millis(15);

/// 1 行の読み込み上限。1 レコードが数百 KB〜数 MB になりうる (FR-C-08) が、
/// 無制限に読むと壊れたファイル 1 つでメモリを食い潰す。超えた行は壊れた行として数える。
const MAX_LINE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexStats {
    pub total_files: usize,
    /// 追記があって実際に読んだファイル
    pub updated_files: usize,
    /// **今回新しく索引に入った**レコード数。2 回目の実行では 0 になる
    pub records_ingested: u64,
    /// パースできなかった行。無言で欠落させない (FR-C-12 / NFR-43)
    pub skipped_lines: u64,
    /// 読めなかったファイル。1 ファイルの失敗で全体を止めない (NFR-24)
    pub failed_files: usize,
}

/// 1 セッション分の入力。
struct SessionFile {
    session_id: String,
    events_path: PathBuf,
    workspace_path: PathBuf,
    size: u64,
    mtime_ms: Option<i64>,
}

/// バッチに溜める 1 レコード。**行の中身は持たない** (facts だけ / INV-6)。
struct Pending {
    byte_offset: u64,
    byte_length: u64,
    facts: RecordFacts,
}

/// ファイルをまたがずに溜めるセッション集計 (FR-C-20)。
#[derive(Default)]
struct Accum {
    row: store::SessionRow,
    subagents: Vec<(SubagentFacts, Option<String>, Option<i64>)>,
    quota_events: Vec<(i64, String, Option<String>, Option<String>)>,
}

impl Accum {
    /// 1 レコードぶんを畳み込む。
    fn absorb(&mut self, facts: &RecordFacts) {
        if let Some(ts) = facts.timestamp_ms {
            self.row.last_activity_at = Some(self.row.last_activity_at.unwrap_or(ts).max(ts));
        }
        if facts.model.is_some() {
            self.row.model = facts.model.clone();
        }

        if let Some(s) = &facts.session {
            // shutdown があればそれを主ソースにする (ADR-0013)。
            // 累計値なので、後に来たものほど正しい = 上書きでよい
            if s.cwd.is_some() {
                self.row.cwd = s.cwd.clone();
            }
            if s.started_at.is_some() {
                self.row.started_at = s.started_at;
            }
            if s.model.is_some() {
                self.row.model = s.model.clone();
            }
            if s.total_nano_aiu.is_some() {
                self.row.total_nano_aiu = s.total_nano_aiu;
            }
            if s.from_shutdown {
                // tokenDetails を持たない shutdown が実データにある。
                // 取れた枠だけ更新し、取れなかった枠は前の値を残す (NFR-23)
                for (dst, src) in [
                    (&mut self.row.input_tokens, s.input_tokens),
                    (&mut self.row.output_tokens, s.output_tokens),
                    (&mut self.row.cache_write_tokens, s.cache_write_tokens),
                    (&mut self.row.cache_read_tokens, s.cache_read_tokens),
                ] {
                    if src.is_some() {
                        *dst = src;
                    }
                }
            }
        }

        if let Some(sa) = &facts.subagent {
            self.subagents
                .push((sa.clone(), facts.agent_id.clone(), facts.timestamp_ms));
        }

        // FR-C-28 / T-4.11。現状この関数は常に None を返す (OQ-01: 種別が未観測)。
        // 呼び出し口を残すのは、実物を観測した日に 1 箇所直せば通るようにするため
        if let Some((kind, reset_text, raw)) = parser::parse_quota_event(facts) {
            if let Some(ts) = facts.timestamp_ms {
                self.quota_events.push((ts, kind, reset_text, raw));
            }
        }
    }
}

/// `session-state/*/events.jsonl` を列挙する。読めないものは黙って飛ばす。
fn collect_files(home: &Path) -> Vec<SessionFile> {
    let Ok(entries) = std::fs::read_dir(home.join("session-state")) else {
        // session-state/ が無い = Copilot 未使用と区別できない。正常系として空を返す
        return Vec::new();
    };

    let mut files = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let events_path = dir.join("events.jsonl");
        let Ok(md) = std::fs::metadata(&events_path) else {
            continue;
        };
        files.push(SessionFile {
            session_id: entry.file_name().to_string_lossy().to_string(),
            workspace_path: dir.join("workspace.yaml"),
            events_path,
            size: md.len(),
            mtime_ms: md.modified().ok().and_then(|m| {
                m.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_millis() as i64)
            }),
        });
    }
    files.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    files
}

/// `workspace.yaml` からセッション行の固定項目を作る (FR-P-51 / FR-C-54)。
///
/// 読めなくても続行する — `events.jsonl` から取れる分だけで行は作れる (NFR-24)。
fn session_row_from_workspace(session_id: &str, workspace_path: &Path) -> store::SessionRow {
    let mut row = store::SessionRow {
        session_id: session_id.to_string(),
        ..Default::default()
    };
    let Ok(text) = std::fs::read_to_string(workspace_path) else {
        return row;
    };
    let meta = parser::parse_workspace_yaml(&text);
    row.path_key = meta.cwd.as_deref().and_then(path_key::path_key);
    row.folder_name = meta
        .cwd
        .as_deref()
        .and_then(|c| Path::new(c).file_name())
        .map(|n| n.to_string_lossy().to_string());
    row.cwd = meta.cwd;
    // name は初回プロンプトの本文がそのまま入る。必ずプレビューを掛ける (INV-6)
    row.title = meta
        .name
        .as_deref()
        .and_then(|n| parser::preview(n, parser::PREVIEW_MAX_CHARS));
    // 生の client_name を入れる。表示用の分類は読み出し時に行う (NFR-40)
    row.entrypoint = meta.client_name;
    row.started_at = meta.created_at;
    // git ブランチはここでは埋めない。埋めるには cwd ごとに git を起動することになり、
    // インデックス 1 回で数十プロセスを起こす。プロジェクト一覧側が実時間で持っている
    row
}

/// 1 バッチを**ひとつのトランザクション**で書く (FR-C-04 / 09)。
///
/// 順序が肝: レコード → サブエージェント → 集計 → **最後にオフセット**。
/// 途中で落ちればトランザクションごと巻き戻り、オフセットは進まない。
/// 次回は同じ位置から読み直し、UNIQUE 制約が二重挿入を吸収する (FR-C-07)。
#[allow(clippy::too_many_arguments)]
fn flush_batch(
    db: &Arc<Mutex<Connection>>,
    file: &SessionFile,
    pending: &mut Vec<Pending>,
    accum: &mut Accum,
    offset: u64,
    now_ms: i64,
    stats: &mut IndexStats,
) -> anyhow::Result<()> {
    let path = file.events_path.to_string_lossy().to_string();
    let mut conn = db
        .lock()
        .map_err(|_| anyhow::anyhow!("DB ロックの取得に失敗しました"))?;
    let tx = conn.transaction()?;

    for p in pending.iter() {
        if store::insert_turn(
            &tx,
            &store::TurnRow {
                session_id: &file.session_id,
                file_path: &path,
                byte_offset: p.byte_offset,
                byte_length: p.byte_length,
                facts: &p.facts,
            },
        )? {
            stats.records_ingested += 1;
        }
    }

    for (facts, agent_id, ts) in accum.subagents.drain(..) {
        store::merge_subagent(
            &tx,
            &file.session_id,
            &path,
            &facts,
            agent_id.as_deref(),
            ts,
            now_ms,
        )?;
    }

    for (occurred_at, kind, reset_text, raw) in accum.quota_events.drain(..) {
        store::insert_quota_event(
            &tx,
            &file.session_id,
            occurred_at,
            &kind,
            reset_text.as_deref(),
            raw.as_deref(),
        )?;
    }

    // 集計は毎バッチ書く。ファイル末尾でまとめて書くと、途中で落ちたときに
    // 「オフセットだけ進んで集計は空」という復旧できない状態になる
    store::upsert_session(&tx, &accum.row, now_ms)?;
    store::refresh_session_counts(&tx, &file.session_id)?;

    // ---- 同一トランザクション内の最後 (FR-C-04) ----
    store::commit_offset(
        &tx,
        &path,
        &file.session_id,
        offset,
        file.size,
        file.mtime_ms,
        now_ms,
    )?;
    tx.commit()?;
    pending.clear();
    Ok(())
}

/// 1 ファイルを差分パースする。戻り値は「実際に読んだか」。
fn index_file(
    db: &Arc<Mutex<Connection>>,
    file: &SessionFile,
    now_ms: i64,
    stats: &mut IndexStats,
    progress: &mut dyn FnMut(&IndexStats, Option<&str>),
) -> anyhow::Result<bool> {
    let path = file.events_path.to_string_lossy().to_string();

    let recorded = {
        let conn = db
            .lock()
            .map_err(|_| anyhow::anyhow!("DB ロックの取得に失敗しました"))?;
        store::file_progress(&conn, &path)?
    };

    // **mtime で新旧を判定しない。** サイズとオフセットだけで決める (FR-C-06 / FR-P-56)
    let mut offset = match delta::decide(
        recorded.last_parsed_offset,
        recorded.file_size_at_parse,
        file.size,
    ) {
        delta::Delta::UpToDate => return Ok(false),
        delta::Delta::Continue { from } => from,
        delta::Delta::Reparse => {
            // truncate / 入れ替わり。古い行を捨ててから先頭に戻る
            let mut conn = db
                .lock()
                .map_err(|_| anyhow::anyhow!("DB ロックの取得に失敗しました"))?;
            let tx = conn.transaction()?;
            store::clear_file(&tx, &path)?;
            tx.commit()?;
            0
        }
    };

    let mut accum = Accum {
        row: session_row_from_workspace(&file.session_id, &file.workspace_path),
        ..Default::default()
    };

    let mut handle = std::fs::File::open(&file.events_path)?;
    handle.seek(SeekFrom::Start(offset))?;
    // 行単位ストリーミング。ファイル全体をメモリに載せない (FR-C-08)
    let mut reader = BufReader::with_capacity(64 * 1024, handle);
    let mut pending: Vec<Pending> = Vec::with_capacity(BATCH_LINES);
    let mut line = Vec::new();

    loop {
        line.clear();
        let read = (&mut reader)
            .take(MAX_LINE_BYTES)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }

        // 改行で終わらない末尾断片は確定させない。この分をオフセットに含めると、
        // 次回に本来の行を丸ごと飛ばす (FR-C-05)
        let Some(body) = delta::committed_line(&line) else {
            break;
        };

        let byte_offset = offset;
        offset += read as u64;

        if !body.is_empty() {
            match std::str::from_utf8(body).ok().and_then(parser::parse_record) {
                Some(facts) => {
                    accum.absorb(&facts);
                    pending.push(Pending {
                        byte_offset,
                        byte_length: body.len() as u64,
                        facts,
                    });
                }
                // 壊れた行は数えて読み飛ばし、他行の処理を続ける (FR-C-12)
                None => stats.skipped_lines += 1,
            }
        }

        if pending.len() >= BATCH_LINES {
            flush_batch(db, file, &mut pending, &mut accum, offset, now_ms, stats)?;
            progress(stats, Some(&path));
            // DB ロックを手放す (FR-C-09)
            std::thread::sleep(BATCH_SLEEP);
        }
    }

    // 端数 + 集計の確定。pending が空でも集計とオフセットは書く
    flush_batch(db, file, &mut pending, &mut accum, offset, now_ms, stats)?;
    Ok(true)
}

/// 全セッションを差分インデックスする (FR-C-01)。
///
/// `progress` は「統計 + 現在のファイル」で呼ばれる。**間引きは呼び出し側の責任** —
/// ここで間引くと、テストから進捗の刻みが見えなくなる (FR-C-11 は通知側の要求)。
pub fn run(
    db: &Arc<Mutex<Connection>>,
    home: &Path,
    now_ms: i64,
    mut progress: impl FnMut(&IndexProgress),
) -> IndexStats {
    let files = collect_files(home);
    let mut stats = IndexStats {
        total_files: files.len(),
        ..Default::default()
    };

    let mut done_files = 0usize;
    emit(&mut progress, "scan", &stats, 0, None);

    for file in &files {
        let path = file.events_path.to_string_lossy().to_string();
        emit(&mut progress, "index", &stats, done_files, Some(&path));

        // 1 ファイルの失敗で全体を止めない (NFR-24)
        let result = {
            let mut batch_progress = |stats: &IndexStats, current: Option<&str>| {
                emit(&mut progress, "index", stats, done_files, current);
            };
            index_file(db, file, now_ms, &mut stats, &mut batch_progress)
        };
        match result {
            Ok(true) => stats.updated_files += 1,
            Ok(false) => {}
            Err(e) => {
                stats.failed_files += 1;
                tracing::warn!(file = %path, error = %e, "セッションログの索引に失敗");
            }
        }
        done_files += 1;
    }

    // 最終通知は間引かない (FR-C-11)
    emit(&mut progress, "done", &stats, done_files, None);
    stats
}

fn emit<F: FnMut(&IndexProgress)>(
    progress: &mut F,
    phase: &str,
    stats: &IndexStats,
    done_files: usize,
    current_file: Option<&str>,
) {
    progress(&IndexProgress {
        phase: phase.to_string(),
        total_files: stats.total_files,
        done_files,
        current_file: current_file.map(|s| s.to_string()),
        records_ingested: stats.records_ingested,
        skipped_lines: stats.skipped_lines,
    });
}

/// 既定の入力元。`COPILOT_HOME` → 無ければ `~/.copilot` (INV-1: 読み取りのみ)。
pub fn default_home() -> Option<PathBuf> {
    sessions::copilot_home()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    const NOW: i64 = 1_800_000_000_000;

    struct Fixture {
        dir: PathBuf,
        db: Arc<Mutex<Connection>>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(name: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "gh-dashboard-indexer-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Fixture {
            dir,
            db: Arc::new(Mutex::new(open_in_memory().unwrap())),
        }
    }

    fn write_session(home: &Path, id: &str, events: &str) {
        let dir = home.join("session-state").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("workspace.yaml"),
            "cwd: D:\\Projects\\Foo\nclient_name: github/cli\nname: \"バグを直す\"\ncreated_at: 2026-09-07T17:00:00.000Z\n",
        )
        .unwrap();
        std::fs::write(dir.join("events.jsonl"), events).unwrap();
    }

    /// 実データの並びを縮めたもの (start → user → assistant → shutdown)
    fn sample_events() -> String {
        [
            r#"{"type":"session.start","id":"r1","parentId":null,"timestamp":"2026-09-07T17:00:00.000Z","data":{"sessionId":"s","startTime":"2026-09-07T17:00:00.000Z","context":{"cwd":"D:\\Projects\\Foo"}}}"#,
            r#"{"type":"user.message","id":"r2","parentId":"r1","timestamp":"2026-09-07T17:00:10.000Z","data":{"content":"バグを直して"}}"#,
            r#"{"type":"assistant.message","id":"r3","parentId":"r2","timestamp":"2026-09-07T17:00:20.000Z","data":{"model":"gpt-5","content":"直しました","outputTokens":42}}"#,
            r#"{"type":"session.shutdown","id":"r4","parentId":"r3","timestamp":"2026-09-07T17:00:30.000Z","data":{"totalNanoAiu":382635000,"currentModel":"gpt-5","sessionStartTime":1788800000000,"tokenDetails":{"input":{"tokenCount":100},"output":{"tokenCount":42},"cache_read":{"tokenCount":7},"cache_write":{"tokenCount":3}}}}"#,
        ]
        .join("\n")
            + "\n"
    }

    fn count(db: &Arc<Mutex<Connection>>, sql: &str) -> i64 {
        db.lock()
            .unwrap()
            .query_row(sql, [], |r| r.get(0))
            .unwrap_or(0)
    }

    /// FR-C-03 / 07: 2 回目の新規レコードが 0 件になる (差分インデックスの核心)
    #[test]
    fn second_run_ingests_nothing_new() {
        let fx = fixture("second-run");
        write_session(&fx.dir, "s1", &sample_events());

        let first = run(&fx.db, &fx.dir, NOW, |_| {});
        assert_eq!(first.total_files, 1);
        assert_eq!(first.updated_files, 1);
        assert_eq!(first.records_ingested, 4);
        assert_eq!(first.skipped_lines, 0);

        let second = run(&fx.db, &fx.dir, NOW + 1000, |_| {});
        assert_eq!(second.records_ingested, 0, "追記が無ければ 1 件も入らない");
        assert_eq!(second.updated_files, 0, "サイズが同じなら読みに行かない");
        assert_eq!(count(&fx.db, "SELECT COUNT(*) FROM turn_index"), 4);
    }

    /// FR-C-03: 追記した分だけが入る
    #[test]
    fn appended_records_are_the_only_new_ones() {
        let fx = fixture("append");
        write_session(&fx.dir, "s1", &sample_events());
        run(&fx.db, &fx.dir, NOW, |_| {});

        let path = fx.dir.join("session-state/s1/events.jsonl");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str(
            r#"{"type":"user.message","id":"r5","parentId":"r4","timestamp":"2026-09-07T18:00:00.000Z","data":{"content":"もう一つ"}}"#,
        );
        text.push('\n');
        std::fs::write(&path, text).unwrap();

        let again = run(&fx.db, &fx.dir, NOW + 1000, |_| {});
        assert_eq!(again.records_ingested, 1);
        assert_eq!(count(&fx.db, "SELECT COUNT(*) FROM turn_index"), 5);
    }

    /// FR-C-05: 書きかけの最終行は確定させず、オフセットにも含めない
    #[test]
    fn trailing_fragment_is_picked_up_after_it_completes() {
        let fx = fixture("fragment");
        let mut text = sample_events();
        // 改行なしの書きかけ行を足す
        let fragment = r#"{"type":"user.message","id":"r5","timestamp":"2026-09-07T18:00:00.000Z","data":{"content":"書きかけ"#;
        text.push_str(fragment);
        write_session(&fx.dir, "s1", &text);

        let first = run(&fx.db, &fx.dir, NOW, |_| {});
        assert_eq!(first.records_ingested, 4, "断片を確定扱いしない");
        assert_eq!(first.skipped_lines, 0, "断片は壊れた行でもない");

        // 書きかけが完成する
        let path = fx.dir.join("session-state/s1/events.jsonl");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("\"}}\n");
        std::fs::write(&path, text).unwrap();

        let second = run(&fx.db, &fx.dir, NOW + 1000, |_| {});
        assert_eq!(second.records_ingested, 1, "完成した行が改めて入る");
        assert_eq!(second.skipped_lines, 0);
    }

    /// FR-C-06: サイズが縮んだら全再パースし、古い行を残さない
    #[test]
    fn shrunk_file_is_reparsed_from_zero() {
        let fx = fixture("shrink");
        write_session(&fx.dir, "s1", &sample_events());
        run(&fx.db, &fx.dir, NOW, |_| {});
        assert_eq!(count(&fx.db, "SELECT COUNT(*) FROM turn_index"), 4);

        // 別の内容に置き換わる (truncate / 入れ替わり)
        let path = fx.dir.join("session-state/s1/events.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"user.message\",\"id\":\"x\",\"timestamp\":\"2026-09-08T00:00:00.000Z\",\"data\":{\"content\":\"別物\"}}\n",
        )
        .unwrap();

        let again = run(&fx.db, &fx.dir, NOW + 1000, |_| {});
        assert_eq!(again.records_ingested, 1);
        assert_eq!(
            count(&fx.db, "SELECT COUNT(*) FROM turn_index"),
            1,
            "古い行が残っていると、同じオフセットに別の内容が 2 つあることになる"
        );
    }

    /// FR-C-12 / NFR-24: 壊れた行を数えて飛ばし、前後の行は処理を続ける
    #[test]
    fn broken_lines_are_counted_and_the_rest_survives() {
        let fx = fixture("broken");
        let text = format!(
            "{}\n{}\n{}\n",
            r#"{"type":"user.message","id":"a","timestamp":"2026-09-07T17:00:00.000Z","data":{"content":"1"}}"#,
            "}{ not json at all",
            r#"{"type":"user.message","id":"b","timestamp":"2026-09-07T17:00:01.000Z","data":{"content":"2"}}"#,
        );
        write_session(&fx.dir, "s1", &text);

        let stats = run(&fx.db, &fx.dir, NOW, |_| {});
        assert_eq!(stats.skipped_lines, 1);
        assert_eq!(stats.records_ingested, 2);
    }

    /// NFR-24: 1 セッションが壊れても他セッションは入る
    #[test]
    fn one_unreadable_session_does_not_stop_the_others() {
        let fx = fixture("isolation");
        write_session(&fx.dir, "s1", &sample_events());
        // events.jsonl を持たないディレクトリは列挙対象にならない
        std::fs::create_dir_all(fx.dir.join("session-state/empty-dir")).unwrap();
        write_session(&fx.dir, "s2", &sample_events());

        let stats = run(&fx.db, &fx.dir, NOW, |_| {});
        assert_eq!(stats.total_files, 2);
        assert_eq!(count(&fx.db, "SELECT COUNT(*) FROM sessions"), 2);
    }

    /// FR-C-20: 集計は session.shutdown が主ソース
    #[test]
    fn session_aggregate_comes_from_shutdown() {
        let fx = fixture("aggregate");
        write_session(&fx.dir, "s1", &sample_events());
        run(&fx.db, &fx.dir, NOW, |_| {});

        let conn = fx.db.lock().unwrap();
        let (title, cwd, folder, entry, aiu, input, output, turns): (
            String,
            String,
            String,
            String,
            i64,
            i64,
            i64,
            i64,
        ) = conn
            .query_row(
                "SELECT title, cwd, folder_name, entrypoint, total_nano_aiu, input_tokens, output_tokens, turn_count FROM sessions",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
            )
            .unwrap();
        assert_eq!(title, "バグを直す");
        assert_eq!(cwd, "D:\\Projects\\Foo");
        assert_eq!(folder, "Foo");
        assert_eq!(entry, "github/cli", "生の client_name を残す");
        assert_eq!(aiu, 382_635_000);
        assert_eq!(input, 100);
        assert_eq!(output, 42);
        assert_eq!(turns, 4);
    }

    /// shutdown が無いセッション (中断・進行中) は usage_checkpoint で埋める
    #[test]
    fn unfinished_session_falls_back_to_usage_checkpoint() {
        let fx = fixture("unfinished");
        let text = format!(
            "{}\n{}\n",
            r#"{"type":"session.start","id":"a","timestamp":"2026-09-07T17:00:00.000Z","data":{"context":{"cwd":"D:\\Projects\\Foo"}}}"#,
            r#"{"type":"session.usage_checkpoint","id":"b","timestamp":"2026-09-07T17:05:00.000Z","data":{"totalNanoAiu":123}}"#,
        );
        write_session(&fx.dir, "s1", &text);
        run(&fx.db, &fx.dir, NOW, |_| {});
        assert_eq!(count(&fx.db, "SELECT total_nano_aiu FROM sessions"), 123);
    }

    /// FR-C-21〜23 / ADR-0016: 親側の起動で行ができ、子側のメタがマージされる
    #[test]
    fn subagent_run_is_merged_under_the_tool_call_id() {
        let fx = fixture("subagent");
        let text = [
            r#"{"type":"tool.execution_start","id":"a","timestamp":"2026-09-07T17:00:00.000Z","data":{"toolCallId":"toolu_01","toolName":"task","model":"gpt-5","arguments":{"agent_type":"helper","description":"挨拶を作る","mode":"sync"}}}"#,
            r#"{"type":"subagent.started","id":"b","agentId":"toolu_01","timestamp":"2026-09-07T17:00:01.000Z","data":{"toolCallId":"toolu_01","agentName":"helper","agentDescription":"短い挨拶文の作成担当","model":"gpt-5-mini"}}"#,
            r#"{"type":"subagent.completed","id":"c","agentId":"toolu_01","timestamp":"2026-09-07T17:05:00.000Z","data":{"toolCallId":"toolu_01","agentName":"helper","totalToolCalls":12,"durationMs":291463}}"#,
        ]
        .join("\n")
            + "\n";
        write_session(&fx.dir, "s1", &text);
        run(&fx.db, &fx.dir, NOW, |_| {});

        let conn = fx.db.lock().unwrap();
        let (n, status, calls, agent_type, ended): (i64, String, i64, String, Option<i64>) = conn
            .query_row(
                "SELECT COUNT(*), status, tool_call_count, agent_type, ended_at FROM subagent_runs",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(calls, 12);
        assert_eq!(agent_type, "helper");
        // OQ-11 が埋まるまで完了へ遷移させない (T-4.9 / T-4.10 保留)
        assert_eq!(status, "running");
        assert_eq!(ended, None);

        let agent_count: i64 = conn
            .query_row("SELECT agent_count FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(agent_count, 1, "件数は集合を数え直した値 (FR-C-51)");
    }

    /// FR-C-28 / T-4.11: 抽出条件が未確定なので 0 件。書き込み経路だけ用意してある
    #[test]
    fn quota_events_stay_empty_until_the_record_kind_is_observed() {
        let fx = fixture("quota");
        write_session(&fx.dir, "s1", &sample_events());
        run(&fx.db, &fx.dir, NOW, |_| {});
        assert_eq!(count(&fx.db, "SELECT COUNT(*) FROM quota_events"), 0);
        assert_eq!(count(&fx.db, "SELECT quota_event_count FROM sessions"), 0);
    }

    /// FR-C-11: 進捗が scan → index → done の順で出る
    #[test]
    fn progress_starts_with_scan_and_ends_with_done() {
        let fx = fixture("progress");
        write_session(&fx.dir, "s1", &sample_events());

        let mut phases = Vec::new();
        let last = std::cell::RefCell::new(None);
        run(&fx.db, &fx.dir, NOW, |p| {
            phases.push(p.phase.clone());
            *last.borrow_mut() = Some(p.clone());
        });

        assert_eq!(phases.first().map(String::as_str), Some("scan"));
        assert_eq!(phases.last().map(String::as_str), Some("done"));
        let last = last.borrow().clone().unwrap();
        assert_eq!(last.total_files, 1);
        assert_eq!(last.done_files, 1);
        assert_eq!(last.records_ingested, 4);
        assert_eq!(last.current_file, None);
    }

    /// FR-C-04: バッチをまたいでも、コミット済みの位置から続きを読める
    #[test]
    fn offset_resumes_from_the_last_committed_batch() {
        let fx = fixture("batch");
        // BATCH_LINES を超える行数を書く
        let mut text = String::new();
        let total = BATCH_LINES + 500;
        for i in 0..total {
            text.push_str(&format!(
                r#"{{"type":"user.message","id":"r{i}","timestamp":"2026-09-07T17:00:00.000Z","data":{{"content":"{i}"}}}}"#
            ));
            text.push('\n');
        }
        write_session(&fx.dir, "s1", &text);

        let first = run(&fx.db, &fx.dir, NOW, |_| {});
        assert_eq!(first.records_ingested, total as u64);

        let second = run(&fx.db, &fx.dir, NOW + 1000, |_| {});
        assert_eq!(second.records_ingested, 0);

        // オフセットがファイル末尾に一致している (行境界からずれていない)
        let conn = fx.db.lock().unwrap();
        let (offset, size): (i64, i64) = conn
            .query_row(
                "SELECT last_parsed_offset, file_size_at_parse FROM index_files",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(offset, size);
        assert_eq!(offset as usize, text.len());
    }

    /// FR-C-02 / IR-15: 索引したオフセットで元の行をシーク読みできる
    #[test]
    fn indexed_offsets_round_trip_to_the_original_record() {
        let fx = fixture("roundtrip");
        write_session(&fx.dir, "s1", &sample_events());
        run(&fx.db, &fx.dir, NOW, |_| {});

        let conn = fx.db.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT file_path, byte_offset, byte_length, record_type FROM turn_index ORDER BY byte_offset")
            .unwrap();
        let rows: Vec<(String, u64, u64, String)> = stmt
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get::<_, i64>(1)? as u64,
                    r.get::<_, i64>(2)? as u64,
                    r.get(3)?,
                ))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert_eq!(rows.len(), 4);
        for (path, offset, len, record_type) in rows {
            use std::io::Read;
            let mut f = std::fs::File::open(&path).unwrap();
            f.seek(SeekFrom::Start(offset)).unwrap();
            let mut buf = vec![0u8; len as usize];
            f.read_exact(&mut buf).unwrap();
            let value: serde_json::Value = serde_json::from_slice(&buf).unwrap();
            assert_eq!(value.get("type").unwrap().as_str().unwrap(), record_type);
        }
    }

    #[test]
    fn missing_session_state_dir_is_not_an_error() {
        let fx = fixture("no-home");
        let stats = run(&fx.db, &fx.dir, NOW, |_| {});
        assert_eq!(stats, IndexStats::default());
    }
}
