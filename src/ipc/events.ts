// イベント購読ラッパー (IR-40〜46)。
//
// ★ **`listen` を書いてよいのはこのファイルだけ。**
// すべての関数は購読解除関数を返す。**必ず呼ぶこと** — 購読が積み上がると
// 2 秒ポーリングが多重発火する。

import { listen, type UnlistenFn } from '@tauri-apps/api/event'

import type {
  DbSnapshot,
  DevState,
  IndexProgress,
  ProjectsSnapshot,
  QuotaGauge,
} from '../types/dto'

type Handler<T> = (payload: T) => void

const on = <T>(event: string, handler: Handler<T>): Promise<UnlistenFn> =>
  listen<T>(event, (e) => handler(e.payload))

/** IR-40: スキャン・設定変更後 */
export const onProjectsSnapshot = (h: Handler<ProjectsSnapshot>) =>
  on<ProjectsSnapshot>('projects-snapshot', h)

/** IR-41: dev 状態が変化したとき */
export const onDevStatus = (h: Handler<{ path_key: string; state: DevState }>) =>
  on('projects-dev-status', h)

/** IR-42: ログ行。バックエンドで 200〜300ms バッファされて届く (FR-P-65) */
export const onDevLog = (h: Handler<{ path_key: string; lines: string[] }>) =>
  on('projects-dev-log', h)

/** IR-43: 差分インデックスの進捗。250〜300ms に間引かれて届く (FR-C-11) */
export const onIndexProgress = (h: Handler<IndexProgress>) =>
  on<IndexProgress>('index-progress', h)

/** IR-44: 差分インデックス完了時 */
export const onSnapshot = (h: Handler<DbSnapshot>) => on<DbSnapshot>('snapshot', h)

/** IR-45: 利用枠の再取得完了時 */
export const onQuota = (h: Handler<QuotaGauge[]>) => on<QuotaGauge[]>('quota', h)

/**
 * IR-46: ネイティブウィンドウの最小化状態が変化したとき (FR-C-43)。
 *
 * **この経路は省略できない。** WebView の `visibilitychange` は最小化で発火せず、
 * 最小化しても `visibilityState` は `visible` のままになる。
 */
export const onWindowVisibility = (h: Handler<{ minimized: boolean }>) =>
  on<{ minimized: boolean }>('window-visibility', h)
