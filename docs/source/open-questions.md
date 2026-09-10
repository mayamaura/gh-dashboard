---
title: 未決事項 (OQ)
lede: 着手前に実測で確定させる項目。要求 付録 B の写しに、段階 0 のプローブ (T-0.1〜0.8) で出た数字を書き戻したもの。**推測で埋めて実装を進めない。**
status: 進行中
version: 0.3
updated: 2026-09-08
---

## 扱い方

| 状態 | 意味 |
|---|---|
| **未調査** | 何も分かっていない。これに依存する実装に着手しない |
| **調査中** | プローブは書いた。数字が出ていない |
| **一部判明** | 分かった範囲を「観測」に書いた。**残りを推測で埋めない** |
| **確定** | 結論と根拠が書かれ、影響を受ける ADR / タスクが更新済み |

> [!注意]
> **OQ を推測で埋めて実装を進めるのが、この計画で最も高くつく間違い。**
> 確認できないなら「取得不可」を返す実装にして、ここに残す。それが NFR-40 (推定を実測として表示しない) の実装でもある。

---

## 現在の状態 (2026-09-08 時点)

| OQ | 題目 | 状態 | 一言 |
|---|---|---|---|
| OQ-01 | `events.jsonl` のレコードスキーマ | **一部判明** | 16 種 / 77 レコードを分類。利用枠到達イベントは未観測 |
| OQ-02 | 1 レコード = 1 行か | **確定** | 成立。バイトオフセット方式を続行してよい (ADR-0003) |
| OQ-03 | VS Code の `chatSessions` とハッシュ | **一部判明** | ハッシュ再計算は 20 中 6 しか当たらない |
| OQ-04 | 稼働中セッションの検出 | **一部判明** | `ide/*.lock` は CLI の生死と対応しない (決定的な否定) |
| OQ-05 | `session-store.db` を安全に読めるか | **一部判明** | 書き込み中でも 12/12 読めた。ただし writer 不在時は未測定 |
| OQ-06 | 経路 A (Copilot SDK) が返す利用枠 | **一部判明** | `getCurrentAuth` なら 3 枠 + 付与額 + 月次リセット日が取れる。`getQuota` は劣化射影 |
| OQ-07 | サブエージェント / 委任 | **確定** | 存在する。親子とも `toolCallId` を持つ。**縮退不要** |
| OQ-08 | SDK / REST の認証 | **一部判明** | SDK は解決。REST は `gh` のトークンで通るが `user` スコープが無い |
| OQ-09 | 古いセッションの自動削除 | **未調査** | 据え置き。観測期間が 1 時間では何も言えない |
| OQ-10 | 実データ量 | **一部判明** | 3 セッションで 1.0 MB。数百 MB 規模は未再現 |
| OQ-11 | 拒否 / 非同期委任の表現 | **未調査** | 新規。T-4.9 / T-4.10 がここで止まる |
| OQ-12 | 経路 B (REST) が個人アカウントで返すもの | **一部判明** | 公開課金 API は 10/10 が 404 (5 件は `user` スコープ不足)。組織 / Enterprise は未検証 |

---

## 測定環境 — 数字を読む前に (2026-09-08)

**ユーザーの実 `~/.copilot` は、測定の前後を通してセッションを 1 件も持っていない。** 9 ファイルのみで、内訳は `config.json` + `ide/` (lock 3 件) + `logs/` (`process-*.log` 5 件)。**`session-state/` も `session-store.db` も存在しない。** 測定によってこの状態は変わっていない (INV-1 / N-5 を崩していない)。

そのため、セッションログを要する OQ-01 / 02 / 05 / 07 / 10 は、**`COPILOT_HOME` をスクラッチパッド配下の隔離ディレクトリに向けて** Copilot CLI 1.0.83 を 4 回起動し、3 セッションを生成して測定した。消費は **1.552 AI Credits / API 呼び出し 11 回**。`COPILOT_HOME` で設定と状態の保存先を丸ごと差し替えられることは OQ-08 の副産物として確認しており、以後のテストでも実データを汚さずに済む。

OQ-03 (VS Code `workspaceStorage`) と OQ-04 の `ide/*.lock` / `logs/` は、**ユーザーの実データ**での測定。

利用枠まわり (OQ-06 / OQ-08 / OQ-12) は**ユーザーの実アカウント**での測定。2026-09-07 の T-0.6 (SDK) に加え、**2026-09-08 に T-0.8 (`tools/probe/quota-rest.mjs`) を実施し、SDK (JSON-RPC + FFI ランタイム) と REST (`gh api`) を数秒差で別プロセスから取得した**。この同時取得が `resetDate` の正体を決めた (OQ-06)。

| 測定対象 | 出所 | 規模 |
|---|---|---|
| `events.jsonl` / `session-store.db` / `workspace.yaml` | 隔離 `COPILOT_HOME` | 3 セッション / 77 レコード / 291,259 バイト |
| `ide/*.lock` / `logs/process-*.log` | ユーザーの実 `~/.copilot` | lock 3 件 / log 5 件 |
| `workspaceStorage/<hash>/` | ユーザーの実 `%APPDATA%\Code` | 20 ワークスペース / `chatSessions` 26 ファイル / 1,522,441 バイト |
| `account.getQuota` / `account.getCurrentAuth` | ユーザーの実アカウント | 2026-09-07 に 2 回 (約 11 分間隔) / 2026-09-08 に再取得 + 3 回連続 (約 2.5 秒間隔、計 8 秒) |
| GitHub REST (`gh api`) | ユーザーの実アカウント (`gh` 2.95.0、スコープ `gist, read:org, repo, workflow`) | 13 リクエスト。200 は 1 件のみ |

> [!注意]
> **サンプルが小さい。** 3 セッション / 77 レコードは、要求が想定する「数百 MB / 数百ファイル」(NFR-01 / 02) の規模を再現していない。
> 以下の数字はすべて「**この規模で観測した**」という限定つきで読むこと。各 OQ の末尾に「この数字から言えないこと」を置いた。

プローブの生出力は `tools/probe-out/*.json` に残るが、**コミットしない** (個人のセッション内容と実パスを含みうる)。

---

## OQ-01 — `events.jsonl` のレコードスキーマ

**状態**: 一部判明

| 項目 | 内容 |
|---|---|
| 知りたいこと | レコード種別 / ロール / タイムスタンプ / トークン usage / クレジット消費 / ツール呼び出し / サブエージェント・委任タスクの表現 |
| 確認方法 | T-0.3 (`tools/probe/record-kinds.mjs`) |
| 影響範囲 | FR-C-01〜28 のほぼ全部 |

### 保存先とセッション 1 件の構成

`<COPILOT_HOME>/session-state/<session-uuid>/` の下に置かれる。

| ファイル | 実測サイズ | 内容 |
|---|---|---|
| `events.jsonl` | 45,354 / 103,224 / 142,681 バイト | レコード列 (本体) |
| `workspace.yaml` | 375 バイト | セッション 1 件のメタデータ (下記) |
| `checkpoints/index.md` | 172 バイト | 見出しだけの空テーブル |
| `rewind-file-snapshots/tracking.json` | 29 バイト | `{"schema":1,"tracking":true}` |
| `.workspace-fork.lock` | 0 バイト | |

`workspace.yaml` は 3 件とも同じ形で、**`events.jsonl` を開かずにセッションと作業ディレクトリを紐付けられる**。

```yaml
id: <session-uuid>
cwd: C:\...\work-b
client_name: github/cli
name: <初回プロンプトの文面がそのまま入る>
user_named: false
summary_count: 0
fork_count: 0
created_at: 2026-09-07T17:10:51.671Z
updated_at: 2026-09-07T17:14:22.666Z
```

- `cwd` は Windows のバックスラッシュ絶対パス。**`session.start.data.context.cwd` と同じ値**。FR-P-51 の `path_key` はここから作れる (FR-P-55 が言う「先頭数十 KB の部分読み」より安い)
- **`client_name` はエントリポイントの識別子。** 観測できたのは `github/cli` の **1 値のみ**
- `name` は `user_named: false` のとき**初回プロンプトの文面がそのまま入る**。FR-C-54 のタイトル・フォールバックに使えるが、**本文であるため 140 字プレビューの上限を掛けて扱う** (FR-C-02 / INV-6)

### レコードの骨格

**全レコード共通のトップレベルは 6 フィールドのみ** (77 レコード走査)。

| キー | 出現率 | 型 |
|---|---|---|
| `type` | 100% (77/77) | string |
| `data` | 100% (77/77) | object |
| `id` | 100% (77/77) | string (UUID) |
| `timestamp` | 100% (77/77) | string (ISO8601) |
| `parentId` | 94.8% (74/77)、`null` 3 件 | string / null |
| `agentId` | 23.4% (18/77) | string (UUID) |

- `parentId` は**直前のレコードの `id`** を指す。`null` は `session.start` の 3 件のみ。**レコード列は連鎖 (木) を成す**
- `agentId` は**サブエージェントに属するレコードだけ**に付く (OQ-07)

### 種別ごとの件数とサイズ (16 種 / 77 レコード / 291,259 バイト)

| type | 件数 | 合計バイト | 平均バイト |
|---|---|---|---|
| `assistant.message` | 11 | 125,053 | 11,368 |
| `system.message` | 4 | 116,892 | 29,223 |
| `session.usage_checkpoint` | 4 | 17,307 | 4,327 |
| `session.shutdown` | 4 | 6,752 | 1,688 |
| `tool.execution_complete` | 6 | 6,501 | 1,084 |
| `user.message` | 5 | 3,951 | 790 |
| `tool.execution_start` | 6 | 3,430 | 572 |
| `assistant.turn_start` | 11 | 2,809 | 255 |
| `session.auto_mode_resolved` | 5 | 2,267 | 453 |
| `assistant.turn_end` | 11 | 2,182 | 198 |
| `session.start` | 3 | 1,635 | 545 |
| `session.model_change` | 3 | 759 | 253 |
| `session.resume` | 1 | 533 | 533 |
| `subagent.started` | 1 | 492 | 492 |
| `subagent.completed` | 1 | 434 | 434 |
| `subagent.configured` | 1 | 262 | 262 |

**`system.message` 4 件で全バイトの 40% を占める** (1 件 36,428 バイトが最大レコード)。本文レコード (FR-C-49 が言う「ユーザー / アシスタント」) は `user.message` / `assistant.message` の 2 種。

### 主要な `data` の構造 (値は伏せ、型と構造のみ)

- `session.start.data` = `{sessionId, version:number, producer:"copilot-agent", copilotVersion:"1.0.83", startTime:ISO8601, contextTier:null, sessionLimits:{maxAiCredits:number}, context:{cwd:string}, alreadyInUse:boolean, remoteSteerable:boolean}` — **`context.cwd` に作業ディレクトリの絶対パス** (実測 126 文字)。`sessionLimits.maxAiCredits` は FR-C-95 に使える
- `session.resume.data` = `{resumeTime:ISO8601, ...}`
- `session.usage_checkpoint.data` = `{totalNanoAiu:number, totalPremiumRequests:number, modelCacheState:[{modelId, cacheExpiresAt:ISO8601, cacheTtlSeconds}], promptCacheBreakState:[{conversation:"main", models, lastActiveModel, pendingRewriteSources}]}`
- `session.shutdown.data` = `{shutdownType:"routine", totalPremiumRequests, totalNanoAiu, tokenDetails:{input:{tokenCount}, cache_read:{tokenCount}, cache_write:{tokenCount}, output:{tokenCount}}, totalApiDurationMs, sessionStartTime:epoch_ms, eventsFileSizeBytes, codeChanges:{linesAdded, linesRemoved, filesModified}, modelMetrics, agentMetrics, currentModel, currentTokens, systemTokens, conversationTokens, toolDefinitionsTokens}` — **FR-C-20 のセッション集計はほぼこの 1 レコードで揃う**
- `tool.execution_start.data` = `{toolCallId, toolName, arguments, turnId, model}` + 子なら `parentToolCallId`、shell 系なら `shellToolInfo`
- `assistant.turn_start.data` = `{turnId:string("0","1",...), interactionId:UUID}` / `assistant.turn_end.data` = `{turnId}`

### 消費クレジットとトークン

**`totalNanoAiu / 10^9 = AI Credits` を実測で確認した。**

| セッション | nanoAIU 合計 | CLI フッター表示 |
|---|---|---|
| `db756c0c…` | 382,635,000 | **0.38 AI Credits** (一致) |
| `240cecb1…` | 598,806,000 (サブエージェント分込み) | **0.6 AI Credits** (一致) |

usage / cost 系キーの出現率 (2 階層目まで): `data.totalNanoAiu` 8 件 (10.4%) / `data.tokenDetails` 4 件 (5.2%) / `data.currentTokens` 4 件 / `data.systemTokens` 4 件 / `data.conversationTokens` 4 件 / `data.toolDefinitionsTokens` 4 件 / `data.totalTokens` 1 件 (1.3%)。

タイムスタンプ候補は `timestamp` (ISO8601、77 件) / `data.startTime` (ISO8601、3) / `data.resumeTime` (ISO8601、1) / `data.sessionStartTime` (**epoch ミリ秒**、4)。**ISO8601 と epoch ミリ秒が混在する。**

### FR-C-49 の裏付け

観測した 3 ファイルの**物理的な最終行は、いずれも `session.shutdown` または `session.usage_checkpoint`** で、本文レコードではなかった。**FR-C-49 (末尾から遡って最初の本文レコードを採る) は実データで必要**である。

### この数字から言えないこと

- **利用枠到達 / レート制限 / セッションのクレジット上限到達のレコードは 1 件も観測していない** (FR-C-28)。種別名も形も分かっていない
- パースエラー / ツール失敗 / 中断のレコードも未観測
- VS Code 拡張 / coding agent が生成する `events.jsonl` は 1 件も見ていない。`client_name` は `github/cli` の 1 値しか観測できていない
- 16 種で全部という保証はない。`copilotVersion` は `1.0.83` の 1 バージョンのみ

---

## OQ-02 — 1 レコード = 1 行か (設計の分水嶺)

**状態**: **確定** — 成立する

| 項目 | 内容 |
|---|---|
| 知りたいこと | `events.jsonl` が **1 レコード = 1 行 / 行内に生の改行を含まない / BOM なし**か |
| 確認方法 | T-0.2 (`tools/probe/jsonl-shape.mjs`) |
| 影響範囲 | FR-C-02〜08、ADR-0003 |

3 ファイル / 291,259 バイトを走査した結果。

| 項目 | 実測 |
|---|---|
| 行数 / parse 成功レコード数 / parse 失敗 | **77 / 77 / 0** |
| BOM ありファイル | **0 / 3** (先頭 3 バイトは `7B 22 74` = `{"t`) |
| CRLF 行 / LF のみ行 | **0 / 77** |
| 末尾が改行で終わらないファイル | **0 / 3** |
| **バイトオフセット往復検証** | **一致 77 / 不一致 0** |
| 行長 (バイト) | min 184 / median 552 / mean 3,922 / p95 17,361 / **max 36,428** |

> [!注意]
> プローブは「生改行を含む行 22 件」も報告するが、これは **parse 後の文字列値に改行文字が含まれる**という意味であり、**JSON としてはエスケープ済み**。行分割は壊れていない (**parse 失敗 0 件**が根拠)。該当キーは `data.content` / `data.reasoningBlocks.blocks.summary.text` / `data.reasoningText` / `data.result.content` / `data.result.detailedContent` / `data.transformedContent`。
> **この区別を曖昧にしないこと。** 「行内に生の改行がある」と読み違えると、成立している前提を誤って捨てることになる。

### 追記のみであることの直接検証

`--resume` で既存セッションを再開して比較した。

- 再開前 133,394 バイト → 再開後 142,681 バイト (**+9,287**)
- **先頭 133,394 バイトの SHA-256 が一致** (バイト単位で不変。既存部分は書き換えられない)
- 追記されたのは 8 レコード: `session.resume` → `session.auto_mode_resolved` → `user.message` → `assistant.turn_start` → `assistant.message` → `assistant.turn_end` → `session.usage_checkpoint` → `session.shutdown`
- `session.resume.parentId` が**直前の `session.shutdown` の `id`** を指す (連鎖は再開をまたいで続く)

> [!注意]
> **1 ファイルに `session.shutdown` が複数現れる** (3 ファイルに 4 件)。「`session.shutdown` が来たらセッション終了」と扱うと**再開で壊れる**。終了判定に使えるのは「**末尾から遡って最初のレコードが `session.shutdown` か**」まで (ADR-0014)。

### 結論

**バイトオフセット方式 (ADR-0003) の前提は成立する。** 代替案 (レコード境界を別に記録する / `session-store.db` を索引の正にする) は採らない。判断と却下理由は ADR-0003 / ADR-0013。

### この数字から言えないこと

- 最大レコードは 36,428 バイトで、FR-C-08 の根拠が言う「数百 KB〜数 MB」は**再現していない**。ストリーミング読みと 512KB 上限 (T-4.12) の必要性は、この測定では証明されていない (**それでも外さない**。外して壊れたときの代償の方が大きい)
- 書き込みの**最中**に読んだわけではない。「改行で終わらない末尾断片」(FR-C-05) は **1 度も遭遇していない**。FR-C-05 は依然として必須で、ユニットテストで固める (T-4.1)
- truncate / ファイル入れ替わり (FR-C-06) は観測していない

---

## OQ-03 — VS Code `chatSessions` とハッシュの導出

**状態**: 一部判明 (**確定にしない**)

| 項目 | 内容 |
|---|---|
| 知りたいこと | `%APPDATA%\Code\User\workspaceStorage\<hash>\chatSessions\` の中身と、`<hash>` の導出アルゴリズム |
| 確認方法 | T-0.5 (`tools/probe/vscode-sessions.mjs`)。ハッシュ再計算と cwd 逆引きの**両方** |
| 影響範囲 | FR-P-50〜58 |

### (a) ハッシュ再計算の照合 — 20 ワークスペース、`workspace.json` は 20/20 存在

30 通りの候補 (uri そのまま / 小文字化 / 末尾スラッシュ有無 / `%3a` 小文字化 / コロン非エンコード / fsPath 各種 × md5 / sha1 / sha256) を総当たりした。

| 候補 | 一致数 |
|---|---|
| uri そのまま / md5 | 5 |
| uri 末尾スラッシュ除去 / md5 | 5 (同一ディレクトリへの重複ヒット) |
| uri `%3a` 小文字化 / md5 | 5 (同上) |
| uri コロン非エンコード化 / md5 | 5 (同上) |
| fsPath バックスラッシュ化 + 小文字 / md5 | 1 |
| 上記以外 25 候補 | 0 |

- **いずれかの候補で一致: 6 / 20。どの候補でも不一致: 14 / 20**
- 実質は 2 系統 (素の `uri.toString()` の MD5 が 5 件、fsPath 変形の MD5 が 1 件)

**帰結**: ハッシュ再計算は**主経路にできない**。FR-P-51 (cwd の完全一致) と FR-P-52 (フォルダ名フォールバック) を主経路とし、ハッシュ再計算には依存しない。

### (b) `chatSessions` の中身

- `chatSessions/` を持つのは **15 / 20** ワークスペース。ファイル 26 件、**全て `.jsonl`**、計 1,522,441 バイト、**最大 690,053 バイト**
- **VS Code 側も JSONL。** 先頭が `kind:0` のスナップショット、以降が `kind:1` / `kind:2` のパッチ行という形式
- `kind:0` のトップレベルキー (26/26 = 100%): `version` / `creationDate` / `initialLocation` / `responderUsername` / `sessionId` / `hasPendingEdits` / `requests` / `pendingRequests` / `inputState`
- **cwd らしき文字列は `kind:0` だけを見ると 0 件。** 全行 (パッチ行込み) を走査すると 9/26 ファイルでヒット (行の内訳は kind:0 が 4 / kind:1 が 14 / kind:2 が 28)
- タイムスタンプは `creationDate` のみ (26/26、`number` = epoch ミリ秒)。**終了時刻に相当するフィールドは無い**
- 孤児化 (`workspace.json` の folder が指すパスが今ディスク上に無い): **2 / 20**
- `<hash>/` 直下の出現数: `state.vscdb` 20 / `state.vscdb.backup` 18 / `workspace.json` 20 / `chatEditingSessions/` 15 / `chatSessions/` 15 / `ms-python.python/` 13 / `GitHub.copilot-chat/` 3

### (c) mtime と `creationDate` のずれ

10 サンプルの差 (ミリ秒): 419,596 / 6,708,065 / 29,571 / 59,743 / 59,652 / 3,518,493 / 59,715 / 1,860,260 / 353,631 / 299,797。**全サンプルで mtime が後**、最小 29,571ms、最大 6,708,065ms (約 1.9 時間)。

**FR-P-56 (mtime を新旧判定に使わない) の裏付け**: `creationDate` と mtime は最大 1.9 時間ずれる。**mtime で並べると「最後に触った順」にはなっても「セッションの新旧」にはならない。**

### この数字から言えないこと

- **不一致 14 件の原因は切り分けられていない。** VS Code のバージョン差 / 多重ルート workspace / `vscode-remote://` スキーム (2 件は fsPath 候補が生成できず uri 系のみ試行) のいずれか、あるいは複合
- `kind:1` / `kind:2` のパッチ行の形式 (JSON Patch なのか独自形式なのか) は解析していない。cwd 逆引きを主経路にするなら、この解析が別途要る
- `chatSessions` から**消費クレジット / トークンが取れるか**は見ていない (FR-P-50 の「直近セッションのクレジット消費」に関わる)

---

## OQ-04 — 稼働中セッションの検出方法

**状態**: 一部判明 (**決定的な否定が出た**)

| 項目 | 内容 |
|---|---|
| 知りたいこと | `~/.copilot/ide/` と `logs/process-*.log` から**読み取り専用で**稼働判定できるか |
| 確認方法 | T-0.4 (`tools/probe/live-detect.mjs`) + 隔離 `COPILOT_HOME` での CLI 4 回起動 |
| 影響範囲 | FR-C-40〜45、FR-C-70〜72 |

### `ide/*.lock` (3 件、ユーザーの実データ)

| pid | ideName | workspaceFolders | `timestamp` − mtime | PID 生存 | プロセス名 |
|---|---|---|---|---|---|
| 42500 | Visual Studio Code | `d:\masah\Projects\gh-dashboard` | 174ms | true | **Code** |
| 30592 | Visual Studio Code | `c:\Users\masah\Documents\a-i-one` | 268ms | true | **Code** |
| 23724 | Visual Studio Code | `d:\masah\Projects\KanjiPractice` | 141ms | true | **Code** |

キーは 3 件とも `socketPath` / `scheme` / `pid` / `ideName` / `timestamp` / `workspaceFolders` / `isTrusted` + **`headers`**。`socketPath` は `\\.\pipe\mcp-<uuid>.sock`。

> [!注意]
> **`headers` の値は一切読み出していない。** INV-2 / FR-C-72 のとおり、読まない・DTO に定義しない・表示しない。OQ-08 で「CLI は資格情報を OS の資格情報ストアに保存し、失敗時は `~/.copilot/` 配下の平文ファイルに落とす」ことが裏付けられたため、この不変条件はいっそう外せない。

### `logs/process-<epoch_ms>-<pid>.log` (5 件、ユーザーの実データ)

- PID 32468 / 40808 / 30068 / 11008 / 36736。**4 件は死亡**。生存している 1 件 (36736) もプロセス名は `Code` で、**ログ内容 (CLI server) と一致しない → PID 再利用の可能性を排除できない**
- ログレベルは **INFO のみ 37 行**。session id / workspace / cwd らしき文字列は **0 件**
- 最終行は "CLI server prepared for shutdown; transport remains open for RPC response" または "Server started, waiting for requests"

### 決定的な否定的結果

- **lock 側 PID 集合 `{42500, 30592, 23724}` と log 側 PID 集合 `{32468, 40808, 30068, 11008, 36736}` の交差は 0 件**
- lock の `pid` は**すべて VS Code 本体 (`Code.exe`) のプロセス ID** だった
- **隔離 `COPILOT_HOME` で CLI を 4 回起動したが、`ide/` ディレクトリは 1 つも作られなかった**

**結論**: `ide/*.lock` は **IDE 側 (VS Code) が書くもので、CLI セッションの生死とは対応しない**。`logs/process-*.log` はセッションを特定できず、PID も信用できない。**FR-C-40 が前提にする「状態ファイル + PID の生存確認」は、CLI 経路には適用できない。**

### 否定の裏側で分かったこと (`logs/` と `ide/` は別のものを数えている)

隔離 `COPILOT_HOME` での実測が、上の否定を「観測できなかった」ではなく「対応しないことが分かった」に変える。

| 観測 | 隔離 `COPILOT_HOME` (CLI のみ 4 回起動) | ユーザーの実 `~/.copilot` (VS Code のみ) |
|---|---|---|
| `logs/process-*.log` | **4 件** (起動回数と 1:1) | 5 件 |
| `ide/*.lock` | **0 件** | 3 件 |
| `session-state/` | 3 セッション | 無し |

- **`logs/process-*.log` は CLI プロセスの起動と 1 対 1 で増える。**ファイル名の PID は CLI プロセスのもの
- **`ide/*.lock` は IDE インスタンスと 1 対 1。**ファイル名は UUID で、`pid` は IDE のもの
- 実 `~/.copilot` の 5 件のログは、**VS Code が CLI を stdio サーバーとして起動したときのもの** (ログ本文が "Starting CLI in server mode (stdio)" で始まる)。**同じ `~/.copilot` を 2 種類の書き手が共有している**

つまり交差 0 件は偶然ではない。**2 つのファイル群は別のものを数えており、突き合わせても稼働セッションは出てこない。**

ただし `logs/` も稼働判定には使えない。**セッション ID がログ本文に一切出ないため** (37 行中 0 件)、どのプロセスがどのセッションを持っているかを結び付けられない。

> [!注意]
> **要求の前提が実測と異なるケース。** 要求本文 (FR-C-40) は書き換えない。代替の判定方法は ADR-0014 に書いた。
> `ide/*.lock` は**役割を FR-C-70〜72 (IDE ワークスペース一覧) に限定する**。`pid` の生存確認は FR-C-71 の「切断済み」表示にそのまま使える (3/3 生存を確認)。

### 代わりに使える経路 (実測)

3 セッションで、鮮度・活動時刻の候補が 3 つあり、**それぞれ数秒ずれる**。

| セッション | `sessions.updated_at` (DB) | `workspace.yaml` mtime | `events.jsonl` mtime | events サイズ |
|---|---|---|---|---|
| db756c0c | 17:09:15.375 | 17:09:15.098 | 17:09:17.880 | 45,354 |
| 240cecb1 | **17:14:22.677** | 17:14:22.667 | 17:14:26.993 | 142,681 |
| ffb77b8d | 17:13:36.502 | 17:13:36.147 | 17:13:50.864 | 103,224 |

- 順序は常に `sessions.updated_at` ≈ `workspace.yaml` mtime (差 10ms 以内) **<** `events.jsonl` mtime
- **`sessions.updated_at` は最後のイベントより 4〜14 秒早い。** ターン開始時に書かれ、`events.jsonl` はその後も追記が続くため
- **`sessions.updated_at` は再開でちゃんと進む** (240cecb1 は created 17:10:51 → updated 17:14:22 で、`--resume` の後)。稼働検出に使える
- **`sessions.updated_at` は DB の列であってファイルの mtime ではない。** FR-P-56 が言う「中身のタイムスタンプ」に該当する。mtime とは別物として扱う
- ただしこれは「値が変化したときだけ更新される観測時刻」(FR-C-86) ではなく**書き込み時刻**である。**FR-C-86 の観測時刻は我々の側で持つ必要がある**

`session.shutdown` の有無も終了の手掛かりになるが、**1 ファイルに複数現れる** (OQ-02) ため、「末尾から遡って最初のレコードが `session.shutdown` か」までしか使えない。

判定方法の決定は ADR-0014 (`events.jsonl` の末尾 + mtime を主経路にし、`session-store.db` には依存しない)。

### SDK が定義するフックイベント (参考)

`@github/copilot-sdk` の `dist/generated/rpc.d.ts` に、`sessionStart` / `sessionEnd` / `postResult` / `prePRDescription` / `errorOccurred` / `agentStop` / **`subagentStart` / `subagentStop`** / `preCompact` / `permissionRequest` / `notification` が定義されている。**採用しない** (ADR-0012。ユーザーの設定ファイルへの書き込みを伴う)。

### この数字から言えないこと

- **CLI background / coding agent / VS Code 拡張の 3 経路は 1 度も動かしていない。** 測定できたのは CLI interactive (`client_name: github/cli`) のみ
- `events.jsonl` の mtime を使う代替経路は**設計として決めただけで、「稼働中に見える / 終了後に消える」ことを通しで確認していない**。段階 5 の実機確認 (T-5.1) で確かめる
- lock ファイルがプロセス終了時に消えるかは未確認 (3 件とも PID 生存中だったため)。FR-C-71 が言う stale な行はまだ観測していない
- **N 秒しきい値 (FR-C-50 の目安 120 秒) の妥当性は測っていない。** 上表のずれは 4〜14 秒で、120 秒には収まる

---

## OQ-05 — `session-store.db` を安全に読めるか

**状態**: 一部判明

| 項目 | 内容 |
|---|---|
| 知りたいこと | スキーマと、**他プロセスが開いている最中に安全に読めるか** (WAL / ロック) |
| 確認方法 | 隔離 `COPILOT_HOME` で CLI 実行中に読み取り専用オープンを反復 |
| 影響範囲 | FR-C-01〜09、ADR-0003 / ADR-0013 |

`<COPILOT_HOME>/session-store.db` + `-wal` + `-shm`。**`PRAGMA journal_mode` = `wal`**。

### 同時読み取りの実測

CLI が**セッションを実行中 (書き込み中)** に、Node 24 の `node:sqlite` で `new DatabaseSync(path, { readOnly: true })` を **1.2 秒間隔で 12 回**試行した。

- **12/12 成功。所要 2〜4ms**
- その間 WAL は 502,672 → 609,792 バイトに増え、`assistant_usage_events` が 7 → 10 行に増えるのを**ライブで読めた**
- **コピー不要** (DR-05 が挙げる「コピーして読む」は不要だった)

### スキーマ

テーブルは `schema_version` / `sessions` / `turns` / `checkpoints` / `session_files` / `session_refs` / `forge_trajectory_events` / **`assistant_usage_events`** / `forge_skill_proposals` / `search_index` (FTS5 仮想テーブル + 付随 5 テーブル) / `dynamic_context_items`。

```sql
CREATE TABLE sessions ( id TEXT PRIMARY KEY, cwd TEXT, repository TEXT, host_type TEXT,
  branch TEXT, summary TEXT, created_at TEXT DEFAULT (datetime('now')),
  updated_at TEXT DEFAULT (datetime('now')) )
CREATE INDEX idx_sessions_cwd ON sessions(cwd)
CREATE INDEX idx_sessions_repo ON sessions(repository)

CREATE TABLE turns ( id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL REFERENCES sessions(id),
  turn_index INTEGER NOT NULL, user_message TEXT, assistant_response TEXT,
  timestamp TEXT DEFAULT (datetime('now')), UNIQUE(session_id, turn_index) )

CREATE TABLE assistant_usage_events ( id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL REFERENCES sessions(id), turn_index INTEGER,
  agent_id TEXT, parent_tool_call_id TEXT, model TEXT NOT NULL,
  input_tokens INTEGER, output_tokens INTEGER, cache_read_tokens INTEGER,
  cache_write_tokens INTEGER, reasoning_tokens INTEGER, total_nano_aiu INTEGER,
  request_multiplier REAL, duration_ms INTEGER, time_to_first_token_ms INTEGER,
  output_ttft_ms REAL, inter_token_latency_ms INTEGER, initiator TEXT, api_endpoint TEXT,
  reasoning_effort TEXT, finish_reason TEXT, content_filter_triggered INTEGER,
  token_details_json TEXT, created_at TEXT DEFAULT (datetime('now')) )
CREATE INDEX idx_assistant_usage_events_session ON assistant_usage_events(session_id, id)
CREATE INDEX idx_assistant_usage_events_session_turn ON assistant_usage_events(session_id, turn_index)
CREATE INDEX idx_assistant_usage_events_model ON assistant_usage_events(model)
```

- `initiator` の実測値: **`user` / `agent` / `sub-agent`**
- `finish_reason` の実測値: `stop` / `tool_calls`
- `sessions.cwd` は Windows のバックスラッシュ絶対パス。`repository` / `host_type` / `branch` は今回のサンプルでは**すべて `null`** (git リポジトリ外の作業ディレクトリだったため)

### 実測行数 (3 セッション時点)

`sessions` 3 / `turns` **2** / `assistant_usage_events` 11 / `checkpoints` 0 / `session_files` 0 / `session_refs` 0 / `forge_trajectory_events` 0 / `forge_skill_proposals` 0 / `dynamic_context_items` 0。

### INV-6 との関係

**`turns` には `user_message` / `assistant_response` の本文がそのまま入る。** ただしこれは**我々の DB ではなく Copilot 側の DB** なので、**読むこと自体は INV-6 に触れない** (INV-6 / FR-C-02 が禁じているのは、我々の `app.db` に本文を複製すること)。

一方で「**本文を持たない索引**」という設計理由は変わらない。むしろ強まった。

- **`turns` は 3 セッションで 2 行しか無い**のに対し `events.jsonl` は 77 レコード。`turns` は全レコードを持たず、**本文の正は `events.jsonl`**
- `turns` にはバイトオフセットに相当する位置情報が無く、FR-C-120 のビューア (1 レコードだけシーク読み) に使えない

判断は ADR-0003 (自前索引を続ける) と ADR-0013 (`session-store.db` の役割を限定する)。

### この数字から言えないこと

- **writer が居ない状態での読み取り専用オープンを測っていない。** 12/12 の成功はすべて CLI が実行中 (= `-shm` が他プロセスに保持されている状態) での測定である。SQLite は WAL の復旧に `-wal` / `-shm` への**書き込み**を要することがあり、それは INV-1 (Copilot 側に書かない) に抵触する。**測っていない条件に賭けない** (ADR-0013)
- 行数が小さすぎる。`turns` が 3 セッションで 2 行しか無い理由 (特定の経路でしか書かれないのか、遅延書き込みなのか) は分かっていない
- `schema_version` の値も、バージョン差でスキーマがどう動くかも見ていない
- 数百 MB 規模の `session-store.db` に対して所要 2〜4ms が保たれるかは分からない

---

## OQ-06 — 経路 A (Copilot SDK) が個人アカウントで何を返すか

**状態**: 一部判明 (**設計判断に必要な部分は決着した。確定にはしない**)

| 項目 | 内容 |
|---|---|
| 知りたいこと | `quotaSnapshots` のキーに何が並ぶか / 月次クレジット残高が取れるか / `entitlementRequests` の意味 / `1 AI Credit = $0.01` の裏取り |
| 確認方法 | T-0.6 (`tools/probe/quota-sdk.mjs`) と T-0.8 (`tools/probe/quota-rest.mjs` の SDK 比較部)。`@github/copilot-sdk` 1.0.13 |
| 影響範囲 | FR-C-80〜95、FR-C-130〜144 |

> [!注意]
> **2026-09-08 の T-0.8 で、このセクションの記述を 3 点訂正した。**
>
> 1. `resetDate` は「**問い合わせ時刻に追従する**」のではない。中身は**利用枠スナップショットの観測時刻** (`timestamp_utc`) である
> 2. `remainingPercentage` が再計算と一致しない理由が判明した。**丸めの異なる 3 つの消費量が返っている**ためで、単位不明だからではない
> 3. **月次の付与額 (分母) と本物のリセット日は、経路 B ではなく経路 A の `account.getCurrentAuth` で取れる**
>
> 訂正前の記述は誤った観測に基づいていた。**訂正前の結論に依存した実装・タスクが無いか確認すること。**

### 経路 A には API が 2 つあり、返る情報量が違う

`account.getQuota` は `account.getCurrentAuth` の**劣化した射影**である。同じサーバ側スナップショットを元にしているが、落ちている情報がある。

| 項目 | `account.getQuota` | `account.getCurrentAuth` |
|---|---|---|
| 枠あたりのフィールド数 | **10** | **12** |
| 付与額 | `entitlementRequests` (200) | `entitlement` (200) — **同値** |
| 残率 | `remainingPercentage` (99.2) | `percent_remaining` (99.2) — **同値** |
| 消費量 | `usedRequests` (2) = **整数丸め** | `quota_remaining` (**198.4**) から小数で出せる |
| 月次リセット日 | **返らない** | **`copilotUser.quota_reset_date` = `2026-10-01`** |
| 枠が適用されるか | `hasQuota` (**型宣言に無い**) | `has_quota` |
| プラン識別 | 返らない | `copilot_plan` / `access_type_sku` |
| 観測時刻 | `resetDate` (**名前が誤り**) | `timestamp_utc` (**同値**) |

`getQuota` が落としている情報: **小数の `quota_remaining`** / **本物の `quota_reset_date`** / `quota_id` / プラン識別子。加えて `timestamp_utc` を **`resetDate` という誤った名前**で返している。

`getCurrentAuth().authInfo.copilotUser` の実測値 (2026-09-08):

```
copilot_plan         : individual
access_type_sku      : free_limited_copilot
token_based_billing  : true
quota_reset_date     : 2026-10-01
quota_reset_date_utc : 2026-10-01T00:00:00.000Z
```

### `resetDate` の正体 — 2 経路の同時取得で確定した

同じ数秒の間に、**SDK (JSON-RPC + FFI ランタイム) と REST (`gh` CLI) を別プロセスで**叩いた。

| 経路 | 値 |
|---|---|
| SDK `probedAt` | `2026-09-08T11:58:32.827Z` |
| SDK `getQuota().quotaSnapshots.chat.resetDate` | **`2026-09-08T11:58:34.381Z`** |
| REST `copilot_internal/user` の `quota_snapshots.chat.timestamp_utc` | **`2026-09-08T04:58:34.381-07:00`** (= `11:58:34.381Z`) |
| REST `probedAt` | `2026-09-08T11:58:35.748Z` |

- **`resetDate` と `timestamp_utc` はミリ秒まで同一** (表記は SDK が `Z`、REST が `-07:00` オフセット)。**別プロセス・別経路での一致なので、片方のクライアントの癖ではない**
- 3 枠とも `timestamp_utc` は同一値 (`allTimestampsIdentical: true` / `uniqueTimestampCount: 1`)
- **`quota_reset_at` は 3 枠とも `0`** (`allQuotaResetAtAreZero: true`)。**使えない**
- **本物のリセット日は `copilotUser.quota_reset_date` = `2026-10-01` / `quota_reset_date_utc` = `2026-10-01T00:00:00.000Z`**

**結論**: `account.getQuota` の **`resetDate` はフィールド名が実体と一致していない**。中身は利用枠スナップショットの観測時刻である。**リセット日として表示しない**という結論は変わらないが、**理由が変わった**。

#### 訂正: 「呼ぶたびに動く」は誤りだった

同一プロセスで **3 回連続** (約 2.5 秒間隔、計 8 秒) に呼んだ結果:

```
#0 getCurrentAuth timestamp_utc = 2026-09-08T04:53:32.286-07:00   getQuota resetDate = 同値
#1 getCurrentAuth timestamp_utc = 同値                            getQuota resetDate = 同値
#2 getCurrentAuth timestamp_utc = 同値                            getQuota resetDate = 同値
```

**3 回とも完全に同一。呼ぶたびに動くのではない。**

- 2026-09-07 に 11 分あけた 2 回の取得で値が動いて見えたのは、**その間に実際に消費が発生してスナップショットが更新されたため** (CLI セッション 4 回 = 1.552 AI Credits)
- 呼び出し時刻との前後関係も一定しない。2026-09-07 の測定では `resetDate` = `17:28:04.715Z` に対し `probedAt` = `17:28:31.134Z` で **26 秒前**。2026-09-08 の測定では `probedAt` の **1.5 秒後**。**「呼んだ瞬間の時刻」ではない**
- 1.5 秒後になるのは **SDK クライアントの起動時 (`client.start()` に 663ms) にスナップショットが取り直されている**ためと見られるが、**これは推測で、検証していない**

#### 鮮度 (FR-C-86) にどこまで使えるか

- **使える**: `timestamp_utc` は FR-C-86 が禁じる「取得しに行った時刻」ではなく、**取得元スナップショットの観測時刻**である。**出所の観測時刻としてはそのまま使える**
- **足りない**: 「**値が変化したときだけ更新される**」保証が無い。**`quota_remaining` が 198.4 のまま `timestamp_utc` だけが動いた実例がある** — `11:51:22.610` / `11:53:32.286` / `11:58:34.381` の 3 時点で `quota_remaining` はいずれも 198.4 (プローブ出力は最終実行で上書きされるため、前 2 者は実行ログからの値)
- したがって「**この値が動いた = 利用枠が変わった**」とは言えない
- → **FR-C-86 が求める観測時刻は、我々の側で `observed_at` として持つ必要がある** (値が変わったときだけ更新する)。**この結論は訂正前と同じだが、理由が違う。** 訂正前は「毎回変わるから使えない」、正しくは「**観測時刻としては正しいが、変化検知には使えない**」
- **FR-C-85 (リセット日時を過ぎた観測を除外) には `quota_reset_date_utc` を使う。** `resetDate` でも `quota_reset_at` (= 0) でもない

### `remainingPercentage` の謎が解けた — 同じ消費が 3 通りの丸めで返る

`chat` 枠の実測 (2026-09-08、`copilot_internal/user` と `getCurrentAuth` で同値):

| 値 | 計算 | 実測 |
|---|---|---|
| `entitlement − quota_remaining` | 200 − 198.4 | **1.6** (小数) |
| `entitlement − remaining` | 200 − 198 | **2** (整数) |
| `credits_used` | — | **1** (REST のみ。`getCurrentAuth` には無い) |

**3 つとも異なる。** そのうえで:

- **`percent_remaining` = `quota_remaining / entitlement × 100` が厳密一致する** (chat 198.4/200 = 99.2、completions 2000/2000 = 100。プローブの `percentMatchesComputedExactly: true`)。`entitlement = 0` の枠だけ計算不能
- `getQuota` の **`usedRequests` は `entitlement − remaining` (整数丸め) と一致する** (chat 2 / 2)

→ **`(entitlementRequests − usedRequests) / entitlementRequests` が `remainingPercentage` と一致しないのは正常。** (200−2)/200 = 99.0% と実値 99.2% (= 198.4/200) の **0.2 ポイント差は丸め誤差そのもの**である。API のバグでも単位不明でもない。

**「再計算しない」という結論は変わらないが、理由が変わった。** 「単位不明の値だから」ではなく「**丸めの異なる 3 つの値が混在しているから**」。

**`usedRequests` についての訂正**: 前の記述は「API 呼び出し数でもトークン量でもない、単位不明の値」としていた。正しくは **`entitlement − remaining`、すなわち整数に丸めた消費量**である。API 呼び出し 11 回に対して 2 しか増えなかったのは、真の消費が **1.55** で整数丸めが効いたため。**API 呼び出し数と一致しないのが正しい。** ただし丸め上げが入るので、**表示にも計算にも使わない**。

### 3 枠の実測値と `has_quota`

| 枠 | entitlement | quota_remaining | remaining | credits_used | percent_remaining | has_quota | quota_reset_at |
|---|---|---|---|---|---|---|---|
| `chat` | 200 | **198.4** | 198 | 1 | **99.2** | true | 0 |
| `completions` | 2000 | 2000 | 2000 | 0 | 100 | true | 0 |
| `premium_interactions` | **0** | 0 | 0 | 0 | **0** | **false** | 0 |

13 フィールドの集合は 3 枠とも同一: `overage_count` / `overage_permitted` / `percent_remaining` / `quota_id` / `quota_remaining` / `unlimited` / `timestamp_utc` / `has_quota` / `quota_reset_at` / `token_based_billing` / `credits_used` / `remaining` / `entitlement`。**`getCurrentAuth` は `credits_used` を除く 12 フィールド。**

**`premium_interactions` は `percent_remaining` に `0` を返す。** これは「残り 0%」ではなく「**この枠は適用されない**」の意味である (`has_quota: false` / `entitlement: 0`)。

> [!注意]
> **`0` をそのままゲージに載せると「枠を使い切った」と読める。** FR-C-88 のしきい値では `90% 以上 = 危険` に該当し、**適用外の枠が真っ赤なゲージになる。**
> 適用外の判定は **`has_quota === false` を第一基準**にする。`entitlement <= 0` は 0 除算ガードとして併用する (FR-C-131)。

### `entitlement` の単位 — AI Credits 建ての可能性が高い (2 点、いずれも導出)

`quota_remaining` は**小数第 1 位までしか返らない**ため、報告値から真の消費は**区間としてしか読めない**。ローカルの `assistant_usage_events.total_nano_aiu` 合計 (= `totalNanoAiu / 10^9`) と突き合わせた。

| 時点 | サーバ側の報告 | そこから読める真の消費 | ローカル実測 | 収まるか |
|---|---|---|---|---|
| 2026-09-07 17:10 (セッション 1 件のみ完了) | `remainingPercentage` = 99.8 | (0.35, 0.45] | **0.3826** (382,635,000 nanoAIU) | **収まる** |
| 2026-09-08 (4 セッション / API 11 回すべて完了後) | `quota_remaining` = 198.4 | (1.55, 1.65] | **1.5525** (1,552,497,000 nanoAIU) | **収まる** |

**2 点とも収まる。** `token_based_billing: true` とも整合する。→ **`chat` 枠の `entitlement: 200` は「200 AI Credits」である可能性が高い。**

> [!注意]
> **2 点とも「区間に収まる」という導出であって、独立した実測ではない。**
>
> - 1 点目は `remainingPercentage` (小数第 1 位) からの逆算。生の `quota_remaining` を記録していないため、丸めの見込み方によって区間は (0.35, 0.45] から (0.3, 0.5] まで広がる。**どちらの読み方でも 0.3826 は収まる**が、区間が広い分だけ弱い
> - 2 点目は「**この期間の chat 消費はすべて隔離 `COPILOT_HOME` のセッションによる**」という前提に依存する。実 `~/.copilot` にセッションが 1 件も無いこと (測定環境) がその根拠だが、VS Code 経由の chat 消費までは排除できていない
> - **1 アカウント・1 プランのみ。** `copilot_plan: individual` / `access_type_sku: free_limited_copilot` = **無料枠**
> - 要求 付録 A.4 は「**月額プラン料金と同額のクレジットが毎月付与される**」と書くが、**このアカウントは無料プランで `entitlement: 200`**。**有料プランで `entitlement` が何を意味するかは測れていない**
>
> → **枠の名前は API が返す `quota_id` (`chat` / `completions` / `premium_interactions`) をそのまま使い、`chat` を「月次 AI Credits」と読み替えない** (ADR-0015 / INV-7)。

### `models.list` と単価 (T-0.6 から変更なし)

**`models.list({})` は呼べたが、返ったのは `"auto"` 1 件のみで `billing` フィールドが無い** (2026-09-08 の再取得でも同じ。`modelCount: 1` / `billingKeys: null`)。→ **FR-C-134 が前提にしている「単価を実行時に読む」経路が、この呼び方では成立しない。**

**`1 AI Credit = $0.01` の裏取り材料は依然として無い。** → ADR-0010 のとおり金額 ($) 換算を出さない。

ただし **`totalNanoAiu / 10^9 = AI Credits` は実測で一致した** (OQ-01)。「**AI Credit をいくら使ったか**」はローカルで正確に出せる。分かっていないのは「**1 credit が何ドルか**」の 1 点に絞られた (**分母は経路 A で取れるようになった**)。

### この数字から言えないこと

- **アカウントは 1 つ、プランも 1 つ** (`individual` / `free_limited_copilot`)。**組織 / Enterprise 管理下・有料プランで同じキー / 同じ `entitlement` の意味になるかは分からない**
- **`overage` / `overage_count` / `overage_permitted` は 3 枠とも `0` / `false`。** FR-C-90 (超過の別建て) が実際にどう返るかを **1 件も観測していない**。`overage_permitted: false` なので、このアカウントでは原理的に超過が起きない
- **`unlimited` は 3 枠とも `false`、`entitlement` に `-1` は 1 度も現れていない。** FR-C-131 が名指しする「`-1` = 無制限」は**未観測**。実在したのは `0` + `has_quota: false` の方
- **`credits_used` の定義が分からない** (chat で `1`。他の 2 つの消費量 1.6 / 2 と一致しない)。切り捨てなのか別のカウンタなのか不明。**`getCurrentAuth` には無い**フィールドなので使わない
- ~~`getCurrentAuth` の戻り値の型宣言を確認していない~~ → **2026-09-08 に `dist/generated/rpc.d.ts` で確認済み。**`quota_snapshots` / `quota_reset_date` / `quota_reset_date_utc` はいずれも**宣言あり**。`CopilotUserResponseQuotaSnapshotsChat` の 12 フィールド (`entitlement` / `quota_remaining` / `percent_remaining` / `has_quota` / `timestamp_utc` / `unlimited` / `overage_count` / `overage_permitted` / `quota_id` / `remaining` / `quota_reset_at` / `token_based_billing`) も**全部宣言あり**。`copilotUser?` は `AuthInfo` の 8 バリアント**すべて**に宣言されている。**`credits_used` だけが型に無く**、`getCurrentAuth` が返さない理由の説明がつく (詳細と帰結は ADR-0017)。**ただし全フィールドが optional (`?`) なので、防御的パースは依然として必須**
- `models.list` が `"auto"` 1 件しか返さない理由 (引数の与え方 / セッション未接続 / プラン依存) を切り分けていない。**`billing` が原理的に取れないのか、呼び方が違うだけなのかが分かっていない**
- **リセット (`2026-10-01`) をまたいだ挙動を見ていない。** FR-C-85 の除外判定は実測で確かめられていない
- Rust から SDK を叩く経路は試していない (OQ-08)。**測定はすべて Node の TypeScript バインディング**

---

## OQ-07 — サブエージェント / 委任タスクの概念があるか

**状態**: **確定** — 存在する。**縮退不要**

| 項目 | 内容 |
|---|---|
| 知りたいこと | 委任に相当する概念があるか。その親子関係が `events.jsonl` に残るか |
| 確認方法 | T-0.3。実際に委任するセッションを作って観察 |
| 影響範囲 | FR-C-21〜27、FR-C-112〜118 |

- サブエージェントは**一級の概念**。`.github/agents/<name>.agent.md` (frontmatter に `name` / `description` / `tools`) で定義し、CLI の **`task` ツール**で委任される
- 委任時の `tool.execution_start.data` = `{toolCallId, toolName:"task", arguments:{description, prompt, agent_type, name, mode:"sync"}, turnId, model}`
- `events.jsonl` に **`subagent.started` / `subagent.configured` / `subagent.completed`** が残る (各 1 件)
- `subagent.started.data` = `{toolCallId, agentName, agentDisplayName, agentDescription, model, resumable, agentType, executionMode}`。`parentId` は**親の `tool.execution_start` の `id`**
- `subagent.completed.data` = `{toolCallId, agentName, agentDisplayName, model, firstDispatchedModel, totalToolCalls, totalTokens, durationMs}`
- サブエージェントは**入れ子でターンを回す**。実測で、子が turnId 0 → 1 → 2 と 3 ターン回す間、親は 1 ターンのままだった

### 親子リンクは両側にある (FR-C-22 が満たせる)

| 候補キー | 件数 | 出現率 | ユニーク数 |
|---|---|---|---|
| `parentId` | 77 | 100.0% | 74 |
| `agentId` | 18 | 23.4% | 1 |
| `data.parentToolCallId` | 7 | 9.1% | 1 |
| `data.parentAgentTaskId` | 5 | 6.5% | 5 |
| `data.agentMetrics` | 4 | 5.2% | — |
| `data.agentName` | 2 | 2.6% | — |
| `data.agentDisplayName` | 2 | 2.6% | — |
| `data.agentDescription` | 1 | 1.3% | — |
| `data.agentType` | 1 | 1.3% | — |

- 子レコードの `agentId` (23.4%、**子側のみ**)
- 子のツール呼び出しの `data.parentToolCallId` が**親の `toolCallId`** を指す
- `session.shutdown.data.agentMetrics` が `main` と `<agentId>` をキーに `agentName` / `agentDisplayName` / `totalApiDurationMs` / `totalNanoAiu` / `modelMetrics` を持つ
- DB 側にも `assistant_usage_events.agent_id` / `parent_tool_call_id` / `initiator='sub-agent'` がある

**主キーは `toolCallId` にする** — 親の `tool.execution_start.data.toolCallId` と、子の `subagent.*.data.toolCallId` の**両方に存在する**唯一の識別子だから。判断と却下理由は ADR-0016。

### この数字から言えないこと

- **委任は 1 件しか観測していない** (`subagent.started` / `configured` / `completed` が各 1 件)。深さは 2 段 (親 → 子) のみで、**N 段の木 (FR-C-21) は再現していない**
- **`mode` / `executionMode` は `sync` しか観測していない。** FR-C-24 が警戒する「起動しただけの暫定応答」は**非同期実行でこそ起きる**はずだが、その経路を踏んでいない
- **拒否された委任 (FR-C-27 の「拒否」) を 1 件も観測していない。** 拒否時に `tool.execution_start` が書かれるのか、`toolCallId` が振られるのかが分かっていない
- 失敗 / 中断した委任も未観測

→ **OQ-11 として切り出した。T-4.9 / T-4.10 はそこが埋まるまで着手しない。**

---

## OQ-08 — SDK / REST の認証方法

**状態**: 一部判明 (SDK 側は解決、REST 側は未測定)

| 項目 | 内容 |
|---|---|
| 知りたいこと | 認証方法と、GitHub CLI の資格情報を再利用できるか。SDK が Copilot CLI の同梱を要求するか |
| 確認方法 | T-0.6 と同時 |
| 影響範囲 | FR-C-130〜141 |

- **SDK は自前ランタイムを同梱する** (`optionalDependencies` に `@github/copilot-sdk-win32-x64` 等、koffi FFI 経由)。`client.start()` が **663ms** で成功し、**Copilot CLI の別途インストールを要求しなかった**
- CLI / SDK は環境変数のトークンを **`COPILOT_GITHUB_TOKEN` → `GH_TOKEN` → `GITHUB_TOKEN`** の優先順で使う
- **`gh auth token` の値を `GH_TOKEN` に渡して CLI セッションが実行でき、SDK の `getQuota` も通った。** 現在の gh スコープは `gist, read:org, repo, workflow` で **`copilot` スコープは無い**が、それでも通った
- SDK は**引数無しでも**既存の Copilot 資格情報を自力で解決できた。`gitHubToken` を明示した場合と**戻り値が完全に同一**
- **classic PAT (`ghp_`) は非対応。** fine-grained PAT は "Copilot Requests" 権限が要る
- **`COPILOT_HOME` で設定と状態の保存先を丸ごと差し替えられる** (今回の測定はこれで実 `~/.copilot` を汚さずに行った)

> [!注意]
> **INV-2 の実裏付け。** CLI は資格情報を **OS の資格情報ストア**に保存し、**失敗時は `~/.copilot/` 配下の平文設定ファイルに落とす** (公式ヘルプの記述)。
> つまり `~/.copilot/config.json` は**平文の資格情報を含みうる**。INV-2 (認証情報を読まない・持たない・表示しない / DTO にフィールドを定義しない) と FR-C-72 は、推測ではなく**この事実に基づく**。

**トークンの値は一切測定しておらず、記録もしていない。**

### この数字から言えないこと

- **REST (経路 B) は T-0.8 で叩いた。** `gh` のトークン (`gho_` 始まり / 40 文字 / スコープ `gist, read:org, repo, workflow`) で `copilot_internal/user` は **200** を返す。**`copilot` スコープが無くても通る**
- 一方 **公開課金 API は 10/10 が 404**。うち 5 件は `gh` が `This API operation needs the "user" scope` を出しており、**404 の真因はスコープ不足** (OQ-12)
- **`gh auth refresh -s user` は実行していない。** ユーザーの資格情報 (スコープ) を変更する操作であり、調査のために勝手に広げない。**スコープを足した再測定は未実施**
- fine-grained PAT / OAuth device flow は試していない
- Rust から SDK を叩く経路 (FR-C-137 が言う言語バインディング) は試していない。**測定は Node の TypeScript バインディングで行った**。Rust 側でどう呼ぶかは段階 6 の設計事項として残る

---

## OQ-09 — 古いセッションが自動削除されるか

**状態**: 未調査 (**据え置き**)

| 項目 | 内容 |
|---|---|
| 知りたいこと | Copilot CLI / Copilot Chat が古いセッションを自動削除するか |
| 確認方法 | ドキュメント + 実データの経時観察 |
| 影響範囲 | NFR-44 |

**観測期間が 1 時間以内 / 4 回起動 / 3 セッションでは何も言えない。** 削除は観測されていないが、それは「削除されない」の根拠にならない。

**削除されるなら「累計セッション数」は原理的に取得できない。** 分かるまでは「**保持されているセッション数**」と表記して実装する (取れない値を提示しない方が常に安全)。

---

## OQ-10 — 実データ量

**状態**: 一部判明

| 項目 | 内容 |
|---|---|
| 知りたいこと | 総容量 / ファイル数 / 行数 / 最大ファイルサイズ |
| 確認方法 | T-0.1 (`tools/probe/copilot-layout.mjs`) |
| 影響範囲 | NFR-01 / 02 の目標値の確定 |

### 隔離 `COPILOT_HOME` (3 セッション)

| 項目 | 実測 |
|---|---|
| 総量 | 23 ファイル / 1.0 MB |
| 直下の構成 | `config.json` / `installed-plugins/` / `logs/` / `session-state/` / `session-store.db` (+ `-shm` + `-wal`) |
| `events.jsonl` | 3 件 / 計 291,259 バイト / 最大 142,681 バイト |
| 1 レコード最大 | 36,428 バイト |
| `session-store.db` 本体 | 4,096 バイト |
| `session-store.db-wal` | **609,792 バイト** (チェックポイント前) |

**1 往復の些細なセッション (「OK とだけ返して」) でも `events.jsonl` は 45,354 バイト。** うち `system.message` 1 レコードが 36,428 バイトを占める。→ **セッション 1 件あたりの下限がおよそ 45KB。** 数百 MB に達するにはセッションが数千件必要になる。

### ユーザーの実 `~/.copilot` (無傷)

9 ファイル。`config.json` + `ide/` (lock 3) + `logs/` (log 5)。**`session-state/` は存在しない。**

### 帰結

- **NFR-01 (5 秒以内) / NFR-02 (1 秒未満) の妥当性は、この測定では判断できない。** 現時点の測定値を根拠に「達成した」と判断しない
- **`session-store.db` 本体 4,096 バイトに対し WAL が 609,792 バイト**という比率は、DB を読む設計にした場合「本体だけコピーしても中身が無い」ことを意味する (ADR-0013 でコピー案を却下した根拠のひとつ)
- 最大レコード 36,428 バイトは、FR-C-47 (末尾 64KB のシーク読み) と T-4.12 (1 レコード 512KB 上限) の budget に収まる

### この数字から言えないこと

- 数百 MB / 数百ファイル規模での挙動は**一切未測定**。差分インデックスの実測 (段階 4 の完了条件) は、実データが溜まってから改めて行う
- 長時間セッション / 巨大なツール出力を含むセッションを作っていない

---

## OQ-11 — 拒否された委任と非同期委任の表現 (新規)

**状態**: 未調査

| 項目 | 内容 |
|---|---|
| 知りたいこと | (a) 親が `task` 呼び出しを**拒否**したとき `events.jsonl` に何が残るか (`tool.execution_start` は書かれるか、`toolCallId` は振られるか)。(b) **非同期 / バックグラウンド委任**があるか。あるなら「起動しただけの暫定応答」がどのレコードで来て、本当の完了がどれで来るか。(c) 失敗 / 中断した委任の表現 |
| 確認方法 | 委任を拒否するセッションと、非同期委任を使うセッションを隔離 `COPILOT_HOME` で作る |
| 影響範囲 | **FR-C-24 / 25 / 26 / 27**、タスク T-4.9 / T-4.10 |

OQ-07 で観測できたのは `mode: "sync"` の委任 1 件だけで、**拒否も非同期も 1 件も見ていない**。

> [!注意]
> **FR-C-24〜26 は「非同期委任が存在する」ことを前提にした要求である。** その前提自体がまだ実測されていない。
> **推測でゲートを組まない。** 実測できるまで T-4.9 / T-4.10 に着手せず、FR-C-27 の「拒否」状態は**値としては定義するが、遷移させる経路を持たない**実装にしておく (ADR-0016)。

---

## OQ-12 — 経路 B (GitHub REST) が個人アカウントで返すもの

**状態**: 一部判明 (**現行スコープでは公開課金 API が全滅。ただし経路 B への依存はもう無い**)

| 項目 | 内容 |
|---|---|
| 知りたいこと | 個人アカウントの課金 API (`/users/{USERNAME}/settings/billing/ai_credit/usage` 等) が **AI Credits の消費額と付与額**を返すか。認証に必要なスコープ。組織 / Enterprise 管理下との差 |
| 確認方法 | T-0.8 (`tools/probe/quota-rest.mjs`)。`gh api` 経由で実アカウントを叩く |
| 影響範囲 | FR-C-80 ①、FR-C-138〜142、ADR-0015 / ADR-0018 |

> [!注意]
> **このセクションの前提を訂正した。** 「**月次 AI Credits の分母は経路 B にしか無い**」は**誤りだった**。
> **`account.getCurrentAuth` (公開 SDK API、経路 A) が `entitlement` (付与額) と `quota_reset_date` (本物の月次リセット日) を返す** (OQ-06)。
> したがって **OQ-12 は段階 6 のブロッカーではなくなった。** 経路 B が埋まらなくてもゲージは分母つきで描ける。

### 測定環境

`gh` 2.95.0 / ログイン済み (`mayamaura`、keyring) / スコープ **`gist, read:org, repo, workflow`** (**`user` も `copilot` も無い**)。総リクエスト **13 件**。

### (a) 公開課金エンドポイントは全滅 — 11 件中 10 件が 404

404 の本文は 10 件すべて `{message, documentation_url, status}` の 3 キー。**`documentation_url` の中身で 2 グループにはっきり分かれる。この 2 つを混同しない。**

**グループ 1 (5 件) — 具体的なアンカー付きの `documentation_url` が返り、`gh` も scope 不足を告げる。エンドポイントは実在し、404 の真因はスコープ不足**

| path (先頭スラッシュ無し) | `documentation_url` のアンカー |
|---|---|
| `users/{login}/settings/billing/ai_credit/usage` | `#get-billing-ai-credit-usage-report-for-a-user` |
| `users/{login}/settings/billing/usage` | `#get-billing-usage-report-for-a-user` |
| `users/{login}/settings/billing/premium_request/usage` | `#get-billing-premium-request-usage-report-for-a-user` |
| `users/{login}/settings/billing/shared-storage` | `#get-shared-storage-billing-for-a-user` |
| `users/{login}/settings/billing/actions` | `#get-github-actions-billing-for-a-user` |

5 件とも `gh` の stderr に `This API operation needs the "user" scope. To request it, run: gh auth refresh -h github.com -s user`。

**グループ 2 (5 件) — 汎用トップ (`https://docs.github.com/rest`) しか返らない。パス名自体が実在しない可能性がある**

`users/{login}/settings/billing/copilot` / `users/{login}/copilot/metrics` / `user/settings/billing/usage` / `user/copilot/metrics` / `user/settings/billing/actions`。

> [!注意]
> **グループ 2 はプローブ作成時に推測で並べたパスを含む。** 「404 だった」を「そのエンドポイントは使えない」と読まないこと。
> **存在しないパスを叩いた結果と、権限不足の結果は別物である。**
>
> また **`gh auth refresh -s user` は実行していない** (ユーザーの資格情報を変更するため)。**グループ 1 の 5 件が `user` スコープで 200 を返すかは分かっていない** — 無料プラン / 個人アカウントでは 404 のまま、という可能性も残る。

### (b) 200 が返った唯一のエンドポイントは非公開の内部 API

**`copilot_internal/user` のみ 200。** トップレベル 25 キーで、`quota_snapshots` (3 枠 × 13 フィールド) / `quota_reset_date` / `quota_reset_date_utc` / `copilot_plan` / `access_type_sku` / `token_based_billing` を含む。**現在の `gh` トークン (`copilot` スコープ無し) で通る。**

**それでも実装には採らない** (ADR-0017 の却下案)。

- **公開 API ではない。** `docs.github.com/rest` に記載が無く、パスも `copilot_internal/`。互換性の約束が無い
- **`getCurrentAuth` (経路 A) との差は `credits_used` 1 フィールドだけ。** 残り 12 フィールドは同一値。**払う risk に対して得るものが 1 フィールドしかない**
- その `credits_used` (chat で `1`) は他の 2 つの消費量 (1.6 / 2) と一致せず、**定義が分からない**。使い道が無い

### (c) 組織 / Enterprise は試行対象そのものが無かった

`user/orgs` の件数 **0** (`organization_list` / `organization_login_list` とも長さ 0)。`orgs/{org}/settings/billing/usage` と `enterprises/{ent}/settings/billing/usage` は **`attempted: false`**。

**FR-C-140 が言う「組織 / Enterprise 管理下」の経路は完全に未検証。** 実装するなら、その環境を持つアカウントでの実測が先。

### 経路 B の位置づけ (更新)

| 用途 | 従来の想定 | T-0.8 の後 |
|---|---|---|
| 月次の付与額 (ゲージの分母) | **経路 B にしか無い** | **経路 A (`getCurrentAuth.entitlement`) で取れる** |
| 月次リセット日 | 経路 B | **経路 A (`quota_reset_date` = `2026-10-01`)** |
| アカウント全体の月次消費額 (過去 24 か月の履歴) | 経路 B | **依然として経路 B のみ。現行スコープでは取れない** (FR-C-138) |
| 日次の `ai_credits_used` | 経路 B | 同上 (FR-C-139) |
| 組織 / Enterprise の枠 | 経路 B | 同上 (FR-C-140)。**未検証** |

→ **経路 B は「あれば嬉しい履歴」の経路に降格する。** ゲージの分母を埋めるための必須経路ではなくなった (ADR-0018)。

### この数字から言えないこと

- **`user` スコープを足したときに 200 が返るかは未測定。** 404 の真因がスコープ不足であることは `gh` のメッセージと `documentation_url` から言えるが、**通った先に何が入っているかは分からない**
- グループ 2 の 5 件は **実在しないパスを叩いただけかもしれない**。エンドポイント名の正しさを検証していない
- **組織 / Enterprise は 1 度も叩いていない** (試行対象が 0 件)
- レート制限 (FR-C-142) には触れていない。13 リクエストでは何も起きない
- **測定は 1 アカウント・無料プラン (`free_limited_copilot`) のみ。** 有料プランなら公開課金 API が 200 を返す可能性は否定できない
- `copilot_internal/user` が**いつまで同じ形で返るか**は分からない (非公開エンドポイント)

---

## 新しく分かったことの書き方

1. 「観測」に**数字か実物**を書く (「〜のようだ」は観測ではない)
2. 状態を更新する。**根拠が足りる範囲だけ格上げする**
3. 「**この数字から言えないこと**」を必ず残す
4. 影響を受ける ADR ([設計判断](decisions.html)) と タスク ([実装計画](implementation-plan.html)) を更新する
5. 実データで見つかった例外パターンは [テスト戦略](test-strategy.html) 7 節にテストケースとして追記する (NFR-51)
