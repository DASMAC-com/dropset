#!/usr/bin/env python3
"""Unit tests for `verb_check.py` (stdlib unittest, no network).

One test per row of the module docstring's outcome table, each over an
embedded title fixture; the Linear read is replaced at the `fetch` seam.
"""

from __future__ import annotations

import io
import os
import unittest
from contextlib import redirect_stderr
from unittest import mock

import verb_check

#: Embedded fixtures: the issue titles the rows below launch against.
TITLES = {
    "plain": "Add the verb title guard",
    "chore_anthropic": "[Chore/Anthropic] Rotate the deploy key",
    "explore_bedrock": "[Explore/Bedrock] Audit the feeds runner",
    "architect_bedrock": "[Architect/Bedrock] Claude: Linear orchestration",
    "task_anthropic": "[Task/Anthropic] Survey the DFlow docs",
    "operator": "[Operator] Grind the mainnet keys",
    "half": "[Chore] Rotate the deploy key",
    "foreign": "[WIP] Something",
}


def _run(argv, title=None, *, error=None, key="lin_api_test"):
    def fetch(api_key, identifier):
        if error is not None:
            raise verb_check.VerbCheckError(error)
        return title

    env = {"LINEAR_API_KEY": key} if key else {}
    err = io.StringIO()
    with mock.patch.dict(os.environ, env, clear=True), redirect_stderr(err):
        code = verb_check.run(argv, fetch=fetch)
    return code, err.getvalue()


class Outcomes(unittest.TestCase):
    def test_a_matching_token_launches(self):
        for title, argv in (
            ("chore_anthropic", ["chore", "anthropic", "1234"]),
            ("explore_bedrock", ["explore", "bedrock", "1234"]),
            ("architect_bedrock", ["architect", "bedrock", "eng-1234"]),
            ("task_anthropic", ["task", "anthropic", "1234"]),
        ):
            with self.subTest(title=title):
                self.assertEqual(_run(argv, TITLES[title]), (0, ""))

    def test_no_token_launches_task_on_bedrock(self):
        self.assertEqual(_run(["task", "bedrock", "1234"], TITLES["plain"]), (0, ""))

    def test_no_token_refuses_any_other_launch(self):
        for argv in (
            ["task", "anthropic", "1234"],
            ["chore", "bedrock", "1234"],
            ["architect", "bedrock", "1234"],
        ):
            with self.subTest(argv=argv):
                code, err = _run(argv, TITLES["plain"])
                self.assertEqual(code, 1)
                self.assertIn("the title says task on Bedrock", err)
                # Spelled out: a bare `task 1234` takes the configured
                # substrate, which may be the one just refused.
                self.assertIn("`task bedrock 1234`", err)

    def test_another_token_refuses_with_the_implied_line(self):
        code, err = _run(["task", "bedrock", "1234"], TITLES["chore_anthropic"])
        self.assertEqual(code, 1)
        self.assertIn("`chore anthropic 1234`", err)
        code, err = _run(["explore", "anthropic", "1234"], TITLES["explore_bedrock"])
        self.assertEqual(code, 1)
        self.assertIn("`explore bedrock 1234`", err)

    def test_operator_refuses(self):
        code, err = _run(["chore", "bedrock", "1234"], TITLES["operator"])
        self.assertEqual(code, 1)
        self.assertIn("this item has no agent; see ruling 27", err)

    def test_an_unrecognized_bracket_refuses(self):
        # A half-token must not read as "no token", or `task` would launch on
        # an issue titled for a chore.
        for title in ("half", "foreign"):
            with self.subTest(title=title):
                code, err = _run(["task", "bedrock", "1234"], TITLES[title])
                self.assertEqual(code, 1)
                self.assertIn("not a launch token", err)

    def test_an_unreachable_linear_warns_and_launches(self):
        code, err = _run(["chore", "bedrock", "1234"], error="timed out")
        self.assertEqual(code, 0)
        self.assertIn("title not checked (timed out)", err)

    def test_a_server_supplied_reason_reaches_the_terminal_printable_only(self):
        code, err = _run(["chore", "bedrock", "1234"], error="HTTP 502: \x1b[2J gone")
        self.assertEqual(code, 0)
        self.assertNotIn("\x1b", err)
        self.assertIn("[2J gone", err)

    def test_no_key_warns_and_launches(self):
        code, err = _run(["chore", "bedrock", "1234"], TITLES["plain"], key="")
        self.assertEqual(code, 0)
        self.assertIn("LINEAR_API_KEY is unset", err)
        self.assertEqual(err.count("\n"), 1)

    def test_a_name_is_not_checked(self):
        def fetch(api_key, identifier):
            raise AssertionError("a name must not reach Linear")

        with redirect_stderr(io.StringIO()):
            self.assertEqual(
                verb_check.run(["explore", "bedrock", "feeds"], fetch=fetch), 0
            )

    def test_a_padded_number_names_the_issue(self):
        seen = []

        def fetch(api_key, identifier):
            seen.append(identifier)
            return TITLES["plain"]

        with mock.patch.dict(os.environ, {"LINEAR_API_KEY": "k"}, clear=True):
            verb_check.run(["task", "bedrock", "eng-0123"], fetch=fetch)
        self.assertEqual(seen, ["ENG-123"])

    def test_usage(self):
        for argv in ([], ["plan", "bedrock", "1"], ["task", "local", "1"]):
            with self.subTest(argv=argv):
                self.assertEqual(_run(argv, TITLES["plain"])[0], 2)


if __name__ == "__main__":
    unittest.main()
