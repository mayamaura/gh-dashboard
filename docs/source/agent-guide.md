---
title: AI エージェント作業ガイド
lede: このリポジトリで作業を始める AI エージェント (と人間) 向けの手引き。リポジトリ内の [CLAUDE.md](../CLAUDE.md) と同じ内容を、背景つきで説明したもの。
status: ドラフト
version: 0.1
updated: 2026-09-08
---

## 1. 最初の 5 分

1. [要求仕様](requirements.html) の 0〜2 章を読む (何を作り、**何を作らないか**)
2. `CLAUDE.md` の不変条件 INV-1〜10 を読む
3. [実装計画](implementation-plan.html) で「未着手」の最も上のタスクを取る
4. [未決事項](open-questions.html) で、そのタスクが OQ に依存していないか確認する
5. 着手する

## 2. この仕様書の性質

要求仕様書は珍しく**「なぜ」が全部書いてある**。表の「根拠」列と引用ブロックには、実際に壊れた経験から来た理由が入っている。

```
FR-C-24  バックグラウンド起動の「起動しただけ」の暫定応答を完了として扱わない
  理由 → 暫定応答を完了とみなすと、数分動いた実行が「1 秒」と記録される
```

**この理由を読まずに実装すると、たいてい要求どおりに書いたつもりで壊れる。**
要求 ID を見たら、まず本文の根拠を読むこと。

## 3. やってはいけない進め方

| やらない | なぜ |
|---|---|
| **OQ を推測で埋めて先に進む** | OQ-02 と OQ-06 は設計の分水嶺。推測が外れると書いたコードがまるごと無駄になる |
| 取れない値を 0 / 空文字で埋める | その瞬間、ゲージも一覧も嘘になる (NFR-40) |
| 「テストが通った」で完了とする | NFR-53 が明示的に禁じている。実機で動かして確認する |
| 便利そうなライブラリを足す | 常駐の軽さが要求 (NFR-10 / 13)。足すなら先に ADR を書く |
| 単価・付与額をコードに書く | 課金体系は実際に一度変わっている (ADR-0010) |
| 要求に無い機能を足す | 2.2 節の非対象は「うっかり作りがちなもの」を名指しで禁じている |
| Copilot の設定ファイルに書き込む | 不変条件 INV-1。ADR-0012 がこれを支えている |

## 4. 迷ったときの判断基準

| 迷い | 基準 |
|---|---|
| 保存すべきか | **導出データなら保存しない** (DR-02)。迷ったら保存しない側 |
| 表示すべきか | **区別できないものは断定しない** (NFR-42)。中立表現にして補足はツールチップへ |
| エラーにすべきか | **ユーザーが何もできない事象はエラーにしない** (FR-C-57)。穏やかに状態として出す |
| どこに置くか | **IO があるか無いか**で分ける。純粋関数は IO を import しないモジュールへ |
| 2 秒経路に入れてよいか | **ネットワークか外部プロセスなら入れない** (INV-4)。例外なし |
| 進行状態をどこに持つか | **タブ切替で消えてよいか**で決める。消えては困るなら `appStore` |

## 5. 実装の型

### 純粋関数から書く

```rust
// 1. まず純粋関数とテスト
pub fn decide(recorded_offset: u64, recorded_size: u64, current_size: u64) -> Delta { ... }

// 2. その外側に IO
pub fn index_file(path: &Path, db: &Db) -> Result<Stats> {
    let delta = delta::decide(rec.offset, rec.size, meta.len());
    ...
}
```

**逆順に書くと、テストできない形が先に固まる。**

### コマンドの型

```rust
#[tauri::command(rename_all = "snake_case")]   // ★ 例外なく付ける
pub async fn projects_settings_update(
    state: tauri::State<'_, AppState>,
    req: OverrideRequest,
) -> Result<ProjectsSnapshot, AppError> {
    validate(&req)?;                                  // 1. 検証
    state.db.save_override(&req).await?;              // 2. 永続化
    let snap = state.cache.remerge(&req)?;            // 3. 再マージ (再スキャンしない)
    state.emit("projects-snapshot", &snap);           // 4a. イベント
    Ok(snap)                                          // 4b. 戻り値
}
```

## 6. 完了の定義

1. `npm run verify` が通る (typecheck + vitest + cargo test)
2. 純粋関数なら**テストがある**
3. **実際にアプリを起動して確認した** (NFR-53)
4. [トレーサビリティ](traceability.html) の該当行を埋めた
5. 置いた仮定を [未決事項](open-questions.html) に書いた
6. 実データで見つかった例外パターンを [テスト戦略](test-strategy.html) 7 節に追記した (NFR-51)

## 7. ドキュメントを直すとき

**編集するのは `docs/source/*.md` だけ。** `docs/*.html` は生成物なので直接編集しない (次回生成で消える)。

```bash
npm run docs:build   # 再生成
npm run docs:check   # 生成物が原本と一致するか検査
```

ページを増やすときは `docs/assets/docs.js` の `NAV` に 1 行足す。`docs/index.html` は手書きなのでカードも足す。

## 8. 参照の順序

| 知りたいこと | 見る場所 |
|---|---|
| 何を作るか | [要求仕様](requirements.html) |
| なぜその設計か | [設計判断](decisions.html) |
| どこに何を書くか | [アーキテクチャ](architecture.html) 4 節 |
| DB に何を置くか | [データモデル](data-model.html) |
| コマンドの名前と形 | [IPC API 仕様](api-spec.html) |
| 画面の文言と規則 | [UI 仕様](ui-spec.html) |
| 書き方の細かい規則 | [コーディング規約](coding-standards.html) |
| 何をテストするか | [テスト戦略](test-strategy.html) |
| 次に何をするか | [実装計画](implementation-plan.html) |
| まだ分かっていないこと | [未決事項](open-questions.html) |
| 同じものを別の名前で呼んでいないか | [用語集](glossary.html) |
