// gh-dashboard — エントリポイント
// リリースビルドでコンソールウィンドウを出さない
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // ログ初期化は gh_dashboard_lib::run() (init_logging, T-X.1 / NFR-22) が行う。
    // ここで先に tracing_subscriber::fmt().init() を呼ぶと、そちらがグローバル
    // subscriber を先取りしてしまい、ファイル出力レイヤーが黙って無効化される。
    gh_dashboard_lib::run();
}
