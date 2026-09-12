//! アプリ状態。
//!
//! **導出データはここ (メモリ) に置き、DB に持たない** (DR-02 / INV-5)。
//! DB に持ってよいのはスキャン対象フォルダ・手動調整・セッション索引だけ。
//!
//! 対応要求: DR-02 / FR-P-04 / FR-P-05 / FR-P-61 / NFR-20

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use tauri::AppHandle;

use crate::platform::win_job::JobHandle;
use crate::projects::dev_server::DevRegistry;

pub struct AppState {
    /// 単一ローカル DB。**ロックを持ったまま `.await` しないこと**
    pub db: Arc<Mutex<Connection>>,

    /// スキャン結果のキャッシュ。**永続化しない** (FR-P-04 / FR-P-05)
    pub projects_cache: Arc<Mutex<Option<crate::projects::ProjectsSnapshot>>>,

    /// dev サーバーのレジストリ。ログはメモリ上のリングバッファのみ (FR-P-64)
    pub dev: Arc<DevRegistry>,

    /// dev サーバーの道連れ終了用 Job (FR-P-61)
    pub job: Arc<JobHandle>,

    /// 差分インデックスの多重起動防止 (FR-C-10)
    pub indexing: Arc<AtomicBool>,

    /// ライブ監視の末尾読みキャッシュ (FR-C-48)。**永続化しない** (INV-5 / DR-02)。
    /// 2 秒ポーリングのたびに全セッションの末尾を読まないためだけに存在する
    pub live_cache: Arc<Mutex<crate::copilot::live::LiveCache>>,

    /// 利用枠の直近観測と経路の可用性。**永続化しない** (INV-5 / DR-02)。
    ///
    /// FR-C-86 の `observed_at` を「値が変わったときだけ」進めるには前回値が要る。
    /// `quota_samples` テーブルは FR-C-94 の時系列専用で、これとは別物
    pub quota: Arc<Mutex<crate::copilot::quota_fetch::QuotaCache>>,
}

impl AppState {
    pub fn init(_app: &AppHandle) -> anyhow::Result<Self> {
        let path = crate::db::default_db_path()
            .ok_or_else(|| anyhow::anyhow!("ローカルデータフォルダを解決できません"))?;
        let conn = crate::db::open(&path)?;

        let job = JobHandle::create()
            .map(Arc::new)
            .map_err(|e| anyhow::anyhow!("Job Object を作成できません: {e}"))?;

        // T-X.3 / DR-07 の間引きは呼び出し側 (lib.rs::run の setup) が
        // spawn_blocking で行う。**ここ (init) はメインスレッドから同期的に
        // 呼ばれるため、DB への追加クエリをここに置かない** (NFR-20 / INV-10)。
        Ok(Self {
            db: Arc::new(Mutex::new(conn)),
            projects_cache: Arc::new(Mutex::new(None)),
            dev: Arc::new(DevRegistry::new()),
            job,
            indexing: Arc::new(AtomicBool::new(false)),
            live_cache: Arc::new(Mutex::new(Default::default())),
            quota: Arc::new(Mutex::new(Default::default())),
        })
    }

    /// 差分インデックスの開始を試みる。すでに実行中なら `false` (FR-C-10)。
    ///
    /// **実行中の再要求は黙って無視する。**エラーにしない。
    pub fn try_begin_indexing(&self) -> bool {
        self.indexing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub fn end_indexing(&self) {
        self.indexing.store(false, Ordering::Release);
    }


    /// アプリ終了時の後始末 (FR-P-62)。
    ///
    /// **ウィンドウを閉じただけでは呼ばない。**終了イベントでのみ呼ぶ。
    pub fn shutdown(&self) {
        self.dev.stop_all();
        // job が drop されると KILL_ON_JOB_CLOSE で取りこぼしも回収される
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexing_guard_prevents_concurrent_runs() {
        let flag = Arc::new(AtomicBool::new(false));
        let begin = |f: &Arc<AtomicBool>| {
            f.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        };
        assert!(begin(&flag), "1 回目は開始できる");
        assert!(!begin(&flag), "実行中の再要求は無視される (FR-C-10)");
        flag.store(false, Ordering::Release);
        assert!(begin(&flag), "終了後は再度開始できる");
    }
}
