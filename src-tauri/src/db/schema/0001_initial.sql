-- 0001_initial — 初期スキーマ
--
-- ここに置いてよいのは次の 3 種類だけ (DR-01〜03 / INV-5)。
--   1. ユーザーの意思    : project_scan_folders / project_overrides / settings
--   2. 一覧性のための索引: index_files / sessions / subagent_runs / turn_index / quota_events
--   3. 時系列 (保持期間あり): quota_samples
--
-- 導出データ (スキャン結果 / git 状態 / Copilot 利用状況 / dev ログ / セッション本文)
-- のテーブルを足さないこと。migrations.rs のテストが検査している。

-- ---------------------------------------------------------------- プロジェクト

CREATE TABLE IF NOT EXISTS project_scan_folders (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  path        TEXT    NOT NULL UNIQUE,
  created_at  INTEGER NOT NULL
);

-- 手動調整。path_key をキーにするので、再スキャンしても失われない (FR-P-31)
CREATE TABLE IF NOT EXISTS project_overrides (
  path_key             TEXT PRIMARY KEY,
  display_name         TEXT,
  command_override     TEXT,
  working_dir_override TEXT,
  sort_order           INTEGER,
  hidden               INTEGER NOT NULL DEFAULT 0,
  archived             INTEGER NOT NULL DEFAULT 0,
  updated_at           INTEGER NOT NULL
);

-- ---------------------------------------------------------------- 差分インデックス

-- 進捗簿。差分判定は last_parsed_offset と現在サイズだけで行う。
-- mtime_ms は調査用の参考値であって、判定には使わない (FR-P-56 / FR-C-06)
CREATE TABLE IF NOT EXISTS index_files (
  file_path          TEXT PRIMARY KEY,
  kind               TEXT    NOT NULL,
  session_id         TEXT,
  agent_id           TEXT,
  last_parsed_offset INTEGER NOT NULL DEFAULT 0,
  file_size_at_parse INTEGER NOT NULL DEFAULT 0,
  mtime_ms           INTEGER,
  updated_at         INTEGER NOT NULL
);

-- セッション集計。無期限保持 (FR-C-14)
CREATE TABLE IF NOT EXISTS sessions (
  session_id         TEXT PRIMARY KEY,
  cwd                TEXT,
  path_key           TEXT,
  folder_name        TEXT,
  git_branch         TEXT,
  entrypoint         TEXT,
  title              TEXT,
  model              TEXT,
  started_at         INTEGER,
  last_activity_at   INTEGER,
  turn_count         INTEGER NOT NULL DEFAULT 0,
  input_tokens       INTEGER NOT NULL DEFAULT 0,
  output_tokens      INTEGER NOT NULL DEFAULT 0,
  cache_write_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
  total_nano_aiu     INTEGER NOT NULL DEFAULT 0,
  agent_count        INTEGER NOT NULL DEFAULT 0,
  quota_event_count  INTEGER NOT NULL DEFAULT 0,
  updated_at         INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sessions_last_activity ON sessions(last_activity_at DESC);
CREATE INDEX IF NOT EXISTS idx_sessions_path_key      ON sessions(path_key);

-- サブエージェント実行。
-- run_key は「親側と子側の両方に必ず存在する識別子」でなければならない (FR-C-22)。
-- agent_id は子側にしか無いことがあるので NULL 可。
CREATE TABLE IF NOT EXISTS subagent_runs (
  run_key          TEXT PRIMARY KEY,
  agent_id         TEXT,
  session_id       TEXT NOT NULL,
  parent_file_path TEXT,
  parent_agent_id  TEXT,
  spawn_depth      INTEGER,
  agent_type       TEXT,
  description      TEXT,
  model            TEXT,
  started_at       INTEGER,
  last_activity_at INTEGER,
  ended_at         INTEGER,
  status           TEXT NOT NULL DEFAULT 'running',
  input_tokens     INTEGER NOT NULL DEFAULT 0,
  output_tokens    INTEGER NOT NULL DEFAULT 0,
  tool_call_count  INTEGER NOT NULL DEFAULT 0,
  updated_at       INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_subagent_session ON subagent_runs(session_id);
CREATE INDEX IF NOT EXISTS idx_subagent_parent  ON subagent_runs(parent_agent_id);

-- タイムライン索引。本文は保存しない (FR-C-02 / INV-6)。
-- UNIQUE(file_path, byte_offset) が二重適用の安全装置 (FR-C-07)
CREATE TABLE IF NOT EXISTS turn_index (
  id                 INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id         TEXT,
  agent_id           TEXT,
  file_path          TEXT    NOT NULL,
  byte_offset        INTEGER NOT NULL,
  byte_length        INTEGER NOT NULL,
  record_type        TEXT,
  role               TEXT,
  model              TEXT,
  timestamp_ms       INTEGER,
  uuid               TEXT,
  parent_uuid        TEXT,
  is_sidechain       INTEGER NOT NULL DEFAULT 0,
  is_quota_error     INTEGER NOT NULL DEFAULT 0,
  input_tokens       INTEGER NOT NULL DEFAULT 0,
  output_tokens      INTEGER NOT NULL DEFAULT 0,
  cache_write_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
  nano_aiu           INTEGER NOT NULL DEFAULT 0,
  preview            TEXT,
  UNIQUE(file_path, byte_offset)
);

CREATE INDEX IF NOT EXISTS idx_turn_session ON turn_index(session_id, timestamp_ms);

-- ---------------------------------------------------------------- 利用枠

CREATE TABLE IF NOT EXISTS quota_events (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id  TEXT    NOT NULL,
  occurred_at INTEGER NOT NULL,
  kind        TEXT    NOT NULL,
  reset_text  TEXT,
  raw_message TEXT,
  UNIQUE(session_id, occurred_at)
);

-- 消費率の時系列。保持期間 (30 日) を設ける唯一のテーブル。
-- 間引きを実装するなら書き込みも必ず実装すること (DR-07)。
--
-- received_at (取りに行った時刻) と observed_at (値が変わった時刻) を必ず別に持つ。
-- 鮮度判定に使ってよいのは observed_at だけ (FR-C-86)
CREATE TABLE IF NOT EXISTS quota_samples (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  received_at INTEGER NOT NULL,
  observed_at INTEGER NOT NULL,
  source      TEXT    NOT NULL,
  quota_kind  TEXT    NOT NULL,
  used        REAL,
  entitlement REAL,
  used_pct    REAL,
  reset_at    INTEGER
);

CREATE INDEX IF NOT EXISTS idx_quota_samples_kind ON quota_samples(quota_kind, observed_at DESC);

-- ---------------------------------------------------------------- 設定

-- 汎用 KV。認証トークン・API キーを入れないこと (DR-06 / NFR-31)
CREATE TABLE IF NOT EXISTS settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
