#!/usr/bin/env node
// T-0.6 / OQ-06 / OQ-08 — Copilot SDK の account.getQuota / getCurrentAuth / models.list を
// 実アカウントで叩き、返る形と認証経路を確かめる (読み取り専用プローブ)。
//
// ★ このスクリプトは **読み取りしかしない** (INV-1)。session.create やプロンプト送信は行わない。
// ★ 認証情報に相当するフィールドは**値を出さない** (INV-2)。キー名だけを報告する。
//   `gh auth token` の値も出力しない (長さと先頭 4 文字だけ)。
// ★ 出力は tools/probe-out/ に置く。**コミットしない**。
// ★ リポジトリの package.json には依存を足さない。SDK は COPILOT_SDK_PATH か既定のスクラッチパッドから
//   動的 import で解決する。
//
//   node tools/probe/quota-sdk.mjs
//   node tools/probe/quota-sdk.mjs --json
//
// 合否判定はしない。数字と事実を出すだけ (NFR-52)。

import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const execFileP = promisify(execFile);

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const OUT_DIR = join(ROOT, 'tools', 'probe-out');
const JSON_ONLY = process.argv.includes('--json');

// SDK の場所。**リポジトリの package.json には足さない** (依存を増やすには先に ADR が要る / NFR-10)。
// 環境変数 → リポジトリ配下の使い捨てディレクトリ → リポジトリの node_modules の順に探す。
// 見つからなければエラーにせず「SDK 未導入」として正常終了する。
//
//   # このプローブを回す準備 (tools/probe-sdk/ は .gitignore 済み)
//   mkdir -p tools/probe-sdk && cd tools/probe-sdk
//   npm init -y && npm install @github/copilot-sdk
//
//   # 別の場所に入れてある場合
//   COPILOT_SDK_PATH=/path/to/node_modules/@github/copilot-sdk node tools/probe/quota-sdk.mjs
const SDK_PATH_CANDIDATES = [
  process.env.COPILOT_SDK_PATH,
  join(ROOT, 'tools', 'probe-sdk', 'node_modules', '@github', 'copilot-sdk'),
  join(ROOT, 'node_modules', '@github', 'copilot-sdk'),
].filter(Boolean);

const sdkPath = SDK_PATH_CANDIDATES.find((p) => existsSync(p)) || SDK_PATH_CANDIDATES[0];

/** 値を絶対に出さないフィールド名 (INV-2 / FR-C-72) */
// 完全一致で伏せるキー名
const SECRET_KEY_EXACT = /^(headers?|authorization|token|secret|password|passwd|cookie|credentials?)$/i;
// 部分一致で伏せるキー名。`authToken` / `accessToken` / `authInfo` のような複合名を拾う。
// **`tokenCount` / `totalTokens` / `token_based_billing` のようなトークン「数」のキーは伏せない** —
// これらは利用量表示の中核であり、秘密ではない (FR-C-132)。
const SECRET_KEY_PART =
  /(auth[-_]?(token|info|header)|access[-_]?token|refresh[-_]?token|id[-_]?token|session[-_]?token|bearer[-_]?token|(github|gh|copilot|oauth|pat)[-_]?token|api[-_]?key|apikey|client[-_]?secret|private[-_]?key|password|passphrase|credential)/i;

/** 値を絶対に出さないフィールド名か (INV-2 / FR-C-72) */
const SECRET_KEYS = { test: (k) => SECRET_KEY_EXACT.test(k) || SECRET_KEY_PART.test(k) };

function redactKeys(obj) {
  if (obj === null || typeof obj !== 'object') return { keys: [], redacted: [] };
  const keys = [];
  const redacted = [];
  for (const k of Object.keys(obj)) {
    if (SECRET_KEYS.test(k)) redacted.push(k);
    else keys.push(k);
  }
  return { keys, redacted };
}

/** authInfo / authErrors のようなオブジェクトを「キー名と型」だけに変換する。値は出さない。 */
function shapeOnly(obj) {
  if (obj === null || obj === undefined) return obj;
  if (Array.isArray(obj)) {
    return { type: 'array', length: obj.length, itemType: obj.length ? typeof obj[0] : null };
  }
  if (typeof obj === 'object') {
    const out = {};
    for (const [k, v] of Object.entries(obj)) {
      if (SECRET_KEYS.test(k)) {
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

function maskTokenLike(str) {
  if (typeof str !== 'string') return str;
  // token 様の文字列 (gh*_ / ghp_ / gho_ / 長い英数字列) を伏せる
  return str.replace(/\b(gh[a-z]?_[A-Za-z0-9]{10,}|[A-Za-z0-9._-]{24,})\b/g, (m) => `${m.slice(0, 4)}...(redacted, len=${m.length})`);
}

const report = {
  probedAt: new Date().toISOString(),
  sdkPathTried: sdkPath,
  sdkResolved: false,
  sdkResolveError: null,
  clientStart: null,
  currentAuth: null,
  quota: {},
  authRoutes: {},
  modelsList: null,
  notes: [],
};

async function loadSdk() {
  if (!existsSync(sdkPath)) {
    report.sdkResolveError = `SDK ディレクトリが見つかりません: ${sdkPath}`;
    return null;
  }
  const indexJsPath = join(sdkPath, 'dist', 'index.js');
  if (!existsSync(indexJsPath)) {
    report.sdkResolveError = `dist/index.js が見つかりません: ${indexJsPath}`;
    return null;
  }
  try {
    const mod = await import(pathToFileURL(indexJsPath).href);
    report.sdkResolved = true;
    return mod;
  } catch (e) {
    report.sdkResolveError = `import 失敗: ${maskTokenLike(String(e && e.message))}`;
    return null;
  }
}

async function getGhAuthToken() {
  try {
    const { stdout } = await execFileP('gh', ['auth', 'token']);
    const token = stdout.trim();
    return {
      ok: true,
      length: token.length,
      prefix4: token.slice(0, 4),
    };
  } catch (e) {
    return {
      ok: false,
      error: maskTokenLike(String(e && e.message)),
    };
  }
}

function classifyError(e) {
  const msg = maskTokenLike(String((e && e.message) || e));
  return {
    name: e && e.name,
    message: msg,
  };
}

async function analyzeQuota(result) {
  const snapshots = result && result.quotaSnapshots ? result.quotaSnapshots : {};
  const keys = Object.keys(snapshots);
  const details = {};
  for (const k of keys) {
    const s = snapshots[k];
    if (!s) {
      details[k] = null;
      continue;
    }
    const entitlement = s.entitlementRequests;
    const used = s.usedRequests;
    const remainingPct = s.remainingPercentage;
    let computedRemainingPct = null;
    let matchesRemainingPercentage = null;
    if (typeof entitlement === 'number' && entitlement > 0 && typeof used === 'number') {
      computedRemainingPct = ((entitlement - used) / entitlement) * 100;
      if (typeof remainingPct === 'number') {
        matchesRemainingPercentage = Math.abs(computedRemainingPct - remainingPct) < 0.5;
      }
    }
    details[k] = {
      isUnlimitedEntitlement: s.isUnlimitedEntitlement,
      entitlementRequests: s.entitlementRequests,
      usedRequests: s.usedRequests,
      usageAllowedWithExhaustedQuota: s.usageAllowedWithExhaustedQuota,
      remainingPercentage: s.remainingPercentage,
      overage: s.overage,
      overageAllowedWithExhaustedQuota: s.overageAllowedWithExhaustedQuota,
      resetDate: s.resetDate,
      resetDateType: typeof s.resetDate,
      allFieldNames: Object.keys(s),
      computedRemainingPercentFromEntitlementMinusUsed: computedRemainingPct,
      matchesReportedRemainingPercentage: matchesRemainingPercentage,
    };
  }
  return {
    quotaSnapshotKeys: keys,
    quotaSnapshotKeyCount: keys.length,
    details,
  };
}

async function main() {
  const mod = await loadSdk();
  if (!mod) {
    report.notes.push('SDK 未導入のため測定できません。');
    finish();
    return;
  }

  const { CopilotClient } = mod;
  if (!CopilotClient) {
    report.notes.push('dist/index.js に CopilotClient が見つかりません。SDK の API が変わった可能性があります。');
    finish();
    return;
  }

  let client;
  const t0 = Date.now();
  try {
    client = new CopilotClient();
    await client.start();
    report.clientStart = { ok: true, ms: Date.now() - t0 };
  } catch (e) {
    report.clientStart = { ok: false, ms: Date.now() - t0, error: classifyError(e) };
    report.notes.push('クライアント起動に失敗したため、以降の呼び出しは試みません。');
    finish();
    return;
  }

  // 2. account.getCurrentAuth — 構造のみ (値は出さない)
  try {
    const auth = await client.rpc.account.getCurrentAuth();
    report.currentAuth = {
      ok: true,
      topLevelKeys: Object.keys(auth || {}),
      authInfoShape: auth && auth.authInfo ? shapeOnly(auth.authInfo) : auth && 'authInfo' in auth ? null : undefined,
      authErrorsPresent: !!(auth && auth.authErrors),
      authErrorsCount: auth && auth.authErrors ? auth.authErrors.length : 0,
      authErrorsMasked: auth && auth.authErrors ? auth.authErrors.map(maskTokenLike) : [],
    };
  } catch (e) {
    report.currentAuth = { ok: false, error: classifyError(e) };
  }

  // 3a. getQuota — 何も渡さない
  try {
    const t = Date.now();
    const result = await client.rpc.account.getQuota({});
    report.authRoutes.noArgs = { ok: true, ms: Date.now() - t };
    report.quota.noArgs = await analyzeQuota(result);
  } catch (e) {
    report.authRoutes.noArgs = { ok: false, error: classifyError(e) };
  }

  // 3b. getQuota — gh auth token を gitHubToken に渡す (FR-C-141 の再利用可否)
  const ghToken = await getGhAuthToken();
  report.authRoutes.ghAuthTokenFetch = ghToken.ok
    ? { ok: true, length: ghToken.length, prefix4: ghToken.prefix4 }
    : { ok: false, error: ghToken.error };

  if (ghToken.ok) {
    try {
      // 実際のトークン文字列はここでしか使わない。ログ・出力には出さない。
      const { stdout } = await execFileP('gh', ['auth', 'token']);
      const rawToken = stdout.trim();
      const t = Date.now();
      const result = await client.rpc.account.getQuota({ gitHubToken: rawToken });
      report.authRoutes.withGhToken = { ok: true, ms: Date.now() - t };
      report.quota.withGhToken = await analyzeQuota(result);
    } catch (e) {
      report.authRoutes.withGhToken = { ok: false, error: classifyError(e) };
    }
  } else {
    report.authRoutes.withGhToken = { ok: false, error: 'gh auth token 取得自体に失敗したため試行せず' };
  }

  // 5. models.list — billing のキー名とモデル数
  try {
    const t = Date.now();
    const modelsResult = await client.rpc.models.list({});
    const models = (modelsResult && modelsResult.models) || [];
    report.modelsList = {
      ok: true,
      ms: Date.now() - t,
      modelCount: models.length,
      sampleBillingKeys: models
        .slice(0, 20)
        .map((m) => ({
          id: m.id,
          billingKeys: m.billing ? Object.keys(m.billing) : null,
          tokenPriceKeys: m.billing && m.billing.tokenPrices ? Object.keys(m.billing.tokenPrices) : null,
        })),
    };
  } catch (e) {
    report.modelsList = { ok: false, error: classifyError(e) };
  }

  try {
    await client.stop();
  } catch {
    // 停止失敗は握りつぶす (プローブの本題ではない)
  }

  finish();
}

function finish() {
  mkdirSync(OUT_DIR, { recursive: true });
  const outPath = join(OUT_DIR, 'quota-sdk.json');
  writeFileSync(outPath, JSON.stringify(report, null, 2), 'utf8');

  if (JSON_ONLY) {
    console.log(JSON.stringify(report, null, 2));
    return;
  }

  console.log('=== Copilot SDK account.getQuota プローブ (読み取り専用) ===\n');
  console.log('SDK パス          :', report.sdkPathTried);
  console.log('SDK 解決          :', report.sdkResolved);
  if (report.sdkResolveError) console.log('解決エラー        :', report.sdkResolveError);
  if (report.clientStart) {
    console.log('\n--- クライアント起動 ---');
    console.log(JSON.stringify(report.clientStart, null, 2));
  }
  if (report.currentAuth) {
    console.log('\n--- account.getCurrentAuth (キー名のみ) ---');
    console.log(JSON.stringify(report.currentAuth, null, 2));
  }
  console.log('\n--- 認証経路 (OQ-08) ---');
  console.log(JSON.stringify(report.authRoutes, null, 2));
  console.log('\n--- account.getQuota 実測 (OQ-06) ---');
  console.log(JSON.stringify(report.quota, null, 2));
  console.log('\n--- models.list (FR-C-134 裏取り) ---');
  console.log(JSON.stringify(report.modelsList, null, 2));
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
