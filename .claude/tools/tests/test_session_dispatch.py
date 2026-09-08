#!/usr/bin/env python3
# cspell:word metacharacter
"""Unit tests for `session_dispatch.py` (stdlib unittest).

Everything here drives the pure half — grammar validation, command composition,
and the fallback text on a blocked dispatch. The iTerm driving lives in
`iterm_api` and is covered by its own suite; the window-creating half of THAT
needs a live iTerm2 and is deliberately not faked, since a mock of the API would
only assert that the module's idea of the API is self-consistent, which is the
one thing that cannot fail in a useful way.
"""

from __future__ import annotations

import io
import unittest
from contextlib import redirect_stderr, redirect_stdout

import iterm_api
import session_dispatch


class Grammar(unittest.TestCase):
    def test_task_by_number(self):
        self.assertEqual(session_dispatch.validate(["task", "1234"]), ["task", "1234"])

    def test_task_by_tag(self):
        self.assertEqual(
            session_dispatch.validate(["task", "eng-1234"]), ["task", "eng-1234"]
        )

    def test_task_local_passes_the_word_through(self):
        # The substrate choice stays at the call site; the dispatcher adds no
        # policy of its own, which is exactly what this asserts.
        self.assertEqual(
            session_dispatch.validate(["task", "local", "1234"]),
            ["task", "local", "1234"],
        )

    def test_task_resume_with_a_number(self):
        self.assertEqual(
            session_dispatch.validate(["task", "resume", "88"]),
            ["task", "resume", "88"],
        )

    def test_task_resume_alone_is_the_picker(self):
        self.assertEqual(
            session_dispatch.validate(["task", "resume"]), ["task", "resume"]
        )

    def test_task_local_alone_is_refused(self):
        # No worktree to open, and the shell says the same.
        with self.assertRaises(ValueError):
            session_dispatch.validate(["task", "local"])

    def test_task_needs_an_argument(self):
        with self.assertRaises(ValueError):
            session_dispatch.validate(["task"])

    def test_task_refuses_a_non_numeric_tag(self):
        with self.assertRaises(ValueError):
            session_dispatch.validate(["task", "main"])

    def test_explore_bare_and_named(self):
        self.assertEqual(session_dispatch.validate(["explore"]), ["explore"])
        self.assertEqual(
            session_dispatch.validate(["explore", "feeds"]), ["explore", "feeds"]
        )

    def test_explore_resume(self):
        self.assertEqual(
            session_dispatch.validate(["explore", "resume", "feeds"]),
            ["explore", "resume", "feeds"],
        )

    def test_architect_needs_a_topic(self):
        self.assertEqual(
            session_dispatch.validate(["architect", "volatility-telemetry"]),
            ["architect", "volatility-telemetry"],
        )
        with self.assertRaises(ValueError):
            session_dispatch.validate(["architect"])

    def test_verbs_that_take_no_arguments(self):
        for verb in ("plan", "housekeeping"):
            with self.subTest(verb=verb):
                self.assertEqual(session_dispatch.validate([verb]), [verb])
                with self.assertRaises(ValueError):
                    session_dispatch.validate([verb, "extra"])

    def test_fleet_and_fleet_go(self):
        self.assertEqual(session_dispatch.validate(["fleet"]), ["fleet"])
        self.assertEqual(session_dispatch.validate(["fleet", "go"]), ["fleet", "go"])
        with self.assertRaises(ValueError):
            session_dispatch.validate(["fleet", "stop"])

    def test_unknown_verb(self):
        with self.assertRaises(ValueError):
            session_dispatch.validate(["deploy", "prod"])

    def test_nothing_at_all(self):
        with self.assertRaises(ValueError):
            session_dispatch.validate([])

    def test_shell_metacharacters_are_refused_not_quoted(self):
        # The real assertion of this suite. The tool types its argument into an
        # interactive shell, so a smuggled command must never reach
        # `command_line` in the first place — quoting it would be a weaker
        # answer than refusing it, because the operator would then watch a
        # nonsense verb get typed into a brand-new window.
        # The canaries are deliberately INERT strings — an echo, a `whoami`, a
        # path traversal. The property under test is that the grammar refuses a
        # shell metacharacter at all, which any separator demonstrates equally
        # well; picking a destructive one would only put a scary literal in the
        # tree for a future reader or a grep to trip over.
        for hostile in (
            ["task", "1234; echo pwned"],
            ["task", "$(whoami)"],
            ["explore", "a`id`"],
            ["architect", "x && echo pwned"],
            ["task", "local", "1|sh"],
            ["explore", "resume", "../../etc/passwd"],
        ):
            with self.subTest(argv=hostile):
                with self.assertRaises(ValueError):
                    session_dispatch.validate(hostile)


class CommandLine(unittest.TestCase):
    def test_plain_words_are_not_mangled(self):
        self.assertEqual(
            session_dispatch.command_line(["task", "local", "1234"]),
            "task local 1234",
        )

    def test_quoting_is_a_property_of_the_function(self):
        # `validate` already rules these inputs out; the quoting is asserted
        # anyway because a future caller composing words itself must not be able
        # to emit an unquoted metacharacter.
        self.assertEqual(session_dispatch.command_line(["task", "a b"]), "task 'a b'")
        self.assertEqual(
            session_dispatch.command_line(["task", "a; echo pwned"]),
            "task 'a; echo pwned'",
        )


class DryRun(unittest.TestCase):
    def test_dry_run_prints_the_line_and_touches_nothing(self):
        out = io.StringIO()
        with redirect_stdout(out):
            rc = session_dispatch.run(["--dry-run", "task", "local", "1234"])
        self.assertEqual(rc, 0)
        self.assertEqual(out.getvalue().strip(), "task local 1234")

    def test_a_bad_verb_exits_two_before_any_dispatch(self):
        err = io.StringIO()
        with redirect_stderr(err):
            rc = session_dispatch.run(["--dry-run", "task", "not-a-tag"])
        self.assertEqual(rc, 2)
        self.assertIn("eng-### tag", err.getvalue())


class BlockedDispatch(unittest.TestCase):
    def _fail_with(self, message):
        def boom(_command):
            raise iterm_api.ItermUnavailable(message)

        original = iterm_api.open_window
        iterm_api.open_window = boom
        self.addCleanup(setattr, iterm_api, "open_window", original)

    def test_a_blocked_dispatch_still_prints_the_verb(self):
        # Best-effort is the ratified bar, and this is what makes it acceptable:
        # the operator loses the convenience, never the launch.
        self._fail_with("iTerm2 is not running")
        err = io.StringIO()
        with redirect_stderr(err):
            rc = session_dispatch.run(["task", "1234"])
        text = err.getvalue()
        self.assertEqual(rc, 1)
        self.assertIn("iTerm2 is not running", text)
        self.assertIn("run this by hand", text)
        self.assertIn("task 1234", text)

    def test_the_printed_verb_is_the_full_line_including_local(self):
        # A `task local` dispatch that falls back must hand over the SUBSTRATE
        # too — printing a bare `task 1234` would send the operator to Bedrock
        # for work that was deliberately routed to the seat.
        self._fail_with("nope")
        err = io.StringIO()
        with redirect_stderr(err):
            session_dispatch.run(["task", "local", "1234"])
        self.assertIn("task local 1234", err.getvalue())

    def test_a_successful_dispatch_is_quiet_and_zero(self):
        seen = []
        original = iterm_api.open_window
        iterm_api.open_window = lambda command: seen.append(command) or "/dev/ttys1"
        self.addCleanup(setattr, iterm_api, "open_window", original)

        err = io.StringIO()
        with redirect_stderr(err):
            rc = session_dispatch.run(["architect", "volatility-telemetry"])
        self.assertEqual(rc, 0)
        self.assertEqual(err.getvalue(), "")
        self.assertEqual(seen, ["architect volatility-telemetry"])


if __name__ == "__main__":
    unittest.main()
