#!/usr/bin/env python3
"""Unit tests for ``linear_issue.py`` (stdlib ``unittest``; no pytest).

The GraphQL transport is replaced with a recorder, so these assert the thing
that cannot be checked by eye and is the entire reason the tool exists: that
the stored issue body never reaches stdout, and that what does reach the wire
is the grown body rather than a replacement.
"""

from __future__ import annotations

import unittest

import linear_issue as li


class _Recorder:
    """Stands in for the GraphQL transport, recording what was sent."""

    def __init__(self, responses):
        self.responses = list(responses)
        self.calls = []

    def __call__(self, api_key, query, variables):
        self.calls.append((query, variables))
        if not self.responses:
            raise AssertionError("unexpected extra GraphQL call")
        return self.responses.pop(0)


class _Stubbed(unittest.TestCase):
    def setUp(self):
        real = li._post
        self.addCleanup(lambda: setattr(li, "_post", real))

    def install(self, responses):
        recorder = _Recorder(responses)
        li._post = recorder
        return recorder


class AppendBody(_Stubbed):
    @staticmethod
    def body(description):
        return {
            "issue": {
                "id": "u1",
                "identifier": "ENG-1",
                "url": "https://example.invalid/ENG-1",
                "description": description,
            }
        }

    @staticmethod
    def ok():
        return {"issueUpdate": {"success": True, "issue": {"identifier": "ENG-1"}}}

    def test_the_stored_body_never_reaches_the_returned_line(self):
        # The whole point of the tool. The body is read into this process,
        # grown, and sent back; what comes out is accounting.
        secret = "STORED BODY THAT MUST NOT BE ECHOED"
        self.install([self.body(secret), self.ok()])
        line = li.append_body("k", "ENG-1", "fresh evidence", dry_run=False)
        self.assertNotIn(secret, line)
        self.assertIn("APPENDED to ENG-1", line)

    def test_the_addition_is_separated_by_exactly_one_blank_line(self):
        recorder = self.install([self.body("first\n"), self.ok()])
        li.append_body("k", "ENG-1", "second", dry_run=False)
        self.assertEqual(recorder.calls[-1][1]["description"], "first\n\nsecond\n")

    def test_the_stored_body_is_preserved_not_replaced(self):
        # An append that dropped the prior body would still print a plausible
        # success line, so the retained prefix is asserted directly.
        recorder = self.install([self.body("keep me"), self.ok()])
        li.append_body("k", "ENG-1", "added", dry_run=False)
        self.assertTrue(recorder.calls[-1][1]["description"].startswith("keep me"))

    def test_an_empty_body_does_not_gain_leading_blank_lines(self):
        recorder = self.install([self.body(""), self.ok()])
        li.append_body("k", "ENG-1", "only", dry_run=False)
        self.assertEqual(recorder.calls[-1][1]["description"], "only\n")

    def test_a_null_description_is_treated_as_empty(self):
        # Linear returns null, not "", for an issue filed with no body.
        recorder = self.install([self.body(None), self.ok()])
        li.append_body("k", "ENG-1", "only", dry_run=False)
        self.assertEqual(recorder.calls[-1][1]["description"], "only\n")

    def test_empty_text_is_refused_rather_than_written(self):
        recorder = self.install([])
        with self.assertRaises(li.LinearIssueError):
            li.append_body("k", "ENG-1", "   \n  ", dry_run=False)
        self.assertEqual(recorder.calls, [])

    def test_dry_run_does_not_even_read_the_body(self):
        recorder = self.install([])
        line = li.append_body("k", "ENG-1", "text", dry_run=True)
        self.assertTrue(line.startswith("WOULD APPEND"))
        self.assertEqual(recorder.calls, [])

    def test_a_reported_failure_raises_rather_than_claiming_success(self):
        self.install([self.body("x"), {"issueUpdate": {"success": False}}])
        with self.assertRaises(li.LinearIssueError):
            li.append_body("k", "ENG-1", "text", dry_run=False)

    def test_a_missing_issue_is_an_error_not_a_silent_skip(self):
        self.install([{"issue": None}])
        with self.assertRaises(li.LinearIssueError):
            li.append_body("k", "ENG-404", "text", dry_run=False)


class Find(_Stubbed):
    @staticmethod
    def rows(nodes):
        return {"issues": {"nodes": nodes}}

    def test_only_identifier_and_title_come_back(self):
        self.install(
            [self.rows([{"identifier": "ENG-7", "title": "Reference price guard"}])]
        )
        got = li.find("k", query="reference", project_id=None, state=None, limit=5)
        self.assertEqual(got, ["ENG-7  Reference price guard"])

    def test_the_filters_are_sent_when_given(self):
        recorder = self.install([self.rows([])])
        li.find("k", query="q", project_id="p1", state="Backlog", limit=5)
        sent = recorder.calls[-1][1]["filter"]
        self.assertEqual(sent["project"], {"id": {"eq": "p1"}})
        self.assertEqual(sent["state"], {"name": {"eq": "Backlog"}})

    def test_absent_filters_are_omitted_rather_than_sent_as_null(self):
        recorder = self.install([self.rows([])])
        li.find("k", query="q", project_id=None, state=None, limit=5)
        sent = recorder.calls[-1][1]["filter"]
        self.assertNotIn("project", sent)
        self.assertNotIn("state", sent)

    def test_the_limit_is_capped(self):
        recorder = self.install([self.rows([])])
        li.find("k", query="q", project_id=None, state=None, limit=10_000)
        self.assertEqual(recorder.calls[-1][1]["first"], li.MAX_FIND)

    def test_no_matches_is_an_empty_list_not_an_error(self):
        self.install([self.rows([])])
        self.assertEqual(
            li.find("k", query="q", project_id=None, state=None, limit=5), []
        )


if __name__ == "__main__":
    unittest.main()
