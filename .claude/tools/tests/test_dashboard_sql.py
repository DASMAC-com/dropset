# cspell:word dedents
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
    def _args(self, dashboards, mirror, alerting=None):
        # `alerting` defaults to None rather than to a directory: these cases are
        # about the dashboard mirror, and a real path here would couple them to
        # the committed alert rules.
        return type(
            "Args",
            (),
            {"dashboards": dashboards, "mirror": mirror, "alerting": alerting},
        )()

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


class BlockScalar(unittest.TestCase):
    """The block-scalar reader, which is what makes the alerting files readable."""

    def read(self, text, at=0):
        return ds.block_scalar(text.splitlines(), at)[0]

    def test_dedents_by_the_first_content_line(self):
        self.assertEqual(self.read("  k: |-\n    a\n    b\n"), "a\nb")

    def test_deeper_indentation_is_content(self):
        # This is the case SQL depends on: an indented continuation line is part
        # of the query, not the end of it.
        self.assertEqual(
            self.read("  k: |-\n    SELECT\n      x\n    FROM t\n"),
            "SELECT\n  x\nFROM t",
        )

    def test_a_blank_line_does_not_terminate_the_value(self):
        # A blank line carries no indentation to compare, so a naive reader ends
        # the scalar here and silently truncates the query.
        self.assertEqual(self.read("  k: |-\n    a\n\n    b\n"), "a\n\nb")

    def test_stops_at_a_sibling_key(self):
        self.assertEqual(self.read("  k: |-\n    a\n  other: 1\n"), "a")

    def test_stops_at_a_shallower_key(self):
        self.assertEqual(self.read("  k: |-\n    a\ntop: 1\n"), "a")

    def test_strip_chomping_drops_the_trailing_newline(self):
        self.assertEqual(self.read("k: |-\n  a\n"), "a")

    def test_clip_chomping_keeps_exactly_one(self):
        self.assertEqual(self.read("k: |\n  a\n"), "a\n")

    def test_returns_the_index_of_the_next_construct(self):
        lines = "  k: |-\n    a\n    b\n  other: 1\n".splitlines()
        self.assertEqual(ds.block_scalar(lines, 0)[1], 3)
        self.assertEqual(lines[3].strip(), "other: 1")

    def test_keep_chomping_retains_every_trailing_newline(self):
        # The only non-trivial arithmetic in the function, and the committed
        # rules all use `|-`, so neither the mirror gate nor the PyYAML
        # equivalence test reaches it. A wrong count here means `check`
        # reports a permanently stale mirror that `extract` cannot settle.
        self.assertEqual(self.read("k: |+\n  a\n\n\n"), "a\n\n\n")

    def test_keep_chomping_with_no_trailing_blanks_matches_clip(self):
        self.assertEqual(self.read("k: |+\n  a\n"), "a\n")

    def test_an_empty_block_is_empty_but_keep_still_keeps(self):
        self.assertEqual(self.read("  k: |-\n  other: 1\n"), "")
        self.assertEqual(self.read("k: |+\n\n\n"), "\n\n")

    def test_dedents_from_the_first_NON_BLANK_line(self):
        # A leading blank line is not popped (only trailing ones are), so
        # measuring `body[0]` gives indent 0 and emits the value still carrying
        # its YAML indentation. PyYAML dedents it, so this is a silent
        # divergence in a value compared byte-for-byte.
        self.assertEqual(self.read("  k: |-\n\n    a\n"), "\na")

    def test_refuses_a_line_indented_less_than_the_block(self):
        # Slicing it would walk past the whitespace and delete a real
        # character — wrong SQL in the mirror rather than an error.
        with self.assertRaises(ds.ExtractionError) as cm:
            self.read("        k: |-\n          SELECT 1\n         FROM t\n")
        self.assertIn("indented less", str(cm.exception))

    def test_refuses_a_tab_in_the_indentation(self):
        with self.assertRaises(ds.ExtractionError) as cm:
            self.read("  k: |-\n    a\n\tb\n")
        self.assertIn("tab", str(cm.exception))


class AlertExtraction(unittest.TestCase):
    RULES = """apiVersion: 1
groups:
- folder: 'Dropset'
  name: 'g'
  rules:
  - annotations:
      summary: 'one'
    data:
    - model:
        rawSql: |-
          SELECT 1
          FROM t
        refId: 'A'
    title: 'First'
    uid: 'rule-one'
  - data:
    - model:
        rawSql: |-
          SELECT 2
    title: 'Second'
    uid: 'rule-two'
"""

    def parse(self, text, name="maker.yml"):
        with tempfile.TemporaryDirectory() as tmp:
            p = pathlib.Path(tmp) / name
            p.write_text(text)
            return ds.parse_alerting(p)

    def test_keys_each_query_on_its_rule_uid(self):
        self.assertEqual(
            [rel for rel, _ in self.parse(self.RULES)],
            ["alerting/rule-one.sql", "alerting/rule-two.sql"],
        )

    def test_extracts_the_query_text(self):
        self.assertEqual(self.parse(self.RULES)[0][1], "SELECT 1\nFROM t")

    def test_reads_the_uid_despite_it_sorting_after_the_query(self):
        # The house yamllint rule orders keys alphabetically, so `uid` always
        # arrives after `data`. A single forward pass that keyed on the most
        # recent uid would key every rule on the PREVIOUS rule's uid.
        self.assertEqual(self.parse(self.RULES)[1][0], "alerting/rule-two.sql")

    def test_ignores_a_uid_nested_deeper_than_the_rule(self):
        text = self.RULES.replace(
            "    - model:\n        rawSql: |-\n          SELECT 1",
            "    - datasource:\n        uid: 'dropset-postgres'\n"
            "      model:\n        rawSql: |-\n          SELECT 1",
        )
        self.assertEqual(self.parse(text)[0][0], "alerting/rule-one.sql")

    def test_numbers_multiple_queries_in_one_rule(self):
        text = self.RULES.replace(
            "        refId: 'A'\n",
            "        refId: 'A'\n    - model:\n        rawSql: |-\n"
            "          SELECT 99\n",
        )
        self.assertEqual(
            [rel for rel, _ in self.parse(text)][:2],
            ["alerting/rule-one-1.sql", "alerting/rule-one-2.sql"],
        )

    def test_comments_between_rules_do_not_end_the_rule_list(self):
        # The committed files comment every rule's thresholds, so a comment at
        # or above the rule indent is the NORMAL case, not an edge one. Treating
        # one as a sibling key ended the list before the first rule and yielded
        # nothing — a silent empty extraction, which the mirror gate then reads
        # as "there is no alert SQL" rather than as a failure.
        text = self.RULES.replace(
            "  rules:\n", "  rules:\n  # why this rule fires\n  #\n  # continued\n"
        )
        self.assertEqual(
            [rel for rel, _ in self.parse(text)],
            ["alerting/rule-one.sql", "alerting/rule-two.sql"],
        )

    def test_a_following_group_ends_the_rule_list(self):
        text = (
            self.RULES
            + """- folder: 'Other'
  name: 'g2'
  rules:
  - data:
    - model:
        rawSql: |-
          SELECT 3
    uid: 'rule-three'
"""
        )
        self.assertEqual(
            [rel for rel, _ in self.parse(text)],
            [
                "alerting/rule-one.sql",
                "alerting/rule-two.sql",
                "alerting/rule-three.sql",
            ],
        )

    def test_refuses_a_rawSql_that_is_not_a_literal_block(self):
        # THE SILENT-HOLE CASE. A quoted single-line scalar was skipped with no
        # error and no mirror file, so `check` stayed green and the query was
        # never linted — the exact shape the module docstring argues against.
        for form in ("'SELECT 1'", "|2-", "|- # inline comment"):
            with self.subTest(form=form):
                text = self.RULES.replace(
                    "rawSql: |-\n          SELECT 1", f"rawSql: {form}", 1
                )
                with self.assertRaises(ds.ExtractionError) as cm:
                    self.parse(text)
                self.assertIn("literal block", str(cm.exception))

    def test_a_query_containing_rules_does_not_fabricate_a_boundary(self):
        # The boundary scan must treat a block-scalar BODY as opaque. A line
        # inside the SQL reading `rules:` or a `- ` at the list's own column
        # would otherwise open or close a rule, silently pairing the wrong uid
        # with the SQL rather than failing.
        text = self.RULES.replace(
            "          SELECT 1\n          FROM t",
            "          SELECT 1\n"
            "          -- rules:\n"
            "          -- - not a rule\n"
            "          FROM t",
        )
        out = self.parse(text)
        self.assertEqual(
            [rel for rel, _ in out], ["alerting/rule-one.sql", "alerting/rule-two.sql"]
        )
        self.assertIn("-- rules:", out[0][1])

    def test_a_sibling_sequence_at_the_rule_indent_closes_the_list(self):
        # Pins the reset branch, which nothing else reaches. A following GROUP
        # cannot pin it — that dash sits at indent 0, so it is skipped whether
        # or not the list was closed — and a sibling sequence whose items carry
        # no query cannot either, because a spurious rule with no SQL is
        # silently dropped rather than surfacing. So the items here carry a
        # `rawSql` AND a `uid`: without the reset they are read as rules and
        # their SQL is extracted under their uid, which is the wrong pairing
        # the branch exists to prevent.
        #
        # Deliberately contrived — the real provisioning format has no such
        # sequence at group level — because the branch is defensive. Verified
        # by mutation: replacing `rules_indent = None` with a no-op makes this
        # fail and leaves every other test green.
        text = (
            self.RULES
            + """  notifications:
  - uid: 'not-a-rule'
    data:
    - model:
        rawSql: |-
          SELECT 99
"""
        )
        self.assertEqual(
            [rel for rel, _ in self.parse(text)],
            ["alerting/rule-one.sql", "alerting/rule-two.sql"],
        )

    def test_reads_a_double_quoted_and_an_unquoted_uid(self):
        for spelling, want in (
            ('"rule-one"', "alerting/rule-one.sql"),
            ("rule-one", "alerting/rule-one.sql"),
            ("rule-one # legacy", "alerting/rule-one.sql"),
        ):
            with self.subTest(spelling=spelling):
                text = self.RULES.replace("uid: 'rule-one'", f"uid: {spelling}", 1)
                self.assertEqual(self.parse(text)[0][0], want)

    def test_refuses_a_folded_scalar(self):
        with self.assertRaises(ds.ExtractionError) as cm:
            self.parse(self.RULES.replace("rawSql: |-", "rawSql: >-", 1))
        self.assertIn("folded", str(cm.exception))

    def test_refuses_a_rule_with_sql_but_no_uid(self):
        with self.assertRaises(ds.ExtractionError) as cm:
            self.parse(self.RULES.replace("    uid: 'rule-one'\n", "", 1))
        self.assertIn("First", str(cm.exception))

    def test_a_rule_without_sql_is_skipped_not_refused(self):
        text = """apiVersion: 1
groups:
- rules:
  - title: 'No query'
    uid: 'rule-none'
"""
        self.assertEqual(self.parse(text), [])


class BareVarGuard(unittest.TestCase):
    """The guard that keeps the 25th bare variable from landing.

    24 sites were converted by hand; a one-time sweep is not a gate.
    """

    def test_rejects_a_bare_variable_inside_a_literal(self):
        with self.assertRaises(ds.ExtractionError) as cm:
            ds.check_bare_var_in_literal("SELECT 1 WHERE s = '$venue_source'", "where")
        self.assertIn("sqlstring", str(cm.exception))

    def test_rejects_the_braced_form_too(self):
        with self.assertRaises(ds.ExtractionError):
            ds.check_bare_var_in_literal("SELECT 1 WHERE s = '${granularity}'", "where")

    def test_allows_the_sqlstring_form_the_message_recommends(self):
        # The gate must not block its own remedy.
        ds.check_bare_var_in_literal(
            "SELECT 1 WHERE s = ${venue_source:sqlstring}", "w"
        )
        ds.check_bare_var_in_literal(
            "SELECT 1 WHERE s = ANY (ARRAY[${pairs:sqlstring}]::text[])", "w"
        )

    def test_allows_a_bare_variable_outside_a_literal(self):
        # The six deliberately-unquoted numeric sites must keep passing.
        ds.check_bare_var_in_literal(
            "SELECT 1 WHERE granularity_secs = $granularity", "w"
        )

    def test_ignores_a_grafana_macro_in_a_literal(self):
        # `$__`-prefixed macros are datasource-substituted, not user values.
        ds.check_bare_var_in_literal("SELECT '$__timeFrom()'", "where")

    def test_ignores_a_variable_named_in_a_comment(self):
        ds.check_bare_var_in_literal(
            "-- do not write '$source' here\nSELECT 1", "where"
        )

    def test_the_committed_dashboards_pass_the_new_guard(self):
        # The whole point: the tree is clean today, so the guard is a floor
        # rather than a wish. This would have failed before the conversion.
        root = pathlib.Path(__file__).resolve().parents[3]
        ds.collect(root / ds.DASHBOARD_DIR, root / ds.ALERTING_DIR)


class DuplicateUid(unittest.TestCase):
    """`ALERTING_DIR`'s comment claims the duplicate guard enforces uniqueness.

    It was claim-only: nothing constructed two rules sharing a uid, so the one
    guard standing between a copy-pasted uid and one rule's SQL silently
    overwriting another's mirror file was untested.
    """

    #: `rules:` sits on its own line, never on the group's dash line. The scan
    #: keys on a line whose whole content is `rules:`, and the house yamllint
    #: rule sorts group keys alphabetically — `folder`, `interval`, `name`,
    #: `orgId` all precede `rules` — so `rules` can never lead a group item.
    RULE = """apiVersion: 1
groups:
- name: 'g'
  rules:
  - data:
    - model:
        rawSql: |-
          SELECT {n}
    uid: 'same-uid'
"""

    def test_two_rules_sharing_a_uid_are_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = pathlib.Path(tmp)
            body = (
                self.RULE.format(n=1)
                + """  - data:
    - model:
        rawSql: |-
          SELECT 2
    uid: 'same-uid'
"""
            )
            (d / "maker.yml").write_text(body)
            with self.assertRaises(ds.ExtractionError) as cm:
                ds.collect(d / "no-dashboards", d)
            self.assertIn("same-uid", str(cm.exception))

    def test_a_uid_reused_across_two_files_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = pathlib.Path(tmp)
            (d / "a.yml").write_text(self.RULE.format(n=1))
            (d / "b.yml").write_text(self.RULE.format(n=2))
            with self.assertRaises(ds.ExtractionError) as cm:
                ds.collect(d / "no-dashboards", d)
            self.assertIn("same-uid", str(cm.exception))


class RealAlerting(unittest.TestCase):
    """The stdlib reader must agree with PyYAML on the committed alerting files.

    This is what licenses reading YAML without a YAML library. The tool cannot
    import PyYAML — the `dashboard-sql-lint` hook runs in an environment that has
    only sqlfluff — but the SUITE runs under the ambient interpreter, so the
    equivalence can be checked here even though it cannot be relied on there.

    The day a rule uses a YAML feature the subset reader does not cover, this
    fails rather than the mirror going quietly wrong.
    """

    #: `.claude/tools/tests/` -> repo root.
    REPO_ROOT = pathlib.Path(__file__).resolve().parents[3]

    def files(self):
        d = self.REPO_ROOT / ds.ALERTING_DIR
        self.assertTrue(d.is_dir(), f"{d} is missing — this test cannot skip")
        found = sorted(d.glob("*.yml"))
        self.assertTrue(found, f"no alerting files under {d}")
        return found

    def test_agrees_with_pyyaml_on_every_committed_rule(self):
        try:
            yaml = __import__("yaml")
        except ImportError as e:  # pragma: no cover - depends on the environment
            # Deliberately NOT skipped: this test is what licenses reading YAML
            # without a YAML library, so it must not go quiet. But name the
            # requirement, or the failure reads as the stdlib-only TOOL having
            # a dependency it does not have.
            raise AssertionError(
                "this SUITE needs PyYAML to cross-validate the stdlib YAML "
                "reader against a real parser; the TOOL itself does not import "
                "it, and must not. Install PyYAML to run this check."
            ) from e

        for path in self.files():
            doc = yaml.safe_load(path.read_text())
            expected = []
            for group in doc.get("groups") or []:
                for rule in group.get("rules") or []:
                    sql = [
                        d["model"]["rawSql"]
                        for d in rule.get("data") or []
                        if isinstance(d.get("model"), dict)
                        and str(d["model"].get("rawSql", "")).strip()
                    ]
                    for idx, q in enumerate(sql):
                        suffix = f"-{idx + 1}" if len(sql) > 1 else ""
                        expected.append(
                            (f"alerting/{ds.slug(rule['uid'], 'rule')}{suffix}.sql", q)
                        )

            # Without this, a file whose top-level shape changed such that
            # `groups` went falsy yields `expected == []`, and a reader
            # returning [] compares equal — the test that licenses the whole
            # design passing while extracting nothing.
            self.assertTrue(expected, f"{path.name}: PyYAML found no rules")

            self.assertEqual(
                ds.parse_alerting(path),
                expected,
                f"{path.name}: the stdlib reader and PyYAML disagree",
            )

    def test_the_committed_alert_rules_extract_cleanly(self):
        found = ds.collect(
            self.REPO_ROOT / ds.DASHBOARD_DIR, self.REPO_ROOT / ds.ALERTING_DIR
        )
        alerts = [r for r in found if r.startswith("alerting/")]
        # A floor rather than an exact count, matching `RealDashboards`: exact
        # churns on every rule added, non-emptiness passes if the walk finds
        # one rule out of six. Six, not five — the committed set is exactly six
        # rules with one query each, so a floor of five passes with a rule
        # silently missing, and six stays monotone as rules are added.
        self.assertGreaterEqual(
            len(alerts), 6, f"only {len(alerts)} alert queries extracted — walk broken?"
        )


if __name__ == "__main__":
    unittest.main()
