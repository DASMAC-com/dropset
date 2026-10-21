#!/usr/bin/env python3
"""Measure the merge-queue critical path from GitHub Actions run data.

The ci-infra audit's instrument, committed so the next pass diffs against the
last one instead of rebuilding it from scratch (the 08-24 pass proposed it and
never built it; the 10-21 pass rebuilt it as a scratchpad script).

The default mode lists every ``merge_group`` workflow run created in the last
``--days`` days, groups the runs by queue ref
(``gh-readonly-queue/main/pr-N-<sha>``), and reports, per queue entry, the
wall-clock critical path: the span from the earliest job start to the latest
job completion across every workflow the entry triggered. Per-job durations
(``completed_at - started_at``) are summarized by job name so a drift in one
job is visible, along with which job was the slowest on each entry and which
entries were not all-green.

Output is a ranked summary; ``--full`` adds every entry. ``--steps JOB`` adds
per-step durations inside one job name. ``--cache FILE`` stores the raw API
payload so a re-run re-reads it instead of re-fetching (the first fetch of a
week is several minutes of ``gh api`` reads).

Two single-job modes read one job's log instead:

* ``--failed-log JOB_ID`` prints the failure-shaped lines (test names, panics,
  compiler errors, ``::error::`` annotations), so a dequeue is classified
  without reading the log.
* ``--hook-timing JOB_ID`` derives per-hook durations inside a Lint job from
  pre-commit's result-line timestamps.

Reads go through ``gh api`` / ``gh run view`` and need no token in the
environment. Run via ``make ci-metrics ARGS='…'``.
"""

from __future__ import annotations

import argparse
import json
import re
import statistics
import subprocess
import sys
from datetime import datetime, timedelta, timezone

REPO = "DASMAC-com/dropset"
QUEUE_PREFIX = "gh-readonly-queue/main/pr-"

FAIL_RE = re.compile(
    r"(panicked at|^.*\bFAILED\b|error\[E|^.*\berror:|assertion .*failed|"
    r"test result: FAILED|--- FAIL|Error: Process completed|"
    r"thread '.*' panicked|\.\.\.\.* ?Failed$|\[(FAIL|TIMEOUT)\]|"
    r"\b(FAIL|TIMEOUT) \[|::error::)"
)
# A pre-commit result line: ``<stamp> <hook name>.....<postfix?><color?>Passed``.
# The postfix is a skipped hook's ``(no files to check)``. The color escape
# appears raw (``\x1b[``) or caret-rendered (``^[[``) depending on how the log
# was captured.
HOOK_RE = re.compile(
    r"^(\d{4}-\d\d-\d\dT[\d:.]+Z) (.+?)\.{3,}(?:\([^)]*\))?"
    r"(?:\x1b\[[0-9;]*m|\^\[\[[0-9;]*m)*(Passed|Failed|Skipped)"
)


def gh_api(path: str, paginate_key: str | None = None):
    """One ``gh api`` read; with ``paginate_key``, every page's list merged."""
    cmd = ["gh", "api"]
    if paginate_key:
        cmd += ["--paginate", "--slurp"]
    cmd.append(path)
    out = subprocess.run(cmd, capture_output=True, text=True, check=True).stdout
    data = json.loads(out)
    if paginate_key:
        return merge_pages(data, paginate_key)
    return data


def merge_pages(pages: list[dict], key: str) -> list:
    """Concatenate ``key`` across ``--slurp`` pages, naming the key explicitly.

    The runs endpoint returns ``total_count`` beside ``workflow_runs``; picking
    the first list-valued key instead would break the day a second list joins.
    """
    merged = []
    for page in pages:
        merged.extend(page.get(key, []))
    return merged


def parse_ts(s: str) -> datetime:
    """Parse a GitHub / Actions-log timestamp, trimming sub-microsecond digits.

    Actions log stamps carry seven fractional digits, which ``fromisoformat``
    rejects before Python 3.11.
    """
    s = s.replace("Z", "+00:00")
    m = re.match(r"^(.*\.\d{6})\d*(\+00:00)$", s)
    if m:
        s = m.group(1) + m.group(2)
    return datetime.fromisoformat(s)


def minutes(td: timedelta) -> float:
    return td.total_seconds() / 60.0


def fetch(days: int) -> dict:
    since = (datetime.now(timezone.utc) - timedelta(days=days)).strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    )
    runs = gh_api(
        f"repos/{REPO}/actions/runs?event=merge_group&per_page=100&created=>={since}",
        paginate_key="workflow_runs",
    )
    jobs = {}
    for run in runs:
        jobs[str(run["id"])] = gh_api(
            f"repos/{REPO}/actions/runs/{run['id']}/jobs?per_page=100"
        )["jobs"]
    return {"since": since, "runs": runs, "jobs": jobs}


def stats(xs: list[float]) -> str:
    if not xs:
        return "n=0"
    xs = sorted(xs)
    p90 = xs[min(len(xs) - 1, int(round(0.9 * (len(xs) - 1))))]
    return (
        f"n={len(xs)} median={statistics.median(xs):.1f}m "
        f"p90={p90:.1f}m min={xs[0]:.1f}m max={xs[-1]:.1f}m"
    )


def analyse(payload: dict, steps_for: str | None = None) -> dict:
    """Reduce a raw payload to queue entries, job durations and step durations.

    Pure: no I/O, so the fixture tests drive it directly.
    """
    jobs = payload["jobs"]
    entries: dict[str, list[dict]] = {}
    for run in payload["runs"]:
        ref = run["head_branch"]
        if ref.startswith(QUEUE_PREFIX):
            entries.setdefault(ref, []).append(run)

    rows = []
    # name -> [(minutes, ref, conclusion)]
    job_durations: dict[str, list[tuple[float, str, str | None]]] = {}
    step_durations: dict[str, list[float]] = {}
    unstarted: list[str] = []
    for ref, ref_runs in entries.items():
        starts, ends, failed_jobs = [], [], []
        conclusions = set()
        best: tuple[float, str] | None = None
        for run in ref_runs:
            conclusions.add(run.get("conclusion"))
            for job in jobs.get(str(run["id"]), []):
                conclusion = job.get("conclusion")
                if conclusion not in (None, "success", "skipped"):
                    failed_jobs.append(f"{job['name']}={conclusion}")
                if steps_for and job["name"] == steps_for and conclusion == "success":
                    for step in job.get("steps", []):
                        if step.get("started_at") and step.get("completed_at"):
                            d = minutes(
                                parse_ts(step["completed_at"])
                                - parse_ts(step["started_at"])
                            )
                            step_durations.setdefault(step["name"], []).append(d)
                if not (job.get("started_at") and job.get("completed_at")):
                    continue
                s = parse_ts(job["started_at"])
                e = parse_ts(job["completed_at"])
                starts.append(s)
                ends.append(e)
                d = minutes(e - s)
                job_durations.setdefault(job["name"], []).append((d, ref, conclusion))
                if conclusion == "success" and (best is None or d > best[0]):
                    best = (d, job["name"])
        if not starts:
            # Queued but not yet picked up: no span to measure, but still an
            # entry in flight, which the header must count.
            if None in conclusions:
                unstarted.append(ref[len(QUEUE_PREFIX) :].split("-")[0])
            continue
        rows.append(
            {
                "pr": ref[len(QUEUE_PREFIX) :].split("-")[0],
                "ref": ref,
                "started": min(starts).isoformat(),
                "wall_min": round(minutes(max(ends) - min(starts)), 1),
                "weekday": min(starts).strftime("%a"),
                "runs": len(ref_runs),
                "conclusions": sorted(c or "none" for c in conclusions),
                "failed_jobs": failed_jobs,
                "slowest_job": best[1] if best else None,
            }
        )
    rows.sort(key=lambda r: r["started"])
    return {
        "rows": rows,
        "unstarted": unstarted,
        "job_durations": job_durations,
        "step_durations": step_durations,
    }


def is_green(row: dict) -> bool:
    """Every run succeeded, or was skipped wholesale by an ``if:``."""
    return "success" in row["conclusions"] and set(row["conclusions"]) <= {
        "success",
        "skipped",
    }


def render(payload: dict, full: bool, steps_for: str | None = None) -> list[str]:
    a = analyse(payload, steps_for)
    # An entry with a run still going is in flight, failed job or not: its span
    # is partial, so it joins none of the statistics below.
    in_flight = [r for r in a["rows"] if "none" in r["conclusions"]]
    rows = [r for r in a["rows"] if r not in in_flight]
    walls = [r["wall_min"] for r in rows]
    green = [r["wall_min"] for r in rows if is_green(r)]
    out = [
        f"merge_group runs since {payload['since']}: "
        f"{len(payload['runs'])} runs, {len(rows)} settled queue entries, "
        f"{len(in_flight) + len(a['unstarted'])} in flight",
        f"entry critical path (all):   {stats(walls)}",
        f"entry critical path (green): {stats(green)}",
        "",
        "per-job duration, successful jobs only, ranked by median:",
    ]
    table = []
    for name, ds in a["job_durations"].items():
        ok = [d for d, _, c in ds if c == "success"]
        if ok:
            table.append((statistics.median(ok), name, ok))
    for _, name, ok in sorted(table, reverse=True):
        out.append(f"  {name:28s} {stats(ok)}")
    out += ["", "critical-path job per entry (slowest successful job):"]
    slowest: dict[str, int] = {}
    for r in rows:
        if r["slowest_job"]:
            slowest[r["slowest_job"]] = slowest.get(r["slowest_job"], 0) + 1
    for name, n in sorted(slowest.items(), key=lambda kv: (-kv[1], kv[0])):
        out.append(f"  {name:28s} {n}")
    not_green = [r for r in rows if not is_green(r)]
    out += ["", f"entries not all-green: {len(not_green)}"]
    for r in not_green:
        out.append(
            f"  pr-{r['pr']} {r['started'][:16]} {r['weekday']} "
            f"{r['wall_min']}m {r['conclusions']} {r['failed_jobs']}"
        )
    flying = [
        f"pr-{r['pr']}" + (f"{r['failed_jobs']}" if r["failed_jobs"] else "")
        for r in in_flight
    ] + [f"pr-{pr}(unstarted)" for pr in a["unstarted"]]
    if flying:
        out.append("in flight (excluded above): " + " ".join(flying))
    if steps_for:
        out += [
            "",
            f"per-step duration inside successful '{steps_for}' jobs, "
            "ranked by median:",
        ]
        for name, ds in sorted(
            a["step_durations"].items(), key=lambda kv: -statistics.median(kv[1])
        ):
            out.append(f"  {name[:40]:40s} {stats(ds)}")
    if full:
        out += ["", "every entry:"]
        for r in rows:
            out.append(
                f"  pr-{r['pr']} {r['started'][:16]} {r['weekday']} "
                f"{r['wall_min']:6.1f}m runs={r['runs']} {r['conclusions']}"
            )
    return out


def job_log(job_id: int) -> list[str]:
    out = subprocess.run(
        ["gh", "run", "view", "--repo", REPO, "--job", str(job_id), "--log"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    return strip_log_prefix(out.splitlines())


def strip_log_prefix(lines: list[str]) -> list[str]:
    """Drop ``gh``'s ``<job>\\t<step>\\t`` prefix, keeping the timestamped tail."""
    return [line.split("\t", 2)[-1] for line in lines]


def failed_lines(lines: list[str]) -> list[str]:
    return [line for line in lines if FAIL_RE.search(line)]


def hook_rows(lines: list[str]) -> list[tuple[float, str, str]]:
    """Per-hook ``(minutes, name, result)``, slowest first.

    Each hook's time is the gap since the previous result line, so the first
    hook in the log has no row of its own.
    """
    prev = None
    rows = []
    for line in lines:
        m = HOOK_RE.match(line)
        if not m:
            continue
        stamp = parse_ts(m.group(1))
        if prev is not None:
            rows.append((minutes(stamp - prev), m.group(2).strip(), m.group(3)))
        prev = stamp
    rows.sort(reverse=True)
    return rows


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument(
        "--failed-log",
        metavar="JOB_ID",
        type=int,
        help="print failure-shaped lines from one job's log",
    )
    ap.add_argument(
        "--hook-timing",
        metavar="JOB_ID",
        type=int,
        help="per-hook durations from one Lint job's log",
    )
    ap.add_argument("--limit", type=int, default=25)
    ap.add_argument("--days", type=int, default=7)
    ap.add_argument("--full", action="store_true", help="print every entry")
    ap.add_argument("--cache", help="raw payload file; read if present, else written")
    ap.add_argument(
        "--steps", metavar="JOB", help="also summarize per-step durations in JOB"
    )
    args = ap.parse_args(argv)

    if args.failed_log:
        lines = job_log(args.failed_log)
        hits = failed_lines(lines)
        print(
            f"job {args.failed_log}: {len(lines)} log lines, {len(hits)} "
            f"failure-shaped lines; first {args.limit}:"
        )
        for line in hits[: args.limit]:
            print("  " + line[:220])
        return 0
    if args.hook_timing:
        lines = job_log(args.hook_timing)
        rows = hook_rows(lines)
        if lines and not rows:
            # A drifted pre-commit line format would otherwise read as a clean
            # "0 hook results".
            print(
                f"job {args.hook_timing}: {len(lines)} log lines but no "
                "pre-commit result line matched; is this a Lint job?",
                file=sys.stderr,
            )
            return 1
        total = sum(r[0] for r in rows)
        print(
            f"job {args.hook_timing}: {len(rows)} hook results spanning "
            f"{total:.1f}m (each hook's time is the gap since the previous "
            "result line)"
        )
        for d, name, res in rows[:15]:
            print(f"  {d:5.2f}m {name[:60]:60s} {res}")
        return 0

    payload = None
    if args.cache:
        try:
            with open(args.cache) as fh:
                payload = json.load(fh)
        except FileNotFoundError:
            pass
    if payload is None:
        payload = fetch(args.days)
        if args.cache:
            with open(args.cache, "w") as fh:
                json.dump(payload, fh)
    print("\n".join(render(payload, args.full, args.steps)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
