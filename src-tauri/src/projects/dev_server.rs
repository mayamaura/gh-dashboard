//! dev サーバーの起動 / 停止 / ログ。
//!
//! ログは**メモリ上のリングバッファ 500 行のみ**。DB に保存しない (FR-P-64)。
//! プロセスは Job Object に割り当てて、アプリが異常終了しても残らないようにする
//! (FR-P-61 / `platform::win_job`)。
//!
//! **Tauri に依存しない。**イベント配信はコールバック (`StatusHook` / `LogHook`) 経由に
//! 留め、`AppHandle` を持ち込まない — テストで実プロセスを起動しても `AppHandle` を
//! 用意せずに済む。呼び出し側 (`commands.rs`) がコールバックで `app.emit` する。
//!
//! 選択的な停止 (dev_stop) は `taskkill /T /F` でプロセスツリーごと落とす。
//! 共有の Job Object (`state.job`) は「アプリごと異常終了したときの道連れ終了」
//! 専用で、個別停止には使わない (INV-8 の単一ファイル制約は変えない)。
//!
//! 対応要求: FR-P-60〜68 / IR-04 / IR-05 / IR-41 / IR-42

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};

use crate::platform::win_job::JobHandle;
use crate::util::url_detect;

/// dev サーバーの 5 状態 (FR-P-60)。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DevState {
    #[default]
    Stopped,
    Starting,
    Running {
        /// 出力から検出した URL。**設定値からは決められない** (FR-P-63)
        url: Option<String>,
        pid: u32,
        started_at: i64,
    },
    Exited {
        code: Option<i32>,
    },
    Failed {
        reason: String,
    },
}

/// ログのリングバッファ上限 (FR-P-64)
pub const LOG_CAPACITY: usize = 500;

/// ログ配信のバッファ間隔 (FR-P-65)
pub const LOG_FLUSH_MS: u64 = 250;

/// 1 プロジェクト分のログ。上限を超えたら古い行から捨てる。
#[derive(Debug, Default)]
pub struct LogRing {
    lines: std::collections::VecDeque<String>,
}

impl LogRing {
    pub fn push(&mut self, line: String) {
        if self.lines.len() == LOG_CAPACITY {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.lines.iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

#[derive(Debug, Default)]
struct DevEntry {
    state: DevState,
    logs: LogRing,
    /// 次回イベント配信までに溜まった未送信行 (FR-P-65)
    pending: Vec<String>,
}

/// 稼働中 dev サーバーのレジストリ。
#[derive(Default)]
pub struct DevRegistry {
    entries: Mutex<HashMap<String, DevEntry>>,
}

impl DevRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state_of(&self, path_key: &str) -> DevState {
        self.entries
            .lock()
            .ok()
            .and_then(|m| m.get(path_key).map(|e| e.state.clone()))
            .unwrap_or(DevState::Stopped)
    }

    pub fn set_state(&self, path_key: &str, state: DevState) {
        if let Ok(mut m) = self.entries.lock() {
            m.entry(path_key.to_string()).or_default().state = state;
        }
    }

    /// 起動中に URL を検出したときだけ呼ぶ。**まだ `Running` かつ URL 未検出のときだけ
    /// 反映する** — 停止・再起動の後に古い検出が紛れ込むのを防ぐ。実際に更新したら
    /// `true` (呼び出し側はこのときだけ状態変化イベントを出す)。
    pub fn set_running_url(&self, path_key: &str, url: &str) -> bool {
        let Ok(mut m) = self.entries.lock() else {
            return false;
        };
        let Some(entry) = m.get_mut(path_key) else {
            return false;
        };
        if let DevState::Running {
            url: u,
            pid,
            started_at,
        } = &entry.state
        {
            if u.is_none() {
                entry.state = DevState::Running {
                    url: Some(url.to_string()),
                    pid: *pid,
                    started_at: *started_at,
                };
                return true;
            }
        }
        false
    }

    /// プロセス終了を検出したときに呼ぶ。**現在も同じ pid で `Running` のときだけ**
    /// `Exited` にする — 先にユーザーが停止していれば (`Stopped`) 上書きしない
    /// (dev_stop との競合防止)。実際に遷移したときだけ `Some` を返す。
    pub fn mark_exited(&self, path_key: &str, pid: u32, code: Option<i32>) -> Option<DevState> {
        let mut m = self.entries.lock().ok()?;
        let entry = m.get_mut(path_key)?;
        if let DevState::Running { pid: current, .. } = &entry.state {
            if *current == pid {
                let exited = DevState::Exited { code };
                entry.state = exited.clone();
                return Some(exited);
            }
        }
        None
    }

    pub fn push_log(&self, path_key: &str, line: String) {
        if let Ok(mut m) = self.entries.lock() {
            let entry = m.entry(path_key.to_string()).or_default();
            entry.logs.push(line.clone());
            entry.pending.push(line);
        }
    }

    /// 未送信分を取り出して空にする (FR-P-65 のバッファ配信用)。
    pub fn take_pending(&self, path_key: &str) -> Vec<String> {
        self.entries
            .lock()
            .ok()
            .and_then(|mut m| m.get_mut(path_key).map(|e| std::mem::take(&mut e.pending)))
            .unwrap_or_default()
    }

    /// 詳細パネルを開いた直後の初期表示用 (IR-05)
    pub fn logs(&self, path_key: &str) -> Vec<String> {
        self.entries
            .lock()
            .ok()
            .and_then(|m| m.get(path_key).map(|e| e.logs.snapshot()))
            .unwrap_or_default()
    }

    /// 停止要求は冪等。未起動でもエラーにせず「停止中」を返す (FR-P-66)。
    pub fn mark_stopped(&self, path_key: &str) -> DevState {
        self.set_state(path_key, DevState::Stopped);
        DevState::Stopped
    }

    /// アプリ終了時に全停止する (FR-P-62)。
    ///
    /// ここでは状態のリセットのみ行う。**実プロセスの回収は共有 Job Object の
    /// `drop` (`KILL_ON_JOB_CLOSE`) に任せる** (FR-P-61) — この関数はメイン
    /// スレッドの `RunEvent::Exit` ハンドラから同期的に呼ばれるため、ここで
    /// `taskkill` 等の外部プロセスを起動すると INV-10 に触れる。
    pub fn stop_all(&self) {
        if let Ok(mut m) = self.entries.lock() {
            for entry in m.values_mut() {
                entry.state = DevState::Stopped;
            }
        }
    }

    pub fn running_keys(&self) -> Vec<String> {
        self.entries
            .lock()
            .map(|m| {
                m.iter()
                    .filter(|(_, e)| matches!(e.state, DevState::Running { .. }))
                    .map(|(k, _)| k.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------- T-3.2〜3.4

/// dev 状態が変化したときに呼ぶコールバック (IR-41 相当)。`AppHandle` を持ち込まず
/// テストしやすくするための抽象化。
pub type StatusHook = Arc<dyn Fn(&str, &DevState) + Send + Sync>;
/// ログがバッファ間隔ごとに溜まったときに呼ぶコールバック (IR-42 相当)。
pub type LogHook = Arc<dyn Fn(&str, &[String]) + Send + Sync>;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// dev サーバーを起動する (T-3.2)。**すぐに返る** — `Starting` → `Running` /
/// `Failed` までを待ち、その後のログ読み取り・終了待ちはバックグラウンドタスクに
/// 委ねる。
///
/// `command` は `cmd /C` 経由で実行する — `npm` 等の PATH 上のコマンドは Windows
/// では `.cmd` シムのことが多く、シェルの助けなしには解決できない。
pub async fn start(
    registry: Arc<DevRegistry>,
    job: Arc<JobHandle>,
    path_key: String,
    working_dir: String,
    command: String,
    on_status: StatusHook,
    on_logs: LogHook,
) -> DevState {
    registry.set_state(&path_key, DevState::Starting);
    on_status(&path_key, &DevState::Starting);

    let mut child = match Command::new("cmd")
        .args(["/C", &command])
        .current_dir(&working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let failed = DevState::Failed {
                reason: format!("{command} を起動できません: {e}"),
            };
            registry.set_state(&path_key, failed.clone());
            on_status(&path_key, &failed);
            return failed;
        }
    };

    // PID は spawn 直後にしか取れない (wait 後は None になる)
    let pid = child.id().unwrap_or(0);
    // Job への割り当ては失敗してもアプリ終了時の道連れ終了が効かないだけで、
    // dev サーバー自体は動くので致命的扱いしない (NFR-24)
    let _ = job.assign(pid);

    let running = DevState::Running {
        url: None,
        pid,
        started_at: now_ms(),
    };
    registry.set_state(&path_key, running.clone());
    on_status(&path_key, &running);

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    tokio::spawn(watch(
        registry, path_key, child, pid, stdout, stderr, on_status, on_logs,
    ));

    running
}

/// 起動後の後始末: 行読み取り (T-3.3) + ログの間引き配信 (T-3.4) + 終了検出。
#[allow(clippy::too_many_arguments)]
async fn watch(
    registry: Arc<DevRegistry>,
    path_key: String,
    mut child: Child,
    pid: u32,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    on_status: StatusHook,
    on_logs: LogHook,
) {
    let mut readers = Vec::new();
    if let Some(out) = stdout {
        readers.push(tokio::spawn(read_lines(
            registry.clone(),
            path_key.clone(),
            out,
            on_status.clone(),
        )));
    }
    if let Some(err) = stderr {
        readers.push(tokio::spawn(read_lines(
            registry.clone(),
            path_key.clone(),
            err,
            on_status.clone(),
        )));
    }
    let flusher = tokio::spawn(flush_logs(
        registry.clone(),
        path_key.clone(),
        on_logs.clone(),
    ));

    let status = child.wait().await;
    for r in readers {
        let _ = r.await;
    }
    flusher.abort();

    // 最後の一巡で溜まった分を取りこぼさない
    let remaining = registry.take_pending(&path_key);
    if !remaining.is_empty() {
        on_logs(&path_key, &remaining);
    }

    let code = status.ok().and_then(|s| s.code());
    if let Some(exited) = registry.mark_exited(&path_key, pid, code) {
        on_status(&path_key, &exited);
    }
}

/// 1 パイプ分の行読み取り。URL を検出したら最初の 1 回だけ状態変化を通知する
/// (FR-P-63)。
async fn read_lines<R>(registry: Arc<DevRegistry>, path_key: String, pipe: R, on_status: StatusHook)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        registry.push_log(&path_key, line.clone());
        if let Some(url) = url_detect::extract_url(&line) {
            if registry.set_running_url(&path_key, &url) {
                on_status(&path_key, &registry.state_of(&path_key));
            }
        }
    }
}

/// 200〜300ms でログイベントをまとめて配信する (FR-P-65)。
async fn flush_logs(registry: Arc<DevRegistry>, path_key: String, on_logs: LogHook) {
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(LOG_FLUSH_MS));
    loop {
        interval.tick().await;
        let lines = registry.take_pending(&path_key);
        if !lines.is_empty() {
            on_logs(&path_key, &lines);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_ring_keeps_only_the_last_500_lines() {
        let mut ring = LogRing::default();
        for i in 0..600 {
            ring.push(format!("line {i}"));
        }
        assert_eq!(ring.len(), LOG_CAPACITY);
        let snap = ring.snapshot();
        assert_eq!(snap.first().unwrap(), "line 100");
        assert_eq!(snap.last().unwrap(), "line 599");
    }

    #[test]
    fn unknown_project_is_stopped_not_an_error() {
        let reg = DevRegistry::new();
        assert_eq!(reg.state_of("d:\\nope"), DevState::Stopped);
    }

    /// FR-P-66: 停止要求は冪等
    #[test]
    fn stopping_an_already_stopped_server_is_ok() {
        let reg = DevRegistry::new();
        assert_eq!(reg.mark_stopped("d:\\a"), DevState::Stopped);
        assert_eq!(reg.mark_stopped("d:\\a"), DevState::Stopped);
    }

    #[test]
    fn running_keys_lists_only_running() {
        let reg = DevRegistry::new();
        reg.set_state(
            "d:\\a",
            DevState::Running {
                url: Some("http://localhost:5173/".into()),
                pid: 1234,
                started_at: 0,
            },
        );
        reg.set_state("d:\\b", DevState::Stopped);
        assert_eq!(reg.running_keys(), vec!["d:\\a".to_string()]);
    }

    #[test]
    fn dev_state_serializes_with_state_tag() {
        let json = serde_json::to_string(&DevState::Failed {
            reason: "npm が見つかりません".into(),
        })
        .unwrap();
        assert!(json.contains("\"state\":\"failed\""));
    }

    #[test]
    fn set_running_url_only_applies_once_while_running() {
        let reg = DevRegistry::new();
        reg.set_state(
            "d:\\a",
            DevState::Running {
                url: None,
                pid: 1,
                started_at: 0,
            },
        );
        assert!(reg.set_running_url("d:\\a", "http://localhost:5173/"));
        // 2 回目は既に埋まっているので何もしない
        assert!(!reg.set_running_url("d:\\a", "http://localhost:9999/"));
        assert_eq!(
            reg.state_of("d:\\a"),
            DevState::Running {
                url: Some("http://localhost:5173/".into()),
                pid: 1,
                started_at: 0,
            }
        );
    }

    #[test]
    fn set_running_url_is_noop_when_not_running() {
        let reg = DevRegistry::new();
        assert!(!reg.set_running_url("d:\\a", "http://localhost:5173/"));
        assert_eq!(reg.state_of("d:\\a"), DevState::Stopped);
    }

    /// dev_stop で先に `Stopped` にした後、バックグラウンドの終了検出が
    /// 追いついても `Exited` で上書きしてはいけない (競合防止)。
    #[test]
    fn mark_exited_does_not_overwrite_a_state_the_user_already_stopped() {
        let reg = DevRegistry::new();
        reg.set_state(
            "d:\\a",
            DevState::Running {
                url: None,
                pid: 42,
                started_at: 0,
            },
        );
        reg.mark_stopped("d:\\a");
        assert_eq!(reg.mark_exited("d:\\a", 42, Some(0)), None);
        assert_eq!(reg.state_of("d:\\a"), DevState::Stopped);
    }

    #[test]
    fn mark_exited_ignores_a_stale_pid_from_a_replaced_run() {
        let reg = DevRegistry::new();
        reg.set_state(
            "d:\\a",
            DevState::Running {
                url: None,
                pid: 99, // 再起動後の新しい pid
                started_at: 0,
            },
        );
        // 古い pid (42) の終了通知は無視する
        assert_eq!(reg.mark_exited("d:\\a", 42, Some(1)), None);
        assert!(matches!(
            reg.state_of("d:\\a"),
            DevState::Running { pid: 99, .. }
        ));
    }

    #[test]
    fn mark_exited_transitions_a_matching_running_pid() {
        let reg = DevRegistry::new();
        reg.set_state(
            "d:\\a",
            DevState::Running {
                url: None,
                pid: 7,
                started_at: 0,
            },
        );
        assert_eq!(
            reg.mark_exited("d:\\a", 7, Some(1)),
            Some(DevState::Exited { code: Some(1) })
        );
    }

    #[test]
    fn take_pending_drains_and_leaves_the_ring_buffer_intact() {
        let reg = DevRegistry::new();
        reg.push_log("d:\\a", "line 1".into());
        reg.push_log("d:\\a", "line 2".into());
        assert_eq!(reg.take_pending("d:\\a"), vec!["line 1", "line 2"]);
        assert_eq!(reg.take_pending("d:\\a"), Vec::<String>::new());
        assert_eq!(reg.logs("d:\\a"), vec!["line 1", "line 2"]);
    }

    /// 実プロセスを起動し、`Starting` → `Running` → `Exited` まで一気通貫で確認する
    /// (NFR-51: 実データ相当の経路をそのままテストケースにする)。
    ///
    /// **`job` をテストの最後まで生かしておくこと。** 本番では `AppState` がアプリ
    /// 寿命で共有 Job を保持し続けるが、テストで唯一の参照 (Arc) を早期に drop する
    /// と `KILL_ON_JOB_CLOSE` が働いて子プロセスが即座に強制終了され、出力を
    /// 読み取る前にパイプが閉じてしまう。
    #[tokio::test]
    async fn start_runs_a_real_process_through_to_exit() {
        let registry = Arc::new(DevRegistry::new());
        let job = Arc::new(JobHandle::create().expect("Job Object を作れること"));

        let statuses: Arc<Mutex<Vec<DevState>>> = Arc::new(Mutex::new(Vec::new()));
        let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let statuses_for_hook = statuses.clone();
        let on_status: StatusHook = Arc::new(move |_key, state| {
            statuses_for_hook.lock().unwrap().push(state.clone());
        });
        let logs_for_hook = logs.clone();
        let on_logs: LogHook = Arc::new(move |_key, lines| {
            logs_for_hook.lock().unwrap().extend_from_slice(lines);
        });

        let working_dir = std::env::temp_dir().to_string_lossy().to_string();
        let result = start(
            registry.clone(),
            job.clone(),
            "test-key".to_string(),
            working_dir,
            "echo hello-from-dev-server".to_string(),
            on_status,
            on_logs,
        )
        .await;
        assert!(matches!(result, DevState::Running { .. }));

        // 終了検出は別タスクなので、少し待って結果を拾う
        for _ in 0..50 {
            if matches!(registry.state_of("test-key"), DevState::Exited { .. }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(
            registry.state_of("test-key"),
            DevState::Exited { code: Some(0) }
        );
        assert!(
            registry
                .logs("test-key")
                .iter()
                .any(|l| l.contains("hello-from-dev-server")),
            "実際の出力行がリングバッファに残っていること: {:?}",
            registry.logs("test-key")
        );
    }
}
