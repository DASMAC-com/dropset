#!/usr/bin/env python3
"""Unit tests for grafana_check.py.

The static mode is the one that matters most: nothing in CI parses the alert
YAML, so a malformed or self-contradicting rule file ships and the first thing
to notice is a human. Nothing here touches the network.
"""

from __future__ import annotations

import io
import json
import os
import sys
import tempfile
import unittest
import urllib.error
from contextlib import redirect_stderr, redirect_stdout
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import grafana_check as gc  # noqa: E402

TWO_RULES = """# The maker's alert rules. Two rules, both per market.
apiVersion: 1
groups:
- name: 'maker'
  rules:
  - condition: 'A'
    title: 'Maker heartbeat dead'
    uid: 'maker-heartbeat-dead'
  - condition: 'B'
    title: 'Feed stale'
    uid: 'maker-feed-stale'
"""


class ParseRulesTests(unittest.TestCase):
    def test_every_rule_is_found_with_its_title(self):
        rules = gc.parse_rules(TWO_RULES)
        self.assertEqual(
            [(r["uid"], r["title"]) for r in rules],
            [
                ("maker-heartbeat-dead", "Maker heartbeat dead"),
                ("maker-feed-stale", "Feed stale"),
            ],
        )

    def test_a_double_quoted_title_is_read_too(self):
        text = "groups:\n  rules:\n  - title: \"Quoted\"\n    uid: 'q'\n"
        self.assertEqual(gc.parse_rules(text)[0]["title"], "Quoted")

    def test_a_file_with_no_rules_yields_nothing(self):
        self.assertEqual(gc.parse_rules("apiVersion: 1\n"), [])

    def test_a_trailing_comment_does_not_hide_a_rule(self):
        # Both identity regexes are end-anchored, so a trailing comment made the
        # line match neither and the rule vanished from the parse — taking it
        # out of the duplicate-uid check too. A gate going blind.
        text = (
            "groups:\n  rules:\n"
            "  - title: 'Kept'  # provisioned 8/24\n"
            "    uid: 'kept'  # do not rename\n"
        )
        rules = gc.parse_rules(text)
        self.assertEqual([(r["uid"], r["title"]) for r in rules], [("kept", "Kept")])

    def test_a_hash_inside_a_quoted_title_is_not_a_comment(self):
        text = "groups:\n  rules:\n  - title: 'Rule #3 fired'\n    uid: 'r3'\n"
        self.assertEqual(gc.parse_rules(text)[0]["title"], "Rule #3 fired")

    def test_a_uid_first_list_item_still_gets_its_title(self):
        # `- uid:` is the ordering the uid regex was widened to accept, but the
        # title then comes AFTER — so carrying it only from above reported
        # "no title" and mis-attached the title to the next rule.
        text = (
            "groups:\n  rules:\n"
            "  - uid: 'first'\n    title: 'First rule'\n"
            "  - uid: 'second'\n    title: 'Second rule'\n"
        )
        rules = gc.parse_rules(text)
        self.assertEqual(
            [(r["uid"], r["title"]) for r in rules],
            [("first", "First rule"), ("second", "Second rule")],
        )

    def test_a_title_never_back_attaches_across_a_list_item_boundary(self):
        # The first rule genuinely has NO title. "Attach to the previous rule if
        # it has none" then reached across the item boundary and stole the
        # second rule's title — leaving rule 1 looking named, rule 2 reported
        # title-less, and the problem pointing at the wrong uid.
        text = (
            "groups:\n  rules:\n"
            "  - uid: 'first'\n    condition: 'A'\n"
            "  - uid: 'second'\n    title: 'Second rule'\n"
        )
        rules = gc.parse_rules(text)
        self.assertEqual(
            [(r["uid"], r["title"]) for r in rules],
            [("first", ""), ("second", "Second rule")],
        )
        problems = gc.check_static(text)["problems"]
        self.assertTrue([p for p in problems if "first" in p and "no title" in p])

    def test_a_uid_first_file_reports_no_missing_titles(self):
        text = (
            "groups:\n  rules:\n"
            "  - uid: 'first'\n    title: 'First rule'\n"
            "  - uid: 'second'\n    title: 'Second rule'\n"
        )
        problems = gc.check_static(text)["problems"]
        self.assertFalse([p for p in problems if "no title" in p])


class StaticCheckTests(unittest.TestCase):
    def test_a_clean_file_has_no_problems(self):
        self.assertEqual(gc.check_static(TWO_RULES)["problems"], [])

    def test_a_duplicate_uid_is_reported(self):
        # Grafana keys on the uid, so one rule silently replaces the other.
        text = TWO_RULES.replace("maker-feed-stale", "maker-heartbeat-dead")
        problems = gc.check_static(text)["problems"]
        self.assertTrue(any("duplicate uid" in p for p in problems))

    def test_a_rule_with_no_title_is_reported(self):
        text = "groups:\n  rules:\n  - uid: 'orphan'\n"
        problems = gc.check_static(text)["problems"]
        self.assertTrue(any("no title" in p for p in problems))

    def test_a_header_count_that_disagrees_with_the_file_is_reported(self):
        # The drift that shipped: a fix commit updated four artifacts and left
        # the alert file's own header claiming a smaller count.
        text = TWO_RULES.replace("Two rules", "Three rules")
        problems = gc.check_static(text)["problems"]
        self.assertTrue(any("claims" in p for p in problems))

    def test_a_positional_claim_about_the_rules_above_is_NOT_a_problem(self):
        # "the two rules above" is a legitimate mid-file reference, and the real
        # repo file contains one. Flagging it would be a false positive that
        # trains the reader to ignore this check — which is worse than no check.
        text = TWO_RULES + "  # The gap the two rules above leave open.\n"
        self.assertEqual(gc.check_static(text)["problems"], [])

    def test_a_positional_claim_with_a_wrong_count_is_still_reported(self):
        text = TWO_RULES + "  # The gap the nine rules above leave open.\n"
        problems = gc.check_static(text)["problems"]
        self.assertTrue(any("claims" in p for p in problems))

    def test_a_count_outside_a_comment_is_not_read_as_a_claim(self):
        # A number inside a rule expression is data, not a claim about the file.
        text = TWO_RULES + "    expr: 'count(x) > 9 rules'\n"
        self.assertEqual(gc.check_static(text)["problems"], [])

    def test_a_rule_declared_without_a_uid_is_reported_not_ignored(self):
        # A rule is identified by its uid, so one without it never became an
        # entry — invisible to the gate, and rejected by Grafana at load. The
        # docstring claimed both were checked; only the title half was.
        text = (
            "groups:\n  rules:\n"
            "  - title: 'Has a uid'\n    uid: 'ok'\n"
            "  - title: 'Missing its uid'\n    condition: 'A'\n"
        )
        problems = gc.check_static(text)["problems"]
        self.assertTrue(any("carry a uid" in p for p in problems))

    def test_an_empty_provisioning_file_is_a_problem(self):
        problems = gc.check_static("apiVersion: 1\n")["problems"]
        self.assertTrue(any("no alert rules" in p for p in problems))


class LiveCheckTests(unittest.TestCase):
    def _payload(self, rules):
        return {"data": {"groups": [{"rules": rules}]}}

    def test_healthy_rules_produce_no_problems(self):
        result = gc.check_live(
            self._payload(
                [{"uid": "a", "name": "A", "health": "ok", "state": "normal"}]
            )
        )
        self.assertEqual(result["problems"], [])

    def test_an_erroring_rule_is_reported_with_its_last_error(self):
        result = gc.check_live(
            self._payload(
                [
                    {
                        "uid": "a",
                        "name": "A",
                        "health": "error",
                        "state": "alerting",
                        "lastError": "bad datasource",
                    }
                ]
            )
        )
        self.assertTrue(any("bad datasource" in p for p in result["problems"]))

    def test_nodata_counts_as_healthy(self):
        # A market quoting normally returns no row, which arrives as NoData —
        # the healthy state for these rules.
        result = gc.check_live(
            self._payload(
                [{"uid": "a", "name": "A", "health": "nodata", "state": "normal"}]
            )
        )
        self.assertEqual(result["problems"], [])

    def test_no_rules_at_all_means_provisioning_did_not_load(self):
        result = gc.check_live({"data": {"groups": []}})
        self.assertTrue(any("did not load" in p for p in result["problems"]))

    def test_an_unreachable_grafana_is_a_clear_error(self):
        with mock.patch.object(
            gc.urllib.request, "urlopen", side_effect=urllib.error.URLError("refused")
        ):
            with self.assertRaises(gc.GrafanaCheckError) as caught:
                gc.fetch_live("http://localhost:3200")
        self.assertIn("collector stack", str(caught.exception))


class CliTests(unittest.TestCase):
    def _run(self, argv):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = gc.run(["grafana_check.py"] + argv)
        return code, out.getvalue(), err.getvalue()

    def test_static_exits_zero_on_a_clean_file(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "maker.yml")
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(TWO_RULES)
            code, out, err = self._run(["static", "--file", path])
        self.assertEqual(code, 0)
        self.assertIn("maker-heartbeat-dead", out)
        self.assertIn("0 problem(s)", err)

    def test_static_exits_non_zero_on_a_problem(self):
        # Non-zero is what makes this usable as a gate rather than a report.
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "maker.yml")
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(
                    TWO_RULES.replace("maker-feed-stale", "maker-heartbeat-dead")
                )
            code, _, err = self._run(["static", "--file", path])
        self.assertEqual(code, 1)
        self.assertIn("PROBLEM", err)

    def test_live_reports_health_and_state_per_rule(self):
        payload = {
            "data": {
                "groups": [
                    {
                        "rules": [
                            {"uid": "a", "name": "A", "health": "ok", "state": "normal"}
                        ]
                    }
                ]
            }
        }
        handle = mock.MagicMock()
        handle.__enter__.return_value = io.BytesIO(json.dumps(payload).encode())
        with mock.patch.object(gc.urllib.request, "urlopen", return_value=handle):
            code, out, _ = self._run(["live", "--url", "http://localhost:3200"])
        self.assertEqual(code, 0)
        self.assertIn("a | ok | normal | A", out)

    def test_a_missing_file_is_a_clear_error(self):
        with self.assertRaises(gc.GrafanaCheckError):
            self._run(["static", "--file", "/nonexistent/maker.yml"])


class RealRepoFileTests(unittest.TestCase):
    """The committed alert file must pass its own gate."""

    def test_the_provisioned_maker_rules_are_clean(self):
        repo = os.path.dirname(
            os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
        )
        path = os.path.join(
            repo, "market-data", "grafana", "provisioning", "alerting", "maker.yml"
        )
        if not os.path.exists(path):
            self.skipTest("alerting file not present in this checkout")
        with open(path, encoding="utf-8") as handle:
            result = gc.check_static(handle.read())
        self.assertEqual(result["problems"], [])
        self.assertGreater(len(result["rules"]), 0)


if __name__ == "__main__":
    unittest.main()
