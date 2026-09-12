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
pub mod logging;
pub mod platform;
pub mod projects;
pub mod state;
pub mod util;
pub mod watchdog;

use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager};

/// ログ初期化 (T-X.1 / NFR-22)。**書き込み先が解決できなくてもアプリは起動する。**
///
/// `EnvFilter` でレベルを制御できるようにし、コンソール + ファイル (5MB×5世代) の
/// 2 レイヤーに出す。`try_init` の失敗 (二重初期化など) は無視する — ログの
/// 初期化に失敗してもアプリ本体を止めない。
fn init_logging() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let console_layer = tracing_subscriber::fmt::layer();

    let log_dir = dirs::data_local_dir().map(|d| d.join("gh-dashboard").join("logs"));
    match log_dir {
        Some(dir) => {
            let writer = logging::RotatingFileWriter::new(dir);
            let file_layer = tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false);
            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(console_layer)
                .with(file_layer)
                .try_init();
        }
        None => {
            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(console_layer)
                .try_init();
        }
    }
}

/// アプリの起動。`main.rs` から呼ぶ。
pub fn run() {
    init_logging();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let state = state::AppState::init(&handle)?;
            app.manage(state);

            // UI スレッドの応答性を監視してログに残す (NFR-21)。
            watchdog::spawn(handle.clone());

            // T-X.3 / DR-07: quota_samples の間引きは起動時に1回。
            // メインスレッド (setup) を DB アクセスでブロックしないよう
            // spawn_blocking に逃がす (NFR-20 / INV-10)。
            if let Some(state) = handle.try_state::<state::AppState>() {
                let db = state.db.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    let conn = match db.lock() {
                        Ok(c) => c,
                        Err(e) => e.into_inner(),
                    };
                    if let Err(e) = crate::db::prune_quota_samples(&conn, now_ms) {
                        tracing::warn!(error = %e, "quota_samples の間引きに失敗しました");
                    }
                });
            }

            // ネイティブウィンドウの最小化を UI に伝える (IR-46 / FR-C-43)。
            // WebView の visibilitychange は最小化で発火しないため、この経路が要る。
            //
            // **ウィンドウを閉じる操作はトレイ常駐へ横流しする** (FR-P-62 の前提)。
            // 実際に終了させるのはトレイの「終了」メニューだけ
            if let Some(window) = app.get_webview_window("main") {
                let emitter = window.clone();
                let hide_target = window.clone();
                window.on_window_event(move |event| match event {
                    tauri::WindowEvent::Resized(_) => {
                        let minimized = emitter.is_minimized().unwrap_or(false);
                        let _ = emitter.emit(
                            "window-visibility",
                            serde_json::json!({ "minimized": minimized }),
                        );
                    }
                    tauri::WindowEvent::CloseRequested { api, .. } => {
                        // 閉じるのではなく隠す。dev サーバーの停止は
                        // RunEvent::Exit (state.shutdown()) だけが担う (FR-P-62)
                        api.prevent_close();
                        let _ = hide_target.hide();
                    }
                    _ => {}
                });
            }

            // トレイ常駐 (T-X.5 / FR-P-62 の前提環境)
            let show_item = MenuItemBuilder::with_id("show", "表示").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "終了").build(app)?;
            let tray_menu = MenuBuilder::new(app).items(&[&show_item, &quit_item]).build()?;

            let mut tray_builder = TrayIconBuilder::new().menu(&tray_menu);
            if let Some(icon) = app.default_window_icon() {
                tray_builder = tray_builder.icon(icon.clone());
            }
            tray_builder
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "quit" => app.exit(0),
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build(app)?;

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
            projects::commands::projects_open_browser,
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
