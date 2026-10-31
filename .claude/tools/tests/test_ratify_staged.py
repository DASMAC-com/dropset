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

import board_batch  # noqa: E402
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

    def test_checkboxes_run_by_issue_number_whatever_the_caller_passed(self):
        # Across both questions, not within each: the operator reads the
        # dialog top to bottom against the batch's tabs left to right.
        issues = [_issue(n) for n in (40, 7, 112, 9, 31)]
        issues[0]["title"] = "(carried) folded into Part 2"
        (call,) = build_ask(issues)["calls"]
        options = [o for q in call["questions"] for o in q["options"]]
        self.assertEqual(
            [o["label"] for o in options],
            ["ENG-7", "ENG-9", "ENG-31", "ENG-40", "ENG-112"],
        )
        # The carried task's description (here its title) rides with it to
        # its sorted place.
        self.assertIn("(carried)", options[3]["description"])

    def test_the_sort_reads_the_identifier_when_number_is_absent(self):
        issues = [_issue(n) for n in (20, 3)]
        for issue in issues:
            del issue["number"]
        (call,) = build_ask(issues)["calls"]
        labels = [o["label"] for o in call["questions"][0]["options"]]
        self.assertEqual(labels, ["ENG-3", "ENG-20"])

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

    def __init__(self, issues, comments=None, more_comments=(), parents=None):
        self.issues = {i["number"]: i for i in issues}
        self.comments = comments or {}
        self.more_comments = set(more_comments)
        # Parent identifier -> the numbers of its sub-issues.
        self.parents = parents or {}
        self.writes = []

    def __call__(self, api_key, query, variables):
        if query == rs._PARENT_QUERY:
            ident = variables["id"]
            known = ident in self.parents
            return {
                "issue": {"id": f"uuid-{ident}", "identifier": ident} if known else None
            }
        parent = (variables.get("filter") or {}).get("parent")
        if parent:
            ident = parent["id"]["eq"].removeprefix("uuid-")
            nodes = [self.issues[n] for n in self.parents[ident]]
            return {"issues": {"pageInfo": {"hasNextPage": False}, "nodes": nodes}}
        if "issueUpdate" in query:
            self.writes.append((variables["id"], variables["input"]))
            return {"issueUpdate": {"success": True}}
        if "comments(" in query:
            ident = variables["id"]
            page = {"hasNextPage": ident in self.more_comments}
            nodes = self.comments.get(ident, [])
            return {"issue": {"comments": {"pageInfo": page, "nodes": nodes}}}
        if "projectMilestones" in query:
            names = (rs.MILESTONE, rs.CURATOR_MILESTONE)
            nodes = [{"id": f"m-{n.split()[0]}", "name": n} for n in names]
            return {"project": {"projectMilestones": {"nodes": nodes}}}
        if "TeamStates" in query:
            names = ("Backlog", "Todo", "Done", "Canceled")
            return {
                "team": {
                    "states": {"nodes": [{"id": f"s-{n}", "name": n} for n in names]}
                }
            }
        wanted = variables["filter"].get("number", {}).get("in")
        nodes = [i for n, i in self.issues.items() if wanted is None or n in wanted]
        return {"issues": {"pageInfo": {"hasNextPage": False}, "nodes": nodes}}


class _RunBase(unittest.TestCase):
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


class RunTests(_RunBase):
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


class ParentRunTests(_RunBase):
    """``--parent`` names the set a seat session filed as sub-issues, so a
    session that died mid-review resumes from its governing issue alone."""

    def _set(self):
        issues = [
            _issue(1, milestone=None, state="Todo", state_type="unstarted"),
            _issue(2),
            _issue(3),
        ]
        return FakeLinear(issues, parents={"ENG-9": [1, 2, 3]})

    def test_the_gate_resolves_the_same_set_as_the_ids(self):
        for argv in (["--parent", "eng-9"], ["--ids", "ENG-1", "ENG-2", "ENG-3"]):
            with self.subTest(argv=argv):
                code, out = self._run(self._set(), ["gate", *argv])
                self.assertEqual(code, 1)
                self.assertEqual(
                    out.strip(), f"OPEN — still under {rs.MILESTONE}: ENG-2, ENG-3"
                )

    def test_list_narrows_to_the_staged_sub_issues(self):
        _, out = self._run(self._set(), ["list", "--parent", "ENG-9"])
        self.assertNotIn("ENG-1", out)
        self.assertIn("ENG-2", out)
        self.assertIn("ENG-3", out)

    def test_review_takes_only_the_sub_issues_still_staged(self):
        _, out = self._run(self._set(), ["review", "--parent", "ENG-9"])
        self.assertEqual(
            out.splitlines(), ["APPROVED ENG-2 | Task 2", "APPROVED ENG-3 | Task 3"]
        )

    def test_approve_moves_every_staged_sub_issue_not_canceled(self):
        fake = self._set()
        code, _ = self._run(fake, ["approve", "--parent", "ENG-9", "--cancel", "ENG-3"])
        self.assertEqual(code, 0)
        self.assertEqual(
            fake.writes,
            [
                ("uuid-2", {"projectMilestoneId": None, "stateId": "s-Todo"}),
                ("uuid-3", {"projectMilestoneId": None, "stateId": "s-Canceled"}),
            ],
        )

    def test_approve_re_runs_cleanly_once_every_child_has_moved(self):
        todo = dict(milestone=None, state="Todo", state_type="unstarted")
        fake = FakeLinear(
            [_issue(1, **todo), _issue(2, **todo)], parents={"ENG-9": [1, 2]}
        )
        code, out = self._run(fake, ["approve", "--parent", "ENG-9"])
        self.assertEqual(code, 0)
        self.assertEqual(fake.writes, [])
        self.assertIn("ALREADY ENG-1", out)
        self.assertIn("ALREADY ENG-2", out)

    def test_approve_names_a_child_that_is_neither_staged_nor_moved(self):
        fake = FakeLinear(
            [_issue(2), _issue(4, milestone=None)], parents={"ENG-9": [2, 4]}
        )
        code, out = self._run(fake, ["approve", "--parent", "ENG-9"])
        self.assertEqual(code, 0)
        self.assertIn("SKIPPED ENG-4", out)
        self.assertEqual([w[0] for w in fake.writes], ["uuid-2"])

    def test_approve_refuses_a_cancel_outside_the_set(self):
        fake = self._set()
        with self.assertRaisesRegex(RatifyError, "not sub-issues of ENG-9: ENG-7"):
            self._run(fake, ["approve", "--parent", "ENG-9", "--cancel", "ENG-7"])
        self.assertEqual(fake.writes, [])

    def test_ask_with_neither_ids_parent_nor_operator_is_refused(self):
        with self.assertRaisesRegex(RatifyError, "nothing to ask"):
            self._run(self._set(), ["ask"])

    def test_ask_refuses_an_operator_item_beside_a_parent(self):
        # --operator rides the inbox; --parent names a staged set.
        with self.assertRaisesRegex(RatifyError, "--pool inbox"):
            self._run(self._set(), ["ask", "--parent", "ENG-9", "--operator", "ENG-5"])

    def test_approve_refuses_a_title_beside_a_parent(self):
        fake = self._set()
        with self.assertRaisesRegex(RatifyError, "--title retitles"):
            self._run(fake, ["approve", "--parent", "ENG-9", "--title", "New"])
        self.assertEqual(fake.writes, [])

    def test_an_empty_parent_is_refused_not_read_as_no_ids(self):
        with self.assertRaisesRegex(RatifyError, "wants an ENG-### identifier"):
            self._run(self._set(), ["gate", "--parent", ""])

    def test_ask_builds_from_the_staged_sub_issues(self):
        _, out = self._run(self._set(), ["ask", "--parent", "ENG-9"])
        payload = json.loads(out)
        self.assertEqual(payload["skipped"], ["ENG-1"])
        self.assertIn("ENG-2", out)
        self.assertIn("ENG-3", out)

    def test_review_falls_back_to_the_whole_set_once_none_is_staged(self):
        todo = dict(milestone=None, state="Todo", state_type="unstarted")
        fake = FakeLinear([_issue(1, **todo)], parents={"ENG-9": [1]})
        _, out = self._run(fake, ["review", "--parent", "ENG-9"])
        self.assertEqual(out.strip(), "NOT STAGED ENG-1 | Task 1")

    def test_a_clear_gate_names_the_set_it_resolved(self):
        fake = FakeLinear(
            [_issue(1, milestone=None), _issue(2, state_type="canceled")],
            parents={"ENG-9": [1, 2]},
        )
        code, out = self._run(fake, ["gate", "--parent", "ENG-9"])
        self.assertEqual(code, 0)
        self.assertIn("(ENG-1, ENG-2)", out)

    def test_an_unknown_or_childless_parent_is_refused(self):
        with self.assertRaisesRegex(RatifyError, "no such issue"):
            self._run(self._set(), ["gate", "--parent", "ENG-8"])
        fake = FakeLinear([_issue(1)], parents={"ENG-9": []})
        with self.assertRaisesRegex(RatifyError, "no sub-issues"):
            self._run(fake, ["gate", "--parent", "ENG-9"])

    def test_parent_and_ids_are_exclusive_and_inbox_refuses_parent(self):
        with self.assertRaises(SystemExit), mock.patch("sys.stderr"):
            rs._parse_args(["gate", "--parent", "ENG-9", "--ids", "ENG-1"])
        with self.assertRaisesRegex(RatifyError, "inbox"):
            self._run(self._set(), ["review", "--pool", "inbox", "--parent", "ENG-9"])

    def test_every_pool_but_staged_refuses_parent_before_any_read(self):
        """On the operator pool an approve-all-children sweep would move an
        operator item to Done without its id ever being named."""
        for cmd in ("list", "review", "approve"):
            with self.subTest(cmd=cmd):
                fake = self._set()
                with mock.patch.object(rs, "fetch_children") as fetched:
                    with self.assertRaisesRegex(RatifyError, "the operator pool"):
                        self._run(
                            fake, [cmd, "--pool", "operator", "--parent", "ENG-9"]
                        )
                fetched.assert_not_called()
                self.assertEqual(fake.writes, [])

    def test_chore_refuses_a_parent_beside_it(self):
        with self.assertRaisesRegex(RatifyError, "--chore is its own write"):
            self._run(self._set(), ["approve", "--chore", "ENG-5", "--parent", "ENG-9"])


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
            [_inbox(1, priority=2), _inbox(2)], notes={"ENG-1": "Carried from 10-26"}
        )
        (question,) = payload["calls"][0]["questions"]
        self.assertEqual(question["question"], rs.ASK_TEXT)
        self.assertEqual(question["header"], rs.ASK_HEADER)
        self.assertEqual(
            [o["description"] for o in question["options"]],
            ["High · Carried from 10-26", "No priority · Task 2"],
        )

    def test_every_checkbox_leads_with_the_filed_priority(self):
        issues = [_issue(n) for n in (1, 2, 3, 4)]
        # Linear's GraphQL types priority as a Float, so 2.0 must read as High.
        for issue, priority in zip(issues, (1, 2.0, 3, 4)):
            issue["priority"] = priority
        (call,) = build_ask(issues)["calls"]
        (question,) = call["questions"]
        self.assertEqual(
            [o["description"] for o in question["options"]],
            [
                "Urgent · Task 1",
                "High · Task 2",
                "Medium · Task 3",
                "Low · Task 4",
            ],
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

    def test_hand_off_ids_are_normalized_and_never_contested(self):
        (option,) = rs.parse_proposals(
            [{"merge": ["ENG-09", "ENG-12"], "reason": "Same fingerprint"}]
        )
        self.assertEqual(option["label"], "Merge ENG-12 into ENG-9")
        with self.assertRaisesRegex(RatifyError, "two distinct"):
            rs.parse_proposals([{"merge": ["ENG-09", "ENG-9"], "reason": "x"}])
        with self.assertRaisesRegex(RatifyError, "not ENG-###"):
            rs.parse_proposals([{"kill": "oops", "reason": "x"}])
        with self.assertRaisesRegex(RatifyError, "not ENG-###"):
            rs.parse_proposals([{"merge": ["ENG-1", "ABC-2"], "reason": "x"}])
        with self.assertRaisesRegex(RatifyError, "mixes kill and merge"):
            rs.parse_proposals(
                [{"kill": "ENG-3", "merge": ["ENG-3", "ENG-4"], "reason": "x"}]
            )
        # A kill and a merge of one issue cannot both execute.
        with self.assertRaisesRegex(RatifyError, "two kills or merges: ENG-3"):
            rs.parse_proposals(
                [
                    {"kill": "ENG-3", "reason": "Superseded"},
                    {"merge": ["ENG-3", "ENG-5"], "reason": "Same fingerprint"},
                ]
            )

    def test_a_priority_change_is_a_proposal_kind(self):
        options = rs.parse_proposals(
            [
                {"priority": {"id": "eng-03", "to": "high"}, "reason": "Demo"},
                {"priority": {"id": "ENG-4", "to": 4}, "reason": "Nice to have"},
            ]
        )
        self.assertEqual(
            options,
            [
                {"label": "ENG-3 to High", "description": "Demo"},
                {"label": "ENG-4 to Low", "description": "Nice to have"},
            ],
        )
        with self.assertRaisesRegex(RatifyError, "unrecognized"):
            rs.parse_proposals(
                [{"priority": {"id": "ENG-3", "to": "soon"}, "reason": "x"}]
            )
        with self.assertRaisesRegex(RatifyError, "target 'to'"):
            rs.parse_proposals([{"priority": {"id": "ENG-3"}, "reason": "x"}])
        with self.assertRaisesRegex(RatifyError, "mixes edge and priority"):
            rs.parse_proposals(
                [
                    {
                        "edge": {"blocker": "ENG-1", "blocked": "ENG-2"},
                        "priority": {"id": "ENG-3", "to": "High"},
                        "reason": "x",
                    }
                ]
            )
        # Two targets for one issue: the ticks could not both stand.
        with self.assertRaisesRegex(RatifyError, "re-ranks an issue twice: ENG-3"):
            rs.parse_proposals(
                [
                    {"priority": {"id": "ENG-3", "to": "High"}, "reason": "x"},
                    {"priority": {"id": "ENG-03", "to": "Low"}, "reason": "y"},
                ]
            )

    def test_approval_never_writes_a_priority(self):
        for pool, issue in (("staged", _issue(1)), ("inbox", _inbox(1, priority=2))):
            updates, _ = plan_approval([issue], [], held=[], pool=pool)
            self.assertNotIn("priority", updates["ENG-1"], pool)

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
            [
                ("ENG-1", "No priority · Task 1"),
                ("ENG-2", "No priority · Operator item, hand-run"),
            ],
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
        self.assertEqual(options[0]["description"], "No priority · Keep")

    def test_a_curator_pending_member_is_still_in_the_inbox(self):
        marked = _issue(1, milestone=rs.CURATOR_MILESTONE)
        self.assertTrue(rs.in_pool(marked, "inbox"))
        self.assertFalse(rs.in_pool(marked, "staged"))
        fake = FakeLinear([marked, _inbox(2)])
        _, out = self._run(fake, ["list", "--pool", "inbox"])
        lines = out.splitlines()
        self.assertEqual([line.split(" | ")[0] for line in lines], ["ENG-1", "ENG-2"])
        self.assertTrue(lines[0].endswith("[curator pending]"))

    def test_the_ticked_move_clears_the_mark_and_only_it(self):
        fake = FakeLinear(
            [
                _issue(1, milestone=rs.CURATOR_MILESTONE),
                _issue(2, milestone=rs.CURATOR_MILESTONE),
                _inbox(3),
            ]
        )
        code, _ = self._run(
            fake,
            ["approve", "--pool", "inbox", "--ids", "ENG-1"]
            + ["--cancel", "ENG-2", "ENG-3"],
        )
        self.assertEqual(code, 0)
        self.assertEqual(
            fake.writes,
            [
                ("uuid-1", {"projectMilestoneId": None, "stateId": "s-Todo"}),
                ("uuid-2", {"projectMilestoneId": None, "stateId": "s-Canceled"}),
                ("uuid-3", {"stateId": "s-Canceled"}),
            ],
        )

    def test_a_ticked_retitle_lands_in_the_same_write(self):
        marked = _issue(1, milestone=rs.CURATOR_MILESTONE)
        marked["title"] = "Claude: Old shorthand"
        fake = FakeLinear([marked])
        code, _ = self._run(
            fake,
            ["approve", "--pool", "inbox", "--ids", "ENG-1"]
            + ["--title", "Claude: The subject the body now has"],
        )
        self.assertEqual(code, 0)
        self.assertEqual(
            fake.writes,
            [
                (
                    "uuid-1",
                    {
                        "projectMilestoneId": None,
                        "stateId": "s-Todo",
                        "title": "Claude: The subject the body now has",
                    },
                )
            ],
        )

    def test_a_retitle_that_changes_the_token_writes_nothing(self):
        issue = _inbox(1, title="[Chore/Anthropic] Old")
        fake = FakeLinear([issue])
        with self.assertRaisesRegex(board_batch.BoardBatchError, "--retoken"):
            self._run(
                fake,
                ["approve", "--pool", "inbox", "--ids", "ENG-1", "--title", "New"],
            )
        self.assertEqual(fake.writes, [])
        with self.assertRaisesRegex(RatifyError, "--title retitles"):
            self._run(fake, ["approve", "--ids", "ENG-1", "--title", "New"])

    def test_a_retitle_on_an_already_moved_task_is_named_not_dropped(self):
        moved = _inbox(1, state_type="unstarted", state="Todo")
        fake = FakeLinear([moved])
        code, out = self._run(
            fake, ["approve", "--pool", "inbox", "--ids", "ENG-1", "--title", "New"]
        )
        self.assertEqual(code, 0)
        self.assertIn("ALREADY ENG-1", out)
        self.assertIn("TITLE NOT APPLIED ENG-1", out)
        self.assertEqual(fake.writes, [])

    def test_unmark_ids_clears_a_marked_task(self):
        fake = FakeLinear([_issue(1, milestone=rs.CURATOR_MILESTONE), _inbox(2)])
        code, _ = self._run(fake, ["unmark", "--ids", "ENG-1,ENG-2"])
        self.assertEqual(code, 0)
        self.assertEqual(fake.writes, [("uuid-1", {"projectMilestoneId": None})])

    def test_mark_stamps_inbox_members_and_skips_the_rest(self):
        marked = _issue(2, milestone=rs.CURATOR_MILESTONE)
        fake = FakeLinear([_inbox(1), marked, _issue(3)])
        code, out = self._run(fake, ["mark", "--ids", "ENG-1,ENG-2,ENG-3"])
        self.assertEqual(code, 0)
        self.assertIn("SKIPPED ENG-3", out)
        # ENG-2 already carries the mark, so it needs no write.
        self.assertEqual(fake.writes, [("uuid-1", {"projectMilestoneId": "m-Curator"})])

    def test_unmark_all_clears_only_the_curator_mark(self):
        fake = FakeLinear(
            [
                _issue(1, milestone=rs.CURATOR_MILESTONE),
                _issue(2),
                _inbox(3),
            ]
        )
        code, _ = self._run(fake, ["unmark", "--all"])
        self.assertEqual(code, 0)
        self.assertEqual(fake.writes, [("uuid-1", {"projectMilestoneId": None})])
        fake = FakeLinear([_inbox(1)])
        code, out = self._run(fake, ["unmark", "--ids", "ENG-1", "--dry-run"])
        self.assertIn("NOTHING to write", out)
        self.assertEqual(fake.writes, [])

    def test_mark_refuses_an_ambiguous_milestone_name(self):
        with self.assertRaisesRegex(RatifyError, "2 milestones"):
            with mock.patch(
                "board_batch._post",
                return_value={
                    "project": {
                        "projectMilestones": {
                            "nodes": [
                                {"id": "a", "name": rs.CURATOR_MILESTONE},
                                {"id": "b", "name": rs.CURATOR_MILESTONE},
                            ]
                        }
                    }
                },
            ):
                rs.milestone_id("k", "p", rs.CURATOR_MILESTONE)

    def test_repeated_proposals_are_refused(self):
        edge = {"edge": {"blocker": "ENG-1", "blocked": "ENG-2"}, "reason": "a"}
        with self.assertRaisesRegex(RatifyError, "repeats: ENG-1 blocks ENG-2"):
            rs.parse_proposals([edge, {**edge, "reason": "b"}])


def _operator(number, title=None, priority=1, state_type="unstarted", state="Todo"):
    issue = _issue(number, milestone=None, state_type=state_type, state=state)
    issue["priority"] = priority
    issue["title"] = title or f"[Operator] Hand step {number}"
    return issue


def _deferral(day, ident="d"):
    return {
        "id": ident,
        "parentId": "t",
        "resolvedAt": None,
        "body": f"Deferred {day}: later",
    }


class OperatorPoolTests(unittest.TestCase):
    def test_membership_is_a_todo_operator_item_never_a_chore(self):
        self.assertTrue(rs.in_pool(_operator(1), "operator"))
        self.assertFalse(rs.in_pool(_inbox(2, title="[Operator] Backlog"), "operator"))
        self.assertFalse(
            rs.in_pool(_operator(3, title="[Chore/Bedrock] Was operator"), "operator")
        )
        self.assertFalse(
            rs.in_pool(_operator(4, title="Mentions [Operator] later"), "operator")
        )
        parked = _operator(5)
        parked["projectMilestone"] = {"name": rs.MILESTONE}
        self.assertFalse(rs.in_pool(parked, "operator"))
        self.assertFalse(rs.in_pool(_operator(6, state_type="completed"), "operator"))
        # An [Operator] item that already carries a chore token is the
        # scheduler's, never the fifth box's.
        self.assertFalse(
            rs.in_pool(_operator(7, title="[Operator] [Chore/Bedrock] X"), "operator")
        )

    def test_the_chore_title_swaps_only_the_operator_token(self):
        self.assertEqual(
            rs.chore_title(_operator(1, title="[Operator] Claude: Rotate the key")),
            "[Chore/Bedrock] Claude: Rotate the key",
        )
        with self.assertRaisesRegex(RatifyError, "no leading \\[Operator\\]"):
            rs.chore_title(_operator(2, title="Rotate the [Operator] key"))
        # A swap beside another launch token would leave two launch lines.
        with self.assertRaisesRegex(RatifyError, "already carries"):
            rs.chore_title(_operator(3, title="[Operator][Explore/Anthropic] X"))

    def test_the_deferral_history_and_the_third_deferral_recommendation(self):
        today = rs.datetime.date(2026, 10, 30)
        comments = [
            _deferral("2026-10-29"),
            {"id": "x", "body": "Not a deferral"},
            _deferral("2026-13-40"),
            _deferral("2026-10-27"),
        ]
        dates = rs.deferrals(comments)
        self.assertEqual([d.isoformat() for d in dates], ["2026-10-27", "2026-10-29"])
        self.assertEqual(rs.operator_note("Step", [], today), "Step")
        note = rs.operator_note("Step", dates, today)
        self.assertIn("deferred 2 times since 2026-10-27 (waited 3 days)", note)
        self.assertNotIn("recommend cancel", note)
        one_day = rs.operator_note("Step", dates[1:], today)
        self.assertIn("deferred 1 time since 2026-10-29 (waited 1 day)", one_day)
        dates.append(rs.datetime.date(2026, 10, 30))
        self.assertIn("recommend cancel", rs.operator_note("Step", dates, today))

    def test_the_fifth_box_is_its_own_question_after_the_four(self):
        operator = {"label": "ENG-9", "description": "Do it"}
        ask = build_ask([_inbox(n) for n in (1, 2, 3, 4)], operator=operator)
        questions = ask["calls"][0]["questions"]
        self.assertEqual([q["header"] for q in questions], ["Reviewed", "Operator"])
        self.assertEqual(len(questions[0]["options"]), 4)
        self.assertEqual(
            [o["label"] for o in questions[1]["options"]], ["ENG-9", "Not today"]
        )
        alone = build_ask([], operator=operator)["calls"][0]["questions"]
        self.assertEqual([q["header"] for q in alone], ["Operator"])


class OperatorRunTests(unittest.TestCase):
    setUp = RunTests.setUp
    _run = RunTests._run

    def test_list_is_urgent_first_then_oldest(self):
        fake = FakeLinear(
            [
                _operator(7, priority=2),
                _operator(5, priority=1),
                _operator(3, priority=2),
                _operator(4, title="[Chore/Bedrock] Handed over"),
                _inbox(2, title="[Operator] Not yet ticked"),
            ]
        )
        code, out = self._run(fake, ["list", "--pool", "operator"])
        self.assertEqual(code, 0)
        self.assertEqual(
            [line.split(" | ")[0] for line in out.splitlines()],
            ["ENG-5", "ENG-3", "ENG-7"],
        )

    def test_ask_carries_the_operator_item_with_its_deferrals(self):
        fake = FakeLinear(
            [_inbox(1), _operator(9)],
            comments={"ENG-9": [_deferral("2026-10-01")]},
        )
        code, out = self._run(
            fake,
            ["ask", "--pool", "inbox", "--ids", "ENG-1", "--operator", "ENG-9"]
            + ["--notes", '{"ENG-9": "Rotate it in the dashboard"}'],
        )
        self.assertEqual(code, 0)
        questions = json.loads(out)["calls"][0]["questions"]
        self.assertEqual([q["header"] for q in questions], ["Reviewed", "Operator"])
        box = questions[1]["options"][0]
        self.assertEqual(box["label"], "ENG-9")
        self.assertTrue(box["description"].startswith("Rotate it in the dashboard"))
        self.assertIn("deferred 1 time since 2026-10-01", box["description"])

    def test_the_operator_items_only_ask(self):
        fake = FakeLinear([_operator(9)])
        code, out = self._run(fake, ["ask", "--pool", "inbox", "--operator", "ENG-9"])
        self.assertEqual(code, 0)
        questions = json.loads(out)["calls"][0]["questions"]
        self.assertEqual([q["header"] for q in questions], ["Operator"])
        with self.assertRaisesRegex(RatifyError, "nothing to ask"):
            self._run(fake, ["ask", "--pool", "inbox"])

    def test_ask_refuses_an_item_outside_the_operator_pool(self):
        fake = FakeLinear([_operator(9, title="[Chore/Bedrock] Handed over")])
        with self.assertRaisesRegex(RatifyError, "ENG-9 is not an \\[Operator\\]"):
            self._run(fake, ["ask", "--pool", "inbox", "--operator", "ENG-9"])
        with self.assertRaisesRegex(RatifyError, "rides --operator"):
            self._run(fake, ["ask", "--pool", "operator", "--ids", "ENG-9"])
        with self.assertRaisesRegex(RatifyError, "pass --pool inbox"):
            self._run(fake, ["ask", "--ids", "ENG-1", "--operator", "ENG-9"])
        two = FakeLinear([_operator(8), _operator(9)])
        with self.assertRaisesRegex(RatifyError, "the fifth box is one box"):
            self._run(two, ["ask", "--pool", "inbox", "--operator", "ENG-8,ENG-9"])

    def test_a_tick_writes_done_one_at_a_time(self):
        fake = FakeLinear([_operator(9), _operator(8)])
        code, _ = self._run(fake, ["approve", "--pool", "operator", "--ids", "ENG-9"])
        self.assertEqual(code, 0)
        self.assertEqual(fake.writes, [("uuid-9", {"stateId": "s-Done"})])
        with self.assertRaisesRegex(RatifyError, "one task at a time"):
            self._run(fake, ["approve", "--pool", "operator", "--ids", "ENG-9,ENG-8"])
        done = _operator(7, state_type="completed", state="Done")
        code, out = self._run(
            FakeLinear([done]), ["approve", "--pool", "operator", "--ids", "ENG-7"]
        )
        self.assertIn("ALREADY ENG-7", out)

    def test_a_held_operator_item_is_not_marked_done(self):
        thread = {"id": "t1", "parentId": None, "resolvedAt": None, "body": "later"}
        fake = FakeLinear([_operator(9)], comments={"ENG-9": [thread]})
        with self.assertRaisesRegex(RatifyError, "still held by an open thread"):
            self._run(fake, ["approve", "--pool", "operator", "--ids", "ENG-9"])
        self.assertEqual(fake.writes, [])
        _, out = self._run(fake, ["review", "--pool", "operator", "--ids", "ENG-9"])
        self.assertIn("HELD ENG-9", out)

    def test_a_drop_cancels_the_operator_item_even_while_held(self):
        thread = {"id": "t1", "parentId": None, "resolvedAt": None, "body": "drop"}
        fake = FakeLinear([_operator(9)], comments={"ENG-9": [thread]})
        code, _ = self._run(
            fake, ["approve", "--pool", "operator", "--cancel", "ENG-9"]
        )
        self.assertEqual(code, 0)
        self.assertEqual(fake.writes, [("uuid-9", {"stateId": "s-Canceled"})])

    def test_review_names_an_item_outside_the_operator_pool(self):
        fake = FakeLinear([_inbox(2)])
        _, out = self._run(fake, ["review", "--pool", "operator", "--ids", "ENG-2"])
        self.assertIn("NOT AN OPERATOR ITEM ENG-2", out)

    def test_attended_chore_swaps_the_token_and_leaves_it_in_todo(self):
        fake = FakeLinear([_operator(9, title="[Operator] Rotate the key")])
        code, _ = self._run(fake, ["approve", "--chore", "ENG-9"])
        self.assertEqual(code, 0)
        self.assertEqual(
            fake.writes, [("uuid-9", {"title": "[Chore/Bedrock] Rotate the key"})]
        )
        with self.assertRaisesRegex(RatifyError, "is its own write"):
            self._run(fake, ["approve", "--chore", "ENG-9", "--ids", "ENG-9"])
        with self.assertRaisesRegex(RatifyError, "one operator item at a time"):
            self._run(fake, ["approve", "--chore", "ENG-8,ENG-9"])
        with self.assertRaisesRegex(RatifyError, "is not an \\[Operator\\]"):
            self._run(FakeLinear([_inbox(3)]), ["approve", "--chore", "ENG-3"])

    def test_the_gate_is_refused_for_the_operator_pool(self):
        with self.assertRaisesRegex(RatifyError, "gate is for the staged pool"):
            self._run(FakeLinear([]), ["gate", "--pool", "operator", "--ids", "ENG-1"])


if __name__ == "__main__":
    unittest.main()
