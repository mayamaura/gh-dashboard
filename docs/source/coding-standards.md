---
title: コーディング規約
lede: この規約はスタイルの好みではなく、要求に書かれた「壊れ方」を構造で防ぐためのもの。各項目に対応する要求 ID を付けてある。
status: ドラフト
version: 0.1
updated: 2026-09-08
---

## 1. 共通

| 規則 | 理由 |
|---|---|
| 純粋関数を先に切り出し、IO はその外側に置く | テストできる形にならない設計は、そもそも要求 (NFR-50) を満たしていない |
| 取れない値を既定値で埋めない。`Option` / `null` のまま運ぶ | 0 で埋めた瞬間、ゲージも一覧も嘘になる (NFR-40 / 43 / 44) |
| 「区別できないもの」をコードで断定しない | 長時間ツール実行と許可待ちはログ上まったく同じ形でしか現れない (FR-C-46 / NFR-42) |
| 定数化してよいのは「仕様として決めた値」だけ。**外部が決める値 (単価・付与額・枠名) は実行時に読む** | 課金体系は実際に一度変わっている (ADR-0010 / FR-C-134) |
| コメントは「なぜ」を書く。「何を」はコードが語る | |
| 日本語のコメント・エラー文言でよい。ログも日本語でよい | 個人利用が前提 (要求 1.2) |

## 2. Rust

### 2.1 構成

```rust
// crate ルート (lib.rs)
#![forbid(unsafe_code)]   // platform/win_job.rs だけが #[allow(unsafe_code)] で開ける
```

| 規則 | 詳細 |
|---|---|
| モジュール分割 | 純粋関数のモジュール (`detect` / `delta` / `tree` / `activity` / `path_key` / `url_detect`) に `std::fs` / `std::process` を import しない。**import が増えたら設計が崩れた合図** |
| `unsafe` | `platform/win_job.rs` のみ (ADR-0011 / FR-P-68) |
| エラー | `anyhow` は内部の伝播に使ってよい。**IPC 境界では `AppError` に変換する。`Result<T, String>` を返さない** |
| ログ | `tracing`。`error!` はユーザーに影響がある失敗にだけ使う。読み飛ばした壊れ行は `debug!` + 件数集計 |
| パニック | ライブラリコードで `unwrap` / `expect` を書かない。`main.rs` の初期化のみ許容 |

### 2.2 Tauri コマンド

```rust
#[tauri::command(rename_all = "snake_case")]   // ★ 全コマンドに必須 (IR-30)
pub async fn projects_scan(
    state: tauri::State<'_, AppState>,
) -> Result<ProjectsSnapshot, AppError> {
    // 同期 IO は必ずブロッキングタスクへ (NFR-20)
    let folders = state.db.scan_folders().await?;
    let snapshot = tokio::task::spawn_blocking(move || scan::run(folders)).await??;
    state.cache.set_projects(snapshot.clone());
    Ok(snapshot)
}
```

| 規則 | 理由 |
|---|---|
| すべてのコマンドに `rename_all = "snake_case"` | 引数名のケース変換事故は型検査でも lint でも検出できない (IR-30) |
| すべて `async fn`。同期 IO は `spawn_blocking` | NFR-20 |
| **`Mutex` のガードを保持したまま `.await` しない** | デッドロックの定番。ロックはスコープを閉じてから await する |
| 変更系は「検証 → 永続化 → スナップショット → イベント + 戻り値」 | IR-32 |

### 2.3 外部プロセス

```rust
use std::os::windows::process::CommandExt;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

Command::new("git")
    .creation_flags(CREATE_NO_WINDOW)   // ★ コンソールを出さない (FR-P-46)
    .args(["rev-parse", "--abbrev-ref", "HEAD"])
    .current_dir(root_path)             // ★ working_dir ではなくリポジトリルート (FR-P-44)
    .output()
```

| 規則 | 理由 |
|---|---|
| `CREATE_NO_WINDOW` を必ず付ける | 一瞬コンソールが点滅する (FR-P-46) |
| 非ゼロ終了を正常系として扱う (git のリモート未設定など) | FR-P-43 |
| **外部ツール起動は spawn の成否だけで判定し、終了コードを見ない** | `explorer.exe` は正常時も 1 を返す (FR-P-72) |
| 同時実行数に上限 (目安 8) を設ける | 数十プロジェクト × 複数コマンドの一斉起動を避ける (FR-P-45) |
| dev サーバーの子プロセスは Job Object に割り当てる | FR-P-61 |

### 2.4 ファイル読み

| 規則 | 理由 |
|---|---|
| ログ全体をメモリに載せない。行単位ストリーミング | 単一レコードが数百 KB〜数 MB になりうる (FR-C-08) |
| 末尾を見たいときは末尾 64KB のシーク読み。足りなければ 512KB で**1 回だけ**再試行 | FR-C-47 |
| 作業ディレクトリを知りたいときは先頭数十 KB の部分読み | FR-P-55 |
| **改行で終わらない末尾断片は確定分から除外し、オフセットに含めない** | 追記中のファイルを読むための必須の安全策 (FR-C-05) |
| 壊れた行は数えて読み飛ばす。件数は UI に返す | FR-C-12 / NFR-43 |

### 2.5 SQLite

| 規則 | 理由 |
|---|---|
| `INSERT OR IGNORE` + UNIQUE 制約で冪等にする | クラッシュ後の再開でも二重適用が起きない (FR-C-07) |
| オフセット更新は**トランザクションの最後** | 最後に成功したコミット位置から再開できる (FR-C-04) |
| 3,000 行ごとにコミットし、間に 15ms スリープ | 他機能と DB ロックを取り合わない (FR-C-09) |
| 他アプリの DB は読み取り専用で開くか、コピーしてから読む | DR-05 |

## 3. TypeScript / React

### 3.1 境界

| 規則 | 理由 |
|---|---|
| `invoke` を書いてよいのは `src/ipc/commands.ts` だけ | IR-31 |
| `listen` を書いてよいのは `src/ipc/events.ts` だけ。**購読解除関数を必ず返し、必ず呼ぶ** | 購読が積み上がると 2 秒ポーリングが多重発火する |
| DTO は `src/types/dto.ts` に集約。ページで独自に型を作らない | ADR-0006 |

### 3.2 ポーリング

```tsx
// 2 秒ポーリングの正しい形 (FR-C-41/42/43)
useEffect(() => {
  if (!visible) return            // ← 最小化・タブ離脱では起動すらしない
  let cancelled = false
  const tick = async () => {
    if (cancelled) return
    setLive(await liveStatusGet())
  }
  void tick()
  const id = setInterval(tick, 2000)
  return () => { cancelled = true; clearInterval(id) }   // ← アンマウントで確実に止まる
}, [visible])
```

| 規則 | 理由 |
|---|---|
| タイマーはコンポーネント内。バックエンドに常駐ループを作らない | ADR-0004 / FR-C-41 / NFR-11 |
| 可視判定は `visibilitychange` と `window-visibility` イベントの**両方**を合成する | 最小化は `visibilitychange` に伝播しない (FR-C-43) |
| `live_status_get` 以外を 2 秒経路に入れない | FR-C-103 / NFR-03 |

### 3.3 状態

| 規則 | 理由 |
|---|---|
| **長時間処理の進行状態はページのローカル state に置かない**。`appStore` に置く | タブ切替でコンポーネントごと破棄され、「処理は続いているのに表示だけ戻る」が起きる (FR-C-161) |
| タブを開いた瞬間はキャッシュ済みスナップショットを即描画し、裏で最新化する | FR-C-164 / FR-P-86 |
| 絞り込み・並べ替えはすべてフロントで完結させる。バックエンドに再問い合わせしない | FR-P-81 |

### 3.4 描画

| 規則 | 理由 |
|---|---|
| 図表は自前 SVG。チャートライブラリを入れない | ADR-0005 / FR-C-163 |
| アニメーションは `transform` / `opacity` / `stroke-dashoffset` / `filter` のみ | GPU 合成される (FR-C-163 / NFR-05) |
| 系統図とガントは**同じ木構築結果**を使う。生データの `depth` を信じない | 階層がずれる (FR-C-114) |
| 行内メニューはクリック位置から座標計算して固定配置で描く | 横スクロールコンテナの overflow クリッピングを避ける (FR-P-88) |
| 推測・推定・不確かなものには必ずラベルを出す | NFR-40 / 41 / 42 |

## 4. 命名

| 対象 | 規則 | 例 |
|---|---|---|
| Rust の関数・変数・モジュール | `snake_case` | `path_key`, `last_parsed_offset` |
| Rust の型 | `PascalCase` | `ProjectsSnapshot`, `AppError` |
| IPC のコマンド名 | `snake_case`、`<領域>_<対象>_<動作>` | `projects_dev_start`, `session_detail_get` |
| IPC の引数 | `snake_case` (`rename_all` で固定) | `path_key` |
| イベント名 | `kebab-case` | `projects-dev-status`, `index-progress` |
| TS の DTO フィールド | **Rust の JSON 表現に合わせて `snake_case`** | `path_key`, `last_activity_at` |
| TS の関数・変数 | `camelCase` | `projectsDevStart`, `liveStatus` |

> [!注意]
> DTO のフィールドだけ `snake_case` なのは意図的。**シリアライズ境界で名前を変換しないことで、「どちらの名前で来るか」を考える場面を消す。**変換したくなったら、変換ではなく `serde(rename)` で片側に寄せる。

## 5. コミット

```
<領域>: <何をしたか>

<なぜ必要だったか。要求 ID があれば書く>
```

領域は `projects` / `copilot` / `db` / `ui` / `docs` / `build` のいずれか。

例:

```
copilot: 差分インデックスの末尾断片を確定分から除外

追記中のファイルを読むと最終行が途中で切れる。切れた行を
確定扱いするとオフセットが進みすぎ、次回に本来の行を飛ばす。
FR-C-05
```

## 6. レビュー時に必ず見る 5 点

1. **2 秒ポーリング経路にネットワーク・外部プロセス起動が混ざっていないか** (INV-4)
2. **導出データを DB に入れていないか** (INV-5)
3. **取れない値を 0 や空文字で埋めていないか** (NFR-40/43/44)
4. **`rename_all = "snake_case"` が付いているか** (IR-30)
5. **純粋関数のモジュールに IO が入っていないか** (NFR-50)
