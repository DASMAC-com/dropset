//! The deterministic core of the same-PR **issue merge**: given the members of
//! a group the skill's agent judged to belong in **one PR**, compute the
//! canonical fold — which issue survives, its merged description, the union of
//! `**Fingerprint**:` / `**Touches**:` fields and `blockedBy` / `blocks`
//! edges, and which members close as duplicates.
//!
//! This is the mechanical half the binary can own (the prose *grouping* —
//! deciding which issues are one PR — stays the agent's call, fed in as the
//! group). Everything here is pure and offline so it is fully unit-testable;
//! `linear.rs` performs the resulting writes (write-before-close), and
//! `main.rs`'s `merge` subcommand drives it.

use anyhow::{bail, Result};

use crate::model::{self, parse_number};

/// One member of a merge group, with the fields the fold needs.
#[derive(Debug, Clone)]
pub struct MergeIssue {
    /// The Linear identifier, e.g. `ENG-585`.
    pub id: String,
    /// Numeric part of the identifier — the "lowest ENG wins" canonical key.
    pub number: u64,
    /// The Linear UUID, needed for the mutations (the identifier addresses
    /// reads, but `issueUpdate` / `issueRelationCreate` take the UUID).
    pub uuid: String,
    /// The full issue description (its prose plus structured fields).
    pub description: String,
    /// Linear priority: 0 none, 1 urgent, 2 high, 3 medium, 4 low.
    pub priority: i64,
    /// Declared `blockedBy` edges (identifiers).
    pub blocked_by: Vec<String>,
    /// Declared `blocks` edges (identifiers).
    pub blocks: Vec<String>,
}

/// The computed fold: everything the executor needs to write the canonical and
/// close the members, with no further decisions to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergePlan {
    /// The surviving issue's identifier (lowest ENG number in the group).
    pub canonical_id: String,
    /// The surviving issue's UUID (the `issueUpdate` target).
    pub canonical_uuid: String,
    /// The folded description to write onto the canonical.
    pub description: String,
    /// The max-urgency priority across the group.
    pub priority: i64,
    /// External `blockedBy` endpoints (identifiers) to add to the canonical:
    /// the union across members, minus intra-group endpoints, minus the edges
    /// the canonical already declares. Sorted by number.
    pub blocked_by_to_add: Vec<String>,
    /// External `blocks` endpoints to add to the canonical, same treatment.
    pub blocks_to_add: Vec<String>,
    /// Members to close as duplicates of the canonical: `(identifier, uuid)`,
    /// ordered by number.
    pub duplicates: Vec<(String, String)>,
}

/// Urgency rank for "max priority wins": urgent (1) is the strongest, low (4)
/// the weakest, none (0) weaker still. Higher rank = more urgent.
fn urgency_rank(priority: i64) -> i64 {
    if priority == 0 {
        0
    } else {
        5 - priority
    }
}

/// Push each item not already present, preserving first-seen order.
fn extend_unique(out: &mut Vec<String>, items: impl IntoIterator<Item = String>) {
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
}

/// Compute the [`MergePlan`] for a group of two or more members. Pure and
/// deterministic: the same group (in any order) yields the same plan.
pub fn plan_merge(members: &[MergeIssue]) -> Result<MergePlan> {
    if members.len() < 2 {
        bail!(
            "a merge group needs at least two issues, got {}",
            members.len()
        );
    }
    let mut sorted: Vec<&MergeIssue> = members.iter().collect();
    sorted.sort_by_key(|m| m.number);
    // A duplicate identifier in the group is a caller error: it would close an
    // issue as a duplicate of itself.
    for pair in sorted.windows(2) {
        if pair[0].id == pair[1].id {
            bail!("merge group lists {} more than once", pair[0].id);
        }
    }

    let canonical = sorted[0];
    let group: std::collections::HashSet<&str> = sorted.iter().map(|m| m.id.as_str()).collect();

    // Description fold: canonical body first, then each member under a
    // `### From ENG-###` sub-heading, with the per-member field lines stripped
    // so the unified fingerprint / touches block can be re-emitted once.
    let mut blocks_md: Vec<String> = vec![model::strip_field_lines(&canonical.description)
        .trim()
        .to_string()];
    for m in sorted.iter().skip(1) {
        let body = model::strip_field_lines(&m.description);
        let body = body.trim();
        blocks_md.push(format!("### From {}\n\n{body}", m.id));
    }
    // Drop an empty canonical body so a fields-only canonical doesn't leave the
    // folded description leading with blank lines (each `### From` block always
    // carries its heading, so only the canonical body can be empty).
    blocks_md.retain(|b| !b.is_empty());

    // Unified fields: union across canonical-then-members (the sorted order),
    // deduped, first-seen order preserved.
    let mut fingerprints: Vec<String> = Vec::new();
    let mut touches: Vec<String> = Vec::new();
    for m in &sorted {
        extend_unique(&mut fingerprints, model::parse_fingerprints(&m.description));
        extend_unique(&mut touches, model::parse_touches(&m.description));
    }
    let mut field_lines: Vec<String> = fingerprints
        .iter()
        .map(|f| format!("**Fingerprint**: {f}"))
        .collect();
    if !touches.is_empty() {
        field_lines.push(format!("**Touches**: {}", touches.join(", ")));
    }

    let mut description = blocks_md.join("\n\n");
    if !field_lines.is_empty() {
        description.push_str("\n\n");
        description.push_str(&field_lines.join("\n"));
    }

    let priority = sorted
        .iter()
        .map(|m| m.priority)
        .max_by_key(|&p| urgency_rank(p))
        .unwrap_or(0);

    // Edge union, minus intra-group endpoints, minus what the canonical already
    // declares — those are the relations the executor must create.
    let edges_to_add = |pick: fn(&MergeIssue) -> &Vec<String>| -> Vec<String> {
        let mut union: Vec<String> = Vec::new();
        for m in &sorted {
            extend_unique(
                &mut union,
                pick(m)
                    .iter()
                    .filter(|e| !group.contains(e.as_str()))
                    .cloned(),
            );
        }
        let existing: std::collections::HashSet<&str> =
            pick(canonical).iter().map(|s| s.as_str()).collect();
        let mut add: Vec<String> = union
            .into_iter()
            .filter(|e| !existing.contains(e.as_str()))
            .collect();
        add.sort_by_key(|e| parse_number(e).unwrap_or(u64::MAX));
        add
    };

    let blocked_by_to_add = edges_to_add(|m| &m.blocked_by);
    let blocks_to_add = edges_to_add(|m| &m.blocks);

    let duplicates: Vec<(String, String)> = sorted
        .iter()
        .skip(1)
        .map(|m| (m.id.clone(), m.uuid.clone()))
        .collect();

    Ok(MergePlan {
        canonical_id: canonical.id.clone(),
        canonical_uuid: canonical.uuid.clone(),
        description,
        priority,
        blocked_by_to_add,
        blocks_to_add,
        duplicates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(number: u64, priority: i64, description: &str) -> MergeIssue {
        MergeIssue {
            id: format!("ENG-{number}"),
            number,
            uuid: format!("uuid-{number}"),
            description: description.to_string(),
            priority,
            blocked_by: Vec::new(),
            blocks: Vec::new(),
        }
    }

    #[test]
    fn rejects_under_two_members() {
        assert!(plan_merge(&[member(1, 0, "x")]).is_err());
        assert!(plan_merge(&[]).is_err());
    }

    #[test]
    fn rejects_duplicate_identifier() {
        let m = member(5, 0, "x");
        assert!(plan_merge(&[m.clone(), m]).is_err());
    }

    #[test]
    fn lowest_number_is_canonical_regardless_of_order() {
        let a = plan_merge(&[member(20, 0, "a"), member(10, 0, "b")]).unwrap();
        let b = plan_merge(&[member(10, 0, "b"), member(20, 0, "a")]).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.canonical_id, "ENG-10");
        assert_eq!(
            a.duplicates,
            vec![("ENG-20".to_string(), "uuid-20".to_string())]
        );
    }

    #[test]
    fn folds_description_under_per_source_subheadings() {
        let plan = plan_merge(&[
            member(
                10,
                0,
                "Canonical body.\n**Touches**: a/\n**Fingerprint**: a.rs:x",
            ),
            member(
                20,
                0,
                "Member body.\n**Touches**: b/\n**Fingerprint**: b.rs:y",
            ),
        ])
        .unwrap();
        assert_eq!(
            plan.description,
            "Canonical body.\n\n\
             ### From ENG-20\n\n\
             Member body.\n\n\
             **Fingerprint**: a.rs:x\n\
             **Fingerprint**: b.rs:y\n\
             **Touches**: a/, b/"
        );
    }

    #[test]
    fn unions_and_dedups_fields() {
        // A fingerprint and a touch glob shared across members appears once.
        let plan = plan_merge(&[
            member(10, 0, "**Touches**: shared/, a/\n**Fingerprint**: dup:fp"),
            member(20, 0, "**Touches**: shared/, b/\n**Fingerprint**: dup:fp"),
        ])
        .unwrap();
        assert!(plan.description.contains("**Fingerprint**: dup:fp"));
        assert_eq!(plan.description.matches("dup:fp").count(), 1);
        assert!(plan.description.contains("**Touches**: shared/, a/, b/"));
    }

    #[test]
    fn max_urgency_priority_wins() {
        // urgent(1) beats medium(3) beats none(0).
        assert_eq!(
            plan_merge(&[member(1, 3, "x"), member(2, 1, "y")])
                .unwrap()
                .priority,
            1
        );
        assert_eq!(
            plan_merge(&[member(1, 0, "x"), member(2, 4, "y")])
                .unwrap()
                .priority,
            4
        );
        assert_eq!(
            plan_merge(&[member(1, 0, "x"), member(2, 0, "y")])
                .unwrap()
                .priority,
            0
        );
    }

    #[test]
    fn edge_union_drops_intra_group_and_existing() {
        let mut canonical = member(10, 0, "c");
        canonical.blocked_by = vec!["ENG-5".to_string()]; // external, already on canonical
        canonical.blocks = vec!["ENG-20".to_string()]; // intra-group → dropped
        let mut other = member(20, 0, "o");
        other.blocked_by = vec!["ENG-10".to_string(), "ENG-7".to_string()]; // 10 intra-group
        other.blocks = vec!["ENG-30".to_string()];
        let plan = plan_merge(&[canonical, other]).unwrap();
        // ENG-5 already on canonical → not re-added; ENG-7 is the new external.
        assert_eq!(plan.blocked_by_to_add, vec!["ENG-7".to_string()]);
        // ENG-20 was intra-group; ENG-30 is the new external block edge.
        assert_eq!(plan.blocks_to_add, vec!["ENG-30".to_string()]);
    }

    #[test]
    fn fields_only_canonical_does_not_lead_with_blank_lines() {
        // The canonical body is empty after stripping its only line; the fold
        // must lead with the first member's heading, not blank lines.
        let plan = plan_merge(&[
            member(10, 0, "**Touches**: a/"),
            member(20, 0, "Member prose."),
        ])
        .unwrap();
        assert_eq!(
            plan.description,
            "### From ENG-20\n\nMember prose.\n\n**Touches**: a/"
        );
    }

    #[test]
    fn omits_field_block_when_no_fields() {
        let plan =
            plan_merge(&[member(10, 0, "Just prose."), member(20, 0, "More prose.")]).unwrap();
        assert_eq!(
            plan.description,
            "Just prose.\n\n### From ENG-20\n\nMore prose."
        );
    }
}
