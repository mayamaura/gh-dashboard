//! git 状態の取得 (IO)。
//!
//! **判定対象は `root_path` (リポジトリルート)。`working_dir` ではない** (FR-P-44)。
//! `.git` が無ければ git コマンドを 1 つも実行しない (FR-P-42)。
//! 個々の git コマンドの非ゼロ終了・起動失敗は正常系として扱い、`None`/デフォルト値に
//! フォールバックする。1 プロジェクトの失敗で全体を落とさない (FR-P-43)。
//!
//! 対応要求: FR-P-40〜47

use std::path::Path;
use std::process::{Command, Output};

use crate::projects::GitStatus;

/// Windows でコンソールウィンドウを出さないためのフラグ (FR-P-46)。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn git_command(root: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(root);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// git コマンドを実行する。起動失敗は `None`。**終了コードは見ず、stdout をそのまま返す**
/// (呼び出し側が空/非空や内容で判定する)。
fn run(root: &Path, args: &[&str]) -> Option<Output> {
    git_command(root, args).output().ok()
}

fn stdout_trimmed(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// 1 プロジェクト分の git 状態を取得する。同期関数 — 呼び出し側が並行実行数を制御する。
///
/// `.git` (ファイルまたはディレクトリ。worktree ではファイル) が無ければ `None` を返し、
/// git コマンドは 1 つも実行しない (FR-P-42)。
pub fn git_status(root: &Path) -> Option<GitStatus> {
    if !root.join(".git").exists() {
        return None;
    }

    let branch = run(root, &["symbolic-ref", "--short", "HEAD"])
        .filter(|o| o.status.success())
        .map(|o| stdout_trimmed(&o))
        .filter(|s| !s.is_empty());

    let dirty = run(root, &["status", "--porcelain"])
        .map(|o| !stdout_trimmed(&o).is_empty())
        .unwrap_or(false);

    let has_remote = run(root, &["remote"])
        .map(|o| !stdout_trimmed(&o).is_empty())
        .unwrap_or(false);

    let last_commit_at = run(root, &["log", "-1", "--format=%ct"])
        .filter(|o| o.status.success())
        .and_then(|o| stdout_trimmed(&o).parse::<i64>().ok())
        .map(|secs| secs * 1000);

    Some(GitStatus {
        branch,
        dirty,
        has_remote,
        last_commit_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn fixture_root(name: &str) -> PathBuf {
        let base =
            std::env::temp_dir().join(format!("gh-dashboard-git-test-{}", std::process::id()));
        let root = base.join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn cleanup(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }

    fn git(root: &Path, args: &[&str]) {
        let status = git_command(root, args)
            .status()
            .unwrap_or_else(|e| panic!("git {args:?} failed to spawn: {e}"));
        assert!(status.success(), "git {args:?} exited non-zero");
    }

    fn configure_identity(root: &Path) {
        git(root, &["config", "user.email", "test@example.com"]);
        git(root, &["config", "user.name", "Test"]);
    }

    #[test]
    fn no_git_dir_returns_none_without_running_git() {
        let root = fixture_root("no_git");
        assert!(git_status(&root).is_none());
        cleanup(&root);
    }

    #[test]
    fn clean_repo_with_commit_reports_branch_and_not_dirty() {
        let root = fixture_root("clean_repo");
        git(&root, &["init"]);
        configure_identity(&root);
        fs::write(root.join("a.txt"), "hello").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "init"]);

        let status = git_status(&root).expect("git status expected");
        assert!(status.branch.is_some(), "{status:?}");
        assert!(!status.dirty);
        assert!(!status.has_remote);
        assert!(status.last_commit_at.is_some());
        cleanup(&root);
    }

    #[test]
    fn dirty_working_tree_is_detected() {
        let root = fixture_root("dirty_repo");
        git(&root, &["init"]);
        configure_identity(&root);
        fs::write(root.join("a.txt"), "hello").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "init"]);
        fs::write(root.join("a.txt"), "changed").unwrap();

        let status = git_status(&root).expect("git status expected");
        assert!(status.dirty);
        cleanup(&root);
    }

    #[test]
    fn detached_head_has_no_branch_name() {
        let root = fixture_root("detached_repo");
        git(&root, &["init"]);
        configure_identity(&root);
        fs::write(root.join("a.txt"), "hello").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "init"]);
        git(&root, &["checkout", "--detach", "HEAD"]);

        let status = git_status(&root).expect("git status expected");
        assert_eq!(status.branch, None, "{status:?}");
        cleanup(&root);
    }

    #[test]
    fn repo_without_commit_has_no_last_commit_at() {
        let root = fixture_root("no_commit_repo");
        git(&root, &["init"]);
        configure_identity(&root);

        let status = git_status(&root).expect("git status expected");
        assert_eq!(status.last_commit_at, None);
        cleanup(&root);
    }
}
