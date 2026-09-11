//! Copilot 履歴とプロジェクトの紐付け (純粋)。
//!
//! 母集団・入力の絞り込み・mtime 近似は IO 層 (段階 2 の rust-io タスク) の
//! 責務。ここでは正規化済みの `SessionCandidate` の集合だけを受け取り、
//! 紐付け・フォールバック・競合解決を行う。
//!
//! 対応要求: FR-P-50〜58 / NFR-41 / NFR-50

use crate::copilot::SessionCandidate;
use crate::projects::{CopilotUsage, MatchedBy};
use std::collections::{BTreeMap, HashMap, HashSet};

/// 同一 path_key を持つ履歴候補の束。競合解決の単位 (FR-P-54)。
#[derive(Debug, Clone)]
pub struct CandidateGroup<'a> {
    /// 履歴側 cwd の path_key (プロジェクト側のキーとは限らない)
    pub source_key: String,
    pub matched_by: MatchedBy,
    pub sessions: Vec<&'a SessionCandidate>,
}

#[derive(Debug, Clone, Default)]
pub struct LinkResult {
    /// key = プロジェクト側の path_key。紐付かなかったプロジェクトはキーごと無い (FR-P-58)
    pub usage: HashMap<String, CopilotUsage>,
    pub unmatched_sessions: usize,
    pub unusable_sessions: usize,
}

/// path_key ごとに束ねる。path_key が `None` の候補は捨てる (呼び出し側が数える)。
pub fn group_by_path_key(candidates: &[SessionCandidate]) -> Vec<CandidateGroup<'_>> {
    let mut map: BTreeMap<String, Vec<&SessionCandidate>> = BTreeMap::new();
    for c in candidates {
        if let Some(pk) = &c.path_key {
            map.entry(pk.clone()).or_default().push(c);
        }
    }
    map.into_iter()
        .map(|(source_key, sessions)| CandidateGroup {
            source_key,
            // 暫定値。link() が Exact / FolderNameFallback を確定させる
            matched_by: MatchedBy::Exact,
            sessions,
        })
        .collect()
}

/// FR-P-52 の 3 条件。3 つすべて成立したときだけ `Some(project_key)`。
///
/// 1. 履歴側の cwd が実在しない (実在するなら別物として扱い、フォールバックしない)
/// 2. 履歴側の末尾フォルダ名と一致するプロジェクトが**ちょうど 1 件**
/// 3. そのプロジェクトに既に完全一致 (Exact) が付いていない (合算しない)
pub fn fallback_target(
    group: &CandidateGroup<'_>,
    projects_by_folder: &BTreeMap<String, Vec<String>>,
    exact_matched: &HashSet<String>,
) -> Option<String> {
    if group.sessions.iter().any(|s| s.cwd_exists) {
        return None;
    }
    let folder = group
        .sessions
        .iter()
        .find_map(|s| s.folder_name.as_deref())?;
    let matches = projects_by_folder.get(folder)?;
    if matches.len() != 1 {
        return None;
    }
    let project_key = &matches[0];
    if exact_matched.contains(project_key) {
        return None;
    }
    Some(project_key.clone())
}

fn newest_last_used_at(group: &CandidateGroup<'_>) -> Option<i64> {
    group.sessions.iter().filter_map(|s| s.last_used_at).max()
}

/// FR-P-54 の競合解決。
///
/// 1. Exact が 1 つでもあれば FolderNameFallback は捨てる (合算しない)
/// 2. 同種同士ならセッション数が多い方
/// 3. 同数なら last_used_at が新しい方 (None は最も古い扱い)
/// 4. それも同じなら source_key の昇順 (決定性のため)
pub fn pick_winner<'a, 'b>(groups: &'b [CandidateGroup<'a>]) -> Option<&'b CandidateGroup<'a>> {
    let has_exact = groups.iter().any(|g| g.matched_by == MatchedBy::Exact);
    let mut candidates: Vec<&CandidateGroup<'a>> = groups
        .iter()
        .filter(|g| !has_exact || g.matched_by == MatchedBy::Exact)
        .collect();

    candidates.sort_by(|a, b| {
        b.sessions
            .len()
            .cmp(&a.sessions.len())
            .then_with(|| newest_last_used_at(b).cmp(&newest_last_used_at(a)))
            .then_with(|| a.source_key.cmp(&b.source_key))
    });

    candidates.into_iter().next()
}

/// 束 → CopilotUsage。
///
/// `session_count` は集合の長さとして導出する (FR-C-51)。
/// `last_*` は「代表 1 件 (last_used_at が最大)」から取る。合算しない。
/// 全部 `None` なら代表を選ばず、それらのフィールドはすべて `None`。
pub fn aggregate(group: &CandidateGroup<'_>) -> CopilotUsage {
    let session_count = group.sessions.len() as i64;
    let is_active = group.sessions.iter().any(|s| s.is_active);
    let representative = group
        .sessions
        .iter()
        .filter(|s| s.last_used_at.is_some())
        .max_by_key(|s| s.last_used_at.unwrap());

    match representative {
        Some(s) => CopilotUsage {
            session_count,
            last_used_at: s.last_used_at,
            last_title: s.title.clone(),
            is_active,
            last_nano_aiu: s.total_nano_aiu,
            lines_added: s.lines_added,
            lines_removed: s.lines_removed,
            matched_by: group.matched_by,
        },
        None => CopilotUsage {
            session_count,
            last_used_at: None,
            last_title: None,
            is_active,
            last_nano_aiu: None,
            lines_added: None,
            lines_removed: None,
            matched_by: group.matched_by,
        },
    }
}

/// 紐付けの入口。exact を全部確定させてから fallback を評価する。
///
/// 1. `group_by_path_key` → path_key が `None` の候補は unusable_sessions に数える
/// 2. プロジェクト側から project_set と projects_by_folder を作る
/// 3. 第 1 パス: source_key が project_set に含まれる束を matched_by = Exact で確定
/// 4. 第 2 パス: 残りの束に fallback_target を適用
/// 5. バケツごとに pick_winner → aggregate → usage に入れる。敗れた束は unmatched_sessions に数える
/// 6. どのバケツにも入らなかった束のセッションも unmatched_sessions に数える
pub fn link(project_keys: &[String], candidates: &[SessionCandidate]) -> LinkResult {
    let mut result = LinkResult {
        unusable_sessions: candidates.iter().filter(|c| c.path_key.is_none()).count(),
        ..Default::default()
    };

    let mut groups = group_by_path_key(candidates);

    let project_set: HashSet<&str> = project_keys.iter().map(|s| s.as_str()).collect();
    let mut projects_by_folder: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for pk in project_keys {
        if let Some(folder) = crate::util::path_key::folder_name(pk) {
            projects_by_folder
                .entry(folder.to_string())
                .or_default()
                .push(pk.clone());
        }
    }

    // 第1パス: Exact を確定させる
    let mut exact_matched: HashSet<String> = HashSet::new();
    for g in groups.iter_mut() {
        if project_set.contains(g.source_key.as_str()) {
            g.matched_by = MatchedBy::Exact;
            exact_matched.insert(g.source_key.clone());
        }
    }

    // 第2パス: 残りにフォールバックを適用する (Exact が固まってから評価する)
    let mut targets: Vec<Option<String>> = vec![None; groups.len()];
    for (i, g) in groups.iter().enumerate() {
        if exact_matched.contains(&g.source_key) {
            targets[i] = Some(g.source_key.clone());
        }
    }
    for (i, g) in groups.iter().enumerate() {
        if targets[i].is_none() {
            targets[i] = fallback_target(g, &projects_by_folder, &exact_matched);
        }
    }
    for (i, g) in groups.iter_mut().enumerate() {
        if targets[i].is_some() && !exact_matched.contains(&g.source_key) {
            g.matched_by = MatchedBy::FolderNameFallback;
        }
    }

    // プロジェクトごとにバケツへ積み、勝者だけを usage に入れる
    let mut buckets: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, t) in targets.iter().enumerate() {
        match t {
            Some(pk) => buckets.entry(pk.clone()).or_default().push(i),
            None => result.unmatched_sessions += groups[i].sessions.len(),
        }
    }

    for (project_key, indices) in buckets {
        let bucket_groups: Vec<CandidateGroup> =
            indices.iter().map(|&i| groups[i].clone()).collect();
        if let Some(winner) = pick_winner(&bucket_groups) {
            let winner_key = winner.source_key.clone();
            for g in &bucket_groups {
                if g.source_key != winner_key {
                    result.unmatched_sessions += g.sessions.len();
                }
            }
            result.usage.insert(project_key, aggregate(winner));
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copilot::TimeSource;

    fn candidate(session_id: &str, path_key: Option<&str>) -> SessionCandidate {
        SessionCandidate {
            session_id: session_id.to_string(),
            cwd_raw: path_key.map(|s| s.to_string()),
            path_key: path_key.map(|s| s.to_string()),
            folder_name: path_key
                .and_then(crate::util::path_key::folder_name)
                .map(|s| s.to_string()),
            cwd_exists: true,
            client_name: None,
            title: None,
            last_used_at: None,
            last_used_source: TimeSource::None,
            total_nano_aiu: None,
            lines_added: None,
            lines_removed: None,
            is_active: false,
        }
    }

    fn orphan(session_id: &str, path_key: &str, last_used_at: Option<i64>) -> SessionCandidate {
        SessionCandidate {
            cwd_exists: false,
            last_used_at,
            ..candidate(session_id, Some(path_key))
        }
    }

    #[test]
    fn exact_cwd_match_links_with_correct_session_count() {
        let candidates = vec![
            candidate("s1", Some("d:\\projects\\foo")),
            candidate("s2", Some("d:\\projects\\foo")),
        ];
        let projects = vec!["d:\\projects\\foo".to_string()];
        let result = link(&projects, &candidates);
        let usage = result.usage.get("d:\\projects\\foo").unwrap();
        assert_eq!(usage.session_count, 2);
        assert_eq!(usage.matched_by, MatchedBy::Exact);
        assert_eq!(result.unmatched_sessions, 0);
    }

    #[test]
    fn case_insensitive_cwd_merges_under_one_project() {
        // path_key 正規化は util 側の責務。ここでは既に正規化済みの値が同じであることを前提とする
        let candidates = vec![
            candidate("s1", Some("d:\\projects\\foo")),
            candidate("s2", Some("d:\\projects\\foo")),
        ];
        let projects = vec!["d:\\projects\\foo".to_string()];
        let result = link(&projects, &candidates);
        assert_eq!(result.usage.len(), 1);
        assert_eq!(result.usage["d:\\projects\\foo"].session_count, 2);
    }

    #[test]
    fn missing_cwd_with_single_matching_folder_name_falls_back() {
        let candidates = vec![orphan("s1", "d:\\old\\foo", None)];
        let projects = vec!["d:\\new\\foo".to_string()];
        let result = link(&projects, &candidates);
        let usage = result.usage.get("d:\\new\\foo").unwrap();
        assert_eq!(usage.matched_by, MatchedBy::FolderNameFallback);
        assert_eq!(usage.session_count, 1);
        assert_eq!(result.unmatched_sessions, 0);
    }

    /// FR-P-52 条件①: cwd が実在するなら、一致するプロジェクトが無くてもフォールバックしない
    #[test]
    fn existing_cwd_without_exact_project_does_not_fall_back() {
        let candidates = vec![candidate("s1", Some("d:\\gone\\foo"))]; // cwd_exists = true (デフォルト)
        let projects = vec!["d:\\new\\foo".to_string()];
        let result = link(&projects, &candidates);
        assert!(result.usage.is_empty());
        assert_eq!(result.unmatched_sessions, 1);
    }

    /// FR-P-52 条件②: 同名プロジェクトが 2 件あれば一意に決まらないので紐付けない
    #[test]
    fn ambiguous_folder_name_with_two_projects_does_not_fall_back() {
        let candidates = vec![orphan("s1", "d:\\old\\foo", None)];
        let projects = vec!["d:\\a\\foo".to_string(), "d:\\b\\foo".to_string()];
        let result = link(&projects, &candidates);
        assert!(result.usage.is_empty());
        assert_eq!(result.unmatched_sessions, 1);
    }

    /// FR-P-52 条件③: 同名プロジェクトに既に完全一致があるなら合算しない
    #[test]
    fn folder_name_target_with_existing_exact_match_does_not_fall_back() {
        let candidates = vec![
            candidate("s1", Some("d:\\new\\foo")), // exact
            orphan("s2", "d:\\old\\foo", None),    // fallback candidate, target already has exact
        ];
        let projects = vec!["d:\\new\\foo".to_string()];
        let result = link(&projects, &candidates);
        let usage = result.usage.get("d:\\new\\foo").unwrap();
        assert_eq!(usage.session_count, 1); // exact のみ。fallback は合算されない
        assert_eq!(usage.matched_by, MatchedBy::Exact);
        assert_eq!(result.unmatched_sessions, 1); // fallback 候補は捨てられた
    }

    /// FR-P-54: 同じフォルダ名の孤児が 2 束あるとき、セッション数が多い方だけ採用する
    #[test]
    fn conflicting_orphans_pick_the_one_with_more_sessions() {
        let candidates = vec![
            orphan("s1", "d:\\old-a\\foo", None),
            orphan("s2", "d:\\old-b\\foo", None),
            orphan("s3", "d:\\old-b\\foo", None),
        ];
        let projects = vec!["d:\\new\\foo".to_string()];
        let result = link(&projects, &candidates);
        let usage = result.usage.get("d:\\new\\foo").unwrap();
        assert_eq!(usage.session_count, 2); // old-b の束 (2件) が勝つ
        assert_eq!(result.unmatched_sessions, 1); // old-a の束 (1件) は敗れる
    }

    #[test]
    fn conflicting_orphans_with_equal_count_pick_newer_last_used_at() {
        let candidates = vec![
            orphan("s1", "d:\\old-a\\foo", Some(100)),
            orphan("s2", "d:\\old-b\\foo", Some(200)),
        ];
        let projects = vec!["d:\\new\\foo".to_string()];
        let result = link(&projects, &candidates);
        let usage = result.usage.get("d:\\new\\foo").unwrap();
        assert_eq!(usage.last_used_at, Some(200));
        assert_eq!(result.unmatched_sessions, 1);
    }

    #[test]
    fn conflicting_orphans_with_equal_count_and_time_are_decided_by_source_key_and_input_order_independent(
    ) {
        let candidates_a = vec![
            orphan("s1", "d:\\old-a\\foo", None),
            orphan("s2", "d:\\old-b\\foo", None),
        ];
        let candidates_b = vec![
            orphan("s2", "d:\\old-b\\foo", None),
            orphan("s1", "d:\\old-a\\foo", None),
        ];
        let projects = vec!["d:\\new\\foo".to_string()];
        let result_a = link(&projects, &candidates_a);
        let result_b = link(&projects, &candidates_b);
        // "d:\old-a\foo" < "d:\old-b\foo" 昇順で old-a が勝つ。入力順を変えても同じ結果
        assert_eq!(
            result_a.usage.get("d:\\new\\foo").unwrap().session_count,
            result_b.usage.get("d:\\new\\foo").unwrap().session_count
        );
        assert_eq!(result_a.unmatched_sessions, result_b.unmatched_sessions);
    }

    #[test]
    fn project_without_any_history_has_no_usage_entry() {
        let candidates: Vec<SessionCandidate> = vec![];
        let projects = vec!["d:\\projects\\foo".to_string()];
        let result = link(&projects, &candidates);
        assert!(!result.usage.contains_key("d:\\projects\\foo"));
        assert!(result.usage.is_empty());
    }

    #[test]
    fn all_last_used_at_none_leaves_representative_fields_none_but_count_correct() {
        let candidates = vec![
            candidate("s1", Some("d:\\projects\\foo")),
            candidate("s2", Some("d:\\projects\\foo")),
        ];
        let projects = vec!["d:\\projects\\foo".to_string()];
        let result = link(&projects, &candidates);
        let usage = result.usage.get("d:\\projects\\foo").unwrap();
        assert_eq!(usage.session_count, 2);
        assert_eq!(usage.last_used_at, None);
        assert_eq!(usage.last_title, None);
        assert_eq!(usage.last_nano_aiu, None);
    }

    /// path_key が無い候補は他の結果に影響せず unusable として数えられるだけ
    #[test]
    fn candidate_without_cwd_is_unusable_and_does_not_affect_others() {
        let mut broken = candidate("broken", None);
        broken.cwd_raw = None;
        let candidates = vec![broken, candidate("s1", Some("d:\\projects\\foo"))];
        let projects = vec!["d:\\projects\\foo".to_string()];
        let result = link(&projects, &candidates);
        assert_eq!(result.unusable_sessions, 1);
        assert_eq!(result.usage["d:\\projects\\foo"].session_count, 1);
    }

    #[test]
    fn empty_input_does_not_panic() {
        let result = link(&[], &[]);
        assert!(result.usage.is_empty());
        assert_eq!(result.unmatched_sessions, 0);
        assert_eq!(result.unusable_sessions, 0);
    }
}
