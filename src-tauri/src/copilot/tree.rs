//! サブエージェント系統の木構築 (純粋)。
//!
//! **系統図とガントは同じ木構築結果を使う** (FR-C-114)。生データの「深さ」は
//! 欠落しうるので信頼しない — 親子関係だけから深さを導く。
//!
//! 対応要求: FR-C-113 / FR-C-114 / FR-C-116 / NFR-50

use std::collections::{HashMap, HashSet};

/// 木構築の入力。DB の `subagent_runs` 1 行に対応する最小限。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunNodeInput {
    pub run_key: String,
    /// `None` = セッション直下
    pub parent_key: Option<String>,
    pub started_at: Option<i64>,
    /// 生データの深さ。**木構築では使わない** (FR-C-114)。保持のみ
    pub reported_depth: Option<i32>,
}

/// 木構築の結果 1 行。表示順に並ぶ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunNode {
    pub run_key: String,
    pub parent_key: Option<String>,
    /// 親子関係から導いた深さ
    pub depth: usize,
    /// 親が見つからずルート直下に置かれたか (FR-C-113)
    pub orphaned: bool,
    pub child_keys: Vec<String>,
}

/// 親子関係を辿って木を構築し、**表示順の平坦なリスト**にして返す。
///
/// - 親が見つからない孤児はルート直下に置く。**捨てない** (FR-C-113)
/// - 循環参照をガードする。無限ループしない (FR-C-113)
/// - 兄弟は開始時刻昇順、時刻が無いものは `run_key` 昇順
pub fn build(nodes: &[RunNodeInput]) -> Vec<RunNode> {
    let known: HashSet<&str> = nodes.iter().map(|n| n.run_key.as_str()).collect();

    // 親 -> 子。親が未知なら孤児としてルート直下 (None) に付け替える。
    let mut children: HashMap<Option<String>, Vec<&RunNodeInput>> = HashMap::new();
    let mut orphans: HashSet<&str> = HashSet::new();

    for n in nodes {
        let parent = match &n.parent_key {
            Some(p) if known.contains(p.as_str()) && p != &n.run_key => Some(p.clone()),
            Some(_) => {
                // 親が存在しない、または自己参照。ルート直下に置く
                orphans.insert(n.run_key.as_str());
                None
            }
            None => None,
        };
        children.entry(parent).or_default().push(n);
    }

    for list in children.values_mut() {
        list.sort_by(|a, b| match (a.started_at, b.started_at) {
            (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.run_key.cmp(&b.run_key)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.run_key.cmp(&b.run_key),
        });
    }

    let mut out = Vec::with_capacity(nodes.len());
    // 循環参照のガード: 一度出した行は二度と辿らない
    let mut emitted: HashSet<String> = HashSet::new();

    // 深さ優先で、親のすぐ下に子が並ぶ順に出す
    fn walk(
        parent: Option<String>,
        depth: usize,
        children: &HashMap<Option<String>, Vec<&RunNodeInput>>,
        orphans: &HashSet<&str>,
        emitted: &mut HashSet<String>,
        out: &mut Vec<RunNode>,
    ) {
        let Some(list) = children.get(&parent) else {
            return;
        };
        for n in list {
            if !emitted.insert(n.run_key.clone()) {
                // すでに出している = 循環。ここで打ち切る
                continue;
            }
            let child_keys = children
                .get(&Some(n.run_key.clone()))
                .map(|c| c.iter().map(|x| x.run_key.clone()).collect())
                .unwrap_or_default();
            out.push(RunNode {
                run_key: n.run_key.clone(),
                parent_key: n.parent_key.clone(),
                depth,
                orphaned: orphans.contains(n.run_key.as_str()),
                child_keys,
            });
            walk(
                Some(n.run_key.clone()),
                depth + 1,
                children,
                orphans,
                emitted,
                out,
            );
        }
    }

    walk(None, 0, &children, &orphans, &mut emitted, &mut out);

    // 循環に閉じ込められて未出力の行があれば、ルート直下に救出する (捨てない)
    for n in nodes {
        if !emitted.contains(&n.run_key) {
            emitted.insert(n.run_key.clone());
            out.push(RunNode {
                run_key: n.run_key.clone(),
                parent_key: n.parent_key.clone(),
                depth: 0,
                orphaned: true,
                child_keys: Vec::new(),
            });
        }
    }

    out
}

/// ライブ表示用の折りたたみ (FR-C-116)。
///
/// 完了したエージェントを各階層で 1 行に畳む。ただし**稼働中の子孫を持つノードは
/// 畳まない (祖先保護)**。畳んだ結果 0 件になっても折りたたみは維持する。
///
/// `running` は**ライブ集合**。DB の状態列ではない (FR-C-115)。
pub fn visible_for_live(tree: &[RunNode], running: &HashSet<String>) -> Vec<String> {
    // 稼働中ノードとその祖先を残す
    let parent_of: HashMap<&str, Option<&str>> = tree
        .iter()
        .map(|n| (n.run_key.as_str(), n.parent_key.as_deref()))
        .collect();

    let mut keep: HashSet<&str> = HashSet::new();
    for key in running {
        let mut cur: Option<&str> = Some(key.as_str());
        let mut guard = 0usize;
        while let Some(k) = cur {
            if !keep.insert(k) {
                break; // すでに辿った = 循環か合流
            }
            guard += 1;
            if guard > tree.len() + 1 {
                break; // 循環ガード
            }
            cur = parent_of.get(k).copied().flatten();
        }
    }

    tree.iter()
        .filter(|n| keep.contains(n.run_key.as_str()))
        .map(|n| n.run_key.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(key: &str, parent: Option<&str>, started: Option<i64>) -> RunNodeInput {
        RunNodeInput {
            run_key: key.to_string(),
            parent_key: parent.map(|s| s.to_string()),
            started_at: started,
            reported_depth: None,
        }
    }

    fn keys(tree: &[RunNode]) -> Vec<&str> {
        tree.iter().map(|n| n.run_key.as_str()).collect()
    }

    #[test]
    fn builds_three_level_tree() {
        let t = build(&[
            node("a", None, Some(1)),
            node("b", Some("a"), Some(2)),
            node("c", Some("b"), Some(3)),
        ]);
        assert_eq!(keys(&t), vec!["a", "b", "c"]);
        assert_eq!(t[0].depth, 0);
        assert_eq!(t[1].depth, 1);
        assert_eq!(t[2].depth, 2);
    }

    #[test]
    fn null_parent_is_session_root() {
        let t = build(&[node("a", None, Some(1)), node("b", None, Some(2))]);
        assert_eq!(t.iter().filter(|n| n.depth == 0).count(), 2);
    }

    /// FR-C-113: 親が見つからない孤児はルート直下に置く。捨てない。
    #[test]
    fn orphan_is_placed_at_root_not_dropped() {
        let t = build(&[node("a", None, Some(1)), node("x", Some("missing"), Some(2))]);
        assert_eq!(t.len(), 2, "孤児を捨ててはいけない");
        let orphan = t.iter().find(|n| n.run_key == "x").unwrap();
        assert_eq!(orphan.depth, 0);
        assert!(orphan.orphaned, "推測で置いたことが分かるようにする");
    }

    /// FR-C-113: 循環参照で無限ループしない。
    #[test]
    fn cycle_does_not_loop_forever() {
        let t = build(&[
            node("a", Some("b"), Some(1)),
            node("b", Some("a"), Some(2)),
        ]);
        assert_eq!(t.len(), 2, "循環でも全行が 1 度ずつ出る");
    }

    #[test]
    fn self_reference_is_treated_as_orphan() {
        let t = build(&[node("a", Some("a"), Some(1))]);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].depth, 0);
        assert!(t[0].orphaned);
    }

    /// FR-C-114: 生データの depth が実際の階層と食い違っても、親子関係を採る。
    #[test]
    fn ignores_reported_depth() {
        let mut child = node("b", Some("a"), Some(2));
        child.reported_depth = Some(7); // でたらめな値
        let t = build(&[node("a", None, Some(1)), child]);
        assert_eq!(t[1].depth, 1, "reported_depth を信じてはいけない");
    }

    #[test]
    fn siblings_sorted_by_start_time() {
        let t = build(&[
            node("root", None, Some(0)),
            node("late", Some("root"), Some(200)),
            node("early", Some("root"), Some(100)),
        ]);
        assert_eq!(keys(&t), vec!["root", "early", "late"]);
    }

    #[test]
    fn siblings_without_time_sort_by_key_and_come_last() {
        let t = build(&[
            node("root", None, Some(0)),
            node("z", Some("root"), None),
            node("a", Some("root"), None),
            node("timed", Some("root"), Some(5)),
        ]);
        assert_eq!(keys(&t), vec!["root", "timed", "a", "z"]);
    }

    /// 系統図とガントが同じ行順を使えること (FR-C-114)
    #[test]
    fn build_is_deterministic() {
        let input = vec![
            node("b", Some("a"), Some(2)),
            node("a", None, Some(1)),
            node("c", Some("a"), Some(3)),
        ];
        assert_eq!(keys(&build(&input)), keys(&build(&input)));
    }

    // ---- ライブ折りたたみ (FR-C-116) ----

    #[test]
    fn live_view_keeps_running_nodes_and_their_ancestors() {
        let t = build(&[
            node("a", None, Some(1)),
            node("b", Some("a"), Some(2)),
            node("c", Some("b"), Some(3)),
            node("done", Some("a"), Some(4)),
        ]);
        let running: HashSet<String> = ["c".to_string()].into_iter().collect();
        let visible = visible_for_live(&t, &running);
        // 祖先保護: c が稼働中なので b と a も畳まない
        assert!(visible.contains(&"a".to_string()));
        assert!(visible.contains(&"b".to_string()));
        assert!(visible.contains(&"c".to_string()));
        // 完了したきょうだいは畳む
        assert!(!visible.contains(&"done".to_string()));
    }

    #[test]
    fn live_view_is_empty_when_nothing_runs() {
        let t = build(&[node("a", None, Some(1)), node("b", Some("a"), Some(2))]);
        let visible = visible_for_live(&t, &HashSet::new());
        // 0 件でも折りたたみは維持する。UI 側で「稼働中のエージェントはありません」を出す
        assert!(visible.is_empty());
    }

    #[test]
    fn live_view_handles_cycle_without_hanging() {
        let t = build(&[
            node("a", Some("b"), Some(1)),
            node("b", Some("a"), Some(2)),
        ]);
        let running: HashSet<String> = ["a".to_string()].into_iter().collect();
        let visible = visible_for_live(&t, &running);
        assert!(!visible.is_empty());
    }
}
