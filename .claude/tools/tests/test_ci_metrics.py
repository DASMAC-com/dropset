"""Stdlib ``unittest`` tests for the merge-queue CI-metrics reader.

The ``gh`` reads are not exercised; these cover the pure reductions over an
embedded fixture — the per-entry critical path, the slowest-job count, the
failed-job listing, the page merge, the day mode's baseline, growth flags,
queue waits and re-entries, the job cache — and the two log readers, including the
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
            "in flight (excluded above): pr-4 pr-5['Lint=failure'] "
            "pr-6(no job finished)",
            out,
        )

    def test_a_running_entry_with_no_finished_job_is_in_flight(self):
        payload = json.loads(json.dumps(PAYLOAD))
        payload["runs"].append(
            {"id": 81, "head_branch": f"{Q}8-hhh", "conclusion": None}
        )
        payload["jobs"]["81"] = [
            {
                "name": "Tests",
                "started_at": "2026-10-20T15:00:00Z",
                "completed_at": None,
                "conclusion": None,
            }
        ]
        out = "\n".join(cm.render(payload, full=False))
        self.assertIn("3 settled queue entries, 1 in flight", out)
        self.assertIn("in flight (excluded above): pr-8(no job finished)", out)

    def test_a_settled_entry_with_no_job_run_is_a_not_green_dequeue(self):
        payload = json.loads(json.dumps(PAYLOAD))
        payload["runs"].append(
            {"id": 71, "head_branch": f"{Q}7-ggg", "conclusion": "startup_failure"}
        )
        payload["jobs"]["71"] = []
        out = "\n".join(cm.render(payload, full=False))
        self.assertIn("4 settled queue entries, 0 in flight", out)
        self.assertIn("entries not all-green: 2", out)
        self.assertIn("pr-7 no job ran ['startup_failure']", out)
        # No span, so the critical-path statistics are unchanged.
        self.assertIn("entry critical path (all):   n=3 ", out)

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
    # The shape CI's `--color=always` pre-commit run prints a failed hook in.
    "Lint\tRun pre-commit\t2026-10-20T10:06:00.0000000Z "
    "cspell...............^[[41mFailed^[[m",
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
        self.assertEqual(len(hits), 5)
        self.assertIn("cspell", hits[4])
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
        self.assertIn("job 7: 7 log lines, 5 failure-shaped lines; first 1:", out)
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
        # GH_LOG minus its trailing pre-commit line: a Tests log, no hooks.
        lines = cm.strip_log_prefix(GH_LOG[:-1])
        with mock.patch.object(cm, "job_log", return_value=lines):
            with redirect_stdout(io.StringIO()), redirect_stderr(err):
                self.assertEqual(cm.main(["--hook-timing", "9"]), 1)
        self.assertIn("no pre-commit result line matched", err.getvalue())

    def test_hook_timing_single_result_line_is_not_a_drift(self):
        with mock.patch.object(cm, "job_log", return_value=LINT_LOG[:1]):
            with redirect_stdout(io.StringIO()) as buf:
                self.assertEqual(cm.main(["--hook-timing", "10"]), 0)
        self.assertIn("0 hook results", buf.getvalue())

    def test_hook_timing_empty_log_fails_loudly(self):
        with mock.patch.object(cm, "job_log", return_value=[]):
            with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                self.assertEqual(cm.main(["--hook-timing", "11"]), 1)

    def test_cache_written_when_absent(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "payload.json")
            with mock.patch.object(cm, "fetch", return_value=PAYLOAD):
                with redirect_stdout(io.StringIO()):
                    cm.main(["--cache", path])
            with open(path) as fh:
                self.assertEqual(json.load(fh), PAYLOAD)


def day_run(rid, pr, created, conclusion="success"):
    return {
        "id": rid,
        "head_branch": f"{Q}{pr}-sha{pr}",
        "created_at": created,
        "conclusion": conclusion,
        "status": "completed",
        "run_attempt": 1,
    }


def day_job(name, start, end, conclusion="success"):
    return {
        "name": name,
        "started_at": start,
        "completed_at": end,
        "conclusion": conclusion,
    }


def add(at):
    return {"__typename": "AddedToMergeQueueEvent", "createdAt": at}


def remove(at, reason):
    return {
        "__typename": "RemovedFromMergeQueueEvent",
        "createdAt": at,
        "reason": reason,
    }


def pr(number, created, merged, *events):
    return {
        "number": number,
        "createdAt": created,
        "mergedAt": merged,
        "timelineItems": {"nodes": list(events)},
    }


# A day-mode payload shaped on the recorded 10-24 queue. The baseline is three
# green 8m entries on 10-20 (Lint 6m, 8m, 8m: median 8m). In the window:
#   pr-620: Lint failed, dequeued, re-entered on the SAME ref 18m later (two
#           builds, one ref) and merged after a 12m Lint.
#   pr-630: a 15m Lint, so a slow entry and a Lint that grew.
#   pr-640: enqueued 01:02 but built 01:20, behind pr-630.
D = "2026-10-24T"
DAY = {
    "since": f"{D}00:00:00Z",
    "until": f"{D}06:00:00Z",
    "runs": [
        day_run(1, 1, "2026-10-20T10:00:00Z"),
        day_run(2, 2, "2026-10-20T11:00:00Z"),
        day_run(3, 3, "2026-10-20T12:00:00Z"),
        day_run(41, 620, f"{D}00:00:45Z", "failure"),
        day_run(42, 620, f"{D}00:00:46Z"),
        day_run(43, 620, f"{D}00:18:44Z"),
        day_run(5, 630, f"{D}01:00:00Z"),
        day_run(6, 640, f"{D}01:20:00Z"),
    ],
    "jobs": {
        "1": [
            day_job("Lint", "2026-10-20T10:01:00Z", "2026-10-20T10:07:00Z"),
            day_job("SDK", "2026-10-20T10:01:00Z", "2026-10-20T10:09:00Z"),
        ],
        "2": [day_job("Lint", "2026-10-20T11:01:00Z", "2026-10-20T11:09:00Z")],
        "3": [day_job("Lint", "2026-10-20T12:01:00Z", "2026-10-20T12:09:00Z")],
        "41": [day_job("Lint", f"{D}00:01:00Z", f"{D}00:09:00Z", "failure")],
        "42": [day_job("SDK", f"{D}00:01:00Z", f"{D}00:04:00Z")],
        "43": [day_job("Lint", f"{D}00:19:00Z", f"{D}00:31:00Z")],
        "5": [day_job("Lint", f"{D}01:00:30Z", f"{D}01:15:30Z")],
        "6": [day_job("Lint", f"{D}01:20:12Z", f"{D}01:26:12Z")],
    },
    "prs": {
        "620": pr(
            620,
            "2026-10-23T23:00:00Z",
            f"{D}00:31:30Z",
            add(f"{D}00:00:40Z"),
            remove(f"{D}00:09:30Z", "failed_checks"),
            add(f"{D}00:18:40Z"),
            remove(f"{D}00:31:30Z", "merged"),
        ),
        "630": pr(630, f"{D}00:30:00Z", f"{D}01:16:00Z", add(f"{D}00:59:50Z")),
        "640": pr(
            640,
            f"{D}00:57:00Z",
            f"{D}01:27:00Z",
            add(f"{D}01:02:00Z"),
            remove(f"{D}01:27:00Z", "merged"),
        ),
        # Merged before the window opened: no row.
        "3": pr(3, "2026-10-20T09:00:00Z", "2026-10-20T12:10:00Z"),
    },
}


class DayReportTests(unittest.TestCase):
    def setUp(self):
        self.r = cm.day_report(DAY)
        self.entries = self.r["entries"]
        self.prs = {p["pr"]: p for p in self.r["prs"]}

    def test_a_re_entry_on_the_same_ref_is_its_own_build(self):
        builds = [e for e in self.entries if e["pr"] == "620"]
        self.assertEqual([e["green"] for e in builds], [False, True])
        self.assertEqual(builds[0]["failed_jobs"], ["Lint=failure"])
        self.assertEqual([e["wall_min"] for e in builds], [8.0, 12.0])

    def test_critical_path_against_the_baseline_median(self):
        self.assertEqual(self.r["baseline"]["median_wall_min"], 8.0)
        self.assertEqual(self.r["baseline"]["entries"], 3)
        slow = [e["pr"] for e in self.entries if e["slow"]]
        self.assertEqual(slow, ["630"])

    def test_a_job_that_grew_is_flagged_and_a_steady_one_is_not(self):
        jobs = {j["name"]: j for j in self.r["jobs"]}
        self.assertTrue(jobs["Lint"]["grew"])
        self.assertEqual(jobs["Lint"]["baseline_median_min"], 8.0)
        # SDK fell from 8m to 3m; its baseline has one point, so no median.
        self.assertFalse(jobs["SDK"]["grew"])
        self.assertIsNone(jobs["SDK"]["baseline_median_min"])

    def test_a_long_wait_names_the_entries_ahead(self):
        e = next(e for e in self.entries if e["pr"] == "640")
        self.assertEqual(e["queue_wait_min"], 18.0)
        self.assertEqual(e["pickup_min"], 0.2)
        self.assertEqual(e["behind"], ["pr-630"])
        short = next(e for e in self.entries if e["pr"] == "630")
        self.assertEqual(short["behind"], [])

    def test_pr_queue_share_re_entries_and_dequeues(self):
        p = self.prs["620"]
        self.assertEqual(p["re_entries"], 1)
        self.assertEqual(p["dequeues"], ["failed_checks"])
        # 00:00:40-00:09:30 plus 00:18:40-00:31:30.
        self.assertEqual(p["queue_min"], 21.7)
        self.assertEqual(p["open_to_merge_min"], 91.5)
        self.assertEqual(p["queue_share"], 0.237)
        # No removal event: the merge closes the interval.
        self.assertEqual(self.prs["630"]["queue_min"], 16.2)
        self.assertNotIn("3", self.prs)

    def test_critical_path_job_is_counted(self):
        # The failed build's critical path is its slowest successful job.
        self.assertEqual(self.r["critical_path"], [("Lint", 3), ("SDK", 1)])

    def test_lines_flag_the_slow_the_failed_and_the_grown(self):
        lines, flags = cm.day_lines(self.r)
        text = "\n".join(lines)
        self.assertIn("**Critical-path job**: Lint on 3 of 4, SDK on 1 of 4", text)
        self.assertIn("behind pr-630", text)
        self.assertIn("re-entered 1x; dequeued failed_checks", text)
        self.assertEqual(len(flags), 3)
        self.assertTrue(any("pr-630 critical path" in f for f in flags))
        self.assertTrue(any("pr-620 entry not all-green" in f for f in flags))
        self.assertTrue(any("job Lint" in f for f in flags))

    def test_unavailable_is_one_line_and_one_flag(self):
        lines, flags = cm.day_lines({"error": "gh exited 1: rate limited"})
        self.assertEqual(lines, ["CI data unavailable: gh exited 1: rate limited"])
        self.assertEqual(flags, lines)

    def test_a_pr_is_read_as_of_the_windows_close(self):
        # pr-620 at 00:20: re-entered and still queued, not yet merged.
        p = cm.pr_queue(DAY["prs"]["620"], cm.parse_ts(f"{D}00:20:00Z"))
        self.assertIsNone(p["merged"])
        self.assertEqual(p["re_entries"], 1)
        self.assertEqual(p["dequeues"], ["failed_checks"])
        # 00:00:40-00:09:30 plus 00:18:40 to the close.
        self.assertEqual(p["queue_min"], 10.2)
        self.assertIsNone(p["queue_share"])

    def test_a_run_created_at_the_close_is_the_next_windows(self):
        r = cm.day_report(dict(DAY, until=f"{D}01:20:00Z"))
        self.assertNotIn("640", [e["pr"] for e in r["entries"]])
        self.assertNotIn("640", [p["pr"] for p in r["prs"]])


class DayFetchTests(unittest.TestCase):
    def test_finished_runs_jobs_are_cached_and_re_read(self):
        jobs = [day_job("Lint", f"{D}00:01:00Z", f"{D}00:02:00Z")]
        with tempfile.TemporaryDirectory() as d:
            cache = cm.Path(d)
            done = day_run(7, 7, f"{D}00:00:00Z")
            with mock.patch.object(cm, "gh_api", return_value={"jobs": jobs}) as gh:
                self.assertEqual(cm.run_jobs(done, cache), jobs)
                self.assertEqual(cm.run_jobs(done, cache), jobs)
            self.assertEqual(gh.call_count, 1)
            running = dict(day_run(8, 8, f"{D}00:00:00Z"), status="in_progress")
            with mock.patch.object(cm, "gh_api", return_value={"jobs": jobs}) as gh:
                cm.run_jobs(running, cache)
                cm.run_jobs(running, cache)
            self.assertEqual(gh.call_count, 2)

    def test_prune_drops_only_files_past_the_keep_window(self):
        with tempfile.TemporaryDirectory() as d:
            cache = cm.Path(d)
            old, fresh = cache / "jobs-1-1.json", cache / "jobs-2-1.json"
            for path in (old, fresh):
                path.write_text("[]")
            now = old.stat().st_mtime + 86400
            past = now - (cm.CACHE_KEEP_DAYS + 1) * 86400
            os.utime(old, (past, past))
            cm.prune_cache(cache, now)
            self.assertFalse(old.exists())
            self.assertTrue(fresh.exists())

    def test_unmerged_window_prs_are_read_by_alias(self):
        search = {"search": {"nodes": [DAY["prs"]["620"], None, {}]}}
        aliased = {"repository": {"p640": DAY["prs"]["640"], "p650": None}}
        with mock.patch.object(cm, "gh_graphql", side_effect=[search, aliased]) as gql:
            found = cm.fetch_prs("A", "B", {"620", "640", "650"})
        self.assertEqual(sorted(found), ["620", "640"])
        query = gql.call_args_list[1].args[0]
        self.assertIn("p640: pullRequest(number: 640)", query)
        self.assertIn("p650: pullRequest(number: 650)", query)
        self.assertNotIn("p620", query)

    def test_fetch_day_spends_no_more_than_its_budget(self):
        runs = [day_run(1, 620, f"{D}00:00:45Z")]
        with mock.patch.object(cm, "gh_api", return_value=runs):
            with mock.patch.object(cm, "run_jobs") as jobs:
                with self.assertRaises(TimeoutError):
                    cm.fetch_day(
                        cm.parse_ts(DAY["since"]), cm.parse_ts(DAY["until"]), budget=-1
                    )
        jobs.assert_not_called()

    def test_only_digit_pr_numbers_reach_the_query(self):
        runs = [
            day_run(1, 620, f"{D}00:00:45Z"),
            dict(day_run(2, 0, f"{D}00:01:00Z"), head_branch=f'{Q}1"}}x-abc'),
            dict(day_run(3, 0, f"{D}00:02:00Z"), head_branch=None),
        ]
        with mock.patch.object(cm, "gh_api", return_value=runs):
            with mock.patch.object(cm, "run_jobs", return_value=[]):
                with mock.patch.object(cm, "fetch_prs", return_value={}) as prs:
                    cm.fetch_day(cm.parse_ts(DAY["since"]), cm.parse_ts(DAY["until"]))
        self.assertEqual(prs.call_args.args[2], {"620"})

    def test_main_since_prints_json(self):
        buf = io.StringIO()
        with mock.patch.object(cm, "fetch_day", return_value=DAY) as fetch:
            with redirect_stdout(buf):
                self.assertEqual(
                    cm.main(["--since", "2026-10-24T00:00:00", "--json"]), 0
                )
        self.assertEqual(
            fetch.call_args.args[0].isoformat(), "2026-10-24T00:00:00+00:00"
        )
        self.assertEqual(json.loads(buf.getvalue())["baseline"]["entries"], 3)


def at(minute: int) -> str:
    return f"2026-10-31T00:{minute:02d}:00Z"


def pr_run(rid, sha, created, conclusion="success", pr=770, branch="eng-1"):
    """A raw API run record of a ``pull_request`` run; its
    ``run_started_at`` is its creation, as the API's is."""
    return {
        "id": rid,
        "name": "Tests",
        "head_sha": sha,
        "head_branch": branch,
        "status": "completed",
        "conclusion": conclusion,
        "created_at": at(created),
        "run_started_at": at(created),
        "pull_requests": [{"number": pr}] if pr else [],
    }


def pr_job(created: int, started: int | None, conclusion: str = "success") -> dict:
    return {
        "created_at": at(created),
        "started_at": at(started) if started is not None else None,
        "conclusion": conclusion,
    }


# PR 770: four runs over two heads — the first superseded, the last cancelled;
# a closed PR's run, keyed by its branch; PR 771: one run, a 40-minute wait.
PR_RUNS = {
    "since": "2026-10-17T00:00:00Z",
    "until": "2026-10-31T01:00:00Z",
    "runs": [
        pr_run(1, "aaa", 0),
        pr_run(2, "bbb", 5),
        pr_run(3, "bbb", 7),
        pr_run(4, "ccc", 2, pr=None, branch="eng-9"),
        pr_run(5, "ddd", 10, pr=771),
        # Cancelled on the latest head, so cancelled and never superseded.
        pr_run(6, "bbb", 8, "cancelled"),
    ],
    # A skipped job never asks for a runner, so its zero wait is not counted.
    "jobs": {
        "1": [pr_job(0, 1)],
        "2": [pr_job(5, 6), pr_job(5, 5, "skipped")],
        "3": [pr_job(7, 10)],
        "4": [pr_job(2, 4)],
        "5": [pr_job(10, 50)],
    },
}


class PrRunsTests(unittest.TestCase):
    def test_superseded_cancelled_and_the_closed_pr_fallback(self):
        report = cm.pr_runs_report(PR_RUNS)
        by_pr = {r["pr"]: r for r in report["per_pr"]}
        self.assertEqual(list(by_pr), ["#770", "#771", "branch:eng-9"])
        self.assertEqual((by_pr["#770"]["runs"], by_pr["#770"]["heads"]), (4, 2))
        self.assertEqual(by_pr["#770"]["superseded"], 1)
        self.assertEqual(by_pr["#770"]["cancelled"], 1)
        self.assertEqual(report["branch_fallback"], 1)
        self.assertEqual((report["runs"], report["superseded"]), (6, 1))
        # Waits 1, 1, 3, 2, 40 minutes; the cancelled run was not sampled.
        self.assertEqual(report["wait_median_min"], 2.0)
        self.assertEqual(report["wait_p90_min"], 40.0)
        self.assertEqual(report["wait_max_min"], 40.0)
        self.assertEqual(report["waited_over_1m"], 5)
        self.assertEqual(by_pr["#770"]["wait_p90_min"], 3.0)

    def test_an_unsampled_or_queued_run_has_no_wait(self):
        payload = {"runs": [pr_run(1, "a", 0), pr_run(2, "a", 1)]}
        payload["jobs"] = {"2": [pr_job(1, None)]}
        report = cm.pr_runs_report(payload)
        self.assertIsNone(report["wait_median_min"])
        self.assertEqual((report["wait_sampled_runs"], report["wait_jobs"]), (1, 0))
        self.assertEqual(report["per_pr"][0]["runs"], 2)

    def test_the_wait_sample_spreads_over_completed_runs(self):
        runs = [pr_run(n, "a", n) for n in range(10)]
        runs.append({**pr_run(99, "a", 59), "status": "queued"})
        self.assertEqual([r["id"] for r in cm.wait_sample(runs, 3)], [0, 3, 6])
        self.assertEqual(len(cm.wait_sample(runs, 50)), 10)
        self.assertEqual(cm.wait_sample(runs, 0), [])

    def test_slim_records_read_the_same(self):
        slim = {**PR_RUNS, "runs": [cm._slim_run(r) for r in PR_RUNS["runs"]]}
        self.assertEqual(cm.pr_runs_report(slim), cm.pr_runs_report(PR_RUNS))

    def test_summary_lines(self):
        lines = cm.pr_runs_lines(cm.pr_runs_report(PR_RUNS), limit=2)
        self.assertIn("6 pull_request runs on 3 PR(s) (1 keyed by branch", lines[0])
        self.assertEqual(lines[1], "superseded 1/6 (17%); cancelled 1/6 (17%)")
        self.assertEqual(len(lines), 6)

    def test_a_capped_slice_at_the_floor_warns(self):
        full = [{"id": n} for n in range(cm.RUNS_SEARCH_CAP)]
        start = cm.parse_ts("2026-10-30T00:00:00Z")
        err = io.StringIO()
        with mock.patch.object(cm, "gh_api", return_value=full), redirect_stderr(err):
            runs = cm._runs_between(start, start + cm.MIN_SLICE)
        self.assertEqual(len(runs), cm.RUNS_SEARCH_CAP)
        self.assertIn("truncated", err.getvalue())

    def test_a_capped_window_is_split(self):
        full = [{"id": n} for n in range(cm.RUNS_SEARCH_CAP)]
        start = cm.parse_ts("2026-10-30T00:00:00Z")
        end = cm.parse_ts("2026-10-30T02:00:00Z")
        with mock.patch.object(
            cm, "gh_api", side_effect=[full, [{"id": 1}], [{"id": 2}]]
        ) as gh:
            runs = cm._runs_between(start, end)
        self.assertEqual(runs, [{"id": 1}, {"id": 2}])
        self.assertIn(
            "created=2026-10-30T00:00:00Z..2026-10-30T01:00:00Z",
            gh.call_args_list[1].args[0],
        )

    def test_fetch_reads_a_day_at_a_time_and_dedups(self):
        now = cm.parse_ts("2026-10-31T00:00:00Z")
        with mock.patch.object(cm, "gh_api", return_value=[pr_run(1, "a", 0)]) as gh:
            with mock.patch.object(cm, "run_jobs", return_value=[pr_job(0, 3)]) as jobs:
                payload = cm.fetch_pr_runs(2, now=now)
        self.assertEqual(gh.call_count, 2)
        self.assertEqual(len(payload["runs"]), 1)
        self.assertEqual(payload["runs"][0]["prs"], [770])
        self.assertEqual(jobs.call_count, 1)
        self.assertEqual(payload["jobs"], {"1": [pr_job(0, 3)]})

    def test_main_pr_runs_writes_and_rereads_the_cache(self):
        with tempfile.TemporaryDirectory() as tmp:
            cache = os.path.join(tmp, "runs.json")
            for _ in range(2):
                buf = io.StringIO()
                with mock.patch.object(
                    cm, "fetch_pr_runs", return_value=PR_RUNS
                ) as fetch:
                    with redirect_stdout(buf):
                        self.assertEqual(
                            cm.main(["--pr-runs", "--cache", cache, "--json"]), 0
                        )
                out = json.loads(buf.getvalue())
                self.assertEqual(out["runs"], 6)
                # The per-PR series rides the JSON, never the summary.
                self.assertEqual([r["pr"] for r in out["per_pr"]][0], "#770")
            self.assertEqual(fetch.call_count, 0)


if __name__ == "__main__":
    unittest.main()
