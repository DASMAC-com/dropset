"""Stdlib ``unittest`` tests for the sync-blockers edge maintainer.

Covers the pure path-glob helpers and the overlap-materialization sweep in both
modes — the full pairwise sweep and the ``--for`` incremental (focus) mode. The
old chips-tree renderer, its tally, and the ``Claude:`` bucketing are gone with
the Task Staging document, so their cases are gone too. Run via the repo's
``make tools-tests``.
"""

import unittest

from sync_blockers import (
    URGENT_PRIORITY,
    Issue,
    SyncBlockersError,
    _parse_args,
    _raw_to_issue,
    materialize_overlap_edges,
    missing_touches,
    parse_number,
    parse_touches,
    todo_blocks_backlog,
    touches_overlap,
    urgent_gated_by_non_urgent,
)


def issue(ident, touches=()):
    return Issue(id=ident, number=parse_number(ident), touches=list(touches))


def with_(ident, touches=(), blocked_by=(), blocks=(), priority=0):
    return Issue(
        id=ident,
        number=parse_number(ident),
        touches=list(touches),
        blocked_by=list(blocked_by),
        blocks=list(blocks),
        priority=priority,
    )


def sweep(issues, **kwargs):
    """The ``filed`` half of a dry-run sweep — most cases only assert on it."""
    return materialize_overlap_edges(issues, None, True, **kwargs)[0]


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
        self.assertEqual(sweep(issues), [("ENG-18", "ENG-22")])

    def test_declared_edge_suppresses_overlap_edge(self):
        # A declared edge in either direction wins; no overlap edge is filed.
        issues = [
            with_("ENG-18", touches=["tui/"]),
            with_("ENG-22", touches=["tui/"], blocked_by=["ENG-18"]),
        ]
        self.assertEqual(sweep(issues), [])

    def test_distinct_files_file_no_edge(self):
        issues = [
            with_("ENG-18", touches=["tui/pane.rs"]),
            with_("ENG-22", touches=["tui/action.rs"]),
        ]
        self.assertEqual(sweep(issues), [])

    def test_sorted_lowest_first(self):
        issues = [
            with_("ENG-30", touches=["a/"]),
            with_("ENG-10", touches=["a/"]),
            with_("ENG-20", touches=["a/"]),
        ]
        # All three share a/: 10↔20, 10↔30, 20↔30, lowest-first.
        self.assertEqual(
            sweep(issues),
            [("ENG-10", "ENG-20"), ("ENG-10", "ENG-30"), ("ENG-20", "ENG-30")],
        )


class PriorityFloorTests(unittest.TestCase):
    """The number-ordered edge is never allowed to gate an Urgent issue behind a
    non-Urgent one — the inversion that made a live one-atom fix unpullable
    behind two unstarted feature issues."""

    def test_urgent_higher_number_is_not_gated(self):
        issues = [
            with_("ENG-778", touches=["a/"], priority=3),
            with_("ENG-783", touches=["a/"], priority=URGENT_PRIORITY),
        ]
        filed, suppressed = materialize_overlap_edges(issues, None, True)
        self.assertEqual(filed, [])
        self.assertEqual(suppressed, [("ENG-778", "ENG-783")])

    def test_every_non_urgent_blocker_is_suppressed(self):
        # The reported case: two Medium issues both colliding with one Urgent.
        issues = [
            with_("ENG-778", touches=["a/"], priority=3),
            with_("ENG-780", touches=["a/"], priority=3),
            with_("ENG-783", touches=["a/"], priority=URGENT_PRIORITY),
        ]
        filed, suppressed = materialize_overlap_edges(issues, None, True)
        # 778↔780 is an ordinary pair and still files.
        self.assertEqual(filed, [("ENG-778", "ENG-780")])
        self.assertEqual(suppressed, [("ENG-778", "ENG-783"), ("ENG-780", "ENG-783")])

    def test_urgent_lower_number_still_blocks_a_non_urgent(self):
        # No inversion here: the Urgent issue is already the blocker.
        issues = [
            with_("ENG-700", touches=["a/"], priority=URGENT_PRIORITY),
            with_("ENG-783", touches=["a/"], priority=3),
        ]
        filed, suppressed = materialize_overlap_edges(issues, None, True)
        self.assertEqual(filed, [("ENG-700", "ENG-783")])
        self.assertEqual(suppressed, [])

    def test_two_urgent_issues_still_link(self):
        issues = [
            with_("ENG-778", touches=["a/"], priority=URGENT_PRIORITY),
            with_("ENG-783", touches=["a/"], priority=URGENT_PRIORITY),
        ]
        filed, suppressed = materialize_overlap_edges(issues, None, True)
        self.assertEqual(filed, [("ENG-778", "ENG-783")])
        self.assertEqual(suppressed, [])

    def test_no_priority_pair_is_unaffected(self):
        issues = [
            with_("ENG-18", touches=["a/"]),
            with_("ENG-22", touches=["a/"]),
        ]
        filed, suppressed = materialize_overlap_edges(issues, None, True)
        self.assertEqual(filed, [("ENG-18", "ENG-22")])
        self.assertEqual(suppressed, [])

    def test_focus_mode_honors_the_floor(self):
        issues = [
            with_("ENG-778", touches=["a/"], priority=3),
            with_("ENG-783", touches=["a/"], priority=URGENT_PRIORITY),
        ]
        filed, suppressed = materialize_overlap_edges(
            issues, None, True, focus_id="ENG-783"
        )
        self.assertEqual(filed, [])
        self.assertEqual(suppressed, [("ENG-778", "ENG-783")])


class UrgentGatedReportTests(unittest.TestCase):
    """The read-only report for inversions already on the board."""

    def test_reports_a_non_urgent_blocker_of_an_urgent_issue(self):
        issues = [
            Issue(
                id="ENG-783",
                number=783,
                priority=URGENT_PRIORITY,
                blocked_by_priority=[("ENG-778", 3), ("ENG-780", 2)],
            )
        ]
        self.assertEqual(
            urgent_gated_by_non_urgent(issues),
            [("ENG-778", "Medium", "ENG-783"), ("ENG-780", "High", "ENG-783")],
        )

    def test_urgent_blocker_of_an_urgent_issue_is_fine(self):
        issues = [
            Issue(
                id="ENG-783",
                number=783,
                priority=URGENT_PRIORITY,
                blocked_by_priority=[("ENG-778", URGENT_PRIORITY)],
            )
        ]
        self.assertEqual(urgent_gated_by_non_urgent(issues), [])

    def test_non_urgent_blocked_issue_is_not_reported(self):
        issues = [
            Issue(
                id="ENG-783",
                number=783,
                priority=3,
                blocked_by_priority=[("ENG-778", 3)],
            )
        ]
        self.assertEqual(urgent_gated_by_non_urgent(issues), [])


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
        self.assertEqual(
            sweep(issues, focus_id="ENG-30"),
            [("ENG-10", "ENG-30"), ("ENG-20", "ENG-30")],
        )

    def test_focus_with_no_overlap_files_nothing(self):
        issues = [
            with_("ENG-10", touches=["a/x.rs"]),
            with_("ENG-30", touches=["b/y.rs"]),
        ]
        self.assertEqual(sweep(issues, focus_id="ENG-30"), [])

    def test_focus_respects_declared_edge(self):
        issues = [
            with_("ENG-10", touches=["a/"]),
            with_("ENG-30", touches=["a/"], blocked_by=["ENG-10"]),
        ]
        self.assertEqual(sweep(issues, focus_id="ENG-30"), [])


def raw_issue(ident, blockers=(), priority=0, blocker_priorities=None):
    """Build a raw GraphQL issue node. ``blockers`` is an iterable of
    ``(identifier, state_type, state_name)`` for its ``blockedBy`` edges;
    ``blocker_priorities`` optionally maps a blocker id to its priority int."""
    by_id = blocker_priorities or {}
    return {
        "id": f"uuid-{ident}",
        "identifier": ident,
        "description": "",
        "priority": priority,
        "relations": {"nodes": []},
        "inverseRelations": {
            "nodes": [
                {
                    "type": "blocks",
                    "issue": {
                        "identifier": bid,
                        "priority": by_id.get(bid, 0),
                        "state": {"name": state_name, "type": state_type},
                    },
                }
                for bid, state_type, state_name in blockers
            ]
        },
    }


class TodoBlocksBacklogTests(unittest.TestCase):
    """A Todo (``unstarted``) issue blocking a Backlog issue is a scheduling
    smell; the report-only detector surfaces it."""

    def test_raw_to_issue_extracts_only_unstarted_blockers(self):
        raw = raw_issue(
            "ENG-50",
            blockers=[
                ("ENG-10", "unstarted", "Todo"),
                ("ENG-20", "started", "In Progress"),
                ("ENG-30", "backlog", "Backlog"),
            ],
        )
        got = _raw_to_issue(raw)
        # blocked_by carries every blocker; todo_blockers only the unstarted one.
        self.assertEqual(got.blocked_by, ["ENG-10", "ENG-20", "ENG-30"])
        self.assertEqual(got.todo_blockers, [("ENG-10", "Todo")])

    def test_raw_to_issue_extracts_priorities(self):
        raw = raw_issue(
            "ENG-783",
            blockers=[("ENG-778", "backlog", "Backlog")],
            priority=URGENT_PRIORITY,
            blocker_priorities={"ENG-778": 3},
        )
        got = _raw_to_issue(raw)
        self.assertEqual(got.priority, URGENT_PRIORITY)
        self.assertEqual(got.blocked_by_priority, [("ENG-778", 3)])

    def test_raw_to_issue_defaults_a_missing_priority_to_zero(self):
        raw = raw_issue("ENG-50")
        del raw["priority"]
        self.assertEqual(_raw_to_issue(raw).priority, 0)

    def test_detector_returns_sorted_triples(self):
        issues = [
            Issue(id="ENG-40", number=40, todo_blockers=[("ENG-9", "Todo")]),
            Issue(
                id="ENG-20",
                number=20,
                todo_blockers=[("ENG-15", "Todo"), ("ENG-5", "Todo")],
            ),
        ]
        self.assertEqual(
            todo_blocks_backlog(issues),
            [
                ("ENG-5", "Todo", "ENG-20"),
                ("ENG-15", "Todo", "ENG-20"),
                ("ENG-9", "Todo", "ENG-40"),
            ],
        )

    def test_no_todo_blockers_is_empty(self):
        issues = [Issue(id="ENG-1", number=1, blocked_by=["ENG-2"])]
        self.assertEqual(todo_blocks_backlog(issues), [])


class ParseArgsTests(unittest.TestCase):
    """The report-only flag and its mutual exclusion with --for."""

    def test_report_todo_flag(self):
        dry_run, focus_id, report_todo = _parse_args(["--report-todo-blocks"])
        self.assertFalse(dry_run)
        self.assertIsNone(focus_id)
        self.assertTrue(report_todo)

    def test_report_todo_rejects_for(self):
        with self.assertRaises(SyncBlockersError):
            _parse_args(["--report-todo-blocks", "--for", "ENG-1"])

    def test_bare_full_sweep(self):
        self.assertEqual(_parse_args([]), (False, None, False))

    def test_for_normalizes_to_upper(self):
        self.assertEqual(_parse_args(["--for", "eng-9"]), (False, "ENG-9", False))


if __name__ == "__main__":
    unittest.main()
