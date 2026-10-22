#!/usr/bin/env python3
"""Unit tests for linear_gate.py.

The property carried hardest is the exit code: a dispatch wrapper refuses on
it, so an issue outside Todo must exit 1 and a failure must exit 2 — a failure
that read as "approved" would dispatch an unratified spec. An open thread is
reported, never refused.

Nothing here touches the network.
"""

from __future__ import annotations

import io
import os
import sys
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import linear_gate as lg  # noqa: E402

ENV = {"LINEAR_API_KEY": "k"}

OPEN = {"resolvedAt": None, "parent": None}
RESOLVED = {"resolvedAt": "2026-10-21T01:00:00.000Z", "parent": None}
REPLY = {"resolvedAt": None, "parent": {"id": "c-1"}}


def _issue(
    identifier: str, state: str = "Todo", comments: list | None = None, more=False
) -> dict:
    return {
        "issue": {
            "identifier": identifier,
            "state": {"name": state},
            "comments": {
                "nodes": comments or [],
                "pageInfo": {"hasNextPage": more},
            },
        }
    }


def _run(
    argv: list[str], posts: list, env: dict = ENV
) -> tuple[int, str, str, mock.Mock]:
    out, err = io.StringIO(), io.StringIO()
    with (
        mock.patch.dict(os.environ, env, clear=True),
        mock.patch.object(lg, "_post", side_effect=posts) as post,
        redirect_stdout(out),
        redirect_stderr(err),
    ):
        code = lg.run(argv)
    return code, out.getvalue(), err.getvalue(), post


class OpenThreadsTests(unittest.TestCase):
    def test_only_unresolved_top_level_comments_count(self):
        comments = {"nodes": [OPEN, RESOLVED, REPLY, OPEN], "pageInfo": {}}
        self.assertEqual(lg.open_threads(comments), "2")

    def test_a_truncated_page_is_a_lower_bound(self):
        comments = {"nodes": [OPEN], "pageInfo": {"hasNextPage": True}}
        self.assertEqual(lg.open_threads(comments), "1+")

    def test_malformed_comments_are_an_error(self):
        for payload in (None, {"nodes": None}, "x"):
            with self.subTest(payload=payload):
                with self.assertRaises(lg.LinearGateError):
                    lg.open_threads(payload)


class ApprovalsTests(unittest.TestCase):
    def test_a_todo_issue_with_no_open_thread_exits_zero(self):
        code, out, _, _ = _run(["approvals", "ENG-1"], [_issue("ENG-1", "Todo")])
        self.assertEqual(code, 0)
        self.assertEqual(out, "APPROVED ENG-1 open-threads=0 state=Todo\n")

    def test_an_open_thread_is_reported_never_refused(self):
        code, out, _, _ = _run(
            ["approvals", "ENG-1"], [_issue("ENG-1", "Todo", [OPEN, RESOLVED])]
        )
        self.assertEqual(code, 0)
        self.assertEqual(out, "APPROVED ENG-1 open-threads=1 state=Todo\n")

    def test_a_backlog_issue_exits_one(self):
        code, out, _, _ = _run(["approvals", "ENG-1"], [_issue("ENG-1", "Backlog")])
        self.assertEqual(code, lg.EXIT_UNAPPROVED)
        self.assertEqual(out, "UNAPPROVED ENG-1 open-threads=0 state=Backlog\n")

    def test_the_state_compares_case_insensitively(self):
        code, _, _, _ = _run(["approvals", "ENG-1"], [_issue("ENG-1", "todo")])
        self.assertEqual(code, 0)

    def test_one_unapproved_issue_exits_one_and_still_reports_all(self):
        code, out, _, _ = _run(
            ["approvals", "ENG-1", "ENG-2"],
            [_issue("ENG-1"), _issue("ENG-2", "In Progress")],
        )
        self.assertEqual(code, lg.EXIT_UNAPPROVED)
        self.assertEqual(
            out,
            "APPROVED ENG-1 open-threads=0 state=Todo\n"
            "UNAPPROVED ENG-2 open-threads=0 state=In Progress\n",
        )

    def test_a_missing_state_is_an_error_not_unapproved(self):
        payload = _issue("ENG-1")
        payload["issue"]["state"] = None
        code, out, err, _ = _run(["approvals", "ENG-1"], [payload])
        self.assertEqual(code, lg.EXIT_ERROR)
        self.assertEqual(out, "")
        self.assertIn("no state", err)

    def test_an_unresolved_issue_is_an_error_not_unapproved(self):
        code, out, err, _ = _run(["approvals", "ENG-9"], [{"issue": None}])
        self.assertEqual(code, lg.EXIT_ERROR)
        self.assertEqual(out, "")
        self.assertIn("ENG-9 did not resolve", err)

    def test_a_malformed_issue_is_an_error_not_unapproved(self):
        code, out, _, _ = _run(["approvals", "ENG-9"], [{"issue": ["x"]}])
        self.assertEqual(code, lg.EXIT_ERROR)
        self.assertEqual(out, "")

    def test_a_failure_mid_batch_prints_nothing_and_exits_two(self):
        code, out, _, _ = _run(
            ["approvals", "ENG-1", "ENG-2"],
            [_issue("ENG-1"), {"issue": None}],
        )
        self.assertEqual(code, lg.EXIT_ERROR)
        self.assertEqual(out, "")

    def test_an_unexpected_exception_exits_two(self):
        code, out, err, _ = _run(["approvals", "ENG-1"], [RuntimeError("boom")])
        self.assertEqual(code, lg.EXIT_ERROR)
        self.assertEqual(out, "")
        self.assertIn("unexpected RuntimeError", err)
        self.assertNotIn("boom", err)

    def test_the_requested_identifier_is_printed_not_the_servers(self):
        _, out, _, post = _run(["approvals", "ENG-1"], [_issue("ENG-777")])
        self.assertEqual(out, "APPROVED ENG-1 open-threads=0 state=Todo\n")
        self.assertEqual(post.call_args.args[2]["id"], "ENG-1")


class ResolveTests(unittest.TestCase):
    def test_a_resolved_thread(self):
        code, out, _, post = _run(
            ["resolve", "c-1", "c-2"], [{"commentResolve": {"success": True}}]
        )
        self.assertEqual(code, 0)
        self.assertEqual(out, "RESOLVED c-1 by c-2\n")
        self.assertEqual(
            post.call_args.args[2], {"id": "c-1", "resolvingCommentId": "c-2"}
        )

    def test_the_resolving_comment_is_optional(self):
        code, out, _, post = _run(
            ["resolve", "c-1"], [{"commentResolve": {"success": True}}]
        )
        self.assertEqual(code, 0)
        self.assertEqual(out, "RESOLVED c-1\n")
        self.assertIsNone(post.call_args.args[2]["resolvingCommentId"])

    def test_an_unsuccessful_resolve_is_an_error(self):
        for payload in ({"success": False}, "x", None):
            with self.subTest(payload=payload):
                code, out, err, _ = _run(
                    ["resolve", "c-1"], [{"commentResolve": payload}]
                )
                self.assertEqual(code, lg.EXIT_ERROR)
                self.assertEqual(out, "")
                self.assertIn("commentResolve failed", err)

    def test_dry_run_sends_nothing(self):
        code, out, _, post = _run(["resolve", "c-1", "--dry-run"], [])
        self.assertEqual(code, 0)
        self.assertEqual(out, "WOULD RESOLVE c-1\n")
        post.assert_not_called()


class MissingEnvTests(unittest.TestCase):
    def test_a_missing_key_is_a_clear_refusal(self):
        code, out, err, post = _run(["approvals", "ENG-1"], [], env={})
        self.assertEqual(code, lg.EXIT_ERROR)
        self.assertEqual(out, "")
        self.assertIn("LINEAR_API_KEY is unset", err)
        post.assert_not_called()

    def test_approvals_no_longer_needs_the_assignee(self):
        code, _, _, _ = _run(
            ["approvals", "ENG-1"], [_issue("ENG-1")], env={"LINEAR_API_KEY": "k"}
        )
        self.assertEqual(code, 0)


if __name__ == "__main__":
    unittest.main()
