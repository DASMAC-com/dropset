"""Stdlib ``unittest`` tests for the CI-wait tool.

The two ``gh`` invocations are stubbed — these cover the parts that decide an
outcome: the bucket→conclusion precedence, the failing-check extraction, the
empty-vs-error distinction in ``gh``'s overloaded exit codes, and the rule that a
timed-out watch never reports ``pass``. Run via the repo's ``make tools-tests``.
"""

from __future__ import annotations

import io
import json
import unittest
from contextlib import redirect_stdout

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

    def test_cancel_is_counted_but_not_red(self):
        out = wfc.summarize([check("a", "pass"), check("b", "cancel")])
        self.assertEqual(out["conclusion"], "pass")
        self.assertEqual(out["counts"]["cancel"], 1)

    def test_no_checks_is_none_not_pass(self):
        out = wfc.summarize([])
        self.assertEqual(out["conclusion"], "none")
        self.assertEqual(out["counts"], {})

    def test_unknown_bucket_is_counted_not_dropped(self):
        out = wfc.summarize([{"name": "x"}])
        self.assertEqual(out["counts"], {"unknown": 1})

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

    def test_exit_eight_with_no_payload_means_no_checks(self):
        self._stub(8, "")
        self.assertEqual(wfc.read_checks(285, "o/r"), [])

    def test_other_nonzero_with_no_payload_is_an_error(self):
        """A bad PR number or missing auth must not read as "no checks"."""
        self._stub(1, "", "could not find pull request")
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


if __name__ == "__main__":
    unittest.main()
