"""Stdlib ``unittest`` tests for the merge-queue CI-metrics reader.

The ``gh`` reads are not exercised; these cover the pure reductions over an
embedded fixture — the per-entry critical path, the slowest-job count, the
failed-job listing, the page merge — and the two log readers, including the
colored result word, a skipped hook's postfix, and the seven-digit Actions
timestamp. Run via the repo's ``make tools-tests``.
"""

from __future__ import annotations

import io
import json
import os
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest import mock

import ci_metrics as cm

Q = cm.QUEUE_PREFIX


def job(name, start, end, conclusion="success", steps=None):
    return {
        "name": name,
        "started_at": f"2026-10-20T{start}:00Z",
        "completed_at": f"2026-10-20T{end}:00Z",
        "conclusion": conclusion,
        "steps": steps or [],
    }


def step(name, start, end):
    return {
        "name": name,
        "started_at": f"2026-10-20T{start}:00Z",
        "completed_at": f"2026-10-20T{end}:00Z",
    }


# Three queue entries, each triggering a Tests run and a Lint run, plus one
# non-queue run that must be ignored:
#   pr-1: green, Tests 20m is the critical path.
#   pr-2: Lint failed.
#   pr-3: green, Tests 45m (a cache-moved slow run).
PAYLOAD = {
    "since": "2026-10-14T00:00:00Z",
    "runs": [
        {"id": 11, "head_branch": f"{Q}1-aaa", "conclusion": "success"},
        {"id": 12, "head_branch": f"{Q}1-aaa", "conclusion": "success"},
        {"id": 21, "head_branch": f"{Q}2-bbb", "conclusion": "success"},
        {"id": 22, "head_branch": f"{Q}2-bbb", "conclusion": "failure"},
        {"id": 31, "head_branch": f"{Q}3-ccc", "conclusion": "success"},
        {"id": 32, "head_branch": f"{Q}3-ccc", "conclusion": "success"},
        {"id": 99, "head_branch": "main", "conclusion": "success"},
    ],
    "jobs": {
        "11": [
            job(
                "Tests",
                "10:00",
                "10:20",
                steps=[
                    step("Restore cache", "10:00", "10:02"),
                    step("Run nextest", "10:02", "10:19"),
                ],
            )
        ],
        "12": [job("Lint", "10:01", "10:05")],
        "21": [job("Tests", "11:00", "11:12")],
        "22": [job("Lint", "11:00", "11:03", conclusion="failure")],
        "31": [
            job(
                "Tests",
                "12:00",
                "12:45",
                steps=[
                    step("Restore cache", "12:00", "12:01"),
                    step("Run nextest", "12:01", "12:44"),
                ],
            )
        ],
        "32": [job("Lint", "12:00", "12:04")],
        "99": [job("Tests", "09:00", "09:30")],
    },
}


class AnalyseTests(unittest.TestCase):
    def setUp(self):
        self.a = cm.analyse(PAYLOAD, steps_for="Tests")
        self.rows = {r["pr"]: r for r in self.a["rows"]}

    def test_non_queue_runs_are_ignored(self):
        self.assertEqual(sorted(self.rows), ["1", "2", "3"])
        self.assertEqual(len(self.a["job_durations"]["Tests"]), 3)

    def test_critical_path_spans_every_workflow(self):
        self.assertEqual(self.rows["1"]["wall_min"], 20.0)
        self.assertEqual(self.rows["2"]["wall_min"], 12.0)
        self.assertEqual(self.rows["3"]["wall_min"], 45.0)
        self.assertEqual(self.rows["1"]["runs"], 2)
        self.assertEqual(self.rows["1"]["weekday"], "Tue")

    def test_slowest_job_count(self):
        self.assertEqual([self.rows[pr]["slowest_job"] for pr in "123"], ["Tests"] * 3)
        out = cm.render(PAYLOAD, full=False)
        i = out.index("critical-path job per entry (slowest successful job):")
        self.assertEqual(out[i + 1].split(), ["Tests", "3"])

    def test_failed_job_listing(self):
        self.assertEqual(self.rows["2"]["failed_jobs"], ["Lint=failure"])
        self.assertEqual(self.rows["2"]["conclusions"], ["failure", "success"])
        self.assertEqual(self.rows["1"]["failed_jobs"], [])

    def test_step_durations_from_successful_named_job(self):
        self.assertEqual(self.a["step_durations"]["Run nextest"], [17.0, 43.0])
        self.assertNotIn("Lint", self.a["step_durations"])

    def test_render_summary(self):
        out = "\n".join(cm.render(PAYLOAD, full=False, steps_for="Tests"))
        self.assertIn("7 runs, 3 settled queue entries, 0 in flight", out)
        self.assertIn("entry critical path (green): n=2 median=32.5m", out)
        self.assertIn("entries not all-green: 1", out)
        self.assertIn("['Lint=failure']", out)
        self.assertIn("per-step duration inside successful 'Tests'", out)
        self.assertNotIn("every entry:", out)
        full = "\n".join(cm.render(PAYLOAD, full=True))
        self.assertIn("every entry:", full)

    def test_in_flight_entry_is_neither_settled_nor_failed(self):
        payload = json.loads(json.dumps(PAYLOAD))
        payload["runs"].append(
            {"id": 41, "head_branch": f"{Q}4-ddd", "conclusion": None}
        )
        payload["jobs"]["41"] = [job("Tests", "13:00", "13:01")]
        # pr-5: a job already failed but a run is still going, so its span is
        # partial — in flight, not not-green. pr-6: queued, no job started.
        payload["runs"] += [
            {"id": 51, "head_branch": f"{Q}5-eee", "conclusion": None},
            {"id": 52, "head_branch": f"{Q}5-eee", "conclusion": "failure"},
            {"id": 61, "head_branch": f"{Q}6-fff", "conclusion": None},
        ]
        payload["jobs"]["51"] = [job("Tests", "14:00", "14:01")]
        payload["jobs"]["52"] = [job("Lint", "14:00", "14:02", "failure")]
        payload["jobs"]["61"] = [
            {"name": "Tests", "started_at": None, "completed_at": None}
        ]
        out = "\n".join(cm.render(payload, full=False))
        self.assertIn("3 settled queue entries, 3 in flight", out)
        self.assertIn("entries not all-green: 1", out)
        self.assertIn("entry critical path (all):   n=3 ", out)
        self.assertIn(
            "in flight (excluded above): pr-4 pr-5['Lint=failure'] pr-6(unstarted)",
            out,
        )

    def test_a_settled_entry_with_no_started_job_is_not_in_flight(self):
        payload = json.loads(json.dumps(PAYLOAD))
        payload["runs"].append(
            {"id": 71, "head_branch": f"{Q}7-ggg", "conclusion": "startup_failure"}
        )
        payload["jobs"]["71"] = []
        out = "\n".join(cm.render(payload, full=False))
        self.assertIn("3 settled queue entries, 0 in flight", out)

    def test_a_wholly_skipped_run_keeps_the_entry_green(self):
        payload = json.loads(json.dumps(PAYLOAD))
        payload["runs"][1]["conclusion"] = "skipped"
        rows = {r["pr"]: r for r in cm.analyse(payload)["rows"]}
        self.assertTrue(cm.is_green(rows["1"]))
        self.assertFalse(cm.is_green(rows["2"]))
        out = "\n".join(cm.render(payload, full=False))
        self.assertIn("entry critical path (green): n=2 ", out)
        self.assertIn("entries not all-green: 1", out)


class MergePagesTests(unittest.TestCase):
    def test_named_key_not_first_list(self):
        pages = [
            {"total_count": 3, "extra": [0], "workflow_runs": [{"id": 1}]},
            {"total_count": 3, "workflow_runs": [{"id": 2}, {"id": 3}]},
        ]
        merged = cm.merge_pages(pages, "workflow_runs")
        self.assertEqual([r["id"] for r in merged], [1, 2, 3])


class StatsTests(unittest.TestCase):
    def test_empty(self):
        self.assertEqual(cm.stats([]), "n=0")

    def test_values(self):
        self.assertEqual(
            cm.stats([3.0, 1.0, 2.0]),
            "n=3 median=2.0m p90=3.0m min=1.0m max=3.0m",
        )


GH_LOG = [
    "Tests\tRun nextest\t2026-10-20T10:02:00.1234567Z     Running 120 tests",
    "Tests\tRun nextest\t2026-10-20T10:03:00.1234567Z         PASS [0.1s] ok",
    "Tests\tRun nextest\t2026-10-20T10:04:00.0000000Z "
    "        FAIL [   0.004s] dropset::tests bad_test",
    "Tests\tRun nextest\t2026-10-20T10:04:01.0000000Z "
    "thread 'bad_test' panicked at src/lib.rs:1",
    "Tests\tRun nextest\t2026-10-20T10:05:00.0000000Z ::error::Process exit 1",
    "Tests\tBuild\t2026-10-20T10:05:01.0000000Z error[E0425]: cannot find value",
]

LINT_LOG = [
    "2026-10-20T10:00:00.0000000Z cargo fmt...................\x1b[42mPassed",
    "2026-10-20T10:00:30.5000000Z clippy......................\x1b[42mPassed",
    "2026-10-20T10:03:00.5000000Z cspell......................^[[41mFailed",
    "2026-10-20T10:03:05.5000000Z yamllint....(no files to check)^[[46mSkipped",
    "2026-10-20T10:03:06.5000000Z biome.......................Passed",
    "2026-10-20T10:03:07.0000000Z some unrelated output line",
]


class LogReaderTests(unittest.TestCase):
    def test_strip_prefix(self):
        lines = cm.strip_log_prefix(GH_LOG)
        self.assertTrue(lines[0].startswith("2026-10-20T10:02:00"))

    def test_failed_lines(self):
        hits = cm.failed_lines(cm.strip_log_prefix(GH_LOG))
        self.assertEqual(len(hits), 4)
        self.assertIn("FAIL [   0.004s] dropset::tests bad_test", hits[0])
        self.assertIn("panicked at", hits[1])
        self.assertIn("::error::", hits[2])
        self.assertIn("error[E0425]", hits[3])

    def test_hook_timing_ordering_colors_and_postfix(self):
        rows = cm.hook_rows(LINT_LOG)
        # cargo fmt is first and has no prior stamp, so no row of its own.
        self.assertEqual(
            [(name, res) for _, name, res in rows],
            [
                ("cspell", "Failed"),
                ("clippy", "Passed"),
                ("yamllint", "Skipped"),
                ("biome", "Passed"),
            ],
        )
        self.assertAlmostEqual(rows[0][0], 2.5)
        self.assertAlmostEqual(rows[1][0], 30.5 / 60)
        self.assertAlmostEqual(rows[3][0], 1 / 60)

    def test_seven_digit_timestamp(self):
        ts = cm.parse_ts("2026-10-20T10:00:00.1234567Z")
        self.assertEqual(ts.microsecond, 123456)


class MainTests(unittest.TestCase):
    def test_cache_round_trip_skips_fetch(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "payload.json")
            with open(path, "w") as fh:
                json.dump(PAYLOAD, fh)
            buf = io.StringIO()
            with mock.patch.object(cm, "fetch") as fetch, redirect_stdout(buf):
                self.assertEqual(cm.main(["--cache", path]), 0)
            fetch.assert_not_called()
            self.assertIn("3 settled queue entries", buf.getvalue())

    def test_failed_log_mode_respects_limit(self):
        buf = io.StringIO()
        lines = cm.strip_log_prefix(GH_LOG)
        with mock.patch.object(cm, "job_log", return_value=lines):
            with redirect_stdout(buf):
                cm.main(["--failed-log", "7", "--limit", "1"])
        out = buf.getvalue()
        self.assertIn("job 7: 6 log lines, 4 failure-shaped lines; first 1:", out)
        self.assertNotIn("::error::", out)

    def test_hook_timing_mode(self):
        buf = io.StringIO()
        with mock.patch.object(cm, "job_log", return_value=LINT_LOG):
            with redirect_stdout(buf):
                self.assertEqual(cm.main(["--hook-timing", "8"]), 0)
        out = buf.getvalue().splitlines()
        self.assertIn("job 8: 4 hook results spanning 3.1m", out[0])
        self.assertEqual(out[1].split()[1:], ["cspell", "Failed"])

    def test_hook_timing_with_no_matching_line_fails_loudly(self):
        err = io.StringIO()
        lines = cm.strip_log_prefix(GH_LOG)
        with mock.patch.object(cm, "job_log", return_value=lines):
            with redirect_stdout(io.StringIO()), redirect_stderr(err):
                self.assertEqual(cm.main(["--hook-timing", "9"]), 1)
        self.assertIn("no pre-commit result line matched", err.getvalue())

    def test_cache_written_when_absent(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "payload.json")
            with mock.patch.object(cm, "fetch", return_value=PAYLOAD):
                with redirect_stdout(io.StringIO()):
                    cm.main(["--cache", path])
            with open(path) as fh:
                self.assertEqual(json.load(fh), PAYLOAD)


if __name__ == "__main__":
    unittest.main()
