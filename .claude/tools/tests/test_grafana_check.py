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


def _var(name, kind, current, **extra):
    return {"name": name, "type": kind, "current": {"value": current}, **extra}


def _frames(*columns, names=None):
    names = names or [f"c{i}" for i in range(len(columns))]
    return {
        "frames": [
            {
                "schema": {"fields": [{"name": n} for n in names]},
                "data": {"values": [list(c) for c in columns]},
            }
        ]
    }


class InterpolateTests(unittest.TestCase):
    def _one(self, values, multi=False, literal=None):
        return {"v": {"values": values, "multi": multi, "literal": literal}}

    def test_sqlstring_quotes_and_escapes_every_value(self):
        text = gc.interpolate("x IN (${v:sqlstring})", self._one(["a", "o'k"]))
        self.assertEqual(text, "x IN ('a','o''k')")

    def test_bare_form_is_raw_for_a_single_select_variable(self):
        self.assertEqual(gc.interpolate("${v} min", self._one(["15"])), "15 min")

    def test_bare_form_quotes_a_multi_value_variable(self):
        self.assertEqual(gc.interpolate("${v}", self._one(["a"], multi=True)), "'a'")

    def test_a_custom_all_value_is_inserted_as_a_literal(self):
        resolved = self._one(["a", "b"], literal="%")
        self.assertEqual(gc.interpolate("${v:sqlstring}", resolved), "%")

    def test_server_side_macros_and_unknown_names_are_left_alone(self):
        text = "$__timeFilter(t) AND ${nope}"
        self.assertEqual(gc.interpolate(text, self._one(["a"])), text)


class ResolveVariablesTests(unittest.TestCase):
    def test_chained_query_variables_resolve_in_order(self):
        # The second query interpolates the first, so order is load-bearing.
        dashboard = {
            "templating": {
                "list": [
                    _var(
                        "class", "query", ["$__all"], query="SELECT c", includeAll=True
                    ),
                    _var(
                        "pid", "query", "gone", query="WHERE c IN (${class:sqlstring})"
                    ),
                ]
            }
        }
        seen = []

        def run_query(_ds, sql):
            seen.append(sql)
            return (["fx", "peg"] if sql == "SELECT c" else ["EUR-USD"]), None

        resolved, rows, problems = gc.resolve_variables(dashboard, {}, run_query)
        self.assertEqual(seen[1], "WHERE c IN ('fx','peg')")
        self.assertEqual(resolved["class"]["values"], ["fx", "peg"])
        self.assertTrue(rows[0]["all"])
        # A stored selection that no longer exists falls back to the first option.
        self.assertEqual(resolved["pid"]["values"], ["EUR-USD"])
        self.assertEqual(problems, [])

    def test_a_custom_variable_reads_its_options_and_keeps_its_selection(self):
        dashboard = {"templating": {"list": [_var("m", "custom", "15", query="5,15")]}}
        resolved, _, _ = gc.resolve_variables(dashboard, {}, None)
        self.assertEqual(resolved["m"]["values"], ["15"])

    def test_an_override_wins_over_the_stored_selection(self):
        dashboard = {"templating": {"list": [_var("m", "custom", "15", query="5,15")]}}
        resolved, _, _ = gc.resolve_variables(dashboard, {"m": ["60"]}, None)
        self.assertEqual(resolved["m"]["values"], ["60"])

    def test_a_failing_or_empty_variable_query_is_a_problem(self):
        dashboard = {"templating": {"list": [_var("v", "query", "", query="SELECT")]}}
        _, _, problems = gc.resolve_variables(
            dashboard, {}, lambda _ds, _sql: ([], "pq: boom")
        )
        self.assertTrue(any("pq: boom" in p for p in problems))
        self.assertTrue(any("no values" in p for p in problems))


class CheckPanelsTests(unittest.TestCase):
    DASHBOARD = {
        "templating": {"list": [_var("pid", "custom", ["a", "b"], query="a,b")]},
        "panels": [
            {
                "id": 100,
                "type": "row",
                "panels": [
                    {
                        "id": 2,
                        "title": "Nested",
                        "targets": [{"refId": "A", "rawSql": "SELECT 2"}],
                    },
                ],
            },
            {
                "id": 1,
                "title": "One ${pid}",
                "repeat": "pid",
                "targets": [
                    {"refId": "A", "rawSql": "WHERE p = ${pid:sqlstring}"},
                    {"refId": "B", "rawSql": "hidden", "hide": True},
                ],
            },
        ],
    }

    def _check(self, results_for, panel_id=None, min_rows=1):
        batches = []

        def run_batch(queries):
            batches.append(queries)
            return {q["refId"]: results_for(q["rawSql"]) for q in queries}

        result = gc.check_panels(self.DASHBOARD, panel_id, {}, run_batch, min_rows)
        return result, batches

    def test_a_repeated_panel_runs_once_per_value_with_it_pinned(self):
        result, batches = self._check(lambda _sql: _frames([1, 2]), panel_id=1)
        statements = [q["rawSql"] for batch in batches for q in batch]
        self.assertEqual(statements, ["WHERE p = 'a'", "WHERE p = 'b'"])
        self.assertEqual([r["title"] for r in result["panels"]], ["One a", "One b"])
        self.assertEqual(result["problems"], [])

    def test_a_panel_inside_a_collapsed_row_is_checked(self):
        result, _ = self._check(lambda _sql: _frames([1]), panel_id=2)
        self.assertEqual(result["panels"][0]["rows"], 1)

    def test_too_few_rows_and_query_errors_are_problems(self):
        def results_for(sql):
            return {"error": "pq: bad"} if "'a'" in sql else _frames([])

        result, _ = self._check(results_for, panel_id=1)
        self.assertTrue(
            any("1[a] query A failed: pq: bad" in p for p in result["problems"])
        )
        self.assertTrue(any("1[b] query A returned 0" in p for p in result["problems"]))

    def test_min_rows_zero_accepts_an_empty_panel(self):
        result, _ = self._check(lambda _sql: _frames([]), panel_id=2, min_rows=0)
        self.assertEqual(result["problems"], [])

    def test_an_unknown_panel_id_names_the_ones_that_exist(self):
        result, _ = self._check(lambda _sql: _frames([1]), panel_id=9)
        self.assertTrue(any("have: 2, 1" in p for p in result["problems"]))

    def test_a_variable_query_takes_its_value_field(self):
        result = _frames(["A", "B"], ["a", "b"], names=["__text", "__value"])
        self.assertEqual(gc.frame_column(result), ["a", "b"])


class FetchJsonTests(unittest.TestCase):
    def test_a_partial_query_failure_returns_the_results_body(self):
        # Grafana answers HTTP 400 when ANY query in a batch fails, with every
        # query's result still in the body; raising would hide which one.
        body = json.dumps({"results": {"A": {"error": "pq: bad"}}}).encode()
        error = urllib.error.HTTPError(
            "http://x/api/ds/query", 400, "Bad Request", {}, io.BytesIO(body)
        )
        with mock.patch.object(gc.urllib.request, "urlopen", side_effect=error):
            payload = gc._fetch_json("http://x/api/ds/query", {"queries": []})
        self.assertEqual(payload["results"]["A"]["error"], "pq: bad")

    def test_a_400_on_a_plain_get_is_still_an_error(self):
        error = urllib.error.HTTPError("http://x", 400, "Bad", {}, io.BytesIO(b"{}"))
        with mock.patch.object(gc.urllib.request, "urlopen", side_effect=error):
            with self.assertRaises(gc.GrafanaCheckError):
                gc._fetch_json("http://x")

    def test_a_malformed_var_override_is_a_clear_error(self):
        with self.assertRaises(gc.GrafanaCheckError):
            gc.parse_overrides(["missing-equals"])
        self.assertEqual(gc.parse_overrides(["a=1", "a=2"]), {"a": ["1", "2"]})


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

    def test_panel_prints_variables_and_rows_and_gates_on_problems(self):
        dashboard = {
            "dashboard": {
                "templating": {"list": [_var("m", "custom", "15", query="15")]},
                "panels": [
                    {
                        "id": 4,
                        "title": "T",
                        "targets": [{"refId": "A", "rawSql": "SELECT ${m}"}],
                    }
                ],
            }
        }

        def fake_fetch(endpoint, body=None):
            if body is None:
                return dashboard
            self.assertEqual(body["queries"][0]["rawSql"], "SELECT 15")
            return {"results": {"A": _frames([])}}

        with mock.patch.object(gc, "_fetch_json", side_effect=fake_fetch):
            code, out, err = self._run(["panel", "--dashboard", "d"])
        self.assertEqual(code, 1)
        self.assertIn("var | m | 15", out)
        self.assertIn("panel | 4 | A | 0 row(s) | T", out)
        self.assertIn("1 problem(s)", err)


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
