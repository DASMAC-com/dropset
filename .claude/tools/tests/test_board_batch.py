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


if __name__ == "__main__":
    unittest.main()
