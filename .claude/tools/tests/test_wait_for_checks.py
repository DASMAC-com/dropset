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
import signal
import tempfile
import time
import unittest
from contextlib import redirect_stdout
from datetime import datetime, timedelta, timezone
from pathlib import Path
from unittest import mock

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

        The pacing is recorded through the injected ``sleep``, never the
        patched global: a real subprocess wait with a timeout polls through
        ``time.sleep`` (CPython's doubling 0.0005..0.05 s loop), so a slow
        runner recorded stray sleeps here when the global was the recorder.
        The `none` probe's gh call is stubbed as well, keeping it offline.
        """
        real_gh = wfc._gh
        wfc._gh = lambda _args: (
            0,
            '{"mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN"}',
            "",
        )
        self.addCleanup(setattr, wfc, "_gh", real_gh)
        pacing = []
        self._stub_rounds([[]])
        wfc.wait(285, repo="o/r", sleep=pacing.append)
        self.assertTrue(pacing)
        self.assertEqual(set(pacing), {wfc.NONE_SETTLE_SECONDS})

        pacing = []
        self._stub_rounds([[check("a", "pending")]])
        wfc.wait(285, repo="o/r", interval=17, sleep=pacing.append)
        self.assertTrue(pacing)
        self.assertEqual(set(pacing), {17})

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


def check_runs(*statuses, conclusion="success"):
    runs = [
        {
            "name": f"job{i}",
            "status": s,
            "conclusion": conclusion if s == "completed" else None,
        }
        for i, s in enumerate(statuses)
    ]
    return {"total_count": len(runs), "check_runs": runs}


class WatchChecksTests(unittest.TestCase):
    """The CI watch loop over recorded check-run responses, time advancing
    only through ``sleep``."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.log = Path(self._tmp.name) / "watch.log"

    def _watch(self, gh, interval=20, timeout=3600):
        now = [0.0]
        self.sleeps = []

        def sleep(seconds):
            self.sleeps.append(seconds)
            now[0] += seconds

        return wfc.watch_checks(
            285,
            "o/r",
            interval,
            timeout,
            self.log,
            client=wfc.Conditional(gh),
            sleep=sleep,
            clock=lambda: now[0],
        )

    def test_settles_when_every_run_completes_and_logs_each_poll(self):
        gh = RecordedGh(
            {
                "pull": [pull()],
                "check_runs": [check_runs("queued", "in_progress")] * 3
                + [check_runs("completed", "completed")],
            }
        )
        self.assertTrue(self._watch(gh))
        self.assertEqual(self.sleeps, [20, 20, 20])
        lines = [json.loads(x) for x in self.log.read_text().splitlines()]
        self.assertEqual([x["open"] for x in lines], [2, 2, 2, 0])
        # Unchanged reads answer 304: the pull on polls 2-4, the runs on 2-3.
        self.assertEqual([s for _, s in gh.statuses].count(304), 5)

    def test_a_failed_run_settles_too(self):
        """A red run is an outcome; the verdict read says which."""
        gh = RecordedGh(
            {
                "pull": [pull()],
                "check_runs": [check_runs("completed", conclusion="failure")],
            }
        )
        self.assertTrue(self._watch(gh))

    def test_no_run_at_all_settles_at_once(self):
        """The registration race is the caller's `none` rounds, not this loop's."""
        gh = RecordedGh({"pull": [pull()], "check_runs": [check_runs()]})
        self.assertTrue(self._watch(gh))
        self.assertEqual(self.sleeps, [])

    def test_a_push_mid_wait_moves_the_watch_to_the_new_head(self):
        gh = RecordedGh(
            {
                "pull": [pull(), pull(sha="b" * 40)],
                "check_runs": [check_runs("in_progress"), check_runs("completed")],
            }
        )
        self.assertTrue(self._watch(gh))
        lines = [json.loads(x) for x in self.log.read_text().splitlines()]
        self.assertEqual([x["sha"] for x in lines], ["a" * 12, "b" * 12])

    def test_an_open_run_times_out(self):
        gh = RecordedGh({"pull": [pull()], "check_runs": [check_runs("in_progress")]})
        self.assertFalse(self._watch(gh, timeout=100))
        self.assertLessEqual(sum(self.sleeps), 100)

    def test_a_low_quota_backs_off(self):
        gh = RecordedGh(
            {
                "pull": [pull()],
                "check_runs": [check_runs("in_progress"), check_runs("completed")],
            },
            remaining=500,
        )
        self.assertTrue(self._watch(gh))
        self.assertEqual(self.sleeps, [wfc.BACKOFF_INTERVAL])

    def test_persistent_read_errors_hand_over_to_the_verdict_read(self):
        client_gh = lambda args: (1, "", "HTTP 401")  # noqa: E731
        self.assertTrue(self._watch(client_gh))
        self.assertEqual(len(self.sleeps), wfc.MAX_PROBE_ERRORS - 1)

    def test_the_log_is_owner_only(self):
        """A CI log can carry build output worth keeping off a shared /tmp — the
        same reason review_diff.py's slices are 0o600."""
        self._watch(RecordedGh({"pull": [pull()], "check_runs": [check_runs()]}))
        self.assertEqual(self.log.stat().st_mode & 0o777, 0o600)


class WatchRunTests(unittest.TestCase):
    """The merge-queue mode. Unlike the checks mode there is no second JSON
    read, so here gh's ``--exit-status`` code *is* the verdict — which is why a
    timeout must not be allowed to look like a pass."""

    # The bound for a shim that exits at once. It only catches a hang, so it is
    # generous: at 30s a busy machine's slow `sh` spawn hit it, the child was
    # killed before writing, and an assertion on the log read back ``''``.
    SETTLE = 300

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
        settled, code = wfc.watch_run("99", "o/r", self.SETTLE, self.log)
        self.assertTrue(settled)
        self.assertEqual(code, 0)
        self.assertIn("build", self.log.read_text(encoding="utf-8"))

    def test_a_dequeued_run_is_non_zero(self):
        """`--exit-status` is what keeps a dequeue from reading as a merge."""
        self._shim("exit 1")
        settled, code = wfc.watch_run("99", "o/r", self.SETTLE, self.log)
        self.assertTrue(settled)
        self.assertNotEqual(code, 0)

    def test_a_hung_run_times_out_and_is_killed(self):
        self._shim("sleep 30")
        settled, code = wfc.watch_run("99", "o/r", 1, self.log)
        self.assertFalse(settled)
        self.assertEqual(code, -1)

    def test_the_log_is_owner_only(self):
        self._shim("echo hi")
        wfc.watch_run("99", "o/r", self.SETTLE, self.log)
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
        settled, _ = wfc.watch_run("99", "o/r", self.SETTLE, self.log, interval=45)
        self.assertTrue(settled, "the shim was killed before it could log")
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
    rest="OPEN",
    merged_at=None,
    runs=(),
    enqueued=None,
    removed=None,
    tl_merged=False,
):
    return {
        "gql_state": gql,
        "gql_merged": merged,
        "entry": entry,
        "auto": auto,
        "rest_state": rest,
        "rest_merged_at": merged_at,
        "events": {"enqueued_at": enqueued, "removed_at": removed, "merged": tl_merged},
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

    def _watch(
        self,
        snaps,
        interval=60,
        timeout=7200,
        since=None,
        await_group_seconds=wfc.AWAIT_GROUP_SECONDS,
    ):
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
            await_group_seconds=await_group_seconds,
        )

    def test_pending_contexts_names_both_context_types_required_or_not(self):
        node = {
            "commits": {
                "nodes": [
                    {
                        "commit": {
                            "statusCheckRollup": {
                                "contexts": {
                                    "nodes": [
                                        {
                                            "__typename": "CheckRun",
                                            "name": "Explorer image",
                                            "status": "IN_PROGRESS",
                                            "isRequired": False,
                                        },
                                        {
                                            "__typename": "CheckRun",
                                            "name": "Tests",
                                            "status": "COMPLETED",
                                            "isRequired": True,
                                        },
                                        {
                                            "__typename": "StatusContext",
                                            "context": "Semantic PR",
                                            "state": "PENDING",
                                            "isRequired": True,
                                        },
                                        {
                                            "__typename": "StatusContext",
                                            "context": "done",
                                            "state": "SUCCESS",
                                            "isRequired": True,
                                        },
                                    ]
                                }
                            }
                        }
                    }
                ]
            }
        }
        self.assertEqual(
            wfc.pending_contexts(node),
            [
                {"name": "Explorer image", "required": False},
                {"name": "Semantic PR", "required": True},
            ],
        )
        self.assertEqual(wfc.pending_contexts({"commits": {"nodes": []}}), [])
        self.assertEqual(wfc.pending_contexts({}), [])

    def test_stays_silent_while_queued_then_reports_the_merge(self):
        v = self._watch([snap(entry=QUEUED)] * 5 + [snap(gql="MERGED", merged=True)])
        self.assertEqual(v["conclusion"], "merged")
        self.assertTrue(v["settled"])
        self.assertEqual(v["polls"], 6)
        self.assertTrue(v["seen_queue_entry"])

    def test_a_stale_entry_does_not_outrank_a_merge_on_the_other_source(self):
        """The PR #420 shape: GraphQL still AWAITING_CHECKS, the PR MERGED."""
        stale = {"state": "AWAITING_CHECKS", "enqueuedAt": ENQUEUED}
        v = self._watch([snap(entry=stale, rest="MERGED", merged_at=ENQUEUED)])
        self.assertEqual(v["conclusion"], "merged")
        self.assertEqual(v["polls"], 1)

    def test_a_graphql_merge_wins_over_a_stale_view(self):
        """The PR #424 shape: the opposite direction of staleness."""
        v = self._watch([snap(gql="MERGED", merged=True, rest="OPEN")])
        self.assertEqual(v["conclusion"], "merged")

    def test_a_timeline_removal_dequeues_at_the_next_poll(self):
        """No confirmation streak and no grace: the event is the answer."""
        v = self._watch(
            [snap(entry=QUEUED, enqueued=ENQUEUED)] * 3
            + [snap(enqueued=ENQUEUED, removed="2026-10-13T12:01:00Z")],
            interval=20,
        )
        self.assertEqual(v["conclusion"], "dequeued")
        self.assertEqual(v["polls"], 4)
        self.assertEqual(v["elapsed_seconds"], 60)
        self.assertEqual(v["last"]["removed_at"], "2026-10-13T12:01:00Z")

    def test_a_merge_writes_a_removal_too_and_is_still_a_merge(self):
        """A merge's timeline carries `removed_from_merge_queue` beside it."""
        merged = snap(enqueued=ENQUEUED, removed=ENQUEUED, tl_merged=True)
        v = self._watch([snap(entry=QUEUED), merged])
        self.assertEqual(v["conclusion"], "merged")

    def test_a_timeline_enqueue_keeps_the_watch_waiting_past_the_grace(self):
        """GraphQL's entry silent, the timeline still queued: not a dequeue."""
        queued = snap(enqueued=ENQUEUED)
        polls = int(wfc.AMBIGUOUS_GRACE_SECONDS / 20) + 3
        v = self._watch([queued] * polls + [snap(rest="MERGED")], interval=20)
        self.assertEqual(v["conclusion"], "merged")
        self.assertTrue(v["seen_queue_entry"])

    def test_a_low_quota_backs_the_poll_off(self):
        client = wfc.Conditional()
        client.remaining, client.limit = 900, 5000
        probe_calls = [0]

        def probe(pr, repo):
            probe_calls[0] += 1
            return snap(entry=QUEUED) if probe_calls[0] < 3 else snap(rest="MERGED")

        probe.client = client
        sleeps = []
        now = [0.0]

        def sleep(seconds):
            sleeps.append(seconds)
            now[0] += seconds

        v = wfc.wait_queue(
            285, interval=20, probe=probe, sleep=sleep, clock=lambda: now[0]
        )
        self.assertEqual(v["conclusion"], "merged")
        self.assertEqual(set(sleeps), {wfc.BACKOFF_INTERVAL})

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
        self.assertEqual(
            v["not_queued"],
            {
                "reading": "never_entered",
                "prior_runs": 0,
                "newest_prior": None,
                "prior_failing_runs": [],
            },
        )

    def test_a_not_queued_beside_an_old_cycles_run_says_so(self):
        """The run-list disambiguation, done by the tool: runs exist, all from
        before this cycle's window, so they are an earlier enqueue's."""
        old = queue_run("failure", created="2026-10-13T11:00:00Z", rid="1")
        newer = queue_run("success", created="2026-10-13T11:10:00Z", rid="2")
        v = self._watch([snap(runs=[old, newer])])
        self.assertEqual(v["conclusion"], "not_queued")
        self.assertEqual(v["queue_runs"], [])
        self.assertEqual(v["not_queued"]["reading"], "prior_cycle_only")
        self.assertEqual(v["not_queued"]["prior_runs"], 2)
        self.assertEqual(
            v["not_queued"]["newest_prior"],
            {"run_id": "2", "conclusion": "success", "created_at": newer["created_at"]},
        )
        # The newest prior branch's failures ride along for classification.
        self.assertEqual(
            [r["run_id"] for r in v["not_queued"]["prior_failing_runs"]], ["1"]
        )

    def test_only_not_queued_carries_the_reading(self):
        v = self._watch([snap(gql="MERGED", merged=True)])
        self.assertNotIn("not_queued", v)

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
        v = self._watch([snap(entry=QUEUED), snap(gql="CLOSED", rest="CLOSED")])
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

    def test_auto_merge_with_no_entry_is_auto_merge_waiting_not_queued(self):
        """The PR 548 shape: auto-merge enabled, a non-required check pending,
        no entry. Once a bounded wait, never a silent `seen_entry` to timeout."""
        auto = {"enabledAt": ENQUEUED}
        pending = [{"name": "Explorer image", "required": False}]
        waiting = dict(snap(auto=auto), merge_state_status="BLOCKED")
        waiting["pending_checks"] = pending
        v = self._watch([waiting])
        self.assertEqual(v["conclusion"], "auto_merge_waiting")
        self.assertTrue(v["settled"])
        self.assertFalse(v["seen_queue_entry"])
        self.assertGreaterEqual(v["elapsed_seconds"], wfc.AUTO_MERGE_WAIT_SECONDS)
        self.assertLess(v["elapsed_seconds"], wfc.AUTO_MERGE_WAIT_SECONDS + 60)
        self.assertEqual(v["last"]["auto_enabled_at"], ENQUEUED)
        self.assertEqual(v["last"]["merge_state_status"], "BLOCKED")
        self.assertEqual(v["last"]["pending_checks"], pending)

    def test_a_required_check_still_running_holds_the_auto_merge_bound(self):
        """Auto-merge legitimately waits on a required check: the bound
        counts only from when nothing required is pending."""
        auto = {"enabledAt": ENQUEUED}
        running = dict(snap(auto=auto))
        running["pending_checks"] = [{"name": "Tests", "required": True}]
        polls = wfc.AUTO_MERGE_WAIT_SECONDS // 60 + 5
        v = self._watch([running] * polls + [snap(auto=auto)])
        self.assertEqual(v["conclusion"], "auto_merge_waiting")
        self.assertGreaterEqual(
            v["elapsed_seconds"], polls * 60 + wfc.AUTO_MERGE_WAIT_SECONDS
        )

    def test_a_flap_out_of_auto_merge_restarts_its_bound(self):
        """auto → all-null → auto: the second stretch is timed from scratch."""
        auto = snap(auto={"enabledAt": ENQUEUED})
        half = wfc.AUTO_MERGE_WAIT_SECONDS // 60 // 2 + 1
        v = self._watch([auto] * half + [snap()] + [auto])
        self.assertEqual(v["conclusion"], "auto_merge_waiting")
        self.assertGreaterEqual(
            v["elapsed_seconds"], (half + 1) * 60 + wfc.AUTO_MERGE_WAIT_SECONDS
        )

    def test_a_timeline_only_enqueue_with_no_group_is_queued_no_group(self):
        """GraphQL's entry null, the timeline still queued, no run: bounded too,
        with the entry fields honestly null."""
        v = self._watch([snap(enqueued=ENQUEUED)], await_group_seconds=600)
        self.assertEqual(v["conclusion"], "queued_no_group")
        self.assertIsNone(v["last"]["entry_state"])
        self.assertIsNone(v["last"]["entry_position"])

    def test_auto_merge_that_enters_the_queue_keeps_waiting(self):
        """Within the bound the entry appears: an ordinary queued merge."""
        auto = {"enabledAt": ENQUEUED}
        v = self._watch(
            [snap(auto=auto)] * 5
            + [snap(entry=QUEUED, runs=[queue_run()])] * 3
            + [snap(merged=True)]
        )
        self.assertEqual(v["conclusion"], "merged")
        self.assertTrue(v["seen_queue_entry"])

    def test_classic_auto_merge_keeps_the_watch_waiting_behind_its_flag(self):
        """No queue on the repo: autoMergeRequest alone is the queued signal."""
        auto = {"enabledAt": ENQUEUED}
        polls = wfc.AUTO_MERGE_WAIT_SECONDS // 60 + 3
        now = [0.0]
        calls = [0]

        def probe(pr, repo):
            calls[0] += 1
            return snap(auto=auto) if calls[0] <= polls else snap(merged=True)

        def sleep(seconds):
            now[0] += seconds

        v = wfc.wait_queue(
            285,
            interval=60,
            probe=probe,
            sleep=sleep,
            clock=lambda: now[0],
            wall=lambda: ENQUEUED_EPOCH + now[0],
            classic_auto_merge=True,
        )
        self.assertEqual(v["conclusion"], "merged")
        self.assertTrue(v["seen_queue_entry"])

    def test_an_entry_with_no_group_run_is_queued_no_group(self):
        """Entry seen, no merge-group run within the bound: a bounded signal,
        reporting the entry's state and position."""
        entry = dict(QUEUED, position=3)
        v = self._watch([snap(entry=entry)], await_group_seconds=600)
        self.assertEqual(v["conclusion"], "queued_no_group")
        self.assertTrue(v["settled"])
        self.assertTrue(v["seen_queue_entry"])
        self.assertGreaterEqual(v["elapsed_seconds"], 600)
        self.assertLess(v["elapsed_seconds"], 660)
        self.assertEqual(v["last"]["entry_state"], "QUEUED")
        self.assertEqual(v["last"]["entry_position"], 3)
        self.assertIsNone(v["first_run_seen_at"])

    def test_a_group_run_records_when_it_appeared_and_disarms_the_bound(self):
        later = "2026-10-13T12:10:00Z"
        group = snap(
            entry=QUEUED, runs=[queue_run(status="in_progress", created=later)]
        )
        v = self._watch(
            [snap(entry=QUEUED)] * 5 + [group] * 20 + [snap(merged=True)],
            await_group_seconds=600,
        )
        self.assertEqual(v["conclusion"], "merged")
        self.assertEqual(v["first_run_seen_at"], "2026-10-13T12:05:00Z")

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


def _route(path: str) -> str:
    for marker, key in (
        ("/timeline", "timeline"),
        ("/check-runs", "check_runs"),
        ("/actions/runs", "runs"),
        ("/pulls/", "pull"),
    ):
        if marker in path:
            return key
    raise AssertionError(f"unrouted path {path}")


class RecordedGh:
    """A ``gh api -i`` stand-in serving recorded responses the way GitHub does.

    ``routes`` maps a route key (:func:`_route`) to its bodies, one per poll —
    the poll being the count of first reads so far, the last body repeating.
    Each body carries an ETag derived from its content (weak, as the timeline
    and check-runs send), and a request whose ``If-None-Match`` matches gets a
    ``304`` that leaves ``remaining`` untouched, with gh's exit 1. The GraphQL
    read answers ``graphql`` (not conditional).
    """

    def __init__(self, routes, graphql=None, remaining=4000, limit=5000):
        self.routes = routes
        self.graphql = graphql or {"state": "OPEN", "merged": False}
        self.remaining = remaining
        self.limit = limit
        self.polls = 0
        self.statuses: list[tuple[str, int]] = []

    def _headers(self, status, etag=None):
        lines = [
            f"HTTP/2.0 {status} {'OK' if status == 200 else 'Not Modified'}",
            f"X-Ratelimit-Limit: {self.limit}",
            f"X-Ratelimit-Remaining: {self.remaining}",
        ]
        if etag:
            lines.append(f"Etag: {etag}")
        return "\r\n".join(lines) + "\r\n\r\n"

    def __call__(self, args):
        if args[:2] == ["api", "graphql"]:
            node = {"autoMergeRequest": None, "mergeQueueEntry": None, **self.graphql}
            body = {"data": {"repository": {"pullRequest": node}}}
            return 0, json.dumps(body), ""
        path = args[2]
        sent = args[args.index("-H") + 1].split(": ", 1)[1] if "-H" in args else None
        key = _route(path)
        # A poll starts with its first read: the timeline in queue mode, the
        # pull in checks mode.
        if key == ("timeline" if "timeline" in self.routes else "pull"):
            self.polls += 1
        bodies = self.routes[key]
        body = bodies[min(max(self.polls, 1) - 1, len(bodies) - 1)]
        text = json.dumps(body)
        etag = f'W/"{abs(hash(text))}"'
        if sent == etag:
            self.statuses.append((key, 304))
            return 1, self._headers(304, etag), "gh: HTTP 304"
        self.remaining -= 1
        self.statuses.append((key, 200))
        return 0, self._headers(200, etag) + text, ""


def pull(merged=False, state="open", sha="a" * 40):
    return {
        "state": "closed" if merged else state,
        "merged": merged,
        "merged_at": ENQUEUED if merged else None,
        "head": {"sha": sha},
    }


def event(name, at=ENQUEUED):
    return {"event": name, "created_at": at}


NO_RUNS = {"workflow_runs": []}


class ConditionalTests(unittest.TestCase):
    def test_an_unchanged_read_is_a_304_served_from_cache(self):
        gh = RecordedGh({"pull": [pull()]})
        client = wfc.Conditional(gh)
        first = client.get("/repos/o/r/pulls/1")
        second = client.get("/repos/o/r/pulls/1")
        self.assertEqual(first, second)
        self.assertEqual([s for _, s in gh.statuses], [200, 304])
        self.assertEqual(
            client.report(),
            {
                "sent": 2,
                "not_modified": 1,
                "rate_remaining": 3999,
            },
        )

    def test_the_quota_header_drives_the_back_off(self):
        client = wfc.Conditional(RecordedGh({"pull": [pull()]}, remaining=999))
        client.get("/repos/o/r/pulls/1")
        self.assertEqual(client.pace(20), wfc.BACKOFF_INTERVAL)
        client = wfc.Conditional(RecordedGh({"pull": [pull()]}, remaining=1001))
        client.get("/repos/o/r/pulls/1")
        self.assertEqual(client.pace(20), 20)

    def test_no_status_line_is_an_error_naming_gh(self):
        client = wfc.Conditional(lambda args: (1, "", "HTTP 401: Bad credentials"))
        with self.assertRaisesRegex(wfc.WaitForChecksError, "Bad credentials"):
            client.get("/repos/o/r/pulls/1")

    def test_a_non_200_is_an_error(self):
        reply = "HTTP/2.0 502 Bad Gateway\r\n\r\n"
        client = wfc.Conditional(lambda args: (1, reply, ""))
        with self.assertRaisesRegex(wfc.WaitForChecksError, "HTTP 502"):
            client.get("/repos/o/r/pulls/1")

    def test_the_poll_knob(self):
        self.assertEqual(wfc.poll_interval({}), wfc.DEFAULT_POLL)
        self.assertEqual(wfc.poll_interval({wfc.POLL_ENV: "15"}), 15)
        for bad in ("", "x", "0", "-5"):
            with self.subTest(bad=bad):
                self.assertEqual(
                    wfc.poll_interval({wfc.POLL_ENV: bad}), wfc.DEFAULT_POLL
                )


class TimelineStateTests(unittest.TestCase):
    def test_a_removal_counts_only_after_the_newest_enqueue(self):
        events = [
            event("added_to_merge_queue", "2026-10-13T11:00:00Z"),
            event("removed_from_merge_queue", "2026-10-13T11:05:00Z"),
            event("added_to_merge_queue"),
        ]
        state = wfc.timeline_state(events)
        self.assertEqual(state["enqueued_at"], ENQUEUED)
        self.assertIsNone(state["removed_at"])
        state = wfc.timeline_state([*events, event("removed_from_merge_queue")])
        self.assertEqual(state["removed_at"], ENQUEUED)
        self.assertFalse(state["merged"])

    def test_no_enqueue_means_no_removal(self):
        state = wfc.timeline_state([event("removed_from_merge_queue")])
        self.assertEqual(
            state, {"enqueued_at": None, "removed_at": None, "merged": False}
        )


class RecordedQueueScenarioTests(unittest.TestCase):
    """The queue watch end to end — :class:`wfc.QueueProbe` over
    :class:`RecordedGh` — with time advancing only through ``sleep``."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        real = wfc.queue_log_path_for
        wfc.queue_log_path_for = lambda pr: Path(self.tmp.name) / f"q-{pr}.log"
        self.addCleanup(setattr, wfc, "queue_log_path_for", real)

    def _watch(self, gh, interval=20):
        real = wfc._gh
        wfc._gh = gh
        self.addCleanup(setattr, wfc, "_gh", real)
        now = [0.0]

        def sleep(seconds):
            now[0] += seconds

        probe = wfc.QueueProbe(wfc.Conditional(gh), clock=lambda: now[0])
        return wfc.wait_queue(
            285,
            interval=interval,
            probe=probe,
            sleep=sleep,
            clock=lambda: now[0],
            wall=lambda: ENQUEUED_EPOCH + now[0],
        )

    def test_a_304_stream_costs_no_quota_until_the_merge(self):
        queued = [event("added_to_merge_queue")]
        gh = RecordedGh(
            {
                "pull": [pull()] * 5 + [pull(merged=True)],
                "timeline": [queued] * 5
                + [
                    [
                        *queued,
                        event("merged"),
                        event("closed"),
                        event("removed_from_merge_queue"),
                    ]
                ],
                "runs": [NO_RUNS],
            },
            graphql={"mergeQueueEntry": QUEUED},
        )
        v = self._watch(gh)
        self.assertEqual(v["conclusion"], "merged")
        self.assertEqual(v["polls"], 6)
        # Polls 2-5: the pull and the timeline both answer 304.
        unchanged = [k for k, s in gh.statuses if s == 304 and k != "runs"]
        self.assertEqual(len(unchanged), 8)
        self.assertEqual(v["requests"]["rate_remaining"], gh.remaining)
        self.assertGreaterEqual(v["requests"]["not_modified"], 8)

    def test_an_enqueue_then_a_dequeue_is_known_at_the_next_poll(self):
        added = [event("added_to_merge_queue")]
        gh = RecordedGh(
            {
                "pull": [pull()],
                "timeline": [
                    [],
                    added,
                    added,
                    [*added, event("removed_from_merge_queue")],
                ],
                "runs": [NO_RUNS],
            }
        )
        v = self._watch(gh)
        self.assertEqual(v["conclusion"], "dequeued")
        self.assertEqual(v["polls"], 4)
        self.assertEqual(v["elapsed_seconds"], 60)
        self.assertTrue(v["seen_queue_entry"])
        self.assertEqual(v["enqueued_at"], ENQUEUED)

    def test_a_prior_cycles_dequeue_is_not_this_cycles(self):
        """Re-enqueued after a fix, its checks still pending: GitHub writes no
        new enqueue event yet, so the newest add-then-remove pair is the last
        cycle's. Auto-merge keeps the watch waiting until the new cycle shows."""
        # The old cycle sits inside the run window, its failed run with it.
        old = [
            event("added_to_merge_queue", "2026-10-13T11:48:00Z"),
            event("removed_from_merge_queue", "2026-10-13T11:52:00Z"),
        ]
        new_add = [*old, event("added_to_merge_queue", "2026-10-13T12:02:00Z")]
        gh = RecordedGh(
            {
                "pull": [pull()],
                "timeline": [old] * 4
                + [
                    new_add,
                    new_add,
                    [
                        *new_add,
                        event("removed_from_merge_queue", "2026-10-13T12:03:00Z"),
                    ],
                ],
                "runs": [
                    {
                        "workflow_runs": [
                            {
                                "id": 9,
                                "conclusion": "failure",
                                "status": "completed",
                                "head_branch": "gh-readonly-queue/main/pr-285-old",
                                "created_at": "2026-10-13T11:50:00Z",
                            }
                        ]
                    }
                ],
            },
            graphql={"autoMergeRequest": {"enabledAt": ENQUEUED}},
        )
        v = self._watch(gh)
        self.assertEqual(v["conclusion"], "dequeued")
        self.assertEqual(v["polls"], 7)
        self.assertEqual(v["last"]["removed_at"], "2026-10-13T12:03:00Z")
        # The old cycle's failed run never entered the window.
        self.assertEqual(v["queue_runs"], [])
        # A standing old removal keeps corroboration at once a minute:
        # polls 1 and 4, then poll 7's new removal.
        self.assertEqual([k for k, _ in gh.statuses].count("runs"), 3)

    def test_since_admits_a_removal_older_than_the_watch(self):
        """A resumed call: the dequeue predates the watch, `--since` vouches."""
        added = [event("added_to_merge_queue", "2026-10-13T11:40:00Z")]
        gone = [*added, event("removed_from_merge_queue", "2026-10-13T11:41:00Z")]
        gh = RecordedGh({"pull": [pull()], "timeline": [gone], "runs": [NO_RUNS]})
        real = wfc._gh
        wfc._gh = gh
        self.addCleanup(setattr, wfc, "_gh", real)
        now = [0.0]
        v = wfc.wait_queue(
            285,
            interval=20,
            probe=wfc.QueueProbe(wfc.Conditional(gh), clock=lambda: now[0]),
            sleep=lambda s: now.__setitem__(0, now[0] + s),
            clock=lambda: now[0],
            wall=lambda: ENQUEUED_EPOCH + now[0],
            since="2026-10-13T11:40:00Z",
        )
        self.assertEqual(v["conclusion"], "dequeued")
        self.assertEqual(v["polls"], 1)

    def test_a_merge_is_reported_at_the_next_poll(self):
        gh = RecordedGh(
            {
                "pull": [pull(), pull(), pull(merged=True)],
                "timeline": [[event("added_to_merge_queue")]],
                "runs": [NO_RUNS],
            },
            graphql={"mergeQueueEntry": QUEUED},
        )
        v = self._watch(gh)
        self.assertEqual(v["conclusion"], "merged")
        self.assertEqual(v["elapsed_seconds"], 40)
        self.assertEqual(v["last"]["rest_state"], "MERGED")

    def test_corroboration_is_once_a_minute_and_at_once_on_a_removal(self):
        added = [event("added_to_merge_queue")]
        gh = RecordedGh(
            {
                "pull": [pull()],
                "timeline": [added] * 5 + [[*added, event("removed_from_merge_queue")]],
                "runs": [NO_RUNS],
            },
            graphql={"mergeQueueEntry": QUEUED},
        )
        self._watch(gh)
        runs_reads = [i for i, (k, _) in enumerate(gh.statuses) if k == "runs"]
        # Polls 1 and 4 (the minute), then poll 6 for the removal.
        self.assertEqual(len(runs_reads), 3)

    def test_the_queue_runs_keep_only_this_prs_branch(self):
        runs = {
            "workflow_runs": [
                {
                    "id": 1,
                    "head_branch": "gh-readonly-queue/main/pr-285-abc",
                    "created_at": ENQUEUED,
                },
                {
                    "id": 2,
                    "head_branch": "gh-readonly-queue/main/pr-2850-abc",
                    "created_at": ENQUEUED,
                },
                {"id": 3, "head_branch": "eng-285", "created_at": ENQUEUED},
            ]
        }
        gh = RecordedGh({"pull": [pull()], "timeline": [[]], "runs": [runs]})
        got = wfc.QueueProbe(wfc.Conditional(gh))
        real = wfc._gh
        wfc._gh = gh
        self.addCleanup(setattr, wfc, "_gh", real)
        self.assertEqual([r["run_id"] for r in got(285, "o/r")["queue_runs"]], ["1"])

    def test_a_null_pull_request_is_a_probe_error_not_a_crash(self):
        reply = json.dumps({"data": {"repository": {"pullRequest": None}}})
        rest = RecordedGh({"pull": [pull()], "timeline": [[]], "runs": [NO_RUNS]})
        real = wfc._gh
        wfc._gh = lambda args: (0, reply, "") if "graphql" in args else rest(args)
        self.addCleanup(setattr, wfc, "_gh", real)
        with self.assertRaises(wfc.WaitForChecksError):
            wfc.QueueProbe(wfc.Conditional())(285, "o/r")

    def test_a_failed_read_raises(self):
        client = wfc.Conditional(lambda args: (1, "", "HTTP 502"))
        with self.assertRaises(wfc.WaitForChecksError):
            wfc.QueueProbe(client)(285, "o/r")


class QueueModeCliTests(unittest.TestCase):
    def _queue_mode(
        self,
        conclusion,
        extra=(),
        runs=(),
        mergeable="MERGEABLE",
        stub=True,
        default_since=(None, None),
        also=None,
    ):
        seen = {}
        real = wfc.wait_queue

        def stub_wait(
            pr,
            repo=None,
            interval=None,
            timeout=None,
            since=None,
            await_group_seconds=None,
            classic_auto_merge=None,
        ):
            seen.update(
                pr=pr,
                repo=repo,
                interval=interval,
                timeout=timeout,
                since=since,
                await_group_seconds=await_group_seconds,
                classic_auto_merge=classic_auto_merge,
            )
            return {
                "conclusion": conclusion,
                "pr": pr,
                "queue_runs": list(runs),
                "failing_runs": [r for r in runs if r.get("conclusion") == "failure"],
                **json.loads(json.dumps(also or {})),
            }

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

        def dismiss(pr, repo, merged=True):
            seen.setdefault("dismissed", []).append((pr, repo))
            seen["unsubscribe"] = merged
            return {
                "dismissed": 1,
                "unsubscribed": int(merged),
                "failed": 0,
                "error": None,
            }

        real_cause = wfc.dequeue_cause

        def cause(pr, repo, enqueued_at=None, conflicting=False):
            seen.setdefault("caused", []).append((pr, repo))
            seen["cause_conflicting"] = conflicting
            return {"dequeue_reason": "merge_conflict", "error": None}

        real_count = wfc.todays_merge_count

        def count(pr, repo):
            seen.setdefault("counted", []).append((pr, repo))
            return 4

        def since(pr, repo):
            seen.setdefault("defaulted", []).append(pr)
            return default_since

        def classify(failing, repo):
            seen.setdefault("classified", []).append(failing)
            return {"cause": "transient", "evidence": [], "next_command": None}

        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        path = Path(tmp.name) / "verdict.json"
        for name, value in (
            ("default_since", since),
            ("classify_dequeue", classify),
            ("queue_verdict_path_for", lambda pr: path),
        ):
            self.addCleanup(setattr, wfc, name, getattr(wfc, name))
            setattr(wfc, name, value)
        if stub:
            wfc.wait_queue = stub_wait
        wfc.dismiss_pr_notifications = dismiss
        wfc.dequeue_cause = cause
        wfc.todays_merge_count = count
        try:
            with redirect_stdout(out):
                code = wfc.run(["wait_for_checks.py", "--queue", "285", *extra])
        finally:
            wfc.wait_queue = real
            wfc.dismiss_pr_notifications = real_dismiss
            wfc.dequeue_cause = real_cause
            wfc.todays_merge_count = real_count
        seen["stdout"] = out.getvalue()
        seen["verdict"] = json.loads(path.read_text())
        return code, seen

    def test_the_queue_bounds_reach_the_watch_and_default_off(self):
        _, seen = self._queue_mode("merged")
        self.assertEqual(seen["await_group_seconds"], wfc.AWAIT_GROUP_SECONDS)
        self.assertFalse(seen["classic_auto_merge"])
        _, seen = self._queue_mode(
            "merged", extra=("--await-group-seconds", "300", "--classic-auto-merge")
        )
        self.assertEqual(seen["await_group_seconds"], 300)
        self.assertTrue(seen["classic_auto_merge"])

    def test_a_non_positive_group_bound_is_refused(self):
        for value in ("0", "-5"):
            with self.subTest(value=value):
                with self.assertRaisesRegex(wfc.WaitForChecksError, "positive"):
                    wfc.run(
                        [
                            "wait_for_checks.py",
                            "--queue",
                            "285",
                            "--await-group-seconds",
                            value,
                        ]
                    )

    def test_the_queue_bounds_are_refused_outside_queue_mode(self):
        for flag in (["--await-group-seconds", "300"], ["--classic-auto-merge"]):
            with self.subTest(flag=flag[0]):
                with self.assertRaisesRegex(
                    wfc.WaitForChecksError, "only with --queue"
                ):
                    wfc.run(["wait_for_checks.py", "--pr", "285", *flag])

    def test_a_merge_dismisses_the_prs_notifications(self):
        _, seen = self._queue_mode("merged")
        self.assertEqual(seen["dismissed"], [(285, wfc.DEFAULT_REPO)])
        self.assertEqual(seen["verdict"]["notifications"]["dismissed"], 1)

    def test_a_merge_reports_todays_merge_count_and_unsubscribes(self):
        _, seen = self._queue_mode("merged")
        self.assertEqual(seen["counted"], [(285, wfc.DEFAULT_REPO)])
        self.assertEqual(seen["verdict"]["todays_merge_count"], 4)
        self.assertTrue(seen["unsubscribe"])

    def test_every_other_exit_but_probe_failed_dismisses_never_counts(self):
        """Unsubscribe only on merge: an exit that is not a merge marks the
        threads done and keeps the subscription, and never counts."""
        for conclusion in (
            "dequeued",
            "closed",
            "not_queued",
            "auto_merge_waiting",
            "queued_no_group",
            "timeout",
        ):
            with self.subTest(conclusion=conclusion):
                _, seen = self._queue_mode(conclusion)
                self.assertEqual(seen["dismissed"], [(285, wfc.DEFAULT_REPO)])
                self.assertIn("notifications", seen["verdict"])
                self.assertFalse(seen["unsubscribe"])
                self.assertNotIn("counted", seen)
                self.assertNotIn("todays_merge_count", seen["verdict"])
        _, seen = self._queue_mode("probe_failed")
        self.assertNotIn("dismissed", seen)
        self.assertNotIn("counted", seen)
        self.assertNotIn("notifications", seen["verdict"])

    def test_a_crash_never_dismisses(self):
        """Like `probe_failed`, a crash never learned the PR's state."""
        dismissed = []
        for name, value in (
            ("wait_queue", lambda *a, **k: (_ for _ in ()).throw(OSError("disk"))),
            ("dismiss_pr_notifications", lambda pr, repo, merged: dismissed.append(pr)),
            ("default_since", lambda pr, repo: (None, None)),
        ):
            self.addCleanup(setattr, wfc, name, getattr(wfc, name))
            setattr(wfc, name, value)
        with self.assertRaises(OSError):
            wfc.watch_queue(285, wfc.DEFAULT_REPO, 20, 60, None)
        self.assertEqual(dismissed, [])

    def test_a_sigterm_mid_wait_still_dismisses(self):
        """The session stops the watch early: the signal raises into the one
        `finally`, which dismisses, and a `stopped` verdict prints."""
        state = {}

        def stopped_mid_wait(pr, **_):
            state["handler"] = signal.getsignal(signal.SIGTERM)
            os.kill(os.getpid(), signal.SIGTERM)
            for _ in range(1000):  # the handler runs between instructions
                time.sleep(0.001)
            raise AssertionError("SIGTERM never raised into the watch")

        before = signal.getsignal(signal.SIGTERM)
        real = wfc.wait_queue
        wfc.wait_queue = stopped_mid_wait
        self.addCleanup(setattr, wfc, "wait_queue", real)
        code, seen = self._queue_mode("unused", stub=False)
        self.assertEqual(code, 1)
        self.assertEqual(seen["verdict"]["conclusion"], "stopped")
        self.assertEqual(seen["verdict"]["signal"], "SIGTERM")
        self.assertEqual(seen["dismissed"], [(285, wfc.DEFAULT_REPO)])
        self.assertEqual(seen["verdict"]["notifications"]["dismissed"], 1)
        # A stopped PR is still open: its subscription is kept, nothing counted.
        self.assertFalse(seen["unsubscribe"])
        self.assertNotIn("counted", seen)
        self.assertIs(state["handler"], wfc._raise_stopped)
        self.assertIs(signal.getsignal(signal.SIGTERM), before)

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

    def test_queue_mode_takes_the_poll_knob_and_its_own_timeout(self):
        real = os.environ.pop(wfc.POLL_ENV, None)
        if real is not None:
            self.addCleanup(os.environ.__setitem__, wfc.POLL_ENV, real)
        code, seen = self._queue_mode("merged")
        self.assertEqual(code, 0)
        self.assertEqual(seen["pr"], 285)
        self.assertEqual(seen["interval"], wfc.DEFAULT_POLL)
        self.assertEqual(seen["timeout"], wfc.QUEUE_TIMEOUT)
        os.environ[wfc.POLL_ENV] = "25"
        self.addCleanup(os.environ.pop, wfc.POLL_ENV, None)
        _, seen = self._queue_mode("merged")
        self.assertEqual(seen["interval"], 25)

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

    def test_a_merge_prints_one_line_and_files_the_full_verdict(self):
        code, seen = self._queue_mode("merged")
        self.assertEqual(code, 0)
        lines = seen["stdout"].splitlines()
        self.assertEqual(len(lines), 1)
        self.assertIn("merged: PR #285", lines[0])
        self.assertIn("Today's merge count is now 4.", lines[0])
        self.assertIn("notifications dismissed: 1", lines[0])
        self.assertEqual(seen["verdict"]["todays_merge_count"], 4)

    def test_every_other_exit_still_prints_the_json_verdict(self):
        _, seen = self._queue_mode("dequeued")
        self.assertEqual(json.loads(seen["stdout"]), seen["verdict"])

    def test_a_failed_count_or_dismissal_says_so_in_the_line(self):
        line = wfc.merged_line(
            {"pr": 9, "todays_merge_count": None,
             "notifications": {"dismissed": 0, "error": "403"}}
        )  # fmt: skip
        self.assertIn("unavailable", line)
        self.assertIn("dismissal failed: 403", line)

    def test_since_defaults_to_the_prs_own_enqueue_time(self):
        _, seen = self._queue_mode("merged", default_since=(ENQUEUED, "auto_merge"))
        self.assertEqual(seen["defaulted"], [285])
        self.assertEqual(seen["since"], ENQUEUED)
        self.assertEqual(seen["verdict"]["since_source"], "auto_merge")

    def test_an_explicit_since_is_never_overridden(self):
        _, seen = self._queue_mode("merged", ["--since", ENQUEUED])
        self.assertNotIn("defaulted", seen)
        self.assertEqual(seen["verdict"]["since_source"], "flag")

    def test_a_dequeue_with_failed_runs_is_classified(self):
        runs = [{"run_id": "1", "conclusion": "failure"}]
        _, seen = self._queue_mode("dequeued", runs=runs)
        self.assertEqual(seen["classified"], [runs])
        self.assertEqual(
            seen["verdict"]["dequeue"]["classification"]["cause"], "transient"
        )

    def test_a_not_queued_beside_a_failed_earlier_cycle_is_classified(self):
        """A firing that began after a dequeue reads `not_queued`: the earlier
        cycle's failures are classified, so the re-enqueue is never blind."""
        failed = [{"run_id": "9", "conclusion": "failure"}]
        _, seen = self._queue_mode(
            "not_queued", also={"not_queued": {"prior_failing_runs": failed}}
        )
        self.assertEqual(seen["classified"], [failed])
        self.assertEqual(
            seen["verdict"]["not_queued"]["classification"]["cause"], "transient"
        )
        _, seen = self._queue_mode(
            "not_queued", also={"not_queued": {"prior_failing_runs": []}}
        )
        self.assertNotIn("classified", seen)
        self.assertNotIn("classification", seen["verdict"]["not_queued"])

    def test_a_merge_with_verdict_out_writes_both_and_names_the_queue_file(self):
        out = Path(tempfile.mkdtemp()) / "ci.json"
        self.addCleanup(lambda: out.unlink(missing_ok=True))
        _, seen = self._queue_mode("merged", ["--verdict-out", str(out)])
        self.assertEqual(json.loads(out.read_text()), seen["verdict"])
        self.assertNotIn(str(out), seen["stdout"])
        self.assertIn("verdict.json", seen["stdout"])

    def test_a_no_run_dequeue_is_unknown_with_the_timelines_reason(self):
        _, seen = self._queue_mode("dequeued")
        self.assertNotIn("classified", seen)
        classification = seen["verdict"]["dequeue"]["classification"]
        self.assertEqual(classification["cause"], "unknown")
        self.assertEqual(classification["reason"], "merge_conflict")

    def test_a_conflicting_verdict_is_not_classified(self):
        _, seen = self._queue_mode("dequeued", mergeable="CONFLICTING")
        self.assertNotIn("classification", seen["verdict"]["dequeue"])

    def test_queue_combined_with_another_mode_is_refused(self):
        with self.assertRaises(wfc.WaitForChecksError):
            with redirect_stdout(io.StringIO()):
                wfc.run(["wait_for_checks.py", "--queue", "285", "--pr", "285"])

    def test_no_watch_is_refused_with_queue(self):
        with self.assertRaises(wfc.WaitForChecksError):
            with redirect_stdout(io.StringIO()):
                wfc.run(["wait_for_checks.py", "--queue", "285", "--no-watch"])


REPO = wfc.DEFAULT_REPO


class DefaultSinceTests(unittest.TestCase):
    def _since(self, node):
        def gql(pr, repo):
            if isinstance(node, Exception):
                raise node
            return node

        with mock.patch.object(wfc, "_queue_graphql", gql):
            return wfc.default_since(285, REPO)

    def test_the_newer_of_the_auto_merge_and_entry_times(self):
        """An auto-merge request that survived an earlier dequeue is older
        than this cycle's entry; anchoring on it would admit the old removal."""
        later = "2026-10-13T12:05:00Z"
        for node, expected in (
            ({"auto": {"enabledAt": ENQUEUED}, "entry": {"enqueuedAt": later}},
             (later, "queue_entry")),
            ({"auto": {"enabledAt": later}, "entry": {"enqueuedAt": ENQUEUED}},
             (later, "auto_merge")),
            ({"auto": None, "entry": {"enqueuedAt": later}}, (later, "queue_entry")),
            ({"auto": {"enabledAt": ENQUEUED}, "entry": None},
             (ENQUEUED, "auto_merge")),
        ):  # fmt: skip
            with self.subTest(node=node):
                self.assertEqual(self._since(node), expected)

    def test_neither_or_a_failed_read_leaves_the_watch_its_own_start(self):
        self.assertEqual(self._since({"auto": None, "entry": None}), (None, None))
        self.assertEqual(
            self._since(wfc.WaitForChecksError("graphql down")), (None, None)
        )


def jobs_payload(*jobs):
    return {"jobs": list(jobs)}


def failed_job(name, step, conclusion="failure"):
    return {
        "name": name,
        "conclusion": conclusion,
        "steps": [
            {"name": "Set up job", "conclusion": "success"},
            {"name": step, "conclusion": "failure"},
        ]
        if step
        else [],
    }


class ClassifyDequeueTests(unittest.TestCase):
    """The queue branch's failed job steps decide the dequeue's cause."""

    def _classify(self, payloads, runs=None):
        runs = runs or [{"run_id": str(i), "conclusion": "failure"}
                        for i in range(len(payloads))]  # fmt: skip
        replies = iter(payloads)

        def fetch(args):
            reply = next(replies)
            return (None, reply) if isinstance(reply, str) else (reply, None)

        with mock.patch.object(wfc, "_fetch_json", fetch):
            return wfc.classify_dequeue(runs, REPO)

    def test_a_toolchain_failure_before_any_test_is_transient(self):
        c = self._classify(
            [
                jobs_payload(
                    failed_job("Rust", "Run ./.github/actions/install-toolchain")
                )
            ]
        )
        self.assertEqual(c["cause"], "transient")
        self.assertEqual(
            c["evidence"][0]["step"], "Run ./.github/actions/install-toolchain"
        )
        self.assertIn("gh run view 0 --repo", c["next_command"])

    def test_a_failing_test_or_build_step_is_integration(self):
        for step in (
            "Run cargo nextest run",
            "Run make program",
            "Run make tools-tests",
        ):
            with self.subTest(step=step):
                c = self._classify([jobs_payload(failed_job("Rust", step))])
                self.assertEqual(c["cause"], "integration")

    def test_integration_anywhere_outranks_a_transient_sibling(self):
        c = self._classify(
            [
                jobs_payload(failed_job("SDK", "Run actions/cache@v5")),
                jobs_payload(failed_job("Rust", "Run cargo nextest run")),
            ]
        )
        self.assertEqual(c["cause"], "integration")

    def test_a_cancelled_sibling_with_no_failed_step_does_not_dilute(self):
        c = self._classify(
            [
                jobs_payload(
                    failed_job("Rust", "Run actions/setup-node@v6"),
                    failed_job("SDK", None, conclusion="cancelled"),
                )
            ]
        )
        self.assertEqual(c["cause"], "transient")
        self.assertEqual(len(c["evidence"]), 2)

    def test_a_locked_install_is_integration_not_setup_noise(self):
        """The base moved a manifest or lockfile: re-enqueueing repeats it."""
        for step in ("Run pnpm install --frozen-lockfile", "Run cargo fetch --locked"):
            with self.subTest(step=step):
                c = self._classify([jobs_payload(failed_job("SDK", step))])
                self.assertEqual(c["cause"], "integration")
        c = self._classify([jobs_payload(failed_job("SDK", "Run pnpm install"))])
        self.assertEqual(c["cause"], "transient")

    def test_an_unread_run_holds_a_transient_sibling_at_unknown(self):
        """The unread run may be the integration failure: never re-enqueue
        on the half that could be read."""
        c = self._classify(
            [jobs_payload(failed_job("SDK", "Run actions/cache@v5")), "empty stdout"]
        )
        self.assertEqual(c["cause"], "unknown")
        self.assertEqual(len(c["evidence"]), 2)

    def test_a_startup_failure_needs_no_read(self):
        c = self._classify([], runs=[{"run_id": "5", "conclusion": "startup_failure"}])
        self.assertEqual(c["cause"], "transient")

    def test_an_unnamed_step_or_a_failed_read_is_unknown(self):
        self.assertEqual(
            self._classify([jobs_payload(failed_job("Rust", "Mystery"))])["cause"],
            "unknown",
        )
        c = self._classify(["empty stdout"])
        self.assertEqual(c["cause"], "unknown")
        self.assertEqual(c["evidence"][0]["error"], "empty stdout")


class EnqueueTests(unittest.TestCase):
    """``--enqueue``: refuse a moved diff, enqueue the way the rules want,
    and probe that the queue took it."""

    HEAD = "c" * 40

    def _enqueue(
        self,
        probes,
        queue_rule=True,
        merge_exits=(0, 0),
        local=HEAD,
        fingerprint="fp1",
        reviewed="fp1",
        pull=None,
    ):
        seen = {"commands": [], "sleeps": 0}
        pull = pull or {"head": {"sha": self.HEAD}, "base": {"ref": "main"}}
        exits = iter(merge_exits)

        def fingerprint_of(base):
            if isinstance(fingerprint, Exception):
                raise fingerprint
            return fingerprint

        replies = iter(probes)

        def gh(args):
            seen["commands"].append(args)
            return next(exits), "", ""

        def probe(pr, repo):
            item = next(replies, probes[-1] if probes else {})
            if isinstance(item, Exception):
                raise item
            return item

        def sleep(_):
            seen["sleeps"] += 1

        with mock.patch.object(wfc, "_gh_json", lambda args, what: pull), \
                mock.patch.object(wfc, "_git", lambda args: (0, local.encode(), "")), \
                mock.patch.object(wfc.review_diff, "diff_fingerprint",
                                  fingerprint_of), \
                mock.patch.object(wfc, "merge_queue_active",
                                  lambda repo, base: queue_rule), \
                mock.patch.object(wfc, "_gh", gh):  # fmt: skip
            verdict = wfc.enqueue(285, REPO, reviewed, probe=probe, sleep=sleep)
        return verdict, seen

    def test_a_queue_rule_enqueues_with_the_plain_merge_first(self):
        v, seen = self._enqueue([{}, {"entry": dict(QUEUED, position=3)}])
        self.assertEqual(v["conclusion"], "queued")
        self.assertEqual(seen["commands"], [["pr", "merge", "285", "--repo", REPO]])
        self.assertEqual(v["entry"]["position"], 3)
        self.assertEqual(v["enqueued_at"], ENQUEUED)

    def test_a_failed_plain_merge_falls_back_to_auto(self):
        v, seen = self._enqueue([{"entry": QUEUED}], merge_exits=(1, 0))
        self.assertEqual(v["conclusion"], "queued")
        self.assertEqual(seen["commands"][-1][-1], "--auto")
        self.assertEqual([a["exit"] for a in v["attempts"]], [1, 0])

    def test_auto_merge_with_no_entry_under_a_queue_rule_is_not_proof(self):
        """The exit-0-never-queued shape: auto-merge on, nothing in the queue.
        The plain merge took nothing either, so `--auto` is tried, and the
        verdict says the queue has not taken it — never `queued`."""
        auto = {"auto": {"enabledAt": ENQUEUED}, "entry": None}
        v, seen = self._enqueue([auto])
        self.assertEqual(len(seen["commands"]), 2)
        self.assertEqual(v["conclusion"], "auto_merge_pending")
        self.assertEqual(seen["sleeps"], 2 * (wfc.ENQUEUE_PROBES - 1))

    def test_no_queue_rule_enables_auto_merge_alone(self):
        v, seen = self._enqueue(
            [{"auto": {"enabledAt": ENQUEUED}}], queue_rule=False, merge_exits=(0,)
        )
        self.assertEqual(
            seen["commands"], [["pr", "merge", "285", "--repo", REPO, "--auto"]]
        )
        self.assertEqual(v["conclusion"], "auto_merge_pending")
        self.assertEqual(v["enqueued_at"], ENQUEUED)

    def test_an_unreadable_rule_never_risks_the_plain_merge(self):
        _, seen = self._enqueue([{"entry": QUEUED}], queue_rule=None, merge_exits=(0,))
        self.assertEqual(seen["commands"][0][-1], "--auto")

    def test_nothing_taking_is_not_queued_and_every_failure_is_enqueue_failed(self):
        v, _ = self._enqueue([{}])
        self.assertEqual(v["conclusion"], "not_queued")
        v, _ = self._enqueue([{}], merge_exits=(1, 1))
        self.assertEqual(v["conclusion"], "enqueue_failed")

    def test_an_attempt_no_probe_answered_is_a_probe_failure_not_absence(self):
        down = wfc.WaitForChecksError("graphql down")
        v, _ = self._enqueue([down])
        self.assertEqual(v["conclusion"], "probe_failed")
        # One good read anywhere is an answer.
        v, _ = self._enqueue([down, down, down, {}, {}])
        self.assertEqual(v["conclusion"], "not_queued")

    def test_an_unreadable_pull_or_fingerprint_raises_before_any_write(self):
        for kwargs in (
            {"pull": {"head": {}, "base": {"ref": "main"}}},
            {"fingerprint": wfc.review_diff.ReviewDiffError("git diff failed")},
        ):
            with self.subTest(kwargs=list(kwargs)):
                with self.assertRaises(wfc.WaitForChecksError):
                    self._enqueue([{"entry": QUEUED}], **kwargs)

    def test_the_queue_rule_read(self):
        for reply, expected in (
            ([{"type": "pull_request"}, {"type": "merge_queue"}], True),
            ([{"type": "pull_request"}, "junk"], False),
            ({"message": "Not Found"}, None),
            (wfc.WaitForChecksError("403"), None),
        ):  # fmt: skip
            with self.subTest(reply=reply):

                def read(args, what, reply=reply):
                    if isinstance(reply, Exception):
                        raise reply
                    return reply

                with mock.patch.object(wfc, "_gh_json", read):
                    self.assertIs(wfc.merge_queue_active(REPO, "main"), expected)

    def test_a_merge_the_probe_sees_is_merged(self):
        v, _ = self._enqueue([{"gql_merged": True}])
        self.assertEqual(v["conclusion"], "merged")

    def test_a_moved_diff_is_refused_before_any_write(self):
        v, seen = self._enqueue([{"entry": QUEUED}], fingerprint="fp2")
        self.assertEqual(v["conclusion"], "refused")
        self.assertIn("scoped re-review", v["error"])
        self.assertEqual(seen["commands"], [])

    def test_an_unpushed_head_is_refused_before_any_write(self):
        v, seen = self._enqueue([{"entry": QUEUED}], local="d" * 40)
        self.assertEqual(v["conclusion"], "refused")
        self.assertIn("push", v["error"])
        self.assertEqual(seen["commands"], [])

    def test_the_cli_pairs_its_flags_and_exits_on_the_verdict(self):
        for argv in (
            ["--enqueue", "285"],
            ["--reviewed-fingerprint", "fp1", "--pr", "285"],
            ["--enqueue", "285", "--reviewed-fingerprint", "fp1", "--timeout", "9"],
        ):
            with self.subTest(argv=argv):
                with self.assertRaises(wfc.WaitForChecksError):
                    with redirect_stdout(io.StringIO()):
                        wfc.run(["wait_for_checks.py", *argv])
        argv = [
            "wait_for_checks.py",
            "--enqueue",
            "285",
            "--reviewed-fingerprint",
            "fp1",
        ]
        for conclusion, code in (("queued", 0), ("auto_merge_pending", 0),
                                 ("refused", 1), ("not_queued", 1)):  # fmt: skip
            with self.subTest(conclusion=conclusion):
                with (
                    mock.patch.object(
                        wfc, "enqueue", lambda pr, repo, fp: {"conclusion": conclusion}
                    ),
                    redirect_stdout(io.StringIO()),
                ):
                    self.assertEqual(wfc.run(argv), code)


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

    def _run(self, list_reply, delete_code=0, merged=True):
        """Through the dismissal tool's `--pr` mode, its gh stubbed."""
        calls = []

        def fake(args):
            calls.append(args)
            if args[:2] == ["api", "-X"]:
                return delete_code, "", "HTTP 404" if delete_code else ""
            return list_reply

        # The shared tool owns the gh calls; the watcher's own `_gh` is unused.
        with mock.patch.object(wfc.pr_notifications, "real_gh", fake):
            return wfc.dismiss_pr_notifications(285, REPO, merged=merged), calls

    def test_only_this_prs_threads_are_unsubscribed_and_marked_done(self):
        result, calls = self._run((0, json.dumps(self.THREADS), ""))
        self.assertIn(f"/repos/{REPO}/notifications?", calls[0][1])
        writes = [c[2:4] for c in calls if c[:2] == ["api", "-X"]]
        self.assertEqual(
            writes,
            [
                ["PUT", "/notifications/threads/11/subscription"],
                ["DELETE", "/notifications/threads/11"],
                ["PUT", "/notifications/threads/14/subscription"],
                ["DELETE", "/notifications/threads/14"],
            ],
        )
        self.assertEqual(
            result, {"dismissed": 2, "unsubscribed": 2, "failed": 0, "error": None}
        )

    def test_a_failed_list_is_reported_not_raised(self):
        result, calls = self._run((1, "", "HTTP 401"))
        self.assertEqual(len(calls), 1)
        self.assertEqual(result["dismissed"], 0)
        self.assertIn("HTTP 401", result["error"])

    def test_a_failed_delete_is_counted(self):
        result, _ = self._run((0, json.dumps(self.THREADS), ""), delete_code=1)
        self.assertEqual(
            result,
            {"dismissed": 0, "unsubscribed": 0, "failed": 4, "error": "HTTP 404"},
        )

    def test_an_exit_that_is_not_a_merge_keeps_the_subscription(self):
        """Unsubscribe only on merge: the PR is still open, and its
        subscription is what re-creates its thread on new activity."""
        result, calls = self._run((0, json.dumps(self.THREADS), ""), merged=False)
        writes = [c[2:4] for c in calls if c[:2] == ["api", "-X"]]
        self.assertEqual(
            writes,
            [
                ["DELETE", "/notifications/threads/11"],
                ["DELETE", "/notifications/threads/14"],
            ],
        )
        self.assertEqual(
            result, {"dismissed": 2, "unsubscribed": 0, "failed": 0, "error": None}
        )

    def test_a_raising_tool_is_reported_not_raised(self):
        with mock.patch.object(
            wfc.pr_notifications, "dismiss", side_effect=RuntimeError("boom")
        ):
            result = wfc.dismiss_pr_notifications(285, REPO)
        self.assertEqual(result["dismissed"], 0)
        self.assertIn("RuntimeError: boom", result["error"])


class TodaysMergeCountTests(unittest.TestCase):
    # 09:30 in UTC+10, so local midnight is 14:00 UTC the day before.
    NOW = datetime(2026, 10, 29, 9, 30, tzinfo=timezone(timedelta(hours=10)))

    def _run(self, reply):
        calls = []

        def fake(args):
            calls.append(args)
            return reply

        real = wfc._gh
        wfc._gh = fake
        self.addCleanup(setattr, wfc, "_gh", real)
        return wfc.todays_merge_count(285, REPO, now=self.NOW), calls

    def test_the_window_opens_at_local_midnight_with_its_offset(self):
        self.assertEqual(wfc.local_midnight(self.NOW), "2026-10-29T00:00:00+10:00")
        _, calls = self._run((0, "[]", ""))
        args = calls[0]
        self.assertEqual(
            args[args.index("--search") + 1], "merged:>=2026-10-29T00:00:00+10:00"
        )
        self.assertEqual(args[args.index("--state") + 1], "merged")
        # `gh pr list` stops at 30 by default; a busy day must not.
        self.assertGreater(int(args[args.index("--limit") + 1]), 30)

    def test_the_just_merged_pr_counts_once_whether_indexed_or_not(self):
        listed = json.dumps([{"number": 280}, {"number": 285}])
        self.assertEqual(self._run((0, listed, ""))[0], 2)
        self.assertEqual(self._run((0, json.dumps([{"number": 280}]), ""))[0], 2)

    def test_a_failed_read_is_none_not_raised(self):
        self.assertIsNone(self._run((1, "", "HTTP 502"))[0])
        self.assertIsNone(self._run((0, "{", ""))[0])

    def test_a_wrong_shaped_reply_is_none_not_raised(self):
        for body in ('[{"num": 1}]', "[1]", '[{"number": "x"}]'):
            with self.subTest(body=body):
                self.assertIsNone(self._run((0, body, ""))[0])

    def test_the_default_midnight_takes_the_zones_own_offset_for_midnight(self):
        # Naive local midnight, localized afterwards: on a DST-change day the
        # offset is midnight's, not the current hour's.
        got = datetime.fromisoformat(wfc.local_midnight())
        expected = datetime.now().replace(hour=0, minute=0, second=0, microsecond=0)
        self.assertEqual(got, expected.astimezone())
        self.assertEqual(got.utcoffset(), expected.astimezone().utcoffset())


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
            "rest_state": "OPEN",
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


CONFLICT_READ = {
    "mergeable": "CONFLICTING",
    "merge_state_status": "DIRTY",
    "head": "a" * 40,
    "base": "main",
    "error": None,
}
UNKNOWN_READ = {
    **CONFLICT_READ,
    "mergeable": "UNKNOWN",
    "merge_state_status": "UNKNOWN",
}


class ConflictWhileQueuedTests(unittest.TestCase):
    """A live queue entry that conflicts concludes ``conflicting`` before
    GitHub removes it — the shape one PR sat in, waiting for that removal."""

    # The queue harness, borrowed rather than inherited (which would re-run
    # every QueueWatchTests case under this class's name).
    setUp = QueueWatchTests.setUp
    _watch = QueueWatchTests._watch

    def _settle_with(self, *reads):
        """Stub the settle loop to answer ``reads`` in turn (the last repeats)."""
        self.settles = []

        def settle(pr, repo, retries, delay, sleep=None):
            self.settles.append((retries, delay))
            return dict(reads[min(len(self.settles) - 1, len(reads) - 1)]), 1

        real = wfc.settle_mergeability
        wfc.settle_mergeability = settle
        self.addCleanup(setattr, wfc, "settle_mergeability", real)

    def test_a_live_entry_that_conflicts_concludes_conflicting(self):
        self._settle_with(CONFLICT_READ)
        dirty = {**snap(entry=QUEUED), "mergeable": "CONFLICTING",
                 "merge_state_status": "DIRTY"}  # fmt: skip
        v = self._watch([snap(entry=QUEUED), dirty, snap(gql="MERGED", merged=True)])
        self.assertEqual(v["conclusion"], "conflicting")
        self.assertEqual(v["polls"], 2)
        self.assertEqual(v["mergeable"], "CONFLICTING")
        self.assertEqual(v["conflict_read"]["head"], "a" * 40)
        # The gate's settle loop, not the cause probe's shorter one.
        self.assertEqual(
            self.settles, [(wfc.MERGEABLE_SETTLE_RETRIES, wfc.MERGEABLE_SETTLE_DELAY)]
        )

    def test_an_unsettled_UNKNOWN_read_is_never_a_verdict(self):
        """GraphQL said DIRTY, but the settle loop ran out on UNKNOWN: wait on."""
        self._settle_with(UNKNOWN_READ)
        dirty = {**snap(entry=QUEUED), "merge_state_status": "DIRTY"}
        v = self._watch([dirty, snap(gql="MERGED", merged=True)])
        self.assertEqual(v["conclusion"], "merged")
        self.assertEqual(len(self.settles), 1)
        self.assertNotIn("conflict_read", v)

    def test_a_reused_graphql_read_is_settled_once_not_every_poll(self):
        self._settle_with(UNKNOWN_READ)
        dirty = {**snap(entry=QUEUED), "mergeable": "CONFLICTING"}
        stale = {**dirty, "slow_fresh": False}
        v = self._watch([dirty, stale, stale, snap(gql="MERGED", merged=True)])
        self.assertEqual(v["conclusion"], "merged")
        self.assertEqual(len(self.settles), 1)

    def test_no_entry_means_no_conflict_read(self):
        """Not queued yet (auto-merge waiting): the gate's own read owns it."""
        self._settle_with(CONFLICT_READ)
        dirty = {**snap(auto={"enabledAt": ENQUEUED}), "mergeable": "CONFLICTING"}
        self._watch([dirty, snap(gql="MERGED", merged=True)])
        self.assertEqual(self.settles, [])

    def test_watch_queue_fills_base_gained_with_a_null_reason(self):
        """Step 22's conflicting branch reads `dequeue.base_gained` either way."""
        verdict = {
            "pr": 285,
            "conclusion": "conflicting",
            "conflict_read": CONFLICT_READ,
        }
        gains = []
        gain = {"base_gained": [{"sha": "8f285869abcd", "subject": "feat: x"}],
                "base_gained_total": 1, "error": None}  # fmt: skip
        with (
            mock.patch.object(wfc, "wait_queue", return_value=dict(verdict)),
            mock.patch.object(
                wfc, "base_gain", side_effect=lambda *a: gains.append(a) or gain
            ),
            mock.patch.object(wfc, "dismiss_pr_notifications", return_value={}),
        ):
            v = wfc.watch_queue(285, "o/r", 60, 7200, since=ENQUEUED)
        self.assertEqual(gains, [("o/r", "a" * 40, "main")])
        self.assertIsNone(v["dequeue"]["dequeue_reason"])
        self.assertEqual(v["dequeue"]["base_gained_total"], 1)
        self.assertNotIn("conflict_read", v)


class ConflictDuringCiWaitTests(unittest.TestCase):
    """A base move under a PR-mode wait: the watch's pull read sees `dirty`,
    ends early, and the verdict settles it before saying `conflicting`."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        real_log = wfc.log_path_for
        wfc.log_path_for = lambda pr: Path(self._tmp.name) / f"w-{pr}.log"
        self.addCleanup(setattr, wfc, "log_path_for", real_log)

    def _wait(self, watches, checks, view):
        """``watches`` / ``checks``: what each round's watch and check read
        return (the last repeats; ``checks`` may be one list for every round);
        ``view``: the ``gh pr view`` payload; a compare names one commit."""
        reads = checks if checks and isinstance(checks[0], list) else [checks]
        self.calls = []

        def fake_gh(args):
            self.calls.append(args)
            if args[0] == "pr":
                return 0, json.dumps(view), ""
            commit = {"sha": "8f285869abcdef", "commit": {"message": "feat: x\n\nbody"}}
            return 0, json.dumps({"ahead_by": 1, "commits": [commit]}), ""

        rounds = []

        def fake_watch(pr, repo, interval, timeout, log):
            rounds.append(1)
            return watches[min(len(rounds) - 1, len(watches) - 1)]

        def fake_read(pr, repo):
            return reads[min(len(rounds) - 1, len(reads) - 1)]

        with (
            mock.patch.object(wfc, "watch_checks", fake_watch),
            mock.patch.object(wfc, "read_checks", fake_read),
            mock.patch.object(wfc, "_gh", fake_gh),
        ):
            return wfc.wait(285, repo="o/r", sleep=lambda _s: None)

    def test_a_dirty_read_mid_wait_concludes_conflicting_with_the_base_gain(self):
        view = {"mergeable": "CONFLICTING", "mergeStateStatus": "DIRTY",
                "headRefOid": "a" * 40, "baseRefName": "main"}  # fmt: skip
        v = self._wait([wfc.CONFLICT_SEEN], [check("Tests", "pending")], view)
        self.assertEqual(v["conclusion"], "conflicting")
        self.assertTrue(v["blocked_by_conflict"])
        self.assertEqual(v["watch_rounds"], 1)
        self.assertEqual(
            v["base_gained"], [{"sha": "8f285869abcd", "subject": "feat: x"}]
        )
        self.assertEqual(v["base_gained_total"], 1)
        self.assertEqual(v["hint"]["gate"], "blocking")
        self.assertIn("compare/" + "a" * 40 + "...main", self.calls[-1][1])

    def test_an_unconfirmed_dirty_read_waits_on_without_spending_the_budget(self):
        """The settle read disagrees: no verdict from the REST field alone, and
        the cut-short rounds (more than the pending cap) are no CI run, so the
        wait reaches the real `pass` rather than a `timeout`."""
        view = {"mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN"}
        cut = wfc.MAX_WATCH_ROUNDS + 1
        v = self._wait(
            [wfc.CONFLICT_SEEN] * cut + [True],
            [[check("Tests", "pending")]] * cut + [[check("Tests", "pass")]],
            view,
        )
        self.assertEqual(v["conclusion"], "pass")
        self.assertEqual(v["watch_rounds"], cut + 1)
        self.assertIsNone(v["base_gained"])

    def test_uncounted_pending_rounds_still_bar_a_green_none(self):
        """Checks seen pending, then gone, never read as `none` — even when
        the pending rounds were cut short and spent no budget."""
        view = {"mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN"}
        v = self._wait(
            [wfc.CONFLICT_SEEN, True],
            [[check("Tests", "pending")], []],
            view,
        )
        self.assertNotEqual(v["conclusion"], "none")

    def test_base_gain_without_a_ref_says_so_rather_than_raising(self):
        gain = wfc.base_gain("o/r", None, "main")
        self.assertIsNone(gain["base_gained"])
        self.assertIn("no head SHA", gain["error"])

    def test_the_queue_probe_marks_a_reused_graphql_read_stale(self):
        """`slow_fresh` is what keeps a conflict settled once per read."""

        class Client:
            def get(self, path):
                return [] if "/timeline" in path else {"state": "open"}

        now = [0.0]
        with (
            mock.patch.object(wfc, "_queue_graphql", lambda pr, repo: {}),
            mock.patch.object(wfc, "_queue_runs", lambda client, repo, pr: []),
        ):
            probe = wfc.QueueProbe(client=Client(), clock=lambda: now[0])
            fresh = [probe(285, "o/r")["slow_fresh"]]
            fresh.append(probe(285, "o/r")["slow_fresh"])
            now[0] += wfc.CORROBORATE_SECONDS
            fresh.append(probe(285, "o/r")["slow_fresh"])
        self.assertEqual(fresh, [True, False, True])

    def test_the_watch_returns_at_once_on_a_dirty_pull(self):
        log = Path(self._tmp.name) / "watch.log"
        sleeps = []
        gh = RecordedGh(
            {
                "pull": [pull(), {**pull(), "mergeable_state": "dirty"}],
                "check_runs": [check_runs("in_progress")],
            }
        )
        got = wfc.watch_checks(
            285, "o/r", 20, 3600, log, client=wfc.Conditional(gh),
            sleep=sleeps.append, clock=lambda: float(len(sleeps) * 20),
        )  # fmt: skip
        self.assertEqual(got, wfc.CONFLICT_SEEN)
        self.assertEqual(sleeps, [20])
        last = json.loads(log.read_text().splitlines()[-1])
        self.assertEqual(last["mergeable_state"], "dirty")


class QueueStateShapeTests(unittest.TestCase):
    """Step 20's overlap read and the queue watch read one shape."""

    def test_every_queue_state_field_is_in_the_watch_query(self):
        path = Path(wfc.__file__).with_name("queue_state.graphql")
        lines = [line.split("#")[0].strip() for line in path.read_text().splitlines()]
        fields = [
            line.split()[0].rstrip("{").strip()
            for line in lines
            if line
            and line[0].isalpha()
            and not line.startswith(("query", "repository", "pullRequest"))
        ]
        self.assertIn("mergeable", fields)
        self.assertIn("mergeStateStatus", fields)
        for field in fields:
            self.assertRegex(wfc.QUEUE_QUERY, rf"\b{field}\b")


if __name__ == "__main__":
    unittest.main()
