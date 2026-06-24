"""Stdlib ``unittest`` tests for the stage-backlog renderer.

Ported from the Rust crate's ``#[cfg(test)]`` cases in ``model.rs`` / ``plan.rs``
(the merge-only helpers and their tests are gone with the merge subcommand),
plus the orphan-cycle regression for the silent-drop bug the port fixes. Run
with ``python3 -m unittest`` from ``tools/stage-backlog``.
"""

import unittest

from stage_backlog import (
    Issue,
    missing_touches,
    parse_number,
    parse_touches,
    render,
    touches_overlap,
)


def issue(ident, touches=()):
    return Issue(id=ident, number=parse_number(ident), touches=list(touches))


def with_(ident, parent=None, touches=(), blocked_by=(), blocks=()):
    return Issue(
        id=ident,
        number=parse_number(ident),
        parent=parent,
        touches=list(touches),
        blocked_by=list(blocked_by),
        blocks=list(blocks),
    )


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

    def test_skill_only_detection(self):
        self.assertTrue(issue("ENG-1", [".claude/skills/foo/SKILL.md"]).is_skill_only())
        self.assertTrue(
            issue("ENG-2", ["CLAUDE.md", ".claude/skills/bar/SKILL.md"]).is_skill_only()
        )
        # mixed with product code is not pure skill work
        self.assertFalse(
            issue("ENG-3", ["CLAUDE.md", "programs/dropset/src/lib.rs"]).is_skill_only()
        )
        # no touches can't be proven skill-only
        self.assertFalse(issue("ENG-4", []).is_skill_only())

    def test_overlap_same_dir_and_file(self):
        self.assertTrue(touches_overlap(issue("ENG-1", ["tui/"]), issue("ENG-2", ["tui/pane.rs"])))
        self.assertTrue(
            touches_overlap(issue("ENG-1", ["sdk/rs/**"]), issue("ENG-2", ["sdk/rs/lib.rs"]))
        )
        self.assertTrue(
            touches_overlap(issue("ENG-1", ["CLAUDE.md"]), issue("ENG-2", ["CLAUDE.md"]))
        )

    def test_no_overlap_distinct_files(self):
        self.assertFalse(
            touches_overlap(
                issue("ENG-1", ["programs/dropset/src/swap.rs"]),
                issue("ENG-2", ["programs/dropset/src/lib.rs"]),
            )
        )
        # a shared string prefix that is not a path boundary must not match
        self.assertFalse(touches_overlap(issue("ENG-1", ["sdk/rs"]), issue("ENG-2", ["sdk/rust"])))


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
        self.assertEqual(out, "# Standalone\n\n- ENG-10\n    - ENG-20\n")

    def test_file_overlap_serializes_higher_under_lower(self):
        # No declared edge, but both declare the tui/ directory → they can't
        # run in parallel, so 22 nests under 18.
        out = render([with_("ENG-18", touches=["tui/"]), with_("ENG-22", touches=["tui/"])])
        self.assertEqual(out, "# Standalone\n\n- ENG-18\n    - ENG-22\n")

    def test_distinct_files_in_same_dir_run_in_parallel(self):
        # Different files under tui/ don't conflict, so both stay top-level.
        out = render(
            [with_("ENG-18", touches=["tui/pane.rs"]), with_("ENG-22", touches=["tui/action.rs"])]
        )
        self.assertEqual(out, "# Standalone\n\n- ENG-18\n- ENG-22\n")

    def test_skills_bucket_comes_first(self):
        out = render(
            [
                with_("ENG-30", touches=["programs/dropset/src/lib.rs"]),
                with_("ENG-5", touches=[".claude/skills/foo/SKILL.md"]),
            ]
        )
        self.assertEqual(out, "# Skills\n\n- ENG-5\n\n# Standalone\n\n- ENG-30\n")

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

    def test_cross_heading_blocker_renders_after_note(self):
        # ENG-60/61 share a parent heading; ENG-61 is blocked by standalone
        # ENG-70, which is under a different heading → (after ENG-70).
        out = render(
            [
                with_("ENG-60", parent="ENG-50", touches=["a/x.rs"]),
                with_("ENG-61", parent="ENG-50", touches=["b/y.rs"], blocked_by=["ENG-70"]),
                with_("ENG-70", touches=["c/z.rs"]),
            ]
        )
        self.assertEqual(
            out,
            "# ENG-50\n\n- ENG-60\n- ENG-61 (after ENG-70)\n\n# Standalone\n\n- ENG-70\n",
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
        self.assertEqual(out, "# Standalone\n\n- ENG-1\n    - ENG-2\n        - ENG-3\n")

    def test_sibling_blocker_still_renders_also_after(self):
        # ENG-2 and ENG-3 both nest under ENG-1 (siblings). ENG-4 is blocked by
        # both: it nests under ENG-3 (higher number breaks the equal-chain tie),
        # but ENG-2 is a sibling — not an ancestor of ENG-4 — so the nesting
        # can't express it and the "(also after ENG-2)" note stays.
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
            "# Standalone\n\n- ENG-1\n    - ENG-2\n    - ENG-3\n        - ENG-4 (also after ENG-2)\n",
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


class OrphanCycleTests(unittest.TestCase):
    """Regression for the silent-drop bug: a directed cycle in the blocker
    graph means every member has a non-None primary, so none is a bucket root.
    The old planner only emitted nodes reachable from a root, so the whole
    cycle vanished with no warning. The orphan sweep must render every member
    and flag the unreached ids."""

    def test_backward_declared_edge_plus_overlap_renders_all_members(self):
        # The maker-bot cluster: all six share bots/** (file-overlap edges put
        # the higher number under the lower), plus one *backward* declared edge
        # — ENG-602 blockedBy ENG-606, though 606 > 602 — which closes the cycle
        # 602 → 606 → 604 → 602. None of the six is a root.
        members = ["ENG-602", "ENG-604", "ENG-605", "ENG-606", "ENG-607", "ENG-608"]
        issues = [with_(m, touches=["bots/**"]) for m in members]
        issues[0].blocked_by = ["ENG-606"]  # ENG-602 blockedBy ENG-606 (backward)

        orphans = []
        out = render(issues, orphans)

        # Every member renders exactly once — none silently dropped.
        for m in members:
            self.assertIn(f"- {m}", out, f"{m} missing from the rendered tree")
            self.assertEqual(out.count(f"- {m}\n") + out.count(f"- {m} "), 1, f"{m} not unique")

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
            orphans,
        )
        self.assertEqual(orphans, [])


if __name__ == "__main__":
    unittest.main()
