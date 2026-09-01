"""Tests for the Grafana dashboard SQL extractor."""

import json
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))

import dashboard_sql as ds  # noqa: E402


def dashboard(panels=(), templating=(), annotations=()):
    """A minimal dashboard document with just the parts the tool reads."""
    return {
        "panels": list(panels),
        "templating": {"list": list(templating)},
        "annotations": {"list": list(annotations)},
    }


def panel(pid, title, sql, **extra):
    return {
        "id": pid,
        "title": title,
        "targets": [{"rawSql": sql}],
        **extra,
    }


def write(tmp, doc, name="board.json"):
    d = pathlib.Path(tmp)
    (d / name).write_text(json.dumps(doc))
    return d


class LiteralSpans(unittest.TestCase):
    """`string_literal_spans` decides both guards, so it gets the most tests."""

    def test_finds_a_plain_literal(self):
        sql = "SELECT 1 WHERE a = 'x'"
        self.assertEqual(
            [sql.index("'x'")], [a for a, _ in ds.string_literal_spans(sql)]
        )

    def test_a_doubled_quote_is_an_escape_not_two_literals(self):
        # `'it''s'` is one literal. Treating it as two would shift every span
        # after it and flip the in/out verdict for the rest of the query.
        sql = "SELECT 'it''s' AS a, 'b'"
        spans = ds.string_literal_spans(sql)
        self.assertEqual(len(spans), 2)
        self.assertEqual(sql[spans[0][0] : spans[0][1]], "'it''s'")
        self.assertEqual(sql[spans[1][0] : spans[1][1]], "'b'")

    def test_an_apostrophe_in_a_line_comment_opens_no_literal(self):
        # THE REGRESSION THIS FILE EXISTS FOR. Prose in a `--` comment routinely
        # contains an apostrophe ("the estimator's output"), and counting it as a
        # quote opens a literal that never closes — inverting the
        # inside/outside verdict for everything after it. Measured against the
        # real dashboards, this bug made the regex guard miss 8 of 14 genuine
        # sites while reporting green.
        sql = "-- the estimator's output\nSELECT 1 WHERE a = 'x'\n"
        spans = ds.string_literal_spans(sql)
        self.assertEqual(len(spans), 1, "the comment must not open a literal")
        self.assertEqual(sql[spans[0][0] : spans[0][1]], "'x'")

    def test_an_apostrophe_in_a_block_comment_opens_no_literal(self):
        sql = "/* Grafana's parser */ SELECT 1 WHERE a = 'x'"
        spans = ds.string_literal_spans(sql)
        self.assertEqual(len(spans), 1)
        self.assertEqual(sql[spans[0][0] : spans[0][1]], "'x'")

    def test_an_unterminated_comment_swallows_the_rest(self):
        # Degenerate but must not crash or mis-span.
        self.assertEqual(ds.string_literal_spans("/* unterminated 'x'"), [])

    def test_a_quoted_identifier_holding_an_apostrophe_opens_no_literal(self):
        # These dashboards use quoted identifiers by NECESSITY — Grafana binds a
        # time series on `AS "time"` and the candlestick panel on the OHLC
        # names — so this is not exotic. `"it's"` would otherwise open a phantom
        # literal exactly like the comment case.
        sql = "SELECT a AS \"it's\", 'x'"
        spans = ds.string_literal_spans(sql)
        self.assertEqual(len(spans), 1)
        self.assertEqual(sql[spans[0][0] : spans[0][1]], "'x'")

    def test_a_quoted_identifier_holding_a_double_dash_opens_no_comment(self):
        # The DANGEROUS direction: read as a line comment, `"col--name"` would
        # swallow the rest of the line including the real literal, so a genuine
        # regex-in-literal site would go undetected and the guard would report
        # green.
        sql = "SELECT a AS \"col--name\", b = 'x'"
        spans = ds.string_literal_spans(sql)
        self.assertEqual(len(spans), 1, "the identifier must not start a comment")
        self.assertEqual(sql[spans[0][0] : spans[0][1]], "'x'")

    def test_block_comments_nest(self):
        # Postgres nests block comments per the SQL standard. Ending at the
        # first `*/` resumes scanning inside the outer comment, where the
        # apostrophe in "Grafana's" opens a phantom literal.
        sql = "/* a /* b */ Grafana's parser */ SELECT 'x'"
        spans = ds.string_literal_spans(sql)
        self.assertEqual(len(spans), 1, "the outer comment must not end early")
        self.assertEqual(sql[spans[0][0] : spans[0][1]], "'x'")

    def test_a_doubled_quote_inside_an_identifier_is_an_escape(self):
        sql = 'SELECT a AS "he""llo", \'x\''
        spans = ds.string_literal_spans(sql)
        self.assertEqual(len(spans), 1)
        self.assertEqual(sql[spans[0][0] : spans[0][1]], "'x'")


class RegexGuard(unittest.TestCase):
    def test_rejects_a_regex_formatter_inside_a_literal(self):
        sql = "SELECT 1 WHERE source ~ '^${source:regex}$'"
        with self.assertRaises(ds.ExtractionError) as cm:
            ds.check_regex_in_literal(sql, "where")
        # The message has to name the fix, or the guard just blocks people.
        self.assertIn("sqlstring", str(cm.exception))

    def test_rejects_it_even_after_a_comment_with_an_apostrophe(self):
        # The combination that was silently passing before the span fix.
        sql = "-- Grafana's macro parser\nSELECT 1 WHERE s ~ '^${source:regex}$'"
        with self.assertRaises(ds.ExtractionError):
            ds.check_regex_in_literal(sql, "where")

    def test_allows_the_sqlstring_form(self):
        # The shape the fix produces must pass, or the gate blocks its own remedy.
        sql = "SELECT 1 WHERE source = ANY (ARRAY[${source:sqlstring}]::text[])"
        ds.check_regex_in_literal(sql, "where")

    def test_allows_a_regex_formatter_outside_a_literal(self):
        # Not the defect: without surrounding quotes there is no quoting bug.
        ds.check_regex_in_literal("SELECT ${source:regex}", "where")


class MacroGuard(unittest.TestCase):
    def test_rejects_a_nested_paren_in_a_macro_argument(self):
        sql = "SELECT $__timeGroup(to_timestamp(ts), '1m')"
        with self.assertRaises(ds.ExtractionError) as cm:
            ds.check_macro_args(sql, "where")
        self.assertIn("truncated", str(cm.exception))

    def test_allows_a_bare_column_argument(self):
        ds.check_macro_args("SELECT $__timeGroup(ts, '1m')", "where")

    def test_rejects_an_unclosed_macro_call(self):
        with self.assertRaises(ds.ExtractionError):
            ds.check_macro_args("SELECT $__timeFilter(ts", "where")

    def test_ignores_a_macro_named_inside_a_comment(self):
        # The guard's own docstring notes that three dashboard queries carry
        # comments explaining they were written around this limitation — and the
        # natural way to write one is to quote the broken form. Refusing that is
        # a false positive its sibling guard never had.
        sql = (
            "-- do NOT write $__timeGroup(to_timestamp(t), '1m') here\n"
            "SELECT $__timeGroup(ts, '1m')"
        )
        ds.check_macro_args(sql, "where")

    def test_ignores_an_unclosed_macro_inside_a_comment(self):
        # This one produced an error message naming nothing the author could
        # act on.
        ds.check_macro_args("-- see $__timeFilter( for why\nSELECT 1", "where")


class Substitution(unittest.TestCase):
    def test_leaves_no_variable_or_macro_behind(self):
        # An unsubstituted reference reaches sqlfluff as a parse error, and the
        # finding then gets blamed on the query rather than on the shim.
        #
        # Asserting "no `$` at all" would be wrong: a regex anchor is a bare `$`
        # and survives substitution legitimately — `'^${p:regex}$'` becomes
        # `'^x$'`, whose trailing `$` is part of the pattern, not a variable.
        # So the assertion is about leftover REFERENCES specifically.
        sql = (
            "SELECT $__timeGroup(ts, '1m') AS t, $granularity AS g\n"
            "FROM x WHERE $__timeFilter(ts)\n"
            "  AND s = ANY (ARRAY[${s:sqlstring}]::text[])\n"
            "  AND p ~ '^${p:regex}$' AND tz = '$hour_tz'"
        )
        out = ds.substitute(sql)
        self.assertNotIn("$__", out, out)
        self.assertNotIn("${", out, out)
        self.assertIsNone(ds.VAR_REF.search(out), out)
        # The anchor is still there, which is the point of the carve-out above.
        self.assertIn("'^x$'", out)

    def test_types_a_variable_by_context(self):
        # Inside a literal a number would still be a string, but outside one a
        # bare word is an identifier — so the two cases cannot share a stand-in.
        self.assertIn("'x'", ds.substitute("SELECT ${v:sqlstring}"))
        self.assertEqual(ds.substitute("SELECT '$v'"), "SELECT 'x'")
        self.assertEqual(ds.substitute("SELECT $v"), "SELECT 1")

    def test_an_unknown_macro_collapses_to_its_argument(self):
        # A new Grafana macro must not break the lint gate; correctness is the
        # guards' job, not the shim's.
        self.assertEqual(ds.substitute("SELECT $__brandNew(ts)"), "SELECT ts")

    def test_the_longer_bare_macro_wins_over_its_own_prefix(self):
        # `$__interval` is a prefix of `$__interval_ms`. Shortest-first leaves
        # `'1 minute'_ms`, silently. The loop sorts by length for this reason,
        # so the dict's own order cannot reintroduce it.
        out = ds.substitute("SELECT $__interval_ms, $__interval")
        self.assertEqual(out, "SELECT 60000, '1 minute'")

    def test_a_multi_segment_formatter_is_still_substituted(self):
        # Grafana's built-ins use several colon-separated segments
        # (`${__from:date:iso}`). Matching only one left the whole reference
        # unmatched, so it survived verbatim and reached sqlfluff as a parse
        # error blamed on the panel rather than on this shim.
        out = ds.substitute("SELECT ${__from:date:iso}")
        self.assertNotIn("$", out, out)


class Extraction(unittest.TestCase):
    def test_names_files_by_panel_id_and_title(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = write(tmp, dashboard(panels=[panel(7, "Rows per minute", "SELECT 1")]))
            self.assertEqual(
                [rel for rel, _ in ds.queries(d / "board.json")],
                ["board/panel-07-rows-per-minute.sql"],
            )

    def test_walks_panels_nested_in_a_row(self):
        # A collapsed row holds its children in its own `panels`, and missing
        # them would silently drop those queries from both mirror and lint.
        with tempfile.TemporaryDirectory() as tmp:
            row = {"id": 1, "title": "Row", "panels": [panel(2, "Inner", "SELECT 2")]}
            d = write(tmp, dashboard(panels=[row]))
            rels = [rel for rel, _ in ds.queries(d / "board.json")]
            self.assertIn("board/panel-02-inner.sql", rels)

    def test_numbers_multiple_targets_on_one_panel(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = panel(1, "Two", "SELECT 1")
            p["targets"].append({"rawSql": "SELECT 2"})
            d = write(tmp, dashboard(panels=[p]))
            self.assertEqual(
                [rel for rel, _ in ds.queries(d / "board.json")],
                ["board/panel-01-two-1.sql", "board/panel-01-two-2.sql"],
            )

    def test_skips_a_custom_variable(self):
        # A `custom` variable's `query` is a comma-separated option list, not
        # SQL; linting one would fail on the commas.
        with tempfile.TemporaryDirectory() as tmp:
            d = write(
                tmp,
                dashboard(
                    templating=[
                        {
                            "name": "tz",
                            "type": "custom",
                            "query": "UTC,America/New_York",
                        },
                        {"name": "src", "type": "query", "query": "SELECT 1"},
                    ]
                ),
            )
            self.assertEqual(
                [rel for rel, _ in ds.queries(d / "board.json")],
                ["board/var-src.sql"],
            )

    def test_rejects_a_definition_that_disagrees_with_its_query(self):
        # Grafana runs `query` and displays `definition`; drift makes the
        # variable editor describe SQL that is not what executes.
        with tempfile.TemporaryDirectory() as tmp:
            d = write(
                tmp,
                dashboard(
                    templating=[
                        {
                            "name": "src",
                            "type": "query",
                            "query": "SELECT 1",
                            "definition": "SELECT 2",
                        }
                    ]
                ),
            )
            with self.assertRaises(ds.ExtractionError) as cm:
                ds.queries(d / "board.json")
            self.assertIn("definition", str(cm.exception))

    def test_extracts_an_annotation_query(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = write(
                tmp,
                dashboard(
                    annotations=[
                        {"name": "FX weekend", "target": {"rawSql": "SELECT 1"}}
                    ]
                ),
            )
            self.assertEqual(
                [rel for rel, _ in ds.queries(d / "board.json")],
                ["board/annotation-fx-weekend.sql"],
            )


class CheckAndExtract(unittest.TestCase):
    def _args(self, dashboards, mirror):
        return type("Args", (), {"dashboards": dashboards, "mirror": mirror})()

    def test_check_fails_on_a_stale_mirror_then_passes_after_extract(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            dash = root / "dash"
            dash.mkdir()
            mirror = root / "mirror"
            (dash / "board.json").write_text(
                json.dumps(dashboard(panels=[panel(1, "One", "SELECT 1")]))
            )
            args = self._args(dash, mirror)
            mirror.mkdir()
            self.assertEqual(ds.cmd_check(args), 1, "an empty mirror must fail")
            self.assertEqual(ds.cmd_extract(args), 0)
            self.assertEqual(ds.cmd_check(args), 0, "must pass right after extract")

    def test_extract_prunes_an_orphaned_mirror_file(self):
        # A renamed panel otherwise leaves its old mirror behind, to be linted
        # forever and to read as a query that still exists.
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            dash = root / "dash"
            dash.mkdir()
            mirror = root / "mirror"
            (dash / "board.json").write_text(
                json.dumps(dashboard(panels=[panel(1, "One", "SELECT 1")]))
            )
            args = self._args(dash, mirror)
            ds.cmd_extract(args)
            orphan = mirror / "board" / "panel-01-stale-name.sql"
            orphan.write_text("SELECT 'left behind'")
            ds.cmd_extract(args)
            self.assertFalse(orphan.exists())

    def test_duplicate_mirror_paths_are_refused(self):
        # Two panels sharing an id and title would otherwise have one silently
        # overwrite the other.
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            dash = root / "dash"
            dash.mkdir()
            (dash / "board.json").write_text(
                json.dumps(
                    dashboard(
                        panels=[
                            panel(1, "Same", "SELECT 1"),
                            panel(1, "Same", "SELECT 2"),
                        ]
                    )
                )
            )
            with self.assertRaises(ds.ExtractionError):
                ds.collect(dash)


class RealDashboards(unittest.TestCase):
    """The committed dashboards must satisfy both guards.

    This is the assertion that would have caught the regex-formatter defect, so
    it must not be able to go quiet. It previously skipped whenever the cwd was
    not the repo root and then asserted only non-emptiness — a test that both
    disappears silently and passes vacuously. The directory is now resolved from
    `__file__` (as the import above already does) so there is no skip, and the
    count has a floor.
    """

    #: `.claude/tools/tests/` -> repo root.
    REPO_ROOT = pathlib.Path(__file__).resolve().parents[3]

    def test_the_committed_dashboards_extract_cleanly(self):
        dashboards = self.REPO_ROOT / ds.DASHBOARD_DIR
        self.assertTrue(
            dashboards.is_dir(),
            f"{dashboards} is missing — this test cannot be allowed to skip",
        )
        found = ds.collect(dashboards)
        # A floor rather than an exact count: exact would churn on every panel
        # added, while non-emptiness would pass if the panel walk silently
        # found one query out of dozens.
        self.assertGreaterEqual(
            len(found), 20, f"only {len(found)} queries extracted — walk broken?"
        )


if __name__ == "__main__":
    unittest.main()
