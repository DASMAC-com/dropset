#!/usr/bin/env python3
# cspell:word proirity
"""Unit tests for board_batch.py.

Every network call goes through ``_post``, so the tests patch that one seam
and assert on the GraphQL operations the tool would have sent. Nothing here
touches Linear.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import board_batch as bb  # noqa: E402
from board_batch import (  # noqa: E402
    BoardBatchError,
    apply_fields,
    build_update_input,
    drop_milestoned,
    format_listing,
    index_by_number,
    normalize_priority,
    place_edges,
    resolve_issue,
    run,
)


def _issue(number, title="A title", priority=0, milestone=None, ident=None):
    return {
        "id": f"uuid-{number}",
        "identifier": ident or f"ENG-{number}",
        "number": number,
        "title": title,
        "priority": priority,
        "state": {"name": "Backlog", "type": "backlog"},
        "projectMilestone": {"name": milestone} if milestone else None,
    }


class PriorityTests(unittest.TestCase):
    def test_names_map_to_the_linear_scale(self):
        self.assertEqual(normalize_priority("urgent"), 1)
        self.assertEqual(normalize_priority("Medium"), 3)
        self.assertEqual(normalize_priority("none"), 0)

    def test_integers_pass_through(self):
        self.assertEqual(normalize_priority(2), 2)

    def test_out_of_range_and_garbage_are_rejected(self):
        for bad in (7, -1, "sideways", None, True):
            with self.subTest(bad=bad), self.assertRaises(BoardBatchError):
                normalize_priority(bad)


class StateAliasTests(unittest.TestCase):
    """The single-issue `state` alias over `fields`.

    It exists so the per-session lifecycle transitions are one command rather
    than a JSON file composed to carry one enum — those writes cost ~3.6k each
    through the MCP, which echoes the whole stored body back.
    """

    def test_it_widens_to_the_fields_shape(self):
        self.assertEqual(
            bb._normalize_state_update("ENG-123", "In Review"),
            {"ENG-123": {"state": "In Review"}},
        )

    def test_the_identifier_is_kept_whole_rather_than_parsed(self):
        # Passed through so `_as_ref` can validate the team prefix. Parsing to
        # an int here would let FIN-123 silently mutate ENG-123.
        self.assertIn("FIN-123", bb._normalize_state_update("FIN-123", "Done"))

    def test_the_parser_accepts_the_subcommand(self):
        args = bb._parse_args(
            ["board_batch.py", "state", "--id", "ENG-9", "--state", "In Review"]
        )
        self.assertEqual(args.cmd, "state")
        self.assertEqual(args.id, "ENG-9")
        self.assertEqual(args.state, "In Review")


class UpdateInputTests(unittest.TestCase):
    def test_maps_field_names_to_mutation_arguments(self):
        got = build_update_input({"priority": "high", "parent": "uuid-1"})
        self.assertEqual(got, {"priority": 2, "parentId": "uuid-1"})

    def test_a_null_milestone_passes_through_to_clear_it(self):
        """Clearing the milestone is how a parked audit finding is promoted,
        so `null` must reach the mutation rather than being dropped."""
        self.assertEqual(
            build_update_input({"milestone": None}), {"projectMilestoneId": None}
        )

    def test_unknown_field_names_are_rejected(self):
        """A typo that silently updated nothing would report success."""
        with self.assertRaises(BoardBatchError) as ctx:
            build_update_input({"proirity": 1})
        self.assertIn("proirity", str(ctx.exception))

    def test_description_is_not_an_accepted_field(self):
        """Body edits stay on the MCP patch path by design."""
        with self.assertRaises(BoardBatchError):
            build_update_input({"description": "nope"})

    def test_empty_field_map_is_rejected(self):
        with self.assertRaises(BoardBatchError):
            build_update_input({})

    def test_labels_maps_to_labelIds_and_requires_a_list(self):
        """The only list-valued field, and the only one whose argument name
        differs non-trivially — a wrong mapping would otherwise ship green."""
        self.assertEqual(
            build_update_input({"labels": ["a", "b"]}), {"labelIds": ["a", "b"]}
        )
        with self.assertRaises(BoardBatchError):
            build_update_input({"labels": "not-a-list"})

    def test_a_relation_key_is_rejected(self):
        """Relations are a separate mutation pair and live in `edges`; three
        docs once promised `fields` handled them."""
        for key in ("relation", "blocks", "relations"):
            with self.subTest(key=key), self.assertRaises(BoardBatchError):
                build_update_input({key: "ENG-1"})


class ResolveIssueTests(unittest.TestCase):
    def setUp(self):
        self.by_number = index_by_number([_issue(10), _issue(11)])

    def test_a_bare_number_resolves(self):
        self.assertEqual(resolve_issue(10, self.by_number)["identifier"], "ENG-10")
        self.assertEqual(resolve_issue("10", self.by_number)["identifier"], "ENG-10")

    def test_a_matching_prefix_resolves(self):
        self.assertEqual(
            resolve_issue("ENG-10", self.by_number)["identifier"], "ENG-10"
        )

    def test_a_foreign_team_prefix_is_refused_not_silently_resolved(self):
        """Linear numbers are per-team, so discarding the prefix would let
        FIN-10 mutate ENG-10."""
        with self.assertRaises(BoardBatchError) as ctx:
            resolve_issue("FIN-10", self.by_number)
        self.assertIn("FIN", str(ctx.exception))

    def test_a_non_positive_or_unreadable_number_is_refused(self):
        for bad in ("-5", "0", "abc", "ENG-", True):
            with self.subTest(bad=bad), self.assertRaises(BoardBatchError):
                resolve_issue(bad, self.by_number)

    def test_a_duplicate_number_makes_the_index_refuse_to_guess(self):
        dup = [_issue(10), _issue(10, ident="FIN-10")]
        with self.assertRaises(BoardBatchError) as ctx:
            index_by_number(dup)
        self.assertIn("ambiguous", str(ctx.exception))


class ListingTests(unittest.TestCase):
    def test_listing_is_one_compact_line_per_issue_sorted_by_number(self):
        lines = format_listing([_issue(20, "Later"), _issue(3, "Earlier", 1)])
        self.assertEqual(len(lines), 2)
        self.assertTrue(lines[0].startswith("ENG-3 "))
        self.assertIn("urgent", lines[0])
        self.assertIn("Earlier", lines[0])
        self.assertTrue(lines[1].startswith("ENG-20 "))

    def test_listing_carries_no_description_field(self):
        joined = "\n".join(format_listing([_issue(1, "T")]))
        self.assertNotIn("description", joined)

    def test_milestoned_issues_are_dropped_client_side(self):
        issues = [_issue(1), _issue(2, milestone="Audit findings")]
        self.assertEqual([i["number"] for i in drop_milestoned(issues)], [1])

    def test_show_milestone_appends_the_name(self):
        lines = format_listing([_issue(1, milestone="Hardening")], show_milestone=True)
        self.assertIn("[Hardening]", lines[0])


class ApplyFieldsTests(unittest.TestCase):
    def setUp(self):
        self.by_number = index_by_number([_issue(10), _issue(11)])

    def test_sends_one_mutation_per_issue_and_reports_a_line_each(self):
        calls = []

        def fake_post(api_key, query, variables):
            calls.append(variables)
            return {"issueUpdate": {"success": True}}

        with mock.patch.object(bb, "_post", side_effect=fake_post):
            lines = apply_fields(
                "k", {"10": {"priority": "low"}, "11": {"priority": 1}}, self.by_number
            )
        self.assertEqual(len(calls), 2)
        self.assertEqual(calls[0]["input"], {"priority": 4})
        self.assertEqual(calls[1]["input"], {"priority": 1})
        self.assertEqual(len(lines), 2)
        self.assertTrue(all(line.startswith("SET ENG-1") for line in lines))

    def test_the_mutation_selects_success_only(self):
        """The whole point: no body comes back."""
        seen = {}

        def fake_post(api_key, query, variables):
            seen["query"] = query
            return {"issueUpdate": {"success": True}}

        with mock.patch.object(bb, "_post", side_effect=fake_post):
            apply_fields("k", {"10": {"priority": 1}}, self.by_number)
        self.assertIn("success", seen["query"])
        self.assertNotIn("description", seen["query"])
        self.assertNotIn("title", seen["query"])

    def test_an_eng_prefixed_number_resolves(self):
        with mock.patch.object(
            bb, "_post", return_value={"issueUpdate": {"success": True}}
        ):
            lines = apply_fields("k", {"ENG-10": {"priority": 1}}, self.by_number)
        self.assertIn("ENG-10", lines[0])

    def test_an_unknown_issue_number_is_refused_not_guessed(self):
        with self.assertRaises(BoardBatchError) as ctx:
            apply_fields("k", {"999": {"priority": 1}}, self.by_number)
        self.assertIn("999", str(ctx.exception))

    def test_a_failed_mutation_raises(self):
        with mock.patch.object(
            bb, "_post", return_value={"issueUpdate": {"success": False}}
        ):
            with self.assertRaises(BoardBatchError):
                apply_fields("k", {"10": {"priority": 1}}, self.by_number)

    def test_dry_run_writes_nothing(self):
        with mock.patch.object(bb, "_post") as posted:
            lines = apply_fields(
                "k", {"10": {"priority": 1}}, self.by_number, dry_run=True
            )
        posted.assert_not_called()
        self.assertTrue(lines[0].startswith("WOULD SET"))


class EdgesTests(unittest.TestCase):
    def setUp(self):
        self.by_number = index_by_number([_issue(10), _issue(11)])

    def test_places_a_blocks_relation(self):
        calls = []

        def fake_post(api_key, query, variables):
            calls.append(variables)
            return {"issueRelationCreate": {"success": True}}

        with mock.patch.object(bb, "_post", side_effect=fake_post):
            lines = place_edges("k", [{"blocker": 10, "blocked": 11}], self.by_number)
        self.assertEqual(
            calls[0]["input"],
            {"issueId": "uuid-10", "relatedIssueId": "uuid-11", "type": "blocks"},
        )
        self.assertEqual(lines, ["LINKED ENG-10 blocks ENG-11"])

    def test_an_empty_pair_list_is_refused(self):
        """No discovery mode: an edge nobody decided is the spurious edge the
        human-curated rule exists to prevent."""
        with self.assertRaises(BoardBatchError) as ctx:
            place_edges("k", [], self.by_number)
        self.assertIn("empty", str(ctx.exception))

    def test_a_self_edge_is_refused(self):
        with self.assertRaises(BoardBatchError):
            place_edges("k", [{"blocker": 10, "blocked": 10}], self.by_number)

    def test_a_malformed_pair_is_refused(self):
        for bad in ([{"blocker": 10}], [{"blocked": 11}], ["ENG-10"]):
            with self.subTest(bad=bad), self.assertRaises(BoardBatchError):
                place_edges("k", bad, self.by_number)

    def test_an_unknown_endpoint_is_refused_not_guessed(self):
        with self.assertRaises(BoardBatchError):
            place_edges("k", [{"blocker": 10, "blocked": 999}], self.by_number)

    def test_dry_run_writes_nothing(self):
        with mock.patch.object(bb, "_post") as posted:
            lines = place_edges(
                "k", [{"blocker": 10, "blocked": 11}], self.by_number, dry_run=True
            )
        posted.assert_not_called()
        self.assertTrue(lines[0].startswith("WOULD LINK"))

    def test_a_bad_pair_late_in_the_batch_places_no_edge_at_all(self):
        """Pre-flight: a half-applied set of blocking edges is worse than none
        — it silently drops issues out of the operator's available set."""
        pairs = [
            {"blocker": 10, "blocked": 11},
            {"blocker": 10, "blocked": 999},
        ]
        with mock.patch.object(bb, "_post") as posted:
            with self.assertRaises(BoardBatchError):
                place_edges("k", pairs, self.by_number)
        posted.assert_not_called()

    def test_remove_deletes_the_matching_relation(self):
        calls = []

        def fake_post(api_key, query, variables):
            calls.append((query, variables))
            if "relations" in query:
                return {
                    "issue": {
                        "relations": {
                            "nodes": [
                                {
                                    "id": "rel-1",
                                    "type": "blocks",
                                    "relatedIssue": {
                                        "id": "uuid-11",
                                        "identifier": "ENG-11",
                                    },
                                }
                            ]
                        }
                    }
                }
            return {"issueRelationDelete": {"success": True}}

        with mock.patch.object(bb, "_post", side_effect=fake_post):
            lines = place_edges(
                "k", [{"blocker": 10, "blocked": 11}], self.by_number, remove=True
            )
        self.assertEqual(lines, ["UNLINKED ENG-10 blocks ENG-11"])
        self.assertEqual(calls[-1][1], {"id": "rel-1"})

    def test_remove_reports_an_absent_edge_without_raising(self):
        """The operator's intended end state is reached either way; aborting
        would strand the rest of the batch."""

        def fake_post(api_key, query, variables):
            return {"issue": {"relations": {"nodes": []}}}

        with mock.patch.object(bb, "_post", side_effect=fake_post):
            lines = place_edges(
                "k", [{"blocker": 10, "blocked": 11}], self.by_number, remove=True
            )
        self.assertTrue(lines[0].startswith("ABSENT"))

    def test_remove_ignores_a_relation_of_another_type_or_target(self):
        def fake_post(api_key, query, variables):
            return {
                "issue": {
                    "relations": {
                        "nodes": [
                            {
                                "id": "rel-x",
                                "type": "related",
                                "relatedIssue": {"id": "uuid-11"},
                            },
                            {
                                "id": "rel-y",
                                "type": "blocks",
                                "relatedIssue": {"id": "uuid-99"},
                            },
                        ]
                    }
                }
            }

        with mock.patch.object(bb, "_post", side_effect=fake_post):
            lines = place_edges(
                "k", [{"blocker": 10, "blocked": 11}], self.by_number, remove=True
            )
        self.assertTrue(lines[0].startswith("ABSENT"))


class PaginationTests(unittest.TestCase):
    """The 250-issue cliff, and the scoped resolver that removes it.

    The tool shipped reading exactly one page and raising if it filled. That
    held until the project crossed the page size in August 2026, at which point
    the resolver read — project-wide and unfiltered — failed on *every* write
    subcommand, and the documented fallback was the full-body MCP write the tool
    exists to avoid. These pin both halves of the fix.
    """

    def test_a_listing_read_follows_the_cursor_across_pages(self):
        pages = [
            {
                "issues": {
                    "pageInfo": {"hasNextPage": True, "endCursor": "c1"},
                    "nodes": [_issue(1)],
                }
            },
            {
                "issues": {
                    "pageInfo": {"hasNextPage": False, "endCursor": None},
                    "nodes": [_issue(2)],
                }
            },
        ]
        seen = []

        def fake_post(api_key, query, variables):
            seen.append(variables.get("after"))
            return pages[len(seen) - 1]

        with mock.patch.object(bb, "_post", side_effect=fake_post):
            issues = bb.fetch_issues("k", "p")
        self.assertEqual([i["number"] for i in issues], [1, 2])
        self.assertEqual(seen, [None, "c1"])

    def test_another_page_with_no_cursor_refuses_rather_than_looping(self):
        with mock.patch.object(
            bb,
            "_post",
            return_value={
                "issues": {
                    "pageInfo": {"hasNextPage": True, "endCursor": None},
                    "nodes": [_issue(1)],
                }
            },
        ):
            with self.assertRaises(BoardBatchError) as caught:
                bb.fetch_issues("k", "p")
        self.assertIn("no cursor", str(caught.exception))

    def test_an_endless_cursor_trips_the_page_backstop(self):
        with mock.patch.object(
            bb,
            "_post",
            return_value={
                "issues": {
                    "pageInfo": {"hasNextPage": True, "endCursor": "always"},
                    "nodes": [_issue(1)],
                }
            },
        ):
            with self.assertRaises(BoardBatchError) as caught:
                bb.fetch_issues("k", "p")
        self.assertIn("did not terminate", str(caught.exception))

    def test_a_short_page_with_no_pageinfo_is_treated_as_a_final_page(self):
        with mock.patch.object(
            bb, "_post", return_value={"issues": {"nodes": [_issue(1)]}}
        ):
            self.assertEqual(len(bb.fetch_issues("k", "p")), 1)

    def test_a_full_page_with_no_pageinfo_is_refused(self):
        """Trusting `hasNextPage` alone leaves a full-but-unmarked page reading as
        a complete read. `list` has no other net, so it would print a truncated
        board as the whole board — refuse instead."""
        nodes = [_issue(n) for n in range(bb.PAGE_SIZE)]
        with mock.patch.object(bb, "_post", return_value={"issues": {"nodes": nodes}}):
            with self.assertRaises(BoardBatchError) as caught:
                bb.fetch_issues("k", "p")
        self.assertIn("no pageInfo", str(caught.exception))

    def test_the_edge_reference_keys_have_one_owner(self):
        # `pair_refs` and `place_edges` must not be able to disagree about which
        # keys a pair carries; a third key in one and not the other would be
        # silently under-fetched.
        self.assertEqual(bb.EDGE_REF_KEYS, ("blocker", "blocked"))
        self.assertEqual(
            bb.pair_refs([{k: 1 for k in bb.EDGE_REF_KEYS}]),
            [1] * len(bb.EDGE_REF_KEYS),
        )

    def test_the_resolver_read_filters_on_the_numbers_it_was_given(self):
        seen = []

        def fake_post(api_key, query, variables):
            seen.append(variables["filter"])
            return {"issues": {"nodes": [_issue(10), _issue(12)]}}

        with mock.patch.object(bb, "_post", side_effect=fake_post):
            bb.fetch_issues_by_number("k", "p", [12, 10, 12])
        self.assertEqual(len(seen), 1)
        # Deduplicated, sorted, and scoped to the project as well as the numbers.
        self.assertEqual(seen[0]["number"], {"in": [10, 12]})
        self.assertEqual(seen[0]["project"], {"id": {"eq": "p"}})

    def test_the_resolver_read_chunks_a_large_reference_set(self):
        numbers = list(range(1, bb.RESOLVE_CHUNK * 2 + 3))
        chunks = []

        def fake_post(api_key, query, variables):
            chunks.append(variables["filter"]["number"]["in"])
            return {"issues": {"nodes": []}}

        with mock.patch.object(bb, "_post", side_effect=fake_post):
            bb.fetch_issues_by_number("k", "p", numbers)
        self.assertEqual(len(chunks), 3)
        self.assertEqual(
            [len(c) for c in chunks], [bb.RESOLVE_CHUNK, bb.RESOLVE_CHUNK, 2]
        )
        self.assertEqual(sorted(n for c in chunks for n in c), numbers)

    def test_an_empty_reference_set_reads_nothing_at_all(self):
        with mock.patch.object(bb, "_post") as posted:
            self.assertEqual(bb.fetch_issues_by_number("k", "p", []), [])
        posted.assert_not_called()

    def test_an_unparseable_reference_is_left_for_the_preflight_to_reject(self):
        self.assertEqual(bb.referenced_numbers(["ENG-7", 9, "junk", "0"]), [7, 9])

    def test_pair_refs_reads_both_ends_and_skips_a_malformed_pair(self):
        pairs = [{"blocker": 1, "blocked": 2}, "nonsense", {"blocker": 3}]
        self.assertEqual(bb.pair_refs(pairs), [1, 2, 3])


class BoardSizeIndependenceTests(unittest.TestCase):
    """A write must not care how large the board has grown.

    Precisely what these pin, since the distinction matters: the resolver read is
    **scoped to the payload's numbers**. They do NOT model a 575-issue board —
    no fixture here serves one — so board-size independence is an *inference*
    from the scoping, not something measured. Pagination is measured separately
    in :class:`PaginationTests`.

    They are still non-vacuous against the defect: the old resolver sent no
    `number` key at all, so `variables["filter"]["number"]` raises `KeyError`
    under the pre-fix implementation before any assertion runs.
    """

    def setUp(self):
        self.env = mock.patch.dict(
            os.environ,
            {"LINEAR_API_KEY": "k", "LINEAR_PROJECT_ID": "p"},
        )
        self.env.start()
        self.addCleanup(self.env.stop)

    def _write(self, payload):
        d = tempfile.mkdtemp()
        path = Path(d) / "payload.json"
        path.write_text(json.dumps(payload), encoding="utf-8")
        return str(path)

    def test_fields_writes_on_a_board_far_past_the_page_size(self):
        reads, writes = [], []

        def fake_post(api_key, query, variables):
            if "issues(" in query:
                reads.append(variables["filter"])
                wanted = variables["filter"]["number"]["in"]
                # The server answers the filter: only the named issues come back,
                # however many the project holds in total.
                return {"issues": {"nodes": [_issue(n) for n in wanted]}}
            writes.append(variables)
            return {"issueUpdate": {"success": True}}

        path = self._write({"430": {"priority": "urgent"}, "889": {"priority": "high"}})
        with mock.patch.object(bb, "_post", side_effect=fake_post):
            rc = run(["board_batch.py", "fields", "--updates", path])

        self.assertEqual(rc, 0)
        self.assertEqual(len(writes), 2)
        # One scoped read, naming only the two issues the payload touches — no
        # project-wide index, so a 575-issue board is indistinguishable from a
        # 5-issue one from here.
        self.assertEqual(len(reads), 1)
        self.assertEqual(reads[0]["number"], {"in": [430, 889]})

    def test_edges_resolves_only_the_issues_its_pairs_name(self):
        reads = []

        def fake_post(api_key, query, variables):
            if "issues(" in query:
                reads.append(variables["filter"]["number"]["in"])
                return {
                    "issues": {
                        "nodes": [
                            _issue(n) for n in variables["filter"]["number"]["in"]
                        ]
                    }
                }
            return {"issueRelationCreate": {"success": True}}

        path = self._write([{"blocker": "ENG-889", "blocked": 914}])
        with mock.patch.object(bb, "_post", side_effect=fake_post):
            rc = run(["board_batch.py", "edges", "--pairs", path])

        self.assertEqual(rc, 0)
        self.assertEqual(reads, [[889, 914]])


class CliTests(unittest.TestCase):
    def setUp(self):
        self.env = mock.patch.dict(
            os.environ,
            {"LINEAR_API_KEY": "k", "LINEAR_PROJECT_ID": "p"},
        )
        self.env.start()
        self.addCleanup(self.env.stop)

    def _write(self, payload):
        d = tempfile.mkdtemp()
        path = Path(d) / "payload.json"
        path.write_text(json.dumps(payload), encoding="utf-8")
        return str(path)

    def test_priorities_widens_a_flat_map(self):
        calls = []

        def fake_post(api_key, query, variables):
            if "issues(" in query:
                return {"issues": {"nodes": [_issue(10)]}}
            calls.append(variables)
            return {"issueUpdate": {"success": True}}

        path = self._write({"10": "urgent"})
        with mock.patch.object(bb, "_post", side_effect=fake_post):
            rc = run(["board_batch.py", "priorities", "--updates", path])
        self.assertEqual(rc, 0)
        self.assertEqual(calls[0]["input"], {"priority": 1})

    def test_list_drops_milestoned_issues_by_default(self):
        nodes = [_issue(1, "Live"), _issue(2, "Parked", milestone="Audit findings")]
        with mock.patch.object(bb, "_post", return_value={"issues": {"nodes": nodes}}):
            with mock.patch("sys.stdout") as out:
                rc = run(["board_batch.py", "list"])
        self.assertEqual(rc, 0)
        printed = "".join(c.args[0] for c in out.write.call_args_list if c.args)
        self.assertIn("Live", printed)
        self.assertNotIn("Parked", printed)

    def test_an_empty_updates_file_is_refused(self):
        path = self._write({})
        with mock.patch.object(
            bb, "_post", return_value={"issues": {"nodes": [_issue(10)]}}
        ):
            with self.assertRaises(BoardBatchError):
                run(["board_batch.py", "fields", "--updates", path])

    def test_a_missing_env_var_is_a_clean_error(self):
        with mock.patch.dict(os.environ, {"LINEAR_API_KEY": ""}):
            with self.assertRaises(BoardBatchError) as ctx:
                run(["board_batch.py", "list"])
        self.assertIn("LINEAR_API_KEY", str(ctx.exception))

    def test_a_non_printable_api_key_is_refused_without_echoing_it(self):
        """An embedded newline otherwise reaches http.client's header
        validation, whose ValueError message quotes the credential."""
        secret = "sk-live-SENTINEL\nX"
        with mock.patch.dict(os.environ, {"LINEAR_API_KEY": secret}):
            with self.assertRaises(BoardBatchError) as ctx:
                run(["board_batch.py", "list"])
        self.assertNotIn("SENTINEL", str(ctx.exception))

    def test_edges_cli_is_wired(self):
        calls = []

        def fake_post(api_key, query, variables):
            if "issues(" in query:
                return {"issues": {"nodes": [_issue(10), _issue(11)]}}
            calls.append(variables)
            return {"issueRelationCreate": {"success": True}}

        path = self._write([{"blocker": 10, "blocked": 11}])
        with mock.patch.object(bb, "_post", side_effect=fake_post):
            rc = run(["board_batch.py", "edges", "--pairs", path])
        self.assertEqual(rc, 0)
        self.assertEqual(calls[0]["input"]["type"], "blocks")

    def test_edges_rejects_a_non_list_payload(self):
        path = self._write({"blocker": 10})
        with mock.patch.object(
            bb, "_post", return_value={"issues": {"nodes": [_issue(10)]}}
        ):
            with self.assertRaises(BoardBatchError):
                run(["board_batch.py", "edges", "--pairs", path])

    def test_dry_run_is_accepted_after_the_subcommand(self):
        """Registered only at the top level, `edges --pairs f --dry-run` — the
        form anyone would type — exited 2 on an unrecognized argument."""
        posted = []

        def fake_post(api_key, query, variables):
            if "issues(" in query:
                return {"issues": {"nodes": [_issue(10), _issue(11)]}}
            posted.append(variables)
            return {"issueRelationCreate": {"success": True}}

        path = self._write([{"blocker": 10, "blocked": 11}])
        with mock.patch.object(bb, "_post", side_effect=fake_post):
            rc = run(["board_batch.py", "edges", "--pairs", path, "--dry-run"])
        self.assertEqual(rc, 0)
        self.assertEqual(posted, [])

    def test_dry_run_before_the_subcommand_is_not_reset_by_the_subparser(self):
        """A `False` default on the subparser would overwrite the top-level
        value after parsing — turning the rehearsal flag into a live run."""
        posted = []

        def fake_post(api_key, query, variables):
            if "issues(" in query:
                return {"issues": {"nodes": [_issue(10), _issue(11)]}}
            posted.append(variables)
            return {"issueRelationCreate": {"success": True}}

        path = self._write([{"blocker": 10, "blocked": 11}])
        with mock.patch.object(bb, "_post", side_effect=fake_post):
            rc = run(["board_batch.py", "--dry-run", "edges", "--pairs", path])
        self.assertEqual(rc, 0)
        self.assertEqual(posted, [])

    def test_a_bad_entry_late_in_a_fields_batch_writes_nothing(self):
        posted = []

        def fake_post(api_key, query, variables):
            if "issues(" in query:
                return {"issues": {"nodes": [_issue(10), _issue(11)]}}
            posted.append(variables)
            return {"issueUpdate": {"success": True}}

        path = self._write({"10": {"priority": 1}, "999": {"priority": 1}})
        with mock.patch.object(bb, "_post", side_effect=fake_post):
            with self.assertRaises(BoardBatchError):
                run(["board_batch.py", "fields", "--updates", path])
        self.assertEqual(posted, [])


class FieldValueResolutionTests(unittest.TestCase):
    """A state NAME must become a UUID before it reaches Linear.

    Passing the name through is not a loud failure — it dies as an unnamed
    ``Argument Validation Error``, which cost one session ~2k and six round
    trips to trace back to a documented example that was simply wrong. Worse,
    ``--dry-run`` reported a confident green throughout, because it resolved
    issue numbers and never field values.
    """

    STATE_ID = "11111111-2222-3333-4444-555555555555"

    def setUp(self):
        issue = _issue(10)
        issue["team"] = {"id": "team-1"}
        self.by_number = index_by_number([issue, _issue(11)])

    def _post_factory(self, calls):
        def fake_post(api_key, query, variables):
            calls.append((query, variables))
            if "TeamStates" in query:
                return {
                    "team": {
                        "states": {
                            "nodes": [
                                {"id": self.STATE_ID, "name": "In Review"},
                                {"id": "other-id", "name": "Done"},
                            ]
                        }
                    }
                }
            return {"issueUpdate": {"success": True}}

        return fake_post

    def test_a_state_name_is_resolved_to_a_uuid(self):
        calls = []
        with mock.patch.object(bb, "_post", side_effect=self._post_factory(calls)):
            apply_fields("k", {"10": {"state": "In Review"}}, self.by_number)
        writes = [v for q, v in calls if "issueUpdate" in q]
        self.assertEqual(writes[0]["input"], {"stateId": self.STATE_ID})

    def test_the_name_match_is_case_insensitive(self):
        calls = []
        with mock.patch.object(bb, "_post", side_effect=self._post_factory(calls)):
            apply_fields("k", {"10": {"state": "in review"}}, self.by_number)
        writes = [v for q, v in calls if "issueUpdate" in q]
        self.assertEqual(writes[0]["input"], {"stateId": self.STATE_ID})

    def test_a_uuid_state_passes_through_without_a_lookup(self):
        calls = []
        with mock.patch.object(bb, "_post", side_effect=self._post_factory(calls)):
            apply_fields("k", {"10": {"state": self.STATE_ID}}, self.by_number)
        self.assertFalse([q for q, _ in calls if "TeamStates" in q])

    def test_an_unknown_state_name_names_the_available_states(self):
        calls = []
        with mock.patch.object(bb, "_post", side_effect=self._post_factory(calls)):
            with self.assertRaises(BoardBatchError) as caught:
                apply_fields("k", {"10": {"state": "Shipped"}}, self.by_number)
        self.assertIn("In Review", str(caught.exception))

    def test_a_dry_run_resolves_field_values_rather_than_reporting_a_false_green(self):
        # The regression this guards: --dry-run resolved issue numbers only, so
        # a bad state name passed the check and then failed on the real run.
        calls = []
        with mock.patch.object(bb, "_post", side_effect=self._post_factory(calls)):
            with self.assertRaises(BoardBatchError):
                apply_fields(
                    "k", {"10": {"state": "Shipped"}}, self.by_number, dry_run=True
                )
        self.assertFalse([q for q, _ in calls if "issueUpdate" in q])

    def test_the_states_lookup_is_read_once_per_team(self):
        calls = []
        issue11 = _issue(11)
        issue11["team"] = {"id": "team-1"}
        by_number = index_by_number([self.by_number[10], issue11])
        with mock.patch.object(bb, "_post", side_effect=self._post_factory(calls)):
            apply_fields(
                "k",
                {"10": {"state": "In Review"}, "11": {"state": "Done"}},
                by_number,
            )
        self.assertEqual(len([q for q, _ in calls if "TeamStates" in q]), 1)

    def test_a_SECOND_team_gets_its_own_lookup(self):
        # With both issues forced onto one team, a cache keyed on nothing at all
        # passes identically — so the "per team" in the name above was untested.
        # A globally-keyed cache would write issue 11 a stateId belonging to
        # team-1, which is the unnamed Argument Validation Error this whole
        # class exists to prevent.
        calls = []
        issue11 = _issue(11)
        issue11["team"] = {"id": "team-2"}
        by_number = index_by_number([self.by_number[10], issue11])
        with mock.patch.object(bb, "_post", side_effect=self._post_factory(calls)):
            apply_fields(
                "k",
                {"10": {"state": "In Review"}, "11": {"state": "In Review"}},
                by_number,
            )
        team_queries = [v for q, v in calls if "TeamStates" in q]
        self.assertEqual(len(team_queries), 2)
        self.assertEqual(
            sorted(v["teamId"] for v in team_queries), ["team-1", "team-2"]
        )

    def test_a_parent_issue_number_resolves_through_the_existing_lookup(self):
        calls = []
        with mock.patch.object(bb, "_post", side_effect=self._post_factory(calls)):
            apply_fields("k", {"10": {"parent": "ENG-11"}}, self.by_number)
        writes = [v for q, v in calls if "issueUpdate" in q]
        self.assertEqual(writes[0]["input"], {"parentId": "uuid-11"})

    def test_a_milestone_name_is_refused_with_the_reason(self):
        # No cheap lookup exists for these, so the next best thing is failing
        # with a message that names the problem instead of an opaque API error.
        with self.assertRaises(BoardBatchError) as caught:
            build_update_input({"milestone": "Audit findings"})
        self.assertIn("name rather than an id", str(caught.exception))

    def test_an_opaque_id_without_whitespace_still_passes_through(self):
        # The guard keys on whitespace precisely so it never second-guesses an
        # id it cannot validate.
        self.assertEqual(
            build_update_input({"milestone": "abc123"}),
            {"projectMilestoneId": "abc123"},
        )

    def test_clearing_a_milestone_with_null_still_works(self):
        # This is how a parked finding is un-parked; it must survive the guard.
        self.assertEqual(
            build_update_input({"milestone": None}), {"projectMilestoneId": None}
        )


if __name__ == "__main__":
    unittest.main()
