//! ディスク走査 (IO)。判定そのものは `detect.rs` (純粋) に委譲する。
//!
//! **同期関数。** 呼び出し側 (`commands.rs`) が `spawn_blocking` で包む (FR-P-06 / NFR-20)。
//! `run` は `Result` を返さない — 1 フォルダ / 1 プロジェクトの失敗はスキップして
//! `warnings` に積み、スキャン全体を失敗させない (FR-P-03 / NFR-24)。
//!
//! 対応要求: FR-P-01〜06 / FR-P-10〜14 / FR-P-20〜23 / FR-P-30〜32

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::projects::detect::{self, DirListing, SubdirCandidate};
use crate::projects::dev_server::DevRegistry;
use crate::projects::store::ProjectOverride;
use crate::projects::{Project, ProjectsSnapshot};
use crate::util::path_key;

/// 1 階層下探索 (FR-P-12) で除外するディレクトリ名。
///
/// `node_modules/<pkg>/vite.config.ts` のような依存物のマニフェストを
/// プロジェクト候補として誤検出しないための除外リスト。
/// `.` 始まりのディレクトリ (`.git` 等はここに書かなくても該当) も併せて除外する。
const EXCLUDED_SUBDIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
];

fn is_excluded_subdir(name: &str) -> bool {
    name.starts_with('.')
        || EXCLUDED_SUBDIRS
            .iter()
            .any(|e| name.eq_ignore_ascii_case(e))
}

/// ディレクトリ直下の一覧を読む。
fn read_listing(dir: &Path) -> std::io::Result<DirListing> {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            // 不正な UTF-16 名は無視する (実在しても判定材料にはできない)
            continue;
        };
        if entry.path().is_dir() {
            dirs.push(name);
        } else {
            files.push(name);
        }
    }
    Ok(DirListing { files, dirs })
}

/// `package.json` の `scripts` キー一覧。**防御的**: 無い / 壊れている場合は
/// エラーにせず空の一覧を返す。
fn read_script_keys(working_dir: &Path) -> Vec<String> {
    let path = working_dir.join("package.json");
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return Vec::new();
    };
    value
        .get("scripts")
        .and_then(|s| s.as_object())
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default()
}

/// `working_dir_override` の絶対/相対パスを解決する (FR-P-32)。
/// **実在検証はしない。** 呼び出し側が `is_dir()` を確認する。
pub fn resolve_working_dir_override(root: &Path, value: &str) -> PathBuf {
    let p = Path::new(value);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    }
}

/// 本アプリ自身のソースルートの `path_key`。
///
/// `CARGO_MANIFEST_DIR` は `src-tauri` を指すので、その親が本アプリのルート (FR-P-22)。
///
/// **限界**: これはビルド時定数なので、配布ビルドには**ビルドした開発機のパス**が
/// 埋め込まれる。ソースツリーを持たない環境では何にも一致しないが、そこには
/// 止めるべき「本アプリ自身のソース」も存在しないので実害は無い。ソースツリーが
/// 別の場所にコピーされた場合は検出できない — その場合は起動が「不適切」なのではなく
/// 単に別のフォルダなので、FR-P-22 の対象外と割り切る。
pub fn self_source_key() -> Option<String> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let parent = Path::new(manifest_dir).parent()?;
    path_key::path_key(&parent.to_string_lossy())
}

/// 起動不可判定の核 (純粋)。`root_key == self_key` かどうかだけを見る。
pub fn is_self_source(root_key: &str, self_key: &str) -> bool {
    root_key == self_key
}

fn self_source_reason(root_key: &str, self_key: Option<&str>) -> Option<String> {
    let self_key = self_key?;
    if is_self_source(root_key, self_key) {
        Some("本アプリ自身のソースです".to_string())
    } else {
        None
    }
}

/// 1 プロジェクト分の判定 + 手動調整の反映。
///
/// 初回スキャン (`run`) と設定変更時の再マージ (`commands::projects_settings_update`)
/// の両方から呼ばれる共有経路。`key` は呼び出し側が確定済みの `path_key` を渡す。
///
/// 読み取れないディレクトリなら `warnings` に積んで `None` を返す。
pub fn build_project(
    root: &Path,
    key: &str,
    ov: Option<&ProjectOverride>,
    dev: &DevRegistry,
    self_key: Option<&str>,
    warnings: &mut Vec<String>,
) -> Option<Project> {
    let root_str = root.to_string_lossy().to_string();

    let root_listing = match read_listing(root) {
        Ok(l) => l,
        Err(err) => {
            warnings.push(format!("{root_str}: 読み取れませんでした ({err})"));
            return None;
        }
    };

    let has_tauri_conf = root.join("src-tauri").join("tauri.conf.json").is_file();

    // 1 階層下の候補。読めないサブディレクトリは黙って除外する (壊れたシンボリックリンク等)。
    let subdirs: Vec<SubdirCandidate> = root_listing
        .dirs
        .iter()
        .filter(|name| !is_excluded_subdir(name))
        .map(|name| {
            let path = root.join(name);
            let listing = read_listing(&path).unwrap_or_default();
            let has_tauri_conf = path.join("src-tauri").join("tauri.conf.json").is_file();
            SubdirCandidate {
                name: name.clone(),
                listing,
                has_tauri_conf,
            }
        })
        .collect();

    let detection = detect::detect(&root_listing, has_tauri_conf, &subdirs);

    let folder_name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| root_str.clone());

    let display_name = ov
        .and_then(|o| o.display_name.clone())
        .unwrap_or(folder_name);

    let detected_working_dir = match &detection.working_dir_rel {
        Some(rel) => root.join(rel),
        None => root.to_path_buf(),
    };

    // working_dir_override は保存時に検証済みでも、その後フォルダが消えることがある。
    // その場合は警告を積んで検出結果に落とす (FR-P-32 の後追い検証)。
    let working_dir = match ov.and_then(|o| o.working_dir_override.as_deref()) {
        Some(value) => {
            let resolved = resolve_working_dir_override(root, value);
            if resolved.is_dir() {
                resolved
            } else {
                warnings.push(format!(
                    "{root_str}: 作業ディレクトリ上書き先が存在しません ({})。検出結果を使用します",
                    resolved.to_string_lossy()
                ));
                detected_working_dir
            }
        }
        None => detected_working_dir,
    };

    let script_keys = read_script_keys(&working_dir);
    let command_candidates = detect::command_candidates(&script_keys);
    let resolved_command =
        detect::resolve_command(ov.and_then(|o| o.command_override.as_deref()), &script_keys);

    let launch_blocked_reason = self_source_reason(key, self_key);

    Some(Project {
        path_key: key.to_string(),
        root_path: root_str,
        working_dir: working_dir.to_string_lossy().to_string(),
        display_name,
        kind: detection.kind,
        kind_label: detection.kind.label().to_string(),
        command_candidates,
        resolved_command,
        launch_blocked_reason,
        hidden: ov.map(|o| o.hidden).unwrap_or(false),
        archived: ov.map(|o| o.archived).unwrap_or(false),
        sort_order: ov.and_then(|o| o.sort_order),
        override_values: ov.cloned(),
        git: None,
        copilot: None,
        dev: dev.state_of(key),
    })
}

/// スキャン本体。渡されたフォルダを走査するだけ — 既定フォルダの解決は
/// `commands.rs` の責務 (FR-P-02)。
pub fn run(
    folders: &[String],
    using_default: bool,
    overrides: &HashMap<String, ProjectOverride>,
    dev: &DevRegistry,
    now_ms: i64,
) -> ProjectsSnapshot {
    let mut warnings = Vec::new();
    let mut projects = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let self_key = self_source_key();

    for folder in folders {
        let dir = Path::new(folder);
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(err) => {
                warnings.push(format!("{folder}: 読み取れませんでした ({err})"));
                continue;
            }
        };

        // 直下 1 階層のディレクトリだけを候補にする。ファイルは無視する (FR-P-01)。
        // `.` 始まりディレクトリ (`.git` / `.vscode` / `.obsidian` 等) はツールの
        // 管理領域であり「その他」プロジェクトとして並ぶのは誤検出になるため除外する。
        // ただし**無言で消さない** — 除外した件数を警告に積む (FR-P-13 / NFR-43)。
        // `.dotfiles` のような本物のプロジェクトを黙って落とすのを防ぐため。
        let (mut candidate_names, dot_dirs): (Vec<String>, Vec<String>) = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .partition(|name| !name.starts_with('.'));
        candidate_names.sort();
        if !dot_dirs.is_empty() {
            let mut shown = dot_dirs.clone();
            shown.sort();
            shown.truncate(5);
            warnings.push(format!(
                "{folder}: `.` で始まる {} 件のフォルダを候補から除外しました ({}{})",
                dot_dirs.len(),
                shown.join(", "),
                if dot_dirs.len() > 5 { ", …" } else { "" }
            ));
        }

        for name in candidate_names {
            let root = dir.join(&name);
            let root_str = root.to_string_lossy().to_string();
            let Some(key) = path_key::path_key(&root_str) else {
                warnings.push(format!("{root_str}: パスを解釈できませんでした"));
                continue;
            };
            if !seen.insert(key.clone()) {
                // 同じフォルダの多重登録・入れ子登録。後勝ちにせず最初の 1 件を残す。
                warnings.push(format!(
                    "{root_str}: 重複したスキャン対象のため 1 つだけ表示します"
                ));
                continue;
            }

            let ov = overrides.get(&key);
            if let Some(project) =
                build_project(&root, &key, ov, dev, self_key.as_deref(), &mut warnings)
            {
                projects.push(project);
            }
        }
    }

    projects.sort_by(|a, b| a.path_key.cmp(&b.path_key));

    ProjectsSnapshot {
        projects,
        scan_folders: folders.to_vec(),
        scanned_at: now_ms,
        using_default_folder: using_default,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::ProjectKind;
    use std::fs;

    fn fixture_root(name: &str) -> PathBuf {
        let base =
            std::env::temp_dir().join(format!("gh-dashboard-scan-test-{}", std::process::id()));
        let root = base.join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn cleanup(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }

    fn dev() -> DevRegistry {
        DevRegistry::new()
    }

    #[test]
    fn detects_nextjs_over_vite_via_run() {
        let scan_folder = fixture_root("nextjs_over_vite");
        let project = scan_folder.join("app");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("next.config.ts"), "").unwrap();
        fs::write(project.join("vite.config.ts"), "").unwrap();

        let folders = vec![scan_folder.to_string_lossy().to_string()];
        let snapshot = run(&folders, false, &HashMap::new(), &dev(), 0);

        assert_eq!(snapshot.projects.len(), 1);
        assert_eq!(snapshot.projects[0].kind, ProjectKind::Nextjs);
        cleanup(&scan_folder);
    }

    #[test]
    fn falls_back_to_subdirectory_working_dir() {
        let scan_folder = fixture_root("subdir_workingdir");
        let project = scan_folder.join("app");
        let frontend = project.join("frontend");
        fs::create_dir_all(&frontend).unwrap();
        fs::write(frontend.join("vite.config.ts"), "").unwrap();

        let folders = vec![scan_folder.to_string_lossy().to_string()];
        let snapshot = run(&folders, false, &HashMap::new(), &dev(), 0);

        assert_eq!(snapshot.projects.len(), 1);
        let p = &snapshot.projects[0];
        assert_eq!(p.kind, ProjectKind::Vite);
        assert!(p.working_dir.ends_with("frontend"), "{}", p.working_dir);
        cleanup(&scan_folder);
    }

    /// `.` 始まりのトップレベルディレクトリは候補から外すが、**無言では消さない**。
    /// 件数と名前を警告に積む (FR-P-13 / NFR-43)。
    #[test]
    fn dot_dirs_are_excluded_but_reported() {
        let scan_folder = fixture_root("dot_dirs_reported");
        fs::create_dir_all(scan_folder.join("real")).unwrap();
        fs::create_dir_all(scan_folder.join(".obsidian")).unwrap();
        fs::create_dir_all(scan_folder.join(".dotfiles")).unwrap();

        let folders = vec![scan_folder.to_string_lossy().to_string()];
        let snapshot = run(&folders, false, &HashMap::new(), &dev(), 0);

        assert_eq!(snapshot.projects.len(), 1);
        assert_eq!(snapshot.projects[0].display_name, "real");
        let note = snapshot
            .warnings
            .iter()
            .find(|w| w.contains("`.` で始まる"))
            .expect("除外した旨の警告が要る");
        assert!(note.contains("2 件"), "{note}");
        assert!(
            note.contains(".dotfiles") && note.contains(".obsidian"),
            "{note}"
        );
        cleanup(&scan_folder);
    }

    #[test]
    fn excludes_node_modules_from_subdir_search() {
        let scan_folder = fixture_root("excludes_node_modules");
        let project = scan_folder.join("app");
        let nested = project.join("node_modules").join("foo");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("vite.config.ts"), "").unwrap();

        let folders = vec![scan_folder.to_string_lossy().to_string()];
        let snapshot = run(&folders, false, &HashMap::new(), &dev(), 0);

        assert_eq!(snapshot.projects.len(), 1);
        assert_eq!(snapshot.projects[0].kind, ProjectKind::Other);
        cleanup(&scan_folder);
    }

    #[test]
    fn nonexistent_scan_folder_produces_warning_not_failure() {
        let missing =
            std::env::temp_dir().join("gh-dashboard-scan-test-missing-does-not-exist-xyz");
        let _ = fs::remove_dir_all(&missing);

        let scan_folder = fixture_root("alongside_missing");
        let project = scan_folder.join("app");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("Cargo.toml"), "").unwrap();

        let folders = vec![
            missing.to_string_lossy().to_string(),
            scan_folder.to_string_lossy().to_string(),
        ];
        let snapshot = run(&folders, false, &HashMap::new(), &dev(), 0);

        assert_eq!(snapshot.warnings.len(), 1);
        assert_eq!(snapshot.projects.len(), 1);
        cleanup(&scan_folder);
    }

    #[test]
    fn reads_command_candidates_from_package_json() {
        let scan_folder = fixture_root("scripts");
        let project = scan_folder.join("app");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("vite.config.ts"), "").unwrap();
        fs::write(
            project.join("package.json"),
            r#"{"scripts": {"test:watch": "vitest", "dev": "vite", "lint": "eslint ."}}"#,
        )
        .unwrap();

        let folders = vec![scan_folder.to_string_lossy().to_string()];
        let snapshot = run(&folders, false, &HashMap::new(), &dev(), 0);

        let p = &snapshot.projects[0];
        assert_eq!(
            p.command_candidates,
            vec!["npm run dev".to_string(), "npm run lint".to_string()]
        );
        assert_eq!(p.resolved_command.as_deref(), Some("npm run dev"));
        cleanup(&scan_folder);
    }

    #[test]
    fn broken_package_json_yields_no_candidates_without_error() {
        let scan_folder = fixture_root("broken_json");
        let project = scan_folder.join("app");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("package.json"), "{ not valid json").unwrap();

        let folders = vec![scan_folder.to_string_lossy().to_string()];
        let snapshot = run(&folders, false, &HashMap::new(), &dev(), 0);

        let p = &snapshot.projects[0];
        assert!(p.command_candidates.is_empty());
        assert_eq!(p.resolved_command, None);
        cleanup(&scan_folder);
    }

    #[test]
    fn manual_override_display_name_and_hidden_applied() {
        let scan_folder = fixture_root("override_display");
        let project = scan_folder.join("app");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("Cargo.toml"), "").unwrap();

        let key = path_key::path_key(&project.to_string_lossy()).unwrap();
        let mut overrides = HashMap::new();
        overrides.insert(
            key,
            ProjectOverride {
                display_name: Some("カスタム名".to_string()),
                hidden: true,
                ..Default::default()
            },
        );

        let folders = vec![scan_folder.to_string_lossy().to_string()];
        let snapshot = run(&folders, false, &overrides, &dev(), 0);

        let p = &snapshot.projects[0];
        assert_eq!(p.display_name, "カスタム名");
        assert!(p.hidden);
        cleanup(&scan_folder);
    }

    #[test]
    fn invalid_working_dir_override_falls_back_with_warning() {
        let scan_folder = fixture_root("bad_override_wd");
        let project = scan_folder.join("app");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("Cargo.toml"), "").unwrap();

        let key = path_key::path_key(&project.to_string_lossy()).unwrap();
        let mut overrides = HashMap::new();
        overrides.insert(
            key,
            ProjectOverride {
                working_dir_override: Some("does-not-exist".to_string()),
                ..Default::default()
            },
        );

        let folders = vec![scan_folder.to_string_lossy().to_string()];
        let snapshot = run(&folders, false, &overrides, &dev(), 0);

        assert_eq!(snapshot.warnings.len(), 1);
        let p = &snapshot.projects[0];
        assert_eq!(p.working_dir, project.to_string_lossy());
        cleanup(&scan_folder);
    }

    #[test]
    fn is_self_source_matches_by_key_equality() {
        assert!(is_self_source("d:\\app", "d:\\app"));
        assert!(!is_self_source("d:\\app", "d:\\other"));
    }
}
