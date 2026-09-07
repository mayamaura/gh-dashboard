//! gh-dashboard コア。
//!
//! 構成と各モジュールの責務は docs/architecture.html を参照。
//! **`unsafe` は `platform::win_job` だけ** (INV-8) — crate 全体で禁止し、
//! 当該ファイルだけが `#[allow(unsafe_code)]` で開ける。

// crate 全体で unsafe を禁じ、platform::win_job だけが #![allow(unsafe_code)] で開ける。
// forbid ではなく deny なのは、forbid が下位モジュールの allow を許さないため (E0453)。
// 「開けてよいのは 1 ファイルだけ」という規約は CLAUDE.md の INV-8 とレビューで担保する。
#![deny(unsafe_code)]

pub mod copilot;
pub mod db;
pub mod error;
pub mod platform;
pub mod projects;
pub mod state;
pub mod util;

use tauri::{Emitter, Manager};

/// アプリの起動。`main.rs` から呼ぶ。
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let state = state::AppState::init(&handle)?;
            app.manage(state);

            // ネイティブウィンドウの最小化を UI に伝える (IR-46 / FR-C-43)。
            // WebView の visibilitychange は最小化で発火しないため、この経路が要る。
            if let Some(window) = app.get_webview_window("main") {
                let emitter = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::Resized(_) = event {
                        let minimized = emitter.is_minimized().unwrap_or(false);
                        let _ = emitter.emit(
                            "window-visibility",
                            serde_json::json!({ "minimized": minimized }),
                        );
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // --- プロジェクト (IR-01..06) ---
            projects::commands::projects_scan,
            projects::commands::projects_settings_update,
            projects::commands::projects_scan_folder_add,
            projects::commands::projects_scan_folder_remove,
            projects::commands::projects_dev_start,
            projects::commands::projects_dev_stop,
            projects::commands::projects_dev_stop_all,
            projects::commands::projects_dev_logs_get,
            projects::commands::projects_open_vscode,
            projects::commands::projects_open_folder,
            projects::commands::projects_open_terminal,
            projects::commands::projects_open_agent,
            // --- Copilot (IR-10..19) ---
            copilot::commands::live_status_get,
            copilot::commands::snapshot_get,
            copilot::commands::index_refresh,
            copilot::commands::sessions_list_get,
            copilot::commands::session_detail_get,
            copilot::commands::turn_body_get,
            copilot::commands::usage_today_get,
            copilot::commands::quota_get,
            copilot::commands::quota_source_status_get,
            copilot::commands::animation_pref_get,
            copilot::commands::animation_pref_set,
        ])
        .build(tauri::generate_context!())
        .expect("Tauri アプリの初期化に失敗しました")
        .run(|app_handle, event| {
            // アプリ終了時に全 dev サーバーを停止する (FR-P-62)。
            // ウィンドウを閉じただけ (トレイ常駐継続) では停止しない。
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app_handle.try_state::<state::AppState>() {
                    state.shutdown();
                }
            }
        });
}
