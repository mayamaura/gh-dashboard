//! Copilot 稼働ダッシュボード。
//!
//! 性質の異なる 3 つの取得経路を**別のモジュールとして**持つ。混ぜると
//! NFR-03 (2 秒ポーリングのコスト) が壊れる。
//!
//! | 経路 | モジュール | ネットワーク | 永続化 |
//! |---|---|---|---|
//! | ライブ | `live` | **禁止** | しない |
//! | インデックス | `indexer` | 禁止 | する |
//! | 利用枠 | `quota` | **ここだけ許可** | 直近値のみ |
//!
//! 対応要求: FR-C-01〜164 / IR-10〜19

pub mod activity;
pub mod commands;
pub mod delta;
pub mod parser;
pub mod quota;
pub mod sessions;
pub mod tree;

use serde::{Deserialize, Serialize};

pub use activity::ActivityState;

/// セッションの起動経路 (用語定義)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Entrypoint {
    CliInteractive,
    CliBackground,
    Vscode,
    CodingAgent,
    Unknown,
}

/// ライブ監視で見える 1 セッション (FR-C-53)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveSession {
    pub session_id: String,
    pub folder_name: Option<String>,
    pub entrypoint: Entrypoint,
    pub title: Option<String>,
    pub activity: ActivityState,
    pub model: Option<String>,
    pub started_at: Option<i64>,
    pub last_activity_at: Option<i64>,
    pub context_used: Option<i64>,
    pub context_limit: Option<i64>,
    pub session_nano_aiu: Option<i64>,
    /// **集合が正。件数は length として導出する** (FR-C-51)
    pub running_subagent_ids: Vec<String>,
}

/// IDE ワークスペース (FR-C-70〜72)。
///
/// 状態ファイル (`~/.copilot/ide/*.lock`) には `headers` フィールドが存在し、
/// 認証情報を含みうる。**この構造体にそのフィールドを定義しない** (INV-2 / FR-C-72)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdeWorkspace {
    pub ide_name: Option<String>,
    pub folders: Vec<String>,
    /// PID が死んでいても**行を捨てず** false にする (FR-C-71)
    pub connected: bool,
}

/// IR-10 の戻り値。**2 秒ごとに呼ばれる** (INV-4)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveStatus {
    pub sessions: Vec<LiveSession>,
    pub ide_workspaces: Vec<IdeWorkspace>,
    pub running_session_count: usize,
    /// 各セッションの `running_subagent_ids` の合計。別に数えない (FR-C-51)
    pub running_subagent_count: usize,
    pub polled_at: i64,
}

impl LiveStatus {
    /// 集合から件数を導出して組み立てる (FR-C-51)。
    ///
    /// 「バッジは稼働中なのに系統図は空」という矛盾が出ようがない形にする。
    pub fn new(sessions: Vec<LiveSession>, ide_workspaces: Vec<IdeWorkspace>, now: i64) -> Self {
        let running_session_count = sessions.len();
        let running_subagent_count = sessions
            .iter()
            .map(|s| s.running_subagent_ids.len())
            .sum::<usize>();
        Self {
            sessions,
            ide_workspaces,
            running_session_count,
            running_subagent_count,
            polled_at: now,
        }
    }
}

/// 差分インデックスの進捗 (IR-43 / FR-C-11)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexProgress {
    pub phase: String,
    pub total_files: usize,
    pub done_files: usize,
    pub current_file: Option<String>,
    pub records_ingested: u64,
    /// 読み飛ばした壊れた行。**無言で欠落させない** (FR-C-12 / NFR-43)
    pub skipped_lines: u64,
}

/// IR-11 の戻り値。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbSnapshot {
    pub last_indexed_at: Option<i64>,
    /// 「保持されているセッション数」。累計ではない (NFR-44)
    pub retained_session_count: i64,
    pub subagent_run_count: i64,
    pub turn_count: i64,
    pub recent_sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub folder_name: Option<String>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub entrypoint: Entrypoint,
    pub started_at: Option<i64>,
    pub last_activity_at: Option<i64>,
    pub turn_count: i64,
    pub total_nano_aiu: i64,
    pub agent_count: i64,
}

/// IR-13 の検索条件。既定 100 / 上限 1000 (FR-C-110)。
#[derive(Debug, Clone, Deserialize)]
pub struct SessionQuery {
    pub text: Option<String>,
    pub limit: Option<usize>,
    /// 「サブエージェントを使ったセッションのみ」(FR-C-111)
    pub with_subagents_only: Option<bool>,
}

pub const SESSION_LIMIT_DEFAULT: usize = 100;
pub const SESSION_LIMIT_MAX: usize = 1000;

impl SessionQuery {
    pub fn effective_limit(&self) -> usize {
        self.limit
            .unwrap_or(SESSION_LIMIT_DEFAULT)
            .min(SESSION_LIMIT_MAX)
    }
}

/// IR-15 の戻り値。本文は都度シーク読みする (FR-C-02 / FR-C-119)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnBody {
    pub turn_id: i64,
    pub body: String,
    /// 上限に達して切り詰めたか (FR-C-120)
    pub truncated: bool,
}

/// 本文読み取りの上限 (FR-C-120)
pub const TURN_BODY_MAX_BYTES: u64 = 512 * 1024;

/// IR-16 の戻り値 (FR-C-100〜105)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageToday {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub turn_count: i64,
    pub session_count: i64,
    pub subagent_count: i64,
    pub total_nano_aiu: i64,
    /// 時間帯別 (入出力のみ。キャッシュは含めない。FR-C-101)
    pub hourly_tokens: Vec<i64>,
    /// フォルダ別の上位 5 件 (FR-C-102)
    pub top_folders: Vec<(String, i64)>,
    /// 集計から除外した件数。**無言で欠落させない** (NFR-43)
    pub excluded_records: i64,
}

/// 段階 2 の紐付け用に、セッション 1 件から読み取った生の候補 (FR-P-50〜52)。
///
/// `Serialize` を derive しない。IPC DTO ではなく中間データ。
#[derive(Debug, Clone)]
pub struct SessionCandidate {
    pub session_id: String,
    pub cwd_raw: Option<String>,
    /// 正規化済み。`None` なら紐付け対象外
    pub path_key: Option<String>,
    /// `path_key` 由来の末尾フォルダ名 (FR-P-52 の条件②)
    pub folder_name: Option<String>,
    /// FR-P-52 の条件①。IO 層が `is_dir()` で埋める。純粋側は stat しない
    pub cwd_exists: bool,
    pub client_name: Option<String>,
    /// `workspace.yaml` の `name` に 140 字プレビューを掛けたもの
    pub title: Option<String>,
    /// 中身のタイムスタンプ (FR-P-56)。mtime ではない
    pub last_used_at: Option<i64>,
    pub last_used_source: TimeSource,
    pub total_nano_aiu: Option<i64>,
    pub lines_added: Option<i64>,
    pub lines_removed: Option<i64>,
    pub is_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeSource {
    EventsTail,
    WorkspaceYaml,
    None,
}

/// アニメーション設定 (FR-C-162 / IR-19)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationPref {
    /// OS の「動きを減らす」設定に追随する
    Auto,
    On,
    Off,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(ids: &[&str]) -> LiveSession {
        LiveSession {
            session_id: "s".into(),
            folder_name: None,
            entrypoint: Entrypoint::Unknown,
            title: None,
            activity: ActivityState::Unknown,
            model: None,
            started_at: None,
            last_activity_at: None,
            context_used: None,
            context_limit: None,
            session_nano_aiu: None,
            running_subagent_ids: ids.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// FR-C-51: 件数と集合が食い違いようがない形にする
    #[test]
    fn subagent_count_is_derived_from_the_id_sets() {
        let status = LiveStatus::new(vec![session(&["a", "b"]), session(&["c"])], vec![], 0);
        assert_eq!(status.running_session_count, 2);
        assert_eq!(status.running_subagent_count, 3);
    }

    #[test]
    fn session_query_limit_is_capped() {
        let q = SessionQuery {
            text: None,
            limit: Some(9999),
            with_subagents_only: None,
        };
        assert_eq!(q.effective_limit(), SESSION_LIMIT_MAX);

        let q = SessionQuery {
            text: None,
            limit: None,
            with_subagents_only: None,
        };
        assert_eq!(q.effective_limit(), SESSION_LIMIT_DEFAULT);
    }

    /// INV-2 / FR-C-72: 認証情報のフィールドを持たない
    #[test]
    fn ide_workspace_has_no_credential_field() {
        let ws = IdeWorkspace {
            ide_name: Some("VS Code".into()),
            folders: vec!["d:\\proj".into()],
            connected: true,
        };
        let json = serde_json::to_string(&ws).unwrap();
        for bad in ["headers", "token", "secret", "authorization"] {
            assert!(!json.contains(bad), "{bad} を DTO に持ってはいけない");
        }
    }
}
