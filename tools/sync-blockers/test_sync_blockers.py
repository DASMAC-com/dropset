"""Stdlib ``unittest`` tests for the sync-blockers edge maintainer.

Covers the pure path-glob helpers and the overlap-materialization sweep in both
modes — the full pairwise sweep and the ``--for`` incremental (focus) mode. The
old chips-tree renderer, its tally, and the ``Claude:`` bucketing are gone with
the Task Staging document, so their cases are gone too. Run with
``python3 -m unittest`` from ``tools/sync-blockers``.
"""

import unittest

from sync_blockers import (
    Issue,
    materialize_overlap_edges,
    missing_touches,
    parse_number,
    parse_touches,
    touches_overlap,
)


def issue(ident, touches=()):
    return Issue(id=ident, number=parse_number(ident), touches=list(touches))


def with_(ident, touches=(), blocked_by=(), blocks=()):
    return Issue(
        id=ident,
        number=parse_number(ident),
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

    def test_missing_touches_reported(self):
        issues = [issue("ENG-9"), with_("ENG-10", touches=["a/b.rs"])]
        self.assertEqual(missing_touches(issues), ["ENG-9"])


class FullSweepTests(unittest.TestCase):
    """The full pairwise sweep files a real ``blocks`` relation (lower blocks
    higher) for every undeclared file-overlap. ``--dry-run`` writes nothing but
    still returns the pairs it would file."""

    def test_overlap_files_lower_blocks_higher(self):
        # Input order higher-first to prove the lower number is chosen.
        issues = [
            with_("ENG-22", touches=["tui/"]),
            with_("ENG-18", touches=["tui/"]),
        ]
        filed = materialize_overlap_edges(issues, None, True)
        self.assertEqual(filed, [("ENG-18", "ENG-22")])

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

    def test_sorted_lowest_first(self):
        issues = [
            with_("ENG-30", touches=["a/"]),
            with_("ENG-10", touches=["a/"]),
            with_("ENG-20", touches=["a/"]),
        ]
        filed = materialize_overlap_edges(issues, None, True)
        # All three share a/: 10↔20, 10↔30, 20↔30, lowest-first.
        self.assertEqual(
            filed, [("ENG-10", "ENG-20"), ("ENG-10", "ENG-30"), ("ENG-20", "ENG-30")]
        )


class IncrementalFocusTests(unittest.TestCase):
    """``--for ENG-###`` files edges for *only* the named issue: pairs that
    don't include the focus issue are left for their own filer."""

    def test_focus_files_only_its_own_overlaps(self):
        # ENG-10 and ENG-20 overlap each other, and ENG-30 (the focus) overlaps
        # both — but the 10↔20 pair is not touched, only 30's two edges.
        issues = [
            with_("ENG-10", touches=["a/"]),
            with_("ENG-20", touches=["a/"]),
            with_("ENG-30", touches=["a/"]),
        ]
        filed = materialize_overlap_edges(issues, None, True, focus_id="ENG-30")
        self.assertEqual(filed, [("ENG-10", "ENG-30"), ("ENG-20", "ENG-30")])

    def test_focus_with_no_overlap_files_nothing(self):
        issues = [
            with_("ENG-10", touches=["a/x.rs"]),
            with_("ENG-30", touches=["b/y.rs"]),
        ]
        self.assertEqual(
            materialize_overlap_edges(issues, None, True, focus_id="ENG-30"), []
        )

    def test_focus_respects_declared_edge(self):
        issues = [
            with_("ENG-10", touches=["a/"]),
            with_("ENG-30", touches=["a/"], blocked_by=["ENG-10"]),
        ]
        self.assertEqual(
            materialize_overlap_edges(issues, None, True, focus_id="ENG-30"), []
        )


if __name__ == "__main__":
    unittest.main()
