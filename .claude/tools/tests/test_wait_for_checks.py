"""Stdlib ``unittest`` tests for the CI-wait tool.

The two ``gh`` invocations are stubbed — these cover the parts that decide an
outcome: the bucket→conclusion precedence, the failing-check extraction, the
empty-vs-error distinction in ``gh``'s overloaded exit codes, and the rule that a
timed-out watch never reports ``pass``. Run via the repo's ``make tools-tests``.
"""

from __future__ import annotations

import io
import json
import os
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

import wait_for_checks as wfc


def check(name, bucket, workflow="Tests", link="", description=""):
    return {
        "name": name,
        "bucket": bucket,
        "state": bucket.upper(),
        "workflow": workflow,
        "link": link,
        "description": description,
        "completedAt": "",
    }


class SummarizeTests(unittest.TestCase):
    """Buckets reduce to one conclusion under a fixed precedence."""

    def test_all_pass(self):
        out = wfc.summarize([check("a", "pass"), check("b", "pass")])
        self.assertEqual(out["conclusion"], "pass")
        self.assertEqual(out["counts"], {"pass": 2})
        self.assertEqual(out["failing"], [])

    def test_a_failure_wins_over_a_pending(self):
        """One red check and one still-queued check reads as failing, not as
        "not done yet" — otherwise a review waits on a build already lost."""
        out = wfc.summarize([check("a", "fail"), check("b", "pending")])
        self.assertEqual(out["conclusion"], "fail")

    def test_pending_wins_over_pass(self):
        out = wfc.summarize([check("a", "pass"), check("b", "pending")])
        self.assertEqual(out["conclusion"], "pending")

    def test_skipping_is_not_a_failure(self):
        """A path-filtered no-op job is the normal case on this repo, since
        test.yml's filter is pull_request-only."""
        out = wfc.summarize([check("a", "pass"), check("b", "skipping")])
        self.assertEqual(out["conclusion"], "pass")
        self.assertEqual(out["counts"], {"pass": 1, "skipping": 1})

    def test_a_cancelled_check_is_not_green(self):
        """A cancelled required check still blocks the merge queue, so it must not
        fall through to `pass` — the module's whole promise is that a caller
        checking only the exit status cannot mistake a red build for green."""
        out = wfc.summarize([check("a", "pass"), check("b", "cancel")])
        self.assertEqual(out["conclusion"], "blocked")
        self.assertEqual(out["counts"]["cancel"], 1)
        self.assertEqual(out["unresolved_buckets"], ["cancel"])

    def test_an_unrecognized_bucket_is_not_green(self):
        """A gh schema change must fail loud, not silently read as passing."""
        out = wfc.summarize([check("a", "pass"), check("b", "some_new_bucket")])
        self.assertEqual(out["conclusion"], "blocked")
        self.assertEqual(out["unresolved_buckets"], ["some_new_bucket"])

    def test_a_failure_still_outranks_an_unresolved_bucket(self):
        out = wfc.summarize([check("a", "fail"), check("b", "cancel")])
        self.assertEqual(out["conclusion"], "fail")

    def test_all_pass_has_no_unresolved_buckets(self):
        out = wfc.summarize([check("a", "pass"), check("b", "skipping")])
        self.assertEqual(out["unresolved_buckets"], [])

    def test_no_checks_is_none_not_pass(self):
        out = wfc.summarize([])
        self.assertEqual(out["conclusion"], "none")
        self.assertEqual(out["counts"], {})

    def test_unknown_bucket_is_counted_not_dropped(self):
        out = wfc.summarize([{"name": "x"}])
        self.assertEqual(out["counts"], {"unknown": 1})
        self.assertEqual(out["conclusion"], "blocked")

    def test_failing_checks_carry_workflow_and_run_id(self):
        link = "https://github.com/DASMAC-com/dropset/actions/runs/12345/job/999"
        out = wfc.summarize([check("Tests (sbf)", "fail", "Tests", link)])
        self.assertEqual(len(out["failing"]), 1)
        failing = out["failing"][0]
        self.assertEqual(failing["name"], "Tests (sbf)")
        self.assertEqual(failing["workflow"], "Tests")
        self.assertEqual(failing["run_id"], "12345")

    def test_failing_sorted_by_workflow_then_name(self):
        out = wfc.summarize(
            [
                check("z", "fail", "Lint"),
                check("a", "fail", "Tests"),
                check("a", "fail", "Lint"),
            ]
        )
        got = [(f["workflow"], f["name"]) for f in out["failing"]]
        self.assertEqual(got, [("Lint", "a"), ("Lint", "z"), ("Tests", "a")])

    def test_pending_checks_names_what_is_outstanding(self):
        """A pending verdict whose only detail is a count sends the reader back
        to gh to find out which check it is waiting on."""
        out = wfc.summarize([check("a", "pass"), check("Explorer image", "pending")])
        self.assertEqual(out["conclusion"], "pending")
        self.assertEqual(out["pending_checks"], ["Explorer image"])

    def test_pending_checks_is_empty_when_nothing_is_outstanding(self):
        self.assertEqual(wfc.summarize([check("a", "pass")])["pending_checks"], [])


class RunIdTests(unittest.TestCase):
    def test_extracts_from_a_job_link(self):
        self.assertEqual(
            wfc.run_id_from_link("https://x/actions/runs/778899/job/1"), "778899"
        )

    def test_extracts_from_a_run_link(self):
        self.assertEqual(wfc.run_id_from_link("https://x/actions/runs/42"), "42")

    def test_missing_is_empty(self):
        self.assertEqual(wfc.run_id_from_link("https://x/checks"), "")
        self.assertEqual(wfc.run_id_from_link(""), "")


class GhCallTests(unittest.TestCase):
    def test_a_hung_gh_is_refused_in_plain_words(self):
        seen = {}

        def hang(cmd, **kwargs):
            seen.update(kwargs)
            raise wfc.subprocess.TimeoutExpired(cmd, kwargs["timeout"])

        real, wfc.subprocess.run = wfc.subprocess.run, hang
        try:
            with self.assertRaises(wfc.WaitForChecksError) as caught:
                wfc._gh(["pr", "view", "656", "--json", "number"])
        finally:
            wfc.subprocess.run = real
        self.assertEqual(seen["timeout"], wfc.GH_CALL_TIMEOUT)
        self.assertEqual(
            str(caught.exception),
            f"gh pr view did not answer within {wfc.GH_CALL_TIMEOUT}s",
        )


class ReadChecksTests(unittest.TestCase):
    """``gh`` overloads its exit code, so an empty payload has to be classified
    rather than trusted."""

    def _stub(self, code, out, err=""):
        def fake(args):
            return code, out, err

        self._real = wfc._gh
        wfc._gh = fake
        self.addCleanup(setattr, wfc, "_gh", self._real)

    def test_parses_a_list(self):
        self._stub(0, json.dumps([check("a", "pass")]))
        got = wfc.read_checks(285, "o/r")
        self.assertEqual(len(got), 1)
        self.assertEqual(got[0]["name"], "a")

    def test_ghs_no_checks_message_means_no_checks(self):
        """Keyed on gh's message, not its exit code — the code for "no checks" has
        varied across gh versions, and 8 is documented as *pending*."""
        self._stub(1, "", "no checks reported on the 'eng-798' branch")
        self.assertEqual(wfc.read_checks(285, "o/r"), [])

    def test_empty_payload_on_exit_zero_means_no_checks(self):
        self._stub(0, "")
        self.assertEqual(wfc.read_checks(285, "o/r"), [])

    def test_other_nonzero_with_no_payload_is_an_error(self):
        """A bad PR number or missing auth must NOT read as "no checks" — the
        caller is told to treat "none" as green, so conflating them ships a red
        build. This must fail loud."""
        self._stub(1, "", "could not find pull request")
        with self.assertRaises(wfc.WaitForChecksError):
            wfc.read_checks(285, "o/r")

    def test_an_auth_failure_is_an_error_not_no_checks(self):
        self._stub(4, "", "gh: authentication required")
        with self.assertRaises(wfc.WaitForChecksError):
            wfc.read_checks(285, "o/r")

    def test_nonzero_with_a_payload_is_still_parsed(self):
        """Non-zero also means "a check failed", which is an outcome to report."""
        self._stub(1, json.dumps([check("a", "fail")]))
        got = wfc.read_checks(285, "o/r")
        self.assertEqual(got[0]["bucket"], "fail")

    def test_malformed_json_errors(self):
        self._stub(0, "{not json")
        with self.assertRaises(wfc.WaitForChecksError):
            wfc.read_checks(285, "o/r")

    def test_non_list_json_errors(self):
        self._stub(0, json.dumps({"checks": []}))
        with self.assertRaises(wfc.WaitForChecksError):
            wfc.read_checks(285, "o/r")


class WaitTests(unittest.TestCase):
    """The whole wait, with both gh calls stubbed."""

    def _stub(self, checks, settled=True):
        wfc_real_watch = wfc.watch_checks
        wfc_real_read = wfc.read_checks
        # Neutralize the inter-round pacing. It is real seconds in production,
        # and a `none` read now re-enters the watch (the registration race), so
        # without this every check-less case here would sleep through its cap.
        real_sleep = wfc.time.sleep
        wfc.watch_checks = lambda pr, repo, interval, timeout, log: settled
        wfc.read_checks = lambda pr, repo: checks
        wfc.time.sleep = lambda _s: None
        # The failure diagnosis and the conflict probe both read through gh;
        # keep the tests network-free.
        real_gh = wfc._gh
        wfc._gh = lambda args: (1, "", "stubbed")
        self.addCleanup(setattr, wfc, "watch_checks", wfc_real_watch)
        self.addCleanup(setattr, wfc, "read_checks", wfc_real_read)
        self.addCleanup(setattr, wfc.time, "sleep", real_sleep)
        self.addCleanup(setattr, wfc, "_gh", real_gh)

    def test_green_build(self):
        self._stub([check("a", "pass")])
        v = wfc.wait(285, repo="o/r")
        self.assertEqual(v["conclusion"], "pass")
        self.assertTrue(v["settled"])
        self.assertEqual(v["pr"], 285)
        self.assertIn("wait-for-checks-285.log", v["log_path"])

    def test_red_build_lists_the_failures(self):
        link = "https://x/actions/runs/7/job/8"
        self._stub([check("Tests (sbf)", "fail", "Tests", link)])
        v = wfc.wait(285, repo="o/r")
        self.assertEqual(v["conclusion"], "fail")
        self.assertEqual(v["failing"][0]["run_id"], "7")

    def test_a_timed_out_watch_never_reports_pass(self):
        """It reports the state it reached, but must not claim green off a
        snapshot it stopped waiting on."""
        self._stub([check("a", "pass")], settled=False)
        v = wfc.wait(285, repo="o/r")
        self.assertEqual(v["conclusion"], "timeout")
        self.assertFalse(v["settled"])
        # the counts it did observe are still reported
        self.assertEqual(v["counts"], {"pass": 1})

    def test_a_timed_out_watch_still_reports_a_definite_failure(self):
        """A `fail` it did observe is definite and more informative than
        `timeout` — a caller must be able to tell a wedged run from a red one."""
        self._stub([check("a", "fail")], settled=False)
        v = wfc.wait(285, repo="o/r")
        self.assertEqual(v["conclusion"], "fail")
        self.assertFalse(v["settled"])

    def test_no_watch_skips_the_wait(self):
        called = []
        wfc_real = wfc.watch_checks

        def fake_watch(*a, **k):
            called.append(a)
            return True

        wfc.watch_checks = fake_watch
        wfc_real_read = wfc.read_checks
        wfc.read_checks = lambda pr, repo: [check("a", "pending")]
        self.addCleanup(setattr, wfc, "watch_checks", wfc_real)
        self.addCleanup(setattr, wfc, "read_checks", wfc_real_read)

        v = wfc.wait(285, repo="o/r", watch=False)
        self.assertEqual(called, [])
        self.assertEqual(v["conclusion"], "pending")
        self.assertTrue(v["settled"])

    def _stub_rounds(self, reads, settled=True, clock=None):
        """Stub both gh calls with a *sequence* of reads — one per watch round.

        The final entry repeats, so a test wanting "pending forever" passes a
        single read. `time.sleep` is neutralized because the pacing between
        rounds is real seconds in production and would make this suite crawl.
        Pass `clock` (a one-element list standing in for the monotonic clock)
        to have the stubbed sleep advance it instead of discarding the delay —
        without that, wall time never moves and a test cannot observe the
        remaining budget shrink. Returns the timeouts handed to each
        `watch_checks` call.
        """
        queue = list(reads)
        real_watch = wfc.watch_checks
        real_read = wfc.read_checks
        real_sleep = wfc.time.sleep
        timeouts = []

        def fake_watch(pr, repo, interval, timeout, log):
            timeouts.append(timeout)
            return settled

        def fake_read(pr, repo):
            return queue.pop(0) if len(queue) > 1 else queue[0]

        self.sleeps = []

        def fake_sleep(seconds):
            # Recorded, not just discarded: the per-state PACING (a `none`
            # settles in seconds, a `pending` waits a whole interval) is
            # otherwise unobservable, and a test that neutralizes sleep without
            # recording it cannot tell the two apart.
            self.sleeps.append(seconds)
            if clock is not None:
                clock[0] += seconds

        wfc.watch_checks = fake_watch
        wfc.read_checks = fake_read
        wfc.time.sleep = fake_sleep
        self.addCleanup(setattr, wfc, "watch_checks", real_watch)
        self.addCleanup(setattr, wfc, "read_checks", real_read)
        self.addCleanup(setattr, wfc.time, "sleep", real_sleep)
        return timeouts

    def _fake_clock(self):
        """Freeze `time.monotonic` onto a list the stubbed sleep advances."""
        clock = [0.0]
        real_monotonic = wfc.time.monotonic
        wfc.time.monotonic = lambda: clock[0]
        self.addCleanup(setattr, wfc.time, "monotonic", real_monotonic)
        return clock

    def test_a_late_registering_check_re_enters_the_watch(self):
        """gh's --watch settles on its *own* census of the check set, so a
        workflow that registers late is still pending when it returns. The
        verdict must come from a re-entered watch, not from that first read."""
        self._stub_rounds(
            [
                [check("a", "pass"), check("Explorer image", "pending")],
                [check("a", "pass"), check("Explorer image", "pass")],
            ]
        )
        v = wfc.wait(285, repo="o/r")
        self.assertEqual(v["conclusion"], "pass")
        self.assertTrue(v["settled"])
        self.assertEqual(v["watch_rounds"], 2)

    def test_a_check_pending_forever_times_out_rather_than_settling(self):
        self._stub_rounds([[check("a", "pass"), check("Explorer image", "pending")]])
        v = wfc.wait(285, repo="o/r")
        self.assertEqual(v["conclusion"], "timeout")
        self.assertFalse(v["settled"])
        self.assertEqual(v["watch_rounds"], wfc.MAX_WATCH_ROUNDS)
        self.assertEqual(v["pending_checks"], ["Explorer image"])

    def test_a_none_that_follows_observed_pending_is_a_timeout_not_an_absence(self):
        """The fail-open the per-state counters exist to close.

        `none` means "no checks apply to this PR", which the caller reads as
        nothing to wait for. `pending` means CI is running. A wait that saw
        pending twice and then `none` has watched checks *disappear* — an
        anomaly — and must never report that as an absence.

        With one shared round counter and a per-state cap this reported `none`:
        round 3 read `none`, compared rounds(3) against the `none` cap(3),
        exited with `exhausted_on == "none"`, and the terminal branch then
        preserved it. Reachable from the pending side, in the exact place the
        `none` retry was added to make safe.
        """
        self._stub_rounds(
            [
                [check("a", "pending")],
                [check("a", "pending")],
                [],
            ]
        )
        v = wfc.wait(285, repo="o/r")
        self.assertEqual(v["conclusion"], "timeout")
        self.assertFalse(v["settled"])

    def test_a_none_reached_without_any_pending_still_reports_none(self):
        """The other side of the same guard: an absence observed from the first
        round is a genuine absence, and must not be collapsed into `timeout`
        just because the counters were separated.
        """
        self._stub_rounds([[]])
        v = wfc.wait(285, repo="o/r")
        self.assertEqual(v["conclusion"], "none")
        self.assertFalse(v["settled"])

    def test_each_retryable_state_gets_its_own_pacing(self):
        """A `none` is a seconds-scale registration race and a `pending` is a
        whole CI run, so they must not share a delay. Nothing asserted this —
        the stubbed sleep discarded its argument.
        """
        self._stub_rounds([[]])
        wfc.wait(285, repo="o/r")
        self.assertTrue(self.sleeps)
        self.assertEqual(set(self.sleeps), {wfc.NONE_SETTLE_SECONDS})

        self.sleeps = []
        self._stub_rounds([[check("a", "pending")]])
        wfc.wait(285, repo="o/r", interval=17)
        self.assertTrue(self.sleeps)
        self.assertEqual(set(self.sleeps), {17})

    def test_settled_never_coexists_with_a_pending_conclusion(self):
        """The contradiction the retry exists to prevent: `settled: true` next
        to `conclusion: "pending"` is not a verdict, it is a bug."""
        self._stub_rounds([[check("Explorer image", "pending")]])
        v = wfc.wait(285, repo="o/r")
        self.assertFalse(v["settled"] and v["conclusion"] == "pending")

    def test_each_round_gets_only_the_remaining_budget(self):
        """The retry must not outlive the timeout the caller set: each round
        is handed what is *left*, not the original budget.

        The fake clock is what gives this test teeth — with wall time frozen,
        handing every round the full timeout would look identical to handing
        it the remainder, which is the bug being pinned.
        """
        clock = self._fake_clock()
        timeouts = self._stub_rounds([[check("a", "pending")]], clock=clock)
        wfc.wait(285, repo="o/r", timeout=600)
        self.assertGreater(len(timeouts), 1)
        self.assertLess(timeouts[1], timeouts[0])
        self.assertEqual(timeouts[0], 600)

    def test_an_exhausted_budget_never_watches_at_all(self):
        timeouts = self._stub_rounds([[check("a", "pass")]])
        v = wfc.wait(285, repo="o/r", timeout=0)
        self.assertEqual(timeouts, [])
        self.assertEqual(v["watch_rounds"], 0)
        self.assertEqual(v["conclusion"], "timeout")

    def test_a_settled_green_build_takes_exactly_one_round(self):
        """The retry is for the pending case only — it must not add a round to
        the common path, which would double every wait's gh calls."""
        self._stub_rounds([[check("a", "pass")]])
        v = wfc.wait(285, repo="o/r")
        self.assertEqual(v["watch_rounds"], 1)


class CliTests(unittest.TestCase):
    def _stub(self, checks):
        wfc_real_watch = wfc.watch_checks
        wfc_real_read = wfc.read_checks
        real_sleep = wfc.time.sleep
        wfc.watch_checks = lambda pr, repo, interval, timeout, log: True
        wfc.read_checks = lambda pr, repo: checks
        wfc.time.sleep = lambda _s: None
        self.addCleanup(setattr, wfc, "watch_checks", wfc_real_watch)
        self.addCleanup(setattr, wfc, "read_checks", wfc_real_read)
        self.addCleanup(setattr, wfc.time, "sleep", real_sleep)

    def _capture(self, argv):
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = wfc.run(argv)
        return code, json.loads(buf.getvalue())

    def test_exits_zero_on_pass(self):
        self._stub([check("a", "pass")])
        code, parsed = self._capture(["wait_for_checks.py", "--pr", "285"])
        self.assertEqual(code, 0)
        self.assertEqual(parsed["conclusion"], "pass")

    def test_exits_non_zero_on_fail(self):
        """A caller that only checks the status can't mistake red for green."""
        self._stub([check("a", "fail")])
        code, _ = self._capture(["wait_for_checks.py", "--pr", "285"])
        self.assertEqual(code, 1)

    def test_exits_non_zero_when_there_are_no_checks(self):
        self._stub([])
        code, parsed = self._capture(["wait_for_checks.py", "--pr", "285"])
        self.assertEqual(code, 1)
        self.assertEqual(parsed["conclusion"], "none")

    def test_defaults_to_the_repo(self):
        self._stub([check("a", "pass")])
        _, parsed = self._capture(["wait_for_checks.py", "--pr", "285"])
        self.assertEqual(parsed["repo"], "DASMAC-com/dropset")


class LogPathTests(unittest.TestCase):
    def test_directory_is_owner_only(self):
        path = wfc.log_path_for(1)
        self.assertEqual(path.parent.stat().st_mode & 0o777, 0o700)

    def test_path_is_per_pr(self):
        self.assertNotEqual(wfc.log_path_for(1), wfc.log_path_for(2))


class WatchChecksTests(unittest.TestCase):
    """`watch_checks` runs a real subprocess, so it gets a `gh` shim on PATH
    rather than being stubbed out — otherwise the log's mode and the timeout kill
    are never exercised."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.bin = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)
        self._path = os.environ.get("PATH", "")
        os.environ["PATH"] = f"{self.bin}{os.pathsep}{self._path}"
        self.addCleanup(os.environ.__setitem__, "PATH", self._path)
        self.log = self.bin / "watch.log"

    def _shim(self, body):
        gh = self.bin / "gh"
        gh.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
        gh.chmod(0o755)

    def test_settles_and_captures_output_to_the_log(self):
        self._shim("echo 'Tests  pass  1m0s'")
        settled = wfc.watch_checks(285, "o/r", 1, 30, self.log)
        self.assertTrue(settled)
        self.assertIn("pass", self.log.read_text(encoding="utf-8"))

    def test_the_log_is_owner_only(self):
        """A CI log can carry build output worth keeping off a shared /tmp — the
        same reason review_diff.py's slices are 0o600."""
        self._shim("echo hi")
        wfc.watch_checks(285, "o/r", 1, 30, self.log)
        self.assertEqual(self.log.stat().st_mode & 0o777, 0o600)

    def test_a_hung_watch_times_out_and_is_killed(self):
        self._shim("sleep 30")
        settled = wfc.watch_checks(285, "o/r", 1, 1, self.log)
        self.assertFalse(settled)

    def test_a_nonzero_watch_still_settles(self):
        """gh exits non-zero when a check failed; that is an outcome, not a
        failure to settle — the verdict comes from the separate JSON read."""
        self._shim("echo 'Tests  fail  1m0s'\nexit 1")
        self.assertTrue(wfc.watch_checks(285, "o/r", 1, 30, self.log))


class WatchRunTests(unittest.TestCase):
    """The merge-queue mode. Unlike the checks mode there is no second JSON
    read, so here gh's ``--exit-status`` code *is* the verdict — which is why a
    timeout must not be allowed to look like a pass."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.bin = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)
        self._path = os.environ.get("PATH", "")
        os.environ["PATH"] = f"{self.bin}{os.pathsep}{self._path}"
        self.addCleanup(os.environ.__setitem__, "PATH", self._path)
        self.log = self.bin / "run.log"

    def _shim(self, body):
        gh = self.bin / "gh"
        gh.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
        gh.chmod(0o755)

    def test_a_green_run_settles_zero_and_captures_the_tree(self):
        self._shim("echo 'X  build  1m0s'")
        settled, code = wfc.watch_run("99", "o/r", 30, self.log)
        self.assertTrue(settled)
        self.assertEqual(code, 0)
        self.assertIn("build", self.log.read_text(encoding="utf-8"))

    def test_a_dequeued_run_is_non_zero(self):
        """`--exit-status` is what keeps a dequeue from reading as a merge."""
        self._shim("exit 1")
        settled, code = wfc.watch_run("99", "o/r", 30, self.log)
        self.assertTrue(settled)
        self.assertNotEqual(code, 0)

    def test_a_hung_run_times_out_and_is_killed(self):
        self._shim("sleep 30")
        settled, code = wfc.watch_run("99", "o/r", 1, self.log)
        self.assertFalse(settled)
        self.assertEqual(code, -1)

    def test_the_log_is_owner_only(self):
        self._shim("echo hi")
        wfc.watch_run("99", "o/r", 30, self.log)
        self.assertEqual(self.log.stat().st_mode & 0o777, 0o600)

    def test_wait_run_maps_exit_codes_to_conclusions(self):
        cases = {(True, 0): "pass", (True, 1): "fail", (False, -1): "timeout"}
        for (settled, code), expected in cases.items():
            with self.subTest(settled=settled, code=code):
                real = wfc.watch_run
                wfc.watch_run = lambda *a, **k: (settled, code)
                try:
                    verdict = wfc.wait_run("99")
                finally:
                    wfc.watch_run = real
                self.assertEqual(verdict["conclusion"], expected)

    def test_a_timed_out_run_never_reads_as_a_pass(self):
        """A killed watch has exit code -1, and must not be mapped by sign."""
        real = wfc.watch_run
        wfc.watch_run = lambda *a, **k: (False, 0)
        try:
            verdict = wfc.wait_run("99")
        finally:
            wfc.watch_run = real
        self.assertEqual(verdict["conclusion"], "timeout")

    def test_run_log_path_is_per_run(self):
        self.assertNotEqual(wfc.run_log_path_for("1"), wfc.run_log_path_for("2"))

    def test_the_interval_is_passed_through_to_gh(self):
        """gh run watch defaults to 3s; a caller throttling a long queue wait
        must not be silently ignored."""
        self._shim('echo "$@"')
        wfc.watch_run("99", "o/r", 30, self.log, interval=45)
        self.assertIn("--interval 45", self.log.read_text(encoding="utf-8"))


class ModeSelectionTests(unittest.TestCase):
    """``--pr`` and ``--run`` are alternatives, and exactly one is required."""

    def _run(self, argv):
        with redirect_stdout(io.StringIO()):
            return wfc.run(argv)

    def test_neither_is_refused(self):
        with self.assertRaises(wfc.WaitForChecksError):
            self._run(["wait_for_checks.py"])

    def test_both_is_refused(self):
        """Otherwise the tool watches one thing and reports about the other."""
        with self.assertRaises(wfc.WaitForChecksError):
            self._run(["wait_for_checks.py", "--pr", "285", "--run", "99"])

    def test_no_watch_is_refused_with_run(self):
        with self.assertRaises(wfc.WaitForChecksError):
            self._run(["wait_for_checks.py", "--run", "99", "--no-watch"])

    def _run_mode(self, conclusion):
        seen = {}
        real = wfc.wait_run

        def stub(rid, repo=None, timeout=None, interval=None):
            seen.update(rid=rid, repo=repo, timeout=timeout, interval=interval)
            return {"conclusion": conclusion, "run_id": rid}

        wfc.wait_run = stub
        try:
            buf = io.StringIO()
            with redirect_stdout(buf):
                code = wfc.run(["wait_for_checks.py", "--run", "99"])
        finally:
            wfc.wait_run = real
        return code, json.loads(buf.getvalue()), seen

    def test_run_mode_reaches_wait_run_with_the_cli_values(self):
        code, parsed, seen = self._run_mode("pass")
        self.assertEqual(code, 0)
        self.assertEqual(parsed["run_id"], "99")
        self.assertEqual(seen["repo"], wfc.DEFAULT_REPO)
        self.assertEqual(seen["timeout"], wfc.DEFAULT_TIMEOUT)
        self.assertEqual(seen["interval"], wfc.DEFAULT_INTERVAL)

    def test_run_mode_exits_non_zero_on_a_dequeue_or_timeout(self):
        """The stated goal of this mode is that a dequeue can never read as a
        merge, so the red exit code is the assertion that matters most."""
        for conclusion in ("fail", "timeout"):
            with self.subTest(conclusion=conclusion):
                code, _, _ = self._run_mode(conclusion)
                self.assertEqual(code, 1)


ENQUEUED = "2026-10-13T12:00:00Z"
ENQUEUED_EPOCH = wfc._parse_time(ENQUEUED)


def snap(
    gql="OPEN",
    merged=False,
    entry=None,
    auto=None,
    view="OPEN",
    merged_at=None,
    runs=(),
):
    return {
        "gql_state": gql,
        "gql_merged": merged,
        "entry": entry,
        "auto": auto,
        "view_state": view,
        "view_merged_at": merged_at,
        "queue_runs": list(runs),
    }


def queue_run(
    conclusion="",
    status="completed",
    created=ENQUEUED,
    rid="7",
    branch="gh-readonly-queue/main/pr-285-new",
):
    return {
        "run_id": rid,
        "head_branch": branch,
        "name": "Tests",
        "status": status,
        "conclusion": conclusion,
        "created_at": created,
        "url": "",
    }


QUEUED = {"state": "QUEUED", "enqueuedAt": ENQUEUED}


class QueueWatchTests(unittest.TestCase):
    """``--queue``: one backgrounded command that exits only on a terminal
    outcome, with a MERGED from either state source authoritative."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        real = wfc.queue_log_path_for
        wfc.queue_log_path_for = lambda pr: Path(self.tmp.name) / f"q-{pr}.log"
        self.addCleanup(setattr, wfc, "queue_log_path_for", real)

    def _watch(self, snaps, interval=60, timeout=7200, since=None):
        """Feed ``snaps`` one per poll (the last repeats); time advances only
        through ``sleep``, so elapsed seconds are exact."""
        now = [0.0]
        calls = [0]

        def probe(pr, repo):
            item = snaps[min(calls[0], len(snaps) - 1)]
            calls[0] += 1
            if isinstance(item, Exception):
                raise item
            return json.loads(json.dumps(item))

        def sleep(seconds):
            now[0] += seconds

        return wfc.wait_queue(
            285,
            interval=interval,
            timeout=timeout,
            probe=probe,
            sleep=sleep,
            clock=lambda: now[0],
            wall=lambda: ENQUEUED_EPOCH + now[0],
            since=since,
        )

    def test_stays_silent_while_queued_then_reports_the_merge(self):
        v = self._watch([snap(entry=QUEUED)] * 5 + [snap(gql="MERGED", merged=True)])
        self.assertEqual(v["conclusion"], "merged")
        self.assertTrue(v["settled"])
        self.assertEqual(v["polls"], 6)
        self.assertTrue(v["seen_queue_entry"])

    def test_a_stale_entry_does_not_outrank_a_merge_on_the_other_source(self):
        """The PR #420 shape: GraphQL still AWAITING_CHECKS, gh pr view MERGED."""
        stale = {"state": "AWAITING_CHECKS", "enqueuedAt": ENQUEUED}
        v = self._watch([snap(entry=stale, view="MERGED", merged_at=ENQUEUED)])
        self.assertEqual(v["conclusion"], "merged")
        self.assertEqual(v["polls"], 1)

    def test_a_graphql_merge_wins_over_a_stale_view(self):
        """The PR #424 shape: the opposite direction of staleness."""
        v = self._watch([snap(gql="MERGED", merged=True, view="OPEN")])
        self.assertEqual(v["conclusion"], "merged")

    def test_a_failed_queue_run_dequeues_after_confirmation(self):
        gone = snap(runs=[queue_run("failure")])
        v = self._watch([snap(entry=QUEUED), gone, gone])
        self.assertEqual(v["conclusion"], "dequeued")
        self.assertEqual(v["polls"], 3)
        self.assertEqual([r["run_id"] for r in v["failing_runs"]], ["7"])

    def test_one_stale_read_cannot_turn_a_merge_into_a_dequeue(self):
        gone = snap(runs=[queue_run("failure")])
        v = self._watch([snap(entry=QUEUED), gone, snap(gql="MERGED", merged=True)])
        self.assertEqual(v["conclusion"], "merged")

    def test_an_unregistered_enqueue_reports_not_queued(self):
        """No entry and no queue run ever: the enqueue did not take."""
        v = self._watch([snap()])
        self.assertEqual(v["conclusion"], "not_queued")
        self.assertFalse(v["seen_queue_entry"])
        self.assertGreaterEqual(v["elapsed_seconds"], wfc.NOT_QUEUED_AFTER_SECONDS)

    def test_the_registration_window_is_not_a_dequeue(self):
        """All-null for the first few seconds, then QUEUED, then merged."""
        v = self._watch([snap(), snap(entry=QUEUED), snap(gql="MERGED", merged=True)])
        self.assertEqual(v["conclusion"], "merged")

    def test_an_unexplained_disappearance_waits_out_the_grace(self):
        """Entry gone, every queue run green: maybe a stale merge, so wait."""
        v = self._watch([snap(entry=QUEUED), snap(runs=[queue_run("success")])])
        self.assertEqual(v["conclusion"], "dequeued")
        self.assertEqual(v["failing_runs"], [])
        self.assertGreaterEqual(v["elapsed_seconds"], wfc.AMBIGUOUS_GRACE_SECONDS)

    def test_an_old_failed_run_from_a_prior_enqueue_is_ignored(self):
        old = queue_run("failure", created="2026-10-13T11:00:00Z", rid="1")
        v = self._watch([snap(entry=QUEUED, runs=[old])] * 3 + [snap(merged=True)])
        self.assertEqual(v["conclusion"], "merged")
        self.assertEqual(v["queue_runs"], [])

    def test_closed_without_merge_is_not_a_merge(self):
        v = self._watch([snap(entry=QUEUED), snap(gql="CLOSED", view="CLOSED")])
        self.assertEqual(v["conclusion"], "closed")

    def test_a_persistent_probe_failure_gives_up_loudly(self):
        v = self._watch([wfc.WaitForChecksError("gh down")])
        self.assertEqual(v["conclusion"], "probe_failed")
        self.assertFalse(v["settled"])
        self.assertEqual(v["polls"], wfc.MAX_PROBE_ERRORS)
        self.assertIn("gh down", v["error"])

    def test_a_transient_probe_failure_is_survived(self):
        v = self._watch(
            [wfc.WaitForChecksError("blip"), snap(entry=QUEUED), snap(merged=True)]
        )
        self.assertEqual(v["conclusion"], "merged")

    def test_a_queue_that_never_resolves_times_out(self):
        v = self._watch([snap(entry=QUEUED)], interval=60, timeout=600)
        self.assertEqual(v["conclusion"], "timeout")
        self.assertFalse(v["settled"])
        self.assertEqual(v["elapsed_seconds"], 600)
        self.assertEqual(v["polls"], 11)

    def test_classic_auto_merge_keeps_the_watch_waiting(self):
        """No queue on the repo: autoMergeRequest alone is the queued signal."""
        auto = {"enabledAt": ENQUEUED}
        v = self._watch([snap(auto=auto)] * 8 + [snap(merged=True)])
        self.assertEqual(v["conclusion"], "merged")
        self.assertTrue(v["seen_queue_entry"])

    def test_auto_merge_never_outranks_a_failed_queue_run(self):
        auto = {"enabledAt": ENQUEUED}
        gone = snap(auto=auto, runs=[queue_run("failure")])
        v = self._watch([snap(entry=QUEUED), gone, gone])
        self.assertEqual(v["conclusion"], "dequeued")

    def test_a_failure_on_an_older_queue_sha_is_not_a_dequeue(self):
        """The queue rebuilt the group: the old SHA's cancelled run is noise."""
        old = queue_run(
            "cancelled", rid="1", branch="gh-readonly-queue/main/pr-285-old"
        )
        new = queue_run(
            "success",
            rid="2",
            created="2026-10-13T12:05:00Z",
            branch="gh-readonly-queue/main/pr-285-new",
        )
        stale = snap(runs=[old, new])
        v = self._watch([snap(entry=QUEUED), stale, stale, snap(merged=True)])
        self.assertEqual(v["conclusion"], "merged")

    def test_a_probe_error_breaks_the_dequeue_confirmation(self):
        gone = snap(runs=[queue_run("failure")])
        blip = wfc.WaitForChecksError("blip")
        # Without the reset, the second `gone` would be streak 2 → dequeued.
        v = self._watch([snap(entry=QUEUED), gone, blip, gone, snap(merged=True)])
        self.assertEqual(v["conclusion"], "merged")

    def test_since_anchors_a_fresh_call_to_an_old_dequeue(self):
        """A cron firing: no entry seen this call, the failed run 1000 s old."""
        old = "2026-10-13T11:43:20Z"  # ENQUEUED - 1000 s
        gone = snap(runs=[queue_run("failure", created=old)])
        bare = self._watch([gone] * 6)
        self.assertEqual(bare["conclusion"], "not_queued")
        anchored = self._watch([gone] * 6, since="2026-10-13T11:40:00Z")
        self.assertEqual(anchored["conclusion"], "dequeued")

    def test_a_failed_final_poll_keeps_the_last_good_snapshot(self):
        blip = wfc.WaitForChecksError("blip")
        v = self._watch([snap(entry=QUEUED), blip], interval=60, timeout=60)
        self.assertEqual(v["conclusion"], "timeout")
        self.assertEqual(v["last"]["entry_state"], "QUEUED")

    def test_a_timeout_reached_through_probe_errors_says_so(self):
        blip = wfc.WaitForChecksError("blip")
        v = self._watch([blip, snap(entry=QUEUED)] * 10, interval=60, timeout=120)
        self.assertEqual(v["conclusion"], "timeout")
        self.assertIn("blip", v["error"])

    def test_the_run_window_floor_applies_without_an_anchor(self):
        """No entry seen yet: a failure older than the window is ignored."""
        ancient = queue_run("failure", created="2026-10-13T11:00:00Z")
        v = self._watch(
            [snap(runs=[ancient])] * 2 + [snap(entry=QUEUED), snap(merged=True)]
        )
        self.assertEqual(v["conclusion"], "merged")

    def test_the_log_is_one_line_per_poll(self):
        v = self._watch([snap(entry=QUEUED)] * 3 + [snap(merged=True)])
        lines = Path(v["log_path"]).read_text().splitlines()
        self.assertEqual(len(lines), 4)
        self.assertEqual(json.loads(lines[0])["entry"], "QUEUED")


class QueueProbeTests(unittest.TestCase):
    """``probe_queue`` keeps only this PR's queue-branch runs."""

    def test_filters_runs_to_this_prs_queue_branch(self):
        replies = [
            json.dumps(
                {
                    "data": {
                        "repository": {
                            "pullRequest": {
                                "state": "OPEN",
                                "merged": False,
                                "mergeQueueEntry": QUEUED,
                                "autoMergeRequest": None,
                            }
                        }
                    }
                }
            ),
            json.dumps({"state": "OPEN", "mergedAt": None}),
            json.dumps(
                [
                    {
                        "databaseId": 1,
                        "headBranch": "gh-readonly-queue/main/pr-285-abc",
                    },
                    {
                        "databaseId": 2,
                        "headBranch": "gh-readonly-queue/main/pr-2850-abc",
                    },
                    {"databaseId": 3, "headBranch": "eng-285"},
                ]
            ),
        ]
        seen = []
        real = wfc._gh

        def fake(args):
            seen.append(args)
            return 0, replies[len(seen) - 1], ""

        wfc._gh = fake
        try:
            got = wfc.probe_queue(285, wfc.DEFAULT_REPO)
        finally:
            wfc._gh = real
        self.assertEqual([r["run_id"] for r in got["queue_runs"]], ["1"])
        self.assertEqual(got["entry"]["state"], "QUEUED")
        self.assertIn("state,mergedAt", seen[1])
        self.assertIn("merge_group", seen[2])

    def test_a_null_pull_request_is_a_probe_error_not_a_crash(self):
        reply = json.dumps({"data": {"repository": {"pullRequest": None}}})
        real = wfc._gh
        wfc._gh = lambda args: (0, reply, "")
        try:
            with self.assertRaises(wfc.WaitForChecksError):
                wfc.probe_queue(285, wfc.DEFAULT_REPO)
        finally:
            wfc._gh = real

    def test_a_failed_read_raises(self):
        real = wfc._gh
        wfc._gh = lambda args: (1, "", "HTTP 502")
        try:
            with self.assertRaises(wfc.WaitForChecksError):
                wfc.probe_queue(285, wfc.DEFAULT_REPO)
        finally:
            wfc._gh = real


class QueueModeCliTests(unittest.TestCase):
    def _queue_mode(self, conclusion, extra=(), runs=(), mergeable="MERGEABLE"):
        seen = {}
        real = wfc.wait_queue

        def stub(pr, repo=None, interval=None, timeout=None, since=None):
            seen.update(
                pr=pr, repo=repo, interval=interval, timeout=timeout, since=since
            )
            return {"conclusion": conclusion, "pr": pr, "queue_runs": list(runs)}

        def settle(pr, repo, retries, delay):
            seen.setdefault("settled", []).append(pr)
            status = "DIRTY" if mergeable == "CONFLICTING" else "BLOCKED"
            return {
                "mergeable": mergeable,
                "merge_state_status": status,
                "error": None,
            }, 1

        real_settle = wfc.settle_mergeability
        wfc.settle_mergeability = settle
        self.addCleanup(setattr, wfc, "settle_mergeability", real_settle)

        real_dismiss = wfc.dismiss_pr_notifications
        out = io.StringIO()

        def dismiss(pr, repo):
            seen.setdefault("dismissed", []).append((pr, repo))
            return {"dismissed": 1, "failed": 0, "error": None}

        real_cause = wfc.dequeue_cause

        def cause(pr, repo, enqueued_at=None, conflicting=False):
            seen.setdefault("caused", []).append((pr, repo))
            seen["cause_conflicting"] = conflicting
            return {"dequeue_reason": "merge_conflict", "error": None}

        wfc.wait_queue = stub
        wfc.dismiss_pr_notifications = dismiss
        wfc.dequeue_cause = cause
        try:
            with redirect_stdout(out):
                code = wfc.run(["wait_for_checks.py", "--queue", "285", *extra])
        finally:
            wfc.wait_queue = real
            wfc.dismiss_pr_notifications = real_dismiss
            wfc.dequeue_cause = real_cause
        seen["verdict"] = json.loads(out.getvalue())
        return code, seen

    def test_a_merge_dismisses_the_prs_notifications(self):
        _, seen = self._queue_mode("merged")
        self.assertEqual(seen["dismissed"], [(285, wfc.DEFAULT_REPO)])
        self.assertEqual(seen["verdict"]["notifications"]["dismissed"], 1)

    def test_no_other_conclusion_dismisses(self):
        for conclusion in ("dequeued", "closed", "not_queued", "timeout"):
            with self.subTest(conclusion=conclusion):
                _, seen = self._queue_mode(conclusion)
                self.assertNotIn("dismissed", seen)
                self.assertNotIn("notifications", seen["verdict"])

    def test_only_a_dequeue_reads_its_cause(self):
        _, seen = self._queue_mode("dequeued")
        self.assertEqual(seen["caused"], [(285, wfc.DEFAULT_REPO)])
        self.assertEqual(seen["verdict"]["dequeue"]["dequeue_reason"], "merge_conflict")
        for conclusion in ("merged", "closed", "not_queued", "timeout"):
            with self.subTest(conclusion=conclusion):
                _, seen = self._queue_mode(conclusion)
                self.assertNotIn("caused", seen)
                self.assertNotIn("dequeue", seen["verdict"])

    def test_a_no_run_dequeue_on_a_conflict_concludes_conflicting(self):
        code, seen = self._queue_mode("dequeued", mergeable="CONFLICTING")
        self.assertEqual(code, 1)
        self.assertEqual(seen["settled"], [285])
        self.assertEqual(seen["verdict"]["conclusion"], "conflicting")
        self.assertEqual(seen["verdict"]["merge_state_status"], "DIRTY")
        # The compare is forced, so base_gained names what landed first.
        self.assertTrue(seen["cause_conflicting"])
        self.assertIn("dequeue", seen["verdict"])

    def test_a_no_run_dequeue_that_still_merges_stays_dequeued(self):
        _, seen = self._queue_mode("dequeued")
        self.assertEqual(seen["verdict"]["conclusion"], "dequeued")
        self.assertEqual(seen["verdict"]["mergeable"], "MERGEABLE")
        self.assertIsNone(seen["verdict"]["mergeable_error"])
        self.assertFalse(seen["cause_conflicting"])

    def test_a_dequeue_after_queue_runs_skips_the_mergeability_read(self):
        runs = [{"run_id": "1", "conclusion": "failure"}]
        _, seen = self._queue_mode("dequeued", runs=runs, mergeable="CONFLICTING")
        self.assertNotIn("settled", seen)
        self.assertEqual(seen["verdict"]["conclusion"], "dequeued")
        self.assertNotIn("mergeable", seen["verdict"])

    def test_queue_mode_uses_its_own_pacing_defaults(self):
        code, seen = self._queue_mode("merged")
        self.assertEqual(code, 0)
        self.assertEqual(seen["pr"], 285)
        self.assertEqual(seen["interval"], wfc.QUEUE_INTERVAL)
        self.assertEqual(seen["timeout"], wfc.QUEUE_TIMEOUT)

    def test_since_is_passed_through_and_validated(self):
        _, seen = self._queue_mode("merged", ["--since", ENQUEUED])
        self.assertEqual(seen["since"], ENQUEUED)
        for argv in (
            ["wait_for_checks.py", "--queue", "285", "--since", "yesterday"],
            ["wait_for_checks.py", "--pr", "285", "--since", ENQUEUED],
        ):
            with self.subTest(argv=argv):
                with self.assertRaises(wfc.WaitForChecksError):
                    with redirect_stdout(io.StringIO()):
                        wfc.run(argv)

    def test_explicit_pacing_is_honored(self):
        _, seen = self._queue_mode("merged", ["--interval", "30", "--timeout", "900"])
        self.assertEqual((seen["interval"], seen["timeout"]), (30, 900))

    def test_only_a_merge_exits_zero(self):
        for conclusion in (
            "dequeued",
            "closed",
            "not_queued",
            "timeout",
            "probe_failed",
        ):
            with self.subTest(conclusion=conclusion):
                code, _ = self._queue_mode(conclusion)
                self.assertEqual(code, 1)

    def test_queue_combined_with_another_mode_is_refused(self):
        with self.assertRaises(wfc.WaitForChecksError):
            with redirect_stdout(io.StringIO()):
                wfc.run(["wait_for_checks.py", "--queue", "285", "--pr", "285"])

    def test_no_watch_is_refused_with_queue(self):
        with self.assertRaises(wfc.WaitForChecksError):
            with redirect_stdout(io.StringIO()):
                wfc.run(["wait_for_checks.py", "--queue", "285", "--no-watch"])


REPO = wfc.DEFAULT_REPO


class DismissNotificationsTests(unittest.TestCase):
    THREADS = [
        {
            "id": "11",
            "subject": {"url": f"https://api.github.com/repos/{REPO}/pulls/285"},
        },
        {
            "id": "12",
            "subject": {"url": f"https://api.github.com/repos/{REPO}/pulls/2850"},
        },
        {"id": "13", "subject": {"url": "https://api.github.com/repos/o/r/pulls/285"}},
        {
            "id": "14",
            "subject": {"url": f"https://api.github.com/repos/{REPO}/pulls/285"},
        },
        {"id": "15", "subject": {"url": None}},
    ]

    def _run(self, list_reply, delete_code=0):
        calls = []

        def fake(args):
            calls.append(args)
            if args[:2] == ["api", "-X"]:
                return delete_code, "", "HTTP 404" if delete_code else ""
            return list_reply

        real = wfc._gh
        wfc._gh = fake
        self.addCleanup(setattr, wfc, "_gh", real)
        return wfc.dismiss_pr_notifications(285, REPO), calls

    def test_only_this_prs_threads_are_marked_done(self):
        result, calls = self._run((0, json.dumps(self.THREADS), ""))
        deleted = [c[-1] for c in calls if c[:2] == ["api", "-X"]]
        self.assertEqual(
            deleted, ["/notifications/threads/11", "/notifications/threads/14"]
        )
        self.assertEqual(result, {"dismissed": 2, "failed": 0, "error": None})

    def test_a_failed_list_is_reported_not_raised(self):
        result, calls = self._run((1, "", "HTTP 401"))
        self.assertEqual(len(calls), 1)
        self.assertEqual(result["dismissed"], 0)
        self.assertIn("HTTP 401", result["error"])

    def test_a_failed_delete_is_counted(self):
        result, _ = self._run((0, json.dumps(self.THREADS), ""), delete_code=1)
        self.assertEqual(result, {"dismissed": 0, "failed": 2, "error": "HTTP 404"})


class DequeueCauseTests(unittest.TestCase):
    HEAD = "a" * 40

    def _gql(self, reason):
        nodes = [] if reason is None else [{"createdAt": ENQUEUED, "reason": reason}]
        pr = {
            "baseRefName": "main",
            "headRefOid": self.HEAD,
            "timelineItems": {"nodes": nodes},
        }
        return json.dumps({"data": {"repository": {"pullRequest": pr}}})

    def _run(
        self,
        reason,
        compare=None,
        compare_code=0,
        gql_code=0,
        enqueued_at=None,
        conflicting=False,
    ):
        calls = []

        def fake(args):
            calls.append(args)
            if args[1] == "graphql":
                return gql_code, self._gql(reason), "HTTP 502" if gql_code else ""
            return compare_code, json.dumps(compare), "HTTP 404" if compare_code else ""

        real = wfc._gh
        wfc._gh = fake
        self.addCleanup(setattr, wfc, "_gh", real)
        return wfc.dequeue_cause(285, REPO, enqueued_at, conflicting), calls

    def test_a_conflict_lists_what_the_base_gained(self):
        commits = [
            {"sha": f"{n:040d}", "commit": {"message": f"feat: landed {n}\n\nbody"}}
            for n in range(wfc.BASE_GAINED_LIMIT + 2)
        ]
        result, calls = self._run(
            "merge_conflict", {"ahead_by": len(commits), "commits": commits}
        )
        self.assertEqual(
            calls[-1], ["api", f"/repos/{REPO}/compare/{self.HEAD}...main"]
        )
        self.assertEqual(result["dequeue_reason"], "merge_conflict")
        self.assertEqual(result["dequeued_at"], ENQUEUED)
        self.assertEqual(result["base_gained_total"], wfc.BASE_GAINED_LIMIT + 2)
        gained = result["base_gained"]
        self.assertEqual(len(gained), wfc.BASE_GAINED_LIMIT)
        # The newest are kept, oldest first, subject line only.
        self.assertEqual(gained[-1]["subject"], f"feat: landed {len(commits) - 1}")
        self.assertEqual(gained[0]["subject"], "feat: landed 2")
        self.assertEqual(len(gained[0]["sha"]), 12)
        self.assertIsNone(result["error"])

    def test_another_reason_skips_the_compare(self):
        result, calls = self._run("checks_failed")
        self.assertEqual(len(calls), 1)
        self.assertEqual(result["dequeue_reason"], "checks_failed")
        self.assertIsNone(result["base_gained"])

    def test_failures_are_reported_not_raised(self):
        result, _ = self._run(None)
        self.assertIn("no RemovedFromMergeQueueEvent", result["error"])
        result, _ = self._run("merge_conflict", compare_code=1)
        self.assertEqual(result["dequeue_reason"], "merge_conflict")
        self.assertIsNone(result["base_gained"])
        self.assertIn("HTTP 404", result["error"])
        result, _ = self._run("merge_conflict", compare={"message": "Not Found"})
        self.assertIn("base compare", result["error"])
        result, calls = self._run("merge_conflict", gql_code=1)
        self.assertEqual(len(calls), 1)
        self.assertIsNone(result["dequeue_reason"])
        self.assertIn("HTTP 502", result["error"])
        commits = [
            {"sha": "b" * 40, "commit": None},
            {"sha": "c" * 40, "commit": {"message": 7}},
        ]
        result, _ = self._run("merge_conflict", {"ahead_by": 2, "commits": commits})
        self.assertEqual([c["subject"] for c in result["base_gained"]], ["", ""])

    def test_a_removal_older_than_this_enqueue_is_withheld(self):
        """An earlier cycle's event must not read as this dequeue's cause."""
        later = "2026-10-13T12:05:00Z"
        result, calls = self._run("merge_conflict", enqueued_at=later)
        self.assertEqual(len(calls), 1)
        self.assertIsNone(result["dequeue_reason"])
        self.assertIsNone(result["base_gained"])
        self.assertIn("predates this enqueue", result["error"])
        result, _ = self._run("checks_failed", enqueued_at=ENQUEUED)
        self.assertEqual(result["dequeue_reason"], "checks_failed")

    def test_a_conflicting_read_lists_the_base_gain_without_the_event(self):
        """The timeline can lag the dequeue; mergeability already proved it."""
        commits = [{"sha": "d" * 40, "commit": {"message": "feat: sibling"}}]
        compare = {"ahead_by": 1, "commits": commits}
        for reason in (None, "checks_failed"):
            with self.subTest(reason=reason):
                result, _ = self._run(reason, compare, conflicting=True)
                self.assertEqual(result["base_gained"][0]["subject"], "feat: sibling")
        result, _ = self._run(
            None, compare_code=1, conflicting=True, enqueued_at=ENQUEUED
        )
        self.assertIn("no RemovedFromMergeQueueEvent", result["error"])
        self.assertIn("HTTP 404", result["error"])

    def test_the_watch_reports_the_enqueue_it_anchored_on(self):
        snap = {
            "gql_state": "OPEN",
            "entry": {"state": "QUEUED", "enqueuedAt": ENQUEUED},
            "view_state": "OPEN",
            "queue_runs": [],
        }
        verdict = wfc.wait_queue(
            285,
            interval=1,
            timeout=0,
            probe=lambda pr, repo: snap,
            sleep=lambda s: None,
        )
        self.assertEqual(verdict["enqueued_at"], ENQUEUED)


class ConflictingPrTests(unittest.TestCase):
    """`none` is ambiguous, and one of its two meanings is blocking.

    A CONFLICTING PR cannot produce ANY pull_request workflow run — the merge
    ref cannot be created — so there is no run, no error, and nothing to tell it
    apart from CI that has not started. One session spent ~20 minutes and ~16
    polls on that, including an amend-and-force-push nudge at an event that
    could never fire.
    """

    def _stub(self, checks, merge_payload=None, code=0):
        real_watch, real_read, real_gh = wfc.watch_checks, wfc.read_checks, wfc._gh
        real_sleep = wfc.time.sleep
        wfc.watch_checks = lambda pr, repo, interval, timeout, log: True
        wfc.read_checks = lambda pr, repo: checks
        wfc._gh = lambda args: (code, json.dumps(merge_payload or {}), "boom")
        # Every case in this class reads `none`, which now re-enters the watch.
        wfc.time.sleep = lambda _s: None
        self.addCleanup(setattr, wfc, "watch_checks", real_watch)
        self.addCleanup(setattr, wfc, "read_checks", real_read)
        self.addCleanup(setattr, wfc, "_gh", real_gh)
        self.addCleanup(setattr, wfc.time, "sleep", real_sleep)

    def test_no_checks_plus_conflicting_reports_conflicting(self):
        self._stub([], {"mergeable": "CONFLICTING", "mergeStateStatus": "DIRTY"})
        verdict = wfc.wait(285, repo="o/r")
        self.assertEqual(verdict["conclusion"], "conflicting")
        self.assertTrue(verdict["blocked_by_conflict"])
        self.assertEqual(verdict["merge_state_status"], "DIRTY")

    def test_no_checks_and_mergeable_stays_none(self):
        # The other meaning: genuinely no checks apply (a docs-only path filter).
        self._stub([], {"mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN"})
        verdict = wfc.wait(285, repo="o/r")
        self.assertEqual(verdict["conclusion"], "none")
        self.assertFalse(verdict["blocked_by_conflict"])

    def test_a_persistent_none_is_re_read_before_being_believed(self):
        """`none` fails OPEN — it declares a commit CI-clean — where `pending`
        fails safe. A wait started right after a push returned in 1 second with
        `none`, `counts: {}`, `settled: true` while four runs were already
        starting. So it is re-read rather than believed."""
        self._stub([], {"mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN"})
        verdict = wfc.wait(285, repo="o/r")
        self.assertEqual(verdict["watch_rounds"], wfc.MAX_NONE_ROUNDS)

    def test_an_exhausted_none_reports_none_not_timeout(self):
        """The rounds established an ABSENCE, which is an answer. Reporting
        `timeout` would claim a wedged run that never existed."""
        self._stub([], {"mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN"})
        verdict = wfc.wait(285, repo="o/r")
        self.assertEqual(verdict["conclusion"], "none")
        # Honest about not having settled, while still naming the cause.
        self.assertFalse(verdict["settled"])

    def test_a_check_that_registers_late_is_caught_rather_than_missed(self):
        """The whole point: the second read sees what the first could not."""
        reads = [[], [check("Tests", "pass")]]
        real_watch, real_read = wfc.watch_checks, wfc.read_checks
        real_sleep = wfc.time.sleep
        wfc.watch_checks = lambda pr, repo, interval, timeout, log: True
        wfc.read_checks = lambda pr, repo: reads.pop(0) if len(reads) > 1 else reads[0]
        wfc.time.sleep = lambda _s: None
        self.addCleanup(setattr, wfc, "watch_checks", real_watch)
        self.addCleanup(setattr, wfc, "read_checks", real_read)
        self.addCleanup(setattr, wfc.time, "sleep", real_sleep)

        verdict = wfc.wait(285, repo="o/r")
        self.assertEqual(verdict["conclusion"], "pass")
        self.assertTrue(verdict["settled"])
        self.assertEqual(verdict["watch_rounds"], 2)

    def test_a_none_that_is_really_a_conflict_still_reports_conflicting(self):
        """The conflict probe must survive the re-read, since a CONFLICTING PR
        produces no run at all and would otherwise look like the race."""
        self._stub([], {"mergeable": "CONFLICTING", "mergeStateStatus": "DIRTY"})
        verdict = wfc.wait(285, repo="o/r")
        self.assertEqual(verdict["conclusion"], "conflicting")

    def test_the_none_cap_is_smaller_than_the_pending_cap(self):
        """A registration race is seconds-scale; a CI run is not. Reusing the
        pending pacing would add minutes to every check-less PR."""
        self.assertLess(wfc.MAX_NONE_ROUNDS, wfc.MAX_WATCH_ROUNDS)
        self.assertLess(wfc.NONE_SETTLE_SECONDS, wfc.DEFAULT_INTERVAL)

    def test_the_probe_is_skipped_when_checks_exist(self):
        # It answers a question only `none` raises; asking always would be a
        # gh call per wait for nothing.
        called = []
        real_gh = wfc._gh
        wfc._gh = lambda args: called.append(args) or (0, "{}", "")
        self.addCleanup(setattr, wfc, "_gh", real_gh)
        real_watch, real_read = wfc.watch_checks, wfc.read_checks
        wfc.watch_checks = lambda pr, repo, interval, timeout, log: True
        wfc.read_checks = lambda pr, repo: [check("a", "pass")]
        self.addCleanup(setattr, wfc, "watch_checks", real_watch)
        self.addCleanup(setattr, wfc, "read_checks", real_read)
        verdict = wfc.wait(285, repo="o/r")
        self.assertEqual(verdict["conclusion"], "pass")
        self.assertEqual(called, [])

    def test_a_conflict_is_not_downgraded_to_timeout(self):
        # `conflicting` is definite and strictly more informative, exactly as
        # `fail` is — a caller branching on conclusion must be able to act.
        real_watch, real_read, real_gh = wfc.watch_checks, wfc.read_checks, wfc._gh
        wfc.watch_checks = lambda pr, repo, interval, timeout, log: False
        wfc.read_checks = lambda pr, repo: []
        wfc._gh = lambda args: (
            0,
            json.dumps({"mergeable": "CONFLICTING", "mergeStateStatus": "DIRTY"}),
            "",
        )
        self.addCleanup(setattr, wfc, "watch_checks", real_watch)
        self.addCleanup(setattr, wfc, "read_checks", real_read)
        self.addCleanup(setattr, wfc, "_gh", real_gh)
        verdict = wfc.wait(285, repo="o/r", timeout=0)
        self.assertEqual(verdict["conclusion"], "conflicting")

    def test_a_failed_probe_leaves_the_verdict_as_none(self):
        # The probe is a diagnosis aid; a gh failure must not invent a state.
        self._stub([], code=1)
        verdict = wfc.wait(285, repo="o/r")
        self.assertEqual(verdict["conclusion"], "none")
        self.assertFalse(verdict["blocked_by_conflict"])

    def test_a_failed_probe_SAYS_it_failed(self):
        # Without this field, "the probe failed" and "not conflicting" both
        # read as mergeable: null — so the promise that `none` is
        # disambiguated for you would be false with no signal.
        self._stub([], code=1)
        verdict = wfc.wait(285, repo="o/r")
        self.assertIsNotNone(verdict["mergeable_error"])

    def test_an_UNKNOWN_mergeable_is_re_probed_rather_than_concluded_from(self):
        # GitHub computes mergeability asynchronously, and right after a push —
        # this probe's own window — UNKNOWN is common. Reading it as
        # "not conflicting" reproduces the dead wait the probe exists to stop.
        payloads = [
            json.dumps({"mergeable": "UNKNOWN", "mergeStateStatus": "UNKNOWN"}),
            json.dumps({"mergeable": "CONFLICTING", "mergeStateStatus": "DIRTY"}),
        ]
        calls = []

        def fake_gh(args):
            calls.append(args)
            return (0, payloads[min(len(calls) - 1, len(payloads) - 1)], "")

        real_watch, real_read, real_gh = wfc.watch_checks, wfc.read_checks, wfc._gh
        real_sleep = wfc.time.sleep
        wfc.watch_checks = lambda pr, repo, interval, timeout, log: True
        wfc.read_checks = lambda pr, repo: []
        wfc._gh = fake_gh
        wfc.time.sleep = lambda _s: None
        self.addCleanup(setattr, wfc, "watch_checks", real_watch)
        self.addCleanup(setattr, wfc, "read_checks", real_read)
        self.addCleanup(setattr, wfc, "_gh", real_gh)
        self.addCleanup(setattr, wfc.time, "sleep", real_sleep)

        verdict = wfc.wait(285, repo="o/r")
        self.assertGreater(len(calls), 1)
        self.assertEqual(verdict["conclusion"], "conflicting")

    def test_a_persistently_UNKNOWN_mergeable_does_not_claim_conflicting(self):
        self._stub([], {"mergeable": "UNKNOWN", "mergeStateStatus": "UNKNOWN"})
        real_sleep = wfc.time.sleep
        wfc.time.sleep = lambda _s: None
        self.addCleanup(setattr, wfc.time, "sleep", real_sleep)
        verdict = wfc.wait(285, repo="o/r")
        self.assertEqual(verdict["conclusion"], "none")
        self.assertFalse(verdict["blocked_by_conflict"])

    def test_the_probe_is_field_selected(self):
        seen = {}
        real_watch, real_read, real_gh = wfc.watch_checks, wfc.read_checks, wfc._gh
        wfc.watch_checks = lambda pr, repo, interval, timeout, log: True
        wfc.read_checks = lambda pr, repo: []
        wfc._gh = lambda args: (seen.update(args=args), (0, "{}", ""))[1]
        self.addCleanup(setattr, wfc, "watch_checks", real_watch)
        self.addCleanup(setattr, wfc, "read_checks", real_read)
        self.addCleanup(setattr, wfc, "_gh", real_gh)
        wfc.wait(285, repo="o/r")
        joined = " ".join(seen["args"])
        self.assertIn("mergeable,mergeStateStatus", joined)
        # The collection-valued fields would reintroduce the payload this tool
        # exists to avoid.
        self.assertNotIn("files", joined)
        self.assertNotIn("commits", joined)


def job(name, steps, conclusion="failure", job_id=8):
    return {
        "id": job_id,
        "name": name,
        "conclusion": conclusion,
        "steps": [{"name": n, "conclusion": c} for n, c in steps],
    }


class DiagnoseFailuresTests(unittest.TestCase):
    """The failed-step read the session used to do by hand with two compound
    ``gh api --jq`` calls, with fixtures shaped on the ledger's incidents."""

    def _stub(self, responses):
        self.calls = []

        def fake(args):
            self.calls.append(args)
            return responses.pop(0)

        real = wfc._gh
        wfc._gh = fake
        self.addCleanup(setattr, wfc, "_gh", real)

    def _failing(self, link):
        return [{"name": "Lint", "link": link, "run_id": wfc.run_id_from_link(link)}]

    def test_the_lint_jobs_pre_commit_step_routes_to_a_local_reproduce(self):
        """PR #235: the batch sits above any tail, so a larger tail is wrong."""
        steps = [
            ("Run make tools-tests", "success"),
            ("Run pre-commit run --all-files --color=always", "failure"),
        ]
        self._stub([(0, json.dumps(job("Lint", steps)), "")])
        [rec] = wfc.diagnose_failures(
            self._failing("https://github.com/o/r/actions/runs/7/job/8"), "o/r"
        )
        self.assertEqual(rec["kind"], "lint_hook")
        self.assertIn("lint_paths.py", rec["next_command"])
        self.assertIn("toolchain skew", rec["note"])
        # The job id from the link is read directly — one call, that job only.
        self.assertEqual(self.calls[0], ["api", "repos/o/r/actions/jobs/8"])

    def test_a_test_runner_step_routes_to_the_quiet_log_fetch(self):
        """ "Tail landed in the epilogue": the summary prints after the failure."""
        steps = [("Run make tools-tests", "failure")]
        self._stub([(0, json.dumps(job("Lint", steps)), "")])
        [rec] = wfc.diagnose_failures(
            self._failing("https://github.com/o/r/actions/runs/7/job/8"), "o/r"
        )
        self.assertEqual(rec["kind"], "test_runner")
        self.assertEqual(
            rec["next_command"],
            "python3 .claude/tools/run_quiet.py -- "
            "gh run view --job 8 --repo o/r --log-failed",
        )

    def test_other_steps_are_single_commands(self):
        self.assertEqual(wfc.classify_step("Run make decks-build"), "single_command")
        self.assertEqual(wfc.classify_step("Run cargo nextest run"), "test_runner")
        self.assertEqual(wfc.classify_step("Run make test-no-teardown"), "test_runner")

    def test_empty_stdout_is_a_fetch_failure_not_nothing_found(self):
        """ "Empty-stdout scan": a read that returned nothing proves nothing."""
        self._stub([(0, "", "")])
        [rec] = wfc.diagnose_failures(
            self._failing("https://github.com/o/r/actions/runs/7/job/8"), "o/r"
        )
        self.assertEqual(rec["kind"], "fetch_failed")
        self.assertEqual(rec["error"], "empty stdout")
        self.assertIn("--log-failed", rec["next_command"])

    def test_a_non_zero_fetch_is_a_fetch_failure(self):
        self._stub([(1, "", "HTTP 404")])
        [rec] = wfc.diagnose_failures(
            self._failing("https://github.com/o/r/actions/runs/7/job/8"), "o/r"
        )
        self.assertEqual((rec["kind"], rec["error"]), ("fetch_failed", "HTTP 404"))

    def test_a_job_with_no_failed_step_says_it_never_ran(self):
        steps = [("Set up job", "success")]
        self._stub([(0, json.dumps(job("Tests", steps, "cancelled")), "")])
        [rec] = wfc.diagnose_failures(
            self._failing("https://github.com/o/r/actions/runs/7/job/8"), "o/r"
        )
        self.assertEqual(rec["kind"], "never_ran")
        self.assertIsNone(rec["step"])
        # No step failed, so `--log-failed` would print nothing.
        self.assertTrue(rec["next_command"].endswith("--log"))

    def test_a_non_numeric_job_id_is_a_fetch_failure(self):
        bad = job("Tests", [("Run cargo test", "failure")], job_id="1; rm")
        self._stub([(0, json.dumps(bad), "")])
        [rec] = wfc.diagnose_failures(
            self._failing("https://github.com/o/r/actions/runs/7/job/8"), "o/r"
        )
        self.assertEqual(rec["kind"], "fetch_failed")

    def test_unexpected_payload_shapes_are_fetch_failures(self):
        for link, body in (
            ("https://github.com/o/r/actions/runs/7/job/8", "[]"),
            ("https://github.com/o/r/actions/runs/7", "[]"),
            ("https://github.com/o/r/actions/runs/7", "{not json"),
            ("https://github.com/o/r/actions/runs/7", json.dumps({"jobs": []})),
        ):
            self._stub([(0, body, "")])
            [rec] = wfc.diagnose_failures(self._failing(link), "o/r")
            self.assertEqual(rec["kind"], "fetch_failed", body)

    def test_a_run_only_link_reads_the_runs_failed_jobs(self):
        jobs = {
            "jobs": [
                job("ok", [("Run x", "success")], "success", 1),
                job("bad", [("Run cargo test", "failure")], "failure", 2),
            ]
        }
        self._stub([(0, json.dumps(jobs), "")])
        [rec] = wfc.diagnose_failures(
            self._failing("https://github.com/o/r/actions/runs/7"), "o/r"
        )
        self.assertEqual((rec["job"], rec["kind"]), ("bad", "test_runner"))
        self.assertIn("runs/7/jobs", self.calls[0][1])

    def test_a_non_actions_check_is_external(self):
        self._stub([])
        [rec] = wfc.diagnose_failures(
            [
                {
                    "name": "deploy preview",
                    "link": "https://deploy.example/x",
                    "run_id": "",
                }
            ],
            "o/r",
        )
        self.assertEqual(rec["kind"], "external")
        # A third-party link is reported, never handed over as a command.
        self.assertIsNone(rec["next_command"])
        self.assertEqual(rec["link"], "https://deploy.example/x")

    def test_a_foreign_host_link_with_a_job_path_is_external(self):
        """A third-party URL must not select a job in this repo to read."""
        self._stub([])
        [rec] = wfc.diagnose_failures(
            self._failing("https://evil.example/runs/7/job/8"), "o/r"
        )
        self.assertEqual(rec["kind"], "external")
        self.assertEqual(self.calls, [])

    def test_wait_attaches_the_diagnosis_only_on_fail(self):
        real_watch, real_read = wfc.watch_checks, wfc.read_checks
        wfc.watch_checks = lambda pr, repo, interval, timeout, log: True
        self.addCleanup(setattr, wfc, "watch_checks", real_watch)
        self.addCleanup(setattr, wfc, "read_checks", real_read)
        self._stub([(0, json.dumps(job("Lint", [("Run cargo test", "failure")])), "")])
        wfc.read_checks = lambda pr, repo: [
            check("Lint", "fail", "Lint", "https://github.com/o/r/actions/runs/7/job/8")
        ]
        v = wfc.wait(285, repo="o/r")
        self.assertEqual(v["failed_steps"][0]["kind"], "test_runner")
        self.assertEqual(v["hint"]["gate"], "blocking")
        wfc.read_checks = lambda pr, repo: [check("Lint", "pass")]
        self.assertEqual(wfc.wait(285, repo="o/r")["failed_steps"], [])


class ActionHintTests(unittest.TestCase):
    """Only `green` is a gate pass; an unverifiable state never is."""

    def _gate(self, conclusion, **extra):
        return wfc.action_hint({"conclusion": conclusion, **extra})["gate"]

    def test_the_gate_per_conclusion(self):
        self.assertEqual(self._gate("pass"), "green")
        self.assertEqual(self._gate("none", mergeable="MERGEABLE"), "green")
        self.assertEqual(self._gate("conflicting"), "blocking")
        self.assertEqual(self._gate("fail", failing=[{"name": "a"}]), "blocking")
        self.assertEqual(self._gate("pending"), "retry")
        self.assertEqual(self._gate("timeout"), "unverified")
        self.assertEqual(self._gate("blocked"), "unverified")

    def test_a_timeout_names_what_is_still_pending(self):
        hint = wfc.action_hint({"conclusion": "timeout", "pending_checks": ["Tests"]})
        self.assertIn("Tests", hint["evidence"])

    def test_a_none_names_a_failed_conflict_probe(self):
        hint = wfc.action_hint(
            {"conclusion": "none", "watch_rounds": 3, "mergeable_error": "boom"}
        )
        self.assertIn("boom", hint["evidence"])

    def test_a_none_whose_conflict_probe_did_not_settle_is_unverified(self):
        """A conflicting PR produces no run at all, so `none` is a pass only
        once the probe has ruled a conflict out."""
        self.assertEqual(self._gate("none", mergeable_error="boom"), "unverified")
        self.assertEqual(self._gate("none", mergeable="UNKNOWN"), "unverified")
        self.assertEqual(self._gate("none", mergeable=None), "unverified")


class MergeableModeTests(unittest.TestCase):
    def _run(self, payloads, code=0):
        reads = []

        def fake(args):
            reads.append(args)
            return code, payloads[min(len(reads), len(payloads)) - 1], "err"

        real = wfc._gh
        wfc._gh = fake
        self.addCleanup(setattr, wfc, "_gh", real)
        return wfc.wait_mergeable(285, repo="o/r", sleep=lambda _s: None)

    @staticmethod
    def _p(mergeable, status):
        return json.dumps({"mergeable": mergeable, "mergeStateStatus": status})

    def test_unknown_settles_before_concluding(self):
        v = self._run([self._p("UNKNOWN", "UNKNOWN"), self._p("MERGEABLE", "BLOCKED")])
        self.assertEqual((v["conclusion"], v["reads"]), ("mergeable", 2))
        self.assertEqual(v["hint"]["gate"], "green")

    def test_a_persistent_unknown_holds_rather_than_passing(self):
        v = self._run([self._p("UNKNOWN", "UNKNOWN")])
        self.assertEqual(v["conclusion"], "unknown")
        self.assertEqual(v["reads"], wfc.MERGEABLE_SETTLE_RETRIES + 1)
        self.assertEqual(v["hint"]["gate"], "unverified")

    def test_conflicting_and_dirty_are_blocking(self):
        self.assertEqual(
            self._run([self._p("CONFLICTING", "DIRTY")])["hint"]["gate"], "blocking"
        )
        self.assertEqual(
            self._run([self._p("MERGEABLE", "DIRTY")])["conclusion"], "conflicting"
        )

    def test_a_failed_probe_is_never_read_as_clean(self):
        v = self._run([""], code=1)
        self.assertEqual(
            (v["conclusion"], v["hint"]["gate"]), ("probe_failed", "unverified")
        )

    def test_the_cli_exits_zero_only_on_mergeable(self):
        real = wfc.wait_mergeable
        self.addCleanup(setattr, wfc, "wait_mergeable", real)
        for conclusion, code in (("mergeable", 0), ("unknown", 1)):
            wfc.wait_mergeable = lambda pr, repo, c=conclusion: {"conclusion": c}
            with redirect_stdout(io.StringIO()):
                self.assertEqual(wfc.run(["w", "--mergeable", "285"]), code)


class SnapshotTests(unittest.TestCase):
    """1943 part 2: the pre-wait spill must carry the work a signing failure
    left uncommitted — committed, staged, unstaged and untracked."""

    def _git(self, *args):
        code, out, err = wfc._git(list(args))
        self.assertEqual(code, 0, err)
        return out

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        cwd = os.getcwd()
        self.addCleanup(os.chdir, cwd)
        os.chdir(self.tmp.name)
        self._git("init", "-q", "-b", "main")
        self._git("config", "user.email", "t@t")
        self._git("config", "user.name", "t")
        self._git("config", "commit.gpgsign", "false")
        Path("a.txt").write_text("base\n")
        self._git("add", "a.txt")
        self._git("commit", "-q", "-m", "base")
        self._git("update-ref", "refs/remotes/origin/main", "HEAD")
        Path("b.txt").write_text("committed\n")
        self._git("add", "b.txt")
        self._git("commit", "-q", "-m", "b")
        Path("c.txt").write_text("staged\n")
        self._git("add", "c.txt")
        Path("a.txt").write_text("unstaged\n")
        Path("d.txt").write_text("untracked\n")

    def test_the_spill_applies_on_the_merge_base_with_every_change(self):
        out = Path(self.tmp.name) / "spill.diff"
        snap = wfc.snapshot(out, "main")
        self.assertIsNone(snap["error"])
        self.assertEqual(snap["untracked"], ["d.txt"])
        self.assertEqual(sorted(snap["uncommitted"]), ["a.txt", "c.txt"])
        # Recovery: a fresh checkout of the merge base, then one `git apply`.
        self._git("checkout", "-q", "-f", snap["merge_base"])
        self._git("clean", "-q", "-fd", "--exclude=spill.diff")
        self._git("apply", str(out))
        self.assertEqual(Path("a.txt").read_text(), "unstaged\n")
        self.assertEqual(Path("b.txt").read_text(), "committed\n")
        self.assertEqual(Path("c.txt").read_text(), "staged\n")
        self.assertEqual(Path("d.txt").read_text(), "untracked\n")

    def test_crlf_and_non_utf8_bytes_survive_the_round_trip(self):
        """Text mode would strip the `\\r` and replace the latin-1 byte."""
        crlf, latin = b"one\r\ntwo\r\n", b"caf\xe9\n"
        Path("a.txt").write_bytes(crlf)
        Path("e.txt").write_bytes(latin)
        out = Path(self.tmp.name) / "spill.diff"
        snap = wfc.snapshot(out, "main")
        self.assertIsNone(snap["error"])
        self._git("checkout", "-q", "-f", snap["merge_base"])
        self._git("clean", "-q", "-fd", "--exclude=spill.diff")
        self._git("apply", str(out))
        self.assertEqual(Path("a.txt").read_bytes(), crlf)
        self.assertEqual(Path("e.txt").read_bytes(), latin)

    def test_untracked_work_is_listed_from_a_subdirectory_too(self):
        Path("sub").mkdir()
        Path("sub/s.txt").write_text("deeper\n")
        os.chdir("sub")
        snap = wfc.snapshot(Path("spill.diff"), "main")
        self.assertIsNone(snap["error"])
        self.assertEqual(sorted(snap["untracked"]), ["d.txt", "sub/s.txt"])

    def test_a_missing_base_is_an_error_not_an_empty_spill(self):
        snap = wfc.snapshot(Path(self.tmp.name) / "spill.diff", "nope")
        self.assertTrue(snap["error"])

    def test_a_failed_untracked_listing_is_an_error_not_a_shorter_spill(self):
        real = wfc._git

        def fake(args, cwd=None):
            if args[0] == "ls-files":
                return 128, b"", "fatal: boom"
            return real(args, cwd)

        wfc._git = fake
        self.addCleanup(setattr, wfc, "_git", real)
        snap = wfc.snapshot(Path(self.tmp.name) / "spill.diff", "main")
        self.assertIn("boom", snap["error"])

    def test_wait_runs_the_snapshot_and_reports_it(self):
        real_watch, real_read = wfc.watch_checks, wfc.read_checks
        wfc.watch_checks = lambda pr, repo, interval, timeout, log: True
        wfc.read_checks = lambda pr, repo: [check("a", "pass")]
        self.addCleanup(setattr, wfc, "watch_checks", real_watch)
        self.addCleanup(setattr, wfc, "read_checks", real_read)
        out = Path(self.tmp.name) / "spill.diff"
        v = wfc.wait(285, repo="o/r", snapshot_path=out, base="main")
        self.assertIsNone(v["snapshot"]["error"])
        self.assertTrue(out.read_bytes())

    def test_the_cli_refuses_a_snapshot_outside_checks_mode(self):
        with self.assertRaises(wfc.WaitForChecksError):
            wfc.run(["w", "--run", "1", "--snapshot", "x.diff"])
        with self.assertRaises(wfc.WaitForChecksError):
            wfc.run(["w", "--pr", "1", "--base", "main"])
        with self.assertRaises(wfc.WaitForChecksError):
            wfc.run(["w", "--mergeable", "1", "--interval", "5"])

    def test_verdict_out_writes_the_same_verdict(self):
        real = wfc.wait_mergeable
        self.addCleanup(setattr, wfc, "wait_mergeable", real)
        wfc.wait_mergeable = lambda pr, repo: {"conclusion": "mergeable", "pr": pr}
        path = Path(self.tmp.name) / "v.json"
        with redirect_stdout(io.StringIO()):
            wfc.run(["w", "--mergeable", "9", "--verdict-out", str(path)])
        self.assertEqual(json.loads(path.read_text())["pr"], 9)

    def test_a_bad_verdict_out_path_still_prints_the_verdict(self):
        """A verdict can take an hour to earn; a bad path must not lose it."""
        real = wfc.wait_mergeable
        self.addCleanup(setattr, wfc, "wait_mergeable", real)
        wfc.wait_mergeable = lambda pr, repo: {"conclusion": "mergeable", "pr": pr}
        buf = io.StringIO()
        with redirect_stdout(buf), self.assertRaises(wfc.WaitForChecksError):
            wfc.run(["w", "--mergeable", "9", "--verdict-out", "/nonexistent/v.json"])
        self.assertEqual(json.loads(buf.getvalue())["pr"], 9)


if __name__ == "__main__":
    unittest.main()
