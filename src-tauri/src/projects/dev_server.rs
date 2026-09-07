//! dev サーバーの起動 / 停止 / ログ。
//!
//! ログは**メモリ上のリングバッファ 500 行のみ**。DB に保存しない (FR-P-64)。
//! プロセスは Job Object に割り当てて、アプリが異常終了しても残らないようにする
//! (FR-P-61 / `platform::win_job`)。
//!
//! 対応要求: FR-P-60〜68 / IR-04 / IR-05 / IR-41 / IR-42
//!
//! 実装状況: 状態の型とリングバッファのみ。起動・停止は T-3.2 で実装する。

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

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

    pub fn push_log(&self, path_key: &str, line: String) {
        if let Ok(mut m) = self.entries.lock() {
            m.entry(path_key.to_string()).or_default().logs.push(line);
        }
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
    pub fn stop_all(&self) {
        // TODO(T-3.2): 実プロセスの停止。現状は状態のリセットのみ。
        // Job Object の drop でプロセスツリー自体は回収される (FR-P-61)。
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
}
