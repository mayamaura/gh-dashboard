//! Copilot ダッシュボードの IPC コマンド (IR-10〜19)。
//!
//! **全コマンドに `rename_all = "snake_case"`** (IR-30)。
//!
//! `live_status_get` は 2 秒ごとに呼ばれる。**この関数から辿れる先に
//! ネットワークアクセスと外部プロセス起動を入れないこと** (INV-4 / NFR-03)。
//!
//! 実装状況: 署名と規約だけが確定した足場。中身は段階 4〜7 で実装する。

use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use rusqlite::OptionalExtension;
use tauri::{AppHandle, Emitter, State};

use crate::copilot::{
    indexer, store, tree, usage, AnimationPref, DbSnapshot, GanttWindow, LiveStatus, ModelUsage,
    ModelUsageSource, QuotaEventMark, SessionDetail, SessionQuery, SessionSummary, SubagentNode,
    TurnBody, TurnMeta, UsageToday, TIMELINE_MAX, TIMELINE_PAGE_SIZE, TURN_BODY_MAX_BYTES,
};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// IR-43 / IR-44。**この文字列を変えると `src/ipc/events.ts` の購読が外れる。**
const EVENT_INDEX_PROGRESS: &str = "index-progress";
const EVENT_SNAPSHOT: &str = "snapshot";

/// 進捗通知の間引き間隔 (FR-C-11)。**最終通知は間引かない。**
const PROGRESS_THROTTLE: std::time::Duration = std::time::Duration::from_millis(250);

/// `Mutex` の汚染をユーザー向けの `AppError` に変換する。
fn db_lock_err<T>(_: std::sync::PoisonError<T>) -> AppError {
    AppError::Db {
        message: "データベースのロック取得に失敗しました".to_string(),
    }
}

/// `spawn_blocking` 自体が failed (パニック等) した場合の変換。
fn spawn_err(e: tokio::task::JoinError) -> AppError {
    AppError::Io {
        message: e.to_string(),
    }
}

fn reindex_hint() -> Option<String> {
    Some("インデックスを再実行してください".to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------- IR-10

/// IR-10: ライブ状況。**2 秒ポーリング前提。ネットワークアクセスなし** (INV-4)。
///
/// 取得できるものが無い状態は正常系。空の一覧を返してよい。
///
/// 中身は `copilot::live::collect`。**DB を開かない** — 段階 4 の差分インデックスに
/// 依存しないので、索引が一度も走っていなくても稼働中セッションは出る (ADR-0014)。
/// ファイル IO を伴うので `spawn_blocking` に載せる (INV-10 / NFR-20)。
#[tauri::command(rename_all = "snake_case")]
pub async fn live_status_get(state: State<'_, AppState>) -> AppResult<LiveStatus> {
    let cache = state.live_cache.clone();
    let now = now_ms();
    tokio::task::spawn_blocking(move || {
        let mut cache = match cache.lock() {
            Ok(c) => c,
            // 汚染してもライブ表示は止めない。キャッシュを捨てて読み直す (NFR-24)
            Err(poisoned) => poisoned.into_inner(),
        };
        crate::copilot::live::collect(now, &mut cache)
    })
    .await
    .map_err(spawn_err)
}

// ---------------------------------------------------------------- IR-11..12

/// IR-11: DB 由来のダイジェスト。
#[tauri::command(rename_all = "snake_case")]
pub async fn snapshot_get(state: State<'_, AppState>) -> AppResult<DbSnapshot> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.lock().map_err(db_lock_err)?;
        store::snapshot(&conn)
    })
    .await
    .map_err(spawn_err)?
}

/// 実行中フラグを**必ず**戻すためのガード (FR-C-10)。
///
/// 途中の `return` でも panic でも `Drop` が走る。`end_indexing()` を手で呼ぶ形にすると、
/// 1 本でも早期 return を足した瞬間にフラグが立ちっぱなしになり、以後インデックスが
/// 二度と動かなくなる (再起動でしか直らない)。
struct IndexingGuard(Arc<AtomicBool>);

impl Drop for IndexingGuard {
    fn drop(&mut self) {
        // AppState::end_indexing と同じ操作
        self.0.store(false, Ordering::Release);
    }
}

/// IR-12: 差分インデックスをバックグラウンド起動する。**即座に返す**。
///
/// 実行中の再要求は**黙って無視する**。エラーにしない (FR-C-10)。
#[tauri::command(rename_all = "snake_case")]
pub async fn index_refresh(app: AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    if !state.try_begin_indexing() {
        // 多重起動防止。呼び出し側にとっては成功扱いでよい
        return Ok(());
    }

    let db = state.db.clone();
    let guard = IndexingGuard(state.indexing.clone());
    let now = now_ms();

    // **待たない。** 起動要求は即座に返す (FR-C-10)
    tokio::task::spawn_blocking(move || {
        let _guard = guard;

        let Some(home) = indexer::default_home() else {
            tracing::warn!("COPILOT_HOME もホームディレクトリも解決できません");
            return;
        };

        let mut last_emit: Option<Instant> = None;
        let stats = indexer::run(&db, &home, now, |progress| {
            // 最終通知 (done) は間引かない (FR-C-11)
            let is_final = progress.phase == "done";
            if is_final || last_emit.map_or(true, |t| t.elapsed() >= PROGRESS_THROTTLE) {
                last_emit = Some(Instant::now());
                let _ = app.emit(EVENT_INDEX_PROGRESS, progress);
            }
        });
        tracing::info!(
            files = stats.total_files,
            updated = stats.updated_files,
            records = stats.records_ingested,
            skipped = stats.skipped_lines,
            failed = stats.failed_files,
            "差分インデックス完了"
        );

        // IR-44: 完了スナップショット。読めなくても実行自体は成功しているので落とさない
        let snapshot = db.lock().ok().and_then(|conn| store::snapshot(&conn).ok());
        if let Some(snapshot) = snapshot {
            let _ = app.emit(EVENT_SNAPSHOT, &snapshot);
        }
    });

    Ok(())
}

// ---------------------------------------------------------------- IR-13..15

/// IR-13: セッション検索。既定 100 / 上限 1000 (FR-C-110)。
#[tauri::command(rename_all = "snake_case")]
pub async fn sessions_list_get(
    state: State<'_, AppState>,
    query: SessionQuery,
) -> AppResult<Vec<SessionSummary>> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.lock().map_err(db_lock_err)?;
        store::search_sessions(&conn, &query)
    })
    .await
    .map_err(spawn_err)?
}

/// IR-14: セッション詳細 (T-7.5〜7.10 / FR-C-112〜119)。
///
/// **未インデックスは `None` (正常系)。エラーにしない** (FR-C-57)。
/// 赤いエラーを出しても、ユーザーには何をすればよいか分からない。
///
/// `timeline_offset` は「もっと見る」用 (FR-C-119)。省略すると先頭 150 件。
///
/// 3 フェーズに分かれる。**DB ロックを持ったままファイルを読まない**
/// (差分インデックスを待たせるため / FR-C-09):
/// 1. DB フェーズ — 集計・木・タイムライン
/// 2. ファイルフェーズ — `session.shutdown` を 1 レコードだけシーク読み (INV-6)
/// 3. ライブフェーズ — 段階 5 の判定で稼働中サブエージェント集合を得る (FR-C-115)
#[tauri::command(rename_all = "snake_case")]
pub async fn session_detail_get(
    state: State<'_, AppState>,
    session_id: String,
    timeline_offset: Option<i64>,
) -> AppResult<Option<SessionDetail>> {
    let db = state.db.clone();
    let live_cache = state.live_cache.clone();
    let now = now_ms();
    let offset = timeline_offset.unwrap_or(0).max(0);

    tokio::task::spawn_blocking(move || -> AppResult<Option<SessionDetail>> {
        // ---- ① DB フェーズ ----
        let Some(raw) = ({
            let conn = db.lock().map_err(db_lock_err)?;
            read_session_detail(&conn, &session_id, offset)?
        }) else {
            // 索引にまだ無い。正常系として None (FR-C-57)
            return Ok(None);
        };

        // ---- ② ファイルフェーズ (ロックを手放してから) ----
        let models = model_breakdown(raw.shutdown_at, raw.turn_index_models);

        // ---- ③ ライブフェーズ ----
        let (is_live, live_ids) = live_subagents(&live_cache, &session_id, now);

        Ok(Some(assemble_detail(raw.base, models, is_live, live_ids)))
    })
    .await
    .map_err(spawn_err)?
}

/// `subagent_runs` 1 行 (DB の生の形)。
struct RunRow {
    run_key: String,
    agent_id: Option<String>,
    parent_agent_id: Option<String>,
    spawn_depth: Option<i32>,
    agent_type: Option<String>,
    description: Option<String>,
    model: Option<String>,
    status: String,
    started_at: Option<i64>,
    last_activity_at: Option<i64>,
    ended_at: Option<i64>,
    tool_call_count: i64,
}

/// DB フェーズで取れたもの一式。ライブ判定とファイル読みはまだ入っていない。
struct DetailBase {
    session: SessionSummary,
    runs: Vec<RunRow>,
    quota_events: Vec<QuotaEventMark>,
    timeline: Vec<TurnMeta>,
    timeline_offset: i64,
    timeline_total: i64,
}

struct DetailRaw {
    base: DetailBase,
    /// `session.shutdown` レコードの位置 (最新の 1 件)。`modelMetrics` の読み出し元
    shutdown_at: Option<(String, u64, u64)>,
    /// `modelMetrics` が取れなかったときのフォールバック (出力トークンと件数だけ)
    turn_index_models: Vec<ModelUsage>,
}

fn read_session_detail(
    conn: &rusqlite::Connection,
    session_id: &str,
    offset: i64,
) -> AppResult<Option<DetailRaw>> {
    let session = conn
        .query_row(
            "SELECT session_id, folder_name, title, cwd, entrypoint, started_at, last_activity_at, \
                    turn_count, total_nano_aiu, agent_count \
             FROM sessions WHERE session_id = ?1",
            [session_id],
            |r| {
                let client_name: Option<String> = r.get(4)?;
                Ok(SessionSummary {
                    session_id: r.get(0)?,
                    folder_name: r.get(1)?,
                    title: r.get(2)?,
                    cwd: r.get(3)?,
                    entrypoint: store::entrypoint_from_client_name(client_name.as_deref()),
                    started_at: r.get(5)?,
                    last_activity_at: r.get(6)?,
                    turn_count: r.get(7)?,
                    total_nano_aiu: r.get(8)?,
                    agent_count: r.get(9)?,
                })
            },
        )
        .optional()?;
    let Some(session) = session else {
        return Ok(None);
    };

    // ---- サブエージェント (T-7.6) ----
    let mut stmt = conn.prepare(
        "SELECT run_key, agent_id, parent_agent_id, spawn_depth, agent_type, description, model, \
                status, started_at, last_activity_at, ended_at, tool_call_count \
         FROM subagent_runs WHERE session_id = ?1",
    )?;
    let runs = stmt
        .query_map([session_id], |r| {
            Ok(RunRow {
                run_key: r.get(0)?,
                agent_id: r.get(1)?,
                parent_agent_id: r.get(2)?,
                spawn_depth: r.get(3)?,
                agent_type: r.get(4)?,
                description: r.get(5)?,
                model: r.get(6)?,
                status: r.get(7)?,
                started_at: r.get(8)?,
                last_activity_at: r.get(9)?,
                ended_at: r.get(10)?,
                tool_call_count: r.get(11)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    // ---- モデル別内訳の読み出し元 (T-7.3) ----
    // **最新の 1 件だけ。**複数の shutdown は累計値なので、足すと二重加算になる
    // (実データ 4 セッションで 2 件ずつ観測、後のものが前のものを含む / ADR-0029)
    let shutdown_at = conn
        .query_row(
            "SELECT file_path, byte_offset, byte_length FROM turn_index \
             WHERE session_id = ?1 AND record_type = 'session.shutdown' \
             ORDER BY COALESCE(timestamp_ms, 0) DESC, byte_offset DESC LIMIT 1",
            [session_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?.max(0) as u64,
                    r.get::<_, i64>(2)?.max(0) as u64,
                ))
            },
        )
        .optional()?;

    let mut stmt = conn.prepare(
        "SELECT model, SUM(output_tokens), COUNT(*) FROM turn_index \
         WHERE session_id = ?1 AND model IS NOT NULL AND model <> '' \
         GROUP BY model ORDER BY 2 DESC, 1 ASC",
    )?;
    let turn_index_models = stmt
        .query_map([session_id], |r| {
            Ok(ModelUsage {
                model: r.get(0)?,
                // レコード単位に載るのは出力トークンだけ。**残りは 0 ではなく None**
                input_tokens: None,
                output_tokens: Some(r.get(1)?),
                cache_read_tokens: None,
                cache_write_tokens: None,
                nano_aiu: None,
                credits: None,
                record_count: Some(r.get(2)?),
                source: ModelUsageSource::TurnIndex,
            })
        })?
        .filter_map(|row| row.ok())
        .filter(|m| !usage::is_synthetic_model(&m.model))
        .collect::<Vec<_>>();
    drop(stmt);

    // ---- 利用枠到達マーカー (T-7.7 / FR-C-118) ----
    let mut stmt = conn.prepare(
        "SELECT occurred_at, kind, reset_text FROM quota_events \
         WHERE session_id = ?1 ORDER BY occurred_at ASC",
    )?;
    let quota_events = stmt
        .query_map([session_id], |r| {
            Ok(QuotaEventMark {
                occurred_at: r.get(0)?,
                kind: r.get(1)?,
                reset_text: r.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    // ---- 本文タイムライン (T-7.10 / FR-C-119) ----
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM turn_index WHERE session_id = ?1",
        [session_id],
        |r| r.get(0),
    )?;
    let timeline_total = total.min(TIMELINE_MAX);
    let limit = TIMELINE_PAGE_SIZE.min((timeline_total - offset).max(0));

    let mut stmt = conn.prepare(
        "SELECT id, timestamp_ms, record_type, role, model, agent_id, is_sidechain, preview, \
                output_tokens, byte_length \
         FROM turn_index WHERE session_id = ?1 \
         ORDER BY COALESCE(timestamp_ms, 0) ASC, byte_offset ASC \
         LIMIT ?2 OFFSET ?3",
    )?;
    let timeline = stmt
        .query_map(rusqlite::params![session_id, limit, offset], |r| {
            Ok(TurnMeta {
                turn_id: r.get(0)?,
                timestamp_ms: r.get(1)?,
                record_type: r.get(2)?,
                role: r.get(3)?,
                model: r.get(4)?,
                agent_id: r.get(5)?,
                is_sidechain: r.get::<_, i64>(6)? != 0,
                preview: r.get(7)?,
                output_tokens: r.get(8)?,
                byte_length: r.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    Ok(Some(DetailRaw {
        base: DetailBase {
            session,
            runs,
            quota_events,
            timeline,
            timeline_offset: offset,
            timeline_total,
        },
        shutdown_at,
        turn_index_models,
    }))
}

/// モデル別内訳 (FR-C-105)。**`session.shutdown` を 1 レコードだけシーク読みする。**
///
/// `modelMetrics` を DB に複製しない (INV-6)。読めなければ `turn_index` 由来の
/// 部分的な内訳に降格する — **出所は `ModelUsage::source` に必ず載る** (INV-7)。
fn model_breakdown(
    shutdown_at: Option<(String, u64, u64)>,
    fallback: Vec<ModelUsage>,
) -> Vec<ModelUsage> {
    let from_shutdown = shutdown_at
        .and_then(|(path, offset, len)| read_record_at(&path, offset, len).ok())
        .map(|line| crate::copilot::parser::parse_model_metrics(&line))
        .unwrap_or_default();

    if from_shutdown.is_empty() {
        // shutdown が無い (進行中 / 中断) か modelMetrics が空 (実データ 5/50 件)
        fallback
    } else {
        from_shutdown
    }
}

/// 段階 5 の判定でこのセッションの稼働状況を得る (FR-C-115 / ADR-0014)。
///
/// **`live::collect` をそのまま使う。**稼働判定を書き直さない — 2 つ持つと
/// 「バッジは稼働中なのに詳細は非稼働」が出る。2 秒ポーリングと同じキャッシュを
/// 共有するので、実際にはディスクをほぼ読まない (FR-C-48)。
fn live_subagents(
    cache: &std::sync::Mutex<crate::copilot::live::LiveCache>,
    session_id: &str,
    now: i64,
) -> (bool, std::collections::HashSet<String>) {
    let mut cache = cache.lock().unwrap_or_else(|p| p.into_inner());
    let status = crate::copilot::live::collect(now, &mut cache);
    match status.sessions.into_iter().find(|s| s.session_id == session_id) {
        // **集合が正。**件数は使わない (FR-C-51)
        Some(s) => (true, s.running_subagent_ids.into_iter().collect()),
        None => (false, std::collections::HashSet::new()),
    }
}

/// 木構築 + 可視判定 + ガント窓 (T-7.6〜7.8)。**純粋** (IO を持ち込まない)。
fn assemble_detail(
    base: DetailBase,
    models: Vec<ModelUsage>,
    is_live: bool,
    live_ids: std::collections::HashSet<String>,
) -> SessionDetail {
    use std::collections::HashMap;

    let by_key: HashMap<&str, &RunRow> = base
        .runs
        .iter()
        .map(|r| (r.run_key.as_str(), r))
        .collect();

    let mut inputs: Vec<tree::RunNodeInput> = base
        .runs
        .iter()
        .map(|r| tree::RunNodeInput {
            run_key: r.run_key.clone(),
            parent_key: r.parent_agent_id.clone(),
            started_at: r.started_at,
            reported_depth: r.spawn_depth,
        })
        .collect();

    // ライブ集合にあって索引に無い実行を補う。
    // **これが無いと「バッジは稼働中なのに系統図は空」が出る** (FR-C-51 の趣旨)。
    // 差分インデックスは 10 秒周期なので、走り始めた直後は必ずこの状態になる
    for id in &live_ids {
        if !by_key.contains_key(id.as_str()) {
            inputs.push(tree::RunNodeInput {
                run_key: id.clone(),
                parent_key: None,
                started_at: None,
                reported_depth: None,
            });
        }
    }

    // **系統図とガントはこの 1 回の木構築結果を共有する** (FR-C-114)
    let nodes = tree::build(&inputs);

    // FR-C-116 / 117: 稼働中なら折りたたみ、振り返り (非稼働) なら全件表示
    let visible: std::collections::HashSet<String> = if is_live {
        tree::visible_for_live(&nodes, &live_ids).into_iter().collect()
    } else {
        nodes.iter().map(|n| n.run_key.clone()).collect()
    };

    let subagents: Vec<SubagentNode> = nodes
        .into_iter()
        .map(|n| {
            let row = by_key.get(n.run_key.as_str());
            SubagentNode {
                agent_id: row.and_then(|r| r.agent_id.clone()),
                agent_type: row.and_then(|r| r.agent_type.clone()),
                description: row.and_then(|r| r.description.clone()),
                model: row.and_then(|r| r.model.clone()),
                // FR-C-115: ライブ集合が優先。集合に無い行は DB の状態列に
                // フォールバックする設計だが、**`status` は現状どの行も
                // `'running'` のまま** (完了/拒否への遷移は OQ-11 待ち /
                // T-4.9・T-4.10 保留)。ここでフォールバックさせると全行が
                // 稼働中になるので、集合に無い行は非稼働として扱う。
                // 生の状態列はそのまま返すので、遷移が入れば UI 側で使える
                status: row.map(|r| r.status.clone()).unwrap_or_else(|| "running".into()),
                started_at: row.and_then(|r| r.started_at),
                last_activity_at: row.and_then(|r| r.last_activity_at),
                // 稼働中は None。フロントが現在時刻で描く (FR-C-118)
                ended_at: row.and_then(|r| r.ended_at),
                tool_call_count: row.map(|r| r.tool_call_count).unwrap_or(0),
                running: live_ids.contains(&n.run_key),
                visible: visible.contains(&n.run_key),
                run_key: n.run_key,
                parent_key: n.parent_key,
                depth: n.depth,
                orphaned: n.orphaned,
                child_keys: n.child_keys,
            }
        })
        .collect();

    let next = base.timeline_offset + base.timeline.len() as i64;
    SessionDetail {
        credits: crate::copilot::quota::credits_from_nano_aiu(base.session.total_nano_aiu),
        gantt: GanttWindow {
            start_at: base.session.started_at,
            // **稼働中は終了時刻を確定させない** (FR-C-118 / FR-C-24 と同じ理屈)
            end_at: if is_live {
                None
            } else {
                base.session.last_activity_at
            },
        },
        session: base.session,
        models,
        subagents,
        is_live,
        quota_events: base.quota_events,
        timeline: base.timeline,
        timeline_offset: base.timeline_offset,
        timeline_total: base.timeline_total,
        timeline_next_offset: (next < base.timeline_total).then_some(next),
    }
}

/// 1 レコードを位置指定でシーク読みする (FR-C-02 / 119 / INV-6)。
///
/// 上限 512KB (FR-C-120)。`(本文, 切り詰めたか)` を返す。
fn read_record_at(file_path: &str, byte_offset: u64, byte_length: u64) -> AppResult<String> {
    let mut file = std::fs::File::open(file_path)?;
    let file_size = file.metadata()?.len();
    if byte_offset >= file_size {
        return Err(AppError::unavailable(
            "本文の位置がファイルの範囲外です".to_string(),
            reindex_hint(),
        ));
    }
    file.seek(SeekFrom::Start(byte_offset))?;
    let capped = byte_length
        .min(TURN_BODY_MAX_BYTES)
        .min(file_size - byte_offset);
    let mut buf = vec![0u8; capped as usize];
    file.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// IR-15: 本文を 1 レコードだけシーク読みする。上限 512KB (FR-C-119 / 120)。
///
/// **本文は DB に複製しない** (FR-C-02 / INV-6)。都度ファイルをシーク読みする。
#[tauri::command(rename_all = "snake_case")]
pub async fn turn_body_get(state: State<'_, AppState>, turn_id: i64) -> AppResult<TurnBody> {
    turn_body_get_impl(state.db.clone(), turn_id).await
}

async fn turn_body_get_impl(
    db: std::sync::Arc<std::sync::Mutex<rusqlite::Connection>>,
    turn_id: i64,
) -> AppResult<TurnBody> {
    let (file_path, byte_offset, byte_length): (String, u64, u64) =
        tokio::task::spawn_blocking(move || -> AppResult<_> {
            let conn = db.lock().map_err(db_lock_err)?;
            conn.query_row(
                "SELECT file_path, byte_offset, byte_length FROM turn_index WHERE id = ?1",
                [turn_id],
                |row| {
                    let offset: i64 = row.get(1)?;
                    let length: i64 = row.get(2)?;
                    Ok((row.get::<_, String>(0)?, offset as u64, length as u64))
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    AppError::not_found(format!("turn_id={turn_id}"))
                }
                other => AppError::from(other),
            })
        })
        .await
        .map_err(spawn_err)??;

    let truncated = byte_length > TURN_BODY_MAX_BYTES;

    tokio::task::spawn_blocking(move || -> AppResult<TurnBody> {
        Ok(TurnBody {
            turn_id,
            body: read_record_at(&file_path, byte_offset, byte_length)?,
            truncated,
        })
    })
    .await
    .map_err(spawn_err)?
}

// ---------------------------------------------------------------- IR-16

/// IR-16: 本日の使用状況。
///
/// **2 秒ポーリングの対象に含めない** (FR-C-103)。タブ表示時と
/// 差分インデックス完了時のみ更新する。
#[tauri::command(rename_all = "snake_case")]
pub async fn usage_today_get(state: State<'_, AppState>) -> AppResult<UsageToday> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.lock().map_err(db_lock_err)?;
        usage_today(&conn)
    })
    .await
    .map_err(spawn_err)?
}

/// ローカル日 0:00 の epoch ミリ秒 (FR-C-100)。
///
/// **SQLite に計算させる。**タイムゾーン用の依存を足さないため (NFR-10)。
/// `date('now','localtime')` でローカル日付にし、`'utc'` 修飾子で
/// 「その日付のローカル 0:00」を UTC に戻す。`strftime('%s', date(...))` だけだと
/// **UTC の 0:00** になり、東京なら 9 時間ずれる。
fn local_day_start_ms(conn: &rusqlite::Connection) -> AppResult<i64> {
    let seconds: i64 = conn.query_row(
        "SELECT CAST(strftime('%s', date('now','localtime'), 'utc') AS INTEGER)",
        [],
        |r| r.get(0),
    )?;
    Ok(seconds * 1000)
}

/// IR-16 の本体 (FR-C-100〜104)。同期関数。呼び出し側が `spawn_blocking` で包む。
///
/// # 二重加算をしない根拠 (FR-C-104)
///
/// - `turn_index` は 1 レコード 1 行。`parse_record` がレコード単位に入れる
///   トークンは `assistant.message.outputTokens` **だけ**で、累計値
///   (`totalNanoAiu` / `tokenDetails`) はレコード側に持たせていない (段階 4)
/// - セッション側の合計は `sessions` を **`IN (サブクエリ)` で絞るだけ**にして
///   JOIN しない。`turn_index` と JOIN すると 1 セッションがレコード数ぶん
///   複製され、合計がレコード件数倍になる
fn usage_today(conn: &rusqlite::Connection) -> AppResult<UsageToday> {
    usage_since(conn, local_day_start_ms(conn)?)
}

/// 起点を指定した集計。**テストと実データ検証のために切り出してある** —
/// 「今日」のデータが無い環境でも、過去の 1 日を入れて同じ経路を通せる。
fn usage_since(conn: &rusqlite::Connection, day_start: i64) -> AppResult<UsageToday> {
    // ---- 時間帯別 (FR-C-101) + 本日のレコード件数 + 合成モデルの除外 ----
    let mut stmt = conn.prepare(
        "SELECT timestamp_ms, model, input_tokens, output_tokens FROM turn_index \
         WHERE timestamp_ms >= ?1",
    )?;
    let rows = stmt
        .query_map([day_start], |r| {
            Ok(usage::TurnTokenRow {
                timestamp_ms: r.get(0)?,
                model: r.get(1)?,
                input_tokens: r.get(2)?,
                output_tokens: r.get(3)?,
            })
        })?
        .filter_map(|r| r.ok());
    let agg = usage::fold_hourly(rows, day_start);
    drop(stmt);

    // ---- セッション単位の合計 (FR-C-100) ----
    // 本日 1 レコードでも書いたセッションを母集団にする。
    // **日をまたぐセッションは分割できない** — `tokenDetails` はセッションに
    // 1 組しか無いため。UsageToday の doc コメントに明記してある
    const TODAY_SESSIONS: &str =
        "SELECT DISTINCT session_id FROM turn_index WHERE timestamp_ms >= ?1 AND session_id IS NOT NULL";

    let (input_tokens, output_tokens, cache_read_tokens, total_nano_aiu, session_count) = conn
        .query_row(
            &format!(
                "SELECT COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), \
                        COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(total_nano_aiu), 0), \
                        COUNT(*) \
                 FROM sessions WHERE session_id IN ({TODAY_SESSIONS})"
            ),
            [day_start],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;

    // 本日動いたサブエージェント実行。started_at が欠けた行を落とさない (NFR-23)
    let subagent_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM subagent_runs \
         WHERE COALESCE(started_at, last_activity_at, 0) >= ?1",
        [day_start],
        |r| r.get(0),
    )?;

    // ---- フォルダ別の上位 5 件 (FR-C-102) ----
    let mut stmt = conn.prepare(&format!(
        "SELECT folder_name, COALESCE(SUM(input_tokens + output_tokens), 0) AS t \
         FROM sessions \
         WHERE session_id IN ({TODAY_SESSIONS}) AND folder_name IS NOT NULL AND folder_name <> '' \
         GROUP BY folder_name ORDER BY t DESC, folder_name ASC LIMIT 5"
    ))?;
    let top_folders = stmt
        .query_map([day_start], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<(String, i64)>, _>>()?;
    drop(stmt);

    Ok(UsageToday {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        turn_count: agg.turn_count,
        session_count,
        subagent_count,
        total_nano_aiu,
        hourly_tokens: agg.hourly_tokens,
        top_folders,
        excluded_records: agg.excluded_records,
    })
}

// ---------------------------------------------------------------- IR-17..18

/// IR-17: 利用枠。**経路 A→(B は未実装)→C の降格込み** (ADR-0018)。
///
/// ネットワークを伴うため下限間隔 (5 分) がある。**2 秒ポーリングに載せない** (FR-C-135)。
/// 取れなくても機能全体を止めない (FR-C-136)。
#[tauri::command(rename_all = "snake_case")]
pub async fn quota_get(
    state: State<'_, AppState>,
    force: bool,
) -> AppResult<Vec<crate::copilot::quota::QuotaGauge>> {
    use crate::copilot::quota_fetch as qf;

    let cache = state.quota.clone();
    let db = state.db.clone();
    // 推定経路とサンプル書き込みは別々の spawn_blocking に載る。`db` は推定の
    // 分岐でしか move されないので、サンプル書き込み用にもう 1 本複製しておく
    let db_for_samples = state.db.clone();
    let now = now_ms();

    // 下限間隔 (FR-C-135 / 142)。直前の値をそのまま返す — 空にしない (NFR-07)
    {
        let c = lock_or_recover(&cache);
        if !force && !c.gauges.is_empty() {
            if let Some(last) = c.last_fetch_at {
                if now.saturating_sub(last) < qf::MIN_FETCH_INTERVAL_MS {
                    return Ok(c.gauges.clone());
                }
            }
        }
    }

    // 経路 A。**ここだけがネットワークに出る** (INV-3)。ロックは持たない
    let sdk = qf::fetch_sdk().await;

    // 経路 C。**経路 A が成功したら走らせない** — 実値がある枠に推定は混ざらない
    // (FR-C-144) ので、全表走査するだけ無駄になる。DB 走査は blocking に載せる (INV-10)
    let estimate = if sdk.is_ok() {
        None
    } else {
        let estimate_cache = cache.clone();
        tokio::task::spawn_blocking(move || {
            let conn = db.lock().ok()?;
            let mut c = lock_or_recover(&estimate_cache);
            qf::estimate(&conn, &mut c, now)
        })
        .await
        .map_err(spawn_err)?
    };

    let mut c = lock_or_recover(&cache);
    let gauges = qf::build_gauges(sdk, estimate, &mut c, now);
    c.gauges = gauges.clone();
    c.last_fetch_at = Some(now);
    drop(c);

    // FR-C-94 / DR-07: 時系列サンプルは実値 (`Actual`) の枠だけ記録する
    // (理由は store::QuotaSampleRow のコメントを参照)。書き込みは spawn_blocking へ (INV-10)。
    // 間引きは起動時に AppState::init 側でまとめて行う (定期実行ループを新設しない)。
    type OwnedSample = (i64, i64, &'static str, String, Option<f64>, Option<f64>, Option<f64>, Option<i64>);
    let samples: Vec<OwnedSample> = gauges
        .iter()
        .filter_map(|g| match &g.origin {
            crate::copilot::quota::QuotaSource::Actual { via, observed_at } => Some((
                now,
                *observed_at,
                actual_via_str(*via),
                g.kind.clone(),
                g.used,
                g.entitlement,
                g.used_pct,
                g.reset_at,
            )),
            _ => None,
        })
        .collect();
    if !samples.is_empty() {
        tokio::task::spawn_blocking(move || {
            let Ok(conn) = db_for_samples.lock() else {
                // 汚染していても利用枠の表示自体は止めない (NFR-24)
                return;
            };
            for (received_at, observed_at, source, quota_kind, used, entitlement, used_pct, reset_at) in &samples {
                let row = store::QuotaSampleRow {
                    received_at: *received_at,
                    observed_at: *observed_at,
                    source,
                    quota_kind,
                    used: *used,
                    entitlement: *entitlement,
                    used_pct: *used_pct,
                    reset_at: *reset_at,
                };
                if let Err(e) = store::insert_quota_sample(&conn, &row) {
                    tracing::warn!(error = %e, kind = %quota_kind, "quota_samples への書き込みに失敗");
                }
            }
        });
    }

    Ok(gauges)
}

/// IR-18: 各取得経路の可用性。「何をすれば取れるようになるか」の材料 (FR-C-83)。
///
/// **この関数は新しく取得しに行かない。** 直近の `quota_get` の結果を読むだけ
/// (INV-4 の趣旨: 表示のための関数からネットワークを起こさない)。
#[tauri::command(rename_all = "snake_case")]
pub async fn quota_source_status_get(state: State<'_, AppState>) -> AppResult<serde_json::Value> {
    let c = lock_or_recover(&state.quota);
    Ok(serde_json::json!({
        "sdk": c.sdk.to_json(),
        "rest": crate::copilot::quota_fetch::rest_status().to_json(),
        "estimate": c.estimate_route.to_json(),
        "checked_at": c.last_fetch_at,
    }))
}

/// `quota_samples.source` に書く文字列 (T-X.3)。
fn actual_via_str(via: crate::copilot::quota::ActualVia) -> &'static str {
    match via {
        crate::copilot::quota::ActualVia::Sdk => "sdk",
        crate::copilot::quota::ActualVia::Rest => "rest",
    }
}

/// 利用枠キャッシュのロック。**汚染しても表示を止めない** (NFR-24)。
/// 中身は導出データなので、壊れていても次の取得で上書きされる
fn lock_or_recover(
    cache: &std::sync::Mutex<crate::copilot::quota_fetch::QuotaCache>,
) -> std::sync::MutexGuard<'_, crate::copilot::quota_fetch::QuotaCache> {
    cache.lock().unwrap_or_else(|p| p.into_inner())
}

// ---------------------------------------------------------------- IR-19

/// `settings.value` (TEXT) ⇔ `AnimationPref` の変換。`serde` の snake_case 表現をそのまま使う。
fn animation_pref_to_str(pref: AnimationPref) -> &'static str {
    match pref {
        AnimationPref::Auto => "auto",
        AnimationPref::On => "on",
        AnimationPref::Off => "off",
    }
}

fn animation_pref_from_str(s: &str) -> Option<AnimationPref> {
    match s {
        "auto" => Some(AnimationPref::Auto),
        "on" => Some(AnimationPref::On),
        "off" => Some(AnimationPref::Off),
        _ => None,
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn animation_pref_get(state: State<'_, AppState>) -> AppResult<AnimationPref> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.lock().map_err(db_lock_err)?;
        let stored = store::animation_pref_get(&conn)?;
        Ok(stored
            .as_deref()
            .and_then(animation_pref_from_str)
            .unwrap_or(AnimationPref::Auto))
    })
    .await
    .map_err(spawn_err)?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn animation_pref_set(
    state: State<'_, AppState>,
    pref: AnimationPref,
) -> AppResult<AnimationPref> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.lock().map_err(db_lock_err)?;
        store::animation_pref_set(&conn, animation_pref_to_str(pref))?;
        Ok(pref)
    })
    .await
    .map_err(spawn_err)?
}

#[cfg(test)]
mod usage_today_tests {
    use super::*;
    use rusqlite::params;

    /// `turn_index` に 1 行 + 対応する `sessions` 行を入れる。
    fn insert_turn(conn: &rusqlite::Connection, session: &str, ts: i64, model: Option<&str>, out: i64) {
        conn.execute(
            "INSERT INTO turn_index (session_id, file_path, byte_offset, byte_length, timestamp_ms, model, output_tokens) \
             VALUES (?1, 'f', (SELECT COALESCE(MAX(byte_offset), -1) + 1 FROM turn_index), 10, ?2, ?3, ?4)",
            params![session, ts, model, out],
        )
        .unwrap();
    }

    fn insert_session(
        conn: &rusqlite::Connection,
        id: &str,
        folder: &str,
        input: i64,
        output: i64,
        cache_read: i64,
        nano: i64,
    ) {
        conn.execute(
            "INSERT INTO sessions (session_id, folder_name, input_tokens, output_tokens, cache_read_tokens, total_nano_aiu, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
            params![id, folder, input, output, cache_read, nano],
        )
        .unwrap();
    }

    /// SQLite に計算させたローカル日 0:00 が、実際に「今日」を指していること。
    /// **UTC 0:00 を返すと東京で 9 時間ずれる** (FR-C-100)
    #[test]
    fn local_day_start_is_within_the_last_24_hours() {
        let conn = crate::db::open_in_memory().unwrap();
        let start = local_day_start_ms(&conn).unwrap();
        let now = now_ms();
        assert!(start <= now, "日の始まりが未来にならない");
        assert!(
            now - start < 24 * 3_600_000,
            "24 時間以上前を指すなら UTC/ローカルの取り違え"
        );
        // 0:00 ちょうど (秒未満の端数が無い)
        assert_eq!(start % 60_000, 0);
    }

    #[test]
    fn empty_db_reports_zeros_with_24_buckets() {
        let conn = crate::db::open_in_memory().unwrap();
        let u = usage_today(&conn).unwrap();
        assert_eq!(u.turn_count, 0);
        assert_eq!(u.session_count, 0);
        assert_eq!(u.hourly_tokens.len(), 24);
        assert!(u.top_folders.is_empty());
    }

    /// FR-C-100 / 104: セッション集計が **1 回だけ**足される。
    ///
    /// `turn_index` と JOIN していたら、レコード件数 (3) 倍になる。
    #[test]
    fn session_totals_are_not_multiplied_by_the_record_count() {
        let conn = crate::db::open_in_memory().unwrap();
        let day = local_day_start_ms(&conn).unwrap();
        insert_session(&conn, "s1", "Foo", 1000, 200, 50, 382_635_000);
        for i in 0..3 {
            insert_turn(&conn, "s1", day + 1000 + i, Some("gpt-5-mini"), 7);
        }

        let u = usage_today(&conn).unwrap();
        assert_eq!(u.input_tokens, 1000, "3 倍になっていたら JOIN で複製している");
        assert_eq!(u.output_tokens, 200);
        assert_eq!(u.cache_read_tokens, 50);
        assert_eq!(u.total_nano_aiu, 382_635_000);
        assert_eq!(u.session_count, 1);
        assert_eq!(u.turn_count, 3);
        assert_eq!(u.hourly_tokens[0], 21, "時間帯別はレコード単位");
        assert_eq!(u.top_folders, vec![("Foo".to_string(), 1200)]);
    }

    /// ローカル日より前のレコードとセッションは入らない
    #[test]
    fn yesterdays_records_and_sessions_are_excluded() {
        let conn = crate::db::open_in_memory().unwrap();
        let day = local_day_start_ms(&conn).unwrap();
        insert_session(&conn, "old", "Old", 999, 999, 999, 999);
        insert_session(&conn, "new", "New", 1, 2, 3, 4);
        insert_turn(&conn, "old", day - 3_600_000, Some("gpt-5-mini"), 500);
        insert_turn(&conn, "new", day + 3_600_000, Some("gpt-5-mini"), 5);

        let u = usage_today(&conn).unwrap();
        assert_eq!(u.session_count, 1);
        assert_eq!(u.input_tokens, 1);
        assert_eq!(u.turn_count, 1);
        assert_eq!(u.hourly_tokens[1], 5);
        assert_eq!(u.hourly_tokens[0], 0);
    }

    /// FR-C-104: 合成モデル (`auto`) のレコードを除外し、件数として出す
    #[test]
    fn synthetic_model_records_are_excluded_and_reported() {
        let conn = crate::db::open_in_memory().unwrap();
        let day = local_day_start_ms(&conn).unwrap();
        insert_session(&conn, "s1", "Foo", 0, 0, 0, 0);
        insert_turn(&conn, "s1", day + 1000, Some("auto"), 999);
        insert_turn(&conn, "s1", day + 2000, Some("gpt-5-mini"), 3);

        let u = usage_today(&conn).unwrap();
        assert_eq!(u.hourly_tokens[0], 3);
        assert_eq!(u.turn_count, 1);
        assert_eq!(u.excluded_records, 1, "無言で落とさない (NFR-43)");
    }

    #[test]
    fn subagent_runs_started_today_are_counted() {
        let conn = crate::db::open_in_memory().unwrap();
        let day = local_day_start_ms(&conn).unwrap();
        for (key, started) in [("a", day + 1), ("b", day - 1)] {
            conn.execute(
                "INSERT INTO subagent_runs (run_key, session_id, started_at, updated_at) VALUES (?1, 's1', ?2, 0)",
                params![key, started],
            )
            .unwrap();
        }
        assert_eq!(usage_today(&conn).unwrap().subagent_count, 1);
    }
}

#[cfg(test)]
mod session_detail_tests {
    use super::*;
    use rusqlite::params;
    use std::collections::HashSet;

    const NOW: i64 = 1_800_000_000_000;

    fn db() -> rusqlite::Connection {
        let conn = crate::db::open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO sessions (session_id, folder_name, title, cwd, entrypoint, started_at, last_activity_at, \
                                   turn_count, total_nano_aiu, agent_count, updated_at) \
             VALUES ('s1', 'Foo', 'バグを直す', 'D:\\Foo', 'github/cli', ?1, ?2, 0, 382635000, 0, 0)",
            params![NOW, NOW + 60_000],
        )
        .unwrap();
        conn
    }

    fn add_run(conn: &rusqlite::Connection, key: &str, parent: Option<&str>, started: i64) {
        conn.execute(
            "INSERT INTO subagent_runs (run_key, agent_id, session_id, parent_agent_id, agent_type, \
                                        model, started_at, last_activity_at, status, tool_call_count, updated_at) \
             VALUES (?1, ?1, 's1', ?2, 'helper', 'gpt-5-mini', ?3, ?3, 'running', 3, 0)",
            params![key, parent, started],
        )
        .unwrap();
    }

    fn add_turn(conn: &rusqlite::Connection, offset: i64, ts: i64, kind: &str, model: Option<&str>) {
        conn.execute(
            "INSERT INTO turn_index (session_id, file_path, byte_offset, byte_length, record_type, \
                                     model, timestamp_ms, output_tokens, preview) \
             VALUES ('s1', 'f', ?1, 10, ?2, ?3, ?4, 5, 'プレビュー')",
            params![offset, kind, model, ts],
        )
        .unwrap();
    }

    fn detail(conn: &rusqlite::Connection, is_live: bool, running: &[&str]) -> SessionDetail {
        let raw = read_session_detail(conn, "s1", 0).unwrap().unwrap();
        let ids: HashSet<String> = running.iter().map(|s| s.to_string()).collect();
        assemble_detail(raw.base, raw.turn_index_models, is_live, ids)
    }

    /// FR-C-57: 索引に無いセッションは `None` (エラーにしない)
    #[test]
    fn unknown_session_is_none_not_an_error() {
        let conn = crate::db::open_in_memory().unwrap();
        assert!(read_session_detail(&conn, "missing", 0).unwrap().is_none());
    }

    /// FR-C-117: 振り返り (非稼働) は全件表示。折りたたまない
    #[test]
    fn past_session_shows_every_node() {
        let conn = db();
        add_run(&conn, "a", None, NOW);
        add_run(&conn, "b", Some("a"), NOW + 10);
        add_run(&conn, "done", Some("a"), NOW + 20);

        let d = detail(&conn, false, &[]);
        assert_eq!(d.subagents.len(), 3);
        assert!(d.subagents.iter().all(|n| n.visible), "全件表示 (FR-C-117)");
        assert!(d.subagents.iter().all(|n| !n.running));
        // ガントの終端は最終活動時刻 (稼働していないので確定している)
        assert_eq!(d.gantt.end_at, Some(NOW + 60_000));
        assert_eq!(d.gantt.start_at, Some(NOW));
        assert!(!d.is_live);
    }

    /// FR-C-115 / 116: 稼働中は祖先だけ残して畳む。DB の status 列では判定しない
    #[test]
    fn live_session_collapses_finished_siblings_but_keeps_ancestors() {
        let conn = db();
        add_run(&conn, "a", None, NOW);
        add_run(&conn, "b", Some("a"), NOW + 10);
        add_run(&conn, "done", Some("a"), NOW + 20);

        let d = detail(&conn, true, &["b"]);
        let visible: Vec<&str> = d
            .subagents
            .iter()
            .filter(|n| n.visible)
            .map(|n| n.run_key.as_str())
            .collect();
        assert_eq!(visible, vec!["a", "b"], "祖先保護 + 完了済みを畳む");
        // DB の status は全行 'running' のまま。ライブ集合が優先される (FR-C-115)
        assert!(d.subagents.iter().all(|n| n.status == "running"));
        let running: Vec<&str> = d
            .subagents
            .iter()
            .filter(|n| n.running)
            .map(|n| n.run_key.as_str())
            .collect();
        assert_eq!(running, vec!["b"]);
        // 稼働中なので終端を確定させない (FR-C-118)
        assert_eq!(d.gantt.end_at, None);
    }

    /// 「バッジは稼働中なのに系統図は空」を作らない (FR-C-51 の趣旨)。
    ///
    /// 差分インデックスが追いつく前は、ライブ集合にあって DB に無い実行がある。
    #[test]
    fn running_subagent_missing_from_the_index_still_gets_a_row() {
        let conn = db();
        let d = detail(&conn, true, &["not_indexed_yet"]);
        assert_eq!(d.subagents.len(), 1);
        assert_eq!(d.subagents[0].run_key, "not_indexed_yet");
        assert!(d.subagents[0].running);
        assert!(d.subagents[0].visible);
        // メタは索引待ち。0 や空文字で埋めない (NFR-43)
        assert_eq!(d.subagents[0].agent_type, None);
    }

    /// FR-C-114: 系統図とガントが同じ 1 本の木構築結果を共有する
    #[test]
    fn tree_order_and_depth_come_from_the_parent_links() {
        let conn = db();
        add_run(&conn, "a", None, NOW);
        add_run(&conn, "c", Some("a"), NOW + 200);
        add_run(&conn, "b", Some("a"), NOW + 100);

        let d = detail(&conn, false, &[]);
        let keys: Vec<&str> = d.subagents.iter().map(|n| n.run_key.as_str()).collect();
        assert_eq!(keys, vec!["a", "b", "c"], "兄弟は開始時刻順");
        assert_eq!(d.subagents[1].depth, 1);
        assert_eq!(d.subagents[0].child_keys.len(), 2);
    }

    /// FR-C-119: 初期 150 件 + 次ページのオフセット。総件数は 1000 で頭打ち
    #[test]
    fn timeline_pages_by_150_and_reports_the_next_offset() {
        let conn = db();
        for i in 0..200 {
            add_turn(&conn, i, NOW + i, "user.message", None);
        }
        let raw = read_session_detail(&conn, "s1", 0).unwrap().unwrap();
        let d = assemble_detail(raw.base, vec![], false, Default::default());
        assert_eq!(d.timeline.len(), TIMELINE_PAGE_SIZE as usize);
        assert_eq!(d.timeline_total, 200);
        assert_eq!(d.timeline_next_offset, Some(150));
        assert_eq!(d.timeline[0].preview.as_deref(), Some("プレビュー"));
        assert!(d.timeline[0].turn_id > 0, "turn_body_get に渡せる id");

        let raw = read_session_detail(&conn, "s1", 150).unwrap().unwrap();
        let d = assemble_detail(raw.base, vec![], false, Default::default());
        assert_eq!(d.timeline.len(), 50);
        assert_eq!(d.timeline_next_offset, None, "これ以上は無い");
    }

    #[test]
    fn timeline_total_is_capped_at_1000() {
        let conn = db();
        // 1000 件を超えて入れる (件数だけ見たいので中身は最小)
        conn.execute_batch("BEGIN").unwrap();
        for i in 0..1050 {
            add_turn(&conn, i, NOW + i, "user.message", None);
        }
        conn.execute_batch("COMMIT").unwrap();

        let raw = read_session_detail(&conn, "s1", 0).unwrap().unwrap();
        assert_eq!(raw.base.timeline_total, TIMELINE_MAX);
    }

    /// FR-C-105: `modelMetrics` が読めないときの降格。**出所が必ず載る** (INV-7)
    #[test]
    fn model_breakdown_falls_back_to_turn_index_with_its_source_labelled() {
        let conn = db();
        add_turn(&conn, 0, NOW, "assistant.message", Some("gpt-5-mini"));
        add_turn(&conn, 1, NOW + 1, "assistant.message", Some("gpt-5-mini"));
        add_turn(&conn, 2, NOW + 2, "assistant.message", Some("auto"));

        let raw = read_session_detail(&conn, "s1", 0).unwrap().unwrap();
        // shutdown レコードが索引に無いのでシーク読みも走らない
        assert!(raw.shutdown_at.is_none());
        let models = model_breakdown(raw.shutdown_at, raw.turn_index_models);

        assert_eq!(models.len(), 1, "合成モデルは内訳に出さない (FR-C-104)");
        assert_eq!(models[0].model, "gpt-5-mini");
        assert_eq!(models[0].source, ModelUsageSource::TurnIndex);
        assert_eq!(models[0].output_tokens, Some(10));
        assert_eq!(models[0].record_count, Some(2));
        // 取れない項目を 0 で埋めない (NFR-43 / INV-7)
        assert_eq!(models[0].input_tokens, None);
        assert_eq!(models[0].credits, None);
    }

    /// 複数の `session.shutdown` があるセッション (実データで 4 件観測)。
    /// **足さずに最新の 1 件を採る** — レコードが累計値だから (ADR-0029)
    #[test]
    fn the_latest_shutdown_record_is_the_one_read() {
        let conn = db();
        add_turn(&conn, 0, NOW, "session.shutdown", None);
        add_turn(&conn, 100, NOW + 5000, "session.shutdown", None);

        let raw = read_session_detail(&conn, "s1", 0).unwrap().unwrap();
        let (_, offset, _) = raw.shutdown_at.unwrap();
        assert_eq!(offset, 100, "後の shutdown が前のものを含む");
    }

    /// 基本情報とクレジット換算 (FR-C-89 / FR-C-112)
    #[test]
    fn base_info_and_credits_are_filled() {
        let conn = db();
        let d = detail(&conn, false, &[]);
        assert_eq!(d.session.title.as_deref(), Some("バグを直す"));
        assert_eq!(d.session.folder_name.as_deref(), Some("Foo"));
        assert_eq!(
            d.session.entrypoint,
            crate::copilot::Entrypoint::CliInteractive
        );
        assert!((d.credits - 0.382635).abs() < 1e-9);
        // 0 件は「無かった」。取得不可ではない (FR-C-118)
        assert!(d.quota_events.is_empty());
    }

    /// 実 `~/.copilot` に対する**読み取り専用**の確認 (NFR-53 / INV-1)。
    ///
    /// 既定では走らない。
    /// `cargo test --lib probe_real_session_detail -- --ignored --nocapture`
    ///
    /// インメモリ DB に実データを索引してから `usage_today` /
    /// `read_session_detail` を通し、モデル別集計・木・タイムラインが
    /// 妥当な値になることを目で確かめる。**何も書かない。**
    #[test]
    #[ignore]
    fn probe_real_session_detail() {
        use std::sync::{Arc, Mutex};

        let _g = crate::copilot::sessions::COPILOT_HOME_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("COPILOT_HOME");
        let Some(home) = crate::copilot::indexer::default_home() else {
            println!("~/.copilot が見つかりません");
            return;
        };

        let db = Arc::new(Mutex::new(crate::db::open_in_memory().unwrap()));
        let t = std::time::Instant::now();
        let stats = crate::copilot::indexer::run(&db, &home, now_ms(), |_| {});
        println!(
            "索引: files={} records={} skipped={} failed={} elapsed={:?}",
            stats.total_files,
            stats.records_ingested,
            stats.skipped_lines,
            stats.failed_files,
            t.elapsed()
        );

        let conn = db.lock().unwrap();
        // 「今日」に活動が無い環境でも経路を通せるよう、データのある最新の日を使う
        let newest: i64 = conn
            .query_row("SELECT COALESCE(MAX(timestamp_ms), 0) FROM turn_index", [], |r| r.get(0))
            .unwrap();
        let day_start = newest - newest.rem_euclid(86_400_000);
        println!("集計起点 (データのある最新日 0:00 UTC) = {day_start}");
        let u = usage_since(&conn, day_start).unwrap();
        println!(
            "本日: in={} out={} cache_read={} turns={} sessions={} subagents={} credits={:.3} excluded={}",
            u.input_tokens,
            u.output_tokens,
            u.cache_read_tokens,
            u.turn_count,
            u.session_count,
            u.subagent_count,
            crate::copilot::quota::credits_from_nano_aiu(u.total_nano_aiu),
            u.excluded_records
        );
        println!("  時間帯別: {:?}", u.hourly_tokens);
        println!("  フォルダ別: {:?}", u.top_folders);

        // サブエージェントを使ったセッションを優先して 3 件見る
        let mut stmt = conn
            .prepare(
                "SELECT session_id FROM sessions ORDER BY agent_count DESC, total_nano_aiu DESC LIMIT 3",
            )
            .unwrap();
        let ids: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        drop(stmt);

        for id in ids {
            let raw = read_session_detail(&conn, &id, 0).unwrap().unwrap();
            let models = model_breakdown(raw.shutdown_at, raw.turn_index_models);
            let d = assemble_detail(raw.base, models, false, Default::default());
            println!(
                "\nセッション {} \"{}\" turns={} credits={:.3} agents={}",
                &d.session.session_id[..8],
                d.session.title.as_deref().unwrap_or("-"),
                d.session.turn_count,
                d.credits,
                d.subagents.len()
            );
            let sum: f64 = d.models.iter().filter_map(|m| m.credits).sum();
            for m in &d.models {
                println!(
                    "  {:>18} src={:?} in={:?} out={:?} cr={:?} cw={:?} credits={:?}",
                    m.model,
                    m.source,
                    m.input_tokens,
                    m.output_tokens,
                    m.cache_read_tokens,
                    m.cache_write_tokens,
                    m.credits
                );
            }
            // モデル別クレジットの合計がセッション総量と一致する (二重加算していない)
            if !d.models.is_empty() && d.models[0].source == ModelUsageSource::ShutdownMetrics {
                println!("  モデル別合計={sum:.6} / セッション総量={:.6}", d.credits);
                assert!(
                    (sum - d.credits).abs() < 1e-6,
                    "モデル別の合計がセッション総量と合わない = 二重加算か取りこぼし"
                );
            }
            for n in d.subagents.iter().take(5) {
                println!(
                    "  {}{} type={:?} depth={} orphan={} calls={}",
                    "  ".repeat(n.depth),
                    n.run_key,
                    n.agent_type,
                    n.depth,
                    n.orphaned,
                    n.tool_call_count
                );
            }
            println!(
                "  タイムライン {}/{} next={:?}",
                d.timeline.len(),
                d.timeline_total,
                d.timeline_next_offset
            );
            for t in d.timeline.iter().take(3) {
                println!(
                    "    #{} {:?} {:?} {:?}",
                    t.turn_id,
                    t.record_type.as_deref().unwrap_or("-"),
                    t.model,
                    t.preview.as_deref().map(|p| p.chars().take(40).collect::<String>())
                );
            }
        }
    }

    #[test]
    fn quota_event_markers_are_returned_in_time_order() {
        let conn = db();
        for t in [NOW + 500, NOW + 100] {
            conn.execute(
                "INSERT INTO quota_events (session_id, occurred_at, kind, reset_text) \
                 VALUES ('s1', ?1, 'credit_exhausted', '明日 0:00')",
                params![t],
            )
            .unwrap();
        }
        let d = detail(&conn, false, &[]);
        assert_eq!(d.quota_events.len(), 2);
        assert_eq!(d.quota_events[0].occurred_at, NOW + 100);
        assert_eq!(d.quota_events[0].kind, "credit_exhausted");
    }
}

#[cfg(test)]
mod turn_body_tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    /// `turn_index` に 1 行 INSERT し、対応する一時ファイルを用意する。
    fn setup(
        body: &[u8],
        byte_length: u64,
    ) -> (Arc<Mutex<rusqlite::Connection>>, i64, std::path::PathBuf) {
        let conn = crate::db::open_in_memory().expect("in-memory db");

        let mut path = std::env::temp_dir();
        path.push(format!(
            "turn_body_test_{}_{}.bin",
            std::process::id(),
            rand_suffix()
        ));
        std::fs::File::create(&path)
            .and_then(|mut f| f.write_all(body))
            .expect("write temp file");

        let byte_offset: i64 = 0;
        conn.execute(
            "INSERT INTO turn_index (file_path, byte_offset, byte_length) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                path.to_string_lossy().to_string(),
                byte_offset,
                byte_length as i64
            ],
        )
        .expect("insert turn_index row");
        let turn_id = conn.last_insert_rowid();

        (Arc::new(Mutex::new(conn)), turn_id, path)
    }

    /// テスト間でファイル名が衝突しないようにするだけの単純な乱数代わり。
    fn rand_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    #[tokio::test]
    async fn reads_exact_byte_range() {
        let body = b"hello turn body";
        let (db, turn_id, path) = setup(body, body.len() as u64);

        let result = turn_body_get_impl(db, turn_id).await.expect("should read");

        assert_eq!(result.turn_id, turn_id);
        assert_eq!(result.body, "hello turn body");
        assert!(!result.truncated);

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn truncates_at_512kb_limit() {
        let body = vec![b'x'; (TURN_BODY_MAX_BYTES + 1000) as usize];
        let (db, turn_id, path) = setup(&body, body.len() as u64);

        let result = turn_body_get_impl(db, turn_id).await.expect("should read");

        assert!(result.truncated, "上限超過は truncated: true");
        assert_eq!(result.body.len() as u64, TURN_BODY_MAX_BYTES);

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn offset_out_of_range_is_an_error_asking_to_reindex() {
        // ファイルは空だが、索引はオフセット 100 を指している (縮小/入れ替わり想定)
        let conn = crate::db::open_in_memory().expect("in-memory db");
        let mut path = std::env::temp_dir();
        path.push(format!(
            "turn_body_test_empty_{}_{}.bin",
            std::process::id(),
            rand_suffix()
        ));
        std::fs::File::create(&path).expect("create empty temp file");

        conn.execute(
            "INSERT INTO turn_index (file_path, byte_offset, byte_length) VALUES (?1, ?2, ?3)",
            rusqlite::params![path.to_string_lossy().to_string(), 100i64, 10i64],
        )
        .expect("insert turn_index row");
        let turn_id = conn.last_insert_rowid();
        let db = Arc::new(Mutex::new(conn));

        let err = turn_body_get_impl(db, turn_id)
            .await
            .expect_err("範囲外オフセットはエラー");

        match err {
            AppError::Unavailable { how_to_fix, .. } => {
                assert!(how_to_fix.unwrap_or_default().contains("再実行"));
            }
            other => panic!("unavailable エラーを期待した: {other:?}"),
        }

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn missing_turn_id_is_not_found() {
        let conn = crate::db::open_in_memory().expect("in-memory db");
        let db = Arc::new(Mutex::new(conn));

        let err = turn_body_get_impl(db, 999)
            .await
            .expect_err("存在しない id はエラー");
        assert!(matches!(err, AppError::NotFound { .. }));
    }
}
