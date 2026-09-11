//! 外部ツール連携 (FR-P-70〜74)。
//!
//! **投げっぱなしで起動し、完了を待たない** (FR-P-71)。**起動成否は spawn の
//! 成否のみで判定し、終了コードを見ない** (FR-P-72) — `explorer.exe` は正常時でも
//! 終了コード 1 を返す。
//!
//! エクスプローラー / ブラウザは `tauri-plugin-opener` (導入済み依存) の OS 既定
//! ハンドラ起動に委ねる — 内部で PowerShell の `Start-Process` を試し、失敗したら
//! `explorer.exe` にフォールバックする実装を持っており、ここで自前に書く必要が
//! ない。VS Code / ターミナル / Copilot CLI は PATH 上の特定コマンドを直接起動する
//! 必要があるため、ここで組み立てる。
//!
//! VS Code / Copilot CLI は npm 由来の `.cmd` シムであることが多く、拡張子なしでは
//! Windows の `CreateProcess` が `.exe` しか自動補完しないため解決できない。
//! `.cmd` を明示することで Windows 側の特別扱い (`cmd.exe /c` 経由の起動) に乗る。
//!
//! 対応要求: FR-P-70〜74 / IR-06

use std::os::windows::process::CommandExt;
use std::process::Command;

use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

use crate::error::AppError;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn open_vscode(dir: &str) -> Result<(), AppError> {
    Command::new("code.cmd")
        .arg(dir)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| {
            AppError::external("VS Code", e.to_string()).with_hint(
                "code コマンドが PATH に見つかりません。VS Code のインストール時に「PATH に追加」を有効にしてください",
            )
        })
}

pub fn open_folder(app: &AppHandle, dir: &str) -> Result<(), AppError> {
    app.opener()
        .open_path(dir, None::<&str>)
        .map_err(|e| AppError::external("エクスプローラー", e.to_string()))
}

/// FR-P-70: 稼働中かつ URL 検出済みのときだけ呼ばれる想定。
pub fn open_browser(app: &AppHandle, url: &str) -> Result<(), AppError> {
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| AppError::external("ブラウザ", e.to_string()))
}

/// Windows Terminal を優先し、失敗したら PowerShell にフォールバックする (FR-P-74)。
pub fn open_terminal(dir: &str) -> Result<(), AppError> {
    if Command::new("wt.exe").args(["-d", dir]).spawn().is_ok() {
        return Ok(());
    }
    Command::new("powershell.exe")
        .current_dir(dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| {
            AppError::external("ターミナル", e.to_string()).with_hint(
                "Windows Terminal も PowerShell も起動できません。PATH を確認してください",
            )
        })
}

/// そのディレクトリで Copilot CLI を起動する (FR-P-70)。対話型 CLI なので
/// ターミナルなしでは操作できない — Windows Terminal / PowerShell を新しく開き、
/// その中で `copilot` を実行する。
pub fn open_agent(dir: &str) -> Result<(), AppError> {
    if Command::new("wt.exe")
        .args(["-d", dir, "copilot.cmd"])
        .spawn()
        .is_ok()
    {
        return Ok(());
    }
    Command::new("powershell.exe")
        .current_dir(dir)
        .args(["-NoExit", "-Command", "copilot"])
        .spawn()
        .map(|_| ())
        .map_err(|e| {
            AppError::external("Copilot CLI", e.to_string())
                .with_hint("copilot コマンドが PATH に見つかりません。Copilot CLI をインストールしてください")
        })
}
