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
import os
import tempfile
import unittest
import unittest.mock
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

import iterm_api
import machine_load
import session_dispatch

GIB = machine_load.GIB


def reading(
    slots=20,
    reasons=(),
    free_slots=None,
    free_reasons=(),
    building=0,
    longest=0,
    waiting=0,
    total=20,
    free=0,
):
    """A probe's JSON in the shape `machine_load.as_json` prints. The free
    verdict defaults to the total's room, as the probe's does."""
    live = building + free
    if free_slots is None:
        free_slots = 0 if free_reasons else max(0, total - live)
    return {
        "status": "green",
        "exit": 0,
        "verdict": {
            "building": {"slots": slots, "reasons": list(reasons)},
            "free": {"slots": free_slots, "reasons": list(free_reasons)},
        },
        "builds": {
            "building": [],
            "waiting": [{"pid": 9, "step": "clippy", "wait_s": longest}] * waiting,
            "longest_wait_s": longest,
        },
        "sessions": {"live": live, "building": building, "free": free, "records": 0},
        "memory": {"headroom": 3 * GIB, "reserve": 8 * GIB},
        "load": {"load1": 30.0, "cores": 12, "max": 16.0},
        "swap_used": 2 * GIB,
        "pressure": "warn",
        "knobs": {
            "build_wait_s": 120,
            "total": total,
            "reserve": 8 * GIB,
            "cpu_load_max": 16.0,
            "notes": [],
        },
    }


#: The required knobs, as the operator's runtime config sets them.
FLEET = {"DS_FLEET_TOTAL": "16", "DS_FLEET_CPU_LOAD_MAX": "16"}


NO_READING = {"status": "no-reading", "exit": 3, "error": "not Darwin (Linux)"}


class Readings:
    """A stub probe handing out readings in order, the last one repeating."""

    def __init__(self, *readings):
        self.readings = list(readings)
        self.calls = 0

    def __call__(self, _slots, _directory):
        self.calls += 1
        return self.readings[min(self.calls, len(self.readings)) - 1]


def stub_gate(case, *readings, bodies=None):
    """Patch the probe, the state directory and the body fetch for `run`."""
    probe = Readings(*(readings or (reading(),)))
    scratch = tempfile.TemporaryDirectory()
    case.addCleanup(scratch.cleanup)
    directory = Path(scratch.name)
    bodies = bodies or {}
    for name, value in (
        ("probe", probe),
        ("state_dir", lambda: directory),
        ("fetch_body", lambda number: bodies.get(number, "")),
    ):
        original = getattr(session_dispatch, name)
        setattr(session_dispatch, name, value)
        case.addCleanup(setattr, session_dispatch, name, original)
    return probe, directory


class Grammar(unittest.TestCase):
    def test_task_by_number(self):
        self.assertEqual(session_dispatch.validate(["task", "1234"]), ["task", "1234"])

    def test_task_by_tag(self):
        self.assertEqual(
            session_dispatch.validate(["task", "eng-1234"]), ["task", "eng-1234"]
        )

    def test_a_task_substrate_word_passes_through(self):
        # The substrate choice stays at the call site; the dispatcher adds no
        # policy of its own, which is exactly what this asserts.
        for word in ("anthropic", "bedrock"):
            with self.subTest(word=word):
                self.assertEqual(
                    session_dispatch.validate(["task", word, "1234"]),
                    ["task", word, "1234"],
                )

    def test_the_retired_task_local_is_refused_with_its_replacement(self):
        with self.assertRaisesRegex(ValueError, "task anthropic"):
            session_dispatch.validate(["task", "local", "1234"])

    def test_plan_and_architect_take_the_pinch_override(self):
        self.assertEqual(
            session_dispatch.validate(["plan", "bedrock"]), ["plan", "bedrock"]
        )
        for word in ("anthropic", "bedrock"):
            with self.subTest(word=word):
                self.assertEqual(
                    session_dispatch.validate(["architect", word, "1572"]),
                    ["architect", word, "1572"],
                )
        for argv in (
            ["plan", "local"],
            ["architect", "local", "1572"],
            ["architect", "bedrock"],
            ["architect", "bedrock", "0"],
            ["architect", "bedrock", "1572", "anthropic"],
        ):
            with self.subTest(argv=argv), self.assertRaises(ValueError):
                session_dispatch.validate(argv)

    def test_architect_s_retired_order_names_the_new_one(self):
        """The word moved before the number, as on every issue verb; the old
        order is refused with the line to type instead."""
        with self.assertRaisesRegex(ValueError, "architect bedrock 1572"):
            session_dispatch.validate(["architect", "1572", "bedrock"])

    def test_task_resume_with_a_number(self):
        self.assertEqual(
            session_dispatch.validate(["task", "resume", "88"]),
            ["task", "resume", "88"],
        )

    def test_task_resume_alone_is_the_picker(self):
        self.assertEqual(
            session_dispatch.validate(["task", "resume"]), ["task", "resume"]
        )

    def test_task_anthropic_alone_is_refused(self):
        # No worktree to open, and the shell says the same.
        with self.assertRaises(ValueError):
            session_dispatch.validate(["task", "anthropic"])

    def test_task_needs_an_argument(self):
        with self.assertRaises(ValueError):
            session_dispatch.validate(["task"])

    def test_task_refuses_a_non_numeric_tag(self):
        with self.assertRaises(ValueError):
            session_dispatch.validate(["task", "main"])

    def test_explore_named(self):
        self.assertEqual(
            session_dispatch.validate(["explore", "feeds"]), ["explore", "feeds"]
        )

    def test_explore_issue_keyed(self):
        """A bare number is the audit-dispatch form: it keys the worktree to the
        Linear issue, so `explore 1196` must validate exactly as a name does."""
        self.assertEqual(
            session_dispatch.validate(["explore", "1196"]), ["explore", "1196"]
        )

    def test_explore_rejects_zero_like_the_shell_does(self):
        """`_NAME` admits `0`, but the shell refuses it — so dispatching would
        open a tab, type the verb and leave an idle session that looks started.
        Every spelling the shell normalizes to zero must be rejected here."""
        for zero in ("0", "00", "eng-0", "eng-000"):
            with self.subTest(arg=zero):
                with self.assertRaises(ValueError):
                    session_dispatch.validate(["explore", zero])

    def test_explore_accepts_the_eng_prefixed_form(self):
        """The shell normalizes `eng-1196` and `1196` to one worktree, so the
        dispatcher must accept both spellings."""
        self.assertEqual(
            session_dispatch.validate(["explore", "eng-1196"]),
            ["explore", "eng-1196"],
        )

    def test_explore_requires_a_name(self):
        """Bare `explore` used to be legal and now is not — a name is required
        because it names a worktree, and the shell rejects the bare form."""
        with self.assertRaises(ValueError):
            session_dispatch.validate(["explore"])

    def test_explore_takes_either_substrate_word(self):
        """Either word before the argument, as `task` takes it, so a
        `[Explore/Bedrock]` title types as `explore bedrock <n>`; a word is
        never a name, in either grammar."""
        for word in ("anthropic", "bedrock"):
            for target in ("1196", "feeds", "eng-1196"):
                with self.subTest(word=word, target=target):
                    self.assertEqual(
                        session_dispatch.validate(["explore", word, target]),
                        ["explore", word, target],
                    )
        for argv in (
            ["explore", "1196", "bedrock"],
            ["explore", "anthropic"],
            ["explore", "anthropic", "0"],
            # The shell strips the word, then refuses the retired twin.
            ["explore", "anthropic", "resume"],
            # A substrate word is never a name, in either grammar.
            ["explore", "bedrock"],
            ["explore", "anthropic", "anthropic"],
        ):
            with self.subTest(argv=argv):
                with self.assertRaises(ValueError):
                    session_dispatch.validate(argv)

    def test_explore_resume_is_retired(self):
        """`explore <n|name>` resumes an existing session itself, so the twin
        would type a command the shell now refuses."""
        with self.assertRaises(ValueError) as caught:
            session_dispatch.validate(["explore", "resume", "feeds"])
        self.assertIn("retired", str(caught.exception))

    def test_architect_takes_the_governing_issue_number(self):
        self.assertEqual(
            session_dispatch.validate(["architect", "1572"]), ["architect", "1572"]
        )
        # There is no topic form any more, and 0 is not an issue.
        for argv in (["architect"], ["architect", "volatility"], ["architect", "0"]):
            with self.subTest(argv=argv), self.assertRaises(ValueError):
                session_dispatch.validate(argv)

    def test_chore_takes_task_s_shapes_without_resume(self):
        for argv in (
            ["chore", "1843"],
            ["chore", "eng-1843"],
            ["chore", "anthropic", "1843"],
            ["chore", "bedrock", "1843"],
        ):
            with self.subTest(argv=argv):
                self.assertEqual(session_dispatch.validate(argv), argv)
        for argv in (
            ["chore"],
            ["chore", "anthropic"],
            ["chore", "0"],
            ["chore", "keygen"],
            ["chore", "resume", "1843"],
            ["chore", "local", "1843"],
            ["chore", "1843", "bedrock"],
        ):
            with self.subTest(argv=argv), self.assertRaises(ValueError):
                session_dispatch.validate(argv)

    def test_plan_takes_no_other_argument(self):
        self.assertEqual(session_dispatch.validate(["plan"]), ["plan"])
        with self.assertRaises(ValueError):
            session_dispatch.validate(["plan", "extra"])

    def test_curate_validates_beside_plan(self):
        self.assertEqual(session_dispatch.validate(["curate"]), ["curate"])
        self.assertEqual(
            session_dispatch.validate(["curate", "anthropic"]), ["curate", "anthropic"]
        )
        for argv in (["curate", "1234"], ["curate", "bedrock", "1234"]):
            with self.assertRaises(ValueError, msg=argv):
                session_dispatch.validate(argv)

    def test_curate_is_a_free_seat(self):
        self.assertEqual(
            session_dispatch.classify(["curate"]).cls, session_dispatch.FREE
        )

    def test_launch_validates_as_a_free_seat(self):
        self.assertEqual(session_dispatch.validate(["launch"]), ["launch"])
        self.assertEqual(
            session_dispatch.validate(["launch", "anthropic"]), ["launch", "anthropic"]
        )
        with self.assertRaises(ValueError):
            session_dispatch.validate(["launch", "1234"])
        self.assertEqual(
            session_dispatch.classify(["launch"]).cls, session_dispatch.FREE
        )

    def test_the_retired_housekeeping_verb_is_unknown(self):
        with self.assertRaises(ValueError):
            session_dispatch.validate(["housekeeping"])

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
            ["task", "anthropic", "1|sh"],
            ["explore", "../../etc/passwd"],
        ):
            with self.subTest(argv=hostile):
                with self.assertRaises(ValueError):
                    session_dispatch.validate(hostile)


class CommandLine(unittest.TestCase):
    def test_plain_words_are_not_mangled(self):
        self.assertEqual(
            session_dispatch.command_line(["task", "anthropic", "1234"]),
            "task anthropic 1234",
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
            rc = session_dispatch.run(["--dry-run", "task", "anthropic", "1234"])
        self.assertEqual(rc, 0)
        self.assertEqual(out.getvalue().strip(), "task anthropic 1234")

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
                ["task", "anthropic", "1234", "+", "plan", "+", "fleet"]
            ),
            [["task", "anthropic", "1234"], ["plan"], ["fleet"]],
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
            ["plan", "+", "+", "fleet"],
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
    def setUp(self):
        # A green machine, so these cases test the tab driving alone.
        stub_gate(self)
        # Always stubbed: the real mark writes a state file and paints a live
        # tty, and a fake tty like /dev/ttys005 may well be a real one here.
        self.marked = []
        self.mark_ok = True

        def mark(tty):
            self.marked.append(tty)
            return self.mark_ok

        original = iterm_api.mark_attention
        iterm_api.mark_attention = mark
        self.addCleanup(setattr, iterm_api, "mark_attention", original)

    def _stub_tabs(self, result):
        """Replace `open_tabs`, recording its argument. `result` is a list or an
        exception to raise."""
        seen = []
        self.roles = []

        def fake(commands, roles=None):
            seen.append(list(commands))
            self.roles.append(roles)
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
                ["task", "anthropic", "1234", "+", "plan", "+", "architect", "56"]
            )

        self.assertEqual(rc, 0)
        self.assertEqual(seen, [["task anthropic 1234", "plan", "architect 56"]])
        text = out.getvalue()
        self.assertIn("task anthropic 1234 -> /dev/a", text)
        self.assertIn("plan (free) -> /dev/b", text)
        self.assertIn("architect 56 (free) -> /dev/c", text)
        # Only the seat verb's tab carries a role; it alone is placed as a seat.
        self.assertEqual(self.roles, [[None, "plan", None]])

    def test_a_curate_tab_is_placed_as_a_seat_beside_the_scheduler(self):
        self._stub_tabs(["/dev/a", "/dev/b"])
        self._stub_names([])
        with redirect_stdout(io.StringIO()):
            rc = session_dispatch.run(["curate", "+", "launch"])
        self.assertEqual(rc, 0)
        self.assertEqual(self.roles, [["curate", "launch"]])

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

    def test_every_dispatched_tab_gets_the_attend_mark(self):
        self._stub_tabs(["/dev/a", "/dev/b"])
        self._stub_names([])
        with redirect_stdout(io.StringIO()):
            rc = session_dispatch.run(["task", "resume", "12", "+", "chore", "34"])
        self.assertEqual(rc, 0)
        self.assertEqual(self.marked, ["/dev/a", "/dev/b"])

    def test_a_lost_mark_is_named_and_does_not_fail_the_dispatch(self):
        self._stub_tabs(["/dev/a", None])
        self._stub_names([])
        self.mark_ok = False
        err = io.StringIO()
        with redirect_stdout(io.StringIO()), redirect_stderr(err):
            rc = session_dispatch.run(["task", "12", "+", "plan"])
        self.assertEqual(rc, 0)
        # A tab with no tty cannot be marked, and is never handed to the script.
        self.assertEqual(self.marked, ["/dev/a"])
        self.assertIn("could not mark green: task 12, plan", err.getvalue())


class BlockedDispatch(unittest.TestCase):
    def setUp(self):
        stub_gate(self)

    def _fail_with(self, message, ttys=None, untyped=None):
        def boom(_commands, roles=None):
            raise iterm_api.ItermUnavailable(message, ttys=ttys, untyped=untyped)

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

    def test_the_printed_verb_is_the_full_line_including_the_substrate(self):
        # A `task anthropic` dispatch that falls back must hand over the SUBSTRATE
        # too — printing a bare `task 1234` would send the operator to Bedrock
        # for work that was deliberately routed to the seat.
        self._fail_with("nope")
        err = io.StringIO()
        with redirect_stderr(err):
            session_dispatch.run(["task", "anthropic", "1234"])
        self.assertIn("task anthropic 1234", err.getvalue())

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

    def test_a_failure_part_way_through_does_not_offer_sent_verbs_for_rerun(self):
        """The double-type this exists to prevent.

        A driver failure after k tabs were opened and typed left those k running,
        and naming the whole batch told the operator to type a second copy of
        each into a new tab. Only the tail that was never sent may appear under
        "run these by hand".
        """
        self._fail_with("driver broke", ttys=["/dev/a"])
        err = io.StringIO()
        with redirect_stderr(err):
            rc = session_dispatch.run(["plan", "+", "task", "1234", "+", "fleet", "go"])
        text = err.getvalue()
        self.assertEqual(rc, 1)

        # The already-typed verb is reported as such, and NOT as a hand-run.
        self.assertIn("already opened", text)
        self.assertIn("/dev/a", text)
        hand_run = text.split("run these by hand")[1]
        # Anchored on the whole listing line, not the bare word: a substring
        # test for "plan" silently stops testing anything the moment this block
        # gains a word that contains it ("planning").
        self.assertNotIn("\n  plan\n", hand_run)
        for command in ("task 1234", "fleet go"):
            self.assertIn(command, hand_run)

    def test_a_tab_that_could_not_be_typed_into_is_offered_for_rerun(self):
        """The silent DROP, which is the opposite failure to the double-type.

        The driver records a position for a tab it created but could not reach a
        session in, so that command never ran. Reading the partial's length as
        the dispatch count reported it as already done and it was never offered
        for a hand-run at all — for `fleet_resume`, an issue that silently never
        got an agent.

        The mutation this pins: deriving the retry set from `len(exc.ttys)`, or
        filtering the falsy entries out of the partial, leaves every other test
        in this file green and re-breaks exactly this case.
        """
        self._fail_with("driver broke", ttys=["/dev/a", None], untyped=[1])
        err = io.StringIO()
        with redirect_stderr(err):
            rc = session_dispatch.run(["plan", "+", "task", "1234", "+", "fleet", "go"])
        text = err.getvalue()
        self.assertEqual(rc, 1)

        # Only `plan` really ran, so only it may be in the do-not-re-run block.
        self.assertIn("1 tab(s) were already opened", text)
        hand_run = text.split("run these by hand")[1]
        for command in ("task 1234", "fleet go"):
            self.assertIn(command, hand_run)
        self.assertNotIn("\n  plan\n", hand_run)

    def test_a_failure_that_opened_nothing_still_offers_the_whole_batch(self):
        # The exemption must not swallow the common case: an iTerm that was never
        # reachable dispatched nothing, so every verb is still a hand-run.
        self._fail_with("iTerm2 is not running")
        err = io.StringIO()
        with redirect_stderr(err):
            rc = session_dispatch.run(["plan", "+", "task", "1234"])
        text = err.getvalue()
        self.assertEqual(rc, 1)
        self.assertNotIn("already opened", text)
        for command in ("plan", "task 1234"):
            self.assertIn(command, text.split("run these by hand")[1])


def slot(words, **bodies):
    return session_dispatch.classify(words, fetch=lambda n: bodies.get(f"b{n}"))


class VerbClass(unittest.TestCase):
    def test_the_advisor_verbs_are_free_and_task_builds(self):
        for words in (
            ["architect", "1"],
            ["plan"],
            ["explore", "feeds"],
            ["fleet", "go"],
        ):
            with self.subTest(words=words):
                self.assertEqual(slot(words).cls, session_dispatch.FREE)
                self.assertTrue(slot(words).shown.endswith("(free)"))
        task = slot(["task", "anthropic", "12"])
        self.assertEqual((task.cls, task.shown), ("building", "task anthropic 12"))

    def test_a_verb_the_table_does_not_name_builds(self):
        # Errs toward refusing: a new verb is building until the table says so.
        unknown = session_dispatch.classify(["deploy"], fetch=lambda n: "")
        self.assertEqual(unknown.cls, session_dispatch.BUILDING)

    def test_a_chore_naming_a_build_command_in_code_builds(self):
        chore = slot(["chore", "2241"], b2241="Run `make demo` and check it.")
        self.assertEqual(chore.cls, "building")
        self.assertEqual(chore.shown, "chore 2241 (builds: make demo)")
        fenced = slot(["chore", "7"], b7="Then:\n\n```sh\ncargo nextest run\n```\n")
        self.assertEqual(fenced.label, "(builds: cargo nextest)")

    def test_a_chore_with_only_prose_and_tools_is_free(self):
        body = (
            "Make sure the doc says so; anchor on the heading. Run "
            "`python3 .claude/tools/make_x.py` and query `anchor-v2`."
        )
        chore = slot(["chore", "eng-2240"], b2240=body)
        self.assertEqual((chore.cls, chore.shown), ("free", "chore eng-2240 (free)"))
        self.assertEqual(chore.record, "2240")

    def test_a_localnet_anywhere_builds(self):
        self.assertEqual(
            slot(["chore", "3"], b3="Bring up the Localnet.").cls, "building"
        )

    def test_an_unreadable_body_builds(self):
        chore = slot(["chore", "bedrock", "9"])
        self.assertEqual(chore.shown, "chore bedrock 9 (builds: body unreadable)")

    def test_the_override_skips_the_fetch(self):
        def boom(_n):
            raise AssertionError("fetched despite --class")

        free = session_dispatch.classify(["chore", "4"], "free", fetch=boom)
        self.assertEqual(free.shown, "chore 4 (free, --class)")
        built = session_dispatch.classify(["chore", "4"], "building", fetch=boom)
        self.assertEqual(built.shown, "chore 4 (builds: --class)")

    def test_a_shell_without_the_key_reads_as_unreadable(self):
        with unittest.mock.patch.dict("os.environ", {"LINEAR_API_KEY": ""}):
            self.assertIsNone(session_dispatch.fetch_body("1"))

    def test_a_failed_fetch_reads_as_unreadable(self):
        with (
            unittest.mock.patch.dict("os.environ", {"LINEAR_API_KEY": "k"}),
            unittest.mock.patch.object(
                session_dispatch.linear_issue,
                "fetch_issue_body",
                side_effect=session_dispatch.linear_issue.LinearIssueError("no issue"),
            ),
        ):
            self.assertIsNone(session_dispatch.fetch_body("1"))

    def test_compose_and_tilde_fences_build(self):
        self.assertEqual(
            slot(["chore", "5"], b5="`docker-compose up`").label,
            "(builds: docker-compose up)",
        )
        tilde = slot(["chore", "6"], b6="~~~\nanchor build\n~~~\n")
        self.assertEqual(tilde.label, "(builds: anchor build)")


class Records(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.dir = Path(scratch.name)

    def test_a_chore_class_round_trips_and_others_write_nothing(self):
        slots = [slot(["chore", "5"], b5="prose"), slot(["task", "6"])]
        session_dispatch.write_records(slots, self.dir)
        self.assertEqual(sorted(p.name for p in self.dir.iterdir()), ["5.class"])
        self.assertEqual(session_dispatch.recorded_class(self.dir, "5"), "free")

    def test_a_live_stamped_record_of_the_same_class_keeps_its_pid(self):
        (self.dir / "5.class").write_text("free 4242\n")
        (self.dir / "6.class").write_text("free 4343\n")
        session_dispatch.write_records(
            [slot(["chore", "5"], b5="prose"), slot(["chore", "6"], b6="`make demo`")],
            self.dir,
        )
        self.assertEqual((self.dir / "5.class").read_text(), "free 4242\n")
        self.assertEqual((self.dir / "6.class").read_text(), "building\n")

    def test_the_launcher_s_stamped_form_and_the_old_key_read_back(self):
        (self.dir / "eng-8.class").write_text("free 12345\n")
        self.assertEqual(session_dispatch.recorded_class(self.dir, "8"), "free")

    def test_no_record_or_a_bad_one_reads_as_building(self):
        (self.dir / "2.class").write_text("\n")
        (self.dir / "3.class").write_text("sideways 1\n")
        for number in ("1", "2", "3"):
            self.assertEqual(
                session_dispatch.recorded_class(self.dir, number), "building"
            )


def gate(slots, *readings, over_budget=False, env=None):
    probe = Readings(*readings)
    result = session_dispatch.gate(
        slots,
        Path("/nonexistent"),
        over_budget=over_budget,
        env=FLEET if env is None else env,
        reader=probe,
    )
    return result, probe


def names(slots):
    return [s.command for s in slots]


BATCH = [
    ["task", "2163"],
    ["architect", "2230"],
    ["chore", "2240"],
    ["task", "2160"],
    ["chore", "2241"],
]


def batch():
    return [slot(words, b2240="prose", b2241="`make demo`") for words in BATCH]


class Admission(unittest.TestCase):
    def test_a_slot_admits_the_first_builder_and_free_verbs_regardless(self):
        result, _ = gate(batch(), reading(slots=1, waiting=1, longest=30))
        self.assertEqual(
            names(result.admitted), ["task 2163", "architect 2230", "chore 2240"]
        )
        self.assertEqual(
            session_dispatch.held_lines(result),
            [
                "held  task 2160: the reading admits 1 more building session(s) "
                "(builds are waiting on the pool lock)",
                "held  chore 2241 (builds: make demo): same",
                "would free a slot: any builder closing",
            ],
        )

    def test_a_wait_refusal_is_re_probed_once_and_stands(self):
        refused = reading(slots=0, reasons=["wait"], longest=465, waiting=2)
        result, probe = gate(batch(), refused)
        self.assertEqual(probe.calls, 2)
        self.assertEqual(names(result.admitted), ["architect 2230", "chore 2240"])
        lines = session_dispatch.held_lines(result)
        self.assertEqual(
            lines[0],
            "held  task 2163: longest build wait 7m45s on the pool lock (limit 2m00s)",
        )
        self.assertEqual(
            lines[1:3],
            ["held  task 2160: same", "held  chore 2241 (builds: make demo): same"],
        )
        self.assertEqual(
            lines[3],
            "would free a slot: the longest wait falling under 2m00s; any builder closing",
        )

    def test_a_re_probe_that_heals_admits_the_batch(self):
        refused = reading(slots=0, reasons=["wait"], longest=130, waiting=1)
        result, probe = gate(batch(), refused, reading())
        self.assertEqual(probe.calls, 2)
        self.assertEqual(len(result.admitted), 5)
        self.assertEqual(result.held, [])

    def test_a_re_probe_with_no_reading_keeps_the_first(self):
        # A missing probe never halts the fleet, the second call included:
        # the first reading's holds stand, rendered from that reading.
        refused = reading(slots=0, reasons=["wait"], longest=465, waiting=1)
        result, probe = gate(batch(), refused, NO_READING)
        self.assertEqual(probe.calls, 2)
        self.assertEqual(names(result.admitted), ["architect 2230", "chore 2240"])
        self.assertIn("longest build wait 7m45s", result.held[0][1])

    def test_memory_and_pressure_refusals_name_their_numbers(self):
        red = reading(
            slots=0,
            reasons=["memory", "pressure"],
            free_slots=0,
            free_reasons=["memory", "pressure"],
        )
        result, _ = gate([slot(["plan"])], red)
        self.assertEqual(
            session_dispatch.held_lines(result),
            [
                "held  plan (free): memory headroom 3.0 GiB (reserve 8.0); "
                "kernel pressure warn",
                "would free a slot: memory headroom over the reserve; "
                "kernel pressure back to normal",
            ],
        )

    def test_a_refusal_with_no_wait_is_not_re_probed(self):
        full = reading(slots=0, reasons=["total"], free_reasons=["total"], building=20)
        result, probe = gate(batch(), full)
        self.assertEqual(probe.calls, 1)
        self.assertIn(
            "20 clients of the total 20", session_dispatch.held_lines(result)[0]
        )

    def test_free_verbs_spend_the_same_total(self):
        result, _ = gate(batch(), reading(total=4, building=2))
        self.assertEqual(names(result.admitted), ["task 2163", "architect 2230"])
        self.assertEqual(
            session_dispatch.held_lines(result),
            [
                "held  chore 2240 (free): 2 clients of the total 4",
                "held  task 2160: same",
                "held  chore 2241 (builds: make demo): same",
                "would free a slot: any session closing",
            ],
        )
        self.assertEqual(result.clients, "clients 4 of 4 (builders 3, free 1)")

    def test_machine_checks_hold_free_verbs_too(self):
        red = reading(
            slots=0,
            reasons=["load"],
            free_slots=0,
            free_reasons=["load"],
        )
        result, _ = gate([slot(["plan"])], red)
        self.assertEqual(result.admitted, [])
        self.assertEqual(
            session_dispatch.held_lines(result),
            [
                "held  plan (free): load 30.0 (limit 16)",
                "would free a slot: load falling under 16",
            ],
        )

    def test_a_builder_the_total_holds_is_held_on_the_total(self):
        # No build waits, so the building slots equal the room: the third
        # builder is out of total, not out of building slots.
        builders = [slot(["task", n]) for n in ("1", "2", "3")]
        result, _ = gate(builders, reading(slots=2, total=4, building=2))
        self.assertEqual(names(result.admitted), ["task 1", "task 2"])
        self.assertEqual(
            session_dispatch.held_lines(result),
            [
                "held  task 3: 2 clients of the total 4",
                "would free a slot: any session closing",
            ],
        )

    def test_over_budget_admits_past_the_machine_and_says_what_it_overrode(self):
        refused = reading(slots=0, reasons=["wait"], longest=465, waiting=1)
        result, probe = gate(batch(), refused, over_budget=True)
        self.assertEqual(probe.calls, 1)
        self.assertEqual(len(result.admitted), 5)
        lines = session_dispatch.held_lines(result)
        self.assertIn(
            "over budget, admitted anyway: task 2163: longest build wait 7m45s",
            lines[0],
        )
        self.assertEqual(len(lines), 3)

    def test_over_budget_does_not_lift_the_total(self):
        full = reading(slots=0, reasons=["wait"], longest=465, building=19)
        result, _ = gate(batch(), full, over_budget=True)
        self.assertEqual(names(result.admitted), ["task 2163"])
        self.assertEqual(
            session_dispatch.held_lines(result)[0],
            "held  architect 2230 (free): 19 clients of the total 20",
        )

    def test_no_reading_admits_under_the_total_alone_and_says_why(self):
        env = {**FLEET, "DS_FLEET_TOTAL": "1"}
        result, _ = gate(batch(), NO_READING, env=env)
        self.assertEqual(names(result.admitted), ["task 2163"])
        self.assertEqual(
            result.notes,
            [
                "no machine reading (not Darwin (Linux)); admitting under the total 1 alone"
            ],
        )
        self.assertIn("the total 1 (no reading counted", result.held[0][1])
        self.assertEqual(result.clients, "")

    def test_no_reading_and_an_unset_total_holds_the_batch_naming_it(self):
        result, _ = gate(batch(), NO_READING, env={})
        self.assertEqual(result.admitted, [])
        self.assertEqual(len(result.held), 5)
        self.assertIn("DS_FLEET_TOTAL is unset", result.held[0][1])


class Probe(unittest.TestCase):
    def _probe(self, stdout):
        original = session_dispatch.subprocess.run
        session_dispatch.subprocess.run = lambda *a, **k: unittest.mock.Mock(
            stdout=stdout
        )
        self.addCleanup(setattr, session_dispatch.subprocess, "run", original)
        return session_dispatch.probe([slot(["task", "1"])], Path("/x"))

    def test_the_probe_s_own_no_reading_passes_through(self):
        got = self._probe('{"status": "no-reading", "exit": 3, "error": "boom"}')
        self.assertEqual(got["error"], "boom")

    def test_output_that_is_no_reading_shape_becomes_one(self):
        for stdout in ("", "not json", "[1]", '{"status": "green"}'):
            with self.subTest(stdout=stdout):
                got = self._probe(stdout)
                self.assertNotIn("verdict", got)
                self.assertEqual(got["exit"], 3)

    def test_a_probe_that_cannot_run_or_hangs_is_no_reading(self):
        for exc in (
            OSError("no python"),
            session_dispatch.subprocess.TimeoutExpired("probe", 60),
        ):
            with self.subTest(exc=type(exc).__name__):
                with unittest.mock.patch.object(
                    session_dispatch.subprocess, "run", side_effect=exc
                ):
                    got = session_dispatch.probe([slot(["task", "1"])], Path("/x"))
                self.assertEqual(got["exit"], 3)
                self.assertIn(type(exc).__name__, got["error"])

    def test_the_probe_is_asked_for_the_batch_s_builders_in_its_directory(self):
        with unittest.mock.patch.object(
            session_dispatch.subprocess,
            "run",
            return_value=unittest.mock.Mock(stdout=""),
        ) as run:
            session_dispatch.probe(
                [slot(["task", "1"]), slot(["plan"]), slot(["task", "2"])], Path("/s")
            )
        argv = run.call_args.args[0]
        self.assertTrue(argv[1].endswith("machine_load.py"))
        self.assertEqual(argv[2:], ["--json", "--sessions", "2", "--state-dir", "/s"])


class GatedRun(unittest.TestCase):
    def _run(self, *argv, readings=(), bodies=None, ttys=None):
        _, directory = stub_gate(self, *readings, bodies=bodies)
        seen = []

        def fake(commands, roles=None):
            seen.append(list(commands))
            return ttys or [f"/dev/t{n}" for n in range(len(commands))]

        for name, value in (
            ("open_tabs", fake),
            ("mark_attention", lambda tty: True),
            ("session_names", lambda: []),
        ):
            original = getattr(iterm_api, name)
            setattr(iterm_api, name, value)
            self.addCleanup(setattr, iterm_api, name, original)
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            rc = session_dispatch.run(list(argv))
        return rc, seen, out.getvalue(), err.getvalue(), directory

    def test_only_the_admitted_open_and_the_held_are_printed(self):
        refused = reading(slots=0, reasons=["wait"], longest=465, waiting=1)
        rc, seen, out, _, directory = self._run(
            "task",
            "2163",
            "+",
            "chore",
            "2240",
            readings=(refused,),
            bodies={"2240": "prose"},
        )
        self.assertEqual(rc, 0)
        self.assertEqual(seen, [["chore 2240"]])
        self.assertIn("chore 2240 (free) -> /dev/t0", out)
        self.assertIn("held  task 2163: longest build wait", out)
        self.assertEqual((directory / "2240.class").read_text(), "free\n")

    def test_an_all_held_batch_opens_nothing(self):
        refused = reading(slots=0, reasons=["total"], building=20)
        rc, seen, out, _, _ = self._run("task", "1", readings=(refused,))
        self.assertEqual((rc, seen), (0, []))
        self.assertIn("held  task 1: 20 clients of the total 20", out)

    def test_the_roll_call_prints_the_clients_line(self):
        rc, _, out, _, _ = self._run("task", "1", readings=(reading(building=3),))
        self.assertEqual(rc, 0)
        self.assertIn("clients 4 of 20 (builders 4, free 0)", out)

    def test_the_class_flag_reaches_the_chore(self):
        rc, seen, out, _, directory = self._run(
            "--class", "building", "chore", "3", bodies={"3": "prose"}
        )
        self.assertEqual(rc, 0)
        self.assertIn("chore 3 (builds: --class) -> /dev/t0", out)
        self.assertEqual((directory / "3.class").read_text(), "building\n")

    def test_a_trailing_gate_flag_is_refused(self):
        for flag in ("--over-budget", "--class=free"):
            with self.subTest(flag=flag):
                rc, seen, _, err, _ = self._run("task", "1", flag)
                self.assertEqual((rc, seen), (2, []))
                self.assertIn("must come BEFORE the verb", err)

    def test_no_reading_still_dispatches_and_says_why(self):
        with unittest.mock.patch.dict(os.environ, FLEET):
            rc, seen, _, err, _ = self._run("task", "1", readings=(NO_READING,))
        self.assertEqual((rc, seen), (0, [["task 1"]]))
        self.assertIn("no machine reading (not Darwin (Linux))", err)


if __name__ == "__main__":
    unittest.main()
