# cspell:word waitpid
# cspell:word WNOHANG
"""Stdlib ``unittest`` tests for the HEAD verification race.

The race runs against fake stages, a scripted CI read, a scripted HEAD and a
fake clock, so every settlement order — local first, CI first, CI skipped,
either side failing, HEAD moved, CI unreachable, timeout — and the verdict
line's shape are asserted without a process, a network call or a wait. Run
via ``make tools-tests``.
"""

from __future__ import annotations

import os
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
    def __init__(self, stages, ci_script=(), heads=None, local_only="", timeout=600):
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
            clock=self.clock,
            sleep=self.clock.sleep,
            interval=30,
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

    def test_ci_tests_skipped_before_local_test_starts(self):
        # Paths-filter skip settles the test check while lint is still local;
        # the local test stage is then never started.
        h = Harness(
            {"lint": (2, 0, ""), "test": (None, 0, "")},
            [
                ci(
                    test=vh.bucket_state(
                        [{"name": n, "bucket": "skipping"} for n in vh.TEST_JOBS],
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
        self.assertEqual(
            vh.bucket_state(checks, vh.CI_TEST, vh.TEST_JOBS)[0], "green"
        )


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
        wfc._gh = lambda args: (self.calls.append(args) or (code, out, "boom"))
        wfc.read_checks = lambda pr, repo: list(checks)

    def test_other_head_is_none(self):
        self.stub(0, '{"number": 7, "headRefOid": "other"}')
        self.assertIsNone(vh.read_ci(SHA, "7", "o/r"))
        self.assertEqual(self.calls[0][:3], ["pr", "view", "7"])

    def test_this_head_reads_checks(self):
        self.stub(0, '{"number": 7, "headRefOid": "%s"}' % SHA,
                  [{"name": "Lint", "bucket": "fail"}])
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


if __name__ == "__main__":
    unittest.main()
