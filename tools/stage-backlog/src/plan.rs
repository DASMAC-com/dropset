//! The deterministic planner: from a set of [`Issue`]s, render the chips-only
//! Task Staging tree.
//!
//! The shape mirrors the `stage-backlog` skill's step 5:
//!
//! * Issues bucket under `# Skills` (pure skill-suite work), a `# ENG-###`
//!   heading per parent with 2+ Backlog subtasks, or a trailing
//!   `# Standalone`.
//! * Within a bucket, issues nest by blocker. A blocker is **declared**
//!   (`blockedBy` / `blocks`) or inferred from a **file overlap** (two issues
//!   touching the same path can't run in parallel, so the higher-numbered one
//!   nests under the lower). The merge-into-one-PR step is left to the skill's
//!   agent pass; the binary represents a same-file collision as a serial
//!   nesting, which keeps the parallelism guarantee without mutating Linear.
//! * A blocker under a different heading can't nest across it, so it renders
//!   as a trailing `(after ENG-###)` note; a second in-heading blocker the
//!   nesting can't show renders as `(also after ENG-###)`.
//!
//! Every reference is a bare `ENG-###` tag so Linear resolves it to a live
//! chip. The render is a pure function of its input and fully deterministic:
//! all iteration that reaches the output is sorted by issue number.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::model::{touches_overlap, Issue};

const INDENT: &str = "    "; // four spaces per nesting level

/// The heading an issue renders under.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Bucket {
    Skills,
    Parent(String),
    Standalone,
}

/// Identifiers of issues that have no `**Touches**:` field — the planner can
/// place them only by declared edges / parent, so the caller warns and the
/// skill's agent step fills the gap.
pub fn missing_touches(issues: &[Issue]) -> Vec<String> {
    issues
        .iter()
        .filter(|i| i.touches.is_empty())
        .map(|i| i.id.clone())
        .collect()
}

/// Render the full Task Staging document body for `issues`.
pub fn render(issues: &[Issue]) -> String {
    if issues.is_empty() {
        return String::new();
    }

    let universe: HashSet<&str> = issues.iter().map(|i| i.id.as_str()).collect();
    let number_of: HashMap<&str, u64> = issues.iter().map(|i| (i.id.as_str(), i.number)).collect();

    let blockers = compute_blockers(issues, &universe);
    let buckets = compute_buckets(issues);

    // In-bucket blockers drive nesting; cross-bucket ones become notes.
    let same_bucket = |a: &str, b: &str| buckets.get(a) == buckets.get(b);

    // chain_len = longest in-bucket blocker chain below an issue; the primary
    // blocker (the one to nest under) is the in-bucket blocker that settles
    // last, i.e. the deepest chain, tie-broken by highest number.
    let primary: HashMap<&str, Option<String>> = issues
        .iter()
        .map(|i| {
            let pick = blockers[&i.id]
                .iter()
                .filter(|b| same_bucket(&i.id, b))
                .max_by_key(|b| {
                    (
                        chain_len(b, &blockers, &same_bucket, &mut HashSet::new()),
                        number_of.get(b.as_str()).copied().unwrap_or(0),
                    )
                })
                .cloned();
            (i.id.as_str(), pick)
        })
        .collect();

    // children[parent] = issues whose primary is `parent`.
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for i in issues {
        if let Some(Some(p)) = primary.get(i.id.as_str()) {
            children.entry(p.as_str()).or_default().push(i.id.as_str());
        }
    }

    let mut sections: Vec<String> = Vec::new();

    // # Skills first.
    if let Some(s) = render_bucket(
        "# Skills",
        &Bucket::Skills,
        issues,
        &buckets,
        &primary,
        &children,
        &blockers,
        &number_of,
    ) {
        sections.push(s);
    }

    // # ENG-### parent headings, ordered by parent number.
    let mut parents: Vec<&str> = buckets
        .values()
        .filter_map(|b| match b {
            Bucket::Parent(p) => Some(p.as_str()),
            _ => None,
        })
        .collect();
    parents.sort_by_key(|p| crate::model::parse_number(p).unwrap_or(0));
    parents.dedup();
    for p in parents {
        if let Some(s) = render_bucket(
            &format!("# {p}"),
            &Bucket::Parent(p.to_string()),
            issues,
            &buckets,
            &primary,
            &children,
            &blockers,
            &number_of,
        ) {
            sections.push(s);
        }
    }

    // # Standalone last.
    if let Some(s) = render_bucket(
        "# Standalone",
        &Bucket::Standalone,
        issues,
        &buckets,
        &primary,
        &children,
        &blockers,
        &number_of,
    ) {
        sections.push(s);
    }

    sections.join("\n")
}

/// Build each issue's blocker set, restricted to the read universe:
/// declared `blockedBy` / `blocks` (symmetric), then file-overlap edges
/// (higher number under lower) for pairs with no declared edge.
fn compute_blockers(
    issues: &[Issue],
    universe: &HashSet<&str>,
) -> HashMap<String, BTreeSet<String>> {
    let mut blockers: HashMap<String, BTreeSet<String>> = issues
        .iter()
        .map(|i| (i.id.clone(), BTreeSet::new()))
        .collect();

    for i in issues {
        for b in &i.blocked_by {
            // Ignore a blocker outside the read set, and a self-edge (a data
            // error) that would otherwise drop the issue from the tree.
            if b != &i.id && universe.contains(b.as_str()) {
                blockers.get_mut(&i.id).unwrap().insert(b.clone());
            }
        }
        for b in &i.blocks {
            // `i blocks b` is the same edge as `b blockedBy i`.
            if b != &i.id && universe.contains(b.as_str()) {
                blockers.get_mut(b).unwrap().insert(i.id.clone());
            }
        }
    }

    for a in 0..issues.len() {
        for c in (a + 1)..issues.len() {
            let (ia, ic) = (&issues[a], &issues[c]);
            if !touches_overlap(ia, ic) {
                continue;
            }
            // A declared edge between the pair (either direction) wins over
            // the inferred "higher under lower" overlap edge — the human
            // asserted the order on purpose — so don't add a second edge.
            let declared = blockers[&ia.id].contains(&ic.id) || blockers[&ic.id].contains(&ia.id);
            if declared {
                continue;
            }
            let (lo, hi) = if ia.number <= ic.number {
                (ia, ic)
            } else {
                (ic, ia)
            };
            blockers.get_mut(&hi.id).unwrap().insert(lo.id.clone());
        }
    }

    blockers
}

/// Assign each issue a bucket: skill-only → `# Skills`; otherwise grouped
/// under its parent when that parent has 2+ non-skill Backlog subtasks, else
/// `# Standalone`.
fn compute_buckets(issues: &[Issue]) -> HashMap<String, Bucket> {
    let mut parent_count: HashMap<&str, usize> = HashMap::new();
    for i in issues {
        if i.is_skill_only() {
            continue;
        }
        if let Some(p) = &i.parent {
            *parent_count.entry(p.as_str()).or_default() += 1;
        }
    }

    issues
        .iter()
        .map(|i| {
            let bucket = if i.is_skill_only() {
                Bucket::Skills
            } else if let Some(p) = &i.parent {
                if parent_count.get(p.as_str()).copied().unwrap_or(0) >= 2 {
                    Bucket::Parent(p.clone())
                } else {
                    Bucket::Standalone
                }
            } else {
                Bucket::Standalone
            };
            (i.id.clone(), bucket)
        })
        .collect()
}

/// Longest in-bucket blocker chain below `id` (0 if no in-bucket blocker),
/// with a `visiting` cycle guard so a mutual declared edge can't loop. Not
/// memoized: the backlog is small (≤ a few hundred issues), and a cross-call
/// memo would cache cycle-truncated values that depend on the entry point.
fn chain_len(
    id: &str,
    blockers: &HashMap<String, BTreeSet<String>>,
    same_bucket: &impl Fn(&str, &str) -> bool,
    visiting: &mut HashSet<String>,
) -> usize {
    if !visiting.insert(id.to_string()) {
        return 0; // cycle — stop descending
    }
    let mut best = 0;
    if let Some(bs) = blockers.get(id) {
        for b in bs {
            if same_bucket(id, b) {
                best = best.max(1 + chain_len(b, blockers, same_bucket, visiting));
            }
        }
    }
    visiting.remove(id);
    best
}

/// Render one heading and its tree, or `None` if the bucket is empty.
#[allow(clippy::too_many_arguments)]
fn render_bucket(
    heading: &str,
    bucket: &Bucket,
    issues: &[Issue],
    buckets: &HashMap<String, Bucket>,
    primary: &HashMap<&str, Option<String>>,
    children: &HashMap<&str, Vec<&str>>,
    blockers: &HashMap<String, BTreeSet<String>>,
    number_of: &HashMap<&str, u64>,
) -> Option<String> {
    // Roots of this bucket: members with no (in-bucket) primary blocker.
    let mut roots: Vec<&str> = issues
        .iter()
        .map(|i| i.id.as_str())
        .filter(|id| buckets.get(*id) == Some(bucket))
        .filter(|id| primary.get(id).map(|p| p.is_none()).unwrap_or(true))
        .collect();
    if roots.is_empty() {
        return None;
    }
    roots.sort_by_key(|id| number_of.get(id).copied().unwrap_or(0));

    let mut out = format!("{heading}\n\n");
    let mut seen: HashSet<&str> = HashSet::new();
    // Proper ancestors of the node currently being rendered, threaded down the
    // descent so a blocker the nesting already expresses isn't repeated as a
    // note. A fresh, empty set per bucket root (a root has no ancestors).
    let mut ancestors: HashSet<&str> = HashSet::new();
    for root in roots {
        render_node(
            root,
            0,
            primary,
            children,
            blockers,
            number_of,
            &mut seen,
            &mut ancestors,
            &mut out,
        );
    }
    Some(out)
}

/// Render `id` as a bullet and recurse into its children. `seen` guards
/// against re-rendering a node reached twice (defensive against cycles).
/// `ancestors` holds the proper ancestors of `id` on the current descent path
/// (every node between `id` and its bucket root, exclusive of `id`); a blocker
/// in that set is already shown by the indentation, so `notes` drops it.
#[allow(clippy::too_many_arguments)]
fn render_node<'a>(
    id: &'a str,
    depth: usize,
    primary: &HashMap<&str, Option<String>>,
    children: &'a HashMap<&str, Vec<&'a str>>,
    blockers: &HashMap<String, BTreeSet<String>>,
    number_of: &HashMap<&str, u64>,
    seen: &mut HashSet<&'a str>,
    ancestors: &mut HashSet<&'a str>,
    out: &mut String,
) {
    if !seen.insert(id) {
        return;
    }
    let indent = INDENT.repeat(depth);
    out.push_str(&format!(
        "{indent}- {id}{}\n",
        notes(id, primary, blockers, number_of, ancestors)
    ));

    if let Some(kids) = children.get(id) {
        let mut kids = kids.clone();
        kids.sort_by_key(|k| number_of.get(k).copied().unwrap_or(0));
        // `id` is a proper ancestor of every node below it; insert before
        // recursing and remove on backtrack, mirroring the `seen` guard.
        ancestors.insert(id);
        for kid in kids {
            render_node(
                kid,
                depth + 1,
                primary,
                children,
                blockers,
                number_of,
                seen,
                ancestors,
                out,
            );
        }
        ancestors.remove(id);
    }
}

/// The trailing `(after …)` / `(also after …)` note for a node: every blocker
/// except the primary **and** except any blocker already an ancestor in the
/// tree (the indentation shows those), sorted by number. A top-level node's
/// first remaining blocker reads `after`; everything else reads `also after`.
fn notes(
    id: &str,
    primary: &HashMap<&str, Option<String>>,
    blockers: &HashMap<String, BTreeSet<String>>,
    number_of: &HashMap<&str, u64>,
    ancestors: &HashSet<&str>,
) -> String {
    let prim = primary.get(id).and_then(|p| p.clone());
    let mut extra: Vec<&String> = blockers
        .get(id)
        .into_iter()
        .flatten()
        .filter(|b| Some((*b).clone()) != prim)
        // A blocker that is an ancestor on the descent path is already
        // expressed by the nesting; only a cross-branch / cross-heading
        // blocker the tree can't show earns a note.
        .filter(|b| !ancestors.contains(b.as_str()))
        .collect();
    if extra.is_empty() {
        return String::new();
    }
    extra.sort_by_key(|b| number_of.get(b.as_str()).copied().unwrap_or(0));

    let nested = prim.is_some();
    let parts: Vec<String> = extra
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let word = if !nested && i == 0 {
                "after"
            } else {
                "also after"
            };
            format!("{word} {b}")
        })
        .collect();
    format!(" ({})", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(id: &str) -> Issue {
        Issue {
            id: id.to_string(),
            number: crate::model::parse_number(id).unwrap(),
            parent: None,
            touches: Vec::new(),
            blocked_by: Vec::new(),
            blocks: Vec::new(),
        }
    }

    fn with(id: &str, parent: Option<&str>, touches: &[&str], blocked_by: &[&str]) -> Issue {
        Issue {
            id: id.to_string(),
            number: crate::model::parse_number(id).unwrap(),
            parent: parent.map(|s| s.to_string()),
            touches: touches.iter().map(|s| s.to_string()).collect(),
            blocked_by: blocked_by.iter().map(|s| s.to_string()).collect(),
            blocks: Vec::new(),
        }
    }

    #[test]
    fn empty_renders_empty() {
        assert_eq!(render(&[]), "");
    }

    #[test]
    fn single_standalone_issue() {
        let out = render(&[with("ENG-10", None, &["a/b.rs"], &[])]);
        assert_eq!(out, "# Standalone\n\n- ENG-10\n");
    }

    #[test]
    fn declared_edge_nests_child_under_blocker() {
        // ENG-20 blockedBy ENG-10 → 20 nests under 10.
        let out = render(&[
            with("ENG-10", None, &["a/x.rs"], &[]),
            with("ENG-20", None, &["b/y.rs"], &["ENG-10"]),
        ]);
        assert_eq!(out, "# Standalone\n\n- ENG-10\n    - ENG-20\n");
    }

    #[test]
    fn file_overlap_serializes_higher_under_lower() {
        // No declared edge, but both declare the tui/ directory → they can't
        // run in parallel, so 22 nests under 18.
        let out = render(&[
            with("ENG-18", None, &["tui/"], &[]),
            with("ENG-22", None, &["tui/"], &[]),
        ]);
        assert_eq!(out, "# Standalone\n\n- ENG-18\n    - ENG-22\n");
    }

    #[test]
    fn distinct_files_in_same_dir_run_in_parallel() {
        // Different files under tui/ don't conflict, so both stay top-level.
        let out = render(&[
            with("ENG-18", None, &["tui/pane.rs"], &[]),
            with("ENG-22", None, &["tui/action.rs"], &[]),
        ]);
        assert_eq!(out, "# Standalone\n\n- ENG-18\n- ENG-22\n");
    }

    #[test]
    fn skills_bucket_comes_first() {
        let out = render(&[
            with("ENG-30", None, &["programs/dropset/src/lib.rs"], &[]),
            with("ENG-5", None, &[".claude/skills/foo/SKILL.md"], &[]),
        ]);
        assert_eq!(out, "# Skills\n\n- ENG-5\n\n# Standalone\n\n- ENG-30\n");
    }

    #[test]
    fn parent_with_two_subtasks_gets_heading() {
        let out = render(&[
            with("ENG-41", Some("ENG-40"), &["a/x.rs"], &[]),
            with("ENG-42", Some("ENG-40"), &["b/y.rs"], &[]),
        ]);
        assert_eq!(out, "# ENG-40\n\n- ENG-41\n- ENG-42\n");
    }

    #[test]
    fn parent_with_one_subtask_is_standalone() {
        let out = render(&[with("ENG-41", Some("ENG-40"), &["a/x.rs"], &[])]);
        assert_eq!(out, "# Standalone\n\n- ENG-41\n");
    }

    #[test]
    fn cross_heading_blocker_renders_after_note() {
        // ENG-60/61 share a parent heading; ENG-61 is blocked by standalone
        // ENG-70, which is under a different heading → (after ENG-70).
        let out = render(&[
            with("ENG-60", Some("ENG-50"), &["a/x.rs"], &[]),
            with("ENG-61", Some("ENG-50"), &["b/y.rs"], &["ENG-70"]),
            with("ENG-70", None, &["c/z.rs"], &[]),
        ]);
        assert_eq!(
            out,
            "# ENG-50\n\n- ENG-60\n- ENG-61 (after ENG-70)\n\n# Standalone\n\n- ENG-70\n"
        );
    }

    #[test]
    fn ancestor_blocker_note_is_suppressed() {
        // ENG-3 is blocked by both ENG-1 and ENG-2 (all standalone, all in
        // one bucket). It nests under the deeper-chain blocker ENG-2; its
        // other blocker ENG-1 is ENG-2's own blocker, so it is an ancestor of
        // ENG-3 in the tree and the nesting already shows it — no note.
        let out = render(&[
            with("ENG-1", None, &["a/x.rs"], &[]),
            with("ENG-2", None, &["b/y.rs"], &["ENG-1"]),
            with("ENG-3", None, &["c/z.rs"], &["ENG-1", "ENG-2"]),
        ]);
        // chain: 1 (0) < 2 (1); ENG-3 nests under ENG-2. ENG-1 is an ancestor
        // (1 → 2 → 3), so the redundant "(also after ENG-1)" is dropped.
        assert_eq!(
            out,
            "# Standalone\n\n- ENG-1\n    - ENG-2\n        - ENG-3\n"
        );
    }

    #[test]
    fn sibling_blocker_still_renders_also_after() {
        // ENG-2 and ENG-3 both nest under ENG-1 (siblings). ENG-4 is blocked
        // by both: it nests under ENG-3 (higher number breaks the equal-chain
        // tie), but ENG-2 is a sibling — not an ancestor of ENG-4 — so the
        // nesting can't express it and the "(also after ENG-2)" note stays.
        let out = render(&[
            with("ENG-1", None, &["a/x.rs"], &[]),
            with("ENG-2", None, &["b/y.rs"], &["ENG-1"]),
            with("ENG-3", None, &["c/z.rs"], &["ENG-1"]),
            with("ENG-4", None, &["d/w.rs"], &["ENG-2", "ENG-3"]),
        ]);
        assert_eq!(
            out,
            "# Standalone\n\n- ENG-1\n    - ENG-2\n    - ENG-3\n        - ENG-4 (also after ENG-2)\n"
        );
    }

    #[test]
    fn deterministic_regardless_of_input_order() {
        let a = render(&[
            with("ENG-2", None, &["b/y.rs"], &["ENG-1"]),
            with("ENG-1", None, &["a/x.rs"], &[]),
        ]);
        let b = render(&[
            with("ENG-1", None, &["a/x.rs"], &[]),
            with("ENG-2", None, &["b/y.rs"], &["ENG-1"]),
        ]);
        assert_eq!(a, b);
    }

    #[test]
    fn self_blocking_issue_still_renders() {
        // A data-error self-edge must not drop the issue from the output.
        let out = render(&[with("ENG-7", None, &["a/b.rs"], &["ENG-7"])]);
        assert_eq!(out, "# Standalone\n\n- ENG-7\n");
    }

    #[test]
    fn missing_touches_reported() {
        let issues = vec![issue("ENG-9"), with("ENG-10", None, &["a/b.rs"], &[])];
        assert_eq!(missing_touches(&issues), vec!["ENG-9".to_string()]);
    }
}
