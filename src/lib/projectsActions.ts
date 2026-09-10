// プロジェクトタブの操作をまとめる場所。
//
// ★ ここは IO を伴うので純粋関数ではない (テスト不要)。
// ★ **進行状態 (スキャン中フラグ / エラー) はページのローカル state に置かず、
//   ここから `appStore` を直接更新する** (FR-C-161)。ヘッダーのボタンと
//   ページ表示時の両方が同じ関数を呼ぶことで、経路を 1 か所に集約する。

import {
  projectsDevStopAll,
  projectsOpenAgent,
  projectsOpenFolder,
  projectsOpenTerminal,
  projectsOpenVscode,
  projectsScan,
  projectsScanFolderAdd,
  projectsScanFolderRemove,
} from '../ipc/commands'
import { describeError } from './format'
import { appStore } from '../store/appStore'

/** IR-01: スキャンを実行し、結果を `appStore.projects` に反映する。 */
export async function scanProjects(): Promise<void> {
  appStore.set({ projectsScanning: true, projectsError: null })
  try {
    const snapshot = await projectsScan()
    appStore.set({ projects: snapshot })
  } catch (e) {
    appStore.set({ projectsError: describeError(e) })
  } finally {
    appStore.set({ projectsScanning: false })
  }
}

/** FR-P-67: 呼び出し側で二段階確認を済ませてから呼ぶこと。 */
export async function stopAllDevServers(): Promise<void> {
  try {
    const snapshot = await projectsDevStopAll()
    appStore.set({ projects: snapshot })
  } catch (e) {
    appStore.set({ projectsError: describeError(e) })
  }
}

/** IR-03: 失敗 (存在しない/重複) は `projectsError` に出す。 */
export async function addScanFolder(path: string): Promise<void> {
  try {
    const snapshot = await projectsScanFolderAdd(path)
    appStore.set({ projects: snapshot, projectsError: null })
  } catch (e) {
    appStore.set({ projectsError: describeError(e) })
  }
}

export async function removeScanFolder(path: string): Promise<void> {
  try {
    const snapshot = await projectsScanFolderRemove(path)
    appStore.set({ projects: snapshot, projectsError: null })
  } catch (e) {
    appStore.set({ projectsError: describeError(e) })
  }
}

/**
 * 外部ツール起動 (FR-P-70)。投げっぱなしで、完了を待たない (FR-P-71)。
 *
 * 段階 3 までは `unavailable` が返る。失敗は具体的な対処とともに
 * `projectsError` に出す (FR-P-73)。
 */
async function openWith(fn: (path_key: string) => Promise<void>, pathKey: string): Promise<void> {
  try {
    await fn(pathKey)
  } catch (e) {
    appStore.set({ projectsError: describeError(e) })
  }
}

export const openVscode = (pathKey: string) => openWith(projectsOpenVscode, pathKey)
export const openFolder = (pathKey: string) => openWith(projectsOpenFolder, pathKey)
export const openTerminal = (pathKey: string) => openWith(projectsOpenTerminal, pathKey)
export const openAgent = (pathKey: string) => openWith(projectsOpenAgent, pathKey)

/** ブラウザで開く。段階 3 まで対応コマンドが無いため、常に「未実装」を出す。 */
export function openBrowser(url: string): void {
  // FR-P-70: 稼働中かつ URL 検出済みのときだけ呼ばれる想定。専用 IPC コマンドは
  // まだ無いため (段階 3)、`window.open` は使わず未実装として明示する。
  // 外部への navigation はブラウザ既定のシステムブラウザ起動と同義になり、
  // バックエンド経由の spawn 成否判定 (FR-P-72) を経ないため保留する。
  void url
  appStore.set({ projectsError: '未実装です (T-3.x) — ブラウザで開く機能は段階 3 で実装します' })
}
