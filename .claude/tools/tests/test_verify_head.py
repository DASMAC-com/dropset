# cspell:word waitpid
"""Stdlib ``unittest`` tests for the HEAD verification race.

The race runs against fake stages, a scripted CI read, a scripted HEAD and a
fake clock, so every settlement order — local first, CI first, CI skipped,
either side failing, HEAD moved, CI unreachable, timeout — and the verdict
line's shape are asserted without a process, a network call or a wait. Run
via ``make tools-tests``.
"""

from __future__ import annotations

import os
import re
import signal
import sys
import tempfile
import time
import unittest
from pathlib import Path

import verify_head as vh
import wait_for_checks as wfc

SHA = "0123456789abcdef0123456789abcdef01234567"
PEND = ("pending", "")
GREEN_LINT = ("green", "Lint")
GREEN_TEST = ("green", "Tests,Tests (no teardown)")


class Clock:
    """Advances one second per sleep, the race's tick."""

    def __init__(self):
        self.now = 0.0

    def __call__(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        self.now += seconds


class FakeStage:
    """Exits with ``code`` once ``ticks`` polls have passed; never exits on None."""

    def __init__(self, name: str, ticks: int | None, code: int, log: Path):
        self.name, self.ticks, self.code, self.log = name, ticks, code, log
        self.polls = 0
        self.killed = False

    def poll(self):
        self.polls += 1
        if self.ticks is not None and self.polls > self.ticks:
            return self.code
        return None

    def kill(self):
        self.killed = True


class Harness:
    def __init__(
        self,
        stages,
        ci_script=(),
        heads=None,
        local_only="",
        timeout=600,
        dirty=lambda: False,
        interval=30,
    ):
        self.dirty = dirty
        self.interval = interval
        self.tmp = Path(tempfile.mkdtemp())
        self.specs = stages  # name -> (ticks, code, log text)
        self.started: dict[str, FakeStage] = {}
        self.ci_script = list(ci_script)  # one entry per CI read
        self.heads = heads or (lambda now: SHA)
        self.clock = Clock()
        self.local_only = local_only
        self.timeout = timeout
        self.ci_reads = 0

    def start(self, name):
        ticks, code, text = self.specs[name]
        log = self.tmp / f"{name}.log"
        log.write_text(text)
        self.started[name] = FakeStage(name, ticks, code, log)
        return self.started[name]

    def ci(self):
        self.ci_reads += 1
        entry = self.ci_script.pop(0) if self.ci_script else None
        if isinstance(entry, Exception):
            raise entry
        return entry

    def run(self) -> vh.Race:
        return vh.race(
            SHA,
            self.local_only,
            start=self.start,
            ci=self.ci,
            head=lambda: self.heads(self.clock.now),
            dirty=self.dirty,
            clock=self.clock,
            sleep=self.clock.sleep,
            interval=self.interval,
            timeout=self.timeout,
        )


def ci(lint=PEND, test=PEND):
    return {"lint": lint, "test": test}


class SettlementOrderTest(unittest.TestCase):
    def test_local_first_both_green(self):
        h = Harness({"lint": (3, 0, "ok\n"), "test": (3, 0, "ok\n")}, [ci()] * 5)
        r = h.run()
        self.assertEqual(r.outcome, "GREEN")
        self.assertEqual(
            {n: s.side for n, s in r.settled.items()},
            {"lint": "local", "test": "local"},
        )
        self.assertEqual(r.killed, [])
        self.assertEqual(list(h.started), ["lint", "test"])

    def test_ci_lint_first_kills_local_lint(self):
        h = Harness(
            {"lint": (None, 0, ""), "test": (2, 0, "")}, [ci(), ci(lint=GREEN_LINT)]
        )
        r = h.run()
        self.assertEqual(r.outcome, "GREEN")
        self.assertEqual(r.settled["lint"].side, "ci")
        self.assertEqual(r.settled["test"].side, "local")
        self.assertTrue(h.started["lint"].killed)
        self.assertEqual(r.killed, ["lint"])

    def test_ci_tests_first_kills_local_test(self):
        h = Harness(
            {"lint": (2, 0, ""), "test": (None, 0, "")}, [ci(), ci(test=GREEN_TEST)]
        )
        r = h.run()
        self.assertEqual(r.outcome, "GREEN")
        self.assertEqual(r.settled["test"].side, "ci")
        self.assertEqual(r.killed, ["test"])

    def test_ci_filtered_tests_pass_before_local_test_starts(self):
        # The code filter reports the Tests jobs as passing within seconds, so
        # CI settles the test check while lint is still local; the local test
        # stage is then never started.
        h = Harness(
            {"lint": (2, 0, ""), "test": (None, 0, "")},
            [
                ci(
                    test=vh.bucket_state(
                        [{"name": n, "bucket": "pass"} for n in vh.TEST_JOBS],
                        vh.CI_TEST,
                        vh.TEST_JOBS,
                    )
                )
            ],
        )
        r = h.run()
        self.assertEqual(r.outcome, "GREEN")
        self.assertNotIn("test", h.started)
        self.assertEqual(r.killed, [])

    def test_ci_skipped_tests_leave_the_local_test_to_settle(self):
        # `skipping` means Detect changes failed: nothing was verified.
        skipped = [{"name": n, "bucket": "skipping"} for n in vh.TEST_JOBS]
        state = vh.bucket_state(skipped, vh.CI_TEST, vh.TEST_JOBS)
        self.assertEqual(state, PEND)
        h = Harness({"lint": (1, 0, ""), "test": (2, 0, "")}, [ci(test=state)] * 5)
        r = h.run()
        self.assertEqual(r.settled["test"].side, "local")

    def test_tree_dirtied_mid_race_stops_believing_ci(self):
        h = Harness(
            # Lint exits on the third tick's poll; the green read would be the
            # fourth, after the rewrite left the tree dirty.
            {"lint": (1, 0, ""), "test": (None, 0, "")},
            [ci(), ci(), ci(), ci(test=GREEN_TEST)],
            dirty=lambda: True,
            interval=1,
            timeout=20,
        )
        r = h.run()
        self.assertEqual(r.ci_note, vh.DIRTY)
        self.assertEqual(r.outcome, "TIMEOUT")
        self.assertNotIn("test", r.settled)

    def test_ci_failure_kills_local_and_names_job(self):
        h = Harness(
            {"lint": (None, 0, ""), "test": (None, 0, "")},
            [ci(), ci(test=("red", "Tests (Postgres)"))],
        )
        r = h.run()
        self.assertEqual(r.outcome, "RED")
        self.assertEqual(r.settled["test"].detail, "Tests (Postgres)")
        self.assertTrue(h.started["lint"].killed)
        self.assertNotIn("lint", r.settled)

    def test_local_failure_names_log_line_and_leaves_ci(self):
        text = "check yaml........Passed\ncspell..........Failed\n- hook id: x\n"
        h = Harness({"lint": (2, 1, text), "test": (2, 0, "")}, [ci()] * 3)
        r = h.run()
        self.assertEqual(r.outcome, "RED")
        self.assertEqual(r.settled["lint"].side, "local")
        self.assertTrue(r.settled["lint"].detail.endswith(":2: cspell..........Failed"))
        self.assertNotIn("test", h.started)
        self.assertEqual(r.killed, [])

    def test_local_test_failure(self):
        text = "running 3 tests\ntest result: FAILED. 2 passed; 1 failed\n"
        h = Harness({"lint": (1, 0, ""), "test": (2, 101, text)}, [ci()] * 3)
        r = h.run()
        self.assertEqual(r.outcome, "RED")
        self.assertTrue(r.settled["test"].detail.endswith(":2: " + text.split("\n")[1]))

    def test_head_moved_kills_local(self):
        h = Harness(
            {"lint": (None, 0, ""), "test": (None, 0, "")},
            [ci()] * 9,
            heads=lambda now: SHA if now < 5 else "f" * 40,
        )
        r = h.run()
        self.assertEqual(r.outcome, "HEAD-MOVED")
        self.assertEqual(r.killed, ["lint"])

    def test_ci_unreachable_falls_back_to_local(self):
        err = wfc.WaitForChecksError("network down")
        h = Harness({"lint": (100, 0, ""), "test": (10, 0, "")}, [err] * 9)
        r = h.run()
        self.assertEqual(r.outcome, "GREEN")
        self.assertEqual(r.ci_note, vh.UNREACHABLE)
        self.assertEqual(h.ci_reads, vh.MAX_CI_ERRORS)

    def test_one_failed_read_does_not_stop_polling(self):
        err = wfc.WaitForChecksError("blip")
        h = Harness(
            {"lint": (None, 0, ""), "test": (None, 0, "")},
            [err, err, ci(GREEN_LINT, GREEN_TEST)],
        )
        r = h.run()
        self.assertEqual(r.outcome, "GREEN")
        self.assertEqual(r.ci_note, "consulted")

    def test_timeout_kills_local(self):
        h = Harness({"lint": (None, 0, ""), "test": (None, 0, "")}, [], timeout=90)
        r = h.run()
        self.assertEqual(r.outcome, "TIMEOUT")
        self.assertEqual(r.killed, ["lint"])

    def test_unpushed_or_dirty_never_reads_ci(self):
        for reason in (vh.NOT_PUSHED, vh.DIRTY):
            h = Harness({"lint": (1, 0, ""), "test": (1, 0, "")}, local_only=reason)
            r = h.run()
            self.assertEqual(r.outcome, "GREEN")
            self.assertEqual(h.ci_reads, 0)
            self.assertEqual(r.ci_note, reason)

    def test_ci_on_another_head_is_pending(self):
        h = Harness({"lint": (40, 0, ""), "test": (1, 0, "")}, [None] * 5)
        r = h.run()
        self.assertEqual({s.side for s in r.settled.values()}, {"local"})


class BucketStateTest(unittest.TestCase):
    def test_states(self):
        checks = [
            {"name": "Lint", "bucket": "pass"},
            {"name": "Tests", "bucket": "pass"},
            {"name": "Tests (Postgres)", "bucket": "pending"},
            {"name": "Detect changes", "bucket": "pass"},
        ]
        lint = (vh.CI_LINT, vh.LINT_JOBS)
        test = (vh.CI_TEST, vh.TEST_JOBS)
        self.assertEqual(vh.bucket_state(checks, *lint), ("green", "Lint"))
        self.assertEqual(vh.bucket_state(checks, *test), PEND)
        checks[2]["bucket"] = "cancel"
        self.assertEqual(vh.bucket_state(checks, *test), ("red", "Tests (Postgres)"))
        self.assertEqual(vh.bucket_state([], *lint), PEND)

    def test_registered_subset_passing_is_pending(self):
        checks = [{"name": "Tests", "bucket": "pass"}]
        self.assertEqual(vh.bucket_state(checks, vh.CI_TEST, vh.TEST_JOBS), PEND)
        checks = [{"name": n, "bucket": "pass"} for n in sorted(vh.TEST_JOBS)]
        self.assertEqual(vh.bucket_state(checks, vh.CI_TEST, vh.TEST_JOBS)[0], "green")

    def test_expected_jobs_match_the_workflows(self):
        # A renamed job would leave a check pending forever (CI never settles
        # it), so pin the sets to the workflows' own job names.
        root = Path(__file__).resolve().parents[3] / ".github" / "workflows"
        named = re.compile(r"^    name: '([^']+)'$", re.MULTILINE)
        tests = {n for n in named.findall((root / "test.yml").read_text())}
        lint = set(named.findall((root / "lint.yml").read_text()))
        self.assertEqual({n for n in tests if vh.CI_TEST.search(n)}, set(vh.TEST_JOBS))
        self.assertLessEqual(set(vh.LINT_JOBS), lint)


class LocalOnlyReasonTest(unittest.TestCase):
    def check(self, upstream, status):
        answers = {"rev-parse": upstream, "status": status}
        real = vh._git
        vh._git = lambda args: answers[args[0]]
        try:
            return vh.local_only_reason(SHA)
        finally:
            vh._git = real

    def test_reasons(self):
        self.assertEqual(self.check("", ""), vh.NOT_PUSHED)
        self.assertEqual(self.check("f" * 40, ""), vh.NOT_PUSHED)
        self.assertEqual(self.check(SHA, " M docs/ci.md"), vh.DIRTY)
        self.assertEqual(self.check(SHA, ""), "")


class ReadCiTest(unittest.TestCase):
    def setUp(self):
        self.calls = []
        self._gh, self._read = wfc._gh, wfc.read_checks
        self.addCleanup(setattr, wfc, "_gh", self._gh)
        self.addCleanup(setattr, wfc, "read_checks", self._read)

    def stub(self, code, out, checks=()):
        wfc._gh = lambda args: self.calls.append(args) or (code, out, "boom")
        wfc.read_checks = lambda pr, repo: list(checks)

    def test_other_head_is_none(self):
        self.stub(0, '{"number": 7, "headRefOid": "other"}')
        self.assertIsNone(vh.read_ci(SHA, "7", "o/r"))
        self.assertEqual(self.calls[0][:3], ["pr", "view", "7"])

    def test_this_head_reads_checks(self):
        self.stub(
            0,
            '{"number": 7, "headRefOid": "%s"}' % SHA,
            [{"name": "Lint", "bucket": "fail"}],
        )
        self.assertEqual(vh.read_ci(SHA, "7", "o/r")["lint"], ("red", "Lint"))

    def test_failures_raise_the_watcher_error(self):
        for code, out in ((1, ""), (0, "[]")):
            self.stub(code, out)
            with self.assertRaises(wfc.WaitForChecksError):
                vh.read_ci(SHA, "7", "o/r")


class VerdictLineTest(unittest.TestCase):
    def test_shape(self):
        r = vh.Race(sha=SHA, outcome="GREEN", ci_note="consulted", killed=["lint"])
        r.settled["lint"] = vh.Settled("ci", True, "Lint")
        r.settled["test"] = vh.Settled("local", True, "/tmp/t.log")
        self.assertEqual(
            vh.render(r, 211.6),
            "verify-head: GREEN sha=0123456789ab lint=ci:green[Lint] "
            "test=local:green[/tmp/t.log] wall=212s killed=lint ci=consulted",
        )

    def test_unsettled_and_unpushed(self):
        r = vh.Race(sha=SHA, outcome="RED", ci_note=vh.NOT_PUSHED)
        r.settled["lint"] = vh.Settled("local", False, "/l.log:3: x.Failed")
        line = vh.render(r, 5)
        self.assertIn("lint=local:red[/l.log:3: x.Failed] test=-:unsettled", line)
        self.assertTrue(line.endswith("killed=- ci=" + vh.NOT_PUSHED))


class FailureLineTest(unittest.TestCase):
    def test_falls_back_to_last_line(self):
        log = Path(tempfile.mkdtemp()) / "x.log"
        log.write_text("compiling\nmake: *** [x] Error 2\n\n")
        self.assertEqual(vh.failure_line(log), f"{log}:2: make: *** [x] Error 2")

    def test_rust_error(self):
        log = Path(tempfile.mkdtemp()) / "x.log"
        log.write_text("warning: x\nerror[E0425]: cannot find value\n")
        self.assertTrue(
            vh.failure_line(log).endswith(":2: error[E0425]: cannot find value")
        )


@unittest.skipUnless(hasattr(os, "killpg"), "needs process groups")
class StageKillTest(unittest.TestCase):
    def test_kill_takes_the_whole_group(self):
        log = Path(tempfile.mkdtemp()) / "s.log"
        # The leader spawns a grandchild sleeper and prints its pid.
        code = (
            "import subprocess,sys,time;"
            "p=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)']);"
            "print(p.pid,flush=True);time.sleep(60)"
        )
        stage = vh.Stage("lint", [sys.executable, "-c", code], log)
        deadline = time.monotonic() + 10
        while not log.read_text().strip() and time.monotonic() < deadline:
            time.sleep(0.05)
        grandchild = int(log.read_text().split()[0])
        stage.kill()
        self.assertIsNotNone(stage.poll())
        for _ in range(100):
            try:
                os.kill(grandchild, 0)
            except ProcessLookupError:
                break
            try:  # reap if it is ours to reap; otherwise init does
                os.waitpid(grandchild, os.WNOHANG)
            except ChildProcessError:
                pass
            time.sleep(0.05)
        else:
            os.kill(grandchild, signal.SIGKILL)
            self.fail("grandchild survived the group kill")

    def exited_stage(self) -> vh.Stage:
        log = Path(tempfile.mkdtemp()) / "s.log"
        stage = vh.Stage("lint", [sys.executable, "-c", "pass"], log)
        stage.proc.wait(timeout=10)
        return stage

    def test_kill_after_the_leader_exited(self):
        stage = self.exited_stage()
        stage.kill()
        stage.force()
        self.assertEqual(stage.poll(), 0)

    def test_eperm_on_a_gone_group_is_tolerated(self):
        # macOS answers EPERM, not ESRCH, for a zombie-only or recycled group.
        stage = self.exited_stage()
        calls = []

        def eperm(pgid, sig):
            calls.append(sig)
            raise PermissionError(1, "Operation not permitted")

        real, os.killpg = os.killpg, eperm
        try:
            stage.kill()
            stage.force()
        finally:
            os.killpg = real
        self.assertEqual(calls, [signal.SIGTERM, signal.SIGKILL])

    def test_race_ci_first_with_eperm_prints_a_verdict(self):
        # CI settles lint while the real local lint runs; the kill hits EPERM.
        tmp = Path(tempfile.mkdtemp())
        reads = iter([ci(), ci(lint=GREEN_LINT), ci(test=GREEN_TEST)])
        real_killpg = os.killpg

        def start(name):
            return vh.Stage(
                name,
                [sys.executable, "-c", "import time;time.sleep(30)"],
                tmp / f"{name}.log",
            )

        def killpg(pgid, sig):
            real_killpg(pgid, sig)
            if sig == signal.SIGKILL:
                raise PermissionError(1, "Operation not permitted")

        clock = Clock()
        os.killpg = killpg
        try:
            r = vh.race(
                SHA,
                "",
                start=start,
                ci=lambda: next(reads, None),
                head=lambda: SHA,
                dirty=lambda: False,
                clock=clock,
                sleep=clock.sleep,
                interval=1,
                timeout=60,
            )
        finally:
            os.killpg = real_killpg
        self.assertEqual(r.outcome, "GREEN")
        self.assertEqual(r.killed, ["lint", "test"])
        self.assertIn("verify-head: GREEN", vh.render(r, 1))


class MainSignalTest(unittest.TestCase):
    """The handler path: an interrupt mid-race cleans up every started group."""

    def setUp(self):
        names = ("race", "read_head", "local_only_reason", "Stage", "STAGES")
        self.saved = {n: getattr(vh, n) for n in names}
        self.handlers = {
            s: signal.getsignal(s)
            for s in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT)
        }
        vh.read_head = lambda: SHA
        vh.local_only_reason = lambda sha: vh.NOT_PUSHED

    def stages(self, code: str) -> None:
        vh.STAGES = (("lint", [sys.executable, "-c", code]),)

    def tearDown(self):
        for name, value in self.saved.items():
            setattr(vh, name, value)
        for sig, handler in self.handlers.items():
            signal.signal(sig, handler)

    def test_interrupt_after_a_stage_exited_raises_only_the_exit(self):
        started = []

        def race(sha, local_only, *, start, **_):
            started.append(start("lint"))
            started[-1].proc.wait(timeout=10)
            os.kill(os.getpid(), signal.SIGTERM)
            time.sleep(5)
            raise AssertionError("SIGTERM was not delivered")

        def eperm(pgid, sig):
            raise PermissionError(1, "Operation not permitted")

        self.stages("pass")
        vh.race = race
        real_killpg, os.killpg = os.killpg, eperm
        try:
            with self.assertRaises(SystemExit) as caught:
                vh.main([])
        finally:
            os.killpg = real_killpg
        self.assertEqual(caught.exception.code, 128 + signal.SIGTERM)
        self.assertEqual(started[0].poll(), 0)

    def test_signal_during_spawn_is_held_until_recorded(self):
        recorded = []

        class SignalledStage(self.saved["Stage"]):
            def __init__(self, *a):
                super().__init__(*a)
                os.kill(os.getpid(), signal.SIGTERM)  # inside the window
                recorded.append(self)

        def race(sha, local_only, *, start, **_):
            start("lint")
            raise AssertionError("the held signal was not raised")

        self.stages("import time;time.sleep(30)")
        vh.Stage = SignalledStage
        vh.race = race
        with self.assertRaises(SystemExit) as caught:
            vh.main([])
        self.assertEqual(caught.exception.code, 128 + signal.SIGTERM)
        self.assertIsNotNone(recorded[0].poll())  # recorded, then killed


if __name__ == "__main__":
    unittest.main()
