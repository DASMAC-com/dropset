#!/usr/bin/env python3
# cspell:word justaname
"""Unit tests for localnet_psql.py.

The argv assembly is pure, so it is tested directly — no container, no database.
The runner itself is exercised with ``subprocess.run`` patched, which is the only
seam that touches the outside world.
"""

from __future__ import annotations

import io
import os
import subprocess
import sys
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import localnet_psql as lp  # noqa: E402
from localnet_psql import LocalnetPsqlError, build_argv, run  # noqa: E402


def _argv(**kwargs):
    base = dict(
        sql="select 1",
        file=None,
        variables=[],
        direct=False,
        aligned=False,
        tuples_only=False,
        container=lp.DEFAULT_CONTAINER,
        db_url=None,
    )
    base.update(kwargs)
    return build_argv(**base)


class BuildArgvTests(unittest.TestCase):
    def test_the_default_is_a_docker_exec_into_the_localnet_container(self):
        got = _argv()
        self.assertEqual(got[:4], ["docker", "exec", "-i", lp.DEFAULT_CONTAINER])
        self.assertIn("psql", got)
        self.assertIn("-U", got)

    def test_unaligned_output_is_the_default(self):
        # psql's aligned default draws box borders and pads every column — pure
        # formatting bytes, replayed on every later turn.
        got = _argv()
        self.assertIn("-A", got)
        self.assertIn(lp.FIELD_SEPARATOR, got)

    def test_aligned_opts_back_into_the_table_form(self):
        got = _argv(aligned=True)
        self.assertNotIn("-A", got)
        self.assertNotIn(lp.FIELD_SEPARATOR, got)

    def test_the_pager_is_disabled_so_a_call_cannot_hang(self):
        got = _argv()
        self.assertIn("pager=off", got)

    def test_errors_stop_the_script_rather_than_continuing(self):
        self.assertIn("ON_ERROR_STOP=1", _argv())

    def test_tuples_only_drops_the_header(self):
        self.assertIn("-t", _argv(tuples_only=True))
        self.assertNotIn("-t", _argv())

    def test_a_statement_is_passed_with_dash_c(self):
        got = _argv(sql="select count(*) from candles")
        self.assertEqual(got[-2:], ["-c", "select count(*) from candles"])

    def test_a_file_is_passed_with_dash_f(self):
        got = _argv(sql=None, file="q.sql")
        self.assertEqual(got[-2:], ["-f", "q.sql"])

    def test_variables_are_forwarded_in_order(self):
        got = _argv(variables=["source=coinbase", "product_id=EURC-USDC"])
        self.assertIn("source=coinbase", got)
        self.assertIn("product_id=EURC-USDC", got)
        self.assertLess(got.index("source=coinbase"), got.index("product_id=EURC-USDC"))

    def test_a_malformed_variable_is_refused(self):
        with self.assertRaises(LocalnetPsqlError) as caught:
            _argv(variables=["justaname"])
        self.assertIn("name=value", str(caught.exception))

    def test_direct_mode_uses_the_url_and_no_docker(self):
        got = _argv(direct=True, db_url="postgres://x/y")
        self.assertEqual(got[0], "psql")
        self.assertIn("postgres://x/y", got)
        self.assertNotIn("docker", got)

    def test_the_connection_string_goes_last_after_every_flag(self):
        # As a LEADING positional this relied on getopt argument permutation,
        # which POSIXLY_CORRECT disables — psql would then read the flags as
        # connection parameters. Trailing is unconditionally correct.
        got = _argv(direct=True, db_url="postgres://x/y", sql="select 1")
        self.assertEqual(got[-1], "postgres://x/y")

    def test_the_fixed_error_stop_guard_cannot_be_overridden_by_a_var(self):
        # psql honors the LAST -v for a name, so the tool's own guard must come
        # after the caller's pairs or `--var ON_ERROR_STOP=0` silently wins.
        got = _argv(variables=["ON_ERROR_STOP=0"])
        self.assertLess(got.index("ON_ERROR_STOP=0"), got.index("ON_ERROR_STOP=1"))

    def test_an_empty_sql_value_is_diagnosed_as_empty_not_as_missing(self):
        with self.assertRaises(LocalnetPsqlError) as caught:
            _argv(sql="", file=None)
        self.assertIn("empty value", str(caught.exception))

    def test_direct_mode_without_a_url_is_refused(self):
        with self.assertRaises(LocalnetPsqlError) as caught:
            _argv(direct=True, db_url=None)
        self.assertIn("DROPSET_DB_URL", str(caught.exception))

    def test_both_sql_and_file_is_refused(self):
        with self.assertRaises(LocalnetPsqlError):
            _argv(sql="select 1", file="q.sql")

    def test_neither_sql_nor_file_is_refused(self):
        with self.assertRaises(LocalnetPsqlError):
            _argv(sql=None, file=None)

    def test_the_container_is_overridable(self):
        self.assertIn("other-pg", _argv(container="other-pg"))


class CapTests(unittest.TestCase):
    def test_output_within_the_cap_is_untouched(self):
        text, dropped = lp._cap("a\nb\n", 5)
        self.assertEqual(text, "a\nb")
        self.assertEqual(dropped, 0)

    def test_output_past_the_cap_reports_what_was_dropped(self):
        text, dropped = lp._cap("\n".join("abcdef"), 2)
        self.assertEqual(text, "a\nb")
        self.assertEqual(dropped, 4)

    def test_a_zero_cap_means_no_cap(self):
        text, dropped = lp._cap("\n".join("abcdef"), 0)
        self.assertEqual(dropped, 0)
        self.assertIn("f", text)


class RunTests(unittest.TestCase):
    def _completed(self, stdout="", stderr="", code=0):
        return subprocess.CompletedProcess([], code, stdout, stderr)

    def _invoke(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = run(["localnet_psql.py", *argv])
        return code, out.getvalue(), err.getvalue()

    def test_a_successful_query_prints_its_rows(self):
        with mock.patch.object(
            subprocess, "run", return_value=self._completed("count\n3\n")
        ):
            code, out, _ = self._invoke("--sql", "select count(*) from t")
        self.assertEqual(code, 0)
        self.assertIn("3", out)

    def test_a_capped_result_announces_the_drop_on_stderr(self):
        rows = "\n".join(str(k) for k in range(100))
        with mock.patch.object(subprocess, "run", return_value=self._completed(rows)):
            code, out, err = self._invoke("--sql", "select x", "--max-rows", "5")
        self.assertEqual(code, 0)
        self.assertIn("NOT shown", err)
        self.assertNotIn("99", out)

    def test_count_reports_the_row_count_and_not_the_rows(self):
        # A distinctive marker, not the letter "a": asserting on a single letter
        # passed for the wrong reason the moment the summary wording changed.
        rows = "ROW-MARKER-1\nROW-MARKER-2\nROW-MARKER-3\n"
        with mock.patch.object(subprocess, "run", return_value=self._completed(rows)):
            code, out, _ = self._invoke("--sql", "select x", "--count")
        self.assertEqual(code, 0)
        self.assertIn("3 row(s)", out)
        self.assertNotIn("ROW-MARKER", out)

    def test_count_implies_tuples_only_so_the_number_means_rows(self):
        # Without -t, psql emits a header and a `(N rows)` footer, so counting
        # non-blank lines answered "how many rows" with N+2.
        seen = {}

        def fake(command, **kwargs):
            seen["command"] = command
            return self._completed("1\n2\n3\n")

        with mock.patch.object(subprocess, "run", side_effect=fake):
            self._invoke("--sql", "select x", "--count")
        self.assertIn("-t", seen["command"])

    def test_a_failed_query_surfaces_the_stderr_tail(self):
        with mock.patch.object(
            subprocess,
            "run",
            return_value=self._completed("", 'ERROR: relation "t" does not exist', 1),
        ):
            with self.assertRaises(LocalnetPsqlError) as caught:
                self._invoke("--sql", "select * from t")
        self.assertIn("does not exist", str(caught.exception))

    def test_a_timeout_is_a_clean_error(self):
        with mock.patch.object(
            subprocess, "run", side_effect=subprocess.TimeoutExpired("psql", 60)
        ):
            with self.assertRaises(LocalnetPsqlError) as caught:
                self._invoke("--sql", "select pg_sleep(99)")
        self.assertIn("did not finish", str(caught.exception))

    def test_a_missing_docker_binary_is_a_clean_error(self):
        with mock.patch.object(subprocess, "run", side_effect=FileNotFoundError("no")):
            with self.assertRaises(LocalnetPsqlError) as caught:
                self._invoke("--sql", "select 1")
        self.assertIn("cannot run", str(caught.exception))

    def test_the_container_env_override_reaches_the_command(self):
        seen = {}

        def fake(command, **kwargs):
            seen["command"] = command
            return self._completed("ok\n")

        with mock.patch.dict(
            os.environ, {"DROPSET_LOCALNET_PG_CONTAINER": "custom-pg"}
        ):
            with mock.patch.object(subprocess, "run", side_effect=fake):
                self._invoke("--sql", "select 1")
        self.assertIn("custom-pg", seen["command"])

    def test_an_empty_container_override_falls_back_to_the_default(self):
        seen = {}

        def fake(command, **kwargs):
            seen["command"] = command
            return self._completed("ok\n")

        with mock.patch.dict(os.environ, {"DROPSET_LOCALNET_PG_CONTAINER": "  "}):
            with mock.patch.object(subprocess, "run", side_effect=fake):
                self._invoke("--sql", "select 1")
        self.assertIn(lp.DEFAULT_CONTAINER, seen["command"])


if __name__ == "__main__":
    unittest.main()
