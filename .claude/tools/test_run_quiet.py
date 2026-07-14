#!/usr/bin/env python3
"""Unit tests for ``run_quiet.py`` (stdlib ``unittest``; no pytest)."""

from __future__ import annotations

import io
import sys
import unittest
from contextlib import redirect_stderr, redirect_stdout

import run_quiet as rq

PY = sys.executable


class ParseArgs(unittest.TestCase):
    def test_defaults(self):
        self.assertEqual(
            rq.parse_args(["--", "make", "lint"]),
            (rq.DEFAULT_TAIL, None, ["make", "lint"]),
        )

    def test_tail_and_label_spaced(self):
        self.assertEqual(
            rq.parse_args(["--tail", "5", "--label", "x", "--", "make", "lint"]),
            (5, "x", ["make", "lint"]),
        )

    def test_tail_and_label_equals(self):
        self.assertEqual(
            rq.parse_args(["--tail=7", "--label=build", "--", "cargo", "test"]),
            (7, "build", ["cargo", "test"]),
        )

    def test_options_after_separator_belong_to_command(self):
        # `--tail` after `--` is the command's argument, not the wrapper's.
        self.assertEqual(
            rq.parse_args(["--", "tool", "--tail", "9"]),
            (rq.DEFAULT_TAIL, None, ["tool", "--tail", "9"]),
        )

    def test_empty_argv_errors(self):
        with self.assertRaises(rq.UsageError):
            rq.parse_args([])

    def test_missing_separator_errors(self):
        with self.assertRaises(rq.UsageError):
            rq.parse_args(["make", "lint"])

    def test_empty_command_errors(self):
        with self.assertRaises(rq.UsageError):
            rq.parse_args(["--"])

    def test_missing_tail_value_errors(self):
        with self.assertRaises(rq.UsageError):
            rq.parse_args(["--tail"])

    def test_non_integer_tail_errors(self):
        with self.assertRaises(rq.UsageError):
            rq.parse_args(["--tail", "abc", "--", "x"])

    def test_negative_tail_errors(self):
        with self.assertRaises(rq.UsageError):
            rq.parse_args(["--tail", "-1", "--", "x"])

    def test_unknown_option_errors(self):
        with self.assertRaises(rq.UsageError):
            rq.parse_args(["--bogus", "--", "x"])


class Sanitize(unittest.TestCase):
    def test_plain(self):
        self.assertEqual(rq.sanitize(["make", "lint"]), "make-lint")

    def test_special_chars_collapse(self):
        self.assertEqual(rq.sanitize(["./x", "a b/c"]), "x-a-b-c")

    def test_all_special_falls_back(self):
        self.assertEqual(rq.sanitize(["///"]), "cmd")

    def test_truncates_long_input(self):
        self.assertLessEqual(len(rq.sanitize(["a" * 200])), 40)


class ReadTailAndCount(unittest.TestCase):
    def _write_lines(self, n):
        path = rq.os.path.join(rq.LOG_DIR, "test-count-%d.log" % rq.os.getpid())
        rq.os.makedirs(rq.LOG_DIR, exist_ok=True)
        with open(path, "w", encoding="utf-8") as fh:
            for k in range(n):
                fh.write("line %d\n" % k)
        return path

    def test_counts_all_and_tails_last(self):
        path = self._write_lines(100)
        total, tail_text, failed, truncated = rq.read_tail_and_count(path, 10)
        self.assertEqual(total, 100)
        self.assertEqual(tail_text.count("\n"), 10)
        self.assertIn("line 99", tail_text)
        self.assertNotIn("line 89", tail_text)
        self.assertEqual(failed, [])
        self.assertFalse(truncated)

    def test_zero_tail_keeps_no_text(self):
        path = self._write_lines(5)
        total, tail_text, failed, truncated = rq.read_tail_and_count(path, 0)
        self.assertEqual(total, 5)
        self.assertEqual(tail_text, "")
        self.assertEqual(failed, [])
        self.assertFalse(truncated)

    def _write_raw(self, lines):
        path = rq.os.path.join(rq.LOG_DIR, "test-raw-%d.log" % rq.os.getpid())
        rq.os.makedirs(rq.LOG_DIR, exist_ok=True)
        with open(path, "w", encoding="utf-8") as fh:
            fh.write("".join(ln + "\n" for ln in lines))
        return path

    def test_collects_failed_hook_lines_beyond_tail(self):
        # A failing hook near the top must be surfaced even with a tiny tail.
        lines = ["yamllint" + "." * 30 + "Failed"]
        lines += ["detail %d" % k for k in range(60)]
        path = self._write_raw(lines)
        _, _, failed, truncated = rq.read_tail_and_count(path, 5)
        self.assertEqual(len(failed), 1)
        self.assertTrue(failed[0].endswith("Failed"))
        self.assertFalse(truncated)

    def test_passed_lines_are_not_collected(self):
        path = self._write_raw(["cspell" + "." * 10 + "Passed", "all good"])
        _, _, failed, truncated = rq.read_tail_and_count(path, 5)
        self.assertEqual(failed, [])
        self.assertFalse(truncated)

    def test_failed_lines_capped_and_flagged_truncated(self):
        # More than MAX failed lines: list is capped AND truncated is set.
        path = self._write_raw(["h%d.....Failed" % k for k in range(100)])
        _, _, failed, truncated = rq.read_tail_and_count(path, 5)
        self.assertEqual(len(failed), rq.MAX_FAILED_LINES)
        self.assertTrue(truncated)

    def test_exactly_max_failed_lines_is_not_truncated(self):
        # Exactly MAX failures fill the list but nothing was omitted.
        path = self._write_raw(
            ["h%d.....Failed" % k for k in range(rq.MAX_FAILED_LINES)]
        )
        _, _, failed, truncated = rq.read_tail_and_count(path, 5)
        self.assertEqual(len(failed), rq.MAX_FAILED_LINES)
        self.assertFalse(truncated)


class Run(unittest.TestCase):
    def _run(self, tail, label, cmd):
        """Run, capturing stdout/stderr; return (exit_code, stdout, stderr)."""
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = rq.run(tail, label, cmd)
        return code, out.getvalue(), err.getvalue()

    def test_success_prints_one_summary_line(self):
        code, out, _ = self._run(
            rq.DEFAULT_TAIL, "ok", [PY, "-c", "print('hello world')"]
        )
        self.assertEqual(code, 0)
        self.assertTrue(out.startswith("✓ ok (exit 0,"))
        self.assertIn("log:", out)
        # No failure tail on success.
        self.assertNotIn("--- last", out)

    def test_failure_propagates_exit_code_and_shows_tail(self):
        code, out, _ = self._run(
            rq.DEFAULT_TAIL,
            "fail",
            [PY, "-c", "print('boom'); import sys; sys.exit(3)"],
        )
        self.assertEqual(code, 3)
        self.assertTrue(out.startswith("✗ fail (exit 3,"))
        self.assertIn("--- last", out)
        self.assertIn("boom", out)

    def test_failure_surfaces_failed_hook_index(self):
        code, out, _ = self._run(
            rq.DEFAULT_TAIL,
            "lint",
            [PY, "-c", "print('yamllint....Failed'); import sys; sys.exit(1)"],
        )
        self.assertEqual(code, 1)
        self.assertIn("--- failed hooks (1) ---", out)
        self.assertIn("yamllint....Failed", out)
        # A small, uncapped index carries no truncation marker.
        self.assertNotIn("truncated", out)

    def test_failure_index_marks_truncation_when_capped(self):
        # More than MAX failed-hook lines → the index is capped and labeled.
        n = rq.MAX_FAILED_LINES + 5
        code, out, _ = self._run(
            rq.DEFAULT_TAIL,
            "lint",
            [
                PY,
                "-c",
                "import sys\nfor i in range(%d): print('h%%d....Failed' %% i)\n"
                "sys.exit(1)" % n,
            ],
        )
        self.assertEqual(code, 1)
        self.assertIn("--- failed hooks (%d) (truncated" % rq.MAX_FAILED_LINES, out)

    def test_label_defaults_to_joined_command(self):
        code, out, _ = self._run(rq.DEFAULT_TAIL, None, [PY, "-c", "pass"])
        self.assertEqual(code, 0)
        self.assertIn(PY, out)

    def test_missing_binary_maps_to_launch_failure(self):
        code, _, err = self._run(
            rq.DEFAULT_TAIL, None, ["this-binary-does-not-exist-zzz"]
        )
        self.assertEqual(code, rq.LAUNCH_FAILURE_CODE)
        self.assertIn("command not found", err)


class MainCli(unittest.TestCase):
    def test_usage_error_returns_2(self):
        err = io.StringIO()
        with redirect_stderr(err):
            code = rq.main(["no-separator"])
        self.assertEqual(code, 2)
        self.assertIn("usage:", err.getvalue())

    def test_main_runs_and_propagates(self):
        out = io.StringIO()
        with redirect_stdout(out):
            code = rq.main(["--label", "via-main", "--", PY, "-c", "pass"])
        self.assertEqual(code, 0)
        self.assertIn("via-main", out.getvalue())


if __name__ == "__main__":
    unittest.main()
