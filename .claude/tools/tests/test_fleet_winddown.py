"""Tests for ``fleet_winddown.py``: the split, the columns, the roll-call."""

from __future__ import annotations

import io
import json
import os
import unittest
from contextlib import redirect_stdout
from unittest import mock

import fleet_winddown as fw


def issue(n: int, state: str) -> dict:
    return {"identifier": f"ENG-{n}", "title": f"t{n}", "state": {"name": state}}


def pr(number, state="OPEN", mergeable="MERGEABLE", merge_state="CLEAN", queue=None):
    return {
        "number": number,
        "state": state,
        "mergeable": mergeable,
        "mergeStateStatus": merge_state,
        "mergeQueueEntry": {"state": queue} if queue else None,
    }


class Split(unittest.TestCase):
    def test_in_review_finishes_and_everything_else_pauses(self):
        self.assertEqual(fw.split("In Review"), fw.FINISH)
        for state in ("In Progress", "Todo", None):
            self.assertEqual(fw.split(state), fw.PAUSE)


class Rows(unittest.TestCase):
    def test_columns_and_order(self):
        issues = [
            issue(30, "In Progress"),
            issue(20, "In Review"),
            issue(10, "In Review"),
            {"identifier": "OPS-1", "state": {"name": "In Review"}},
        ]
        prs = {
            "10": [pr(100, queue="QUEUED")],
            "20": [pr(200, mergeable="CONFLICTING", merge_state="DIRTY")],
            "30": [],
        }
        listed = fw.rows(issues, {"10", "20"}, prs)
        self.assertEqual([r["tag"] for r in listed], ["10", "20", "30"])
        first, second, third = listed
        self.assertEqual((first["pr"], first["queue"]), (100, "QUEUED"))
        self.assertFalse(first["doomed"])
        self.assertTrue(second["doomed"])
        self.assertEqual((third["group"], third["live"]), (fw.PAUSE, False))

    def test_doomed_only_for_a_live_finisher_with_an_open_pr(self):
        conflicting = [pr(1, mergeable="CONFLICTING")]
        merged = [pr(1, state="MERGED")]
        cases = (
            ("In Review", True, conflicting, True),
            ("In Review", False, conflicting, False),  # parked: tomorrow's
            ("In Progress", True, conflicting, False),  # a pauser
            ("In Review", True, merged, False),  # only its tail left
            ("In Review", True, [], False),  # PR-less by design
        )
        for state, live, branch, want in cases:
            with self.subTest(state=state, live=live, branch=branch):
                row = fw.rows(
                    [issue(5, state)], {"5"} if live else set(), {"5": branch}
                )
                self.assertEqual(row[0]["doomed"], want)

    def test_unread_prs_mark_the_columns_unknown_and_doom_nothing(self):
        row = fw.rows([issue(5, "In Review")], {"5"}, None)[0]
        self.assertEqual((row["mergeable"], row["merge_state"]), ("?", "?"))
        self.assertFalse(row["doomed"])


class Summary(unittest.TestCase):
    def test_the_head_counts_and_both_messages_print(self):
        listed = fw.rows(
            [issue(1, "In Review"), issue(2, "In Progress")],
            {"1"},
            {"1": [pr(9, mergeable="CONFLICTING")], "2": []},
        )
        text = "\n".join(fw.summary(listed, None))
        self.assertIn("2 in flight | 1 live | 1 parked | 1 doomed", text)
        self.assertIn("PR #9 CONFLICTING/CLEAN DOOMED", text)
        self.assertIn(fw.FINISH_MESSAGE, text)
        self.assertIn(fw.PAUSE_MESSAGE, text)

    def test_the_messages_carry_the_rule_and_the_park(self):
        for message in (fw.FINISH_MESSAGE, fw.PAUSE_MESSAGE):
            self.assertIn("close_session.py --park", message)
            self.assertIn("Standing down; ready to resume tomorrow.", message)
        self.assertIn("linear", fw.FINISH_MESSAGE)
        self.assertIn("first safe point", fw.PAUSE_MESSAGE)


class Cli(unittest.TestCase):
    def test_json_run_reads_once_and_survives_a_failed_pr_read(self):
        env = {"LINEAR_API_KEY": "k", "LINEAR_PROJECT_ID": "p"}
        out = io.StringIO()
        failing = mock.MagicMock(side_effect=fw.fleet_resume.FleetResumeError("down"))
        with (
            mock.patch.dict(os.environ, env),
            mock.patch.object(
                fw.fleet_resume, "in_flight", return_value=[issue(4, "In Review")]
            ),
            mock.patch.object(fw.fleet_resume, "live_tags", return_value={"4"}),
            mock.patch.object(fw.fleet_resume, "branch_prs", failing),
            redirect_stdout(out),
        ):
            self.assertEqual(fw.run(["fleet_winddown.py", "--json"]), 0)
        payload = json.loads(out.getvalue())
        failing.assert_called_once_with(["4"])
        self.assertIn("PR columns unread", payload["note"])
        self.assertEqual(payload["rows"][0]["group"], fw.FINISH)


if __name__ == "__main__":
    unittest.main()
