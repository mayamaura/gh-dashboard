// Rust 側 DTO と 1:1 の型定義。
//
// **フィールド名は Rust の JSON 表現に合わせて snake_case のまま** にする
// (docs/coding-standards.html 4 節)。シリアライズ境界で名前を変換しないことで、
// 「どちらの名前で来るか」を考える場面を消す。
//
// 生成器は使わず手で同期する (ADR-0006)。DTO が 50 個を超えたら再検討。

// ---------------------------------------------------------------- 共通

/** IPC 境界のエラー。Rust の `AppError` と対応する。 */
export type AppError =
  | { kind: 'not_found'; what: string }
  | { kind: 'invalid_input'; field: string; message: string }
  | { kind: 'io'; message: string }
  | { kind: 'db'; message: string }
  | { kind: 'external'; tool: string; message: string; hint: string | null }
  | { kind: 'unavailable'; reason: string; how_to_fix: string | null }

// ---------------------------------------------------------------- プロジェクト

export type ProjectKind =
  | 'tauri'
  | 'nextjs'
  | 'sveltekit'
  | 'vite'
  | 'python_package'
  | 'rust'
  | 'python'
  | 'notebook'
  | 'other'

/** dev サーバーの 5 状態 (FR-P-60) */
export type DevState =
  | { state: 'stopped' }
  | { state: 'starting' }
  | { state: 'running'; url: string | null; pid: number; started_at: number }
  | { state: 'exited'; code: number | null }
  | { state: 'failed'; reason: string }

export interface GitStatus {
  /** detached HEAD は null (FR-P-41) */
  branch: string | null
  dirty: boolean
  has_remote: boolean
  last_commit_at: number | null
}

/** 履歴との照合方法。**推測は推測として出す** (FR-P-53 / NFR-41) */
export type MatchedBy = 'exact' | 'folder_name_fallback'

/** 履歴が一切ないプロジェクトでは `null` になる。**0 で埋めない** (FR-P-58) */
export interface CopilotUsage {
  /** 「保持されているセッション数」。累計ではない (NFR-44) */
  session_count: number
  last_used_at: number | null
  last_title: string | null
  is_active: boolean
  last_nano_aiu: number | null
  lines_added: number | null
  lines_removed: number | null
  matched_by: MatchedBy
}

/** 保存済みの手動調整そのもの (FR-P-30)。フォームの初期値に使う */
export interface ProjectOverride {
  display_name: string | null
  command_override: string | null
  working_dir_override: string | null
  sort_order: number | null
  hidden: boolean
  archived: boolean
}

export interface Project {
  path_key: string
  /** リポジトリルート。git の判定対象 (FR-P-44) */
  root_path: string
  /** 起動コマンドを実行するディレクトリ (FR-P-12) */
  working_dir: string
  display_name: string
  kind: ProjectKind
  kind_label: string
  command_candidates: string[]
  resolved_command: string | null
  /** 起動不可の理由。隠さず表示する (FR-P-22) */
  launch_blocked_reason: string | null
  hidden: boolean
  archived: boolean
  sort_order: number | null
  /** 保存済みの手動調整。無ければ null。**表示用の値と混同しない** */
  override_values: ProjectOverride | null
  git: GitStatus | null
  copilot: CopilotUsage | null
  dev: DevState
}

export interface ProjectsSnapshot {
  projects: Project[]
  scan_folders: string[]
  scanned_at: number
  /** 既定フォルダをその場限りで使ったか。DB には書いていない (FR-P-02) */
  using_default_folder: boolean
  /** 読めなかったフォルダなど。**無言で欠落させない** (FR-P-03 / NFR-43) */
  warnings: string[]
}

export interface ProjectOverrideRequest {
  path_key: string
  display_name?: string | null
  command_override?: string | null
  working_dir_override?: string | null
  sort_order?: number | null
  hidden?: boolean | null
  archived?: boolean | null
}

// ---------------------------------------------------------------- Copilot ライブ

export type Entrypoint =
  | 'cli_interactive'
  | 'cli_background'
  | 'vscode'
  | 'coding_agent'
  | 'unknown'

/**
 * 活動状態の 5 値 (FR-C-44)。
 *
 * `tool_running` は「長時間のツール実行中」と「ツール許可プロンプトで停止中」の
 * **両方**を表す。両者はログ上まったく同じ形でしか現れないため、UI でも
 * 断定しない (FR-C-46 / NFR-42)。
 */
export type ActivityState =
  | 'generating'
  | 'tool_running'
  | 'waiting_input'
  | 'subagent_running'
  | 'unknown'

export interface LiveSession {
  session_id: string
  folder_name: string | null
  entrypoint: Entrypoint
  title: string | null
  activity: ActivityState
  model: string | null
  started_at: number | null
  last_activity_at: number | null
  context_used: number | null
  context_limit: number | null
  session_nano_aiu: number | null
  /** **集合が正。バッジの件数は length で導く** (FR-C-51) */
  running_subagent_ids: string[]
}

/**
 * IDE ワークスペース (FR-C-70〜72)。
 *
 * 状態ファイル (`~/.copilot/ide/*.lock`) には `headers` があり認証情報を
 * 含みうるが、**この型にそのフィールドを定義しない** (INV-2 / FR-C-72)。
 */
export interface IdeWorkspace {
  ide_name: string | null
  folders: string[]
  /** PID が死んでいても行を捨てず false にする (FR-C-71) */
  connected: boolean
}

export interface LiveStatus {
  sessions: LiveSession[]
  ide_workspaces: IdeWorkspace[]
  running_session_count: number
  running_subagent_count: number
  polled_at: number
  /** 最新ログの mtime。自動インデックス起動の判定にのみ使う (FR-C-58) */
  newest_log_mtime_ms: number | null
}

// ---------------------------------------------------------------- インデックス

export interface IndexProgress {
  phase: string
  total_files: number
  done_files: number
  current_file: string | null
  records_ingested: number
  /** 読み飛ばした壊れた行。**無言で欠落させない** (NFR-43) */
  skipped_lines: number
}

export interface SessionSummary {
  session_id: string
  folder_name: string | null
  title: string | null
  cwd: string | null
  entrypoint: Entrypoint
  started_at: number | null
  last_activity_at: number | null
  turn_count: number
  total_nano_aiu: number
  agent_count: number
}

export interface DbSnapshot {
  last_indexed_at: number | null
  /** 「保持されているセッション数」(NFR-44) */
  retained_session_count: number
  subagent_run_count: number
  turn_count: number
  recent_sessions: SessionSummary[]
}

export interface SessionQuery {
  text?: string | null
  limit?: number | null
  with_subagents_only?: boolean | null
}

export interface TurnBody {
  turn_id: number
  body: string
  truncated: boolean
}

/**
 * IR-16 (FR-C-100〜105)。
 *
 * **集計の基準が 2 つある** (実データの制約):
 * - `input_tokens` / `output_tokens` / `cache_read_tokens` / `total_nano_aiu` は
 *   **本日活動のあったセッションの集計を丸ごと**計上する (日をまたぐセッションは
 *   分割できない。`tokenDetails` はセッションに 1 組しか無いため)
 * - `hourly_tokens` だけは `turn_index` のレコード単位 = **実質は出力トークンのみ**
 *
 * **合計とグラフの縦軸は一致しない。**UI に注記すること (NFR-40)。
 */
export interface UsageToday {
  input_tokens: number
  output_tokens: number
  cache_read_tokens: number
  /** 本日分の索引レコード件数 */
  turn_count: number
  /** **本日活動のあったセッション数。**「今稼働中」ではない (そちらは LiveStatus) */
  session_count: number
  /** 本日起動したサブエージェント実行の件数。同上 */
  subagent_count: number
  total_nano_aiu: number
  /** 時間帯別 (入出力のみ。キャッシュは含めない。FR-C-101)。24 要素固定 */
  hourly_tokens: number[]
  /** フォルダ別の上位 5 件 (FR-C-102)。`[フォルダ名, 入出力トークン合計]` */
  top_folders: Array<[string, number]>
  /** 合成モデル (`auto`) として除外したレコード件数 (FR-C-104 / NFR-43) */
  excluded_records: number
}

// ------------------------------------------------- セッション詳細 (IR-14)

/**
 * モデル別内訳の出所 (INV-7)。**どちらかで「取れる項目」が違う。**
 *
 * - `shutdown_metrics`: `session.shutdown.data.modelMetrics` 由来。トークン 4 種 +
 *   クレジットが揃う実値
 * - `turn_index`: 索引を model で畳んだもの。**出力トークンと件数しか無い。**
 *   他は `null` になる — 0 として描かないこと
 */
export type ModelUsageSource = 'shutdown_metrics' | 'turn_index'

/** モデル別のトークン・クレジット内訳 1 行 (FR-C-105)。`null` = 取得不可 */
export interface ModelUsage {
  /** 実モデル名。合成モデル (`auto`) は除外済み (FR-C-104) */
  model: string
  /** キャッシュを含まない純粋な入力。`usage.inputTokens` ではない (ADR-0029) */
  input_tokens: number | null
  output_tokens: number | null
  cache_read_tokens: number | null
  cache_write_tokens: number | null
  nano_aiu: number | null
  /** `nano_aiu / 10^9`。除数を TS 側に持たないため Rust 側で割ってある */
  credits: number | null
  /** `turn_index` 由来のときのレコード件数 */
  record_count: number | null
  source: ModelUsageSource
}

/**
 * 系統図 1 ノード = ガント 1 行 (FR-C-112〜118)。
 *
 * **系統図とガントはこの同じ配列を使う。**別々に集計しないこと (FR-C-114)。
 * 配列はすでに表示順 (親のすぐ下に子) で並んでいる。
 */
export interface SubagentNode {
  /** `toolCallId`。親側と子側の両方に存在する識別子 (FR-C-22) */
  run_key: string
  /** `null` = セッション直下 */
  parent_key: string | null
  /** 親子関係から導いた深さ。生データの深さは使っていない (FR-C-114) */
  depth: number
  /** 親が見つからずルート直下に置かれた (FR-C-113)。UI で「推測」と示す */
  orphaned: boolean
  child_keys: string[]
  agent_id: string | null
  agent_type: string | null
  description: string | null
  model: string | null
  /** DB の状態列。**現状は全行 `running`** (遷移は OQ-11 待ち) */
  status: string
  started_at: number | null
  last_activity_at: number | null
  /** **稼働中 / 未確定は `null`。**現在時刻で描く (FR-C-118) */
  ended_at: number | null
  tool_call_count: number
  /** ライブ集合に居るか (FR-C-115)。非稼働セッションでは常に false */
  running: boolean
  /** 今この行を表示すべきか。非稼働セッションでは全行 true (FR-C-116 / 117) */
  visible: boolean
}

/** ガントの横軸 (FR-C-118)。`end_at: null` = 稼働中 → 現在時刻で描く */
export interface GanttWindow {
  start_at: number | null
  end_at: number | null
}

/** ガントに重ねる利用枠到達マーカー (FR-C-118)。**0 件は「無かった」** */
export interface QuotaEventMark {
  occurred_at: number
  /** `credit_exhausted` / `rate_limit` / `session_limit` / `unknown` */
  kind: string
  reset_text: string | null
}

/** 本文タイムラインの 1 行 (FR-C-119)。**本文は入らない** (INV-6) */
export interface TurnMeta {
  /** `turnBodyGet(turn_id)` に渡す id */
  turn_id: number
  timestamp_ms: number | null
  record_type: string | null
  role: string | null
  model: string | null
  /** サブエージェント配下のレコードにだけ付く。`SubagentNode.run_key` と同じ値 */
  agent_id: string | null
  is_sidechain: boolean
  /** 140 字プレビュー (FR-C-02) */
  preview: string | null
  output_tokens: number
  /** 元レコードのバイト長。512KB 超は本文が切り詰められる (FR-C-120) */
  byte_length: number
}

/** IR-14 の戻り値 (FR-C-112)。**未インデックスは `null`** (正常系) */
export interface SessionDetail {
  session: SessionSummary
  /** `total_nano_aiu / 10^9` (FR-C-89 の金額併記用) */
  credits: number
  /** モデル別内訳。**空配列 = 内訳が取れなかった。**消費 0 ではない (NFR-43) */
  models: ModelUsage[]
  /** 系統図 = ガントの行。表示順 (FR-C-114) */
  subagents: SubagentNode[]
  /** このセッションが今稼働中か。false なら subagents は全行 visible */
  is_live: boolean
  gantt: GanttWindow
  /** 縦マーカー。0 件でも空配列 (FR-C-118) */
  quota_events: QuotaEventMark[]
  timeline: TurnMeta[]
  timeline_offset: number
  /** **上限 1000 でキャップ済みの総件数** (FR-C-119) */
  timeline_total: number
  /** 「もっと見る」で次に渡すオフセット。`null` = これ以上無い */
  timeline_next_offset: number | null
}

// ---------------------------------------------------------------- 利用枠

/**
 * 値の出所 (FR-C-81)。**3 状態を型で強制する。**
 *
 * 取れない値を 0 で埋めた瞬間、そのゲージは嘘になる (NFR-40 / INV-7)。
 * `unavailable` はエラーではなく正常な状態のひとつ。
 */
export type QuotaSource =
  | { source: 'actual'; via: 'sdk' | 'rest'; observed_at: number }
  | { source: 'estimated'; basis: string; observed_at: number }
  | { source: 'unavailable'; reason: string; how_to_fix: string | null }

export interface QuotaGauge {
  kind: string
  label: string
  used: number | null
  /** -1 = 無制限 (FR-C-131) */
  entitlement: number | null
  /** **100 でクランプしない** (FR-C-90) */
  used_pct: number | null
  /** 付与超過分。別建てで出す (FR-C-90) */
  overage: number | null
  unlimited: boolean
  reset_at: number | null
  origin: QuotaSource
  /** false ならこのプランにそもそも存在しない枠 (FR-C-131 / ADR-0015)。既定 true */
  has_quota: boolean
}

export type AnimationPref = 'auto' | 'on' | 'off'

/** 経路ごとの可用性。`how_to_fix` は FR-C-83 の「何をすれば取れるようになるか」 */
export interface QuotaRouteStatus {
  available: boolean
  reason: string
  how_to_fix: string | null
}

export interface QuotaSourceStatus {
  sdk: QuotaRouteStatus
  /** 経路 B は v1 では未実装。常に `available: false` (ADR-0018) */
  rest: QuotaRouteStatus
  estimate: QuotaRouteStatus
  /** 直近に `quota_get` が走った時刻。**鮮度には使わない** — 鮮度は observed_at (FR-C-86) */
  checked_at: number | null
}
