// プロジェクトタブの操作をまとめる場所。
//
// ★ ここは IO を伴うので純粋関数ではない (テスト不要)。
// ★ **進行状態 (スキャン中フラグ / エラー) はページのローカル state に置かず、
//   ここから `appStore` を直接更新する** (FR-C-161)。ヘッダーのボタンと
//   ページ表示時の両方が同じ関数を呼ぶことで、経路を 1 か所に集約する。

import {
  projectsDevStart,
  projectsDevStop,
  projectsDevStopAll,
  projectsOpenAgent,
  projectsOpenBrowser,
  projectsOpenFolder,
  projectsOpenTerminal,
  projectsOpenVscode,
  projectsScan,
  projectsScanFolderAdd,
  projectsScanFolderRemove,
} from '../ipc/commands'
import { describeError } from './format'
import { appStore, patchProjectDev, pushNotice } from '../store/appStore'

/** IR-01: スキャンを実行し、結果を `appStore.projects` に反映する。 */
export async function scanProjects(): Promise<void> {
  appStore.set({ projectsScanning: true })
  try {
    const snapshot = await projectsScan()
    appStore.set({ projects: snapshot })
  } catch (e) {
    pushNotice('error', describeError(e))
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
    pushNotice('error', describeError(e))
  }
}

/**
 * IR-04: dev サーバーの起動 / 停止。
 *
 * 戻り値の `DevState` をその場で反映する — `projects-dev-status` イベント
 * (IR-41) は非同期の状態変化 (URL 検出・終了) を追いかけるためのものであり、
 * ボタンを押した直後の反映をそれ待ちにはしない (IR-32)。
 */
export async function startDevServer(pathKey: string): Promise<void> {
  try {
    const dev = await projectsDevStart(pathKey)
    patchProjectDev(pathKey, dev)
  } catch (e) {
    pushNotice('error', describeError(e))
  }
}

export async function stopDevServer(pathKey: string): Promise<void> {
  try {
    const dev = await projectsDevStop(pathKey)
    patchProjectDev(pathKey, dev)
  } catch (e) {
    pushNotice('error', describeError(e))
  }
}

/** IR-03: 失敗 (存在しない/重複) は通知に出す。 */
export async function addScanFolder(path: string): Promise<void> {
  try {
    const snapshot = await projectsScanFolderAdd(path)
    appStore.set({ projects: snapshot })
  } catch (e) {
    pushNotice('error', describeError(e))
  }
}

export async function removeScanFolder(path: string): Promise<void> {
  try {
    const snapshot = await projectsScanFolderRemove(path)
    appStore.set({ projects: snapshot })
  } catch (e) {
    pushNotice('error', describeError(e))
  }
}

/**
 * 外部ツール起動 (FR-P-70)。投げっぱなしで、完了を待たない (FR-P-71)。
 *
 * 失敗は具体的な対処とともに通知に出す (FR-P-73)。
 */
async function openWith(fn: (path_key: string) => Promise<void>, pathKey: string): Promise<void> {
  try {
    await fn(pathKey)
  } catch (e) {
    pushNotice('error', describeError(e))
  }
}

export const openVscode = (pathKey: string) => openWith(projectsOpenVscode, pathKey)
export const openFolder = (pathKey: string) => openWith(projectsOpenFolder, pathKey)
export const openTerminal = (pathKey: string) => openWith(projectsOpenTerminal, pathKey)
export const openAgent = (pathKey: string) => openWith(projectsOpenAgent, pathKey)

/** FR-P-70: 稼働中かつ URL 検出済みのときだけ呼ばれる想定。 */
export async function openBrowser(url: string): Promise<void> {
  if (url === '') return
  try {
    await projectsOpenBrowser(url)
  } catch (e) {
    pushNotice('error', describeError(e))
  }
}
