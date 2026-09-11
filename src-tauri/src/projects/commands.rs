//! プロジェクト機能の IPC コマンド (IR-01〜06)。
//!
//! **全コマンドに `rename_all = "snake_case"` を付ける** (IR-30)。
//! Tauri は既定で引数名を camelCase に変換し、`path_key` を渡したつもりが
//! `pathKey` を要求されて失敗する。型検査でも lint でも検出できない事故クラス。
//!
//! 変更系は「検証 → 永続化 → スナップショット → イベント + 戻り値」(IR-32)。
//!
//! 実装状況: T-1.4 / T-1.7 でプロジェクト系コマンドを実装済み。dev サーバー起動・
//! 外部ツール起動は T-3.2〜3.7 で実装済み。

use std::path::Path;

use tauri::{AppHandle, Emitter, State};

use crate::error::{AppError, AppResult};
use crate::projects::{
    dev_server, external, git, scan, store, DevState, Project, ProjectOverride,
    ProjectOverrideRequest, ProjectsSnapshot,
};
use crate::state::AppState;

/// git 状態取得の同時実行数上限 (FR-P-45)。
const GIT_STATUS_CONCURRENCY: usize = 8;

/// スナップショット中の各プロジェクトの git 状態を取得して埋める。
///
/// `git::git_status` は同期関数なので `spawn_blocking` で包み、
/// `tokio::sync::Semaphore` で同時実行数を制限する (NFR-20 / FR-P-45)。
async fn fill_git_status(projects: &mut [Project]) {
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(GIT_STATUS_CONCURRENCY));
    let tasks: Vec<_> = projects
        .iter()
        .map(|p| {
            let root = p.root_path.clone();
            let semaphore = semaphore.clone();
            tokio::spawn(async move {
                // Semaphore は閉じない限り acquire は失敗しない。
                let _permit = semaphore.acquire_owned().await.ok();
                tokio::task::spawn_blocking(move || git::git_status(Path::new(&root)))
                    .await
                    .unwrap_or(None)
            })
        })
        .collect();

    for (project, task) in projects.iter_mut().zip(tasks) {
        project.git = task.await.unwrap_or(None);
    }
}

/// スナップショットに Copilot 利用状況を埋める (FR-P-50〜58)。
///
/// 読み取りと紐付けは同期処理なので `spawn_blocking` で包む (INV-10 / NFR-20)。
/// 失敗してもスキャン全体は落とさない — 警告を積んで `copilot` は `None` のまま進める
/// (NFR-24)。読めない件数・紐付かない件数は `warnings` に積む (NFR-43)。
async fn fill_copilot_usage(snapshot: &mut ProjectsSnapshot) {
    let project_keys: Vec<String> = snapshot
        .projects
        .iter()
        .map(|p| p.path_key.clone())
        .collect();
    let now = now_ms();
    let outcome = tokio::task::spawn_blocking(move || {
        let collected = crate::copilot::sessions::collect_candidates(now);
        let link_result =
            crate::projects::copilot_link::link(&project_keys, &collected.candidates);
        (collected, link_result)
    })
    .await;

    let Ok((collected, link_result)) = outcome else {
        snapshot
            .warnings
            .push("Copilot 利用状況の取得に失敗しました".to_string());
        return;
    };

    for project in snapshot.projects.iter_mut() {
        project.copilot = link_result.usage.get(&project.path_key).cloned();
    }

    if collected.unreadable_sessions > 0 {
        snapshot.warnings.push(format!(
            "読み取れなかった Copilot セッション: {} 件",
            collected.unreadable_sessions
        ));
    }
    if collected.skipped_lines > 0 {
        snapshot.warnings.push(format!(
            "解釈できなかった Copilot ログ行: {} 件",
            collected.skipped_lines
        ));
    }
    if link_result.unmatched_sessions > 0 {
        snapshot.warnings.push(format!(
            "プロジェクトに紐付かなかった Copilot セッション: {} 件",
            link_result.unmatched_sessions
        ));
    }
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

    fill_git_status(&mut snapshot.projects).await;
    fill_copilot_usage(&mut snapshot).await;

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

    let mut rebuilt = rebuilt;
    rebuilt.git = tokio::task::spawn_blocking({
        let root = rebuilt.root_path.clone();
        move || git::git_status(Path::new(&root))
    })
    .await
    .map_err(spawn_err)?;

    let mut new_snapshot = snapshot;
    if let Some(slot) = new_snapshot
        .projects
        .iter_mut()
        .find(|p| p.path_key == path_key)
    {
        // 再スキャンせず、直前のキャッシュに付いていた copilot 値をそのまま引き継ぐ
        // (FR-P-05: フル再計算は行わない。path_key が同じなら引き継ぎは常に正しい)
        rebuilt.copilot = slot.copilot.clone();
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

/// キャッシュ済みスナップショットから 1 プロジェクトを引く。dev 操作・外部ツール
/// 起動はどれもここから `working_dir` / `root_path` / `resolved_command` 等を得る。
fn cached_project(state: &AppState, path_key: &str) -> AppResult<Project> {
    state
        .projects_cache
        .lock()
        .map_err(cache_lock_err)?
        .clone()
        .and_then(|s| s.projects.into_iter().find(|p| p.path_key == path_key))
        .ok_or_else(|| AppError::not_found("プロジェクト"))
}

/// `app.emit` を包む `StatusHook` / `LogHook` を組み立てる。`dev_server` は
/// `AppHandle` を知らない (テストしやすさのため) ので、ここで橋渡しする。
fn dev_hooks(app: &AppHandle) -> (dev_server::StatusHook, dev_server::LogHook) {
    let status_app = app.clone();
    let on_status: dev_server::StatusHook = std::sync::Arc::new(move |path_key, state| {
        let _ = status_app.emit(
            "projects-dev-status",
            serde_json::json!({ "path_key": path_key, "state": state }),
        );
    });
    let log_app = app.clone();
    let on_logs: dev_server::LogHook = std::sync::Arc::new(move |path_key, lines| {
        let _ = log_app.emit(
            "projects-dev-log",
            serde_json::json!({ "path_key": path_key, "lines": lines }),
        );
    });
    (on_status, on_logs)
}

/// プロセスツリーごと強制終了する。**結果は見ない** — dev_stop は冪等に「停止中」
/// を返す仕様なので (FR-P-66)、`taskkill` 自体の成否で分岐しない。
async fn kill_process_tree(pid: u32) {
    let _ = tokio::task::spawn_blocking(move || {
        std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output()
    })
    .await;
}

/// IR-04: 起動。**起動可否は UI のボタン無効化だけに頼らず、ここでも再チェックする** (FR-P-23)。
#[tauri::command(rename_all = "snake_case")]
pub async fn projects_dev_start(
    app: AppHandle,
    state: State<'_, AppState>,
    path_key: String,
    command: Option<String>,
) -> AppResult<DevState> {
    // 二重起動防止。UI がボタンを無効化していても、直接呼ばれた場合に備える
    if matches!(
        state.dev.state_of(&path_key),
        DevState::Starting | DevState::Running { .. }
    ) {
        return Ok(state.dev.state_of(&path_key));
    }

    let project = cached_project(&state, &path_key)?;
    let (on_status, on_logs) = dev_hooks(&app);

    // FR-P-23: 起動不可 (自アプリ自身など) はここでも弾く。5 状態の一部として
    // `Failed` を返す — IPC エラーにはしない (起動試行の「結果」として扱う)
    if let Some(reason) = project.launch_blocked_reason {
        let failed = DevState::Failed { reason };
        state.dev.set_state(&path_key, failed.clone());
        on_status(&path_key, &failed);
        return Ok(failed);
    }

    let resolved = command
        .filter(|c| !c.trim().is_empty())
        .or(project.resolved_command);
    let Some(resolved) = resolved else {
        let failed = DevState::Failed {
            reason: "起動コマンドがありません。手動調整で起動コマンドを設定してください".to_string(),
        };
        state.dev.set_state(&path_key, failed.clone());
        on_status(&path_key, &failed);
        return Ok(failed);
    };

    Ok(dev_server::start(
        state.dev.clone(),
        state.job.clone(),
        path_key,
        project.working_dir,
        resolved,
        on_status,
        on_logs,
    )
    .await)
}

/// IR-04: 停止。**未起動でもエラーにせず「停止中」を返す** (冪等。FR-P-66)。
#[tauri::command(rename_all = "snake_case")]
pub async fn projects_dev_stop(
    app: AppHandle,
    state: State<'_, AppState>,
    path_key: String,
) -> AppResult<DevState> {
    // **先に `Stopped` にしてから** `taskkill` を待つ。逆順だと、バックグラウンドの
    // 終了検出 (`mark_exited`) が taskkill の完了より先に届き、`Exited` → `Stopped`
    // と一瞬で二重に状態変化イベントが飛ぶことがある。先に `Stopped` にしておけば
    // `mark_exited` の pid ガードで無視される (dev_server.rs)。
    let pid = match state.dev.state_of(&path_key) {
        DevState::Running { pid, .. } => Some(pid),
        _ => None,
    };
    let stopped = state.dev.mark_stopped(&path_key);
    let _ = app.emit(
        "projects-dev-status",
        serde_json::json!({ "path_key": path_key, "state": stopped }),
    );
    if let Some(pid) = pid {
        kill_process_tree(pid).await;
    }
    Ok(stopped)
}

/// IR-04: すべて停止。UI 側で二段階確認を挟む (FR-P-67)。
#[tauri::command(rename_all = "snake_case")]
pub async fn projects_dev_stop_all(
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<ProjectsSnapshot> {
    for key in state.dev.running_keys() {
        // 単体の `projects_dev_stop` と同じ理由で、`Stopped` にしてから kill する
        let pid = match state.dev.state_of(&key) {
            DevState::Running { pid, .. } => Some(pid),
            _ => None,
        };
        let stopped = state.dev.mark_stopped(&key);
        let _ = app.emit(
            "projects-dev-status",
            serde_json::json!({ "path_key": key, "state": stopped }),
        );
        if let Some(pid) = pid {
            kill_process_tree(pid).await;
        }
    }

    // 再スキャンはせず、キャッシュの dev 状態だけ最新化する (FR-P-05 と同じ考え方)
    let cached = state.projects_cache.lock().map_err(cache_lock_err)?.clone();
    let mut snapshot = match cached {
        Some(s) => s,
        None => return scan_and_cache(&app, &state).await,
    };
    for p in snapshot.projects.iter_mut() {
        p.dev = state.dev.state_of(&p.path_key);
    }
    *state.projects_cache.lock().map_err(cache_lock_err)? = Some(snapshot.clone());
    let _ = app.emit(EVENT_SNAPSHOT, &snapshot);
    Ok(snapshot)
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
pub async fn projects_open_vscode(state: State<'_, AppState>, path_key: String) -> AppResult<()> {
    let dir = cached_project(&state, &path_key)?.root_path;
    tokio::task::spawn_blocking(move || external::open_vscode(&dir))
        .await
        .map_err(spawn_err)?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn projects_open_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    path_key: String,
) -> AppResult<()> {
    let dir = cached_project(&state, &path_key)?.root_path;
    tokio::task::spawn_blocking(move || external::open_folder(&app, &dir))
        .await
        .map_err(spawn_err)?
}

/// Windows Terminal を優先し、失敗したら PowerShell にフォールバック (FR-P-74)。
#[tauri::command(rename_all = "snake_case")]
pub async fn projects_open_terminal(
    state: State<'_, AppState>,
    path_key: String,
) -> AppResult<()> {
    let dir = cached_project(&state, &path_key)?.working_dir;
    tokio::task::spawn_blocking(move || external::open_terminal(&dir))
        .await
        .map_err(spawn_err)?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn projects_open_agent(state: State<'_, AppState>, path_key: String) -> AppResult<()> {
    let dir = cached_project(&state, &path_key)?.working_dir;
    tokio::task::spawn_blocking(move || external::open_agent(&dir))
        .await
        .map_err(spawn_err)?
}

/// FR-P-70 の最後の導線 (稼働中かつ URL 検出済みのときだけ UI が呼ぶ)。
/// プロジェクトの検索は不要 — フロントが既に持っている URL をそのまま渡す。
#[tauri::command(rename_all = "snake_case")]
pub async fn projects_open_browser(app: AppHandle, url: String) -> AppResult<()> {
    tokio::task::spawn_blocking(move || external::open_browser(&app, &url))
        .await
        .map_err(spawn_err)?
}
