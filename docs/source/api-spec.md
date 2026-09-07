---
title: IPC API 仕様
lede: UI とコアの境界。要求 8 章 (IR-01..46) の実装形。フロントから `invoke` を書いてよいのは `src/ipc/commands.ts` だけ。
status: ドラフト
version: 0.1
updated: 2026-09-08
---

## 1. 引数名の規約 — 最初に読むこと

> [!注意]
> **すべてのコマンドに `#[tauri::command(rename_all = "snake_case")]` を付ける。**
> Tauri は既定で引数名を `camelCase` に変換する。`path_key` を渡したつもりが `pathKey` を要求されて失敗する事故は、**型検査でも lint でも検出できず、実際に呼ぶまで気づけない** (IR-30)。規約でしか防げない。

```rust
#[tauri::command(rename_all = "snake_case")]
async fn projects_dev_start(
    state: tauri::State<'_, AppState>,
    path_key: String,
    command: Option<String>,
) -> Result<DevStatus, AppError> { ... }
```

```ts
// src/ipc/commands.ts — invoke を書いてよい唯一の場所 (IR-31)
export const projectsDevStart = (path_key: string, command?: string) =>
  invoke<DevStatus>('projects_dev_start', { path_key, command })
```

呼び出し側は `commands.ts` の関数だけを使う。**ページから `invoke()` を直接呼ばない。**

## 2. 変更系コマンドの共通形 (IR-32)

```
バリデーション → 永続化 → スナップショット組み立て → イベント通知と戻り値の両方で返す
```

戻り値だけだと他の画面が更新されず、イベントだけだと呼び出し元が「反映されたか」を判断できない。**両方返す。**

## 3. エラーの形

`Result<T, String>` にしない。型のある `AppError` を定義し、UI が扱える形で返す。

```rust
#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppError {
    NotFound { what: String },
    InvalidInput { field: String, message: String },
    Io { message: String },
    Db { message: String },
    External { tool: String, message: String, hint: Option<String> },  // FR-P-73
    Unavailable { reason: String, how_to_fix: Option<String> },        // FR-C-83
}
```

`hint` / `how_to_fix` は「PATH に無い」「認証が必要」など、**ユーザーが次にやることを書く欄**。埋められないなら `None` にする。適当な文言で埋めない。

## 4. コマンド一覧

### 4.1 プロジェクト機能

| ID | コマンド | 引数 | 戻り値 | 注意 |
|---|---|---|---|---|
| IR-01 | `projects_scan` | — | `ProjectsSnapshot` | **必ず実スキャンしてから返す。**読み出し専用版は作らない |
| IR-02 | `projects_settings_update` | `ProjectOverrideRequest` | `ProjectsSnapshot` | **再スキャンしない。**キャッシュ済み結果と再マージ (FR-P-05) |
| IR-03 | `projects_scan_folder_add` | `path: String` | `ProjectsSnapshot` | 存在確認 + 重複を分かりやすいメッセージに変換 |
| IR-03 | `projects_scan_folder_remove` | `path: String` | `ProjectsSnapshot` | |
| IR-04 | `projects_dev_start` | `path_key`, `command?` | `DevStatus` | **起動可否をバックエンドでも再チェック (FR-P-23)** |
| IR-04 | `projects_dev_stop` | `path_key` | `DevStatus` | 未起動でもエラーにせず「停止中」を返す (冪等。FR-P-66) |
| IR-04 | `projects_dev_stop_all` | — | `ProjectsSnapshot` | UI 側で二段階確認 (FR-P-67) |
| IR-05 | `projects_dev_logs_get` | `path_key` | `Vec<String>` | 最大 500 行。詳細パネルを開いた直後の初期表示用 |
| IR-06 | `projects_open_vscode` | `path_key` | `()` | spawn 成否のみで判定 (FR-P-72) |
| IR-06 | `projects_open_folder` | `path_key` | `()` | 同上。`explorer.exe` は正常時も終了コード 1 |
| IR-06 | `projects_open_terminal` | `path_key` | `()` | Windows Terminal → 失敗したら PowerShell (FR-P-74) |
| IR-06 | `projects_open_agent` | `path_key` | `()` | そのディレクトリで Copilot CLI を起動 |

### 4.2 Copilot ダッシュボード

| ID | コマンド | 引数 | 戻り値 | 注意 |
|---|---|---|---|---|
| IR-10 | `live_status_get` | — | `LiveStatus` | **2 秒ごとに呼ばれる。ネットワーク・外部プロセス起動を含めない (INV-4)** |
| IR-11 | `snapshot_get` | — | `DbSnapshot` | 最終インデックス時刻・各種件数・直近セッション 20 件 |
| IR-12 | `index_refresh` | — | `()` | バックグラウンド起動、即座に返す。多重起動は黙って無視 (FR-C-10) |
| IR-13 | `sessions_list_get` | `SessionQuery` | `Vec<SessionSummary>` | 既定 100 / 上限 1000。入力は UI 側で 250ms デバウンス |
| IR-14 | `session_detail_get` | `session_id` | `Option<SessionDetail>` | **未インデックスは `null` (正常系。エラーにしない)** |
| IR-15 | `turn_body_get` | `turn_id` | `TurnBody` | 都度シーク読み。上限 512KB。範囲外なら再インデックスを案内 |
| IR-16 | `usage_today_get` | — | `UsageToday` | ローカル日 0:00〜現在 |
| IR-17 | `quota_get` | `force: bool` | `QuotaView` | 経路 A→B→C の降格込み。下限間隔あり |
| IR-18 | `quota_source_status_get` | — | `QuotaSourceStatus` | 認証状態 / SDK 有無 / 直近の取得結果と失敗理由 |
| IR-19 | `animation_pref_get` | — | `AnimationPref` | |
| IR-19 | `animation_pref_set` | `pref` | `AnimationPref` | |

## 5. イベント

| ID | イベント | ペイロード | 発火契機 |
|---|---|---|---|
| IR-40 | `projects-snapshot` | `ProjectsSnapshot` | スキャン・設定変更後 |
| IR-41 | `projects-dev-status` | `DevStatus` | dev 状態が変化したとき |
| IR-42 | `projects-dev-log` | `{ path_key, lines: string[] }` | 200〜300ms バッファリング (FR-P-65) |
| IR-43 | `index-progress` | `IndexProgress` | 250〜300ms 間引き (FR-C-11) |
| IR-44 | `snapshot` | `DbSnapshot` | 差分インデックス完了時 |
| IR-45 | `quota` | `QuotaView` | 利用枠の再取得完了時 |
| IR-46 | `window-visibility` | `{ minimized: boolean }` | ネイティブウィンドウの最小化状態が変化したとき |

> [!注意]
> IR-46 は省略できない。**ネイティブウィンドウの最小化は WebView の `visibilitychange` に伝播しない** — 最小化しても `visibilityState` は `visible` のままなので、ブラウザ側のイベントだけに頼ると最小化中もポーリングが回り続ける (FR-C-43)。

## 6. 主要 DTO

TypeScript 側の定義は `src/types/dto.ts`。Rust 側と**手で同期する** (生成器は入れない。理由は ADR-0006)。

```ts
// ---- プロジェクト ----

export type ProjectKind =
  | 'tauri' | 'nextjs' | 'sveltekit' | 'vite' | 'python_package'
  | 'rust' | 'python' | 'notebook' | 'other'

export type DevState =
  | { state: 'stopped' }
  | { state: 'starting' }
  | { state: 'running'; url: string | null; pid: number; started_at: number }
  | { state: 'exited'; code: number | null }
  | { state: 'failed'; reason: string }

export interface GitStatus {
  branch: string | null          // detached HEAD は null (FR-P-41)
  dirty: boolean
  has_remote: boolean
  last_commit_at: number | null
}

export interface CopilotUsage {
  session_count: number          // 「保持されているセッション数」 (NFR-44)
  last_used_at: number | null
  last_title: string | null
  is_active: boolean
  last_nano_aiu: number | null
  lines_added: number | null
  lines_removed: number | null
  matched_by: 'exact' | 'folder_name_fallback'   // FR-P-53: 推測は推測と出す
}

export interface Project {
  path_key: string
  root_path: string
  working_dir: string
  display_name: string
  kind: ProjectKind
  command_candidates: string[]   // 許可リスト順 (FR-P-20)
  resolved_command: string | null
  launch_blocked_reason: string | null   // 起動不可の理由。隠さず出す (FR-P-22)
  hidden: boolean
  archived: boolean
  sort_order: number | null
  git: GitStatus | null          // .git が無ければ null (FR-P-42)
  copilot: CopilotUsage | null   // 履歴が無ければ null。0 で埋めない (FR-P-58)
  dev: DevState
}

export interface ProjectsSnapshot {
  projects: Project[]
  scan_folders: string[]
  scanned_at: number
  using_default_folder: boolean  // 既定フォルダをその場限りで使ったか (FR-P-02)
  warnings: string[]             // 読めなかったフォルダなど (FR-P-03)
}

// ---- Copilot ライブ ----

export type ActivityState =
  | 'generating' | 'tool_running' | 'waiting_input' | 'subagent_running' | 'unknown'

export interface LiveSession {
  session_id: string
  folder_name: string | null
  entrypoint: string
  title: string | null
  activity: ActivityState
  model: string | null
  started_at: number | null
  last_activity_at: number | null
  context_used: number | null    // 使用トークン
  context_limit: number | null
  session_nano_aiu: number | null
  running_subagent_ids: string[] // ★ 集合が正。件数は length で導く (FR-C-51)
}

export interface IdeWorkspace {
  ide_name: string | null
  folders: string[]
  connected: boolean             // PID が死んでいても行を捨てず false にする (FR-C-71)
  // ★ 認証トークンのフィールドは定義しない (FR-C-72 / INV-2)
}

export interface LiveStatus {
  sessions: LiveSession[]
  ide_workspaces: IdeWorkspace[]
  running_session_count: number
  running_subagent_count: number // 集合の合計から導出する
  polled_at: number
}

// ---- 利用枠 ----

export type QuotaSource =
  | { source: 'actual'; via: 'sdk' | 'rest'; observed_at: number }
  | { source: 'estimated'; basis: string; observed_at: number }   // FR-C-82
  | { source: 'unavailable'; reason: string; how_to_fix: string | null }

export interface QuotaGauge {
  kind: string                   // 'monthly_credits' | 'quota:<name>' | 'context_window'
  label: string
  used: number | null
  entitlement: number | null     // -1 = 無制限 (FR-C-131)
  used_pct: number | null        // 100 でクランプしない (FR-C-90)
  overage: number | null         // 付与超過分。別建てで出す (FR-C-90)
  unit: 'credits' | 'requests' | 'tokens'
  currency_amount: string | null // 例 "$6.20 / $10.00" (FR-C-89)
  reset_at: number | null
  origin: QuotaSource            // 枠ごとに独立 (FR-C-84)
}

export interface QuotaView {
  gauges: QuotaGauge[]
  fetched_at: number
  note: string                   // 「金額は概算。請求額の正は GitHub の課金画面」(NFR-45)
}
```

## 7. UI 側の呼び分け

| 契機 | 呼ぶもの |
|---|---|
| Copilot タブを開いた | `snapshot_get` → 即描画 / `quota_get(false)` / `usage_today_get` / `index_refresh` |
| Copilot タブ表示中 2 秒ごと | `live_status_get` **のみ** |
| 更新ボタン | `index_refresh` + `quota_get(true)` |
| 未取り込みの追記を検知 (下限 10 秒) | `index_refresh` (バッジは出さない。FR-C-60) |
| 長周期タイマー (5 分以上、画面表示中のみ) | `quota_get(false)` |
| プロジェクトタブを開いた | キャッシュを即描画 → `projects_scan` |
| プロジェクトの手動調整を保存 | `projects_settings_update` (再スキャンしない) |

> [!注意]
> **`usage_today_get` を 2 秒ポーリングに載せない (FR-C-103)。** 更新はタブ表示時と差分インデックス完了時だけ。
