---
title: 要求トレーサビリティ
lede: 要求 ID → 実装モジュール → テスト の対応表。タスクを完了したらここを埋める。空欄は「まだ実装されていない」という意味であって、「対応不要」ではない。
status: 進行中
version: 0.1
updated: 2026-09-08
---

## 使い方

- 実装したら「実装」列にファイルを、「テスト」列にテスト名を書く
- **要求を実装しないと決めたら、空欄のままにせず「見送り (理由)」と書く**
- 新しい要求 ID が増えたら行を足す。**削除した ID は再利用しない** (要求 0.1)

凡例: **済** = 実装 + テストあり / **部分** = 一部のみ / **保留** = OQ 待ち / 空欄 = 未着手

---

## プロジェクト機能 (FR-P)

| 要求 | 内容 | 実装 | テスト | 状態 |
|---|---|---|---|---|
| FR-P-01 | 直下 1 階層のみ走査 | `projects/scan.rs` | | |
| FR-P-02 | 既定フォルダを DB に書かない | `projects/scan.rs` | | |
| FR-P-03 | 読めないフォルダはスキップ | `projects/scan.rs` | | |
| FR-P-04 | スキャン結果を永続化しない | `state.rs` | | |
| FR-P-05 | プロセス内キャッシュ | `state.rs` | | |
| FR-P-06 | UI スレッドを塞がない | `projects/commands.rs` | | |
| FR-P-10〜14 | 種別判定 (純粋) | `projects/detect.rs` | `detect::tests` | 済 |
| FR-P-20〜23 | 起動コマンドの解決 | `projects/detect.rs` | `detect::tests::command_*` | 部分 |
| FR-P-30〜33 | 手動調整 | `projects/overrides.rs` | | |
| FR-P-40〜47 | git 状態 | `projects/git.rs` | | |
| FR-P-50〜58 | Copilot 利用状況の紐付け | `projects/copilot_link.rs` | `tools/probe/vscode-sessions.mjs` | |
| FR-P-51 | workspace.yaml 経由のパース | | | ADR-0003 |
| FR-P-56 | タイムスタンプの読み取り | | | ADR-0014 |
| FR-P-60〜68 | dev サーバー | `projects/dev_server.rs`, `platform/win_job.rs` | | |
| FR-P-63 | URL 自動検出 (純粋) | `util/url_detect.rs` | `url_detect::tests` | 済 |
| FR-P-50〜58 (OQ-03 検証) | Copilot セッション紐付けの実装基盤 | | `tools/probe/vscode-sessions.mjs` | |
| FR-P-70〜72 (OQ-04 検証) | IDE 稼働判定 | | `tools/probe/live-detect.mjs` | |
| FR-P-70〜74 | 外部ツール連携 | `projects/external.rs` | | |
| FR-P-80〜88 | 一覧 UI | `pages/ProjectsPage.tsx` | `lib/projectList.test.ts` | |

## Copilot ダッシュボード (FR-C)

| 要求 | 内容 | 実装 | テスト | 状態 |
|---|---|---|---|---|
| FR-C-01〜02 | 全期間 / 本文を複製しない | `copilot/indexer.rs` | `tools/probe/jsonl-shape.mjs` | ADR-0003 / ADR-0013 |
| FR-C-02〜08 | セッション記録の形式と変分 | | `tools/probe/jsonl-shape.mjs` | |
| FR-C-03/05/06 | 差分判定・末尾断片 (純粋) | `copilot/delta.rs` | `delta::tests` | 済 |
| FR-C-04 | オフセット更新はトランザクション最後 | `copilot/indexer.rs` | | |
| FR-C-07 | UNIQUE で二重適用を防ぐ | `db/migrations.rs` | | |
| FR-C-08〜09 | ストリーミング / バッチコミット | `copilot/indexer.rs` | | |
| FR-C-10〜11 | バックグラウンド / 進捗通知 | `copilot/indexer.rs` | | |
| FR-C-12〜13 | 防御的パース | `copilot/record.rs` | `tools/probe/record-kinds.mjs` | |
| FR-C-14 | 無期限保持 | `db/migrations.rs` | | |
| FR-C-20 | セッション集計 | `copilot/aggregate.rs` | `tools/probe/record-kinds.mjs` | |
| FR-C-21〜29 | サブエージェント系統 | `copilot/aggregate.rs` | | ADR-0016 |
| FR-C-40〜43 | ライブ監視の起動と停止 | `copilot/live.rs`, `hooks/useLivePoll.ts` | | ADR-0014 |
| FR-C-44〜46 | 活動状態の合成 (純粋) | `copilot/activity.rs` | `activity::tests` | 済 |
| FR-C-47〜49 | 末尾シーク読みとキャッシュ | `copilot/live.rs`, `util/tail.rs` | | |
| FR-C-50〜52 | 稼働中サブエージェント集合 | `copilot/live.rs` | | |
| FR-C-53〜57 | セッションカード | `components/SessionCard.tsx` | | |
| FR-C-58〜60 | 自動インデックスの発火 | `pages/CopilotPage.tsx` | | |
| FR-C-61 | 稼働サマリー | `components/LiveSummary.tsx` | | |
| FR-C-70〜72 | IDE ワークスペース | `copilot/live.rs` | `tools/probe/live-detect.mjs` | ADR-0014 |
| FR-C-80〜95 | 利用枠ゲージ (純粋部分) | `copilot/quota.rs` | `quota::tests` | 部分 |
| FR-C-85 | リセット日判定に `quota_reset_date_utc` を使う | | | ADR-0015 / ADR-0017 |
| FR-C-86 | 鮮度判定と警告色 | | | ADR-0015 |
| FR-C-89 | 超過表示 | | | ADR-0010 |
| FR-C-100〜105 | 本日の使用状況 | `copilot/aggregate.rs` | | |
| FR-C-110〜111 | セッション検索 | `copilot/commands.rs` | | |
| FR-C-112〜118 | 系統図・ガント (木構築は純粋) | `copilot/tree.rs` | `tree::tests` | 部分 |
| FR-C-119〜121 | 本文ビューア | `components/TurnViewer.tsx` | | |
| FR-C-130 | 経路 A: クォータ枠の取得 | | `tools/probe/quota-sdk.mjs` | ADR-0015 / ADR-0017 |
| FR-C-131 | 適用外の枠は `has_quota === false` で判定 | | | ADR-0015 |
| FR-C-134 | モデル別単価 | | | ADR-0010 / ADR-0015 |
| FR-C-138 | 組織の月次消費履歴 | | | 取得不可 / 未検証 (ADR-0018) |
| FR-C-139 | 個人の月次消費履歴 | | | 取得不可 / 未検証 (ADR-0018) |
| FR-C-140 | Enterprise 組織の月次消費 | | | 取得不可 / 未検証 (ADR-0018) |
| FR-C-160〜164 | 表示・設定 | `App.tsx`, `store/appStore.ts` | | |

## データ要求 (DR)

| 要求 | 内容 | 実装 | テスト | 状態 |
|---|---|---|---|---|
| DR-01 | 単一ローカル DB | `db/mod.rs` | | 済 |
| DR-02 | 導出データを永続化しない | (スキーマに存在しないこと) | `migrations::tests::no_derived_tables` | 済 |
| DR-03 | 例外は索引のみ | `db/migrations.rs` | | 済 |
| DR-04 | バージョン付きマイグレーション | `db/migrations.rs` | `migrations::tests` | 部分 |
| DR-05 | 他アプリの DB は読み取り専用 | | | ADR-0013 |
| DR-06 | トークンを DB に保存しない | (スキーマに存在しないこと) | | 済 |
| OQ-02 / FR-C-02〜08 | バイトオフセット方式 (1 レコード 1 行・改行コード・BOM) | | `tools/probe/jsonl-shape.mjs` | |
| OQ-01 / OQ-07 | レコード種別と構造 | | `tools/probe/record-kinds.mjs` | |
| OQ-04 / FR-C-70〜72 | IDE 稼働判定経路 | | `tools/probe/live-detect.mjs` | |
| OQ-03 / FR-P-50〜58 | Copilot セッション紐付け | | `tools/probe/vscode-sessions.mjs` | |
| OQ-06 / OQ-08 / FR-C-130〜137 | 利用枠 API | | `tools/probe/quota-sdk.mjs` | |
| OQ-12 / OQ-06 / FR-C-138〜140 | 経路 B: 課金 API | | `tools/probe/quota-rest.mjs` | |
| OQ-10 | Copilot ディレクトリ構成 | | `tools/probe/copilot-layout.mjs` | |
| DR-07 | 保持期間を設けるなら書き込みも実装 | | | |

## インタフェース要求 (IR)

| 要求 | 内容 | 実装 | テスト | 状態 |
|---|---|---|---|---|
| IR-01〜06 | プロジェクト系コマンド | `projects/commands.rs`, `ipc/commands.ts` | | |
| IR-10〜19 | Copilot 系コマンド | `copilot/commands.rs`, `ipc/commands.ts` | | |
| IR-30 | 引数名のケース固定 | 全コマンドの `rename_all` | `tests/ipc_naming.rs` | |
| IR-31 | 型付きラッパーに集約 | `ipc/commands.ts` | | 部分 |
| IR-32 | 変更系の統一パターン | `projects/commands.rs` | | |
| IR-40〜46 | イベント | `ipc/events.ts` | | 部分 |

## 非機能要求 (NFR)

| 要求 | 内容 | どう確かめるか | 状態 |
|---|---|---|---|
| NFR-01 | 初回インデックス 5 秒以内 | 実データで計測 (段階 4 の完了条件) | |
| NFR-02 | 2 回目 1 秒未満・新規 0 件 | 同上 + DB テスト | |
| NFR-03 | 2 秒ポーリングのコスト | コードレビュー (INV-4) + 実測 | |
| NFR-04 | インデックス中 CPU 15% 以下 | 実測 | |
| NFR-05 | アニメーション時 CPU +2pt 以内 | 実測 | |
| NFR-06 | スキャンが待たされない | 実機確認 | |
| NFR-07 | 利用枠取得が操作をブロックしない | 実機確認 (ネットワーク切断) | |
| NFR-10 | 常駐の軽さ | ADR-0001 で判断済み。依存追加時に再確認 | 済 |
| NFR-11 | 常駐ポーリングループを作らない | コードレビュー | 済 (ADR-0004) |
| NFR-12 | 見ていないときのコストをゼロに | 実機確認 (段階 5) | |
| NFR-20 | UI スレッドで重い処理をしない | コードレビュー | |
| NFR-21 | UI 応答性の watchdog | T-X.2 | |
| NFR-22 | ログのローテーション | T-X.1 | |
| NFR-23 | 防御的な外部データ読み取り | `copilot/record.rs` | |
| NFR-24 | 1 件の失敗が全体を落とさない | 各所 + テスト | |
| NFR-25 | デバッグシンボル | T-X.4 | |
| NFR-30〜33 | セキュリティ / プライバシー | **INV-1〜3 としてレビュー項目化** | 済 (規約) |
| NFR-40〜45 | 表示の正直さ | UI 仕様 6 節の文言ルール + レビュー | 部分 |
| NFR-50 | 純粋関数として切り出す 8 つ | 下表 | 部分 |
| NFR-51 | 実データの例外をテストに | テスト戦略 7 節 | 進行中 |
| NFR-52 | 読み取り専用プローブ | `tools/probe/` | 部分 |
| NFR-53 | 実機確認で完了とする | テスト戦略 5 節のチェックリスト | 済 (規約) |
| NFR-54 | 合成イベントを使わない | コードレビュー | 済 (規約) |

### NFR-50 が名指しする 8 つ

| ロジック | 実装 | テスト | 状態 |
|---|---|---|---|
| 種別判定 | `projects/detect.rs` | `detect::tests` | 済 |
| パス正規化 | `util/path_key.rs` | `path_key::tests` | 済 |
| 差分判定 | `copilot/delta.rs` | `delta::tests` | 済 |
| 末尾断片の切り捨て | `copilot/delta.rs` | `delta::tests` | 済 |
| 木構築 | `copilot/tree.rs` | `tree::tests` | 済 |
| URL 抽出 | `util/url_detect.rs` | `url_detect::tests` | 済 |
| 活動状態の合成 | `copilot/activity.rs` | `activity::tests` | 済 |
| 利用枠ビューの降格判定 | `copilot/quota.rs` | `quota::tests` | 済 |

## 不変条件 (INV)

CLAUDE.md の INV-1〜10。**コードレビューの必須確認項目**であり、自動テストで完全には担保できない。

| ID | 内容 | 担保の仕方 |
|---|---|---|
| INV-1 | Copilot 側に書き込まない | レビュー + プローブが読み取り専用であることの確認 |
| INV-2 | 認証情報を読まない・DTO に定義しない | **型定義に `headers` / `token` 等が無いことをレビューで確認** |
| INV-3 | 内容を外部送信しない | レビュー (ネットワーク呼び出しは `quota.rs` のみ) |
| INV-4 | 2 秒経路にネットワーク・外部プロセスを入れない | レビュー + `live.rs` の import 制限 |
| INV-5 | 導出データを永続化しない | スキーマのテスト (`no_derived_tables`) |
| INV-6 | セッション本文を DB に複製しない | 同上 |
| INV-7 | 推定を実測として表示しない | `QuotaSource` 型が 3 状態を強制 |
| INV-8 | `unsafe` は 1 ファイルのみ | `#![forbid(unsafe_code)]` + `#[allow]` の位置 |
| INV-9 | git 操作等を実装しない | レビュー |
| INV-10 | UI スレッドで重い処理をしない | レビュー |
