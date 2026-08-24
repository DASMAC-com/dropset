#!/usr/bin/env python3
# cspell:word kwyjibo
# cspell:word loneword
"""Unit tests for ``run_quiet.py`` (stdlib ``unittest``; no pytest)."""

from __future__ import annotations

import io
import os
import re
import sys
import tempfile
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
        got = rq.read_tail_and_count(path, 10)
        self.assertEqual(got.lines, 100)
        self.assertEqual(got.tail_text.count("\n"), 10)
        self.assertIn("line 99", got.tail_text)
        self.assertNotIn("line 89", got.tail_text)
        self.assertEqual(got.failed, [])
        self.assertFalse(got.truncated)

    def test_zero_tail_keeps_no_text(self):
        path = self._write_lines(5)
        got = rq.read_tail_and_count(path, 0)
        self.assertEqual(got.lines, 5)
        self.assertEqual(got.tail_text, "")
        self.assertEqual(got.failed, [])
        self.assertFalse(got.truncated)

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
        got = rq.read_tail_and_count(path, 5)
        self.assertEqual(len(got.failed), 1)
        self.assertTrue(got.failed[0].endswith("Failed"))
        self.assertFalse(got.truncated)

    def test_passed_lines_are_not_collected(self):
        path = self._write_raw(["cspell" + "." * 10 + "Passed", "all good"])
        got = rq.read_tail_and_count(path, 5)
        self.assertEqual(got.failed, [])
        self.assertFalse(got.truncated)

    def test_failed_lines_capped_and_flagged_truncated(self):
        # More than MAX failed lines: list is capped AND truncated is set.
        path = self._write_raw(["h%d.....Failed" % k for k in range(100)])
        got = rq.read_tail_and_count(path, 5)
        self.assertEqual(len(got.failed), rq.MAX_FAILED_LINES)
        self.assertTrue(got.truncated)

    def test_exactly_max_failed_lines_is_not_truncated(self):
        # Exactly MAX failures fill the list but nothing was omitted.
        path = self._write_raw(
            ["h%d.....Failed" % k for k in range(rq.MAX_FAILED_LINES)]
        )
        got = rq.read_tail_and_count(path, 5)
        self.assertEqual(len(got.failed), rq.MAX_FAILED_LINES)
        self.assertFalse(got.truncated)


class UnknownWords(unittest.TestCase):
    """The cspell index.

    cspell chunks the tree, so the failure detail is routinely *outside* the tail
    window while a later passing chunk sits inside it. These pin that the offender
    is recovered from anywhere in the log.
    """

    def _write_raw(self, lines):
        path = rq.os.path.join(rq.LOG_DIR, "test-words-%d.log" % rq.os.getpid())
        rq.os.makedirs(rq.LOG_DIR, exist_ok=True)
        with open(path, "w", encoding="utf-8") as fh:
            fh.write("".join(ln + "\n" for ln in lines))
        return path

    def test_parses_word_and_location(self):
        got = rq.parse_unknown_word("docs/foo.md:12:5 - Unknown word (kwyjibo)")
        self.assertEqual(got, ("kwyjibo", "docs/foo.md:12:5"))

    def test_parses_a_forbidden_word_the_same_way(self):
        got = rq.parse_unknown_word("a.md:1:1 - Forbidden word (teh)")
        self.assertEqual(got[0], "teh")

    def test_a_line_without_an_offender_is_none(self):
        self.assertIsNone(rq.parse_unknown_word("Issues found: 0 in 0 files"))

    def test_a_location_less_line_still_yields_the_word(self):
        got = rq.parse_unknown_word("Unknown word (loneword)")
        self.assertEqual(got, ("loneword", ""))

    def test_the_offender_is_found_far_above_the_tail_window(self):
        # The exact shape that cost one session four follow-up greps: the real
        # failure in an early chunk, a passing chunk's summary in the tail.
        lines = ["docs/a.md:3:1 - Unknown word (kwyjibo)"]
        lines += ["chunk %d ok" % k for k in range(80)]
        lines += ["Issues found: 0 in 0 files"]
        got = rq.read_tail_and_count(self._write_raw(lines), 5)
        self.assertEqual(got.unknown_words, [("kwyjibo", "docs/a.md:3:1")])
        self.assertNotIn("kwyjibo", got.tail_text)

    def test_repeats_of_one_word_collapse_to_the_first_location(self):
        lines = [
            "a.md:1:1 - Unknown word (dupe)",
            "b.md:9:9 - Unknown word (dupe)",
        ]
        got = rq.read_tail_and_count(self._write_raw(lines), 5)
        self.assertEqual(got.unknown_words, [("dupe", "a.md:1:1")])

    def test_distinct_words_keep_encounter_order(self):
        lines = [
            "a.md:1:1 - Unknown word (zeta)",
            "a.md:2:1 - Unknown word (alpha)",
        ]
        got = rq.read_tail_and_count(self._write_raw(lines), 5)
        self.assertEqual([w for w, _ in got.unknown_words], ["zeta", "alpha"])

    def test_words_are_capped_and_flagged_truncated(self):
        lines = [
            "a.md:%d:1 - Unknown word (w%d)" % (k, k)
            for k in range(rq.MAX_UNKNOWN_WORDS + 5)
        ]
        got = rq.read_tail_and_count(self._write_raw(lines), 5)
        self.assertEqual(len(got.unknown_words), rq.MAX_UNKNOWN_WORDS)
        self.assertTrue(got.unknown_truncated)

    def test_a_clean_log_reports_no_words(self):
        got = rq.read_tail_and_count(self._write_raw(["all good"]), 5)
        self.assertEqual(got.unknown_words, [])
        self.assertFalse(got.unknown_truncated)


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
        # A small, uncapped index carries no truncation marker. This assertion
        # belongs HERE, where an index actually exists — inserting a new test
        # above it silently adopted it into a case that prints no index at all,
        # where it is trivially true.
        self.assertNotIn("truncated", out)

    def test_failure_surfaces_the_spelling_index_above_the_tail(self):
        script = (
            "print('cspell....Failed');"
            "print('docs/a.md:3:1 - Unknown word (kwyjibo)');"
            "print('Issues found: 0 in 0 files');"
            "import sys; sys.exit(1)"
        )
        code, out, _ = self._run(rq.DEFAULT_TAIL, "lint", [PY, "-c", script])
        self.assertEqual(code, 1)
        self.assertIn("--- unknown words (1) ---", out)
        self.assertIn("kwyjibo — docs/a.md:3:1", out)
        # The index has to precede the tail: the tail is the window that shows a
        # later passing chunk and reads as "nothing wrong here".
        self.assertLess(out.index("unknown words"), out.index("--- last"))

    def test_a_clean_failure_prints_no_spelling_block(self):
        code, out, _ = self._run(
            rq.DEFAULT_TAIL, "t", [PY, "-c", "print('boom'); import sys; sys.exit(1)"]
        )
        self.assertEqual(code, 1)
        self.assertNotIn("unknown words", out)
        # The tail is still shown — this asserts the spelling block is absent,
        # not that the failure output is.
        self.assertIn("boom", out)

    def test_an_offender_on_a_failed_hook_line_is_still_indexed(self):
        # The failed-hook and offender matchers are disjoint only by accident, so
        # the scan must not skip the word check for a hook line.
        script = (
            "print('a.md:1:1 - Unknown word (kwyjibo) ... Failed');"
            "import sys; sys.exit(1)"
        )
        code, out, _ = self._run(rq.DEFAULT_TAIL, "lint", [PY, "-c", script])
        self.assertEqual(code, 1)
        self.assertIn("kwyjibo", out)

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

    def test_lock_wait_line_is_echoed_and_noted_on_success(self):
        code, out, _ = self._run(
            rq.DEFAULT_TAIL,
            "blocked",
            [
                PY,
                "-c",
                "print('    Blocking waiting for file lock on build directory')",
            ],
        )
        self.assertEqual(code, 0)
        # Echoed live, ahead of the summary line.
        self.assertIn("⏳ Blocking waiting for file lock on build directory", out)
        self.assertLess(out.index("⏳"), out.index("✓ blocked"))
        # And recalled in the summary, so a slow green run explains itself.
        self.assertIn("waited on a cargo file lock", out)

    def test_lock_wait_noted_on_failure_too(self):
        code, out, _ = self._run(
            rq.DEFAULT_TAIL,
            "blocked-fail",
            [
                PY,
                "-c",
                "print('Blocking waiting for file lock on package cache');"
                " import sys; sys.exit(4)",
            ],
        )
        self.assertEqual(code, 4)
        self.assertIn("waited on a cargo file lock", out)

    def test_only_the_first_lock_wait_is_echoed(self):
        code, out, _ = self._run(
            rq.DEFAULT_TAIL,
            "twice",
            [
                PY,
                "-c",
                "print('Blocking waiting for file lock on A');"
                " print('Blocking waiting for file lock on B')",
            ],
        )
        self.assertEqual(code, 0)
        self.assertEqual(out.count("⏳"), 1)
        self.assertIn("lock on A", out)

    def test_ordinary_output_is_not_echoed(self):
        code, out, _ = self._run(
            rq.DEFAULT_TAIL, "quiet", [PY, "-c", "print('Compiling dropset v0.1.0')"]
        )
        self.assertEqual(code, 0)
        self.assertNotIn("⏳", out)
        self.assertNotIn("Compiling", out)
        self.assertNotIn("waited on a cargo file lock", out)


class SanitizeForEcho(unittest.TestCase):
    """The lock-wait echo is the one channel out of the capture, so what goes
    through it is scrubbed — any build script can emit a line carrying the
    marker, and it lands in the terminal AND the model's transcript."""

    def test_strips_ansi_escapes(self):
        got = rq.sanitize_for_echo("\x1b[2JBlocking waiting for file lock\x1b[0m\n")
        self.assertNotIn("\x1b", got)
        self.assertIn("Blocking waiting for file lock", got)

    def test_strips_carriage_returns_and_control_chars(self):
        got = rq.sanitize_for_echo("Blocking\r\x07 waiting\x00 for file lock\n")
        self.assertNotIn("\r", got)
        self.assertNotIn("\x07", got)
        self.assertNotIn("\x00", got)

    def test_truncates_an_unbounded_line(self):
        got = rq.sanitize_for_echo("Blocking waiting for file lock " + "x" * 5000)
        self.assertLessEqual(len(got), rq.MAX_ECHO_CHARS + 40)
        self.assertIn("truncated", got)

    def test_keeps_an_ordinary_line_intact(self):
        line = "    Blocking waiting for file lock on build directory\n"
        self.assertEqual(
            rq.sanitize_for_echo(line),
            "Blocking waiting for file lock on build directory",
        )


class EchoIsSanitized(unittest.TestCase):
    def _run(self, tail, label, cmd):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = rq.run(tail, label, cmd)
        return code, out.getvalue(), err.getvalue()

    def test_hostile_child_output_cannot_inject_escapes_or_a_fake_summary(self):
        # A build script that spoofs a clean summary after the marker.
        payload = (
            "Blocking waiting for file lock\\x1b[2J\\n"
            "\\u2713 make lint (exit 0, 12 lines; log: /tmp/fake.log)"
        )
        code, out, _ = self._run(
            rq.DEFAULT_TAIL, "hostile", [PY, "-c", f"print('{payload}')"]
        )
        self.assertEqual(code, 0)
        self.assertNotIn("\x1b", out)
        # The spoofed line is on the same physical line as the marker, so the
        # scrub keeps it inert text rather than letting it clear the screen.
        self.assertIn("⏳", out)
        # Exactly one real summary line, and it is ours.
        self.assertEqual(out.count("✓ hostile"), 1)


class IsLockWaitLine(unittest.TestCase):
    def test_matches_cargo_status(self):
        self.assertTrue(
            rq.is_lock_wait_line(
                "    Blocking waiting for file lock on build directory\n"
            )
        )

    def test_matches_package_cache_variant(self):
        self.assertTrue(
            rq.is_lock_wait_line("Blocking waiting for file lock on package cache")
        )

    def test_ignores_unrelated_lines(self):
        self.assertFalse(rq.is_lock_wait_line("   Compiling dropset v0.1.0\n"))
        self.assertFalse(rq.is_lock_wait_line("waiting for the file\n"))


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


class ParseInspectArgs(unittest.TestCase):
    def test_the_log_path_is_positional(self):
        got = rq.parse_inspect_args(["inspect", "/tmp/x.log"])
        self.assertEqual(got.path, "/tmp/x.log")
        self.assertIsNone(got.grep)
        self.assertFalse(got.failing)
        self.assertEqual(got.tail, rq.DEFAULT_TAIL)

    def test_both_flag_spellings_are_accepted(self):
        for argv in (
            ["inspect", "/tmp/x.log", "--grep", "boom", "--context", "2"],
            ["inspect", "/tmp/x.log", "--grep=boom", "--context=2"],
        ):
            with self.subTest(argv=argv):
                got = rq.parse_inspect_args(argv)
                self.assertEqual(got.grep, "boom")
                self.assertEqual(got.context, 2)

    def test_a_missing_path_is_a_usage_error(self):
        with self.assertRaises(rq.UsageError):
            rq.parse_inspect_args(["inspect", "--grep", "x"])

    def test_a_second_positional_is_refused_as_a_probable_glob(self):
        # A shell glob that expanded is the likeliest cause, and silently
        # inspecting the first of several is how a reader diagnoses the wrong
        # run's failure.
        with self.assertRaises(rq.UsageError) as caught:
            rq.parse_inspect_args(["inspect", "/tmp/a.log", "/tmp/b.log"])
        self.assertIn("glob", str(caught.exception))

    def test_an_unknown_option_is_refused(self):
        with self.assertRaises(rq.UsageError):
            rq.parse_inspect_args(["inspect", "/tmp/x.log", "--latest"])


class InspectLog(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = self._tmp.name

    def _log(self, lines, name="run.log"):
        path = os.path.join(self.root, name)
        with open(path, "w", encoding="utf-8") as fh:
            fh.write("".join(ln + "\n" for ln in lines))
        return path

    def _inspect(self, *argv):
        out = io.StringIO()
        with redirect_stdout(out):
            code = rq.main(["inspect", *argv])
        return code, out.getvalue()

    def test_grep_prints_numbered_matches_only(self):
        path = self._log(["alpha", "boom here", "gamma", "delta"])
        code, out = self._inspect(path, "--grep", "boom")
        self.assertEqual(code, 0)
        self.assertIn("2:boom here", out)
        self.assertNotIn("alpha", out)
        self.assertIn("1 match(es) of 4 line(s)", out)

    def test_grep_context_includes_both_sides(self):
        path = self._log(["a", "b", "boom", "d", "e"])
        _, out = self._inspect(path, "--grep", "boom", "--context", "1")
        self.assertIn("2:b", out)
        self.assertIn("3:boom", out)
        self.assertIn("4:d", out)
        self.assertNotIn("1:a", out)
        self.assertNotIn("5:e", out)

    def test_disjoint_regions_are_separated(self):
        path = self._log(["boom"] + ["pad %d" % k for k in range(10)] + ["boom"])
        _, out = self._inspect(path, "--grep", "boom", "--context", "1")
        self.assertIn("--", out)

    def test_no_match_exits_non_zero(self):
        # So a caller checking only the status can tell absent from present.
        path = self._log(["alpha", "beta"])
        code, out = self._inspect(path, "--grep", "boom")
        self.assertEqual(code, 1)
        self.assertIn("0 match(es)", out)

    def test_an_over_broad_pattern_is_capped_rather_than_dumped(self):
        path = self._log(["line %d" % k for k in range(rq.MAX_GREP_MATCHES + 20)])
        _, out = self._inspect(path, "--grep", "line")
        self.assertIn("narrow the pattern", out)
        # Count the numbered match lines exactly. An earlier version counted
        # lines merely starting with "1", which is 11 whether the cap fires or
        # not — an assertion that passed identically with the cap removed.
        emitted = [ln for ln in out.splitlines() if re.match(r"^\d+:", ln)]
        self.assertEqual(len(emitted), rq.MAX_GREP_MATCHES)

    def test_the_capped_summary_reports_what_it_printed(self):
        # Not one more. Counting before the cap test reported 41 for a run that
        # emitted 40 regions — neither what the caller saw nor what the log has.
        path = self._log(["line %d" % k for k in range(rq.MAX_GREP_MATCHES + 20)])
        _, out = self._inspect(path, "--grep", "line")
        self.assertIn("%d match(es)+" % rq.MAX_GREP_MATCHES, out)

    def test_grep_takes_precedence_over_failing(self):
        # Documented precedence rather than silent behavior: given both, the
        # narrower view wins.
        path = self._log(["cspell" + "." * 30 + "Failed", "boom here"])
        _, out = self._inspect(path, "--grep", "boom", "--failing")
        self.assertIn("2:boom here", out)
        self.assertNotIn("failed hooks", out)

    def test_a_bad_pattern_is_a_usage_error_not_a_traceback(self):
        path = self._log(["alpha"])
        err = io.StringIO()
        with redirect_stderr(err):
            code = rq.main(["inspect", path, "--grep", "([unclosed"])
        self.assertEqual(code, 2)
        self.assertIn("bad --grep pattern", err.getvalue())

    def test_no_flags_reprints_the_failure_summary(self):
        path = self._log(
            [
                "cspell" + "." * 30 + "Failed",
                "docs/x.md:1:1 - Unknown word (zzz)",
                "trailing detail",
            ]
        )
        code, out = self._inspect(path)
        self.assertEqual(code, 0)
        self.assertIn("failed hooks (1)", out)
        self.assertIn("zzz", out)
        self.assertIn("trailing detail", out)

    def test_failing_suppresses_the_tail_but_keeps_the_indexes(self):
        path = self._log(
            ["cspell" + "." * 30 + "Failed"] + ["noise %d" % k for k in range(5)]
        )
        _, out = self._inspect(path, "--failing")
        self.assertIn("failed hooks (1)", out)
        self.assertNotIn("noise 4", out)

    def test_a_missing_log_is_a_clean_error(self):
        err = io.StringIO()
        with redirect_stderr(err):
            code = rq.main(["inspect", os.path.join(self.root, "gone.log")])
        self.assertEqual(code, 2)
        self.assertIn("cannot read", err.getvalue())

    def test_the_inspect_verb_does_not_shadow_a_command_named_inspect(self):
        # `run_quiet.py -- inspect ...` still RUNS a command called inspect:
        # the verb is only recognized as the very first argument.
        out = io.StringIO()
        with redirect_stdout(out):
            code = rq.main(["--", PY, "-c", "print('ran')"])
        self.assertEqual(code, 0)
        self.assertIn("exit 0", out.getvalue())


if __name__ == "__main__":
    unittest.main()
