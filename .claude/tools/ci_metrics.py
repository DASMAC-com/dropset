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

``--since STAMP`` is the day mode the daily session digest builds its CI
section from (:func:`fetch_day`, :func:`day_report`, :func:`day_lines`): the
queue entries created since the stamp, each critical path beside the rolling
:data:`BASELINE_DAYS`-day median, per-job medians with a job that grew past
:data:`JOB_GROWTH_FACTOR` flagged, the entries not all-green with the failed
job named, and **waiting apart from running** — per entry the time from its
PR's enqueue to the entry's build (with the entries it waited behind named on
a long wait) and from the build to the first job start, and per PR the
dequeues, re-entries and, once merged, the open-to-merge span with the share
spent in the queue. ``--cache-dir DIR`` keeps each finished run's jobs, so a
day's re-fetch of the baseline reads only the new runs.

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
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

REPO = "DASMAC-com/dropset"
QUEUE_PREFIX = "gh-readonly-queue/main/pr-"
# One build's workflows are created within a second of each other.
BUILD_GAP = timedelta(minutes=1)
# One read hung on the network must not hold the daily digest past upkeep's
# step timeout; a timed-out read is the digest's "unavailable" line.
GH_TIMEOUT = 60

# The day mode's baseline: the entries in the days before the window, the
# fewest that make a median, and how far past it a figure is flagged. A job is
# flagged only when it grew by both the factor and the floor, so a 30-second
# job doubling to a minute is not a morning line.
BASELINE_DAYS = 7
BASELINE_MIN_POINTS = 3
ENTRY_SLOW_FACTOR = 1.5
JOB_GROWTH_FACTOR = 1.25
JOB_GROWTH_FLOOR_MIN = 1.0
# An enqueue-to-build wait this long names the entries it waited behind.
LONG_WAIT_MIN = 10.0
# Cached job payloads older than this have left every baseline.
CACHE_KEEP_DAYS = BASELINE_DAYS * 2
# The merge-queue events of one PR's timeline; a merge closes the last
# interval with a removal whose reason is ``merged``.
PR_FIELDS = (
    "number createdAt mergedAt timelineItems(first: 100, itemTypes: "
    "[ADDED_TO_MERGE_QUEUE_EVENT, REMOVED_FROM_MERGE_QUEUE_EVENT]) { nodes { "
    "__typename ... on AddedToMergeQueueEvent { createdAt } "
    "... on RemovedFromMergeQueueEvent { createdAt reason } } }"
)

# What sits between a pre-commit hook's dot leader and its result word: an
# optional skipped-hook postfix ``(no files to check)``, then color escapes.
# CI runs pre-commit with ``--color=always``, and ``gh run view --log`` renders
# the escape caret-style (``^[[42m``); a raw ``\x1b[`` is accepted too.
PRE_RESULT = r"\.{3,}(?:\([^)]*\))?(?:\x1b\[[0-9;]*m|\^\[\[[0-9;]*m)*"
FAIL_RE = re.compile(
    r"(panicked at|^.*\bFAILED\b|error\[E|^.*\berror:|assertion .*failed|"
    r"test result: FAILED|--- FAIL|Error: Process completed|"
    rf"thread '.*' panicked|{PRE_RESULT}Failed|\[(FAIL|TIMEOUT)\]|"
    r"\b(FAIL|TIMEOUT) \[|::error::)"
)
# A pre-commit result line: ``<stamp> <hook name>.....<postfix?><color?>Passed``.
HOOK_RE = re.compile(
    rf"^(\d{{4}}-\d\d-\d\dT[\d:.]+Z) (.+?){PRE_RESULT}(Passed|Failed|Skipped)"
)


def gh_api(path: str, paginate_key: str | None = None):
    """One ``gh api`` read; with ``paginate_key``, every page's list merged."""
    cmd = ["gh", "api"]
    if paginate_key:
        cmd += ["--paginate", "--slurp"]
    cmd.append(path)
    out = subprocess.run(
        cmd, capture_output=True, text=True, check=True, timeout=GH_TIMEOUT
    ).stdout
    data = json.loads(out)
    if paginate_key:
        return merge_pages(data, paginate_key)
    return data


def gh_graphql(query: str) -> dict:
    """One ``gh api graphql`` read, its ``data`` object."""
    out = subprocess.run(
        ["gh", "api", "graphql", "-f", f"query={query}"],
        capture_output=True,
        text=True,
        check=True,
        timeout=GH_TIMEOUT,
    ).stdout
    return json.loads(out)["data"]


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


def utc(dt: datetime) -> datetime:
    """A bare stamp is UTC, as every GitHub one is."""
    return dt.replace(tzinfo=timezone.utc) if dt.tzinfo is None else dt


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


def stamp(dt: datetime) -> str:
    return dt.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def run_jobs(run: dict, cache_dir: Path | None) -> list:
    """One run's jobs, read from ``cache_dir`` when a finished run left them.

    Keyed by attempt as well as id: a re-run replaces the latest jobs."""
    path = None
    if cache_dir is not None:
        path = cache_dir / f"jobs-{run['id']}-{run.get('run_attempt', 1)}.json"
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            pass
    jobs = gh_api(f"repos/{REPO}/actions/runs/{run['id']}/jobs?per_page=100")["jobs"]
    if path is not None and run.get("status") == "completed":
        path.write_text(json.dumps(jobs), encoding="utf-8")
    return jobs


def prune_cache(cache_dir: Path, now: float) -> None:
    for path in cache_dir.glob("jobs-*.json"):
        try:
            if now - path.stat().st_mtime > CACHE_KEEP_DAYS * 86400:
                path.unlink()
        except OSError:
            continue


def fetch_prs(since: str, until: str, numbers: set[str]) -> dict[str, dict]:
    """Queue timelines of the PRs merged in ``[since, until)`` and of
    ``numbers`` (the window's entries, merged or not), keyed by number."""
    query = (
        f'{{ search(query: "repo:{REPO} is:pr is:merged merged:{since}..{until}", '
        f"type: ISSUE, first: 100) {{ nodes {{ ... on PullRequest {{ {PR_FIELDS} "
        "} } } }"
    )
    found = {str(n["number"]): n for n in gh_graphql(query)["search"]["nodes"] if n}
    missing = sorted(numbers - set(found), key=int)
    if missing:
        owner, name = REPO.split("/")
        aliases = " ".join(
            f"p{n}: pullRequest(number: {n}) {{ {PR_FIELDS} }}" for n in missing
        )
        data = gh_graphql(
            f'{{ repository(owner: "{owner}", name: "{name}") {{ {aliases} }} }}'
        )
        for pr in data["repository"].values():
            if pr:
                found[str(pr["number"])] = pr
    return found


def fetch_day(since: datetime, until: datetime, cache_dir: Path | None = None) -> dict:
    """The day mode's payload: every ``merge_group`` run created from
    :data:`BASELINE_DAYS` before ``since`` to ``until`` (the baseline and the
    window in one listing), their jobs, and the queue timelines of the PRs the
    window touched."""
    if cache_dir is not None:
        cache_dir.mkdir(parents=True, exist_ok=True)
        prune_cache(cache_dir, time.time())
    start = stamp(since - timedelta(days=BASELINE_DAYS))
    runs = gh_api(
        f"repos/{REPO}/actions/runs?event=merge_group&per_page=100"
        f"&created={start}..{stamp(until)}",
        paginate_key="workflow_runs",
    )
    jobs = {str(run["id"]): run_jobs(run, cache_dir) for run in runs}
    numbers = {
        run["head_branch"][len(QUEUE_PREFIX) :].split("-")[0]
        for run in runs
        if run["head_branch"].startswith(QUEUE_PREFIX)
        and parse_ts(run["created_at"]) >= since
    }
    return {
        "since": stamp(since),
        "until": stamp(until),
        "runs": runs,
        "jobs": jobs,
        "prs": fetch_prs(stamp(since), stamp(until), numbers),
    }


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
    by_ref: dict[str, list[dict]] = {}
    for run in payload["runs"]:
        ref = run["head_branch"]
        if ref.startswith(QUEUE_PREFIX):
            by_ref.setdefault(ref, []).append(run)
    # A PR re-entering on an unchanged base is rebuilt on the same ref, so one
    # ref can carry several builds; runs created more than BUILD_GAP after the
    # last are the next build, never one long entry.
    entries: list[tuple[str, list[dict]]] = []
    for ref, ref_runs in by_ref.items():
        if not all(r.get("created_at") for r in ref_runs):
            entries.append((ref, ref_runs))
            continue
        ref_runs = sorted(ref_runs, key=lambda r: parse_ts(r["created_at"]))
        build = [ref_runs[0]]
        for run in ref_runs[1:]:
            gap = parse_ts(run["created_at"]) - parse_ts(build[-1]["created_at"])
            if gap > BUILD_GAP:
                entries.append((ref, build))
                build = []
            build.append(run)
        entries.append((ref, build))

    rows = []
    # name -> [(minutes, ref, conclusion)]
    job_durations: dict[str, list[tuple[float, str, str | None]]] = {}
    step_durations: dict[str, list[float]] = {}
    spanless: list[dict] = []
    for ref, ref_runs in entries:
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
            # No job has finished: queued or running (in flight), or settled
            # without a job ever running (a startup_failure, a cancel before
            # pickup). No span to measure, but the entry must still be counted.
            spanless.append(
                {
                    "pr": ref[len(QUEUE_PREFIX) :].split("-")[0],
                    "conclusions": sorted(c or "none" for c in conclusions),
                    "failed_jobs": failed_jobs,
                }
            )
            continue
        rows.append(
            {
                "pr": ref[len(QUEUE_PREFIX) :].split("-")[0],
                "ref": ref,
                # When the entry was built: its earliest run's creation.
                "created": min(parse_ts(r["created_at"]) for r in ref_runs).isoformat()
                if all(r.get("created_at") for r in ref_runs)
                else None,
                "started": min(starts).isoformat(),
                "ended": max(ends).isoformat(),
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
        "spanless": spanless,
        "job_durations": job_durations,
        "step_durations": step_durations,
    }


def is_green(row: dict) -> bool:
    """Every run succeeded, or was skipped wholesale by an ``if:``."""
    return "success" in row["conclusions"] and set(row["conclusions"]) <= {
        "success",
        "skipped",
    }


def _median(xs: list[float]) -> float | None:
    return statistics.median(xs) if len(xs) >= BASELINE_MIN_POINTS else None


def _queue_events(pr: dict) -> list[tuple[datetime, str, str | None]]:
    """``(instant, "add" | "remove", reason)``, oldest first."""
    events = []
    for node in (pr.get("timelineItems") or {}).get("nodes") or []:
        if not node or not node.get("createdAt"):
            continue
        kind = node.get("__typename")
        if kind == "AddedToMergeQueueEvent":
            events.append((parse_ts(node["createdAt"]), "add", None))
        elif kind == "RemovedFromMergeQueueEvent":
            events.append((parse_ts(node["createdAt"]), "remove", node.get("reason")))
    events.sort(key=lambda e: e[0])
    return events


def pr_queue(pr: dict, until: datetime) -> dict:
    """One PR's queue history: time in the queue (each add to the next
    removal, or ``until`` while still queued), re-entries, and the reasons of
    the removals that were not the merge."""
    events = _queue_events(pr)
    queued = timedelta()
    opened = None
    adds = 0
    dequeues = []
    for at, kind, reason in events:
        if kind == "add":
            adds += 1
            if opened is None:
                opened = at
        else:
            if reason != "merged":
                dequeues.append(reason or "unknown")
            if opened is not None:
                queued += at - opened
                opened = None
    if opened is not None:
        end = parse_ts(pr["mergedAt"]) if pr.get("mergedAt") else until
        queued += max(end - opened, timedelta())
    row = {
        "pr": str(pr["number"]),
        "merged": pr.get("mergedAt"),
        "queue_min": round(minutes(queued), 1),
        "re_entries": max(adds - 1, 0),
        "dequeues": dequeues,
        "open_to_merge_min": None,
        "queue_share": None,
    }
    if pr.get("mergedAt") and pr.get("createdAt"):
        span = minutes(parse_ts(pr["mergedAt"]) - parse_ts(pr["createdAt"]))
        row["open_to_merge_min"] = round(span, 1)
        row["queue_share"] = round(minutes(queued) / span, 3) if span > 0 else None
    return row


def day_report(payload: dict) -> dict:
    """Reduce a :func:`fetch_day` payload to the digest's CI section data.

    Pure, like :func:`analyse`: the fixture tests drive it directly. The
    window is the entries whose runs were created at or after ``since``; the
    earlier runs are the baseline.
    """
    since = parse_ts(payload["since"])
    until = parse_ts(payload["until"])
    window_runs = [r for r in payload["runs"] if parse_ts(r["created_at"]) >= since]
    base_runs = [r for r in payload["runs"] if parse_ts(r["created_at"]) < since]
    a = analyse({"runs": window_runs, "jobs": payload["jobs"]})
    b = analyse({"runs": base_runs, "jobs": payload["jobs"]})
    base_rows = [r for r in b["rows"] if "none" not in r["conclusions"]]
    base_median = _median([r["wall_min"] for r in base_rows if is_green(r)])
    settled = [r for r in a["rows"] if "none" not in r["conclusions"]]
    prs = payload.get("prs") or {}
    adds_of = {
        n: [at for at, kind, _ in _queue_events(pr) if kind == "add"]
        for n, pr in prs.items()
    }
    # Every entry with a span, either side of the window: the candidates an
    # entry in the window waited behind.
    spans = [
        (r["pr"], parse_ts(r["created"]), parse_ts(r["ended"]))
        for r in a["rows"] + b["rows"]
        if r.get("created")
    ]

    entries = []
    for r in settled:
        started = parse_ts(r["started"])
        created = parse_ts(r["created"]) if r.get("created") else None
        entry = {
            "pr": r["pr"],
            "started": r["started"],
            "wall_min": r["wall_min"],
            "green": is_green(r),
            "failed_jobs": r["failed_jobs"],
            "slowest_job": r["slowest_job"],
            "slow": base_median is not None
            and r["wall_min"] > ENTRY_SLOW_FACTOR * base_median,
            "queue_wait_min": None,
            "pickup_min": None,
            "behind": [],
        }
        if created is not None:
            entry["pickup_min"] = round(minutes(started - created), 1)
            enqueued = [at for at in adds_of.get(r["pr"], []) if at <= created]
            if enqueued:
                wait = minutes(created - enqueued[-1])
                entry["queue_wait_min"] = round(wait, 1)
                if wait >= LONG_WAIT_MIN:
                    entry["behind"] = sorted(
                        {
                            f"pr-{pr}"
                            for pr, c, e in spans
                            if pr != r["pr"] and c < created and e > enqueued[-1]
                        }
                    )
        entries.append(entry)

    jobs = []
    for name, ds in a["job_durations"].items():
        ok = [d for d, _, c in ds if c == "success"]
        if not ok:
            continue
        median = statistics.median(ok)
        base = _median(
            [d for d, _, c in b["job_durations"].get(name, []) if c == "success"]
        )
        jobs.append(
            {
                "name": name,
                "n": len(ok),
                "median_min": round(median, 1),
                "baseline_median_min": None if base is None else round(base, 1),
                "grew": base is not None
                and median > JOB_GROWTH_FACTOR * base
                and median - base >= JOB_GROWTH_FLOOR_MIN,
            }
        )
    jobs.sort(key=lambda j: -j["median_min"])

    critical: dict[str, int] = {}
    for r in settled:
        if r["slowest_job"]:
            critical[r["slowest_job"]] = critical.get(r["slowest_job"], 0) + 1

    in_window = {r["pr"] for r in a["rows"]} | {s["pr"] for s in a["spanless"]}
    pr_rows = []
    for n, pr in sorted(prs.items(), key=lambda kv: int(kv[0])):
        merged = pr.get("mergedAt")
        if n in in_window or (merged and since <= parse_ts(merged) < until):
            pr_rows.append(pr_queue(pr, until))

    return {
        "since": payload["since"],
        "until": payload["until"],
        "baseline": {
            "days": BASELINE_DAYS,
            "entries": len(base_rows),
            "median_wall_min": None if base_median is None else round(base_median, 1),
        },
        "entries": entries,
        "never_ran": [s for s in a["spanless"] if "none" not in s["conclusions"]],
        "in_flight": len(a["rows"])
        - len(settled)
        + sum("none" in s["conclusions"] for s in a["spanless"]),
        "jobs": jobs,
        "critical_path": sorted(critical.items(), key=lambda kv: (-kv[1], kv[0])),
        "prs": pr_rows,
    }


def _m(value: float | None) -> str:
    return "n/a" if value is None else f"{value:.1f}m"


def day_lines(report: dict) -> tuple[list[str], list[str]]:
    """The digest's CI section as Markdown lines, and its flagged lines."""
    flags: list[str] = []
    if report.get("error"):
        return [f"CI data unavailable: {report['error']}"], flags
    base = report["baseline"]
    entries = report["entries"]
    out = [
        f"**Window**: {len(entries) + len(report['never_ran'])} settled queue "
        f"entries, {report['in_flight']} in flight; baseline "
        f"{base['entries']} entries over the prior {base['days']} days, "
        f"green critical-path median {_m(base['median_wall_min'])}",
    ]
    total = sum(n for _, n in report["critical_path"])
    if report["critical_path"]:
        out.append(
            "- **Critical-path job**: "
            + ", ".join(
                f"{name} on {n} of {total}" for name, n in report["critical_path"]
            )
        )
    out.append("- **Entries** (critical path; enqueue to build; build to first job):")
    for e in entries:
        text = (
            f"  - pr-{e['pr']} {e['started'][:16]} {_m(e['wall_min'])}; "
            f"waited {_m(e['queue_wait_min'])}; pickup {_m(e['pickup_min'])}"
        )
        if e["slowest_job"]:
            text += f"; slowest {e['slowest_job']}"
        if e["behind"]:
            text += f"; behind {' '.join(e['behind'])}"
        if e["slow"]:
            text += " FLAGGED"
            flags.append(
                f"CI pr-{e['pr']} critical path {_m(e['wall_min'])} is over "
                f"{ENTRY_SLOW_FACTOR:g}x the {base['days']}-day median"
            )
        if not e["green"]:
            text += f" NOT GREEN {e['failed_jobs']}"
            flags.append(f"CI pr-{e['pr']} entry not all-green {e['failed_jobs']}")
        out.append(text)
    for s in report["never_ran"]:
        out.append(f"  - pr-{s['pr']} no job ran {s['conclusions']} NOT GREEN")
        flags.append(f"CI pr-{s['pr']} entry settled with no job run")
    if not entries and not report["never_ran"]:
        out.append("  - none")
    out.append("- **Jobs** (window median vs baseline median):")
    for j in report["jobs"]:
        text = (
            f"  - {j['name']} {_m(j['median_min'])} vs "
            f"{_m(j['baseline_median_min'])} (n={j['n']})"
        )
        if j["grew"]:
            text += " FLAGGED"
            flags.append(
                f"CI job {j['name']} median {_m(j['median_min'])} grew past "
                f"{JOB_GROWTH_FACTOR:g}x its baseline {_m(j['baseline_median_min'])}"
            )
        out.append(text)
    out.append("- **PRs** (open to merge; in queue; re-entries; dequeues):")
    for p in report["prs"]:
        share = "" if p["queue_share"] is None else f" ({p['queue_share'] * 100:.0f}%)"
        merged = (
            f"merged after {_m(p['open_to_merge_min'])}"
            if p["merged"]
            else "not merged"
        )
        text = f"  - pr-{p['pr']} {merged}; queued {_m(p['queue_min'])}{share}"
        if p["re_entries"]:
            text += f"; re-entered {p['re_entries']}x"
        if p["dequeues"]:
            text += f"; dequeued {', '.join(p['dequeues'])}"
        out.append(text)
    if not report["prs"]:
        out.append("  - none")
    return out, flags


def render(payload: dict, full: bool, steps_for: str | None = None) -> list[str]:
    a = analyse(payload, steps_for)
    # An entry with a run still going is in flight, failed job or not: its span
    # is partial, so it joins none of the per-entry statistics below. Its
    # completed jobs are still valid durations, so the per-job table keeps them.
    in_flight = [r for r in a["rows"] if "none" in r["conclusions"]]
    rows = [r for r in a["rows"] if r not in in_flight]
    spanless_flying = [s for s in a["spanless"] if "none" in s["conclusions"]]
    # Settled with no job ever run: a dequeue with no span, never green.
    never_ran = [s for s in a["spanless"] if s not in spanless_flying]
    walls = [r["wall_min"] for r in rows]
    green = [r["wall_min"] for r in rows if is_green(r)]
    out = [
        f"merge_group runs since {payload['since']}: "
        f"{len(payload['runs'])} runs, "
        f"{len(rows) + len(never_ran)} settled queue entries, "
        f"{len(in_flight) + len(spanless_flying)} in flight",
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
    out += ["", f"entries not all-green: {len(not_green) + len(never_ran)}"]
    for r in not_green:
        out.append(
            f"  pr-{r['pr']} {r['started'][:16]} {r['weekday']} "
            f"{r['wall_min']}m {r['conclusions']} {r['failed_jobs']}"
        )
    for s in never_ran:
        out.append(f"  pr-{s['pr']} no job ran {s['conclusions']}")
    flying = [
        f"pr-{r['pr']}" + (f"{r['failed_jobs']}" if r["failed_jobs"] else "")
        for r in in_flight
    ] + [
        f"pr-{s['pr']}(no job finished)"
        + (f"{s['failed_jobs']}" if s["failed_jobs"] else "")
        for s in spanless_flying
    ]
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
    ap.add_argument(
        "--since",
        metavar="STAMP",
        help="day mode: the entries created since this ISO instant",
    )
    ap.add_argument("--until", metavar="STAMP", help="day mode's close (default: now)")
    ap.add_argument(
        "--cache-dir", help="day mode: keep each finished run's jobs in this directory"
    )
    ap.add_argument(
        "--json", action="store_true", help="day mode: print the report as JSON"
    )
    args = ap.parse_args(argv)

    if args.since:
        try:
            since = utc(parse_ts(args.since))
            until = (
                utc(parse_ts(args.until)) if args.until else datetime.now(timezone.utc)
            )
        except ValueError as exc:
            ap.error(f"not an ISO instant: {exc}")
        cache_dir = Path(args.cache_dir) if args.cache_dir else None
        report = day_report(fetch_day(since, until, cache_dir))
        if args.json:
            print(json.dumps(report))
        else:
            lines, flags = day_lines(report)
            print("\n".join(lines + [f"flagged: {len(flags)}"] + flags))
        return 0
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
        if not any(HOOK_RE.match(line) for line in lines):
            # A drifted pre-commit line format (or an empty log) would
            # otherwise read as a clean "0 hook results". Test the lines, not
            # the rows: the first matched line never yields a row.
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
