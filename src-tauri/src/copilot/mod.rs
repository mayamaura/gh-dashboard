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
pub mod indexer;
pub mod live;
pub mod parser;
pub mod quota;
pub mod quota_fetch;
pub mod sessions;
pub mod store;
pub mod tree;
pub mod usage;

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
    /// **全セッション** (稼働 / 非稼働を問わず) の `events.jsonl` mtime の最大値。
    ///
    /// 自動インデックスの発火判定はこの値だけで行う。**固定間隔で更新される値を
    /// 混ぜない** — 混ぜるとアイドル時も回り続ける (FR-C-58 / FR-C-59)。
    /// 1 件も読めなければ `None`。**0 で埋めない** (NFR-43)
    pub newest_log_mtime_ms: Option<i64>,
    pub polled_at: i64,
}

impl LiveStatus {
    /// 集合から件数を導出して組み立てる (FR-C-51)。
    ///
    /// 「バッジは稼働中なのに系統図は空」という矛盾が出ようがない形にする。
    pub fn new(
        sessions: Vec<LiveSession>,
        ide_workspaces: Vec<IdeWorkspace>,
        newest_log_mtime_ms: Option<i64>,
        now: i64,
    ) -> Self {
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
            newest_log_mtime_ms,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

// ---------------------------------------------------------------- IR-14 (T-7.5〜7.10)

/// モデル別内訳の出所 (FR-C-81 の考え方を内訳にも適用する / INV-7)。
///
/// **どちらの出所かで「取れる項目」が違う。**フロントは必ずこれを見て
/// 「—」と「0」を描き分けること。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelUsageSource {
    /// `session.shutdown.data.modelMetrics` 由来。**トークン 4 種 + クレジットが揃う実値**
    ShutdownMetrics,
    /// `turn_index` を `model` で畳んだもの。**出力トークンと件数しか無い** —
    /// レコード単位に載るトークンは `assistant.message.outputTokens` だけだから
    /// (実測 1,041 レコード)。入力・キャッシュ・クレジットは `None` になる
    TurnIndex,
}

/// モデル別のトークン・クレジット内訳 1 行 (FR-C-105 / FR-C-112)。
///
/// **取れなかった項目は `None`。0 で埋めない** (NFR-43 / INV-7)。
/// 単価はモデルごとに違うので、トークン数とクレジットの両方を出す (FR-C-105)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelUsage {
    /// 実モデル名 (`gpt-5-mini` / `claude-haiku-4.5` 等)。
    /// **合成モデル (`auto`) はここに来ない** — 集計前に除外する (FR-C-104)
    pub model: String,
    /// キャッシュを含まない純粋な入力 (`tokenDetails.input.tokenCount`)。
    /// `usage.inputTokens` ではない — あちらはキャッシュ分を含む上位集計で、
    /// 足すと二重加算になる (FR-C-104 / ADR-0029)
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    /// このモデルの消費 (nano AIU)。`ShutdownMetrics` 由来のときだけ入る
    pub nano_aiu: Option<i64>,
    /// `nano_aiu / 10^9`。**除数を TS 側に複製しないためにここで割る**
    pub credits: Option<f64>,
    /// `TurnIndex` 由来のときの索引レコード件数。`ShutdownMetrics` では `None`
    pub record_count: Option<i64>,
    pub source: ModelUsageSource,
}

/// 系統図 1 ノード = ガント 1 行 (FR-C-112〜118)。
///
/// **系統図とガントは同じこの配列を使う。**別々に集計しないこと (FR-C-114)。
/// 配列はすでに表示順 (親のすぐ下に子) で並んでいる。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentNode {
    /// `subagent_runs.run_key` = `toolCallId`。**親側と子側の両方に存在する識別子**
    /// (FR-C-22 / ADR-0016)。ライブ集合の要素と直接照合できる
    pub run_key: String,
    /// `None` = セッション直下
    pub parent_key: Option<String>,
    /// 親子関係から導いた深さ。生データの `spawn_depth` は使わない (FR-C-114)
    pub depth: usize,
    /// 親が見つからずルート直下に置かれた (FR-C-113)。UI で「推測で置いた」と示す
    pub orphaned: bool,
    pub child_keys: Vec<String>,
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    pub description: Option<String>,
    pub model: Option<String>,
    /// DB の状態列。**現状は全行 `running`** — 完了/拒否への遷移は OQ-11 待ちで
    /// 保留中 (T-4.9 / T-4.10 / ADR-0016)。ライブ集合と照合できない行の
    /// フォールバックにだけ使う (FR-C-115)
    pub status: String,
    pub started_at: Option<i64>,
    pub last_activity_at: Option<i64>,
    /// **稼働中 / 未確定は `None`。**フロントは現在時刻で描く (FR-C-118)
    pub ended_at: Option<i64>,
    pub tool_call_count: i64,
    /// **ライブ集合に居るか** (FR-C-115)。非稼働セッションでは常に `false`
    pub running: bool,
    /// 今この行を表示すべきか (FR-C-116 / 117)。
    ///
    /// - セッションが稼働中: `tree::visible_for_live` の結果 (完了済みを畳み、
    ///   稼働中の祖先は畳まない)
    /// - 非稼働 (振り返り): **全行 `true`** = 全件表示
    pub visible: bool,
}

/// ガントの横軸 (FR-C-118)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GanttWindow {
    pub start_at: Option<i64>,
    /// **稼働中なら `None`。**フロントが現在時刻を使う (FR-C-118)
    pub end_at: Option<i64>,
}

/// ガントに重ねる利用枠到達マーカー (FR-C-118)。
///
/// **0 件は「無かった」。取得不可ではない** — 空配列で返す。
/// 実データでは現状 0 件 (レコード種別が未観測 / OQ-01)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaEventMark {
    pub occurred_at: i64,
    /// `credit_exhausted` / `rate_limit` / `session_limit` / `unknown`
    pub kind: String,
    pub reset_text: Option<String>,
}

/// 本文タイムラインの 1 行 (FR-C-119)。**本文は入らない** (INV-6)。
///
/// 行クリック時に `turn_id` を `turn_body_get` (IR-15) に渡して 1 レコードだけ
/// シーク読みする。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnMeta {
    /// `turn_index.id`。`turn_body_get` の引数
    pub turn_id: i64,
    pub timestamp_ms: Option<i64>,
    pub record_type: Option<String>,
    pub role: Option<String>,
    pub model: Option<String>,
    /// サブエージェント配下のレコードにだけ付く。`SubagentNode::run_key` と
    /// 同じ値なので、系統図の選択でタイムラインを絞り込める (FR-C-121)
    pub agent_id: Option<String>,
    pub is_sidechain: bool,
    /// 140 字プレビュー (FR-C-02)
    pub preview: Option<String>,
    pub output_tokens: i64,
    /// 元レコードのバイト長。512KB を超えると本文が切り詰められる (FR-C-120)
    pub byte_length: i64,
}

/// タイムラインの 1 ページ件数 (FR-C-119)。
pub const TIMELINE_PAGE_SIZE: i64 = 150;
/// タイムラインの総件数上限 (FR-C-119)。
pub const TIMELINE_MAX: i64 = 1000;

/// IR-14 の戻り値 (FR-C-112)。**未インデックスは `None`** (正常系 / FR-C-57)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionDetail {
    /// タイトル・基本情報。一覧行 (IR-11 / IR-13) と同じ型
    pub session: SessionSummary,
    /// `session.total_nano_aiu / 10^9` (FR-C-89 の金額併記用)
    pub credits: f64,
    /// モデル別内訳 (FR-C-105)。**空配列 = 内訳が取れなかった。**
    /// 消費が 0 だったという意味ではない (NFR-43)
    pub models: Vec<ModelUsage>,
    /// 系統図 = ガントの行。表示順に並ぶ (FR-C-114)
    pub subagents: Vec<SubagentNode>,
    /// このセッションが**今この瞬間**稼働中か (段階 5 の判定 / ADR-0014)。
    /// `false` なら `subagents` は全行 `visible: true` (全件表示 / FR-C-117)
    pub is_live: bool,
    pub gantt: GanttWindow,
    /// 縦マーカー。**0 件でも空配列** (FR-C-118)
    pub quota_events: Vec<QuotaEventMark>,
    /// 本文タイムラインの 1 ページ (FR-C-119)
    pub timeline: Vec<TurnMeta>,
    /// このページの先頭位置
    pub timeline_offset: i64,
    /// **上限 1000 でキャップ済みの総件数** (FR-C-119)。進捗表示に使う
    pub timeline_total: i64,
    /// 「もっと見る」で次に渡すオフセット。`None` = これ以上無い
    pub timeline_next_offset: Option<i64>,
}

/// IR-16 の戻り値 (FR-C-100〜105)。
///
/// # 集計の基準が 2 つあること (実データの制約)
///
/// レコード単位に載るトークンは `assistant.message.outputTokens` だけで、
/// 入力・キャッシュは `session.shutdown.tokenDetails` にしか無い (実測 1,041
/// レコード)。そのため:
///
/// - `input_tokens` / `output_tokens` / `cache_read_tokens` / `total_nano_aiu`
///   は**本日活動のあったセッションの集計を丸ごと**計上する (日をまたぐ
///   セッションは分割できない)
/// - `hourly_tokens` だけは `turn_index` のレコード単位 (= 実質出力のみ)
///
/// **合計と時間帯別グラフの縦軸は一致しない。**UI に注記すること (NFR-40)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageToday {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    /// `turn_index` の本日分レコード件数。`sessions.turn_count` と同じ数え方
    pub turn_count: i64,
    /// **本日活動のあったセッション数。**「今この瞬間稼働中」の件数ではない
    /// (そちらは `LiveStatus::running_session_count` / FR-C-61)
    pub session_count: i64,
    /// 本日起動したサブエージェント実行の件数。同上
    pub subagent_count: i64,
    pub total_nano_aiu: i64,
    /// 時間帯別 (入出力のみ。キャッシュは含めない。FR-C-101)。
    /// ローカル日 0:00 起点の 24 要素。**レコード単位なので実質は出力トークン**
    pub hourly_tokens: Vec<i64>,
    /// フォルダ別の上位 5 件 (FR-C-102)。セッション集計の入出力トークン合計
    pub top_folders: Vec<(String, i64)>,
    /// 合成モデル (`auto`) として集計から除外したレコード件数 (FR-C-104)。
    /// **無言で欠落させない** (NFR-43)
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
        let status = LiveStatus::new(vec![session(&["a", "b"]), session(&["c"])], vec![], None, 0);
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
