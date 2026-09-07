# CLAUDE.md — AI エージェント向け作業規約

このリポジトリは **GitHub Copilot 稼働ダッシュボード + プロジェクト管理アプリ** (Windows 常駐、Tauri v2 + Rust + React) を開発する。

**唯一の正は `docs/requirements.html` (要求仕様書)。** 実装判断に迷ったら要求 ID を引くこと。要求に無いものは作らない。

- ドキュメント一覧: [docs/index.html](docs/index.html)
- 実装順序とタスク: [docs/implementation-plan.html](docs/implementation-plan.html)
- 未決事項 (着手前に実測): [docs/open-questions.html](docs/open-questions.html)
- 要求 ID ↔ 実装/テストの対応: [docs/traceability.html](docs/traceability.html)

---

## 1. 絶対に破ってはいけない不変条件

要求 N-1〜N-10 / NFR-30〜33 由来。**これらに違反する変更は、動いていても差し戻し。**

| # | 不変条件 | 根拠 |
|---|---|---|
| INV-1 | **Copilot 側のデータは読み取り専用。** `~/.copilot/**` と VS Code の `workspaceStorage/**` に書き込まない。既存ファイルを編集しない | N-5 / NFR-32 |
| INV-2 | **認証情報を読まない・持たない・表示しない。** `~/.copilot/config.json`、`mcp-secrets/`、`ide/*.lock` の `headers` は読まない。**DTO にフィールドを定義しない** | N-6 / FR-C-72 / NFR-31 |
| INV-3 | **セッション本文・プロジェクト情報を外部送信しない。** 外向き通信は利用枠の取得のみ | N-7 / NFR-30 |
| INV-4 | **2 秒ポーリング経路にネットワークアクセス・外部プロセス起動を入れない** | FR-C-42 / NFR-03 |
| INV-5 | **導出データを永続化しない。** DB に持ってよいのは「スキャン対象フォルダ」「手動調整」「セッションログの索引・集計」だけ | DR-02 / DR-03 |
| INV-6 | **セッション本文を DB に複製しない。** 索引はパス + オフセット + 長さ + メタ + 140 字プレビューまで | FR-C-02 |
| INV-7 | **推定値を実測値として表示しない。** 出所ラベル (実値/推定/取得不可) を必ず付ける | NFR-40 |
| INV-8 | **`unsafe` / FFI は `src-tauri/src/platform/win_job.rs` だけ。** 他ファイルに書かない | FR-P-68 |
| INV-9 | **git 操作・ビルド・デプロイ・Copilot セッションへの介入を実装しない。** 表示と dev サーバー起動停止のみ | N-1〜N-4 |
| INV-10 | **UI (メイン) スレッドで DB / 外部プロセス / ロック / ネットワークを触らない** | NFR-20 |

## 2. 設計上の必須ルール (事故が起きた実績のある箇所)

| ルール | 内容 |
|---|---|
| IPC 引数名 | **`snake_case` に固定する。** Tauri は既定で `camelCase` に変換するため、`#[tauri::command(rename_all = "snake_case")]` を全コマンドに付ける。型検査でも lint でも検出できない事故クラス (IR-30) |
| IPC 呼び出し | フロントから `invoke()` を直接呼ばない。**必ず `src/ipc/commands.ts` の型付きラッパー経由** (IR-31) |
| 鮮度判定 | 「取得しに行った時刻」ではなく「**値が変化したときだけ更新される観測時刻**」を使う (FR-C-86) |
| mtime | **セッションの新旧判定に mtime を使わない。** 中身のタイムスタンプを読む (FR-P-56)。差分判定はサイズとオフセットで行う (FR-C-06)。mtime を使ってよいのは「追記があったか」「サブエージェントが生きているか」の近似だけ (FR-C-50 / 58) |
| 件数と集合 | **件数を集合と別に算出しない。** 集合を作り、件数はその長さとして導出する (FR-C-51) |
| 末尾断片 | 改行で終わらない最終行は確定分から除外し、オフセットに含めない (FR-C-05) |
| オフセット更新 | レコード挿入と**同一トランザクション内の最後**に行う (FR-C-04) |
| 外部ツール起動 | **終了コードを見ない。** spawn の成否だけで判定する (`explorer.exe` は正常時も 1 を返す) (FR-P-72) |
| 失敗の隔離 | 1 プロジェクト / 1 セッション / 1 取得経路の失敗で全体を落とさない (NFR-24) |
| 進行状態 | 長時間処理の進行状態はページのローカル state に置かない。**タブ横断の共有ストア** (`src/store/appStore.ts`) に置く (FR-C-161) |

## 3. コードの置き場所

```
src/                     フロント (React + TS)。ネットワーク・FS を直接触らない
  ipc/commands.ts        型付き IPC ラッパー。invoke はここだけ
  ipc/events.ts          イベント購読ラッパー
  types/dto.ts           Rust 側 DTO と 1:1 の型。手で同期する (ADR-0006)
  store/appStore.ts      タブ横断の共有状態 (FR-C-161 / 164)
  lib/*.ts               純粋関数。テスト必須
src-tauri/src/
  util/                  純粋関数 (path_key / url 抽出 / 末尾読み)
  db/                    スキーマとマイグレーションのみ
  projects/detect.rs     種別判定 (純粋)。IO を持ち込まない
  projects/*.rs          スキャン / git / dev サーバー / 外部ツール
  copilot/parser.rs      レコードパース (純粋)
  copilot/indexer.rs     差分インデックス
  copilot/live.rs        ライブ監視 (2 秒ポーリングで呼ばれる。INV-4)
  copilot/quota.rs       利用枠取得 (経路 A→B→C の降格)
  platform/win_job.rs    unsafe はここだけ (INV-8)
```

## 4. 純粋関数として切り出すもの (NFR-50、テスト必須)

種別判定 / パス正規化 / 差分判定 / 末尾断片の切り捨て / 木構築 / URL 抽出 / 活動状態の合成 / 利用枠ビューの降格判定。
**実データで見つかった例外パターンは、そのままテストケースにする (NFR-51)。**

## 5. 完了の定義

1. `npm run verify` (typecheck + vitest + cargo test) が通る
2. 変更に対応するユニットテストがある (純粋関数なら必須)
3. **実際にアプリを起動して挙動を確認した (NFR-53)。「テストが通る」で完了としない**
4. 要求 ID を実装に紐付け、`docs/traceability.html` を更新した
5. 未確定の仮定を置いたなら `docs/open-questions.html` に書いた

## 6. やってはいけない進め方

- **OQ (未決事項) を推測で埋めて実装を進めない。** 実データで確認するか、確認できないなら「取得不可」を返す実装にして OQ に残す
- 単価・付与額・枠の名前をコードに埋め込まない。実行時に取得する (FR-C-134)
- 取れない値を「0」や空文字で埋めない。**取得不可は取得不可として表現する** (NFR-43 / 44)
- 図表のために外部チャートライブラリを追加しない (FR-C-163 / NFR-13)
- 依存を増やすときは先に ADR (`docs/decisions.html`) を書く (NFR-10)

## 7. コマンド

```bash
npm run verify      # 型 + フロントテスト + Rust テスト (PR 前に必須)
npm run app:dev     # アプリを起動 (WebView2 必須)
npm run rs:lint     # clippy (警告をエラー扱い)
node tools/probe/copilot-layout.mjs   # ローカル Copilot データの読み取り専用プローブ (OQ 調査)
```
