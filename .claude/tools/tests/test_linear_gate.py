#!/usr/bin/env python3
"""Unit tests for linear_gate.py.

The property carried hardest is the exit code: a dispatch wrapper refuses on
it, so an unapproved issue must exit 1 and a failure must exit 2 — a failure
that read as "approved" would dispatch an unratified spec.

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

OPERATOR = "user-operator"
ENV = {"LINEAR_API_KEY": "k", "LINEAR_ASSIGNEE_ID": OPERATOR}


def _issue(identifier: str, reaction_data: object) -> dict:
    return {"issue": {"identifier": identifier, "reactionData": reaction_data}}


def _thumbs(user_id: str, at: str = "2026-10-20T01:00:00.000Z") -> list:
    return [{"emoji": "+1", "reactions": [{"userId": user_id, "reactedAt": at}]}]


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


class ApprovalTimeTests(unittest.TestCase):
    def test_the_operators_thumbs_up_is_the_approval(self):
        self.assertEqual(lg.approval_time(_thumbs(OPERATOR, "T1"), OPERATOR), "T1")

    def test_someone_elses_thumbs_up_is_not(self):
        self.assertIsNone(lg.approval_time(_thumbs("user-other"), OPERATOR))

    def test_another_emoji_by_the_operator_is_not(self):
        data = [{"emoji": "eyes", "reactions": [{"userId": OPERATOR}]}]
        self.assertIsNone(lg.approval_time(data, OPERATOR))

    def test_null_reaction_data_is_no_approval(self):
        self.assertIsNone(lg.approval_time(None, OPERATOR))

    def test_the_glyph_form_is_an_approval_too(self):
        data = [
            {
                "emoji": "\N{THUMBS UP SIGN}",
                "reactions": [{"userId": OPERATOR, "reactedAt": "T2"}],
            }
        ]
        self.assertEqual(lg.approval_time(data, OPERATOR), "T2")

    def test_a_thumbs_up_without_a_timestamp_still_approves(self):
        data = [{"emoji": "+1", "reactions": [{"userId": OPERATOR}]}]
        self.assertEqual(lg.approval_time(data, OPERATOR), "unknown time")


class ApprovalsTests(unittest.TestCase):
    def test_an_approved_issue_exits_zero(self):
        code, out, _, _ = _run(
            ["approvals", "ENG-1"], [_issue("ENG-1", _thumbs(OPERATOR, "T1"))]
        )
        self.assertEqual(code, 0)
        self.assertEqual(out, "APPROVED ENG-1 T1\n")

    def test_one_unapproved_issue_exits_one_and_still_reports_all(self):
        code, out, _, _ = _run(
            ["approvals", "ENG-1", "ENG-2"],
            [_issue("ENG-1", _thumbs(OPERATOR, "T1")), _issue("ENG-2", None)],
        )
        self.assertEqual(code, lg.EXIT_UNAPPROVED)
        self.assertEqual(out, "APPROVED ENG-1 T1\nUNAPPROVED ENG-2\n")

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
            [_issue("ENG-1", _thumbs(OPERATOR)), {"issue": None}],
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
        _, out, _, post = _run(
            ["approvals", "ENG-1"], [_issue("ENG-777", _thumbs(OPERATOR, "T1"))]
        )
        self.assertEqual(out, "APPROVED ENG-1 T1\n")
        self.assertEqual(post.call_args.args[2], {"id": "ENG-1"})


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

    def test_a_missing_assignee_refuses_approvals(self):
        code, _, err, post = _run(
            ["approvals", "ENG-1"], [], env={"LINEAR_API_KEY": "k"}
        )
        self.assertEqual(code, lg.EXIT_ERROR)
        self.assertIn("LINEAR_ASSIGNEE_ID is unset", err)
        post.assert_not_called()


if __name__ == "__main__":
    unittest.main()
