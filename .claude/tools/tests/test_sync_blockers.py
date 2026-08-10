"""Stdlib ``unittest`` tests for the sync-blockers relation maintainer.

Covers the pure path-glob helpers, the overlap-materialization sweep in both
modes (full pairwise and ``--for`` incremental), the three sweep-report builders,
and the ``--demote`` migration's candidate detection. The load-bearing assertion
throughout is the **negative** one: no code path files a ``blocks`` edge, because
blocking is human-curated. Run via the repo's ``make tools-tests``.
"""

import unittest

import sync_blockers
from sync_blockers import (
    URGENT_PRIORITY,
    BlockEdge,
    Issue,
    Options,
    SyncBlockersError,
    _parse_args,
    _raw_to_issue,
    collision_clusters,
    demote_candidates,
    materialize_overlap_relations,
    missing_touches,
    overlapping_paths,
    parse_number,
    parse_pair_filter,
    parse_touches,
    semantic_blocks,
    todo_blocks_backlog,
    touches_overlap,
    urgent_gated_by_non_urgent,
)


def issue(ident, touches=()):
    return Issue(id=ident, number=parse_number(ident), touches=list(touches))


def with_(
    ident,
    touches=(),
    blocked_by=(),
    blocks=(),
    related_to=(),
    block_edges=(),
    priority=0,
):
    return Issue(
        id=ident,
        number=parse_number(ident),
        uuid=f"uuid-{ident}",
        touches=list(touches),
        blocked_by=list(blocked_by),
        blocks=list(blocks),
        related_to=list(related_to),
        block_edges=list(block_edges),
        priority=priority,
    )


def sweep(issues, **kwargs):
    """A dry-run sweep reduced to its ``(a, b)`` pairs — most cases only need
    which pairs got linked, not the paths they collide on."""
    return [
        (a, b)
        for a, b, _paths in materialize_overlap_relations(issues, None, True, **kwargs)
    ]


def sweep_full(issues, **kwargs):
    """A dry-run sweep with the shared paths kept, for the cases that assert on
    the collision line's content."""
    return materialize_overlap_relations(issues, None, True, **kwargs)


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


class OverlappingPathsTests(unittest.TestCase):
    """A collision names the region the two issues actually meet on — the deeper
    of the two globs — which is what makes a collision line and a cluster's shared
    paths readable."""

    def test_names_the_deeper_glob_as_the_region(self):
        self.assertEqual(
            overlapping_paths(
                issue("ENG-1", ["tui/**"]), issue("ENG-2", ["tui/pane.rs"])
            ),
            ["tui/pane.rs"],
        )

    def test_identical_globs_name_themselves(self):
        self.assertEqual(
            overlapping_paths(
                issue("ENG-1", ["CLAUDE.md"]), issue("ENG-2", ["CLAUDE.md"])
            ),
            ["CLAUDE.md"],
        )

    def test_multiple_regions_are_deduped_and_sorted(self):
        a = issue("ENG-1", ["tui/**", "bots/maker-bot/**", "docs/x.md"])
        b = issue("ENG-2", ["tui/pane.rs", "bots/maker-bot/src/lib.rs", "other/y.md"])
        self.assertEqual(
            overlapping_paths(a, b),
            ["bots/maker-bot/src/lib.rs", "tui/pane.rs"],
        )

    def test_no_collision_is_empty(self):
        self.assertEqual(
            overlapping_paths(issue("ENG-1", ["a/x.rs"]), issue("ENG-2", ["b/y.rs"])),
            [],
        )


class FullSweepTests(unittest.TestCase):
    """The full pairwise sweep relates every unlinked file-overlap. ``--dry-run``
    writes nothing but still returns the pairs it would link."""

    def test_overlap_relates_the_pair(self):
        # Input order higher-first to prove output is ordered by number.
        issues = [
            with_("ENG-22", touches=["tui/"]),
            with_("ENG-18", touches=["tui/"]),
        ]
        self.assertEqual(sweep(issues), [("ENG-18", "ENG-22")])

    def test_reports_the_paths_the_pair_collides_on(self):
        issues = [
            with_("ENG-18", touches=["tui/**"]),
            with_("ENG-22", touches=["tui/pane.rs"]),
        ]
        self.assertEqual(sweep_full(issues), [("ENG-18", "ENG-22", ["tui/pane.rs"])])

    def test_declared_block_suppresses_the_related_link(self):
        # A human-declared block already expresses the coupling more strongly.
        issues = [
            with_("ENG-18", touches=["tui/"]),
            with_("ENG-22", touches=["tui/"], blocked_by=["ENG-18"]),
        ]
        self.assertEqual(sweep(issues), [])

    def test_existing_related_link_is_not_duplicated(self):
        issues = [
            with_("ENG-18", touches=["tui/"], related_to=["ENG-22"]),
            with_("ENG-22", touches=["tui/"]),
        ]
        self.assertEqual(sweep(issues), [])

    def test_existing_related_link_seen_from_either_side(self):
        # Linear stores `related` one-directional, so the inverse side counts too.
        issues = [
            with_("ENG-18", touches=["tui/"]),
            with_("ENG-22", touches=["tui/"], related_to=["ENG-18"]),
        ]
        self.assertEqual(sweep(issues), [])

    def test_distinct_files_link_nothing(self):
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


class NoBlockingEdgeTests(unittest.TestCase):
    """The rework's central guarantee: the sweep writes ``related``, never
    ``blocks``, and therefore has no priority floor left to get wrong."""

    def setUp(self):
        self.calls = []
        self._real = sync_blockers.issue_relation_create

        def spy(api_key, issue_uuid, related_uuid, relation_type="related"):
            self.calls.append((issue_uuid, related_uuid, relation_type))

        sync_blockers.issue_relation_create = spy
        self.addCleanup(setattr, sync_blockers, "issue_relation_create", self._real)

    def test_a_real_sweep_files_related_not_blocks(self):
        issues = [
            with_("ENG-18", touches=["a/"]),
            with_("ENG-22", touches=["a/"]),
        ]
        materialize_overlap_relations(issues, "key", False)
        self.assertEqual(self.calls, [("uuid-ENG-18", "uuid-ENG-22", "related")])

    def test_dry_run_writes_nothing(self):
        issues = [
            with_("ENG-18", touches=["a/"]),
            with_("ENG-22", touches=["a/"]),
        ]
        materialize_overlap_relations(issues, "key", True)
        self.assertEqual(self.calls, [])

    def test_an_urgent_issue_is_still_related_no_floor_applies(self):
        """The old priority floor suppressed this pair, because the edge it filed
        would have gated the Urgent issue behind a Medium one. A symmetric related
        link gates nothing, so there is nothing to suppress."""
        issues = [
            with_("ENG-778", touches=["a/"], priority=3),
            with_("ENG-783", touches=["a/"], priority=URGENT_PRIORITY),
        ]
        self.assertEqual(sweep(issues), [("ENG-778", "ENG-783")])

    def test_an_unreadable_priority_no_longer_suppresses(self):
        issues = [
            with_("ENG-778", touches=["a/"], priority=None),
            with_("ENG-783", touches=["a/"], priority=URGENT_PRIORITY),
        ]
        self.assertEqual(sweep(issues), [("ENG-778", "ENG-783")])


class CollisionClusterTests(unittest.TestCase):
    """Clusters are the candidate merge groups housekeeping proposes over, keyed
    per shared **path** rather than by connected component."""

    def test_groups_issues_sharing_a_path(self):
        issues = [
            with_("ENG-10", touches=["tui/**"]),
            with_("ENG-20", touches=["tui/pane.rs"]),
            with_("ENG-30", touches=["frontend/**"]),
        ]
        self.assertEqual(
            collision_clusters(issues),
            [(["ENG-10", "ENG-20"], ["tui/pane.rs"])],
        )

    def test_a_lone_issue_is_not_a_cluster(self):
        issues = [
            with_("ENG-10", touches=["a/x.rs"]),
            with_("ENG-20", touches=["b/y.rs"]),
        ]
        self.assertEqual(collision_clusters(issues), [])

    def test_a_shared_middle_does_NOT_merge_two_groups(self):
        """The load-bearing case. A and C never touch; B collides with each on a
        different path. Transitive grouping would report one 3-issue cluster —
        which over the real Backlog collapsed 25 of 27 issues into one useless
        "merge everything" proposal. Two path-keyed groups is the useful answer.
        """
        issues = [
            with_("ENG-10", touches=["a/**"]),
            with_("ENG-20", touches=["a/x.rs", "b/y.rs"]),
            with_("ENG-30", touches=["b/**"]),
        ]
        self.assertEqual(
            collision_clusters(issues),
            [
                (["ENG-10", "ENG-20"], ["a/x.rs"]),
                (["ENG-20", "ENG-30"], ["b/y.rs"]),
            ],
        )

    def test_three_issues_on_one_path_are_one_group(self):
        """Genuine co-location still groups — it is chaining that doesn't."""
        issues = [
            with_("ENG-10", touches=["feeds/mod.rs"]),
            with_("ENG-20", touches=["feeds/mod.rs"]),
            with_("ENG-30", touches=["feeds/mod.rs"]),
        ]
        self.assertEqual(
            collision_clusters(issues),
            [(["ENG-10", "ENG-20", "ENG-30"], ["feeds/mod.rs"])],
        )

    def test_paths_with_the_same_members_are_merged(self):
        """One group, two names — not the same proposal twice."""
        issues = [
            with_("ENG-10", touches=["a/x.rs", "b/y.rs"]),
            with_("ENG-20", touches=["a/x.rs", "b/y.rs"]),
        ]
        self.assertEqual(
            collision_clusters(issues),
            [(["ENG-10", "ENG-20"], ["a/x.rs", "b/y.rs"])],
        )

    def test_clusters_may_overlap_in_membership(self):
        """An issue appears in every cluster whose path it touches, so the member
        lists deliberately do not partition the Backlog."""
        issues = [
            with_("ENG-10", touches=["a/x.rs"]),
            with_("ENG-20", touches=["a/x.rs", "b/y.rs"]),
            with_("ENG-30", touches=["b/y.rs"]),
        ]
        clusters = collision_clusters(issues)
        appearances = [c for c in clusters if "ENG-20" in c[0]]
        self.assertEqual(len(appearances), 2)

    def test_clusters_sorted_by_lowest_member(self):
        issues = [
            with_("ENG-50", touches=["z/**"]),
            with_("ENG-60", touches=["z/a.rs"]),
            with_("ENG-10", touches=["y/**"]),
            with_("ENG-20", touches=["y/a.rs"]),
        ]
        got = [members for members, _paths in collision_clusters(issues)]
        self.assertEqual(got, [["ENG-10", "ENG-20"], ["ENG-50", "ENG-60"]])

    def test_a_declared_block_does_not_hide_the_collision(self):
        """The related link is skipped for an already-linked pair, but the cluster
        report is about file coupling, so it still shows the pair."""
        issues = [
            with_("ENG-10", touches=["a/**"]),
            with_("ENG-20", touches=["a/x.rs"], blocked_by=["ENG-10"]),
        ]
        self.assertEqual(
            collision_clusters(issues), [(["ENG-10", "ENG-20"], ["a/x.rs"])]
        )


class SemanticBlocksTests(unittest.TestCase):
    """Every surviving block edge is human-declared, so the section is the
    intended scheduling order."""

    def test_collects_from_both_directions(self):
        issues = [
            with_("ENG-10", blocks=["ENG-20"]),
            with_("ENG-30", blocked_by=["ENG-25"]),
        ]
        self.assertEqual(
            semantic_blocks(issues), [("ENG-10", "ENG-20"), ("ENG-25", "ENG-30")]
        )

    def test_one_edge_seen_from_both_ends_is_deduped(self):
        issues = [
            with_("ENG-10", blocks=["ENG-20"]),
            with_("ENG-20", blocked_by=["ENG-10"]),
        ]
        self.assertEqual(semantic_blocks(issues), [("ENG-10", "ENG-20")])

    def test_no_edges_is_empty(self):
        self.assertEqual(semantic_blocks([with_("ENG-10", touches=["a/"])]), [])


class DemoteCandidateTests(unittest.TestCase):
    """``--demote`` proposes over every block edge between two open Backlog
    issues, flagging which ones the old sweep would re-derive today."""

    def test_overlap_derived_candidate_carries_its_paths(self):
        issues = [
            with_(
                "ENG-10",
                touches=["a/**"],
                blocks=["ENG-20"],
                block_edges=[BlockEdge("rel-1", "ENG-10", "ENG-20")],
            ),
            with_("ENG-20", touches=["a/x.rs"], blocked_by=["ENG-10"]),
        ]
        self.assertEqual(
            demote_candidates(issues),
            [(BlockEdge("rel-1", "ENG-10", "ENG-20"), ["a/x.rs"])],
        )

    def test_hand_placed_candidate_has_no_paths(self):
        """No current Touches collision, so the old sweep could not have derived
        it — still a candidate, but flagged as likely hand-placed."""
        issues = [
            with_(
                "ENG-10",
                touches=["a/x.rs"],
                blocks=["ENG-20"],
                block_edges=[BlockEdge("rel-2", "ENG-10", "ENG-20")],
            ),
            with_("ENG-20", touches=["b/y.rs"], blocked_by=["ENG-10"]),
        ]
        self.assertEqual(
            demote_candidates(issues), [(BlockEdge("rel-2", "ENG-10", "ENG-20"), [])]
        )

    def test_an_edge_leaving_the_backlog_is_not_a_candidate(self):
        """The old sweep only ever wrote between two open Backlog issues, so an
        edge to anything else was placed by a human and is out of scope."""
        issues = [
            with_(
                "ENG-10",
                touches=["a/**"],
                blocks=["ENG-900"],
                block_edges=[BlockEdge("rel-3", "ENG-10", "ENG-900")],
            ),
        ]
        self.assertEqual(demote_candidates(issues), [])

    def test_candidates_sorted_by_blocker_then_blocked(self):
        issues = [
            with_(
                "ENG-30",
                touches=["a/**"],
                block_edges=[BlockEdge("rel-b", "ENG-30", "ENG-40")],
            ),
            with_("ENG-40", touches=["a/**"]),
            with_(
                "ENG-10",
                touches=["a/**"],
                block_edges=[BlockEdge("rel-a", "ENG-10", "ENG-40")],
            ),
        ]
        got = [(e.blocker, e.blocked) for e, _paths in demote_candidates(issues)]
        self.assertEqual(got, [("ENG-10", "ENG-40"), ("ENG-30", "ENG-40")])


class RunDemoteTests(unittest.TestCase):
    """The only destructive path in the tool: it deletes board state a human may
    have placed, so every gate on it is asserted here."""

    def setUp(self):
        self.deleted = []
        self.created = []
        real_delete = sync_blockers.issue_relation_delete
        real_create = sync_blockers.issue_relation_create
        sync_blockers.issue_relation_delete = lambda key, rid: self.deleted.append(rid)
        sync_blockers.issue_relation_create = lambda key, a, b, t="related": (
            self.created.append((a, b, t))
        )
        self.addCleanup(setattr, sync_blockers, "issue_relation_delete", real_delete)
        self.addCleanup(setattr, sync_blockers, "issue_relation_create", real_create)

    def _issues(self, shared=True):
        """A single overlap-derived candidate: ENG-10 blocks ENG-20."""
        touches = ["a/**"] if shared else ["a/x.rs"]
        other = ["a/y.rs"] if shared else ["b/z.rs"]
        return [
            with_(
                "ENG-10",
                touches=touches,
                blocks=["ENG-20"],
                block_edges=[BlockEdge("rel-1", "ENG-10", "ENG-20")],
            ),
            with_("ENG-20", touches=other, blocked_by=["ENG-10"]),
        ]

    def test_proposal_only_writes_nothing(self):
        rc = sync_blockers._run_demote(self._issues(), "key", Options(demote=True))
        self.assertEqual(rc, 0)
        self.assertEqual(self.deleted, [])
        self.assertEqual(self.created, [])

    def test_apply_deletes_the_block_then_creates_the_related(self):
        """Order matters: a failure between the two must leave the pair unlinked,
        not carrying both claims."""
        rc = sync_blockers._run_demote(
            self._issues(), "key", Options(demote=True, apply=True)
        )
        self.assertEqual(rc, 0)
        self.assertEqual(self.deleted, ["rel-1"])
        self.assertEqual(self.created, [("uuid-ENG-10", "uuid-ENG-20", "related")])

    def test_apply_skips_the_related_when_one_already_exists(self):
        issues = self._issues()
        issues[0].related_to = ["ENG-20"]
        sync_blockers._run_demote(issues, "key", Options(demote=True, apply=True))
        self.assertEqual(self.deleted, ["rel-1"])
        self.assertEqual(self.created, [])

    def test_a_reciprocal_pair_is_not_double_linked(self):
        """A blocks B *and* B blocks A yields two candidates over one pair; a
        snapshot-only check would create the related link twice."""
        issues = [
            with_(
                "ENG-10",
                touches=["a/**"],
                block_edges=[BlockEdge("rel-1", "ENG-10", "ENG-20")],
            ),
            with_(
                "ENG-20",
                touches=["a/**"],
                block_edges=[BlockEdge("rel-2", "ENG-20", "ENG-10")],
            ),
        ]
        sync_blockers._run_demote(issues, "key", Options(demote=True, apply=True))
        self.assertEqual(sorted(self.deleted), ["rel-1", "rel-2"])
        self.assertEqual(len(self.created), 1)

    def test_hand_placed_candidates_are_held_back_by_default(self):
        """No current collision means the automation could not have derived the
        edge, and the demote is unrecoverable for it — so it needs an opt-in."""
        sync_blockers._run_demote(
            self._issues(shared=False), "key", Options(demote=True, apply=True)
        )
        self.assertEqual(self.deleted, [])
        self.assertEqual(self.created, [])

    def test_include_hand_placed_converts_them(self):
        sync_blockers._run_demote(
            self._issues(shared=False),
            "key",
            Options(demote=True, apply=True, include_hand_placed=True),
        )
        self.assertEqual(self.deleted, ["rel-1"])

    def test_only_restricts_to_the_named_pair(self):
        issues = [
            with_(
                "ENG-10",
                touches=["a/**"],
                block_edges=[
                    BlockEdge("rel-1", "ENG-10", "ENG-20"),
                    BlockEdge("rel-2", "ENG-10", "ENG-30"),
                ],
            ),
            with_("ENG-20", touches=["a/**"]),
            with_("ENG-30", touches=["a/**"]),
        ]
        sync_blockers._run_demote(
            issues,
            "key",
            Options(demote=True, apply=True, only={frozenset(("ENG-10", "ENG-20"))}),
        )
        self.assertEqual(self.deleted, ["rel-1"])

    def test_only_naming_a_non_candidate_deletes_nothing(self):
        sync_blockers._run_demote(
            self._issues(),
            "key",
            Options(demote=True, apply=True, only={frozenset(("ENG-99", "ENG-98"))}),
        )
        self.assertEqual(self.deleted, [])

    def test_an_edge_with_no_relation_id_is_skipped_not_sent(self):
        """Sending "" to issueRelationDelete would abort the migration mid-loop."""
        issues = [
            with_(
                "ENG-10",
                touches=["a/**"],
                block_edges=[BlockEdge("", "ENG-10", "ENG-20")],
            ),
            with_("ENG-20", touches=["a/**"]),
        ]
        rc = sync_blockers._run_demote(issues, "key", Options(demote=True, apply=True))
        self.assertEqual(rc, 0)
        self.assertEqual(self.deleted, [])


class PairFilterTests(unittest.TestCase):
    """``--only`` selects a confirmed subset of demotion candidates."""

    def test_parses_pairs(self):
        self.assertEqual(
            parse_pair_filter("ENG-1:ENG-2,ENG-3:ENG-4"),
            {frozenset(("ENG-1", "ENG-2")), frozenset(("ENG-3", "ENG-4"))},
        )

    def test_is_order_insensitive_and_upper_cases(self):
        self.assertEqual(
            parse_pair_filter("eng-2:eng-1"), {frozenset(("ENG-1", "ENG-2"))}
        )

    def test_tolerates_whitespace_and_trailing_comma(self):
        self.assertEqual(
            parse_pair_filter(" ENG-1 : ENG-2 , "), {frozenset(("ENG-1", "ENG-2"))}
        )

    def test_rejects_a_malformed_pair(self):
        with self.assertRaises(SyncBlockersError):
            parse_pair_filter("ENG-1")
        with self.assertRaises(SyncBlockersError):
            parse_pair_filter("ENG-1:")

    def test_rejects_an_empty_selection(self):
        with self.assertRaises(SyncBlockersError):
            parse_pair_filter(",")


class UrgentGatedReportTests(unittest.TestCase):
    """The read-only report for inversions among the human-declared edges."""

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
    """``--for ENG-###`` links only the named issue's collisions: pairs that
    don't include the focus issue are left for their own filer."""

    def test_focus_links_only_its_own_overlaps(self):
        # ENG-10 and ENG-20 overlap each other, and ENG-30 (the focus) overlaps
        # both — but the 10↔20 pair is not touched, only 30's two links.
        issues = [
            with_("ENG-10", touches=["a/"]),
            with_("ENG-20", touches=["a/"]),
            with_("ENG-30", touches=["a/"]),
        ]
        self.assertEqual(
            sweep(issues, focus_id="ENG-30"),
            [("ENG-10", "ENG-30"), ("ENG-20", "ENG-30")],
        )

    def test_focus_with_no_overlap_links_nothing(self):
        issues = [
            with_("ENG-10", touches=["a/x.rs"]),
            with_("ENG-30", touches=["b/y.rs"]),
        ]
        self.assertEqual(sweep(issues, focus_id="ENG-30"), [])

    def test_focus_respects_a_declared_edge(self):
        issues = [
            with_("ENG-10", touches=["a/"]),
            with_("ENG-30", touches=["a/"], blocked_by=["ENG-10"]),
        ]
        self.assertEqual(sweep(issues, focus_id="ENG-30"), [])


def raw_issue(
    ident,
    blockers=(),
    priority=0,
    blocker_priorities=None,
    outgoing=(),
):
    """Build a raw GraphQL issue node. ``blockers`` is an iterable of
    ``(identifier, state_type, state_name)`` for its ``blockedBy`` edges;
    ``blocker_priorities`` optionally maps a blocker id to its priority int;
    ``outgoing`` is an iterable of ``(relation_id, type, identifier)`` for its own
    outgoing relations."""
    by_id = blocker_priorities or {}
    return {
        "id": f"uuid-{ident}",
        "identifier": ident,
        "description": "",
        "priority": priority,
        "relations": {
            "nodes": [
                {"id": rel_id, "type": rel_type, "relatedIssue": {"identifier": other}}
                for rel_id, rel_type, other in outgoing
            ]
        },
        "inverseRelations": {
            "nodes": [
                {
                    "id": f"rel-{ident}-{bid}",
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


class RawToIssueTests(unittest.TestCase):
    """The GraphQL→model mapping, including the two fields the rework added."""

    def test_extracts_only_unstarted_blockers(self):
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

    def test_extracts_priorities(self):
        raw = raw_issue(
            "ENG-783",
            blockers=[("ENG-778", "backlog", "Backlog")],
            priority=URGENT_PRIORITY,
            blocker_priorities={"ENG-778": 3},
        )
        got = _raw_to_issue(raw)
        self.assertEqual(got.priority, URGENT_PRIORITY)
        self.assertEqual(got.blocked_by_priority, [("ENG-778", 3)])

    def test_maps_a_missing_priority_to_none_not_zero(self):
        """Linear's scale is inverted (Urgent == 1, "No priority" == 0), so
        coercing an absent field to 0 would read as "not Urgent" and silently
        under-report an inversion. Unknown must stay distinguishable from 0."""
        raw = raw_issue("ENG-50")
        del raw["priority"]
        self.assertIsNone(_raw_to_issue(raw).priority)

    def test_keeps_an_explicit_zero_as_zero(self):
        self.assertEqual(_raw_to_issue(raw_issue("ENG-50", priority=0)).priority, 0)

    def test_maps_a_non_numeric_priority_to_none(self):
        raw = raw_issue("ENG-50")
        raw["priority"] = "High"
        self.assertIsNone(_raw_to_issue(raw).priority)

    def test_collects_outgoing_block_edges_with_relation_ids(self):
        raw = raw_issue("ENG-10", outgoing=[("rel-9", "blocks", "ENG-20")])
        got = _raw_to_issue(raw)
        self.assertEqual(got.blocks, ["ENG-20"])
        self.assertEqual(got.block_edges, [BlockEdge("rel-9", "ENG-10", "ENG-20")])

    def test_collects_related_from_both_directions(self):
        raw = raw_issue(
            "ENG-10",
            outgoing=[("rel-1", "related", "ENG-20")],
        )
        raw["inverseRelations"]["nodes"].append(
            {
                "id": "rel-2",
                "type": "related",
                "issue": {
                    "identifier": "ENG-30",
                    "priority": 0,
                    "state": {"name": "Backlog", "type": "backlog"},
                },
            }
        )
        got = _raw_to_issue(raw)
        self.assertEqual(sorted(got.related_to), ["ENG-20", "ENG-30"])

    def test_a_related_relation_is_not_a_block_edge(self):
        raw = raw_issue("ENG-10", outgoing=[("rel-1", "related", "ENG-20")])
        got = _raw_to_issue(raw)
        self.assertEqual(got.block_edges, [])
        self.assertEqual(got.blocks, [])


class TodoBlocksBacklogTests(unittest.TestCase):
    """A Todo (``unstarted``) issue blocking a Backlog issue is a scheduling
    smell; the detector surfaces it."""

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
    """The four modes and the combinations that must be rejected rather than
    silently ignored."""

    def test_bare_full_sweep(self):
        self.assertEqual(_parse_args([]), Options())

    def test_report_todo_flag(self):
        opts = _parse_args(["--report-todo-blocks"])
        self.assertFalse(opts.dry_run)
        self.assertIsNone(opts.focus_id)
        self.assertTrue(opts.report_todo)

    def test_report_todo_rejects_for(self):
        with self.assertRaises(SyncBlockersError):
            _parse_args(["--report-todo-blocks", "--for", "ENG-1"])

    def test_for_normalizes_to_upper(self):
        self.assertEqual(_parse_args(["--for", "eng-9"]), Options(focus_id="ENG-9"))

    def test_demote_defaults_to_proposal_only(self):
        opts = _parse_args(["--demote"])
        self.assertTrue(opts.demote)
        self.assertFalse(opts.apply)
        self.assertIsNone(opts.only)

    def test_demote_with_apply_and_only(self):
        opts = _parse_args(["--demote", "--apply", "--only", "ENG-1:ENG-2"])
        self.assertTrue(opts.demote)
        self.assertTrue(opts.apply)
        self.assertEqual(opts.only, {frozenset(("ENG-1", "ENG-2"))})

    def test_demote_rejects_for(self):
        with self.assertRaises(SyncBlockersError):
            _parse_args(["--demote", "--for", "ENG-1"])

    def test_demote_rejects_report_todo(self):
        with self.assertRaises(SyncBlockersError):
            _parse_args(["--demote", "--report-todo-blocks"])

    def test_apply_without_demote_is_refused(self):
        """A silently-ignored --apply would read as a completed migration."""
        with self.assertRaises(SyncBlockersError):
            _parse_args(["--apply"])

    def test_only_without_demote_is_refused(self):
        """A silently-ignored --only would widen a confirmed subset."""
        with self.assertRaises(SyncBlockersError):
            _parse_args(["--only", "ENG-1:ENG-2"])

    def test_only_requires_an_argument(self):
        with self.assertRaises(SyncBlockersError):
            _parse_args(["--demote", "--only"])

    def test_demote_refuses_dry_run(self):
        """`--demote --apply --dry-run` would delete relations while --dry-run
        promises "write nothing" — the combination must be refused, not resolved."""
        with self.assertRaises(SyncBlockersError):
            _parse_args(["--demote", "--dry-run"])
        with self.assertRaises(SyncBlockersError):
            _parse_args(["--demote", "--apply", "--dry-run"])

    def test_dry_run_is_still_fine_on_the_sweep(self):
        self.assertEqual(_parse_args(["--dry-run"]), Options(dry_run=True))
        self.assertEqual(
            _parse_args(["--for", "eng-9", "--dry-run"]),
            Options(dry_run=True, focus_id="ENG-9"),
        )

    def test_include_hand_placed_without_demote_is_refused(self):
        with self.assertRaises(SyncBlockersError):
            _parse_args(["--include-hand-placed"])

    def test_include_hand_placed_parses_under_demote(self):
        opts = _parse_args(["--demote", "--apply", "--include-hand-placed"])
        self.assertTrue(opts.include_hand_placed)


if __name__ == "__main__":
    unittest.main()
