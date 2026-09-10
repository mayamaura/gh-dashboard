#!/usr/bin/env node
// T-0.8 / OQ-12 / OQ-06 の残差 — 経路 B (GitHub REST 課金 API) が個人アカウントで
// 何を返すかを、`gh` CLI に認証を委ねて測る (読み取り専用プローブ)。
//
// ★ このスクリプトは **読み取り (GET) しかしない** (INV-1)。POST / PATCH / DELETE は一切呼ばない。
// ★ `gh auth login` / `gh auth refresh` は絶対に実行しない (ユーザーの資格情報を変更するため)。
// ★ トークンの値は自分のプロセスに読み込まない (INV-2)。認証はすべて `gh api` に委ねる。
// ★ 認証情報に相当するフィールドは値を出さない。キー名だけを報告する。
// ★ 出力は tools/probe-out/ に置く。**コミットしない**。
// ★ Node 標準モジュールのみ。リポジトリの package.json には依存を足さない。
//
//   node tools/probe/quota-rest.mjs
//   node tools/probe/quota-rest.mjs --json
//
// 合否判定はしない。数字と事実を出すだけ (NFR-52)。

import { existsSync, mkdirSync, writeFileSync, readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const execFileP = promisify(execFile);

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const OUT_DIR = join(ROOT, 'tools', 'probe-out');
const JSON_ONLY = process.argv.includes('--json');

// quota-sdk.mjs の SECRET_KEY 切り分けをそのまま流用する。
// 完全一致で伏せるキー名
const SECRET_KEY_EXACT = /^(headers?|authorization|token|secret|password|passwd|cookie|credentials?)$/i;
// 部分一致で伏せるキー名。`token_based_billing` のような課金方式の boolean は**伏せない**
// (トークン「数」や課金方式の話であり、秘密ではない。指示文どおり)。
const SECRET_KEY_PART =
  /(auth[-_]?(token|info|header)|access[-_]?token|refresh[-_]?token|id[-_]?token|session[-_]?token|bearer[-_]?token|(github|gh|copilot|oauth|pat)[-_]?token|api[-_]?key|apikey|client[-_]?secret|private[-_]?key|password|passphrase|credential)/i;

const SECRET_KEYS = { test: (k) => SECRET_KEY_EXACT.test(k) || SECRET_KEY_PART.test(k) };

function isSecretKey(k) {
  return SECRET_KEYS.test(k);
}

/** analytics_tracking_id のような「識別子だが秘密ではない」値は長さだけ出す */
function lengthOnly(v) {
  if (v === null || v === undefined) return v;
  return { type: typeof v, length: String(v).length };
}

/** オブジェクトを「キー名と型」だけに変換する。SECRET_KEYS に該当するキーは値を伏せる。 */
function shapeOnly(obj) {
  if (obj === null || obj === undefined) return obj;
  if (Array.isArray(obj)) {
    return { type: 'array', length: obj.length, itemType: obj.length ? typeof obj[0] : null };
  }
  if (typeof obj === 'object') {
    const out = {};
    for (const [k, v] of Object.entries(obj)) {
      if (isSecretKey(k)) {
        out[k] = '(redacted: value not shown, INV-2)';
      } else if (v === null || v === undefined) {
        out[k] = String(v);
      } else if (typeof v === 'object') {
        out[k] = shapeOnly(v);
      } else {
        out[k] = typeof v;
      }
    }
    return out;
  }
  return typeof obj;
}

/** トップレベルのキー一覧を返す。秘密キーは名前だけ残し、通常キーも値は呼び出し側の判断で出す。 */
function topLevelKeys(obj) {
  if (obj === null || typeof obj !== 'object') return [];
  return Object.keys(obj);
}

/**
 * 2 つの時刻文字列が同じ瞬間を指すか。
 * SDK は `2026-09-08T11:58:34.381Z`、REST は `2026-09-08T04:58:34.381-07:00` のように
 * **同じ瞬間を別のオフセット表記**で返すため、文字列比較では一致しない。
 */
function sameInstant(a, b) {
  if (typeof a !== 'string' || typeof b !== 'string') return null;
  const ta = Date.parse(a);
  const tb = Date.parse(b);
  if (Number.isNaN(ta) || Number.isNaN(tb)) return null;
  return ta === tb;
}

function maskTokenLike(str) {
  if (typeof str !== 'string') return str;
  return str.replace(/\b(gh[a-z]?_[A-Za-z0-9]{10,}|[A-Za-z0-9._-]{24,})\b/g, (m) => `${m.slice(0, 4)}...(redacted, len=${m.length})`);
}

const report = {
  probedAt: new Date().toISOString(),
  ghAvailable: false,
  ghVersion: null,
  ghAuthStatus: null,
  login: null,
  requestCount: 0,
  endpoints: {},
  copilotInternalUser: null,
  quotaSnapshotAnalysis: null,
  sdkComparison: null,
  notes: [],
};

/**
 * `gh api <path>` を叩く。パスの先頭にスラッシュを付けてはいけない
 * (Git Bash / MSYS が `/users/...` をファイルシステムパスとして書き換え、
 *  `invalid API endpoint: "C:/Program Files/Git/users/..."` のように失敗する実測済みの落とし穴)。
 * GET のみ。--include で HTTP ステータスを取れる形にする。
 */
async function ghApiGet(path) {
  report.requestCount += 1;
  try {
    // gh api はデフォルトで GET。エラー時 (4xx/5xx) は非ゼロ終了かつ stdout に本文、
    // stderr にスコープ不足等のメッセージが乗る実測仕様に合わせて拾う。
    const { stdout, stderr } = await execFileP('gh', ['api', path, '--include'], { maxBuffer: 8 * 1024 * 1024 });
    return parseGhIncludeOutput(stdout, stderr, 0);
  } catch (e) {
    // execFile は非ゼロ終了時に例外を投げるが、stdout/stderr は e.stdout / e.stderr に載る
    const stdout = (e && e.stdout) || '';
    const stderr = (e && e.stderr) || '';
    return parseGhIncludeOutput(stdout, stderr, e && typeof e.code === 'number' ? e.code : null);
  }
}

function parseGhIncludeOutput(stdout, stderr, exitCode) {
  // --include はレスポンスヘッダ + 空行 + 本文の形。複数の HTTP レスポンス行が混ざりうるので
  // 最後の "HTTP/x.y NNN" 行をステータスとして採用する。
  const lines = stdout.split(/\r?\n/);
  let status = null;
  let bodyStartIdx = -1;
  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(/^HTTP\/\d(?:\.\d)?\s+(\d{3})/);
    if (m) {
      status = Number(m[1]);
      bodyStartIdx = -1; // ヘッダブロックが変わるたびリセットし、直後の空行を再探索
      for (let j = i + 1; j < lines.length; j++) {
        if (lines[j] === '') {
          bodyStartIdx = j + 1;
          break;
        }
      }
    }
  }
  const bodyText = bodyStartIdx >= 0 ? lines.slice(bodyStartIdx).join('\n').trim() : '';
  let body = null;
  let bodyParseError = null;
  if (bodyText) {
    try {
      body = JSON.parse(bodyText);
    } catch (e) {
      bodyParseError = maskTokenLike(String(e && e.message));
    }
  }
  const stderrMasked = maskTokenLike(String(stderr || '')).trim();
  return {
    status,
    exitCode,
    documentationUrl: body && typeof body === 'object' ? body.documentation_url || null : null,
    message: body && typeof body === 'object' ? body.message || null : null,
    bodyParseError,
    bodyIsObject: body !== null && typeof body === 'object',
    bodyTopLevelKeyCount: body && typeof body === 'object' && !Array.isArray(body) ? Object.keys(body).length : null,
    stderrHasScopeMessage: /needs the .* scope/i.test(stderrMasked),
    stderrScopeMessage: /needs the .* scope/i.test(stderrMasked)
      ? stderrMasked.split('\n').find((l) => /needs the .* scope/i.test(l)) || null
      : null,
    body, // 本文全体。SECRET_KEYS でない限りそのまま扱う (この後の各処理で伏せる)
  };
}

async function checkGh() {
  try {
    const { stdout } = await execFileP('gh', ['--version']);
    report.ghAvailable = true;
    report.ghVersion = stdout.split('\n')[0].trim();
  } catch (e) {
    report.ghAvailable = false;
    report.notes.push('gh 未導入または未導入相当のため測定できません。');
    return false;
  }
  try {
    const { stdout, stderr } = await execFileP('gh', ['auth', 'status']);
    report.ghAuthStatus = { ok: true, summary: maskTokenLike(String(stdout || stderr)) };
  } catch (e) {
    report.ghAuthStatus = { ok: false, error: maskTokenLike(String((e && e.stderr) || (e && e.message))) };
    report.notes.push('gh 未認証のため測定できません。');
    return false;
  }
  return true;
}

async function getLogin() {
  try {
    report.requestCount += 1;
    const { stdout } = await execFileP('gh', ['api', 'user', '--jq', '.login']);
    return stdout.trim();
  } catch (e) {
    report.notes.push(`login の取得に失敗しました: ${maskTokenLike(String((e && e.stderr) || (e && e.message)))}`);
    return null;
  }
}

async function getOrgList() {
  try {
    report.requestCount += 1;
    const { stdout } = await execFileP('gh', ['api', 'user/orgs', '--jq', '[.[].login]']);
    return JSON.parse(stdout.trim() || '[]');
  } catch (e) {
    return null;
  }
}

function analyzeQuotaSnapshots(snapshots) {
  if (!snapshots || typeof snapshots !== 'object') return null;
  const keys = Object.keys(snapshots);
  const details = {};
  const timestamps = [];
  const resetAts = [];
  for (const k of keys) {
    const s = snapshots[k];
    if (!s || typeof s !== 'object') {
      details[k] = null;
      continue;
    }
    const entitlement = s.entitlement;
    const quotaRemaining = s.quota_remaining;
    const remaining = s.remaining;
    const creditsUsed = s.credits_used;
    const percentRemaining = s.percent_remaining;

    let computedPercentFromQuotaRemaining = null;
    let percentMatchesComputed = null;
    if (typeof entitlement === 'number' && entitlement > 0 && typeof quotaRemaining === 'number') {
      computedPercentFromQuotaRemaining = (quotaRemaining / entitlement) * 100;
      if (typeof percentRemaining === 'number') {
        percentMatchesComputed = Math.abs(computedPercentFromQuotaRemaining - percentRemaining) < 1e-9;
      }
    }

    const consumedFromQuotaRemaining =
      typeof entitlement === 'number' && typeof quotaRemaining === 'number' ? entitlement - quotaRemaining : null;
    const consumedFromRemaining =
      typeof entitlement === 'number' && typeof remaining === 'number' ? entitlement - remaining : null;

    details[k] = {
      fieldsPresent: Object.keys(s),
      entitlement,
      quota_remaining: quotaRemaining,
      remaining,
      credits_used: creditsUsed,
      percent_remaining: percentRemaining,
      has_quota: s.has_quota,
      unlimited: s.unlimited,
      overage_count: s.overage_count,
      overage_permitted: s.overage_permitted,
      timestamp_utc: s.timestamp_utc,
      quota_reset_at: s.quota_reset_at,
      token_based_billing: s.token_based_billing,
      computedPercentFromQuotaRemaining,
      percentMatchesComputedExactly: percentMatchesComputed,
      threeWayConsumed: {
        entitlementMinusQuotaRemaining: consumedFromQuotaRemaining,
        entitlementMinusRemaining: consumedFromRemaining,
        credits_used: creditsUsed,
        allThreeEqual:
          consumedFromQuotaRemaining !== null && consumedFromRemaining !== null && typeof creditsUsed === 'number'
            ? Math.round(consumedFromQuotaRemaining) === consumedFromRemaining && consumedFromRemaining === creditsUsed
            : null,
      },
    };
    if (typeof s.timestamp_utc === 'string') timestamps.push(s.timestamp_utc);
    if (s.quota_reset_at !== undefined) resetAts.push(s.quota_reset_at);
  }
  const uniqueTimestamps = [...new Set(timestamps)];
  return {
    keys,
    keyCount: keys.length,
    details,
    allTimestampsIdentical: uniqueTimestamps.length === 1 && timestamps.length === keys.length,
    uniqueTimestampCount: uniqueTimestamps.length,
    quotaResetAtValues: resetAts,
    allQuotaResetAtAreZero: resetAts.length > 0 && resetAts.every((v) => v === 0),
  };
}

function compareWithSdk(copilotInternalUser, quotaAnalysis) {
  const sdkPath = join(OUT_DIR, 'quota-sdk.json');
  if (!existsSync(sdkPath)) {
    return { available: false, note: 'quota-sdk.json が無いため比較できません。' };
  }
  let sdkReport;
  try {
    sdkReport = JSON.parse(readFileSync(sdkPath, 'utf8'));
  } catch (e) {
    return { available: false, note: `quota-sdk.json の読み込みに失敗しました: ${maskTokenLike(String(e && e.message))}` };
  }

  const sdkQuota =
    (sdkReport.quota && sdkReport.quota.noArgs && sdkReport.quota.noArgs.details) ||
    (sdkReport.quota && sdkReport.quota.withGhToken && sdkReport.quota.withGhToken.details) ||
    null;
  if (!sdkQuota) {
    return { available: false, note: 'quota-sdk.json に quota.noArgs / quota.withGhToken の詳細が無いため比較できません。' };
  }

  const restKeys = quotaAnalysis ? quotaAnalysis.keys : [];
  const sdkKeys = Object.keys(sdkQuota);
  const commonKeys = restKeys.filter((k) => sdkKeys.includes(k));

  const perKey = {};
  for (const k of commonKeys) {
    const rest = quotaAnalysis.details[k];
    const sdk = sdkQuota[k];
    perKey[k] = {
      entitlement: { sdk: sdk.entitlementRequests, rest: rest.entitlement, equal: sdk.entitlementRequests === rest.entitlement },
      used: {
        sdk_usedRequests: sdk.usedRequests,
        rest_entitlementMinusRemaining: rest.threeWayConsumed.entitlementMinusRemaining,
        rest_entitlementMinusQuotaRemaining: rest.threeWayConsumed.entitlementMinusQuotaRemaining,
        rest_credits_used: rest.threeWayConsumed.credits_used,
      },
      remainingPercentage: {
        sdk: sdk.remainingPercentage,
        rest_percent_remaining: rest.percent_remaining,
        equal:
          typeof sdk.remainingPercentage === 'number' && typeof rest.percent_remaining === 'number'
            ? sdk.remainingPercentage === rest.percent_remaining
            : null,
      },
      // SDK の `resetDate` の比較相手は `quota_reset_at` ではなく `timestamp_utc`。
      // 実測では両者が同一時刻を指す (別プロセス・別経路での同時取得で一致)。
      // `quota_reset_at` は 3 枠とも 0 で、比較相手にならない。
      resetDate: {
        sdk_resetDate: sdk.resetDate,
        rest_timestamp_utc: rest.timestamp_utc,
        equalAsInstant: sameInstant(sdk.resetDate, rest.timestamp_utc),
        rest_quota_reset_at: rest.quota_reset_at,
      },
    };
  }

  const restResetDate = copilotInternalUser ? copilotInternalUser.quota_reset_date : null;
  const restResetDateUtc = copilotInternalUser ? copilotInternalUser.quota_reset_date_utc : null;
  const sdkResetDateSample = commonKeys.length ? sdkQuota[commonKeys[0]].resetDate : null;
  const restTimestampSample = commonKeys.length ? quotaAnalysis.details[commonKeys[0]].timestamp_utc : null;

  return {
    available: true,
    restKeys,
    sdkKeys,
    commonKeys,
    perKey,
    resetDateComparison: {
      rest_quota_reset_date: restResetDate,
      rest_quota_reset_date_utc: restResetDateUtc,
      sdk_resetDate_sample: sdkResetDateSample,
      rest_timestamp_utc_sample: restTimestampSample,
      sdkResetDateEqualsRestTimestampUtc: sameInstant(sdkResetDateSample, restTimestampSample),
      sdkProbedAt: sdkReport.probedAt,
      note:
        'REST の quota_reset_date は月初固定日。SDK の resetDate はそれとは別物で、REST の timestamp_utc (スナップショットの観測時刻) と同じ時刻を指す。ここでは値を並べて一致を計算するだけで、判定はしない。',
    },
  };
}

async function main() {
  const okGh = await checkGh();
  if (!okGh) {
    finish();
    return;
  }

  const login = await getLogin();
  report.login = login;
  if (!login) {
    report.notes.push('login が取得できなかったため、login を含むエンドポイントは試行しません。');
  }

  const orgList = await getOrgList();
  report.organizationList = {
    fetched: orgList !== null,
    count: Array.isArray(orgList) ? orgList.length : null,
    isEmpty: Array.isArray(orgList) ? orgList.length === 0 : null,
  };

  // 試行するエンドポイント一覧。先頭スラッシュを付けない (Git Bash の落とし穴)。
  const endpointSpecs = [];
  if (login) {
    endpointSpecs.push(
      { key: 'user_ai_credit_usage', path: `users/${login}/settings/billing/ai_credit/usage` },
      { key: 'user_billing_usage', path: `users/${login}/settings/billing/usage` },
      { key: 'user_billing_copilot', path: `users/${login}/settings/billing/copilot` },
      { key: 'user_copilot_metrics_by_login', path: `users/${login}/copilot/metrics` },
      { key: 'user_billing_shared_storage', path: `users/${login}/settings/billing/shared-storage` },
      { key: 'user_billing_actions', path: `users/${login}/settings/billing/actions` },
      { key: 'user_premium_request_usage', path: `users/${login}/settings/billing/premium_request/usage` },
    );
  }
  endpointSpecs.push(
    { key: 'user_settings_billing_usage_root', path: 'user/settings/billing/usage' },
    { key: 'user_copilot_metrics_root', path: 'user/copilot/metrics' },
    { key: 'user_settings_billing_actions_root', path: 'user/settings/billing/actions' },
    { key: 'copilot_internal_user', path: 'copilot_internal/user' },
  );

  for (const spec of endpointSpecs) {
    const result = await ghApiGet(spec.path);
    const { body, ...rest } = result;
    report.endpoints[spec.key] = { path: spec.path, ...rest };
    if (spec.key === 'copilot_internal_user' && result.status === 200 && body && typeof body === 'object') {
      report.copilotInternalUser = extractCopilotInternalUser(body);
    }
  }

  // 組織 / Enterprise エンドポイントは対象組織が無いため実行不能なことを数字で示す。
  report.orgBillingEndpoint = {
    attempted: Array.isArray(orgList) ? orgList.length > 0 : false,
    reason: Array.isArray(orgList) && orgList.length === 0 ? 'organization_list が空のため試行対象の組織が無い' : null,
    orgCount: Array.isArray(orgList) ? orgList.length : null,
  };
  report.enterpriseBillingEndpoint = {
    attempted: false,
    reason: '対象 Enterprise が無い (organization_list が空で、gh からは enterprise slug を得る経路も無い)',
  };

  if (report.copilotInternalUser && report.copilotInternalUser.quota_snapshots_raw) {
    report.quotaSnapshotAnalysis = analyzeQuotaSnapshots(report.copilotInternalUser.quota_snapshots_raw);
    // quota_snapshots_raw は解析専用の内部データなので最終出力からは落とし、解析結果だけ残す
    delete report.copilotInternalUser.quota_snapshots_raw;
  }

  report.sdkComparison = compareWithSdk(report.copilotInternalUser, report.quotaSnapshotAnalysis);

  finish();
}

function extractCopilotInternalUser(body) {
  const out = { topLevelKeys: Object.keys(body), topLevelKeyCount: Object.keys(body).length };
  for (const [k, v] of Object.entries(body)) {
    if (isSecretKey(k)) {
      out[k] = '(redacted: value not shown, INV-2)';
      continue;
    }
    if (k === 'analytics_tracking_id') {
      out[k] = lengthOnly(v);
      continue;
    }
    if (k === 'quota_snapshots') {
      out.quota_snapshots_raw = v; // 解析用に一時保持。最終出力前に削除する。
      out.quota_snapshots_shape = shapeOnly(v);
      continue;
    }
    if (k === 'endpoints') {
      out.endpoints_keys = v && typeof v === 'object' ? Object.keys(v) : null;
      continue;
    }
    if (k === 'organization_list' || k === 'organization_login_list') {
      out[k] = { length: Array.isArray(v) ? v.length : null, value: Array.isArray(v) && v.length <= 5 ? v : '(長いため省略)' };
      continue;
    }
    if (v === null || v === undefined || typeof v !== 'object') {
      out[k] = v;
    } else {
      out[k] = shapeOnly(v);
    }
  }
  return out;
}

function finish() {
  mkdirSync(OUT_DIR, { recursive: true });
  const outPath = join(OUT_DIR, 'quota-rest.json');
  writeFileSync(outPath, JSON.stringify(report, null, 2), 'utf8');

  if (JSON_ONLY) {
    console.log(JSON.stringify(report, null, 2));
    return;
  }

  console.log('=== GitHub REST 課金 API プローブ (経路 B, 読み取り専用) ===\n');
  console.log('gh 導入        :', report.ghAvailable);
  console.log('gh バージョン  :', report.ghVersion);
  if (report.ghAuthStatus) console.log('gh 認証状態    :', JSON.stringify(report.ghAuthStatus, null, 2));
  console.log('login          :', report.login);
  console.log('総リクエスト数 :', report.requestCount, '(目安 15 回以内)');
  if (report.organizationList) {
    console.log('\n--- organization_list (user/orgs) ---');
    console.log(JSON.stringify(report.organizationList, null, 2));
  }
  console.log('\n--- エンドポイント別 status ---');
  console.log(JSON.stringify(report.endpoints, null, 2));
  if (report.orgBillingEndpoint) {
    console.log('\n--- 組織課金エンドポイント (対象なし) ---');
    console.log(JSON.stringify(report.orgBillingEndpoint, null, 2));
  }
  if (report.enterpriseBillingEndpoint) {
    console.log('\n--- Enterprise 課金エンドポイント (対象なし) ---');
    console.log(JSON.stringify(report.enterpriseBillingEndpoint, null, 2));
  }
  if (report.copilotInternalUser) {
    console.log('\n--- copilot_internal/user (200) ---');
    console.log(JSON.stringify(report.copilotInternalUser, null, 2));
  }
  if (report.quotaSnapshotAnalysis) {
    console.log('\n--- quota_snapshots 解析 ---');
    console.log(JSON.stringify(report.quotaSnapshotAnalysis, null, 2));
  }
  if (report.sdkComparison) {
    console.log('\n--- SDK (quota-sdk.json) との突き合わせ ---');
    console.log(JSON.stringify(report.sdkComparison, null, 2));
  }
  if (report.notes.length) {
    console.log('\n--- 注記 ---');
    for (const n of report.notes) console.log('*', n);
  }
  console.log('\n結果を書き出しました:', outPath, '(コミットしないこと)');
}

main().catch((e) => {
  report.notes.push(`予期しない例外で終了しました: ${maskTokenLike(String(e && e.stack ? e.stack : e))}`);
  finish();
});
