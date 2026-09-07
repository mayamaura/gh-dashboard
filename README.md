# gh-dashboard

**GitHub Copilot 稼働ダッシュボード / プロジェクトダッシュボード**

GitHub Copilot (Copilot CLI / VS Code の Copilot Chat・エージェントモード) の稼働状況と利用枠の消費を可視化し、あわせてローカルの開発プロジェクトを一覧・起動できる、Windows 常駐型のデスクトップアプリ。

- **状態**: 設計完了 / 実装は段階 0 (実データ・API 調査) から
- **要求仕様**: [docs/requirements.html](docs/requirements.html) (版 1.0 / 2026-09-03)
- **ドキュメント入口**: [docs/index.html](docs/index.html)
- **AI エージェント向け作業規約**: [CLAUDE.md](CLAUDE.md)

## できること (計画)

| タブ | 機能 |
|---|---|
| **Copilot** | セッションの差分インデックス、ライブ稼働監視、利用枠 (AI Credits / クォータ / コンテキスト) のゲージ、本日の使用状況、系統図・ガント・本文ビューアでの振り返り |
| **プロジェクト** | ローカルプロジェクトの自動スキャンと一覧、種別判定、git 状態、Copilot 利用状況の紐付け、dev サーバーの起動/停止/ログ/URL 検出、外部ツール連携 |

## 技術スタック

| 層 | 採用 | 理由 |
|---|---|---|
| シェル | Tauri v2 (WebView2) | 常駐アプリとして軽量 (NFR-10)。ランタイム同梱なし |
| コア | Rust | 差分パース・プロセスツリー管理・Job Object の要求に合う |
| UI | React 19 + TypeScript + Vite | |
| 永続化 | SQLite (rusqlite, bundled) | 単一ローカル DB (DR-01) |
| 図表 | 自前 SVG 描画 | 外部チャートライブラリを増やさない (FR-C-163) |

判断の記録は [docs/decisions.html](docs/decisions.html) (ADR) を参照。

## 開発

### 必要なもの

- Windows 11
- Node.js 20+ / npm
- Rust (stable) + MSVC ビルドツール
- WebView2 ランタイム (Windows 11 は標準搭載)

### セットアップ

```bash
npm install
npm run verify        # 型 + フロントテスト + Rust テスト
npm run app:dev       # アプリ起動
```

### スクリプト

| コマンド | 内容 |
|---|---|
| `npm run verify` | 型検査 + vitest + cargo test。**PR 前に必須** |
| `npm run app:dev` | Tauri 開発起動 |
| `npm run app:build` | リリースビルド |
| `npm run rs:lint` | clippy (警告をエラー扱い) |
| `npm run test:watch` | フロントのテストを監視実行 |
| `node tools/probe/copilot-layout.mjs` | ローカル Copilot データの**読み取り専用**プローブ (段階 0 の調査用) |

## リポジトリ構成

```
docs/          ドキュメント一式 (HTML)。docs/source/ に元の Markdown
src/           フロント (React + TypeScript)
src-tauri/     コア (Rust + Tauri)
tools/probe/   実データ調査用の読み取り専用スクリプト
```

## 方針として「やらないこと」

セッションの再開・強制終了、プロジェクトの作成・削除・移動、ビルド/テスト/デプロイの実行、git 操作、Copilot 設定ファイルの書き換え、認証情報の読み取り、内容の外部送信、GitHub 側の課金設定変更、LLM による要約生成。

詳細は要求仕様 2.2 節、および [CLAUDE.md](CLAUDE.md) の不変条件 INV-1〜INV-10。

## ライセンス / 配布

個人利用を前提とし、配布・マルチユーザー対応・組織展開は目的としない (要求 1.2)。
