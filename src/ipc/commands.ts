// 型付き IPC ラッパー。
//
// ★ **`invoke` を書いてよいのはこのファイルだけ** (IR-31)。
//
// 引数名は Rust 側と揃えて **snake_case のまま**渡す。Rust 側の全コマンドに
// `#[tauri::command(rename_all = "snake_case")]` が付いているのが前提 (IR-30)。
// このずれは型検査でも lint でも検出できず、実際に呼ぶまで気づけない。

import { invoke } from '@tauri-apps/api/core'

import type {
  AnimationPref,
  DbSnapshot,
  DevState,
  LiveStatus,
  ProjectOverrideRequest,
  ProjectsSnapshot,
  QuotaGauge,
  QuotaSourceStatus,
  SessionQuery,
  SessionSummary,
  TurnBody,
  UsageToday,
} from '../types/dto'

// ---------------------------------------------------------------- プロジェクト

/** IR-01: **必ず実スキャンしてから返る。**読み出し専用版は無い */
export const projectsScan = () => invoke<ProjectsSnapshot>('projects_scan')

/** IR-02: 再スキャンせず、キャッシュ済み結果と再マージする */
export const projectsSettingsUpdate = (req: ProjectOverrideRequest) =>
  invoke<ProjectsSnapshot>('projects_settings_update', { req })

export const projectsScanFolderAdd = (path: string) =>
  invoke<ProjectsSnapshot>('projects_scan_folder_add', { path })

export const projectsScanFolderRemove = (path: string) =>
  invoke<ProjectsSnapshot>('projects_scan_folder_remove', { path })

/** IR-04: 起動。起動可否はバックエンドでも再チェックされる (FR-P-23) */
export const projectsDevStart = (path_key: string, command?: string) =>
  invoke<DevState>('projects_dev_start', { path_key, command })

/** IR-04: 停止。未起動でもエラーにならず「停止中」が返る (FR-P-66) */
export const projectsDevStop = (path_key: string) =>
  invoke<DevState>('projects_dev_stop', { path_key })

/** IR-04: すべて停止。**呼ぶ前に二段階確認する** (FR-P-67) */
export const projectsDevStopAll = () => invoke<ProjectsSnapshot>('projects_dev_stop_all')

/** IR-05: ログ最大 500 行 */
export const projectsDevLogsGet = (path_key: string) =>
  invoke<string[]>('projects_dev_logs_get', { path_key })

// IR-06: 外部ツール。投げっぱなしで起動し、完了を待たない (FR-P-71)
export const projectsOpenVscode = (path_key: string) =>
  invoke<void>('projects_open_vscode', { path_key })

export const projectsOpenFolder = (path_key: string) =>
  invoke<void>('projects_open_folder', { path_key })

export const projectsOpenTerminal = (path_key: string) =>
  invoke<void>('projects_open_terminal', { path_key })

export const projectsOpenAgent = (path_key: string) =>
  invoke<void>('projects_open_agent', { path_key })

// ---------------------------------------------------------------- Copilot

/**
 * IR-10: ライブ状況。
 *
 * **2 秒ごとに呼ぶのはこれだけ。**他のコマンドをポーリング経路に混ぜない
 * (INV-4 / NFR-03 / FR-C-103)。
 */
export const liveStatusGet = () => invoke<LiveStatus>('live_status_get')

/** IR-11: DB 由来のダイジェスト。タブを開いた瞬間の即描画に使う */
export const snapshotGet = () => invoke<DbSnapshot>('snapshot_get')

/** IR-12: バックグラウンド起動。即座に返る。実行中の再要求は黙って無視される */
export const indexRefresh = () => invoke<void>('index_refresh')

/** IR-13: 既定 100 / 上限 1000。入力は呼び出し側で 250ms デバウンスする */
export const sessionsListGet = (query: SessionQuery) =>
  invoke<SessionSummary[]>('sessions_list_get', { query })

/**
 * IR-14: セッション詳細。
 *
 * **未インデックスは `null` が返る (正常系)。**エラーとして扱わず、
 * 「まだ記録がありません」と表示して 1 回だけ `indexRefresh()` を呼ぶ (FR-C-57)。
 */
export const sessionDetailGet = (session_id: string) =>
  invoke<unknown | null>('session_detail_get', { session_id })

/** IR-15: 本文を 1 レコードだけシーク読み。上限 512KB */
export const turnBodyGet = (turn_id: number) => invoke<TurnBody>('turn_body_get', { turn_id })

/** IR-16: **2 秒ポーリングに載せない** (FR-C-103) */
export const usageTodayGet = () => invoke<UsageToday>('usage_today_get')

/** IR-17: 利用枠。ネットワークを伴うので下限間隔がある。**2 秒経路に載せない** */
export const quotaGet = (force = false) => invoke<QuotaGauge[]>('quota_get', { force })

/** IR-18: 各取得経路の可用性。「何をすれば取れるか」の材料 */
export const quotaSourceStatusGet = () =>
  invoke<QuotaSourceStatus>('quota_source_status_get')

export const animationPrefGet = () => invoke<AnimationPref>('animation_pref_get')

export const animationPrefSet = (pref: AnimationPref) =>
  invoke<AnimationPref>('animation_pref_set', { pref })
