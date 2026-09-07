---
title: データモデル
lede: SQLite に何を置き、何を置かないか。要求 7 章 (DR-01..07) の実装形。DDL はここが正で、`src-tauri/src/db/migrations.rs` はこの写し。
status: ドラフト
version: 0.1
updated: 2026-09-08
---

## 1. 置く / 置かない

| | 対象 |
|---|---|
| **置く** | スキャン対象フォルダ、手動調整、セッションログの索引・集計、利用枠の時系列 (任意)、汎用設定 |
| **置かない** | プロジェクトスキャン結果、種別判定、git 状態、Copilot 利用状況、dev サーバーのログ、**セッション本文**、認証情報 |

置かない理由は 2 つに集約される。

1. **導出データを持つと同期ずれの世話が永続的に発生する** (実体が消えても DB に残る / リネームで別物になる) — DR-02 / FR-P-04
2. **数百 MB の本文のうち読み返されるのは開いた 1 レコードだけ** — 全文複製は使われないデータのために帯域を倍払う — DR-03 / FR-C-02

例外は「一覧性のための索引」だけ。数百ファイルを毎回全走査してはセッション一覧すら描けないため、索引だけは持つ。

## 2. 接続とマイグレーション

| 項目 | 方針 |
|---|---|
| ファイル | `%LOCALAPPDATA%\gh-dashboard\app.db` |
| ジャーナル | `PRAGMA journal_mode = WAL` |
| 待ち | `PRAGMA busy_timeout = 5000` |
| 外部キー | `PRAGMA foreign_keys = ON` |
| マイグレーション | `user_version` を段数として使い、上げるだけの前進マイグレーション (DR-04) |
| バックアップ | 破壊的変更を含む段の適用前に `app.db.bak-<version>` を残す (DR-04) |
| 他アプリの DB | `session-store.db` 等を読む場合は**読み取り専用オープン、またはコピーしてから読む**。書き込みロックを取らない (DR-05 / OQ-05) |

> [!注意]
> **保持期間を設けたテーブルを作るなら、書き込みも必ず実装する (DR-07)。** 間引き処理だけが動く未使用テーブルを残さない。現時点で保持期間を持つのは `quota_samples` のみ。

## 3. スキーマ

### 3.1 プロジェクト機能

このタブで永続化するのは次の 2 つだけ。

```sql
CREATE TABLE project_scan_folders (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  path        TEXT    NOT NULL UNIQUE,
  created_at  INTEGER NOT NULL          -- unix ms
);

CREATE TABLE project_overrides (
  path_key            TEXT PRIMARY KEY,  -- 正規化済みパス (util::path_key)
  display_name        TEXT,
  command_override    TEXT,
  working_dir_override TEXT,
  sort_order          INTEGER,
  hidden              INTEGER NOT NULL DEFAULT 0,
  archived            INTEGER NOT NULL DEFAULT 0,
  updated_at          INTEGER NOT NULL
);
```

- `path_key` は「小文字化 + 区切りを `\` に統一 + 末尾区切り除去」。**大文字小文字を保った生パスをキーにしない**
- 再スキャンしても手動調整が失われないのは、キーがパスであってスキャン結果の行 id ではないから (FR-P-31)
- `working_dir_override` は**保存時に実在検証**して、通らなければ書き込まない (FR-P-32)
- 既定フォルダ (`%USERPROFILE%\Documents\Projects`) を**自動登録しない**。登録が空のときだけ、その場限りの対象として使う (FR-P-02)

### 3.2 差分インデックスの進捗簿

```sql
CREATE TABLE index_files (
  file_path           TEXT PRIMARY KEY,
  kind                TEXT NOT NULL,     -- 'main' | 'subagent'
  session_id          TEXT,
  agent_id            TEXT,
  last_parsed_offset  INTEGER NOT NULL DEFAULT 0,
  file_size_at_parse  INTEGER NOT NULL DEFAULT 0,
  mtime_ms            INTEGER,           -- 参考値。差分判定には使わない
  updated_at          INTEGER NOT NULL
);
```

差分判定は `last_parsed_offset` と現在のファイルサイズだけで行う。

| 状況 | 判定 |
|---|---|
| `現在サイズ > last_parsed_offset` | 継続 (追記分のみパース) |
| `現在サイズ < file_size_at_parse` | **全再パース** (truncate / 入れ替わり。FR-C-06) |
| `現在サイズ == last_parsed_offset` | 更新なし |

> [!注意]
> `mtime_ms` を差分判定に使ってはいけない。PC 移行やバックアップ復元で**複数ファイルが同一 mtime になる**ことが実際にある (FR-P-56)。この列は調査用の参考値として持つだけ。

### 3.3 セッション集計

```sql
CREATE TABLE sessions (
  session_id         TEXT PRIMARY KEY,
  cwd                TEXT,
  path_key           TEXT,               -- cwd の正規化。プロジェクト紐付け用
  folder_name        TEXT,
  git_branch         TEXT,
  entrypoint         TEXT,               -- 'cli_interactive'|'cli_background'|'vscode'|'coding_agent'|'unknown'
  title              TEXT,
  model              TEXT,
  started_at         INTEGER,
  last_activity_at   INTEGER,
  turn_count         INTEGER NOT NULL DEFAULT 0,
  input_tokens       INTEGER NOT NULL DEFAULT 0,
  output_tokens      INTEGER NOT NULL DEFAULT 0,
  cache_write_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
  total_nano_aiu     INTEGER NOT NULL DEFAULT 0,   -- 10^9 で割ると AI Credit
  agent_count        INTEGER NOT NULL DEFAULT 0,
  quota_event_count  INTEGER NOT NULL DEFAULT 0,
  updated_at         INTEGER NOT NULL
);
CREATE INDEX idx_sessions_last_activity ON sessions(last_activity_at DESC);
CREATE INDEX idx_sessions_path_key      ON sessions(path_key);
```

**無期限保持 (FR-C-14)。** 集計行は数百件規模なので放置してよい。

`total_nano_aiu` は整数で持つ。**浮動小数で持つと合算で誤差が出て、金額表示が請求額と食い違って見える。**

### 3.4 サブエージェント実行

```sql
CREATE TABLE subagent_runs (
  run_key           TEXT PRIMARY KEY,   -- 親側と子側の両方に必ず存在する識別子 (FR-C-22)
  agent_id          TEXT,               -- 子側にしか無いことがある。NULL 可
  session_id        TEXT NOT NULL,
  parent_file_path  TEXT,
  parent_agent_id   TEXT,               -- NULL = セッション直下
  spawn_depth       INTEGER,            -- 参考値。木構築では信頼しない (FR-C-114)
  agent_type        TEXT,
  description       TEXT,
  model             TEXT,
  started_at        INTEGER,
  last_activity_at  INTEGER,
  ended_at          INTEGER,
  status            TEXT NOT NULL,      -- 'running' | 'completed' | 'declined'
  input_tokens      INTEGER NOT NULL DEFAULT 0,
  output_tokens     INTEGER NOT NULL DEFAULT 0,
  tool_call_count   INTEGER NOT NULL DEFAULT 0,
  updated_at        INTEGER NOT NULL
);
CREATE INDEX idx_subagent_session ON subagent_runs(session_id);
CREATE INDEX idx_subagent_parent  ON subagent_runs(parent_agent_id);
```

> [!注意]
> **主キーは親側と子側の両方に存在する識別子でなければならない (FR-C-22)。** 片側にしか無い ID を主キーにすると、(a) 拒否されたケースでキーを作れず、(b) 親の行と子のファイルが別々のインデックス実行に分かれたとき親子関係を復元できず、系統図がフラットに退行する。

`status` の 3 値は混同してはいけない。

| 値 | 意味 |
|---|---|
| `running` | 起動して、まだ完了通知が来ていない。**「起動しました」という暫定応答を完了として扱わない (FR-C-24)** |
| `completed` | 本当の完了通知が届いた。完了通知は**単一のソースからのみ**拾う (FR-C-25) |
| `declined` | 親が呼び出しを**拒否した**。実際に起動して失敗した `completed` とは別物 (FR-C-27) |

`spawn_depth` は欠落しうるので、系統図もガントも `parent_agent_id` を辿って作った木を使う (FR-C-114)。

### 3.5 タイムライン索引

```sql
CREATE TABLE turn_index (
  id                 INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id         TEXT,
  agent_id           TEXT,
  file_path          TEXT    NOT NULL,
  byte_offset        INTEGER NOT NULL,
  byte_length        INTEGER NOT NULL,
  record_type        TEXT,
  role               TEXT,
  model              TEXT,
  timestamp_ms       INTEGER,            -- NULL 可
  uuid               TEXT,
  parent_uuid        TEXT,
  is_sidechain       INTEGER NOT NULL DEFAULT 0,
  is_quota_error     INTEGER NOT NULL DEFAULT 0,
  input_tokens       INTEGER NOT NULL DEFAULT 0,
  output_tokens      INTEGER NOT NULL DEFAULT 0,
  cache_write_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
  nano_aiu           INTEGER NOT NULL DEFAULT 0,
  preview            TEXT,               -- 先頭 140 字程度
  UNIQUE(file_path, byte_offset)
);
CREATE INDEX idx_turn_session ON turn_index(session_id, timestamp_ms);
```

**`UNIQUE(file_path, byte_offset)` が二重適用の安全装置 (FR-C-07)。** クラッシュ後の再開でも `INSERT OR IGNORE` が重複を吸収するので、冪等性がスキーマで保証される。

本文はここに入れない。ビューアを開いた瞬間に `file_path` + `byte_offset` + `byte_length` でシーク読みする (FR-C-02 / IR-15)。

### 3.6 利用枠

```sql
CREATE TABLE quota_events (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id   TEXT NOT NULL,
  occurred_at  INTEGER NOT NULL,
  kind         TEXT NOT NULL,   -- 'credit_exhausted'|'rate_limit'|'session_limit'|'unknown'
  reset_text   TEXT,
  raw_message  TEXT,
  UNIQUE(session_id, occurred_at)
);

CREATE TABLE quota_samples (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  received_at  INTEGER NOT NULL,   -- 取得しに行った時刻。鮮度判定に使わない
  observed_at  INTEGER NOT NULL,   -- 値が変化したときだけ更新される観測時刻。★ 鮮度はこちら
  source       TEXT NOT NULL,      -- 'sdk' | 'rest' | 'estimated'
  quota_kind   TEXT NOT NULL,
  used         REAL,
  entitlement  REAL,               -- -1 = 無制限
  used_pct     REAL,
  reset_at     INTEGER
);
CREATE INDEX idx_quota_samples_kind ON quota_samples(quota_kind, observed_at DESC);
```

> [!注意]
> `received_at` と `observed_at` を必ず別に持つ (FR-C-86)。定期取得は値の更新と無関係に走るため、取得時刻を鮮度に使うと**期限切れの古い値が「最新」として選ばれる**。値と観測時刻はペアで引き継ぐ。

`quota_samples` は**保持期間 30 日で間引く唯一のテーブル**。間引き処理を書くなら書き込みも必ず実装する (DR-07)。

`entitlement = -1` は「無制限」。率を計算せず「無制限」と表示する。**0 除算やマイナス率を出さない (FR-C-131)。**

### 3.7 設定

```sql
CREATE TABLE settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
```

用途: アニメーション設定 (`auto` / `on` / `off`)、最後に開いたタブ、アイドルセッションの表示トグル、など。

**認証トークン・API キーをここに入れない (DR-06 / NFR-31)。** 認証は OS の資格情報ストアまたは既存の GitHub CLI 資格情報を参照する。

## 4. 保持と間引き

| テーブル | 保持 | 間引き |
|---|---|---|
| `sessions` / `subagent_runs` / `quota_events` | 無期限 (FR-C-14) | しない |
| `turn_index` | 無期限 | しない (索引本体) |
| `index_files` | 無期限 | 元ファイルが消えた行は次回インデックスで掃除してよい |
| `quota_samples` | 30 日 | 起動時 + 1 日 1 回 |
| `project_scan_folders` / `project_overrides` | 無期限 | しない (ユーザーの意思) |

## 5. 取れない値の表現

| 状況 | 表現 | やってはいけないこと |
|---|---|---|
| 履歴が一切ないプロジェクト | `copilot: null` | 0 や空文字で埋める (FR-P-58) |
| 未インデックスのセッション | `null` を返す (正常系) | エラーを返す (FR-C-57 / IR-14) |
| 利用枠が取れない | `source = unavailable` + 理由 | 0% として描く (FR-C-83) |
| エージェントが古いセッションを自動削除する場合 | 「保持されているセッション数」と表記 | 「累計セッション数」と称する (NFR-44) |

**取れない値を 0 で埋めた瞬間、そのゲージは嘘になる。** DTO では `Option` / `null` を潰さないこと。
