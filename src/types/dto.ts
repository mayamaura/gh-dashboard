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

export interface UsageToday {
  input_tokens: number
  output_tokens: number
  cache_read_tokens: number
  turn_count: number
  session_count: number
  subagent_count: number
  total_nano_aiu: number
  /** 時間帯別 (入出力のみ。キャッシュは含めない。FR-C-101) */
  hourly_tokens: number[]
  /** フォルダ別の上位 5 件 (FR-C-102) */
  top_folders: Array<[string, number]>
  excluded_records: number
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
}

export type AnimationPref = 'auto' | 'on' | 'off'

export interface QuotaSourceStatus {
  sdk: { available: boolean; reason: string }
  rest: { available: boolean; reason: string }
  estimate: { available: boolean; reason: string }
}
