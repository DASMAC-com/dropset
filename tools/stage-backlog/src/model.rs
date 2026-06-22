//! The issue model and the pure path-glob helpers the planner builds on.
//!
//! An [`Issue`] is the distilled shape the planner needs: its identifier and
//! number, its Linear parent, the file globs it touches, and its declared
//! blocking edges. Everything here is deterministic and offline — the Linear
//! client (see `linear.rs`) maps the GraphQL payload into these structs, and
//! the planner (see `plan.rs`) renders a tree from them.

/// One open Backlog issue, reduced to what the planner needs.
#[derive(Debug, Clone)]
pub struct Issue {
    /// The Linear identifier, e.g. `ENG-578`.
    pub id: String,
    /// The numeric part of the identifier, e.g. `578` — the canonical sort
    /// key and the tie-break for "lowest ENG wins".
    pub number: u64,
    /// The parent issue's identifier, if any (`parentId` resolved to its
    /// identifier).
    pub parent: Option<String>,
    /// The file globs this issue's work touches, from its `**Touches**:`
    /// field. Empty when the issue predates the convention (the
    /// missing-data fallback the skill's agent step handles).
    pub touches: Vec<String>,
    /// Declared `blockedBy` edges (identifiers) — issues that must land first.
    pub blocked_by: Vec<String>,
    /// Declared `blocks` edges (identifiers) — issues this one gates.
    pub blocks: Vec<String>,
}

impl Issue {
    /// True when the issue touches **only** the skill suite — files under
    /// `.claude/skills/**` or `CLAUDE.md`, with no product code — so it folds
    /// into the consolidated `# Skills` PR. An issue with no `touches:` is
    /// never skill-only (we can't prove it).
    pub fn is_skill_only(&self) -> bool {
        !self.touches.is_empty() && self.touches.iter().all(|g| is_skill_glob(g))
    }
}

/// Parse the trailing number out of an `ENG-###` identifier.
pub fn parse_number(id: &str) -> Option<u64> {
    id.rsplit('-').next()?.parse().ok()
}

/// Pull every glob off an issue description's `**Touches**:` line(s). A line
/// is `**Touches**: glob1, glob2, …`; globs are comma-separated, trimmed, and
/// stripped of surrounding backticks. Multiple `**Touches**:` lines union.
pub fn parse_touches(description: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in description.lines() {
        let line = line.trim();
        // Tolerate a single leading list marker, e.g. "- **Touches**: …"
        // (without eating the bold `**` that follows).
        let line = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .unwrap_or(line);
        if let Some(rest) = line.strip_prefix("**Touches**:") {
            for glob in rest.split(',') {
                let glob = glob.trim().trim_matches('`').trim();
                if !glob.is_empty() {
                    out.push(glob.to_string());
                }
            }
        }
    }
    out
}

/// A glob counts as skill-suite when it names `CLAUDE.md` or sits under
/// `.claude/skills/`.
fn is_skill_glob(glob: &str) -> bool {
    let glob = glob.trim_start_matches("./");
    glob == "CLAUDE.md" || glob.starts_with(".claude/skills")
}

/// Reduce a glob to a comparable path prefix: drop a trailing `/**` or `/*`
/// and any trailing slash, so `sdk/rs/**` and `sdk/rs/` both become `sdk/rs`.
pub fn normalize_glob(glob: &str) -> String {
    let glob = glob.trim().trim_start_matches("./");
    let glob = glob.strip_suffix("/**").unwrap_or(glob);
    let glob = glob.strip_suffix("/*").unwrap_or(glob);
    glob.trim_end_matches('/').to_string()
}

/// True when `a` is `b` or a path-segment ancestor of `b` (`sdk` is a prefix
/// of `sdk/rs`, but `sd` is not).
fn is_path_prefix(a: &str, b: &str) -> bool {
    b == a || b.starts_with(&format!("{a}/"))
}

/// Two issues' file sets overlap when any normalized touch-glob of one is the
/// same path as, or an ancestor/descendant of, a touch-glob of the other.
/// This is the deterministic set-intersection that the prose-reading skeptic
/// used to do by hand (the `tui/` collision case).
pub fn touches_overlap(a: &Issue, b: &Issue) -> bool {
    for ga in &a.touches {
        let na = normalize_glob(ga);
        if na.is_empty() {
            continue;
        }
        for gb in &b.touches {
            let nb = normalize_glob(gb);
            if nb.is_empty() {
                continue;
            }
            if is_path_prefix(&na, &nb) || is_path_prefix(&nb, &na) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(id: &str, touches: &[&str]) -> Issue {
        Issue {
            id: id.to_string(),
            number: parse_number(id).unwrap(),
            parent: None,
            touches: touches.iter().map(|s| s.to_string()).collect(),
            blocked_by: Vec::new(),
            blocks: Vec::new(),
        }
    }

    #[test]
    fn parses_number() {
        assert_eq!(parse_number("ENG-578"), Some(578));
        assert_eq!(parse_number("ENG-1"), Some(1));
        assert_eq!(parse_number("nope"), None);
    }

    #[test]
    fn parses_touches_field() {
        let desc = "**What**: a thing\n**Touches**: `tui/`, sdk/rs/**, CLAUDE.md\n";
        assert_eq!(parse_touches(desc), vec!["tui/", "sdk/rs/**", "CLAUDE.md"]);
    }

    #[test]
    fn parses_touches_list_marker_and_multiple_lines() {
        let desc = "- **Touches**: a/\n- **Touches**: b/\n";
        assert_eq!(parse_touches(desc), vec!["a/", "b/"]);
    }

    #[test]
    fn no_touches_is_empty() {
        assert!(parse_touches("**What**: nothing structured").is_empty());
    }

    #[test]
    fn skill_only_detection() {
        assert!(issue("ENG-1", &[".claude/skills/foo/SKILL.md"]).is_skill_only());
        assert!(issue("ENG-2", &["CLAUDE.md", ".claude/skills/bar/SKILL.md"]).is_skill_only());
        // mixed with product code is not pure skill work
        assert!(!issue("ENG-3", &["CLAUDE.md", "programs/dropset/src/lib.rs"]).is_skill_only());
        // no touches can't be proven skill-only
        assert!(!issue("ENG-4", &[]).is_skill_only());
    }

    #[test]
    fn overlap_same_dir_and_file() {
        assert!(touches_overlap(
            &issue("ENG-1", &["tui/"]),
            &issue("ENG-2", &["tui/pane.rs"])
        ));
        assert!(touches_overlap(
            &issue("ENG-1", &["sdk/rs/**"]),
            &issue("ENG-2", &["sdk/rs/lib.rs"])
        ));
        assert!(touches_overlap(
            &issue("ENG-1", &["CLAUDE.md"]),
            &issue("ENG-2", &["CLAUDE.md"])
        ));
    }

    #[test]
    fn no_overlap_distinct_files() {
        assert!(!touches_overlap(
            &issue("ENG-1", &["programs/dropset/src/swap.rs"]),
            &issue("ENG-2", &["programs/dropset/src/lib.rs"])
        ));
        // a shared string prefix that is not a path boundary must not match
        assert!(!touches_overlap(
            &issue("ENG-1", &["sdk/rs"]),
            &issue("ENG-2", &["sdk/rust"])
        ));
    }
}
