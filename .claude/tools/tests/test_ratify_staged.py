#!/usr/bin/env python3
"""Unit tests for ratify_staged.py.

Every network call goes through ``board_batch._post``, so the end-to-end tests
patch that one seam and answer by the operation the tool sent. Nothing here
touches Linear.
"""

from __future__ import annotations

import io
import os
import sys
import unittest
from contextlib import redirect_stdout
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import ratify_staged as rs  # noqa: E402
from ratify_staged import (  # noqa: E402
    RatifyError,
    build_ask,
    group_sizes,
    open_threads,
    parse_ids,
    plan_approval,
)


def _issue(number, milestone=rs.MILESTONE, state_type="backlog", state="Backlog"):
    return {
        "id": f"uuid-{number}",
        "identifier": f"ENG-{number}",
        "number": number,
        "title": f"Task {number}",
        "priority": 0,
        "state": {"name": state, "type": state_type},
        "projectMilestone": {"name": milestone} if milestone else None,
        "team": {"id": "team-1"},
        "relations": {"nodes": []},
        "inverseRelations": {"nodes": []},
    }


class AskTests(unittest.TestCase):
    def test_group_sizes_never_strand_a_single_task(self):
        self.assertEqual(group_sizes(1), [1])
        self.assertEqual(group_sizes(4), [4])
        self.assertEqual(group_sizes(5), [3, 2])
        self.assertEqual(group_sizes(9), [3, 3, 3])
        self.assertEqual(group_sizes(16), [4, 4, 4, 4])
        for n in range(2, 40):
            sizes = group_sizes(n)
            self.assertEqual(sum(sizes), n)
            self.assertTrue(all(2 <= s <= 4 for s in sizes), (n, sizes))

    def test_one_checkbox_per_task_with_the_verbatim_text(self):
        payload = build_ask([_issue(n) for n in (1, 2, 3, 4, 5)])
        (call,) = payload["calls"]
        labels = [o["label"] for q in call["questions"] for o in q["options"]]
        self.assertEqual(labels, ["ENG-1", "ENG-2", "ENG-3", "ENG-4", "ENG-5"])
        for q in call["questions"]:
            self.assertTrue(q["multiSelect"])
            self.assertTrue(q["question"].startswith(rs.ASK_TEXT))
            self.assertLessEqual(len(q["header"]), 12)
        self.assertTrue(call["questions"][1]["question"].endswith("(2 of 2)"))

    def test_a_lone_task_gets_a_not_yet_option(self):
        (call,) = build_ask([_issue(7)])["calls"]
        (question,) = call["questions"]
        self.assertEqual(question["question"], rs.ASK_TEXT)
        self.assertEqual(
            [o["label"] for o in question["options"]], ["ENG-7", "Not yet"]
        )

    def test_sixteen_fit_one_call_and_seventeen_spill(self):
        self.assertEqual(len(build_ask([_issue(n) for n in range(1, 17)])["calls"]), 1)
        calls = build_ask([_issue(n) for n in range(1, 18)])["calls"]
        self.assertEqual([len(c["questions"]) for c in calls], [4, 1])

    def test_every_question_is_valid_for_any_count(self):
        for n in range(1, 40):
            labels = []
            for call in build_ask([_issue(k) for k in range(1, n + 1)])["calls"]:
                self.assertLessEqual(len(call["questions"]), rs.MAX_QUESTIONS)
                headers = [q["header"] for q in call["questions"]]
                self.assertEqual(len(headers), len(set(headers)), n)
                for q in call["questions"]:
                    self.assertTrue(2 <= len(q["options"]) <= 4, n)
                    self.assertLessEqual(len(q["header"]), 12)
                    labels += [o["label"] for o in q["options"]]
            tasks = [label for label in labels if label != "Not yet"]
            self.assertEqual(tasks, [f"ENG-{k}" for k in range(1, n + 1)])


class ReviewTests(unittest.TestCase):
    def test_only_unresolved_top_level_comments_hold(self):
        comments = [
            {"id": "a", "parentId": None, "resolvedAt": None},
            {"id": "b", "parentId": None, "resolvedAt": "2026-10-23T00:00:00Z"},
            {"id": "c", "parentId": "a", "resolvedAt": None},
        ]
        self.assertEqual([c["id"] for c in open_threads(comments)], ["a"])

    def test_parse_ids_accepts_commas_and_spaces(self):
        self.assertEqual(
            parse_ids(["eng-1,ENG-2", "ENG-3", "ENG-1"]), ["ENG-1", "ENG-2", "ENG-3"]
        )
        with self.assertRaises(RatifyError):
            parse_ids([" , "])


class ApprovalTests(unittest.TestCase):
    def test_the_batch_clears_the_milestone_and_moves_to_todo(self):
        updates, already = plan_approval([_issue(1), _issue(2)], [_issue(3)], held=[])
        self.assertEqual(
            updates,
            {
                "ENG-1": {"milestone": None, "state": "Todo"},
                "ENG-2": {"milestone": None, "state": "Todo"},
                "ENG-3": {"milestone": None, "state": "Canceled"},
            },
        )
        self.assertEqual(already, [])

    def test_a_rerun_skips_what_a_partial_write_already_moved(self):
        moved = _issue(1, milestone=None, state_type="unstarted", state="Todo")
        rejected = _issue(3, state_type="canceled", state="Canceled")
        duplicate = _issue(4, state_type="duplicate", state="Duplicate")
        updates, already = plan_approval(
            [moved, _issue(2)], [rejected, duplicate], held=[]
        )
        self.assertEqual(updates, {"ENG-2": {"milestone": None, "state": "Todo"}})
        self.assertEqual(already, ["ENG-1", "ENG-3", "ENG-4"])

    def test_a_held_or_unstaged_task_refuses_the_whole_batch(self):
        with self.assertRaisesRegex(RatifyError, "still held by an open thread: ENG-2"):
            plan_approval([_issue(1), _issue(2)], [], held=["ENG-2"])
        with self.assertRaisesRegex(RatifyError, "not open under"):
            plan_approval([_issue(1), _issue(5, state_type="duplicate")], [], held=[])
        with self.assertRaisesRegex(RatifyError, "not open under"):
            plan_approval([_issue(1), _issue(4, milestone=None)], [], held=[])
        with self.assertRaisesRegex(RatifyError, "both approved and canceled"):
            plan_approval([_issue(1)], [_issue(1)], held=[])


class FakeLinear:
    """Answers ``board_batch._post`` by operation, recording every write."""

    def __init__(self, issues, comments=None, more_comments=()):
        self.issues = {i["number"]: i for i in issues}
        self.comments = comments or {}
        self.more_comments = set(more_comments)
        self.writes = []

    def __call__(self, api_key, query, variables):
        if "issueUpdate" in query:
            self.writes.append((variables["id"], variables["input"]))
            return {"issueUpdate": {"success": True}}
        if "comments(" in query:
            ident = variables["id"]
            page = {"hasNextPage": ident in self.more_comments}
            nodes = self.comments.get(ident, [])
            return {"issue": {"comments": {"pageInfo": page, "nodes": nodes}}}
        if "TeamStates" in query:
            names = ("Backlog", "Todo", "Canceled")
            return {
                "team": {
                    "states": {"nodes": [{"id": f"s-{n}", "name": n} for n in names]}
                }
            }
        wanted = variables["filter"].get("number", {}).get("in")
        nodes = [i for n, i in self.issues.items() if wanted is None or n in wanted]
        return {"issues": {"pageInfo": {"hasNextPage": False}, "nodes": nodes}}


class RunTests(unittest.TestCase):
    def setUp(self):
        env = {"LINEAR_API_KEY": "key", "LINEAR_PROJECT_ID": "proj"}
        patcher = mock.patch.dict(os.environ, env)
        patcher.start()
        self.addCleanup(patcher.stop)

    def _run(self, fake, argv):
        out = io.StringIO()
        with mock.patch("board_batch._post", fake), redirect_stdout(out):
            code = rs.run(argv)
        return code, out.getvalue()

    def test_approve_writes_nothing_while_a_thread_is_open(self):
        thread = {"id": "t1", "parentId": None, "resolvedAt": None, "body": "fix"}
        fake = FakeLinear([_issue(1), _issue(2)], comments={"ENG-2": [thread]})
        with self.assertRaisesRegex(RatifyError, "still held by an open thread: ENG-2"):
            self._run(fake, ["approve", "--ids", "ENG-1", "ENG-2"])
        self.assertEqual(fake.writes, [])

    def test_a_partial_comment_read_refuses_rather_than_approves(self):
        fake = FakeLinear([_issue(1)], more_comments={"ENG-1"})
        with self.assertRaisesRegex(RatifyError, "partial read"):
            self._run(fake, ["approve", "--ids", "ENG-1"])
        self.assertEqual(fake.writes, [])

    def test_dry_run_writes_nothing(self):
        fake = FakeLinear([_issue(1)])
        code, out = self._run(fake, ["approve", "--ids", "ENG-1", "--dry-run"])
        self.assertEqual(code, 0)
        self.assertEqual(fake.writes, [])
        self.assertIn("WOULD SET ENG-1", out)

    def test_approve_moves_the_whole_set_in_one_batch(self):
        fake = FakeLinear([_issue(1), _issue(2), _issue(3)])
        code, _ = self._run(
            fake, ["approve", "--ids", "ENG-1,ENG-2", "--cancel", "ENG-3"]
        )
        self.assertEqual(code, 0)
        self.assertEqual(
            fake.writes,
            [
                ("uuid-1", {"projectMilestoneId": None, "stateId": "s-Todo"}),
                ("uuid-2", {"projectMilestoneId": None, "stateId": "s-Todo"}),
                ("uuid-3", {"projectMilestoneId": None, "stateId": "s-Canceled"}),
            ],
        )

    def test_review_reports_held_threads_and_approved_ticks(self):
        thread = {
            "id": "t1",
            "parentId": None,
            "resolvedAt": None,
            "body": "Split part 2",
            "quotedText": "Part 2",
            "user": {"name": "Operator"},
        }
        fake = FakeLinear([_issue(1), _issue(2)], comments={"ENG-2": [thread]})
        _, out = self._run(fake, ["review", "--ids", "ENG-1", "ENG-2"])
        self.assertIn("APPROVED ENG-1", out)
        self.assertIn("HELD ENG-2 | 1 open thread(s)", out)
        self.assertIn("thread t1 (Operator)", out)
        self.assertIn("on: Part 2", out)

    def test_the_gate_fails_while_a_task_is_still_staged(self):
        fake = FakeLinear([_issue(1, milestone=None), _issue(2)])
        code, out = self._run(fake, ["gate", "--ids", "ENG-1", "ENG-2"])
        self.assertEqual(code, 1)
        self.assertIn("ENG-2", out)
        fake = FakeLinear(
            [
                _issue(1, milestone=None),
                _issue(2, state_type="canceled"),
                _issue(3, state_type="duplicate"),
            ]
        )
        code, out = self._run(fake, ["gate", "--ids", "ENG-1", "ENG-2", "ENG-3"])
        self.assertEqual(code, 0)
        self.assertIn("CLEAR", out)


if __name__ == "__main__":
    unittest.main()
