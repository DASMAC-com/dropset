#!/usr/bin/env python3
# cspell:word metacharacter
"""Unit tests for `session_dispatch.py` (stdlib unittest).

Everything here drives the pure half — grammar validation, verb splitting,
command composition, the reported ttys and roll-call, and the fallback text on a
blocked dispatch. The iTerm driving lives in `iterm_api` and is covered by its
own suite; the tab-creating half of THAT needs a live iTerm2 and is deliberately
not faked, since a mock of the API would only assert that the module's idea of
the API is self-consistent, which is the one thing that cannot fail in a useful
way.

What IS faked here is `iterm_api.open_tabs` itself — the seam between this tool
and the automation, not the automation. That is the boundary worth asserting:
that one call carries every verb in order, and that nothing is typed when any
verb fails to validate.
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

    def test_dry_run_prints_one_line_per_verb(self):
        out = io.StringIO()
        with redirect_stdout(out):
            rc = session_dispatch.run(
                ["--dry-run", "task", "1234", "+", "plan", "+", "fleet", "go"]
            )
        self.assertEqual(rc, 0)
        self.assertEqual(out.getvalue().splitlines(), ["task 1234", "plan", "fleet go"])

    def test_a_bad_verb_exits_two_before_any_dispatch(self):
        err = io.StringIO()
        with redirect_stderr(err):
            rc = session_dispatch.run(["--dry-run", "task", "not-a-tag"])
        self.assertEqual(rc, 2)
        self.assertIn("eng-### tag", err.getvalue())


class SplitVerbs(unittest.TestCase):
    def test_one_verb_needs_no_separator(self):
        self.assertEqual(
            session_dispatch.split_verbs(["task", "1234"]), [["task", "1234"]]
        )

    def test_several_verbs_split_on_the_separator(self):
        self.assertEqual(
            session_dispatch.split_verbs(
                ["task", "local", "1234", "+", "plan", "+", "housekeeping"]
            ),
            [["task", "local", "1234"], ["plan"], ["housekeeping"]],
        )

    def test_nothing_at_all_is_one_empty_group(self):
        # Not an error here — `validate` is what rejects an empty verb, and it
        # already has the message for it. Splitting has no opinion on emptiness
        # until a separator implies a verb that is not there.
        self.assertEqual(session_dispatch.split_verbs([]), [[]])

    def test_a_dangling_separator_is_refused(self):
        # A typo, not a request. Dropping the empty group instead would dispatch
        # a batch of a different size than the caller wrote.
        for argv in (
            ["plan", "+"],
            ["+", "plan"],
            ["plan", "+", "+", "housekeeping"],
        ):
            with self.subTest(argv=argv):
                with self.assertRaises(ValueError):
                    session_dispatch.split_verbs(argv)

    def test_the_separator_is_not_a_legal_verb_or_argument(self):
        # What makes `+` safe as a separator: the grammar cannot produce it, so
        # splitting can never eat a real word.
        with self.assertRaises(ValueError):
            session_dispatch.validate([session_dispatch.VERB_SEPARATOR])
        with self.assertRaises(ValueError):
            session_dispatch.validate(["architect", session_dispatch.VERB_SEPARATOR])


class Dispatch(unittest.TestCase):
    def _stub_tabs(self, result):
        """Replace `open_tabs`, recording its argument. `result` is a list or an
        exception to raise."""
        seen = []

        def fake(commands):
            seen.append(list(commands))
            if isinstance(result, Exception):
                raise result
            return result

        original = iterm_api.open_tabs
        iterm_api.open_tabs = fake
        self.addCleanup(setattr, iterm_api, "open_tabs", original)
        return seen

    def _stub_names(self, names):
        original = iterm_api.session_names
        iterm_api.session_names = lambda: names
        self.addCleanup(setattr, iterm_api, "session_names", original)

    def test_a_dispatch_opens_a_tab_and_reports_its_tty(self):
        seen = self._stub_tabs(["/dev/ttys005"])
        self._stub_names(["plan-10", "eng-1194"])

        out = io.StringIO()
        with redirect_stdout(out):
            rc = session_dispatch.run(["task", "1194"])

        self.assertEqual(rc, 0)
        self.assertEqual(seen, [["task 1194"]])
        text = out.getvalue()
        self.assertIn("task 1194 -> /dev/ttys005", text)
        # The roll-call is the independent confirmation: the tty above comes
        # from the call that created the tab, so it cannot disprove its own
        # success, whereas a fresh listing can.
        self.assertIn("eng-1194", text)

    def test_a_batch_is_one_call_with_every_verb_in_order(self):
        # The whole point of taking several verbs: one driver round trip.
        seen = self._stub_tabs(["/dev/a", "/dev/b", "/dev/c"])
        self._stub_names([])

        out = io.StringIO()
        with redirect_stdout(out):
            rc = session_dispatch.run(
                ["task", "local", "1234", "+", "plan", "+", "architect", "fx"]
            )

        self.assertEqual(rc, 0)
        self.assertEqual(seen, [["task local 1234", "plan", "architect fx"]])
        text = out.getvalue()
        self.assertIn("task local 1234 -> /dev/a", text)
        self.assertIn("plan -> /dev/b", text)
        self.assertIn("architect fx -> /dev/c", text)

    def test_one_bad_verb_in_a_batch_dispatches_nothing(self):
        # A partial batch is the bad outcome: the caller cannot tell which tabs
        # it got, and re-running to fix the typo would double the ones that
        # worked.
        seen = self._stub_tabs(["/dev/a"])
        err = io.StringIO()
        with redirect_stderr(err):
            rc = session_dispatch.run(["plan", "+", "task", "not-a-tag"])
        self.assertEqual(rc, 2)
        self.assertEqual(seen, [])
        self.assertIn("eng-### tag", err.getvalue())

    def test_an_unreadable_tty_is_named_not_dropped(self):
        self._stub_tabs([None])
        self._stub_names([])
        out = io.StringIO()
        with redirect_stdout(out):
            rc = session_dispatch.run(["plan"])
        self.assertEqual(rc, 0)
        self.assertIn("tty unknown", out.getvalue())

    def test_a_lost_roll_call_does_not_fail_a_dispatch_that_worked(self):
        # The tabs are already open and typed into by then, so a non-zero exit
        # would misreport a launch that happened.
        self._stub_tabs(["/dev/a"])
        original = iterm_api.session_names

        def boom():
            raise iterm_api.ItermUnavailable("API went away")

        iterm_api.session_names = boom
        self.addCleanup(setattr, iterm_api, "session_names", original)

        out = io.StringIO()
        with redirect_stdout(out):
            rc = session_dispatch.run(["plan"])
        self.assertEqual(rc, 0)
        self.assertIn("roll-call unavailable", out.getvalue())


class BlockedDispatch(unittest.TestCase):
    def _fail_with(self, message):
        def boom(_commands):
            raise iterm_api.ItermUnavailable(message)

        original = iterm_api.open_tabs
        iterm_api.open_tabs = boom
        self.addCleanup(setattr, iterm_api, "open_tabs", original)

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

    def test_a_blocked_batch_names_every_verb_not_just_the_first(self):
        # The operator has to hand-run the whole batch, so naming only the head
        # of it would silently drop the rest.
        self._fail_with("nope")
        err = io.StringIO()
        with redirect_stderr(err):
            rc = session_dispatch.run(["plan", "+", "task", "1234", "+", "fleet", "go"])
        text = err.getvalue()
        self.assertEqual(rc, 1)
        self.assertIn("run these by hand", text)
        for command in ("plan", "task 1234", "fleet go"):
            self.assertIn(command, text)


if __name__ == "__main__":
    unittest.main()
