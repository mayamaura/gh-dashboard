//! プロジェクト機能。
//!
//! **永続化するのは「スキャン対象フォルダ」と「手動調整」の 2 つだけ** (FR-P-04)。
//! スキャン結果・種別判定・git 状態・Copilot 利用状況は導出データなので、
//! 要求時に読み直してメモリに置く。
//!
//! 対応要求: FR-P-01〜88 / IR-01〜06

pub mod commands;
pub mod copilot_link;
pub mod detect;
pub mod dev_server;
pub mod git;
pub mod scan;
pub mod store;

use serde::{Deserialize, Serialize};

pub use detect::ProjectKind;
pub use dev_server::DevState;
pub use store::ProjectOverride;

/// git 状態。`.git` が無ければそもそもこの構造体を作らない (FR-P-42)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatus {
    /// detached HEAD はブランチ名なし (FR-P-41)
    pub branch: Option<String>,
    pub dirty: bool,
    pub has_remote: bool,
    pub last_commit_at: Option<i64>,
}

/// 履歴との照合方法。**推測による紐付けを事実として提示しない** (FR-P-53 / NFR-41)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchedBy {
    /// 作業ディレクトリの完全一致
    Exact,
    /// フォルダ名フォールバック。UI に「旧パスの履歴」と明示する
    FolderNameFallback,
}

/// Copilot 利用状況。**履歴が一切ないプロジェクトではこの構造体を作らない** (FR-P-58)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopilotUsage {
    /// 「保持されているセッション数」。累計ではない (NFR-44)
    pub session_count: i64,
    pub last_used_at: Option<i64>,
    pub last_title: Option<String>,
    pub is_active: bool,
    pub last_nano_aiu: Option<i64>,
    pub lines_added: Option<i64>,
    pub lines_removed: Option<i64>,
    pub matched_by: MatchedBy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub path_key: String,
    /// リポジトリルート。**git の判定対象はこちら** (FR-P-44)
    pub root_path: String,
    /// 起動コマンドを実行するディレクトリ (FR-P-12)
    pub working_dir: String,
    pub display_name: String,
    pub kind: ProjectKind,
    pub kind_label: String,
    /// 許可リスト順の候補 (FR-P-20)
    pub command_candidates: Vec<String>,
    /// 解決済みコマンド。**起動不可は None。空文字で埋めない** (FR-P-21)
    pub resolved_command: Option<String>,
    /// 起動不可の理由。一覧から隠さず、これを表示する (FR-P-22)
    pub launch_blocked_reason: Option<String>,
    pub hidden: bool,
    pub archived: bool,
    pub sort_order: Option<i64>,
    /// 保存済みの手動調整そのもの。手動調整フォームの初期値に使う。
    /// 無ければ `None` (FR-P-30)。**表示用の値 (`display_name` 等) と混同しない**
    pub override_values: Option<ProjectOverride>,
    pub git: Option<GitStatus>,
    pub copilot: Option<CopilotUsage>,
    pub dev: DevState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectsSnapshot {
    pub projects: Vec<Project>,
    pub scan_folders: Vec<String>,
    pub scanned_at: i64,
    /// 既定フォルダをその場限りで使ったか。**DB には書いていない** (FR-P-02)
    pub using_default_folder: bool,
    /// 読めなかったフォルダなど。**無言で欠落させない** (FR-P-03 / NFR-43)
    pub warnings: Vec<String>,
}

/// 手動調整の更新リクエスト (IR-02)。
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectOverrideRequest {
    pub path_key: String,
    pub display_name: Option<String>,
    pub command_override: Option<String>,
    pub working_dir_override: Option<String>,
    pub sort_order: Option<i64>,
    pub hidden: Option<bool>,
    pub archived: Option<bool>,
}
