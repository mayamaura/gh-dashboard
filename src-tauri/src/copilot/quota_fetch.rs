//! 利用枠の取得経路。**ここだけが外向き通信を起こしてよい** (INV-3)。
//!
//! | 経路 | 実体 | 状態 |
//! |---|---|---|
//! | A (SDK) | Node 橋渡し + `account.getCurrentAuth` | 実装済み (ADR-0017 / ADR-0028) |
//! | B (REST) | GitHub 課金 API | **実装しない** (ADR-0018) |
//! | C (推定) | 差分インデックスのトークン集計 | 実装済み (FR-C-92 / 93) |
//!
//! **2 秒ポーリングから呼ばない** (FR-C-135 / INV-4)。呼び出し口は
//! `commands::quota_get` だけ (タブ表示 / 更新ボタン / 長周期タイマー)。
//!
//! 対応要求: FR-C-83〜93 / FR-C-130〜144 / NFR-40 / NFR-43

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::copilot::quota::{
    self, FetchOutcome, QuotaGauge, RawObservation, ESTIMATE_BASELINE_DAYS, ESTIMATE_WINDOW_HOURS,
};

/// 取得の下限間隔 (FR-C-135 / 142)。`force` で無視できる。
pub const MIN_FETCH_INTERVAL_MS: i64 = 5 * 60 * 1000;

/// 経路 C の TTL (FR-C-93)。全表走査を伴うため。
pub const ESTIMATE_TTL_MS: i64 = 60 * 1000;

/// 橋渡しの上限時間。**超えたら殺して「取得不可」にする。**
/// 待ち続けると更新ボタンが固まる (NFR-07)
const BRIDGE_TIMEOUT: Duration = Duration::from_secs(25);

/// 標準出力に混ざる SDK / CLI のログと区別するための目印。
/// これが無いと「たまたま JSON に見える行」を結果として読んでしまう
const SENTINEL: &str = "@@GHDQ@@";

// ---------------------------------------------------------------- 橋渡し

/// Node に**標準入力から**渡す橋渡しスクリプト (ADR-0028)。
///
/// `node --input-type=module -e <script>` は実測で**何も出力せずに 0 で終了した**
/// (Node 24.18.0)。引数経由は Windows のエスケープ規則も絡むので、
/// **標準入力に流す**。長さ制限もエスケープも無い。
///
/// **このスクリプトも INV-2 の対象。** 読んでよいのは
/// `copilotUser.quota_snapshots` / `quota_reset_date_utc` / `copilot_plan` /
/// `access_type_sku` とその配下だけ。`authInfo.*` や `copilotUser.login` /
/// `analytics_tracking_id` / `endpoints` / `organization_*` は**読まない・出さない**。
/// 例外メッセージはトークン様の文字列を伏せてから出す。
const BRIDGE_SCRIPT: &str = r#"
const mask = (s) => String(s == null ? '' : s).replace(/\b(gh[a-z]?_[A-Za-z0-9]{10,}|[A-Za-z0-9._-]{24,})\b/g, (m) => {
  // 識別子チェーン (client.rpc.account.getCurrentAuth = 33 字) を伏せない。
  // 伏せると "X is not a function" が読めず、API 差分を診断できなくなる。
  // トークンは区切りを挟まない長い塊なので、最長セグメントで見分ける
  if (!/^gh[a-z]?_/.test(m) && Math.max(...m.split(/[._-]/).map((p) => p.length)) < 20) return m;
  return m.slice(0, 4) + '...(redacted,len=' + m.length + ')';
});
let done = false;
const emit = (o) => { if (done) return; done = true; process.stdout.write('@@GHDQ@@' + JSON.stringify(o) + '\n', () => process.exit(0)); };
const run = async () => {
  const { pathToFileURL } = await import('node:url');
  const sdkPath = process.env.GHD_SDK_PATH;
  if (!sdkPath) return emit({ ok: false, error: 'SDK のパスが渡されていません' });
  const mod = await import(pathToFileURL(sdkPath).href);
  if (!mod.CopilotClient) return emit({ ok: false, error: 'SDK に CopilotClient がありません (API 変更の可能性)' });
  const cli = process.env.GHD_CLI_PATH;
  let opts = {};
  if (cli) {
    opts = (mod.RuntimeConnection && typeof mod.RuntimeConnection.forStdio === 'function')
      ? { connection: mod.RuntimeConnection.forStdio({ path: cli }) }
      : { cliPath: cli };
  }
  const client = new mod.CopilotClient(opts);
  await client.start();
  const acct = (client.rpc && client.rpc.account) || {};
  const notes = [];
  let out = null;
  // 経路 A-1: getCurrentAuth。情報量が多い (小数の消費 / 本物のリセット日 / quota_id)
  if (typeof acct.getCurrentAuth === 'function') {
    try {
      const auth = await acct.getCurrentAuth();
      const u = (auth && auth.authInfo && auth.authInfo.copilotUser) || null;
      if (!u) {
        notes.push('getCurrentAuth: copilotUser が返りませんでした');
      } else {
        const snaps = u.quota_snapshots || {};
        const quotas = {};
        for (const k of Object.keys(snaps)) {
          const s = snaps[k] || {};
          quotas[k] = {
            quota_id: typeof s.quota_id === 'string' ? s.quota_id : k,
            entitlement: s.entitlement,
            quota_remaining: s.quota_remaining,
            percent_remaining: s.percent_remaining,
            has_quota: s.has_quota,
            unlimited: s.unlimited,
            timestamp_utc: s.timestamp_utc,
          };
        }
        const rd = u.quota_reset_date_utc;
        out = {
          ok: true, api: 'account.getCurrentAuth',
          copilot_plan: u.copilot_plan || null, access_type_sku: u.access_type_sku || null,
          quota_reset_at_ms: (typeof rd === 'string' && Number.isFinite(Date.parse(rd))) ? Date.parse(rd) : null,
          quotas,
        };
      }
    } catch (e) { notes.push('getCurrentAuth: ' + mask((e && e.message) || e)); }
  } else {
    notes.push('getCurrentAuth: このSDKには無い');
  }
  // 経路 A-2: getQuota (FR-C-130 が第一候補とする公開 API)。getCurrentAuth が
  // 無い SDK が実在する。劣化射影だが付与額と残率は取れる (OQ-06)
  if (!out && typeof acct.getQuota === 'function') {
    try {
      const q = await acct.getQuota();
      const snaps = (q && q.quotaSnapshots) || {};
      const quotas = {};
      for (const k of Object.keys(snaps)) {
        const s = snaps[k] || {};
        const ent = s.entitlementRequests;
        const pct = s.remainingPercentage;
        quotas[k] = {
          quota_id: k,
          entitlement: ent,
          // usedRequests は整数丸めで率と整合しない。残率から小数で戻す (ADR-0015)
          quota_remaining: (typeof ent === 'number' && typeof pct === 'number') ? ent * pct / 100 : undefined,
          percent_remaining: pct,
          has_quota: s.hasQuota,
          unlimited: s.unlimited,
        };
      }
      // resetDate はリセット日ではなく観測時刻 (OQ-06)。リセット日はこの API では取れない
      out = { ok: true, api: 'account.getQuota', copilot_plan: null, access_type_sku: null, quota_reset_at_ms: null, quotas };
    } catch (e) { notes.push('getQuota: ' + mask((e && e.message) || e)); }
  } else if (!out) {
    notes.push('getQuota: このSDKには無い');
  }
  try { await client.stop(); } catch (_) { }
  if (out) return emit(out);
  // 何が使えるかを添える。名前だけなので認証情報は含まない (INV-2)
  const names = Object.keys(acct).filter((k) => typeof acct[k] === 'function');
  return emit({ ok: false, error: notes.join(' / ') + ' -- account が持つ関数: ' + (names.join(',') || '(なし)') });
};
run().catch((e) => emit({ ok: false, error: mask((e && e.message) || e) }));
"#;

#[derive(Debug, Deserialize)]
struct BridgeOutput {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    /// 実際に使えた SDK の API 名。SDK のバージョンで変わるので記録する
    #[serde(default)]
    api: Option<String>,
    #[serde(default)]
    quota_reset_at_ms: Option<i64>,
    #[serde(default)]
    quotas: HashMap<String, BridgeQuota>,
}

/// **全フィールドが欠落しうる前提** (NFR-23)。消えた枠は `unavailable` に落ちる。
#[derive(Debug, Deserialize)]
struct BridgeQuota {
    #[serde(default)]
    quota_id: Option<String>,
    #[serde(default)]
    entitlement: Option<f64>,
    #[serde(default)]
    quota_remaining: Option<f64>,
    #[serde(default)]
    percent_remaining: Option<f64>,
    #[serde(default)]
    has_quota: Option<bool>,
    #[serde(default)]
    unlimited: Option<bool>,
}

// ---------------------------------------------------------------- 経路の可用性

#[derive(Debug, Clone)]
pub struct RouteStatus {
    pub available: bool,
    pub reason: String,
    pub how_to_fix: Option<String>,
}

impl RouteStatus {
    fn ok(reason: &str) -> Self {
        Self {
            available: true,
            reason: reason.to_string(),
            how_to_fix: None,
        }
    }
    fn down(reason: String, how_to_fix: Option<String>) -> Self {
        Self {
            available: false,
            reason,
            how_to_fix,
        }
    }
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "available": self.available,
            "reason": self.reason,
            "how_to_fix": self.how_to_fix,
        })
    }
}

impl Default for RouteStatus {
    fn default() -> Self {
        Self::down("まだ取得を試みていません".to_string(), None)
    }
}

/// 経路 B は v1 の対象外 (ADR-0018)。**「実装したが常に失敗する」形にしない。**
pub fn rest_status() -> RouteStatus {
    RouteStatus::down(
        "未実装 (v1 では対象外 / ADR-0018)".to_string(),
        Some(
            "公開課金 API は実測で 10/10 が 404 でした。user スコープを持つアカウント、\
             または組織 / Enterprise 管理下で実測できたら再開します"
                .to_string(),
        ),
    )
}

// ---------------------------------------------------------------- プロセス内キャッシュ

/// **DB に入れない** (INV-5 / DR-02)。`quota_samples` は FR-C-94 の時系列専用で、これとは別。
#[derive(Default)]
pub struct QuotaCache {
    /// 枠ごとの直近観測。FR-C-86 の `observed_at` はここで据え置く
    pub last: HashMap<String, RawObservation>,
    /// 直近に組み立てたゲージ。下限間隔中はこれを返す (NFR-07)
    pub gauges: Vec<QuotaGauge>,
    pub last_fetch_at: Option<i64>,
    pub sdk: RouteStatus,
    pub estimate_route: RouteStatus,
    /// 経路 C の TTL キャッシュ: (率, 計算時刻, 値が変わった時刻)
    pub estimate: Option<(f64, i64, i64)>,
}

// ---------------------------------------------------------------- 経路 A

/// SDK 橋渡しの入力。見つからなければ `Err(RouteStatus)`。
fn locate() -> Result<(PathBuf, Option<PathBuf>), RouteStatus> {
    let sdk = sdk_candidates().into_iter().find(|p| p.is_file());
    let Some(sdk) = sdk else {
        return Err(RouteStatus::down(
            "Copilot SDK が見つかりません".to_string(),
            Some(
                "GitHub Copilot CLI をインストールしてください (winget install GitHub.Copilot)。\
                 別の場所にある場合は環境変数 COPILOT_SDK_PATH に copilot-sdk/index.js を指定してください"
                    .to_string(),
            ),
        ));
    };
    Ok((sdk, cli_path()))
}

/// SDK の探索順。**実機で見つかった配置を上から並べる** (ADR-0028)。
fn sdk_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();

    // 1. 明示指定 (開発・非標準配置)
    if let Some(raw) = std::env::var_os("COPILOT_SDK_PATH") {
        let p = PathBuf::from(raw);
        if p.extension().is_some_and(|e| e == "js" || e == "mjs") {
            out.push(p);
        } else {
            // 同梱 CLI は index.js、npm 版は dist/index.js と配置が違う
            out.push(p.join("index.js"));
            out.push(p.join("dist").join("index.js"));
        }
    }

    if let Some(local) = dirs::data_local_dir() {
        // 2. CLI 本体が展開する場所: %LOCALAPPDATA%/copilot/pkg/<platform>/<version>/copilot-sdk/index.js
        //
        // ponytail: バージョン順ではなく mtime 降順で選ぶ。"1.0.100" < "1.0.79" に
        // なる辞書順を避けるため。semver 比較が要るほど差が出たら差し替える
        let pkg = local.join("copilot").join("pkg");
        let mut versioned: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
        for platform in read_dirs(&pkg) {
            for version in read_dirs(&platform) {
                let js = version.join("copilot-sdk").join("index.js");
                if js.is_file() {
                    let t = version
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    versioned.push((t, js));
                }
            }
        }
        versioned.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
        out.extend(versioned.into_iter().map(|(_, p)| p));

        // 3. デスクトップアプリ同梱
        out.push(
            local
                .join("Programs")
                .join("GitHub Copilot")
                .join("copilot-sdk")
                .join("index.js"),
        );
    }

    // 4. npm でグローバル導入した場合
    if let Some(roaming) = dirs::data_dir() {
        out.push(
            roaming
                .join("npm")
                .join("node_modules")
                .join("@github")
                .join("copilot-sdk")
                .join("dist")
                .join("index.js"),
        );
    }

    out
}

fn read_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

/// CLI 実行ファイル。SDK は既定で `@github/copilot` を npm から解決しようとするが、
/// winget / インストーラ版にはその npm パッケージが無いため**明示的に渡す**必要がある。
fn cli_path() -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os("COPILOT_CLI_PATH") {
        let p = PathBuf::from(raw);
        if p.is_file() {
            return Some(p);
        }
    }
    which_in_path(if cfg!(windows) { "copilot.exe" } else { "copilot" })
}

fn which_in_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe))
        .find(|p| p.is_file())
}

fn bridge_command() -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("node");
    cmd.arg("--input-type=module")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // CREATE_NO_WINDOW: 5 分ごとにコンソールが点滅しない
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    cmd
}

/// スクリプトを標準入力に流して結果を待つ。
async fn run_bridge(mut cmd: tokio::process::Command) -> std::io::Result<std::process::Output> {
    use tokio::io::AsyncWriteExt;
    let mut child = cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(BRIDGE_SCRIPT.as_bytes()).await?;
        stdin.shutdown().await?; // EOF を渡さないと node が読み終わらない
    }
    child.wait_with_output().await
}

/// 経路 A を 1 回だけ叩く。**ネットワークと外部プロセスを伴う** (INV-4 の対象外経路)。
///
/// T-6.6 / FR-C-141: **認証情報は一切扱わない。** 実測 (2026-09-12) で
/// `getCurrentAuth()` は引数なしで成功した — CLI 自身の資格情報で解決される。
/// `gh auth token` も OS 資格情報ストアも読まない。持たなければ漏れない (NFR-31)。
pub async fn fetch_sdk() -> Result<(HashMap<String, RawObservation>, Option<i64>), RouteStatus> {
    let (sdk, cli) = locate()?;

    let mut cmd = bridge_command();
    cmd.env("GHD_SDK_PATH", &sdk);
    if let Some(cli) = &cli {
        cmd.env("GHD_CLI_PATH", cli);
    }

    let out = match tokio::time::timeout(BRIDGE_TIMEOUT, run_bridge(cmd)).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(RouteStatus::down(
                "Node.js が見つかりません".to_string(),
                Some(
                    "利用枠の取得には Node.js が要ります (SDK は JavaScript 実装のため)。\
                     https://nodejs.org からインストールしてください"
                        .to_string(),
                ),
            ));
        }
        Ok(Err(e)) => {
            return Err(RouteStatus::down(
                format!("橋渡しプロセスを起動できません: {e}"),
                None,
            ))
        }
        Err(_) => {
            return Err(RouteStatus::down(
                format!("{} 秒で応答がありません", BRIDGE_TIMEOUT.as_secs()),
                Some("ネットワークと Copilot CLI の状態を確認してください".to_string()),
            ))
        }
    };

    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed = parse_bridge_stdout(&stdout).ok_or_else(|| {
        // stderr は BRIDGE_SCRIPT の mask() を経由していない生ログ (INV-2)。
        // 画面向けの reason に含めず、ログにだけ残す
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: String = stderr.lines().rev().take(3).collect::<Vec<_>>().join(" / ");
        tracing::warn!(stderr_tail = %truncate(&tail, 300), "橋渡しの出力を読めません");
        RouteStatus::down(
            "橋渡しの出力を読めませんでした (詳細はログを参照)".to_string(),
            Some("copilot コマンドが動作するか確認してください".to_string()),
        )
    })?;

    if !parsed.ok {
        let msg = parsed.error.unwrap_or_else(|| "原因不明".to_string());
        return Err(RouteStatus::down(
            format!("SDK 呼び出しに失敗しました: {}", truncate(&msg, 300)),
            Some("copilot コマンドでログイン済みか確認してください (copilot login)".to_string()),
        ));
    }

    // どちらの API で取れたかは SDK のバージョン差の手がかりになる (OQ-06)
    tracing::info!(
        api = %parsed.api.as_deref().unwrap_or("不明"),
        quotas = parsed.quotas.len(),
        "利用枠を取得しました"
    );

    let reset_at = parsed.quota_reset_at_ms;
    let mut observations = HashMap::new();
    for (key, q) in parsed.quotas {
        // 枠の名前は API が返す quota_id をそのまま使う (ADR-0015 / FR-C-134)
        let kind = q.quota_id.unwrap_or(key);
        let (Some(entitlement), Some(remaining)) = (q.entitlement, q.quota_remaining) else {
            // フィールドが消えたらその枠だけ落とす。他の枠は生かす (NFR-23 / NFR-24)
            tracing::warn!(quota = %kind, "entitlement / quota_remaining が返りません");
            continue;
        };
        observations.insert(
            kind,
            RawObservation {
                // ADR-0015: 小数の quota_remaining から出す。remaining / credits_used と混ぜない
                used: entitlement - remaining,
                entitlement,
                remaining_pct: q.percent_remaining,
                reset_at,
                // 消えていたら「適用外」と断定しない (既定 true)
                has_quota: q.has_quota.unwrap_or(true),
                unlimited: q.unlimited.unwrap_or(false),
                // 呼び出し側が settle_observed_at で据え置く (FR-C-86)
                observed_at: 0,
            },
        );
    }

    if observations.is_empty() {
        return Err(RouteStatus::down(
            "返った枠が 1 つもありません".to_string(),
            Some("SDK の戻り値の形が変わった可能性があります".to_string()),
        ));
    }
    Ok((observations, reset_at))
}

/// 目印付きの行だけを結果として読む (純粋)。
fn parse_bridge_stdout(stdout: &str) -> Option<BridgeOutput> {
    stdout
        .lines()
        .rev()
        .find_map(|l| l.split_once(SENTINEL))
        .and_then(|(_, json)| serde_json::from_str(json.trim()).ok())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

// ---------------------------------------------------------------- 経路 C

/// インデックス済みのトークンを 1 時間バケットで集計する (FR-C-92)。
///
/// **`sessions` ではなく `turn_index` を見る** — 時間帯別に割れるのはこちらだけ。
fn hourly_token_buckets(conn: &rusqlite::Connection, since_ms: i64) -> rusqlite::Result<Vec<(i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT timestamp_ms / 3600000 AS bucket, SUM(input_tokens + output_tokens)
         FROM turn_index
         WHERE timestamp_ms IS NOT NULL AND timestamp_ms >= ?1
         GROUP BY bucket",
    )?;
    let rows = stmt.query_map([since_ms], |r| Ok((r.get(0)?, r.get::<_, i64>(1)?)))?;
    rows.collect()
}

/// 経路 C。60 秒 TTL のキャッシュ付き (FR-C-93)。
///
/// 戻り値は `degrade` の `estimate` 引数にそのまま渡せる形。
pub fn estimate(
    conn: &rusqlite::Connection,
    cache: &mut QuotaCache,
    now: i64,
) -> Option<(f64, String, i64)> {
    if let Some((pct, computed_at, observed_at)) = cache.estimate {
        if now.saturating_sub(computed_at) < ESTIMATE_TTL_MS {
            return Some((pct, estimate_basis(), observed_at));
        }
    }

    let since = now - ESTIMATE_BASELINE_DAYS * 24 * 3_600_000;
    let buckets = match hourly_token_buckets(conn, since) {
        Ok(b) => b,
        Err(e) => {
            cache.estimate_route = RouteStatus::down(format!("索引を読めません: {e}"), None);
            return None;
        }
    };

    let Some(pct) = quota::estimate_usage_pct(&buckets, now / 3_600_000, ESTIMATE_WINDOW_HOURS)
    else {
        cache.estimate_route = RouteStatus::down(
            "推定の母数になる記録がありません".to_string(),
            Some("差分インデックスを実行してください".to_string()),
        );
        cache.estimate = None;
        return None;
    };

    // FR-C-86: 率が変わったときだけ観測時刻を進める。TTL 切れの再計算では進めない
    let observed_at = match cache.estimate {
        Some((prev, _, prev_observed)) if prev == pct => prev_observed,
        _ => now,
    };
    cache.estimate = Some((pct, now, observed_at));
    cache.estimate_route = RouteStatus::ok("インデックス済みのトークン集計から算出");
    Some((pct, estimate_basis(), observed_at))
}

fn estimate_basis() -> String {
    format!(
        "直近 {ESTIMATE_WINDOW_HOURS} 時間のトークン合計 ÷ 過去 {ESTIMATE_BASELINE_DAYS} 日の最大 {ESTIMATE_WINDOW_HOURS} 時間"
    )
}

// ---------------------------------------------------------------- 組み立て

/// 枠ごとに独立に降格させる (FR-C-84 / ADR-0015)。
///
/// 経路 A が返さなかった枠に推定を混ぜない。**経路 A 自体が落ちたときだけ**、
/// 枠名を騙らない 1 本のゲージとして推定 (または取得不可) を出す。
pub fn build_gauges(
    sdk: Result<(HashMap<String, RawObservation>, Option<i64>), RouteStatus>,
    estimate: Option<(f64, String, i64)>,
    cache: &mut QuotaCache,
    now: i64,
) -> Vec<QuotaGauge> {
    match sdk {
        Ok((observations, _reset_at)) => {
            // 使えた API は SDK のバージョンで変わる (getCurrentAuth / getQuota)。
            // ここで名前を騙らない。実際に使った API 名はログに出る
            cache.sdk = RouteStatus::ok("Copilot SDK");
            // 実値があるので推定は走らせていない。「使えない」と書かない (NFR-43)
            cache.estimate_route = RouteStatus::down(
                "実値が取れているため使用しません".to_string(),
                Some("経路 A が使えなくなったときだけ推定に降格します".to_string()),
            );
            let mut kinds: Vec<String> = observations.keys().cloned().collect();
            kinds.sort(); // 並び順を安定させる (毎回入れ替わると読めない)
            let mut next_last = HashMap::new();
            let gauges = kinds
                .into_iter()
                .map(|kind| {
                    let mut obs = observations[&kind].clone();
                    obs.observed_at = quota::settle_observed_at(cache.last.get(&kind), &obs, now);
                    next_last.insert(kind.clone(), obs.clone());
                    // 枠名は quota_id のまま。「月次 AI Credits」等に読み替えない (ADR-0015)
                    quota::degrade(
                        &kind,
                        &kind,
                        Some(FetchOutcome::Ok(obs)),
                        None, // 経路 B: ADR-0018
                        None, // 実値がある枠に推定を混ぜない (FR-C-144)
                        now,
                    )
                })
                .collect();
            cache.last = next_last;
            gauges
        }
        Err(status) => {
            let outcome = FetchOutcome::Failed {
                reason: status.reason.clone(),
                how_to_fix: status.how_to_fix.clone(),
            };
            cache.sdk = status;
            cache.last.clear();
            let rest = rest_status();
            vec![quota::degrade(
                "unknown",
                "利用枠",
                Some(outcome),
                Some(FetchOutcome::Failed {
                    reason: rest.reason,
                    how_to_fix: rest.how_to_fix,
                }),
                estimate,
                now,
            )]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn raw(used: f64, ent: f64) -> RawObservation {
        RawObservation {
            used,
            entitlement: ent,
            remaining_pct: None,
            reset_at: Some(NOW + 86_400_000),
            has_quota: true,
            unlimited: false,
            observed_at: 0,
        }
    }

    #[test]
    fn sentinel_line_is_picked_out_of_noisy_stdout() {
        let stdout = "some sdk log\n{\"ok\":true}\n@@GHDQ@@{\"ok\":true,\"quotas\":{}}\ntrailing\n";
        let parsed = parse_bridge_stdout(stdout).expect("目印付きの行を読む");
        assert!(parsed.ok);
        assert!(
            parse_bridge_stdout("just noise\n{\"ok\":true}\n").is_none(),
            "目印の無い JSON らしき行を結果にしない"
        );
    }

    /// 2026-09-12 実測の形をそのまま読む (NFR-51)
    #[test]
    fn parses_the_shape_observed_on_2026_09_12() {
        let json = concat!(
            r#"@@GHDQ@@{"ok":true,"copilot_plan":"individual","quota_reset_at_ms":1790812800000,"quotas":{"#,
            r#""chat":{"quota_id":"chat","entitlement":0,"quota_remaining":0,"percent_remaining":100,"has_quota":true,"unlimited":true},"#,
            r#""premium_interactions":{"quota_id":"premium_interactions","entitlement":1500,"quota_remaining":1500,"percent_remaining":100,"has_quota":true,"unlimited":false}}}"#
        );
        let p = parse_bridge_stdout(json).expect("読める");
        assert!(p.ok);
        assert_eq!(p.quotas.len(), 2);
        assert_eq!(p.quotas["chat"].unlimited, Some(true));
        assert_eq!(p.quotas["premium_interactions"].entitlement, Some(1500.0));
    }

    /// NFR-23: フィールドが全部消えても panic しない
    #[test]
    fn missing_fields_do_not_panic() {
        let p = parse_bridge_stdout("@@GHDQ@@{\"ok\":true,\"quotas\":{\"chat\":{}}}").expect("読める");
        let c = &p.quotas["chat"];
        assert_eq!(c.entitlement, None);
        assert_eq!(c.has_quota, None);
    }

    /// FR-C-86: 2 回続けて同じ値なら観測時刻を進めない
    #[test]
    fn repeated_identical_fetch_does_not_refresh_observed_at() {
        let mut cache = QuotaCache::default();
        let obs: HashMap<String, RawObservation> = [("chat".to_string(), raw(1.6, 200.0))].into();

        let first = build_gauges(Ok((obs.clone(), None)), None, &mut cache, NOW);
        let t1 = observed_at_of(&first[0]);
        assert_eq!(t1, Some(NOW));

        let second = build_gauges(Ok((obs, None)), None, &mut cache, NOW + 600_000);
        assert_eq!(
            observed_at_of(&second[0]),
            t1,
            "値が同じなら 10 分後の取得でも観測時刻は据え置き"
        );

        let moved: HashMap<String, RawObservation> = [("chat".to_string(), raw(2.4, 200.0))].into();
        let third = build_gauges(Ok((moved, None)), None, &mut cache, NOW + 1_200_000);
        assert_eq!(observed_at_of(&third[0]), Some(NOW + 1_200_000));
    }

    fn observed_at_of(g: &QuotaGauge) -> Option<i64> {
        match g.origin {
            quota::QuotaSource::Actual { observed_at, .. } => Some(observed_at),
            _ => None,
        }
    }

    /// FR-C-144 / ADR-0015: 実値が取れた枠に推定を混ぜない
    #[test]
    fn actual_gauges_never_carry_an_estimate() {
        let mut cache = QuotaCache::default();
        let obs: HashMap<String, RawObservation> = [("chat".to_string(), raw(1.6, 200.0))].into();
        let gauges = build_gauges(
            Ok((obs, None)),
            Some((42.0, "推定".to_string(), NOW)),
            &mut cache,
            NOW,
        );
        assert!(matches!(
            gauges[0].origin,
            quota::QuotaSource::Actual { .. }
        ));
    }

    /// FR-C-83 / ADR-0018: 経路 A が落ちたら理由と対処が付いて出る。経路 B は常に未実装
    #[test]
    fn sdk_failure_falls_through_to_estimate_then_unavailable() {
        let mut cache = QuotaCache::default();
        let fail = || {
            Err(RouteStatus::down(
                "Copilot SDK が見つかりません".to_string(),
                Some("インストールしてください".to_string()),
            ))
        };

        let g = build_gauges(fail(), Some((42.0, "推定".to_string(), NOW)), &mut cache, NOW);
        assert!(matches!(g[0].origin, quota::QuotaSource::Estimated { .. }));
        assert!(!cache.sdk.available);

        let g = build_gauges(fail(), None, &mut cache, NOW);
        match &g[0].origin {
            quota::QuotaSource::Unavailable { reason, how_to_fix } => {
                assert!(reason.contains("見つかりません"));
                assert!(reason.contains("ADR-0018"), "経路 B の未実装も理由に出る");
                assert!(how_to_fix.is_some());
            }
            other => panic!("Unavailable のはず: {other:?}"),
        }
        assert_eq!(g[0].used_pct, None, "取れない値を 0 で埋めない");
    }

    /// 経路 B は「実装したが常に失敗する」ではなく「未実装」と言う (ADR-0018)
    #[test]
    fn rest_route_is_reported_as_not_implemented() {
        let s = rest_status();
        assert!(!s.available);
        assert!(s.reason.contains("未実装"));
        assert!(s.how_to_fix.is_some(), "再開条件を出す (FR-C-83)");
    }

    /// FR-C-93: TTL 内は再走査しない
    #[test]
    fn estimate_is_cached_for_its_ttl() {
        let conn = crate::db::open_in_memory().expect("in-memory db");
        let mut cache = QuotaCache {
            estimate: Some((33.0, NOW, NOW - 5_000)),
            ..Default::default()
        };

        let (pct, _, observed_at) = estimate(&conn, &mut cache, NOW + 1_000).expect("キャッシュを返す");
        assert_eq!(pct, 33.0);
        assert_eq!(observed_at, NOW - 5_000, "TTL 内は観測時刻も動かさない");

        // TTL 超過 + 空の索引 → 推定できない。0% と断定しない (NFR-43)
        assert!(estimate(&conn, &mut cache, NOW + ESTIMATE_TTL_MS + 1).is_none());
        assert!(!cache.estimate_route.available);
    }

    /// 実機の経路 A を通しで叩く手動プローブ (NFR-53)。**ネットワークに出るので既定では走らない**。
    ///
    ///   cargo test --manifest-path src-tauri/Cargo.toml probe_real_quota -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "ネットワークと実アカウントを使う。手動で実行する"]
    async fn probe_real_quota() {
        println!("SDK 候補:");
        for p in sdk_candidates() {
            println!("  {} exists={}", p.display(), p.is_file());
        }
        println!("CLI: {:?}", cli_path());

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let mut cache = QuotaCache::default();

        // 経路 C も実 DB で回す (索引済みなら数字が出る)
        let est = crate::db::default_db_path()
            .filter(|p| p.exists())
            .and_then(|p| crate::db::open(&p).ok())
            .and_then(|conn| estimate(&conn, &mut cache, now));
        println!("推定: {est:?} / 経路の状態: {:?}", cache.estimate_route);

        let sdk = fetch_sdk().await;
        match &sdk {
            Ok((obs, reset_at)) => println!("SDK ok: {} 枠 / reset_at={reset_at:?}", obs.len()),
            // 取れないことは失敗ではない。理由と対処が出ていれば設計どおり (FR-C-83)
            Err(s) => println!("SDK 取得不可: {} / 対処: {:?}", s.reason, s.how_to_fix),
        }

        println!("--- 組み立て後のゲージ ---");
        for g in build_gauges(sdk, est, &mut cache, now) {
            println!(
                "  {}: used={:?} ent={:?} pct={:?} overage={:?} unlimited={} has_quota={} reset_at={:?} origin={:?}",
                g.kind, g.used, g.entitlement, g.used_pct, g.overage, g.unlimited, g.has_quota, g.reset_at, g.origin
            );
            assert!(
                !(g.unlimited && g.used_pct.is_some()),
                "無制限の枠に率を出してはいけない"
            );
            assert!(
                !(!g.has_quota && g.used_pct.is_some()),
                "適用外の枠に率を出してはいけない"
            );
        }
    }

    /// 経路 C を**実データ**で回す手動プローブ。実 `~/.copilot` を使い捨て DB に
    /// 索引してから推定を出す (読み取り専用 / INV-1)。
    ///
    ///   cargo test --manifest-path src-tauri/Cargo.toml probe_real_estimate -- --ignored --nocapture
    #[test]
    #[ignore = "実 ~/.copilot を全走査する。手動で実行する"]
    fn probe_real_estimate() {
        let Some(home) = crate::copilot::indexer::default_home() else {
            println!("COPILOT_HOME を解決できません");
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let db = std::sync::Arc::new(std::sync::Mutex::new(
            crate::db::open_in_memory().expect("in-memory db"),
        ));
        let stats = crate::copilot::indexer::run(&db, &home, now, |_| {});
        println!(
            "索引: files={} records={} skipped={}",
            stats.total_files, stats.records_ingested, stats.skipped_lines
        );

        let conn = db.lock().unwrap();
        let since = now - ESTIMATE_BASELINE_DAYS * 24 * 3_600_000;
        let buckets = hourly_token_buckets(&conn, since).expect("集計できる");
        println!("直近 30 日で記録のある時間帯: {} 個", buckets.len());
        let mut cache = QuotaCache::default();
        match estimate(&conn, &mut cache, now) {
            Some((pct, basis, observed_at)) => {
                println!("推定 {pct:.1}% / 根拠={basis} / observed_at={observed_at}");
                assert!((0.0..=100.0).contains(&pct), "0〜100 にクランプ (FR-C-92)");
            }
            None => println!("推定できず: {:?}", cache.estimate_route),
        }
    }

    /// 探索先が 1 つも無くても落ちない。見つからないことは正常な結果
    #[test]
    fn sdk_discovery_never_panics() {
        let _ = sdk_candidates();
        let _ = cli_path();
    }

    /// 橋渡しスクリプトが**実際に node で動き、1 行だけ結果を返す**ことを確かめる。
    ///
    /// `-e` 経由は実測で無言終了した。この経路が壊れると「利用枠が永久に取得不可」に
    /// なるのに、型検査でも他のテストでも捕まらない。
    /// SDK パスを渡さないのでネットワークには出ない。node が無い環境では読み飛ばす。
    #[tokio::test]
    async fn bridge_script_runs_and_answers_exactly_once() {
        let mut cmd = bridge_command();
        cmd.env_remove("GHD_SDK_PATH").env_remove("GHD_CLI_PATH");
        let Ok(out) = run_bridge(cmd).await else {
            eprintln!("node が無いので読み飛ばします");
            return;
        };
        let stdout = String::from_utf8_lossy(&out.stdout);
        let parsed = parse_bridge_stdout(&stdout).unwrap_or_else(|| {
            panic!(
                "橋渡しが結果を返しません。stdout={stdout:?} stderr={:?}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        assert!(!parsed.ok);
        assert!(parsed.error.unwrap_or_default().contains("SDK"));
        assert_eq!(
            stdout.matches(SENTINEL).count(),
            1,
            "結果行は 1 本だけ。2 本出ると読む側が取り違える"
        );
    }

    /// 偽 SDK を書いて橋渡しを走らせる。ネットワークには出ない。
    /// node が無い環境では `None` (呼び出し側が読み飛ばす)
    async fn bridge_with_fake_sdk(name: &str, source: &str) -> Option<BridgeOutput> {
        let sdk = std::env::temp_dir().join(format!("ghd-fake-sdk-{name}.mjs"));
        std::fs::write(&sdk, source).expect("偽 SDK を書ける");

        let mut cmd = bridge_command();
        cmd.env("GHD_SDK_PATH", &sdk).env_remove("GHD_CLI_PATH");
        let out = run_bridge(cmd).await.ok()?;
        let _ = std::fs::remove_file(&sdk);

        let stdout = String::from_utf8_lossy(&out.stdout);
        Some(parse_bridge_stdout(&stdout).unwrap_or_else(|| {
            panic!(
                "橋渡しが結果を返しません。stdout={stdout:?} stderr={:?}",
                String::from_utf8_lossy(&out.stderr)
            )
        }))
    }

    /// **2026-09-17 実測 (Copilot Business 契約 PC)**: `getCurrentAuth` を持たない
    /// SDK が実在する (`client.rpc.account.getCurrentAuth is not a function`)。
    /// FR-C-130 が第一候補とする `getQuota` に落ちて取得できること。
    #[tokio::test]
    async fn falls_back_to_get_quota_when_get_current_auth_is_absent() {
        let source = r#"
export class CopilotClient {
  constructor() {
    this.rpc = { account: { getQuota: async () => ({ quotaSnapshots: {
      chat: { entitlementRequests: 200, remainingPercentage: 99.2, usedRequests: 2, hasQuota: true },
    } }) } };
  }
  async start() {}
  async stop() {}
}
"#;
        let Some(parsed) = bridge_with_fake_sdk("getquota", source).await else {
            eprintln!("node が無いので読み飛ばします");
            return;
        };
        assert!(parsed.ok, "getQuota に降りて取得できる: {:?}", parsed.error);
        assert_eq!(parsed.api.as_deref(), Some("account.getQuota"));
        let chat = &parsed.quotas["chat"];
        assert_eq!(chat.entitlement, Some(200.0));
        assert_eq!(chat.percent_remaining, Some(99.2));
        assert_eq!(chat.has_quota, Some(true));
        // 整数丸めの usedRequests (2) を使わず、残率から小数で戻す (ADR-0015)
        let rem = chat.quota_remaining.expect("残量が率から戻る");
        assert!((rem - 198.4).abs() < 1e-9, "200 × 99.2% = 198.4 のはず: {rem}");
        // リセット日は getQuota からは取れない。resetDate は観測時刻 (OQ-06)
        assert_eq!(parsed.quota_reset_at_ms, None);
    }

    /// **2026-09-17 実測**: 画面に `clie...(redacted,len=33) is not a function` としか
    /// 出ず、原因 (`client.rpc.account.getCurrentAuth`) が読めなかった。
    /// 識別子チェーンは伏せない。トークンは伏せる (INV-2)。
    #[tokio::test]
    async fn masking_keeps_identifier_chains_and_hides_tokens() {
        let source = r#"
export class CopilotClient {
  constructor() { this.rpc = { account: {} }; }
  async start() { throw new Error('client.rpc.account.getCurrentAuth is not a function token=ghp_0123456789abcdefghijABCDEFGHIJ'); }
  async stop() {}
}
"#;
        let Some(parsed) = bridge_with_fake_sdk("masking", source).await else {
            eprintln!("node が無いので読み飛ばします");
            return;
        };
        assert!(!parsed.ok);
        let err = parsed.error.expect("理由が返る");
        assert!(
            err.contains("client.rpc.account.getCurrentAuth"),
            "識別子チェーンを伏せると診断できない: {err}"
        );
        assert!(
            !err.contains("0123456789abcdefghij"),
            "トークンは伏せる (INV-2): {err}"
        );
        assert!(err.contains("redacted"), "伏せた印は残す: {err}");
    }

    /// どちらの API も無い SDK では、何が使えるかを添えて返す (FR-C-83)。
    /// 関数名だけなので認証情報は含まない (INV-2)
    #[tokio::test]
    async fn unknown_account_api_reports_what_is_available() {
        let source = r#"
export class CopilotClient {
  constructor() { this.rpc = { account: { getAllUsers: async () => ({}) } }; }
  async start() {}
  async stop() {}
}
"#;
        let Some(parsed) = bridge_with_fake_sdk("unknown-api", source).await else {
            eprintln!("node が無いので読み飛ばします");
            return;
        };
        assert!(!parsed.ok);
        let err = parsed.error.expect("理由が返る");
        assert!(err.contains("getAllUsers"), "使える関数名を添える: {err}");
    }
}
