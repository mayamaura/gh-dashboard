// gh-dashboard — エントリポイント
// リリースビルドでコンソールウィンドウを出さない
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // TODO(T-X.1): ローテーション付きファイル出力に差し替える (NFR-22)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    gh_dashboard_lib::run();
}
