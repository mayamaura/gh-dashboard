//! プロジェクト種別の判定と起動コマンドの解決 (純粋)。
//!
//! **このモジュールに IO を持ち込まない。** 判定材料はマニフェストファイルの
//! 「存在有無」だけで、中身は読まない (FR-P-10)。呼び出し側がディレクトリを
//! 走査してファイル名の一覧を渡す。
//!
//! 対応要求: FR-P-10〜14 / FR-P-20〜23 / NFR-50

use serde::{Deserialize, Serialize};

/// プロジェクト種別。判定の優先順はこの宣言順と一致させる (FR-P-11)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectKind {
    Tauri,
    Nextjs,
    Sveltekit,
    Vite,
    PythonPackage,
    Rust,
    Python,
    Notebook,
    Other,
}

impl ProjectKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tauri => "Tauri",
            Self::Nextjs => "Next.js",
            Self::Sveltekit => "SvelteKit",
            Self::Vite => "Vite SPA",
            Self::PythonPackage => "Python パッケージ",
            Self::Rust => "Rust",
            Self::Python => "Python",
            Self::Notebook => "Notebook",
            Self::Other => "その他",
        }
    }
}

/// 1 ディレクトリ分の「見えたもの」。呼び出し側が走査して埋める。
#[derive(Debug, Clone, Default)]
pub struct DirListing {
    /// 直下のファイル名 (ディレクトリ名は含めない)
    pub files: Vec<String>,
    /// 直下のディレクトリ名
    pub dirs: Vec<String>,
}

impl DirListing {
    pub fn has(&self, name: &str) -> bool {
        self.files.iter().any(|f| f.eq_ignore_ascii_case(name))
    }

    pub fn has_dir(&self, name: &str) -> bool {
        self.dirs.iter().any(|d| d.eq_ignore_ascii_case(name))
    }

    pub fn has_ext(&self, ext: &str) -> bool {
        let suffix = format!(".{ext}");
        self.files
            .iter()
            .any(|f| f.to_ascii_lowercase().ends_with(&suffix))
    }

    /// 拡張子違いを許すマニフェスト検出 (`next.config.{js,ts,mjs,cjs,mts,cts}` 等)
    pub fn has_stem(&self, stem: &str, exts: &[&str]) -> bool {
        exts.iter().any(|e| self.has(&format!("{stem}.{e}")))
    }
}

const CONFIG_EXTS: &[&str] = &["js", "ts", "mjs", "cjs", "mts", "cts"];

/// ルート直下だけを見た種別判定。該当しなければ `None`。
///
/// `src-tauri/tauri.conf.json` の判定には子ディレクトリの中身が要るため、
/// 呼び出し側が `has_tauri_conf` として渡す。
pub fn detect_in_dir(listing: &DirListing, has_tauri_conf: bool) -> Option<ProjectKind> {
    // 優先順は FR-P-11 のとおり。**Next.js を Vite より必ず先に見る。**
    // Next.js プロジェクトが Vitest 用の vite.config.ts を持つことは珍しくない。
    if has_tauri_conf {
        return Some(ProjectKind::Tauri);
    }
    if listing.has_stem("next.config", CONFIG_EXTS) {
        return Some(ProjectKind::Nextjs);
    }
    if listing.has("svelte.config.js") || listing.has_stem("svelte.config", CONFIG_EXTS) {
        return Some(ProjectKind::Sveltekit);
    }
    if listing.has_stem("vite.config", CONFIG_EXTS) {
        return Some(ProjectKind::Vite);
    }
    if listing.has("pyproject.toml") {
        return Some(ProjectKind::PythonPackage);
    }
    if listing.has("Cargo.toml") {
        return Some(ProjectKind::Rust);
    }
    if listing.has("requirements.txt") {
        return Some(ProjectKind::Python);
    }
    if listing.has_ext("ipynb") {
        return Some(ProjectKind::Notebook);
    }
    None
}

/// サブディレクトリの候補。`detect` に渡す。
#[derive(Debug, Clone)]
pub struct SubdirCandidate {
    pub name: String,
    pub listing: DirListing,
    pub has_tauri_conf: bool,
}

/// 判定結果。`working_dir_rel` が `None` ならルートが作業ディレクトリ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub kind: ProjectKind,
    /// ルートからの相対サブディレクトリ名 (FR-P-12)
    pub working_dir_rel: Option<String>,
}

/// 種別判定の本体 (FR-P-10〜13)。
///
/// 1. ルート直下で判定する
/// 2. 付かなければ**1 階層下だけ**を探す
/// 3. 複数候補が出たら優先順位が最上位、同順位ならディレクトリ名昇順で 1 つに決める
/// 4. どれにも該当しなければ `Other` (**一覧から隠さない**)
pub fn detect(
    root: &DirListing,
    root_has_tauri_conf: bool,
    subdirs: &[SubdirCandidate],
) -> Detection {
    if let Some(kind) = detect_in_dir(root, root_has_tauri_conf) {
        return Detection {
            kind,
            working_dir_rel: None,
        };
    }

    // 1 階層下。優先順位 (ProjectKind の Ord) が最上位、同順位なら名前昇順。
    let mut best: Option<(ProjectKind, &str)> = None;
    for sub in subdirs {
        let Some(kind) = detect_in_dir(&sub.listing, sub.has_tauri_conf) else {
            continue;
        };
        best = match best {
            None => Some((kind, sub.name.as_str())),
            Some((best_kind, best_name)) => {
                if kind < best_kind || (kind == best_kind && sub.name.as_str() < best_name) {
                    Some((kind, sub.name.as_str()))
                } else {
                    Some((best_kind, best_name))
                }
            }
        };
    }

    match best {
        Some((kind, name)) => Detection {
            kind,
            working_dir_rel: Some(name.to_string()),
        },
        None => Detection {
            kind: ProjectKind::Other,
            working_dir_rel: None,
        },
    }
}

// ---------------------------------------------------------------- 起動コマンド

/// 起動候補として提示してよい npm スクリプト。**この順で提示する** (FR-P-20)。
pub const ALLOWED_SCRIPTS: &[&str] = &["dev", "build", "start", "preview", "test", "lint"];

/// `package.json` の scripts のキー一覧から、提示する候補を許可リスト順で返す。
///
/// `test:watch` のような派生キーは含めない (完全一致のみ)。
pub fn command_candidates(script_keys: &[String]) -> Vec<String> {
    ALLOWED_SCRIPTS
        .iter()
        .filter(|allowed| script_keys.iter().any(|k| k == *allowed))
        .map(|allowed| format!("npm run {allowed}"))
        .collect()
}

/// 実行コマンドの解決 (FR-P-21)。
///
/// ①上書き → ②`dev` → ③`start` → ④なし (起動不可)
pub fn resolve_command(override_cmd: Option<&str>, script_keys: &[String]) -> Option<String> {
    if let Some(cmd) = override_cmd {
        let trimmed = cmd.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if script_keys.iter().any(|k| k == "dev") {
        return Some("npm run dev".to_string());
    }
    if script_keys.iter().any(|k| k == "start") {
        return Some("npm run start".to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(files: &[&str]) -> DirListing {
        DirListing {
            files: files.iter().map(|s| s.to_string()).collect(),
            dirs: Vec::new(),
        }
    }

    fn detect_root(files: &[&str]) -> Detection {
        detect(&listing(files), false, &[])
    }

    #[test]
    fn tauri_wins_over_package_json() {
        let d = detect(&listing(&["package.json"]), true, &[]);
        assert_eq!(d.kind, ProjectKind::Tauri);
    }

    /// FR-P-11 の核心。素朴な判定順ではここで落ちる。
    #[test]
    fn nextjs_wins_over_vite_when_both_present() {
        let d = detect_root(&["next.config.ts", "vite.config.ts", "package.json"]);
        assert_eq!(d.kind, ProjectKind::Nextjs);
    }

    #[test]
    fn nextjs_config_extensions() {
        for ext in ["js", "ts", "mjs", "cjs", "mts", "cts"] {
            let name = format!("next.config.{ext}");
            let d = detect_root(&[&name]);
            assert_eq!(
                d.kind,
                ProjectKind::Nextjs,
                "拡張子 {ext} で判定できていない"
            );
        }
    }

    #[test]
    fn sveltekit_detected() {
        assert_eq!(
            detect_root(&["svelte.config.js"]).kind,
            ProjectKind::Sveltekit
        );
    }

    #[test]
    fn python_package_wins_over_requirements() {
        let d = detect_root(&["pyproject.toml", "requirements.txt"]);
        assert_eq!(d.kind, ProjectKind::PythonPackage);
    }

    #[test]
    fn notebook_detected_by_extension() {
        assert_eq!(detect_root(&["analysis.ipynb"]).kind, ProjectKind::Notebook);
    }

    #[test]
    fn unknown_becomes_other_not_hidden() {
        // FR-P-13: 該当しなくても「その他」として扱い、一覧から隠さない
        let d = detect_root(&["README.md", "notes.txt"]);
        assert_eq!(d.kind, ProjectKind::Other);
        assert_eq!(d.working_dir_rel, None);
    }

    #[test]
    fn falls_back_to_single_subdirectory() {
        // FR-P-12: ルートにマニフェストが無ければ 1 階層下だけを探す
        let subs = vec![SubdirCandidate {
            name: "frontend".to_string(),
            listing: listing(&["vite.config.ts"]),
            has_tauri_conf: false,
        }];
        let d = detect(&listing(&["README.md"]), false, &subs);
        assert_eq!(d.kind, ProjectKind::Vite);
        assert_eq!(d.working_dir_rel.as_deref(), Some("frontend"));
    }

    #[test]
    fn picks_highest_priority_subdirectory() {
        // frontend/ と backend/ の両方にマニフェストがある実在ケース
        let subs = vec![
            SubdirCandidate {
                name: "backend".to_string(),
                listing: listing(&["pyproject.toml"]),
                has_tauri_conf: false,
            },
            SubdirCandidate {
                name: "frontend".to_string(),
                listing: listing(&["next.config.ts"]),
                has_tauri_conf: false,
            },
        ];
        let d = detect(&listing(&[]), false, &subs);
        // Next.js (2 位) が Python パッケージ (5 位) に優先する
        assert_eq!(d.kind, ProjectKind::Nextjs);
        assert_eq!(d.working_dir_rel.as_deref(), Some("frontend"));
    }

    #[test]
    fn ties_broken_by_directory_name_ascending() {
        let subs = vec![
            SubdirCandidate {
                name: "web".to_string(),
                listing: listing(&["vite.config.ts"]),
                has_tauri_conf: false,
            },
            SubdirCandidate {
                name: "admin".to_string(),
                listing: listing(&["vite.config.ts"]),
                has_tauri_conf: false,
            },
        ];
        let d = detect(&listing(&[]), false, &subs);
        assert_eq!(d.working_dir_rel.as_deref(), Some("admin"));
    }

    #[test]
    fn root_detection_beats_subdirectory() {
        let subs = vec![SubdirCandidate {
            name: "frontend".to_string(),
            listing: listing(&["next.config.ts"]),
            has_tauri_conf: false,
        }];
        let d = detect(&listing(&["Cargo.toml"]), false, &subs);
        assert_eq!(d.kind, ProjectKind::Rust);
        assert_eq!(d.working_dir_rel, None);
    }

    // ---- 起動コマンド ----

    #[test]
    fn command_candidates_follow_allowlist_order() {
        let keys: Vec<String> = ["lint", "dev", "test:watch", "build"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            command_candidates(&keys),
            vec!["npm run dev", "npm run build", "npm run lint"]
        );
    }

    #[test]
    fn command_candidates_exclude_derived_keys() {
        // `test:watch` は許可リストに完全一致しないので含めない
        let keys = vec!["test:watch".to_string()];
        assert!(command_candidates(&keys).is_empty());
    }

    #[test]
    fn resolve_prefers_override() {
        let keys = vec!["dev".to_string()];
        assert_eq!(
            resolve_command(Some(".\\backend\\.venv\\Scripts\\python.exe -m app"), &keys)
                .as_deref(),
            Some(".\\backend\\.venv\\Scripts\\python.exe -m app")
        );
    }

    #[test]
    fn resolve_ignores_blank_override() {
        let keys = vec!["dev".to_string()];
        assert_eq!(
            resolve_command(Some("   "), &keys).as_deref(),
            Some("npm run dev")
        );
    }

    #[test]
    fn resolve_falls_back_to_start_then_none() {
        assert_eq!(
            resolve_command(None, &["start".to_string()]).as_deref(),
            Some("npm run start")
        );
        // 起動不可は None。空文字で埋めない
        assert_eq!(resolve_command(None, &["lint".to_string()]), None);
    }
}
