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

use tauri::{AppHandle, Emitter, State};

use crate::copilot::{
    indexer, store, AnimationPref, DbSnapshot, LiveStatus, SessionQuery, SessionSummary, TurnBody,
    UsageToday, TURN_BODY_MAX_BYTES,
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
#[tauri::command(rename_all = "snake_case")]
pub async fn live_status_get(_state: State<'_, AppState>) -> AppResult<LiveStatus> {
    // TODO(T-5.1..5.4):
    //   - 状態ファイルを列挙し、PID 生存確認で死んだものを除外 (FR-C-40)
    //   - (パス, サイズ, mtime) が同じならディスクを読み直さない (FR-C-48)
    //   - 末尾 64KB のシーク読み、足りなければ 512KB で 1 回だけ (FR-C-47)
    //   - 稼働中サブエージェントは "集合" を先に作る (FR-C-51)
    //   - activity::synthesize で活動状態を合成 (FR-C-45)
    Ok(LiveStatus::new(vec![], vec![], now_ms()))
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
        let snapshot = db
            .lock()
            .ok()
            .and_then(|conn| store::snapshot(&conn).ok());
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
    _state: State<'_, AppState>,
    query: SessionQuery,
) -> AppResult<Vec<SessionSummary>> {
    let _limit = query.effective_limit();
    // TODO(T-7.4): フォルダ名・タイトル・作業ディレクトリの部分一致で検索
    Ok(vec![])
}

/// IR-14: セッション詳細。
///
/// **未インデックスは `None` (正常系)。エラーにしない** (FR-C-57)。
/// 赤いエラーを出しても、ユーザーには何をすればよいか分からない。
#[tauri::command(rename_all = "snake_case")]
pub async fn session_detail_get(
    _state: State<'_, AppState>,
    _session_id: String,
) -> AppResult<Option<serde_json::Value>> {
    // TODO(T-7.5..7.7): 集計 + 木構築 + ガント用データを返す
    Ok(None)
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

    let read_len = byte_length.min(TURN_BODY_MAX_BYTES);
    let truncated = byte_length > TURN_BODY_MAX_BYTES;

    tokio::task::spawn_blocking(move || -> AppResult<TurnBody> {
        let mut file = std::fs::File::open(&file_path)?;
        let file_size = file.metadata()?.len();
        if byte_offset >= file_size {
            return Err(AppError::unavailable(
                "本文の位置がファイルの範囲外です".to_string(),
                reindex_hint(),
            ));
        }
        file.seek(SeekFrom::Start(byte_offset))?;
        let capped_len = read_len.min(file_size - byte_offset);
        let mut buf = vec![0u8; capped_len as usize];
        file.read_exact(&mut buf)?;
        Ok(TurnBody {
            turn_id,
            body: String::from_utf8_lossy(&buf).into_owned(),
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
pub async fn usage_today_get(_state: State<'_, AppState>) -> AppResult<UsageToday> {
    // TODO(T-7.1..7.2): ローカル日 0:00 からの集計。
    //   合成モデルのレコードを除外し、usage の内訳を二重加算しない (FR-C-104)
    Ok(UsageToday {
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        turn_count: 0,
        session_count: 0,
        subagent_count: 0,
        total_nano_aiu: 0,
        hourly_tokens: vec![0; 24],
        top_folders: vec![],
        excluded_records: 0,
    })
}

// ---------------------------------------------------------------- IR-17..18

/// IR-17: 利用枠。**経路 A→B→C の降格込み**。
///
/// ネットワークを伴うため下限間隔 (5 分) がある。**2 秒ポーリングに載せない** (FR-C-135)。
#[tauri::command(rename_all = "snake_case")]
pub async fn quota_get(
    _state: State<'_, AppState>,
    _force: bool,
) -> AppResult<Vec<crate::copilot::quota::QuotaGauge>> {
    // TODO(T-6.3..6.7): SDK → REST → 推定 の順に試し、quota::degrade で組み立てる。
    //   取れなくても機能全体を止めない (FR-C-136)。枠ごとに独立して評価する (FR-C-84)
    Ok(vec![crate::copilot::quota::degrade(
        "monthly_credits",
        "月次 AI Credits",
        None,
        None,
        None,
        now_ms(),
    )])
}

/// IR-18: 各取得経路の可用性。「何をすれば取れるようになるか」の材料 (FR-C-83)。
#[tauri::command(rename_all = "snake_case")]
pub async fn quota_source_status_get(_state: State<'_, AppState>) -> AppResult<serde_json::Value> {
    // TODO(T-6.11): 認証状態 / SDK 有無 / 直近の取得結果と失敗理由
    Ok(serde_json::json!({
        "sdk": { "available": false, "reason": "未調査 (OQ-06)" },
        "rest": { "available": false, "reason": "未実装 (T-6.5)" },
        "estimate": { "available": false, "reason": "インデックス未実装 (段階 4)" }
    }))
}

// ---------------------------------------------------------------- IR-19

#[tauri::command(rename_all = "snake_case")]
pub async fn animation_pref_get(_state: State<'_, AppState>) -> AppResult<AnimationPref> {
    // TODO(T-7.14): settings テーブルから読む
    Ok(AnimationPref::Auto)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn animation_pref_set(
    _state: State<'_, AppState>,
    pref: AnimationPref,
) -> AppResult<AnimationPref> {
    // TODO(T-7.14): settings テーブルに書く
    Ok(pref)
}

#[cfg(test)]
mod turn_body_tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    /// `turn_index` に 1 行 INSERT し、対応する一時ファイルを用意する。
    fn setup(body: &[u8], byte_length: u64) -> (Arc<Mutex<rusqlite::Connection>>, i64, std::path::PathBuf) {
        let conn = crate::db::open_in_memory().expect("in-memory db");

        let mut path = std::env::temp_dir();
        path.push(format!("turn_body_test_{}_{}.bin", std::process::id(), rand_suffix()));
        std::fs::File::create(&path)
            .and_then(|mut f| f.write_all(body))
            .expect("write temp file");

        let byte_offset: i64 = 0;
        conn.execute(
            "INSERT INTO turn_index (file_path, byte_offset, byte_length) VALUES (?1, ?2, ?3)",
            rusqlite::params![path.to_string_lossy().to_string(), byte_offset, byte_length as i64],
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
        path.push(format!("turn_body_test_empty_{}_{}.bin", std::process::id(), rand_suffix()));
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

        let err = turn_body_get_impl(db, 999).await.expect_err("存在しない id はエラー");
        assert!(matches!(err, AppError::NotFound { .. }));
    }
}
