//! プロジェクト機能の IPC コマンド (IR-01〜06)。
//!
//! **全コマンドに `rename_all = "snake_case"` を付ける** (IR-30)。
//! Tauri は既定で引数名を camelCase に変換し、`path_key` を渡したつもりが
//! `pathKey` を要求されて失敗する。型検査でも lint でも検出できない事故クラス。
//!
//! 変更系は「検証 → 永続化 → スナップショット → イベント + 戻り値」(IR-32)。
//!
//! 実装状況: 署名と規約だけが確定した足場。中身は段階 1〜3 で実装する。

use tauri::State;

use crate::error::{AppError, AppResult};
use crate::projects::{DevState, ProjectOverrideRequest, ProjectsSnapshot};
use crate::state::AppState;

/// 未実装のコマンドが「静かに空を返す」ことを防ぐ。
fn todo_err(task: &str) -> AppError {
    AppError::unavailable(
        format!("未実装です ({task})"),
        Some("docs/implementation-plan.html の該当タスクを参照してください".to_string()),
    )
}

// ---------------------------------------------------------------- IR-01..03

/// IR-01: **必ず実スキャンしてから返す。**読み出し専用版は持たない。
#[tauri::command(rename_all = "snake_case")]
pub async fn projects_scan(_state: State<'_, AppState>) -> AppResult<ProjectsSnapshot> {
    // TODO(T-1.5): スキャン対象フォルダを読み、直下 1 階層だけを走査する (FR-P-01)。
    //   - 走査は spawn_blocking で行う (FR-P-06 / NFR-20)
    //   - 読めないフォルダはスキップして warnings に積む (FR-P-03)
    //   - 登録が空なら既定フォルダをその場限りで使う。**DB には書かない** (FR-P-02)
    //   - 結果は projects_cache に置く。**永続化しない** (FR-P-04)
    Err(todo_err("T-1.5"))
}

/// IR-02: **再スキャンせず**、キャッシュ済み結果と再マージする (FR-P-05)。
#[tauri::command(rename_all = "snake_case")]
pub async fn projects_settings_update(
    _state: State<'_, AppState>,
    _req: ProjectOverrideRequest,
) -> AppResult<ProjectsSnapshot> {
    // TODO(T-1.4): 検証 → 永続化 → 再マージ → イベント + 戻り値 (IR-32)
    //   - working_dir_override は実在検証し、通らなければ書き込まない (FR-P-32)
    Err(todo_err("T-1.4"))
}

/// IR-03: 追加時は存在確認 + 重複を分かりやすいメッセージに変換。
#[tauri::command(rename_all = "snake_case")]
pub async fn projects_scan_folder_add(
    _state: State<'_, AppState>,
    _path: String,
) -> AppResult<ProjectsSnapshot> {
    Err(todo_err("T-1.4"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn projects_scan_folder_remove(
    _state: State<'_, AppState>,
    _path: String,
) -> AppResult<ProjectsSnapshot> {
    Err(todo_err("T-1.4"))
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
pub async fn projects_open_vscode(
    _state: State<'_, AppState>,
    _path_key: String,
) -> AppResult<()> {
    Err(todo_err("T-3.7"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn projects_open_folder(
    _state: State<'_, AppState>,
    _path_key: String,
) -> AppResult<()> {
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
pub async fn projects_open_agent(
    _state: State<'_, AppState>,
    _path_key: String,
) -> AppResult<()> {
    Err(todo_err("T-3.7"))
}
