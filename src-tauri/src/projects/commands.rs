//! プロジェクト機能の IPC コマンド (IR-01〜06)。
//!
//! **全コマンドに `rename_all = "snake_case"` を付ける** (IR-30)。
//! Tauri は既定で引数名を camelCase に変換し、`path_key` を渡したつもりが
//! `pathKey` を要求されて失敗する。型検査でも lint でも検出できない事故クラス。
//!
//! 変更系は「検証 → 永続化 → スナップショット → イベント + 戻り値」(IR-32)。
//!
//! 実装状況: T-1.4 / T-1.7 でプロジェクト系コマンドを実装済み。dev サーバー起動・
//! 外部ツール起動は段階 3 (T-3.x) で実装する。

use std::path::Path;

use tauri::{AppHandle, Emitter, State};

use crate::error::{AppError, AppResult};
use crate::projects::{
    scan, store, DevState, Project, ProjectOverride, ProjectOverrideRequest, ProjectsSnapshot,
};
use crate::state::AppState;

/// 未実装のコマンドが「静かに空を返す」ことを防ぐ。
fn todo_err(task: &str) -> AppError {
    AppError::unavailable(
        format!("未実装です ({task})"),
        Some("docs/implementation-plan.html の該当タスクを参照してください".to_string()),
    )
}

/// スナップショット更新イベント (IR-40)。
const EVENT_SNAPSHOT: &str = "projects-snapshot";

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `Mutex` の汚染をユーザー向けの `AppError` に変換する。
fn db_lock_err<T>(_: std::sync::PoisonError<T>) -> AppError {
    AppError::Db {
        message: "データベースのロック取得に失敗しました".to_string(),
    }
}

fn cache_lock_err<T>(_: std::sync::PoisonError<T>) -> AppError {
    AppError::Db {
        message: "内部キャッシュのロック取得に失敗しました".to_string(),
    }
}

/// `spawn_blocking` 自体が failed (パニック等) した場合の変換。
fn spawn_err(e: tokio::task::JoinError) -> AppError {
    AppError::Io {
        message: e.to_string(),
    }
}

/// 未登録時に**その場限りで**使う既定フォルダ (FR-P-02)。**DB には書かない。**
///
/// 要求は `%USERPROFILE%\Documents\Projects` と書いている。`dirs::document_dir()` は
/// シェルの「ドキュメント」既知フォルダ (OneDrive にリダイレクトされていると
/// `OneDrive\ドキュメント`) を返し、要求と食い違う実例があったため使わない。
/// `USERPROFILE` が無い環境でだけ `dirs::home_dir()` に落とす。
fn default_scan_folder() -> Option<String> {
    let home = std::env::var_os("USERPROFILE")
        .map(std::path::PathBuf::from)
        .or_else(dirs::home_dir)?;
    Some(
        home.join("Documents")
            .join("Projects")
            .to_string_lossy()
            .to_string(),
    )
}

// ---------------------------------------------------------------- IR-01..03

/// 実スキャンして `state.projects_cache` を更新し、イベント + 戻り値で返す共通経路。
///
/// **DB アクセスとディスク走査は必ず `spawn_blocking`** (NFR-20)。
async fn scan_and_cache(app: &AppHandle, state: &AppState) -> AppResult<ProjectsSnapshot> {
    let db = state.db.clone();
    let (folders, overrides) = tokio::task::spawn_blocking(move || -> AppResult<_> {
        let conn = db.lock().map_err(db_lock_err)?;
        let folders = store::scan_folders(&conn)?;
        let overrides = store::overrides(&conn)?;
        Ok((folders, overrides))
    })
    .await
    .map_err(spawn_err)??;

    // 登録が空なら既定フォルダをその場限りで使う。DB には書かない (FR-P-02)。
    let mut default_warnings = Vec::new();
    let (effective_folders, using_default) = if folders.is_empty() {
        match default_scan_folder() {
            Some(path) if Path::new(&path).is_dir() => (vec![path], true),
            Some(path) => {
                default_warnings.push(format!(
                    "既定フォルダ {path} が存在しません。スキャン対象フォルダを追加してください"
                ));
                (Vec::new(), true)
            }
            None => {
                default_warnings.push(
                    "既定フォルダを解決できません。スキャン対象フォルダを追加してください"
                        .to_string(),
                );
                (Vec::new(), true)
            }
        }
    } else {
        (folders, false)
    };

    let dev = state.dev.clone();
    let ts = now_ms();
    let mut snapshot = tokio::task::spawn_blocking(move || {
        scan::run(&effective_folders, using_default, &overrides, &dev, ts)
    })
    .await
    .map_err(spawn_err)?;

    if !default_warnings.is_empty() {
        default_warnings.extend(snapshot.warnings);
        snapshot.warnings = default_warnings;
    }

    *state.projects_cache.lock().map_err(cache_lock_err)? = Some(snapshot.clone());

    // イベントと戻り値の両方で返す (IR-32)
    let _ = app.emit(EVENT_SNAPSHOT, &snapshot);
    Ok(snapshot)
}

/// IR-01: **必ず実スキャンしてから返す。**読み出し専用版は持たない。
#[tauri::command(rename_all = "snake_case")]
pub async fn projects_scan(
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<ProjectsSnapshot> {
    scan_and_cache(&app, &state).await
}

/// IR-02: **再スキャンせず**、キャッシュ済み結果と再マージする (FR-P-05)。
#[tauri::command(rename_all = "snake_case")]
pub async fn projects_settings_update(
    app: AppHandle,
    state: State<'_, AppState>,
    req: ProjectOverrideRequest,
) -> AppResult<ProjectsSnapshot> {
    let cached = state.projects_cache.lock().map_err(cache_lock_err)?.clone();
    let snapshot = match cached {
        Some(s) => s,
        // 未スキャンならまず実スキャンする
        None => scan_and_cache(&app, &state).await?,
    };

    let root_path = snapshot
        .projects
        .iter()
        .find(|p| p.path_key == req.path_key)
        .map(|p| p.root_path.clone())
        .ok_or_else(|| AppError::not_found("プロジェクト"))?;

    let ProjectOverrideRequest {
        path_key,
        display_name,
        command_override,
        working_dir_override,
        sort_order,
        hidden,
        archived,
    } = req;

    // (a) working_dir_override の実在検証。通らなければ DB に書かない (FR-P-32)。
    if let Some(value) = working_dir_override.as_deref() {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            let root_for_check = root_path.clone();
            let value_owned = trimmed.to_string();
            let exists = tokio::task::spawn_blocking(move || {
                scan::resolve_working_dir_override(Path::new(&root_for_check), &value_owned)
                    .is_dir()
            })
            .await
            .map_err(spawn_err)?;
            if !exists {
                return Err(AppError::invalid(
                    "working_dir_override",
                    format!("指定したディレクトリが存在しません: {trimmed}"),
                ));
            }
        }
    }

    // (b) 永続化
    let db = state.db.clone();
    let ts = now_ms();
    let key_for_persist = path_key.clone();
    let fields = ProjectOverride {
        display_name,
        command_override,
        working_dir_override,
        sort_order,
        hidden: hidden.unwrap_or(false),
        archived: archived.unwrap_or(false),
    };
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let conn = db.lock().map_err(db_lock_err)?;
        store::override_upsert(&conn, &key_for_persist, &fields, ts)
    })
    .await
    .map_err(spawn_err)??;

    // (c) 再スキャンせず、該当プロジェクトだけ再マージする (FR-P-05)。
    let db2 = state.db.clone();
    let dev = state.dev.clone();
    let self_key = scan::self_source_key();
    let key_for_merge = path_key.clone();
    let root_for_merge = root_path.clone();
    let (rebuilt, merge_warnings) =
        tokio::task::spawn_blocking(move || -> AppResult<(Project, Vec<String>)> {
            let conn = db2.lock().map_err(db_lock_err)?;
            let ov = store::override_of(&conn, &key_for_merge)?;
            let mut warnings = Vec::new();
            let project = scan::build_project(
                Path::new(&root_for_merge),
                &key_for_merge,
                ov.as_ref(),
                &dev,
                self_key.as_deref(),
                &mut warnings,
            )
            .ok_or_else(|| AppError::not_found("プロジェクト"))?;
            Ok((project, warnings))
        })
        .await
        .map_err(spawn_err)??;

    let mut new_snapshot = snapshot;
    if let Some(slot) = new_snapshot
        .projects
        .iter_mut()
        .find(|p| p.path_key == path_key)
    {
        *slot = rebuilt;
    }
    new_snapshot.warnings.extend(merge_warnings);

    *state.projects_cache.lock().map_err(cache_lock_err)? = Some(new_snapshot.clone());
    let _ = app.emit(EVENT_SNAPSHOT, &new_snapshot);
    Ok(new_snapshot)
}

/// IR-03: 追加時は存在確認 + 重複を分かりやすいメッセージに変換。
#[tauri::command(rename_all = "snake_case")]
pub async fn projects_scan_folder_add(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> AppResult<ProjectsSnapshot> {
    let db = state.db.clone();
    let ts = now_ms();
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let conn = db.lock().map_err(db_lock_err)?;
        store::scan_folder_add(&conn, &path, ts)
    })
    .await
    .map_err(spawn_err)??;

    scan_and_cache(&app, &state).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn projects_scan_folder_remove(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> AppResult<ProjectsSnapshot> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let conn = db.lock().map_err(db_lock_err)?;
        store::scan_folder_remove(&conn, &path)
    })
    .await
    .map_err(spawn_err)??;

    scan_and_cache(&app, &state).await
}

// ---------------------------------------------------------------- IR-04..05

/// IR-04: 起動。**起動可否は UI のボタン無効化だけに頼らず、ここでも再チェックする** (FR-P-23)。
#[tauri::command(rename_all = "snake_case")]
pub async fn projects_dev_start(
    _state: State<'_, AppState>,
    _path_key: String,
    _command: Option<String>,
) -> AppResult<DevState> {
    // TODO(T-3.2): 子プロセスを起こし、Job Object に割り当てる (FR-P-61)
    //   - stdout/stderr を非同期に行読みし、url_detect で URL を拾う (FR-P-63)
    Err(todo_err("T-3.2"))
}

/// IR-04: 停止。**未起動でもエラーにせず「停止中」を返す** (冪等。FR-P-66)。
#[tauri::command(rename_all = "snake_case")]
pub async fn projects_dev_stop(
    state: State<'_, AppState>,
    path_key: String,
) -> AppResult<DevState> {
    // TODO(T-3.2): 実プロセスの停止。現状は状態のリセットのみ。
    Ok(state.dev.mark_stopped(&path_key))
}

/// IR-04: すべて停止。UI 側で二段階確認を挟む (FR-P-67)。
#[tauri::command(rename_all = "snake_case")]
pub async fn projects_dev_stop_all(_state: State<'_, AppState>) -> AppResult<ProjectsSnapshot> {
    Err(todo_err("T-3.2"))
}

/// IR-05: ログ最大 500 行。詳細パネルを開いた直後の初期表示用。
#[tauri::command(rename_all = "snake_case")]
pub async fn projects_dev_logs_get(
    state: State<'_, AppState>,
    path_key: String,
) -> AppResult<Vec<String>> {
    Ok(state.dev.logs(&path_key))
}

// ---------------------------------------------------------------- IR-06

// 外部ツールは投げっぱなしで起動し、完了を待たない (FR-P-71)。
// **起動成否は spawn の成否のみで判定し、終了コードを見ない** (FR-P-72) —
// explorer.exe は正常時でも終了コード 1 を返す。

#[tauri::command(rename_all = "snake_case")]
pub async fn projects_open_vscode(_state: State<'_, AppState>, _path_key: String) -> AppResult<()> {
    Err(todo_err("T-3.7"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn projects_open_folder(_state: State<'_, AppState>, _path_key: String) -> AppResult<()> {
    Err(todo_err("T-3.7"))
}

/// Windows Terminal を優先し、失敗したら PowerShell にフォールバック (FR-P-74)。
#[tauri::command(rename_all = "snake_case")]
pub async fn projects_open_terminal(
    _state: State<'_, AppState>,
    _path_key: String,
) -> AppResult<()> {
    Err(todo_err("T-3.7"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn projects_open_agent(_state: State<'_, AppState>, _path_key: String) -> AppResult<()> {
    Err(todo_err("T-3.7"))
}
