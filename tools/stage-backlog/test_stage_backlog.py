"""Stdlib ``unittest`` tests for the stage-backlog renderer.

Ported from the Rust crate's ``#[cfg(test)]`` cases in ``model.rs`` / ``plan.rs``
(the merge-only helpers and their tests are gone with the merge subcommand),
plus the orphan-cycle regression for the silent-drop bug the port fixes, and the
edge-driven-blocking cases (materialized overlap, cross-state blockers, bare-tag
notes, transitive tally). Run with ``python3 -m unittest`` from
``tools/stage-backlog``.
"""

import unittest

from stage_backlog import (
    Issue,
    block_counts,
    compute_blockers,
    materialize_overlap_edges,
    missing_touches,
    parse_number,
    parse_touches,
    prefix_touches_drift,
    render,
    render_tally,
    touches_overlap,
)


def issue(ident, touches=(), title=""):
    return Issue(
        id=ident, number=parse_number(ident), touches=list(touches), title=title
    )


def with_(ident, parent=None, touches=(), blocked_by=(), blocks=(), title=""):
    return Issue(
        id=ident,
        number=parse_number(ident),
        title=title,
        parent=parent,
        touches=list(touches),
        blocked_by=list(blocked_by),
        blocks=list(blocks),
    )


def all_backlog(issues):
    """A ``state_of`` map treating every issue as live Backlog."""
    return {i.id: "backlog" for i in issues}


class ModelTests(unittest.TestCase):
    def test_parses_number(self):
        self.assertEqual(parse_number("ENG-578"), 578)
        self.assertEqual(parse_number("ENG-1"), 1)
        self.assertIsNone(parse_number("nope"))

    def test_parses_touches_field(self):
        desc = "**What**: a thing\n**Touches**: `tui/`, sdk/rs/**, CLAUDE.md\n"
        self.assertEqual(parse_touches(desc), ["tui/", "sdk/rs/**", "CLAUDE.md"])

    def test_parses_touches_list_marker_and_multiple_lines(self):
        desc = "- **Touches**: a/\n- **Touches**: b/\n"
        self.assertEqual(parse_touches(desc), ["a/", "b/"])

    def test_no_touches_is_empty(self):
        self.assertEqual(parse_touches("**What**: nothing structured"), [])

    def test_meta_only_detection(self):
        # The meta surface is .claude/**, CLAUDE.md, docs/conventions/**, tools/**.
        self.assertTrue(issue("ENG-1", [".claude/skills/foo/SKILL.md"]).is_meta_only())
        self.assertTrue(
            issue("ENG-2", ["CLAUDE.md", "docs/conventions/x.md"]).is_meta_only()
        )
        self.assertTrue(issue("ENG-3", ["tools/stage-backlog/**"]).is_meta_only())
        # mixed with product code is not pure meta work
        self.assertFalse(
            issue("ENG-4", ["CLAUDE.md", "programs/dropset/src/lib.rs"]).is_meta_only()
        )
        # docs/ outside conventions/ is product docs, not meta
        self.assertFalse(issue("ENG-5", ["docs/indexer.md"]).is_meta_only())
        # no touches can't be proven meta-only
        self.assertFalse(issue("ENG-6", []).is_meta_only())

    def test_claude_prefix_detection(self):
        self.assertTrue(with_("ENG-1", title="Claude: Do a thing").has_claude_prefix())
        # needs the capital C, colon, and space
        self.assertFalse(with_("ENG-2", title="claude: lower").has_claude_prefix())
        self.assertFalse(with_("ENG-3", title="Claude things").has_claude_prefix())
        self.assertFalse(with_("ENG-4", title="Add a skill").has_claude_prefix())

    def test_overlap_same_dir_and_file(self):
        self.assertTrue(
            touches_overlap(issue("ENG-1", ["tui/"]), issue("ENG-2", ["tui/pane.rs"]))
        )
        self.assertTrue(
            touches_overlap(
                issue("ENG-1", ["sdk/rs/**"]), issue("ENG-2", ["sdk/rs/lib.rs"])
            )
        )
        self.assertTrue(
            touches_overlap(
                issue("ENG-1", ["CLAUDE.md"]), issue("ENG-2", ["CLAUDE.md"])
            )
        )

    def test_no_overlap_distinct_files(self):
        self.assertFalse(
            touches_overlap(
                issue("ENG-1", ["programs/dropset/src/swap.rs"]),
                issue("ENG-2", ["programs/dropset/src/lib.rs"]),
            )
        )
        # a shared string prefix that is not a path boundary must not match
        self.assertFalse(
            touches_overlap(issue("ENG-1", ["sdk/rs"]), issue("ENG-2", ["sdk/rust"]))
        )


class MaterializeOverlapTests(unittest.TestCase):
    """File-overlap is turned into a real ``blocks`` relation (lower blocks
    higher) before the tree is built — not inferred in the renderer. Under
    ``--dry-run`` nothing is written, but the edge is still reflected in memory
    so the previewed tree matches a real run."""

    def test_overlap_files_lower_blocks_higher(self):
        # Input order higher-first to prove the lower number is chosen.
        issues = [
            with_("ENG-22", touches=["tui/"]),
            with_("ENG-18", touches=["tui/"]),
        ]
        filed = materialize_overlap_edges(issues, None, True)
        self.assertEqual(filed, [("ENG-18", "ENG-22")])
        eng18 = next(i for i in issues if i.id == "ENG-18")
        self.assertIn("ENG-22", eng18.blocks)  # reflected in memory

    def test_declared_edge_suppresses_overlap_edge(self):
        # A declared edge in either direction wins; no overlap edge is filed.
        issues = [
            with_("ENG-18", touches=["tui/"]),
            with_("ENG-22", touches=["tui/"], blocked_by=["ENG-18"]),
        ]
        self.assertEqual(materialize_overlap_edges(issues, None, True), [])

    def test_distinct_files_file_no_edge(self):
        issues = [
            with_("ENG-18", touches=["tui/pane.rs"]),
            with_("ENG-22", touches=["tui/action.rs"]),
        ]
        self.assertEqual(materialize_overlap_edges(issues, None, True), [])


class PlanTests(unittest.TestCase):
    def test_empty_renders_empty(self):
        self.assertEqual(render([]), "")

    def test_single_standalone_issue(self):
        out = render([with_("ENG-10", touches=["a/b.rs"])])
        self.assertEqual(out, "# Standalone\n\n- ENG-10\n")

    def test_declared_edge_nests_child_under_blocker(self):
        # ENG-20 blockedBy ENG-10 → 20 nests under 10.
        out = render(
            [
                with_("ENG-10", touches=["a/x.rs"]),
                with_("ENG-20", touches=["b/y.rs"], blocked_by=["ENG-10"]),
            ]
        )
        self.assertEqual(
            out,
            "# Most blocking\n\n- ENG-10 — blocks 1 issue\n\n"
            "# Standalone\n\n- ENG-10\n    - ENG-20\n",
        )

    def test_materialized_overlap_serializes_higher_under_lower(self):
        # No declared edge, but both declare the tui/ directory → materialize
        # files ENG-18 blocks ENG-22, so 22 nests under 18.
        issues = [with_("ENG-18", touches=["tui/"]), with_("ENG-22", touches=["tui/"])]
        materialize_overlap_edges(issues, None, True)
        self.assertEqual(
            render(issues),
            "# Most blocking\n\n- ENG-18 — blocks 1 issue\n\n"
            "# Standalone\n\n- ENG-18\n    - ENG-22\n",
        )

    def test_distinct_files_in_same_dir_run_in_parallel(self):
        # Different files under tui/ don't conflict, so both stay top-level.
        out = render(
            [
                with_("ENG-18", touches=["tui/pane.rs"]),
                with_("ENG-22", touches=["tui/action.rs"]),
            ]
        )
        self.assertEqual(out, "# Standalone\n\n- ENG-18\n- ENG-22\n")

    def test_claude_bucket_comes_first_and_keys_on_prefix(self):
        # Bucketing is by the Claude: title prefix, not by file globs: ENG-5
        # carries the prefix → # Claude; ENG-30 doesn't → # Standalone.
        out = render(
            [
                with_("ENG-30", touches=["programs/dropset/src/lib.rs"]),
                with_(
                    "ENG-5",
                    touches=[".claude/skills/foo/SKILL.md"],
                    title="Claude: Tweak a skill",
                ),
            ]
        )
        self.assertEqual(out, "# Claude\n\n- ENG-5\n\n# Standalone\n\n- ENG-30\n")

    def test_parent_with_two_subtasks_gets_heading(self):
        out = render(
            [
                with_("ENG-41", parent="ENG-40", touches=["a/x.rs"]),
                with_("ENG-42", parent="ENG-40", touches=["b/y.rs"]),
            ]
        )
        self.assertEqual(out, "# ENG-40\n\n- ENG-41\n- ENG-42\n")

    def test_parent_with_one_subtask_is_standalone(self):
        out = render([with_("ENG-41", parent="ENG-40", touches=["a/x.rs"])])
        self.assertEqual(out, "# Standalone\n\n- ENG-41\n")

    def test_cross_heading_blocker_renders_bare_tag_note(self):
        # ENG-60/61 share a parent heading; ENG-61 is blocked by standalone
        # ENG-70, under a different heading → bare-tag note (ENG-70).
        out = render(
            [
                with_("ENG-60", parent="ENG-50", touches=["a/x.rs"]),
                with_(
                    "ENG-61", parent="ENG-50", touches=["b/y.rs"], blocked_by=["ENG-70"]
                ),
                with_("ENG-70", touches=["c/z.rs"]),
            ]
        )
        self.assertEqual(
            out,
            "# Most blocking\n\n- ENG-70 — blocks 1 issue\n\n"
            "# ENG-50\n\n- ENG-60\n- ENG-61 (ENG-70)\n\n# Standalone\n\n- ENG-70\n",
        )

    def test_ancestor_blocker_note_is_suppressed(self):
        # ENG-3 is blocked by both ENG-1 and ENG-2 (all standalone, one bucket).
        # It nests under the deeper-chain blocker ENG-2; its other blocker ENG-1
        # is ENG-2's own blocker, so it is an ancestor of ENG-3 in the tree and
        # the nesting already shows it — no note.
        out = render(
            [
                with_("ENG-1", touches=["a/x.rs"]),
                with_("ENG-2", touches=["b/y.rs"], blocked_by=["ENG-1"]),
                with_("ENG-3", touches=["c/z.rs"], blocked_by=["ENG-1", "ENG-2"]),
            ]
        )
        self.assertEqual(
            out,
            "# Most blocking\n\n- ENG-1 — blocks 2 issues\n- ENG-2 — blocks 1 issue\n\n"
            "# Standalone\n\n- ENG-1\n    - ENG-2\n        - ENG-3\n",
        )

    def test_sibling_blocker_renders_in_bare_tag_note(self):
        # ENG-2 and ENG-3 both nest under ENG-1 (siblings). ENG-4 is blocked by
        # both: it nests under ENG-3 (higher number breaks the equal-chain tie),
        # but ENG-2 is a sibling — not an ancestor of ENG-4 — so the nesting
        # can't express it and the bare-tag "(ENG-2)" note stays. ENG-1 blocks
        # ENG-4 transitively (1 → 2/3 → 4), so its tally count is 3.
        out = render(
            [
                with_("ENG-1", touches=["a/x.rs"]),
                with_("ENG-2", touches=["b/y.rs"], blocked_by=["ENG-1"]),
                with_("ENG-3", touches=["c/z.rs"], blocked_by=["ENG-1"]),
                with_("ENG-4", touches=["d/w.rs"], blocked_by=["ENG-2", "ENG-3"]),
            ]
        )
        self.assertEqual(
            out,
            "# Most blocking\n\n- ENG-1 — blocks 3 issues\n- ENG-2 — blocks 1 issue\n"
            "- ENG-3 — blocks 1 issue\n\n"
            "# Standalone\n\n- ENG-1\n    - ENG-2\n    - ENG-3\n        - ENG-4 (ENG-2)\n",
        )

    def test_deterministic_regardless_of_input_order(self):
        a = render(
            [
                with_("ENG-2", touches=["b/y.rs"], blocked_by=["ENG-1"]),
                with_("ENG-1", touches=["a/x.rs"]),
            ]
        )
        b = render(
            [
                with_("ENG-1", touches=["a/x.rs"]),
                with_("ENG-2", touches=["b/y.rs"], blocked_by=["ENG-1"]),
            ]
        )
        self.assertEqual(a, b)

    def test_self_blocking_issue_still_renders(self):
        # A data-error self-edge must not drop the issue from the output.
        out = render([with_("ENG-7", touches=["a/b.rs"], blocked_by=["ENG-7"])])
        self.assertEqual(out, "# Standalone\n\n- ENG-7\n")

    def test_missing_touches_reported(self):
        issues = [issue("ENG-9"), with_("ENG-10", touches=["a/b.rs"])]
        self.assertEqual(missing_touches(issues), ["ENG-9"])


class CrossStateBlockerTests(unittest.TestCase):
    """A blocker is honoured by its **state**, not by whether it is itself in
    the Backlog: a live external (In-Progress / In-Review) blocker stays visible
    as a tag and ranks in the tally; a resolved (Done / Cancelled) blocker is
    dropped."""

    def test_live_external_blocker_kept_as_tag_and_ranked(self):
        issues = [with_("ENG-10", touches=["a/b.rs"], blocked_by=["ENG-5"])]
        state_of = {"ENG-10": "backlog", "ENG-5": "started"}
        out = render(issues, state_of)
        self.assertIn("- ENG-10 (ENG-5)", out)
        self.assertIn("- ENG-5 — blocks 1 issue", out)

    def test_completed_blocker_dropped(self):
        issues = [with_("ENG-10", touches=["a/b.rs"], blocked_by=["ENG-5"])]
        out = render(issues, {"ENG-10": "backlog", "ENG-5": "completed"})
        self.assertEqual(out, "# Standalone\n\n- ENG-10\n")

    def test_canceled_blocker_dropped(self):
        issues = [with_("ENG-10", touches=["a/b.rs"], blocked_by=["ENG-5"])]
        out = render(issues, {"ENG-10": "backlog", "ENG-5": "canceled"})
        self.assertEqual(out, "# Standalone\n\n- ENG-10\n")

    def test_unknown_state_blocker_assumed_live(self):
        # No state entry for the blocker → assumed live, never silently dropped.
        issues = [with_("ENG-10", touches=["a/b.rs"], blocked_by=["ENG-5"])]
        out = render(issues, {"ENG-10": "backlog"})
        self.assertIn("- ENG-10 (ENG-5)", out)


class OrphanCycleTests(unittest.TestCase):
    """Regression for the silent-drop bug: a directed cycle in the blocker
    graph means every member has a non-None primary, so none is a bucket root.
    The old planner only emitted nodes reachable from a root, so the whole
    cycle vanished with no warning. The orphan sweep must render every member
    and flag the unreached ids."""

    def test_backward_declared_edge_plus_overlap_renders_all_members(self):
        # The maker-bot cluster: all six share bots/** (materialized overlap
        # edges put the higher number under the lower), plus one *backward*
        # declared edge — ENG-602 blockedBy ENG-606, though 606 > 602 — which
        # closes the cycle 602 → 606 → 604 → 602. None of the six is a root.
        members = ["ENG-602", "ENG-604", "ENG-605", "ENG-606", "ENG-607", "ENG-608"]
        issues = [with_(m, touches=["bots/**"]) for m in members]
        issues[0].blocked_by = ["ENG-606"]  # ENG-602 blockedBy ENG-606 (backward)
        materialize_overlap_edges(issues, None, True)

        orphans = []
        out = render(issues, all_backlog(issues), orphans)

        # Every member renders exactly once in the tree — none silently
        # dropped. A tree bullet is `- {m}\n` (leaf) or `- {m} (…)` (with
        # notes); the `# Most blocking` tally line is `- {m} — blocks …`, which
        # matches neither, so it doesn't inflate the tree count.
        for m in members:
            self.assertIn(f"- {m}", out, f"{m} missing from the rendered tree")
            tree_occurrences = out.count(f"- {m}\n") + out.count(f"- {m} (")
            self.assertEqual(tree_occurrences, 1, f"{m} not rendered exactly once")

        # The unreached ids are flagged so it can never fail silently again.
        self.assertTrue(orphans, "orphan sweep recorded no cyclic ids")
        self.assertIn("ENG-602", orphans)

    def test_no_orphans_when_a_root_exists(self):
        # A clean acyclic bucket leaves the orphan list empty.
        orphans = []
        render(
            [
                with_("ENG-1", touches=["a/x.rs"]),
                with_("ENG-2", touches=["b/y.rs"], blocked_by=["ENG-1"]),
            ],
            orphans=orphans,
        )
        self.assertEqual(orphans, [])


class MostBlockingTallyTests(unittest.TestCase):
    """The `# Most blocking` tally ranks issues by how many others they block
    **transitively**, so the issue to start on first sits at the top."""

    def test_absent_when_nothing_blocks(self):
        # No edges → no tally section at all, just the buckets.
        out = render([with_("ENG-10", touches=["a/b.rs"])])
        self.assertNotIn("# Most blocking", out)

    def test_ranks_transitively_desc_with_lowest_number_tie_break(self):
        # ENG-1 blocks ENG-2 and ENG-3, each of which blocks ENG-4 — so ENG-1
        # transitively blocks 3 (2, 3, and 4); ENG-2 and ENG-3 block 1 apiece.
        out = render(
            [
                with_("ENG-1", touches=["a/x.rs"]),
                with_("ENG-2", touches=["b/y.rs"], blocked_by=["ENG-1"]),
                with_("ENG-3", touches=["c/z.rs"], blocked_by=["ENG-1"]),
                with_("ENG-4", touches=["d/w.rs"], blocked_by=["ENG-2", "ENG-3"]),
            ]
        )
        tally = out.split("\n\n#", 1)[0]
        self.assertEqual(
            tally,
            "# Most blocking\n\n- ENG-1 — blocks 3 issues\n"
            "- ENG-2 — blocks 1 issue\n- ENG-3 — blocks 1 issue",
        )

    def test_singular_plural_noun(self):
        # block_counts feeds the noun: 1 → "issue", >1 → "issues".
        issues = [
            with_("ENG-1", touches=["a/x.rs"]),
            with_("ENG-2", touches=["b/y.rs"], blocked_by=["ENG-1"]),
        ]
        blockers = compute_blockers(issues, all_backlog(issues))
        counts = block_counts(issues, blockers)
        self.assertEqual(counts, {"ENG-1": 1, "ENG-2": 0})
        tally = render_tally(counts, {i.id: i.number for i in issues})
        self.assertEqual(tally, "# Most blocking\n\n- ENG-1 — blocks 1 issue\n")

    def test_cycle_members_counted_transitively(self):
        # In the maker-bot cycle ENG-602 reaches every other member transitively
        # (604/605/607/608 directly, 606 and itself via the cycle) — 5 distinct
        # others, never counting itself.
        members = ["ENG-602", "ENG-604", "ENG-605", "ENG-606", "ENG-607", "ENG-608"]
        issues = [with_(m, touches=["bots/**"]) for m in members]
        issues[0].blocked_by = ["ENG-606"]
        materialize_overlap_edges(issues, None, True)
        out = render(issues, all_backlog(issues))
        self.assertIn("# Most blocking", out)
        self.assertIn("- ENG-602 — blocks 5 issues", out)

    def test_live_external_blocker_ranks_in_tally(self):
        # A live external (In-Progress) blocker gating a Backlog chain ranks by
        # its transitive reach: ENG-5 → ENG-10 → ENG-20 means ENG-5 blocks 2.
        issues = [
            with_("ENG-10", touches=["a/b.rs"], blocked_by=["ENG-5"]),
            with_("ENG-20", touches=["c/d.rs"], blocked_by=["ENG-10"]),
        ]
        state_of = {"ENG-10": "backlog", "ENG-20": "backlog", "ENG-5": "started"}
        out = render(issues, state_of)
        self.assertIn("- ENG-5 — blocks 2 issues", out)


class PrefixTouchesDriftTests(unittest.TestCase):
    """The consistency check that supersedes the glob-only bucketing: flag a
    `Claude:`-prefixed issue that touches non-meta paths, or a meta-only-touches
    issue with no prefix."""

    def test_clean_prefixed_meta_issue_not_flagged(self):
        issues = [with_("ENG-1", touches=[".claude/skills/x"], title="Claude: x")]
        self.assertEqual(prefix_touches_drift(issues), [])

    def test_clean_unprefixed_product_issue_not_flagged(self):
        issues = [with_("ENG-1", touches=["programs/dropset/src/lib.rs"])]
        self.assertEqual(prefix_touches_drift(issues), [])

    def test_prefixed_but_touches_product_is_flagged(self):
        issues = [
            with_("ENG-1", touches=["CLAUDE.md", "programs/x.rs"], title="Claude: x")
        ]
        drift = prefix_touches_drift(issues)
        self.assertEqual(len(drift), 1)
        self.assertEqual(drift[0][0], "ENG-1")

    def test_meta_only_without_prefix_is_flagged(self):
        issues = [with_("ENG-1", touches=["tools/stage-backlog/**"])]
        drift = prefix_touches_drift(issues)
        self.assertEqual(len(drift), 1)
        self.assertEqual(drift[0][0], "ENG-1")

    def test_prefixed_with_no_touches_is_not_flagged(self):
        # Nothing to check the prefix against — left alone.
        issues = [with_("ENG-1", title="Claude: no touches yet")]
        self.assertEqual(prefix_touches_drift(issues), [])


if __name__ == "__main__":
    unittest.main()
