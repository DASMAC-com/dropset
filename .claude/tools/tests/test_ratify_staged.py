#!/usr/bin/env python3
"""Unit tests for ratify_staged.py.

Every network call goes through ``board_batch._post``, so the end-to-end tests
patch that one seam and answer by the operation the tool sent. Nothing here
touches Linear.
"""

from __future__ import annotations

import io
import json
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


def _inbox(number, title=None, priority=0, **kwargs):
    issue = _issue(number, milestone=None, **kwargs)
    issue["priority"] = priority
    if title:
        issue["title"] = title
    return issue


class InboxPoolTests(unittest.TestCase):
    def test_membership_differs_by_pool(self):
        staged, inbox = _issue(1), _inbox(2)
        todo = _inbox(3, state_type="unstarted", state="Todo")
        parked = _issue(4, milestone="Follow-ups")
        closed = _inbox(5, state_type="canceled", state="Canceled")
        self.assertTrue(rs.in_pool(staged, "staged"))
        self.assertFalse(rs.in_pool(staged, "inbox"))
        self.assertTrue(rs.in_pool(inbox, "inbox"))
        self.assertFalse(rs.in_pool(inbox, "staged"))
        for outside in (todo, parked, closed):
            self.assertFalse(rs.in_pool(outside, "inbox"), outside["identifier"])

    def test_notes_become_the_checkbox_descriptions(self):
        payload = build_ask(
            [_inbox(1), _inbox(2)], notes={"ENG-1": "Carried from 10-26"}
        )
        (question,) = payload["calls"][0]["questions"]
        self.assertEqual(question["question"], rs.ASK_TEXT)
        self.assertEqual(question["header"], rs.ASK_HEADER)
        self.assertEqual(
            [o["description"] for o in question["options"]],
            ["Carried from 10-26", "Task 2"],
        )

    def test_a_note_for_an_id_outside_the_ask_is_refused(self):
        with self.assertRaisesRegex(RatifyError, "ENG-9"):
            rs.parse_notes({"eng-9": "typo"}, ["ENG-1"])
        self.assertEqual(rs.parse_notes({"eng-1": " ok "}, ["ENG-1"]), {"ENG-1": "ok"})

    def test_proposals_render_as_a_further_question(self):
        proposals = rs.parse_proposals(
            [
                {"edge": {"blocker": "eng-1", "blocked": "ENG-2"}, "reason": "API"},
                {"kill": "eng-3", "reason": "No recurring cost"},
                {"merge": ["ENG-12", "eng-9"], "reason": "Same fingerprint"},
            ]
        )
        (call,) = build_ask([_inbox(1), _inbox(2)], proposals=proposals)["calls"]
        review, extra = call["questions"]
        self.assertEqual(review["header"], rs.ASK_HEADER)
        self.assertEqual(extra["header"], rs.PROPOSALS_HEADER)
        self.assertTrue(extra["multiSelect"])
        self.assertEqual(
            extra["options"],
            [
                {"label": "ENG-1 blocks ENG-2", "description": "API"},
                {"label": "Cancel ENG-3", "description": "No recurring cost"},
                # Numeric, not lexical: ENG-9 survives ENG-12.
                {"label": "Merge ENG-12 into ENG-9", "description": "Same fingerprint"},
            ],
        )

    def test_a_lone_proposal_is_padded_and_a_bad_one_refused(self):
        (option,) = rs.parse_proposals([{"kill": "ENG-3", "reason": "Superseded"}])
        (call,) = build_ask([_inbox(1), _inbox(2)], proposals=[option])["calls"]
        labels = [o["label"] for o in call["questions"][1]["options"]]
        self.assertEqual(labels, ["Cancel ENG-3", "None"])
        with self.assertRaisesRegex(RatifyError, "reason"):
            rs.parse_proposals([{"edge": {"blocker": "ENG-1", "blocked": "ENG-2"}}])
        with self.assertRaisesRegex(RatifyError, "neither an edge"):
            rs.parse_proposals([{"relates": "ENG-1", "reason": "x"}])
        with self.assertRaisesRegex(RatifyError, "two distinct"):
            rs.parse_proposals([{"merge": ["ENG-4", "eng-4"], "reason": "x"}])

    def test_the_curator_cannot_propose_a_priority(self):
        with self.assertRaisesRegex(RatifyError, "neither an edge"):
            rs.parse_proposals(
                [{"priority": {"id": "ENG-3", "to": "High"}, "reason": "Demo"}]
            )

    def test_the_inbox_approves_one_task_at_a_time(self):
        with self.assertRaisesRegex(RatifyError, "one task at a time, got 2"):
            plan_approval([_inbox(1), _inbox(2)], [], held=[], pool="inbox")
        updates, already = plan_approval(
            [_inbox(1)], [_inbox(2), _inbox(3)], held=[], pool="inbox"
        )
        self.assertEqual(
            updates,
            {
                "ENG-1": {"state": "Todo"},
                "ENG-2": {"state": "Canceled"},
                "ENG-3": {"state": "Canceled"},
            },
        )
        self.assertEqual(already, [])
        with self.assertRaisesRegex(RatifyError, "not open in Backlog"):
            plan_approval([_issue(4)], [], held=[], pool="inbox")


class InboxRunTests(unittest.TestCase):
    # Borrowed, not inherited, so RunTests' own tests do not run twice.
    setUp = RunTests.setUp
    _run = RunTests._run

    def test_list_ranks_by_priority_and_marks_operator_items(self):
        fake = FakeLinear(
            [
                _inbox(1, priority=0),
                _inbox(2, priority=3),
                _inbox(3, title="[Operator] Rotate the key", priority=1),
                _inbox(4, state_type="unstarted", state="Todo", priority=1),
            ]
        )
        code, out = self._run(fake, ["list", "--pool", "inbox"])
        self.assertEqual(code, 0)
        lines = out.splitlines()
        self.assertEqual(
            [line.split(" | ")[0] for line in lines], ["ENG-3", "ENG-2", "ENG-1"]
        )
        self.assertTrue(lines[0].endswith("[operator]"))
        self.assertFalse(lines[1].endswith("[operator]"))

    def test_ask_skips_and_names_out_of_pool_ids(self):
        fake = FakeLinear([_inbox(1), _inbox(2), _issue(3), _inbox(4, state="Todo")])
        code, out = self._run(
            fake,
            ["ask", "--pool", "inbox", "--ids", "ENG-1,ENG-2,ENG-3,ENG-4"]
            + ["--notes", '{"ENG-2": "Operator item, hand-run"}'],
        )
        self.assertEqual(code, 0)
        payload = json.loads(out)
        self.assertEqual(payload["skipped"], ["ENG-3", "ENG-4"])
        options = payload["calls"][0]["questions"][0]["options"]
        self.assertEqual(
            [(o["label"], o["description"]) for o in options],
            [("ENG-1", "Task 1"), ("ENG-2", "Operator item, hand-run")],
        )

    def test_approve_refuses_a_second_id_and_writes_nothing(self):
        fake = FakeLinear([_inbox(1), _inbox(2)])
        with self.assertRaisesRegex(RatifyError, "one task at a time"):
            self._run(fake, ["approve", "--pool", "inbox", "--ids", "ENG-1,ENG-2"])
        self.assertEqual(fake.writes, [])

    def test_approve_refuses_a_held_or_out_of_pool_id(self):
        thread = {"id": "t1", "parentId": None, "resolvedAt": None, "body": "no"}
        fake = FakeLinear([_inbox(1), _issue(2)], comments={"ENG-1": [thread]})
        with self.assertRaisesRegex(RatifyError, "still held by an open thread: ENG-1"):
            self._run(fake, ["approve", "--pool", "inbox", "--ids", "ENG-1"])
        with self.assertRaisesRegex(RatifyError, "not open in Backlog.*ENG-2"):
            self._run(fake, ["approve", "--pool", "inbox", "--ids", "ENG-2"])
        self.assertEqual(fake.writes, [])

    def test_approve_moves_one_to_todo_and_cancels_the_rejects(self):
        moved = _inbox(3, state_type="unstarted", state="Todo")
        fake = FakeLinear([_inbox(1), _inbox(2), moved])
        code, out = self._run(
            fake, ["approve", "--pool", "inbox", "--ids", "ENG-1", "--cancel", "ENG-2"]
        )
        self.assertEqual(code, 0)
        self.assertEqual(
            fake.writes,
            [("uuid-1", {"stateId": "s-Todo"}), ("uuid-2", {"stateId": "s-Canceled"})],
        )
        code, out = self._run(fake, ["approve", "--pool", "inbox", "--ids", "ENG-3"])
        self.assertEqual(code, 0)
        self.assertIn("ALREADY ENG-3", out)

    def test_review_reads_inbox_members(self):
        fake = FakeLinear([_inbox(1), _issue(2)])
        _, out = self._run(fake, ["review", "--pool", "inbox", "--ids", "ENG-1,ENG-2"])
        self.assertIn("APPROVED ENG-1", out)
        self.assertIn("NOT IN INBOX ENG-2", out)

    def test_the_gate_is_refused_for_the_inbox(self):
        fake = FakeLinear([_inbox(1)])
        with self.assertRaisesRegex(RatifyError, "nothing is staged in the inbox"):
            self._run(fake, ["gate", "--pool", "inbox", "--ids", "ENG-1"])

    def test_inbox_rejects_are_pool_checked_and_safe_to_repeat(self):
        todo = _inbox(2, state_type="unstarted", state="Todo")
        canceled = _inbox(3, state_type="canceled", state="Canceled")
        fake = FakeLinear([_inbox(1), todo, canceled])
        with self.assertRaisesRegex(RatifyError, "not open in Backlog.*ENG-2"):
            self._run(fake, ["approve", "--pool", "inbox", "--cancel", "ENG-2"])
        self.assertEqual(fake.writes, [])
        code, out = self._run(
            fake, ["approve", "--pool", "inbox", "--cancel", "ENG-1", "ENG-3"]
        )
        self.assertEqual(code, 0)
        self.assertIn("ALREADY ENG-3", out)
        self.assertEqual(fake.writes, [("uuid-1", {"stateId": "s-Canceled"})])

    def test_a_note_for_a_skipped_id_does_not_refuse_the_ask(self):
        fake = FakeLinear([_inbox(1), _issue(2)])
        code, out = self._run(
            fake,
            ["ask", "--pool", "inbox", "--ids", "ENG-1,ENG-2"]
            + ["--notes", '{"ENG-1": "Keep", "ENG-2": "Moved since"}'],
        )
        self.assertEqual(code, 0)
        payload = json.loads(out)
        self.assertEqual(payload["skipped"], ["ENG-2"])
        options = payload["calls"][0]["questions"][0]["options"]
        self.assertEqual(options[0]["description"], "Keep")

    def test_repeated_proposals_are_refused(self):
        edge = {"edge": {"blocker": "ENG-1", "blocked": "ENG-2"}, "reason": "a"}
        with self.assertRaisesRegex(RatifyError, "repeats: ENG-1 blocks ENG-2"):
            rs.parse_proposals([edge, {**edge, "reason": "b"}])


if __name__ == "__main__":
    unittest.main()
