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
        wfc.watch_checks = lambda pr, repo, interval, timeout, log: settled
        wfc.read_checks = lambda pr, repo: checks
        self.addCleanup(setattr, wfc, "watch_checks", wfc_real_watch)
        self.addCleanup(setattr, wfc, "read_checks", wfc_real_read)

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


class CliTests(unittest.TestCase):
    def _stub(self, checks):
        wfc_real_watch = wfc.watch_checks
        wfc_real_read = wfc.read_checks
        wfc.watch_checks = lambda pr, repo, interval, timeout, log: True
        wfc.read_checks = lambda pr, repo: checks
        self.addCleanup(setattr, wfc, "watch_checks", wfc_real_watch)
        self.addCleanup(setattr, wfc, "read_checks", wfc_real_read)

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

    def test_run_mode_reaches_wait_run(self):
        real = wfc.wait_run
        wfc.wait_run = lambda rid, repo="r", timeout=0: {
            "conclusion": "pass",
            "run_id": rid,
        }
        try:
            buf = io.StringIO()
            with redirect_stdout(buf):
                code = wfc.run(["wait_for_checks.py", "--run", "99"])
        finally:
            wfc.wait_run = real
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(buf.getvalue())["run_id"], "99")


if __name__ == "__main__":
    unittest.main()
