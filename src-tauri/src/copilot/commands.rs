//! Copilot ダッシュボードの IPC コマンド (IR-10〜19)。
//!
//! **全コマンドに `rename_all = "snake_case"`** (IR-30)。
//!
//! `live_status_get` は 2 秒ごとに呼ばれる。**この関数から辿れる先に
//! ネットワークアクセスと外部プロセス起動を入れないこと** (INV-4 / NFR-03)。
//!
//! 実装状況: 署名と規約だけが確定した足場。中身は段階 4〜7 で実装する。

use tauri::State;

use crate::copilot::{
    AnimationPref, DbSnapshot, LiveStatus, SessionQuery, SessionSummary, TurnBody, UsageToday,
};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

fn todo_err(task: &str) -> AppError {
    AppError::unavailable(
        format!("未実装です ({task})"),
        Some("docs/implementation-plan.html の該当タスクを参照してください".to_string()),
    )
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------- IR-10

/// IR-10: ライブ状況。**2 秒ポーリング前提。ネットワークアクセスなし** (INV-4)。
///
/// 取得できるものが無い状態は正常系。空の一覧を返してよい。
#[tauri::command(rename_all = "snake_case")]
pub async fn live_status_get(_state: State<'_, AppState>) -> AppResult<LiveStatus> {
    // TODO(T-5.1..5.4):
    //   - 状態ファイルを列挙し、PID 生存確認で死んだものを除外 (FR-C-40)
    //   - (パス, サイズ, mtime) が同じならディスクを読み直さない (FR-C-48)
    //   - 末尾 64KB のシーク読み、足りなければ 512KB で 1 回だけ (FR-C-47)
    //   - 稼働中サブエージェントは "集合" を先に作る (FR-C-51)
    //   - activity::synthesize で活動状態を合成 (FR-C-45)
    Ok(LiveStatus::new(vec![], vec![], now_ms()))
}

// ---------------------------------------------------------------- IR-11..12

/// IR-11: DB 由来のダイジェスト。
#[tauri::command(rename_all = "snake_case")]
pub async fn snapshot_get(_state: State<'_, AppState>) -> AppResult<DbSnapshot> {
    // TODO(T-4.7): 索引から件数と直近 20 件を読む
    Ok(DbSnapshot {
        last_indexed_at: None,
        retained_session_count: 0,
        subagent_run_count: 0,
        turn_count: 0,
        recent_sessions: vec![],
    })
}

/// IR-12: 差分インデックスをバックグラウンド起動する。**即座に返す**。
///
/// 実行中の再要求は**黙って無視する**。エラーにしない (FR-C-10)。
#[tauri::command(rename_all = "snake_case")]
pub async fn index_refresh(state: State<'_, AppState>) -> AppResult<()> {
    if !state.try_begin_indexing() {
        // 多重起動防止。呼び出し側にとっては成功扱いでよい
        return Ok(());
    }
    // TODO(T-4.5): spawn_blocking でインデックスを回し、
    //   進捗を index-progress、完了を snapshot で通知する (IR-43 / IR-44)。
    state.end_indexing();
    Ok(())
}

// ---------------------------------------------------------------- IR-13..15

/// IR-13: セッション検索。既定 100 / 上限 1000 (FR-C-110)。
#[tauri::command(rename_all = "snake_case")]
pub async fn sessions_list_get(
    _state: State<'_, AppState>,
    query: SessionQuery,
) -> AppResult<Vec<SessionSummary>> {
    let _limit = query.effective_limit();
    // TODO(T-7.4): フォルダ名・タイトル・作業ディレクトリの部分一致で検索
    Ok(vec![])
}

/// IR-14: セッション詳細。
///
/// **未インデックスは `None` (正常系)。エラーにしない** (FR-C-57)。
/// 赤いエラーを出しても、ユーザーには何をすればよいか分からない。
#[tauri::command(rename_all = "snake_case")]
pub async fn session_detail_get(
    _state: State<'_, AppState>,
    _session_id: String,
) -> AppResult<Option<serde_json::Value>> {
    // TODO(T-7.5..7.7): 集計 + 木構築 + ガント用データを返す
    Ok(None)
}

/// IR-15: 本文を 1 レコードだけシーク読みする。上限 512KB (FR-C-119 / 120)。
#[tauri::command(rename_all = "snake_case")]
pub async fn turn_body_get(_state: State<'_, AppState>, _turn_id: i64) -> AppResult<TurnBody> {
    // TODO(T-4.12): file_path + byte_offset + byte_length でシーク読み。
    //   オフセットがファイル範囲外なら「インデックスを再実行してください」と案内 (FR-C-120)
    Err(todo_err("T-4.12"))
}

// ---------------------------------------------------------------- IR-16

/// IR-16: 本日の使用状況。
///
/// **2 秒ポーリングの対象に含めない** (FR-C-103)。タブ表示時と
/// 差分インデックス完了時のみ更新する。
#[tauri::command(rename_all = "snake_case")]
pub async fn usage_today_get(_state: State<'_, AppState>) -> AppResult<UsageToday> {
    // TODO(T-7.1..7.2): ローカル日 0:00 からの集計。
    //   合成モデルのレコードを除外し、usage の内訳を二重加算しない (FR-C-104)
    Ok(UsageToday {
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        turn_count: 0,
        session_count: 0,
        subagent_count: 0,
        total_nano_aiu: 0,
        hourly_tokens: vec![0; 24],
        top_folders: vec![],
        excluded_records: 0,
    })
}

// ---------------------------------------------------------------- IR-17..18

/// IR-17: 利用枠。**経路 A→B→C の降格込み**。
///
/// ネットワークを伴うため下限間隔 (5 分) がある。**2 秒ポーリングに載せない** (FR-C-135)。
#[tauri::command(rename_all = "snake_case")]
pub async fn quota_get(
    _state: State<'_, AppState>,
    _force: bool,
) -> AppResult<Vec<crate::copilot::quota::QuotaGauge>> {
    // TODO(T-6.3..6.7): SDK → REST → 推定 の順に試し、quota::degrade で組み立てる。
    //   取れなくても機能全体を止めない (FR-C-136)。枠ごとに独立して評価する (FR-C-84)
    Ok(vec![crate::copilot::quota::degrade(
        "monthly_credits",
        "月次 AI Credits",
        None,
        None,
        None,
        now_ms(),
    )])
}

/// IR-18: 各取得経路の可用性。「何をすれば取れるようになるか」の材料 (FR-C-83)。
#[tauri::command(rename_all = "snake_case")]
pub async fn quota_source_status_get(_state: State<'_, AppState>) -> AppResult<serde_json::Value> {
    // TODO(T-6.11): 認証状態 / SDK 有無 / 直近の取得結果と失敗理由
    Ok(serde_json::json!({
        "sdk": { "available": false, "reason": "未調査 (OQ-06)" },
        "rest": { "available": false, "reason": "未実装 (T-6.5)" },
        "estimate": { "available": false, "reason": "インデックス未実装 (段階 4)" }
    }))
}

// ---------------------------------------------------------------- IR-19

#[tauri::command(rename_all = "snake_case")]
pub async fn animation_pref_get(_state: State<'_, AppState>) -> AppResult<AnimationPref> {
    // TODO(T-7.14): settings テーブルから読む
    Ok(AnimationPref::Auto)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn animation_pref_set(
    _state: State<'_, AppState>,
    pref: AnimationPref,
) -> AppResult<AnimationPref> {
    // TODO(T-7.14): settings テーブルに書く
    Ok(pref)
}
