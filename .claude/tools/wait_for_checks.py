#!/usr/bin/env python3
"""Wait for a PR's CI checks to settle, then print one compact verdict.

This is ``review-pr`` step 18's wait, hardened into a tool. What it replaces was
a model-driven loop, and the loop was the problem in three separate ways measured
across eight sessions:

* **It busy-polled.** Repeated ``gh pr checks`` calls with no reliable pacing
  primitive returned byte-identical all-pending snapshots — two of them on one
  run — each replayed as input on every later turn for zero information.
* **It reached for a shell compound.** One run emitted a background ``for``-loop
  under the ``#compound-ok`` escape marker twice, plus eleven ``gh pr view``
  calls. A single command that reduces to one allow-rule is the whole point of
  the shell-discipline rule (``CLAUDE.md`` → "Shell commands").
* **It read the wrong thing on failure.** Diagnosing which check failed took
  repeated per-check reads, when one JSON read answers it completely.

So the wait is a **bounded** pair of ``gh`` invocations — no model-driven loop:

1. ``gh pr checks --watch`` — gh does the pacing and exits when the checks
   settle. Its live-updating table is captured to a log, never to stdout, for
   the same reason ``run_quiet.py`` captures a build log.
2. ``gh pr checks --json`` — one structured read that *is* the verdict, including
   every failing check's name, workflow, and link.

The JSON read, not the watch's exit code, is the authority on the outcome: ``gh``
overloads its exit status (non-zero covers "a check failed", "checks are still
pending", and "there are no checks at all"), and a review must distinguish those.

**The pair repeats when the read disagrees with the watch.** ``--watch`` settles
on gh's *own* census of the check set, which is not the same as the PR being
done: a workflow that registers late — one whose trigger matches a path only
some PRs touch — is still pending when gh returns. Believing that single
post-watch read is how this tool once reported ``settled: true`` alongside
``conclusion: "pending"``, twice in a row, for ~14 minutes of dead wall-clock on
a PR where nothing was actually wrong. A pending read therefore re-enters the
watch instead of being believed, bounded by both :data:`MAX_WATCH_ROUNDS` and
the caller's original ``--timeout`` so it cannot outlive the budget that was
set, and ``pending_checks`` names whatever is still outstanding.

The two retryable states hold **separate** round counters, so the reported
``watch_rounds`` is bounded by their SUM (:data:`MAX_WATCH_ROUNDS` +
:data:`MAX_NONE_ROUNDS`) rather than by either alone — a wait that observes
`pending` several times and then `none` spends rounds from both budgets. The
``--timeout`` bound is unaffected and remains the real ceiling.

A second mode, ``--run <id>``, watches **one workflow run** to its terminal
state instead of a PR's checks. That is the merge-queue half of the same job:
once ``mergeQueueEntry`` names the queue branch's check run, this blocks on it.
It exists for the same reason as the checks mode — a bare ``gh run watch``
re-prints the entire job tree on every refresh, and one such call emitted
**64.6KB**, overflowed the tool-result cap, was persisted to disk, and still
needed the terminal state re-probed afterwards.

A third mode, ``--queue <pr>``, watches an **enqueued PR** through to merged or
dequeued, including the window before any queue run exists (where ``--run`` has
no id to take). It polls both PR-state sources and the PR's queue-branch runs,
prints nothing until a terminal outcome, and is meant to run as one backgrounded
command so its exit wakes the session. See :func:`wait_queue`.

A fourth, ``--mergeable <pr>``, is the mergeability tri-state with its settle
loop — one call serving ``review-pr`` steps 16 and 20. See :func:`wait_mergeable`.

Usage::

    python3 .claude/tools/wait_for_checks.py --pr 285 \
      --snapshot <scratchpad>/pre-wait.diff --verdict-out <scratchpad>/ci.json
    python3 .claude/tools/wait_for_checks.py --pr 285 --interval 30 --timeout 1800
    python3 .claude/tools/wait_for_checks.py --run 1234567890
    python3 .claude/tools/wait_for_checks.py --queue 285
    python3 .claude/tools/wait_for_checks.py --mergeable 285

Options: exactly one of ``--pr`` / ``--run`` / ``--queue`` / ``--mergeable``,
``--repo`` (default ``DASMAC-com/dropset``), ``--verdict-out <path>`` (also
write the verdict there), ``--snapshot <path>`` with ``--base`` (checks mode:
see :func:`snapshot`),
``--interval`` (seconds between refreshes, default 30, or 60 under
``--queue``; honored in every watching mode), ``--timeout``
(seconds before giving up on the watch, default 3600, or 7200 under
``--queue``) — both rejected with ``--mergeable``, whose settle loop is
fixed — ``--no-watch`` (skip the
watch and just read the current state once — a resumed session where the
background task is gone; checks mode only).

Prints JSON::

    {
      "pr": 285,
      "repo": "DASMAC-com/dropset",
      "conclusion": "pass",      // pass | fail | pending | none |
                                 // conflicting | timeout
      "settled": true,
      "elapsed_seconds": 127,
      "watch_rounds": 1,         // >1 means a check registered late
      "counts": {"pass": 12, "fail": 0, "pending": 0, "skipping": 3},
      "failing": [{"name": "…", "workflow": "…", "link": "…", "run_id": "…"}],
      "pending_checks": [],      // names still outstanding, when any are
      "mergeable": null,         // probed only when the conclusion was `none`
      "merge_state_status": null,
      "blocked_by_conflict": false,
      "failed_steps": [{"check": "…", "job_id": "…", "step": "…",
                        "kind": "test_runner", "next_command": "…"}],
      "snapshot": {"path": "…", "bytes": 4096, "uncommitted": [], …},
      "hint": {"gate": "green", "do": "…", "evidence": "…"},
      "log_path": "/…/wait-for-checks-285.log"
    }

``hint.gate`` is ``green`` / ``blocking`` / ``unverified`` / ``retry``; act on
it. ``failed_steps`` is filled only on ``fail``: one record per failing step,
whose ``kind`` (``lint_hook`` / ``test_runner`` / ``single_command`` /
``never_ran`` / ``external`` / ``fetch_failed``) routes the one
``next_command`` that reads it — ``null`` for ``external``, whose third-party
``link`` is reported instead, never as a command. See :func:`diagnose_failures`.

``none`` is **ambiguous and is disambiguated for you.** A CONFLICTING PR cannot
produce any ``pull_request`` workflow run at all — the merge ref cannot be
created — so there is no run, no error, and nothing to tell it apart from CI
that has not started yet. One session spent ~20 minutes and ~16 polls on that,
including an amend-and-force-push nudge at an event that could never fire. So
on ``none`` this tool probes ``mergeable`` and reports ``conflicting`` when
that is the cause. Treat ``none`` as "no checks apply"; treat ``conflicting``
as **blocking** — rebase, then re-run.

or, under ``--run``::

    {
      "run_id": "1234567890",
      "repo": "DASMAC-com/dropset",
      "conclusion": "pass",      // pass | fail | timeout
      "settled": true,
      "elapsed_seconds": 214,
      "exit_code": 0,
      "log_path": "/…/wait-for-run-1234567890.log"
    }

or, under ``--queue``::

    {
      "pr": 285,
      "mode": "queue",
      "conclusion": "merged",    // merged | dequeued | closed | not_queued |
                                 // timeout | probe_failed
      "settled": true,
      "elapsed_seconds": 1310,
      "polls": 22,
      "seen_queue_entry": true,
      "last": {"gql_state": "MERGED", "entry_state": null, …},
      "queue_runs": [{"run_id": "…", "name": "…", "conclusion": "…", …}],
      "failing_runs": [],
      "notifications": {"dismissed": 1, "failed": 0, "error": null},
      "log_path": "/…/wait-for-queue-285.log"
    }

``notifications`` is filled only on ``merged``: the watch marks the PR's GitHub
notification threads done itself, so no session turn sits between the merge and
the dismissal. Best-effort — a failure is reported there, never changes the
conclusion. See :func:`dismiss_pr_notifications`.

Exit code is 0 when ``conclusion`` is ``pass`` (or ``merged``, or
``mergeable``), else 1 — so a
caller that only checks the status still cannot mistake a red build for green.

Standard library only. A Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling"). Tests live in ``tests/test_wait_for_checks.py``, run via
``make tools-tests``.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time
from datetime import datetime
from pathlib import Path

DEFAULT_REPO = "DASMAC-com/dropset"

# How many times to re-probe a mergeable value of UNKNOWN, and how long to wait
# between. GitHub computes mergeability asynchronously; a couple of seconds is
# usually enough, and concluding from UNKNOWN is the failure being avoided.
MERGEABLE_RETRIES = 3
MERGEABLE_RETRY_DELAY = 2

# gh's own refresh cadence under --watch. 30s matches CI's granularity: the
# shortest job on this repo is several seconds and the longest many minutes, so a
# tighter interval only produces redraws nobody reads.
DEFAULT_INTERVAL = 30

# An outer bound on the watch, so a wedged or cancelled run can't hold the
# session open indefinitely. An hour is well past this repo's slowest suite.
DEFAULT_TIMEOUT = 3600

# How many times the watch may be re-entered after it exits with a check still
# pending. Each re-entry costs one gh invocation, and in practice a late
# registration is caught by the second round; the cap exists so that a check
# wedged in `pending` reports a timeout at a predictable point instead of
# re-watching until the full `--timeout` elapses.
MAX_WATCH_ROUNDS = 5

#: Read conclusions that mean "ask again" rather than "this is the answer".
#:
#: `pending` is obvious. `none` is here because it is the *registration race*:
#: `gh` exits immediately when no check has registered yet, which is the norm
#: in the seconds after a push, and believing it declares a CI-unverified
#: commit green. The tool already re-entered the watch for `pending`; `none`
#: was left believing itself, even though `pending` fails safe and `none`
#: fails open.
RETRY_CONCLUSIONS = ("pending", "none")

#: Rounds and pacing for a `none` re-read, deliberately NOT the pending ones.
#:
#: The registration race resolves in **seconds** — the measured instance
#: returned in 1 second with four runs already starting — while
#: `DEFAULT_INTERVAL` is sized for a whole CI run. Reusing the pending pacing
#: would add minutes of dead wall-clock to every genuinely check-less PR, which
#: is a real cost paid on the common case to guard the rare one. A short settle
#: bounded by a few rounds separates the race from the absence at almost no
#: cost.
NONE_SETTLE_SECONDS = 3
MAX_NONE_ROUNDS = 3

# The fields `gh pr checks --json` exposes that a review actually needs — and
# nothing more, since the point of this read is to be the one compact payload.
# `bucket` is the normalized outcome (pass / fail / pending / skipping / cancel)
# and is what `summarize` keys on; the rest only decorate a failure. (`state` and
# `completedAt` were requested here at first and read by nothing — dropped.)
JSON_FIELDS = "name,bucket,link,workflow,description"

# A GitHub Actions check's link ends in /runs/<run_id>/job/<job_id> or
# /actions/runs/<run_id>. The run id is what `get_job_logs` needs, so pull it out
# rather than making the caller parse a URL by hand.
_RUN_ID_RE = re.compile(r"/runs/(\d+)")
_JOB_ID_RE = re.compile(r"/job/(\d+)")

# Step conclusions that mark the step a failed job died on.
FAILED_STEP_CONCLUSIONS = frozenset({"failure", "timed_out", "cancelled"})

# How a failing step's log is read, keyed on what the step IS. A test runner
# prints its summary after the failure and the post-steps epilogue after that,
# so a tail lands in the epilogue (history.md → "Tail landed in the epilogue");
# the Lint job runs every hook over `--all-files`, so its failing batch sits
# above any tail and only a local reproduce is trustworthy.
_LINT_STEP_RE = re.compile(r"\bpre-commit run\b")
_TEST_STEP_RE = re.compile(
    r"\b(nextest|cargo test|pnpm (?:run )?test|pytest|unittest"
    r"|make [\w-]*tests?)\b"
)

STEP_KIND_NOTES = {
    "lint_hook": (
        "reproduce locally and trust its exit code; never re-fetch a larger "
        "tail; once the output names the failing hook, re-run with "
        "`-- <hook-id>`. A red CI Lint with a green local reproduce is "
        "toolchain skew — confirm on another branch's Lint job, never edit "
        "unrelated code; pin or report the toolchain"
    ),
    "test_runner": "grep the captured log for the runner's failure summary",
    "single_command": "its last output is its error: read the captured log's tail",
    "never_ran": "the job failed with no failed step (setup, cancel, or runner)",
    "external": "not an Actions check: its link is third-party; open it, never run it",
}

# The settle loop for `--mergeable`, longer than the `none` probe's, because
# this read is the gate itself (steps 16 and 20) rather than a cause probe.
# Re-reads after the first, so the bound is one more read than this.
MERGEABLE_SETTLE_RETRIES = 6
MERGEABLE_SETTLE_DELAY = 5


class WaitForChecksError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def log_path_for(pr: int) -> Path:
    """Where the watch's captured output goes.

    Same shape as ``run_quiet.py``'s log dir — ``gettempdir()`` plus a fixed
    name, created ``0o700``. On macOS ``gettempdir()`` is already per-user, which
    is what actually separates two users here: ``exist_ok=True`` does **not**
    re-apply the mode to a directory that already exists, so the mode protects
    the create, not an inherited directory.

    **Deliberate divergence from the sibling:** ``run_quiet.py`` disambiguates
    concurrent runs with ``os.getpid()``; this keys on the **PR number** instead.
    A stable, guessable path is worth more here (a human or a later turn can find
    the log for PR 285 without hunting a pid), and the cost is bounded — two
    concurrent watches of the *same* PR would interleave into one file. The
    verdict is unaffected either way, since it comes from the separate JSON read,
    not from this log.
    """
    base = Path(tempfile.gettempdir()) / "claude-wait-checks"
    base.mkdir(mode=0o700, parents=True, exist_ok=True)
    return base / f"wait-for-checks-{pr}.log"


def run_log_path_for(run_id: str) -> Path:
    """Where a ``--run`` watch's captured output goes.

    Keyed on the run id for the same reason :func:`log_path_for` keys on the PR
    number: a stable, guessable path beats a pid nobody can reconstruct.
    """
    base = Path(tempfile.gettempdir()) / "claude-wait-checks"
    base.mkdir(mode=0o700, parents=True, exist_ok=True)
    return base / f"wait-for-run-{run_id}.log"


def _gh(args: list[str]) -> tuple[int, str, str]:
    """Run a ``gh`` command, returning ``(returncode, stdout, stderr)``.

    Never raises on a non-zero exit — ``gh pr checks`` uses non-zero to mean
    "some check failed", which is an outcome to report, not an error.
    """
    try:
        completed = subprocess.run(
            ["gh", *args],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            errors="replace",
            check=False,
        )
    except OSError as exc:
        raise WaitForChecksError(f"cannot run gh: {exc}") from exc
    return completed.returncode, completed.stdout, completed.stderr


def watch_checks(pr: int, repo: str, interval: int, timeout: int, log: Path) -> bool:
    """Block until the PR's checks settle. Returns ``False`` on timeout.

    gh owns the pacing, so there is no loop here — just one child process whose
    output is written straight to ``log``. On timeout the child is killed and the
    caller still reads the current state, so a timed-out wait reports what the
    checks *were* rather than nothing at all.
    """
    args = [
        "pr",
        "checks",
        str(pr),
        "--repo",
        repo,
        "--watch",
        "--interval",
        str(interval),
    ]
    try:
        fd = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    except OSError as exc:
        raise WaitForChecksError(f"cannot write {log}: {exc}") from exc
    with os.fdopen(fd, "w", encoding="utf-8", errors="replace") as fh:
        try:
            proc = subprocess.Popen(
                ["gh", *args], stdout=fh, stderr=subprocess.STDOUT, text=True
            )
        except OSError as exc:
            raise WaitForChecksError(f"cannot run gh: {exc}") from exc
        try:
            proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
            return False
    return True


def watch_run(
    run_id: str, repo: str, timeout: int, log: Path, interval: int = DEFAULT_INTERVAL
) -> tuple[bool, int]:
    """Block until one workflow run settles. Returns ``(settled, exit_code)``.

    The merge-queue sibling of :func:`watch_checks`, and it exists for the same
    reason: ``gh run watch`` re-prints the **whole job tree** on every refresh,
    so called bare it lands one enormous result in context. A single such call
    emitted **64.6KB**, overflowed the tool-result cap, was persisted to disk,
    and the terminal state had to be re-probed afterwards anyway — the largest
    single result of that session, fetched twice.

    ``--exit-status`` makes a failed run a non-zero exit, so a dequeue can never
    read as a merge. On timeout the child is killed and ``settled`` is ``False``;
    the exit code is then meaningless and reported as ``-1``.

    ``interval`` is passed through to ``gh run watch -i``. It has to be: gh's
    own default is **3 seconds**, so a caller throttling a long merge-queue wait
    would otherwise get 3s polling and no diagnostic — the silently-ignored-flag
    failure this module rejects elsewhere by construction.
    """
    args = [
        "run",
        "watch",
        run_id,
        "--repo",
        repo,
        "--exit-status",
        "--interval",
        str(interval),
    ]
    try:
        fd = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    except OSError as exc:
        raise WaitForChecksError(f"cannot write {log}: {exc}") from exc
    with os.fdopen(fd, "w", encoding="utf-8", errors="replace") as fh:
        try:
            proc = subprocess.Popen(
                ["gh", *args], stdout=fh, stderr=subprocess.STDOUT, text=True
            )
        except OSError as exc:
            raise WaitForChecksError(f"cannot run gh: {exc}") from exc
        try:
            proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
            return False, -1
    return True, proc.returncode


def wait_run(
    run_id: str,
    repo: str = DEFAULT_REPO,
    timeout: int = DEFAULT_TIMEOUT,
    interval: int = DEFAULT_INTERVAL,
):
    """Watch one workflow run and return the same ``conclusion`` contract as
    :func:`wait` — **not** the same verdict shape.

    Only ``conclusion`` / ``settled`` / ``elapsed_seconds`` / ``log_path`` are
    common. This returns ``run_id`` and ``exit_code``; :func:`wait` returns
    ``pr``, ``counts``, ``failing``, and ``unresolved_buckets``. A caller that
    reads ``verdict["failing"]`` off this one gets a ``KeyError``, so branch on
    ``conclusion`` and nothing else if you handle both.

    ``conclusion`` is ``pass`` / ``fail`` / ``timeout`` — there is no ``pending``,
    because the watch only returns once the run is terminal.
    """
    log = run_log_path_for(run_id)
    started = time.monotonic()
    settled, code = watch_run(run_id, repo, timeout, log, interval)
    elapsed = int(time.monotonic() - started)

    if not settled:
        conclusion = "timeout"
    elif code == 0:
        conclusion = "pass"
    else:
        conclusion = "fail"

    return {
        "run_id": run_id,
        "repo": repo,
        "conclusion": conclusion,
        "settled": settled,
        "elapsed_seconds": elapsed,
        "exit_code": code,
        "log_path": str(log),
    }


def queue_log_path_for(pr: int) -> Path:
    """Where a ``--queue`` watch's one-line-per-poll log goes."""
    base = Path(tempfile.gettempdir()) / "claude-wait-checks"
    base.mkdir(mode=0o700, parents=True, exist_ok=True)
    return base / f"wait-for-queue-{pr}.log"


# One GraphQL read answers "landed?", "still queued?" and "dequeued?" at once.
# `mergeQueueEntry` is the queue signal: on a merge-queue repo a genuinely
# queued PR reports `autoMergeRequest: null`, so keying on the latter announced
# a dequeue on every run. `autoMergeRequest` stays for classic auto-merge.
QUEUE_QUERY = """
query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      state
      merged
      mergeQueueEntry { state enqueuedAt }
      autoMergeRequest { enabledAt }
    }
  }
}
"""

# Queue state moves on the scale of a whole CI run behind other branches, so a
# poll a minute is ample; each poll is three small reads, written to the log.
QUEUE_INTERVAL = 60

# A queue wait sits behind every branch ahead of it, so it gets a longer outer
# bound than one CI run does.
QUEUE_TIMEOUT = 7200

# All-null with no queue-branch run for this long means the enqueue never took.
# Registration lands within seconds of a good enqueue, so this is generous.
NOT_QUEUED_AFTER_SECONDS = 180

# All-null after the entry demonstrably existed, with no failed queue run to
# explain it. Both state sources were once stale together for several minutes
# after a real merge, so wait this long before calling it a dequeue.
AMBIGUOUS_GRACE_SECONDS = 600

# A dequeue backed by a failed queue run must hold on this many consecutive
# polls, so one stale read of both sources cannot turn a merge into a dequeue.
DEQUEUE_CONFIRM_POLLS = 2

# With no queue entry seen yet to anchor on, how far back a queue-branch run
# may have been created and still count as this enqueue's.
RUN_WINDOW_SECONDS = 900

# Consecutive probe failures before the watch gives up rather than spinning.
MAX_PROBE_ERRORS = 3

# Run conclusions that dequeue a PR. Keyed on the conclusion, not `status`.
FAILED_RUN_CONCLUSIONS = frozenset(
    {"failure", "cancelled", "timed_out", "startup_failure", "action_required"}
)


def _parse_time(text: str | None) -> float | None:
    """An ISO-8601 GitHub timestamp as epoch seconds, or ``None``."""
    if not text:
        return None
    try:
        return datetime.fromisoformat(text.replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


def _gh_json(args: list[str], what: str):
    code, out, err = _gh(args)
    if code != 0:
        raise WaitForChecksError(f"{what} failed: {err.strip() or f'exit {code}'}")
    try:
        return json.loads(out)
    except json.JSONDecodeError as exc:
        raise WaitForChecksError(f"decoding {what}: {exc}") from exc


def probe_queue(pr: int, repo: str) -> dict:
    """One snapshot of a queued PR: both state sources plus its queue runs.

    Two sources because they disagree in **both** directions: GraphQL once read
    ``MERGED`` while ``gh pr view`` still said ``OPEN``, and once still read
    ``AWAITING_CHECKS`` after ``gh pr view`` said ``MERGED``.
    """
    owner, _, name = repo.partition("/")
    gql = _gh_json(
        [
            "api",
            "graphql",
            "-f",
            f"owner={owner}",
            "-f",
            f"name={name}",
            "-F",
            f"number={pr}",
            "-f",
            f"query={QUEUE_QUERY}",
        ],
        "queue graphql probe",
    )
    try:
        node = gql["data"]["repository"]["pullRequest"]
    except (KeyError, TypeError) as exc:
        raise WaitForChecksError("queue graphql probe returned no PR") from exc
    if not isinstance(node, dict):
        raise WaitForChecksError("queue graphql probe returned no PR")
    view = _gh_json(
        ["pr", "view", str(pr), "--repo", repo, "--json", "state,mergedAt"],
        "gh pr view",
    )
    if not isinstance(view, dict):
        raise WaitForChecksError("gh pr view did not return an object")
    # `--event merge_group` narrows server-side, so the limit spans queue runs
    # only rather than every push on a busy repo.
    runs = _gh_json(
        [
            "run",
            "list",
            "--repo",
            repo,
            "--event",
            "merge_group",
            "--limit",
            "100",
            "--json",
            "databaseId,headBranch,name,status,conclusion,createdAt,url",
        ],
        "gh run list",
    )
    if not isinstance(runs, list):
        raise WaitForChecksError("gh run list did not return a list")
    marker = f"/pr-{pr}-"
    queue_runs = [
        {
            "run_id": str(r.get("databaseId", "")),
            "head_branch": str(r.get("headBranch", "")),
            "name": r.get("name", ""),
            "status": r.get("status", ""),
            "conclusion": r.get("conclusion", ""),
            "created_at": r.get("createdAt", ""),
            "url": r.get("url", ""),
        }
        for r in runs
        if isinstance(r, dict)
        and str(r.get("headBranch", "")).startswith("gh-readonly-queue/")
        and marker in str(r.get("headBranch", ""))
    ]
    return {
        "gql_state": node.get("state"),
        "gql_merged": bool(node.get("merged")),
        "entry": node.get("mergeQueueEntry"),
        "auto": node.get("autoMergeRequest"),
        "view_state": view.get("state"),
        "view_merged_at": view.get("mergedAt"),
        "queue_runs": queue_runs,
    }


def _failing_on_latest(runs: list[dict]) -> list[dict]:
    """Failed runs on this PR's NEWEST queue branch only.

    The queue rebuilds a group on a new SHA and cancels the old group's runs,
    so a failure or cancellation on an older ``pr-<n>-<sha>`` branch is no
    evidence about the current entry — counting it once could turn a real
    merge, read stale by both sources, into a dequeue.
    """
    if not runs:
        return []
    latest = max(runs, key=lambda r: _parse_time(r.get("created_at")) or 0.0)
    branch = latest.get("head_branch")
    return [
        r
        for r in runs
        if r.get("head_branch") == branch
        and r.get("conclusion") in FAILED_RUN_CONCLUSIONS
    ]


def wait_queue(
    pr: int,
    repo: str = DEFAULT_REPO,
    interval: int = QUEUE_INTERVAL,
    timeout: int = QUEUE_TIMEOUT,
    probe=probe_queue,
    sleep=time.sleep,
    clock=time.monotonic,
    wall=time.time,
    since: str | None = None,
):
    """Watch an enqueued PR until it merges or leaves the queue.

    Built to run as **one backgrounded command** right after a verified
    enqueue: it prints nothing until a terminal outcome, so the harness's
    exit notification is the session's wake-up. That replaces re-probing
    across turns, which nothing woke the session for, and a session-scoped
    cron, which the Bedrock auto-mode classifier can deny as persistence.

    ``conclusion`` is ``merged`` (exit 0), or ``dequeued`` / ``closed`` /
    ``not_queued`` / ``timeout`` / ``probe_failed`` (exit 1). A terminal
    ``MERGED`` from **either** source wins; nothing else concludes until both
    sources agree the PR is not merged.

    ``since`` (the enqueue time, ISO-8601) anchors the run window when no
    entry has been seen in *this* call — which is every firing of a cron
    fallback. Without it a fresh call keeps only runs from the last
    :data:`RUN_WINDOW_SECONDS`, so an older dequeue reads as ``not_queued``.
    """
    log = queue_log_path_for(pr)
    started = clock()
    started_wall = wall()
    seen_entry = False
    anchor: float | None = _parse_time(since)
    null_since: float | None = None
    dequeue_streak = 0
    probe_errors = 0
    polls = 0
    snap: dict = {}
    conclusion = "timeout"
    error = None

    fd = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8", errors="replace") as fh:
        while True:
            now = clock()
            polls += 1
            fresh = False
            try:
                snap = probe(pr, repo)
                probe_errors = 0
                fresh = True
            except WaitForChecksError as exc:
                probe_errors += 1
                error = str(exc)
                fh.write(json.dumps({"t": int(now - started), "error": error}) + "\n")
                fh.flush()
                if probe_errors >= MAX_PROBE_ERRORS:
                    conclusion = "probe_failed"
                    break
                # An unread poll breaks the "consecutive" in the confirmation.
                # The last good snapshot is kept for the verdict's report.
                dequeue_streak = 0

            if fresh:
                entry = snap.get("entry")
                runs = snap.get("queue_runs", [])
                enqueued = _parse_time((entry or {}).get("enqueuedAt"))
                if enqueued is not None and (anchor is None or enqueued < anchor):
                    anchor = enqueued
                floor = (
                    anchor if anchor is not None else started_wall - RUN_WINDOW_SECONDS
                )
                runs = [
                    r
                    for r in runs
                    if (_parse_time(r.get("created_at")) or floor) >= floor
                ]
                snap["queue_runs"] = runs
                failing = _failing_on_latest(runs)
                fh.write(
                    json.dumps(
                        {
                            "t": int(now - started),
                            "gql": snap.get("gql_state"),
                            "entry": (entry or {}).get("state"),
                            "view": snap.get("view_state"),
                            "runs": len(runs),
                            "failing": len(failing),
                        }
                    )
                    + "\n"
                )
                fh.flush()

                merged = (
                    snap.get("gql_merged")
                    or snap.get("gql_state") == "MERGED"
                    or snap.get("view_state") == "MERGED"
                    or bool(snap.get("view_merged_at"))
                )
                if merged:
                    conclusion = "merged"
                    break
                if "CLOSED" in (snap.get("gql_state"), snap.get("view_state")):
                    conclusion = "closed"
                    break
                # `autoMergeRequest` keeps classic auto-merge (no queue, so no
                # queue runs) waiting, but never outranks a failed queue run.
                if entry or (snap.get("auto") and not failing):
                    seen_entry = True
                    null_since = None
                    dequeue_streak = 0
                else:
                    if null_since is None:
                        null_since = now
                    if failing:
                        dequeue_streak += 1
                        if dequeue_streak >= DEQUEUE_CONFIRM_POLLS:
                            conclusion = "dequeued"
                            break
                    else:
                        dequeue_streak = 0
                        if seen_entry or runs:
                            if now - null_since >= AMBIGUOUS_GRACE_SECONDS:
                                conclusion = "dequeued"
                                break
                        elif now - null_since >= NOT_QUEUED_AFTER_SECONDS:
                            conclusion = "not_queued"
                            break

            if clock() - started + interval > timeout:
                break
            sleep(interval)

    runs = snap.get("queue_runs", [])
    return {
        "pr": pr,
        "repo": repo,
        "mode": "queue",
        "conclusion": conclusion,
        "settled": conclusion not in ("timeout", "probe_failed"),
        "elapsed_seconds": int(clock() - started),
        "polls": polls,
        "seen_queue_entry": seen_entry,
        "last": {
            "gql_state": snap.get("gql_state"),
            "entry_state": (snap.get("entry") or {}).get("state"),
            "view_state": snap.get("view_state"),
            "view_merged_at": snap.get("view_merged_at"),
        },
        "queue_runs": runs,
        "failing_runs": _failing_on_latest(runs),
        # The last probe error, whatever the conclusion: a timeout reached
        # through failed probes must not read as a quiet queue.
        "error": error,
        "log_path": str(log),
    }


# `all=true`: the default lists UNREAD threads only, and a thread read but never
# marked done still sits in the inbox.
NOTIFICATIONS_PATH = "/notifications?all=true&per_page=50"


def dismiss_pr_notifications(pr: int, repo: str) -> dict:
    """Mark every notification thread for ``repo``'s PR ``pr`` as done.

    Called on a ``merged`` queue verdict. ``DELETE`` on a thread is GitHub's
    "mark as done"; marking it read would leave it in the inbox. Never raises:
    the merge already happened, so a failed dismissal is reported, and
    the upkeep pass's finished-PR notification sweep is the fallback.
    """
    suffix = f"/repos/{repo}/pulls/{pr}"
    try:
        threads = _gh_json(["api", NOTIFICATIONS_PATH], "notification list")
    except WaitForChecksError as exc:
        return {"dismissed": 0, "failed": 0, "error": str(exc)}
    ids = [
        str(t["id"])
        for t in threads or []
        if isinstance(t, dict)
        and ((t.get("subject") or {}).get("url") or "").endswith(suffix)
    ]
    failed = 0
    error = None
    for tid in ids:
        try:
            code, _, err = _gh(["api", "-X", "DELETE", f"/notifications/threads/{tid}"])
        except WaitForChecksError as exc:
            code, err = 1, str(exc)
        if code != 0:
            failed += 1
            error = err.strip() or f"exit {code}"
    return {"dismissed": len(ids) - failed, "failed": failed, "error": error}


def read_checks(pr: int, repo: str) -> list[dict]:
    """The PR's checks as a list of dicts, via one ``gh pr checks --json`` read.

    An empty list means the PR has no checks at all, which ``gh`` signals with a
    non-zero exit and an empty payload — distinguished from failure by the
    caller, not conflated with it.
    """
    code, out, err = _gh(
        ["pr", "checks", str(pr), "--repo", repo, "--json", JSON_FIELDS]
    )
    text = out.strip()
    if not text:
        # An empty payload is either "this PR genuinely has no checks" or a real
        # failure (bad PR number, no auth, no network) — and the two must not be
        # conflated, because a caller is told to treat "no checks" as green.
        #
        # gh's exit codes are documented as 0 (all passed), 1 (a check failed),
        # and 8 (pending), with "no checks reported" surfaced as an error whose
        # code has varied across versions. So don't key the distinction on the
        # code: key it on gh's own message, and fail **loud** for anything else.
        if code == 0 or "no checks" in err.lower():
            return []
        detail = err.strip() or f"exit {code}"
        raise WaitForChecksError(f"gh pr checks --json failed: {detail}")
    try:
        parsed = json.loads(text)
    except json.JSONDecodeError as exc:
        raise WaitForChecksError(f"decoding gh pr checks --json: {exc}") from exc
    if not isinstance(parsed, list):
        raise WaitForChecksError("gh pr checks --json did not return a list")
    return parsed


def run_id_from_link(link: str) -> str:
    """The Actions run id embedded in a check's link, or ``""``.

    Handed through so a failing check can be passed straight to the GitHub MCP's
    ``get_job_logs`` without the caller re-parsing the URL.
    """
    match = _RUN_ID_RE.search(link or "")
    return match.group(1) if match else ""


def classify_step(name: str) -> str:
    """``lint_hook`` / ``test_runner`` / ``single_command`` for a step name."""
    if _LINT_STEP_RE.search(name or ""):
        return "lint_hook"
    if _TEST_STEP_RE.search(name or ""):
        return "test_runner"
    return "single_command"


def next_command(kind: str, repo: str, job_id: str) -> str | None:
    """The one command that reads a failing step, routed on its ``kind``.

    ``None`` for an ``external`` check: its link is set by a third-party
    provider, so it is reported as ``link`` and never handed over as a command.
    A ``never_ran`` job has no failed step, so ``--log-failed`` would print
    nothing; it gets the whole job log.
    """
    if kind == "lint_hook":
        return (
            "python3 .claude/tools/run_quiet.py -- python3 .claude/tools/lint_paths.py"
        )
    if kind == "external":
        return None
    which = "--log" if kind == "never_ran" else "--log-failed"
    return (
        f"python3 .claude/tools/run_quiet.py -- "
        f"gh run view --job {job_id} --repo {repo} {which}"
    )


def _fetch_json(args: list[str]):
    """``(payload, error)`` for a ``gh api`` read; exactly one is ``None``.

    Empty stdout is a **fetch failure**, never an empty answer: an Actions
    log read through ``gh api`` returns nothing at all (history.md →
    "Empty-stdout scan"), and reading that as "no failed step" hides the cause.
    """
    code, out, err = _gh(args)
    if code != 0:
        return None, err.strip() or f"exit {code}"
    if not out.strip():
        return None, "empty stdout"
    try:
        return json.loads(out), None
    except json.JSONDecodeError as exc:
        return None, f"undecodable: {exc}"


def _fetch_failed(check: str, run_id: str, repo: str, error: str) -> dict:
    return {
        "check": check,
        "run_id": run_id,
        "kind": "fetch_failed",
        "error": error,
        "next_command": (
            f"python3 .claude/tools/run_quiet.py -- "
            f"gh run view {run_id} --repo {repo} --log-failed"
        ),
    }


def _job_records(job: dict, repo: str, check: str, run_id: str) -> list[dict]:
    job_id = str(job.get("id", ""))
    if not job_id.isdigit():
        return [_fetch_failed(check, run_id, repo, f"job id {job_id!r} is not numeric")]
    failed = [
        s
        for s in job.get("steps") or []
        if isinstance(s, dict) and s.get("conclusion") in FAILED_STEP_CONCLUSIONS
    ]
    base = {"check": check, "job": job.get("name") or "", "job_id": job_id}
    if not failed:
        return [
            {
                **base,
                "step": None,
                "conclusion": job.get("conclusion"),
                "kind": "never_ran",
                "next_command": next_command("never_ran", repo, job_id),
                "note": STEP_KIND_NOTES["never_ran"],
            }
        ]
    records = []
    for step in failed:
        kind = classify_step(step.get("name") or "")
        records.append(
            {
                **base,
                "step": step.get("name") or "",
                "conclusion": step.get("conclusion"),
                "kind": kind,
                "next_command": next_command(kind, repo, job_id),
                "note": STEP_KIND_NOTES[kind],
            }
        )
    return records


def diagnose_failures(failing: list[dict], repo: str) -> list[dict]:
    """One failed-step record per failing check: the step, its kind, and the
    one next command — the step conclusions read **before** any log, since
    they are bounded by step count and the step's name is often the diagnosis.

    A check's link names its job (``/runs/<run>/job/<job>``), so one read per
    check returns exactly that job's steps; a link naming only a run falls back
    to the run's failed jobs. A fetch that fails is recorded as ``fetch_failed``
    with its error — never dropped, never read as "nothing found". Only a link
    into this repo's Actions pages counts as an Actions check; any other link
    is ``external``, so a third-party URL can never select a job to read.
    """
    records: list[dict] = []
    actions_prefix = f"https://github.com/{repo}/actions/"
    for f in failing:
        link = f.get("link") or ""
        run_id = f.get("run_id") or ""
        check = f.get("name") or ""
        if not run_id or not link.startswith(actions_prefix):
            records.append(
                {
                    "check": check,
                    "kind": "external",
                    "link": link,
                    "next_command": None,
                    "note": STEP_KIND_NOTES["external"],
                }
            )
            continue
        job_match = _JOB_ID_RE.search(link)
        if job_match:
            payload, error = _fetch_json(
                ["api", f"repos/{repo}/actions/jobs/{job_match.group(1)}"]
            )
            if error is None and not isinstance(payload, dict):
                error = "unexpected job payload shape"
            jobs = [payload] if error is None else []
        else:
            payload, error = _fetch_json(
                ["api", f"repos/{repo}/actions/runs/{run_id}/jobs?per_page=100"]
            )
            listed = payload.get("jobs") if isinstance(payload, dict) else None
            if error is None and not isinstance(listed, list):
                error = "unexpected jobs payload shape"
            jobs = [
                j
                for j in listed or []
                if isinstance(j, dict) and j.get("conclusion") in FAILED_RUN_CONCLUSIONS
            ]
            if error is None and not jobs:
                error = "no failed job in the run"
        if error is not None:
            records.append(_fetch_failed(check, run_id, repo, error))
            continue
        for job in jobs:
            records.extend(_job_records(job, repo, check, run_id))
    return records


def action_hint(verdict: dict) -> dict:
    """What the conclusion obliges the caller to do, and the evidence for it.

    ``gate`` is ``green`` / ``blocking`` / ``unverified`` / ``retry``. Only
    ``green`` is a gate pass: a state the tool could not verify never is.
    """
    c = verdict["conclusion"]
    counts = verdict.get("counts") or {}
    rounds = verdict.get("watch_rounds")
    if c == "pass":
        return {
            "gate": "green",
            "do": "print the summary",
            "evidence": f"counts {counts}",
        }
    if c == "none":
        evidence = (
            f"no check registered over {rounds} round(s); mergeable "
            f"{verdict.get('mergeable')}"
        )
        # A conflicting PR produces no run at all, so `none` is green only once
        # the probe has RULED OUT a conflict; a failed or unsettled probe has not.
        if verdict.get("mergeable_error") or (
            (verdict.get("mergeable") or "").upper() != "MERGEABLE"
        ):
            return {
                "gate": "unverified",
                "do": "conflict not ruled out; re-run --mergeable before trusting none",
                "evidence": evidence
                + (
                    f", probe error: {verdict['mergeable_error']}"
                    if verdict.get("mergeable_error")
                    else ""
                ),
            }
        return {
            "gate": "green",
            "do": "no checks apply; treat as green",
            "evidence": evidence,
        }
    if c == "conflicting":
        return {
            "gate": "blocking",
            "do": "rebase onto origin/<base>, push, re-run the wait; never green",
            "evidence": f"mergeable CONFLICTING ({verdict.get('merge_state_status')})",
        }
    if c == "fail":
        return {
            "gate": "blocking",
            "do": (
                "catalogue each failing check as blocking, run each "
                "failed_steps[].next_command, re-draft the PR"
            ),
            "evidence": ", ".join(f["name"] for f in verdict.get("failing", [])),
        }
    if c == "pending":
        return {
            "gate": "retry",
            "do": "re-run without --no-watch",
            "evidence": f"pending: {verdict.get('pending_checks')}",
        }
    # `timeout`, or `blocked` on a bucket gh grew: never a pass.
    return {
        "gate": "unverified",
        "do": "treat as unverified; report the pending or unresolved checks",
        "evidence": (
            f"pending: {verdict.get('pending_checks')}; unresolved buckets: "
            f"{verdict.get('unresolved_buckets')}"
        ),
    }


def summarize(checks: list[dict]) -> dict:
    """Reduce the raw check list to counts, failures, and one conclusion.

    The conclusion is derived in a fixed precedence — ``fail`` beats ``pending``
    beats ``pass`` — so a run with one red check and one still-queued check reads
    as failing rather than as "not done yet".

    **Only ``pass`` and ``skipping`` count as green.** ``skipping`` is exempt on
    purpose: a path-filtered no-op job is the normal case on this repo
    (``.github/workflows/test.yml`` filters on ``pull_request``), not a problem.
    Nothing else gets that exemption — in particular a **cancelled** check
    (which still blocks the merge queue) and an **unrecognized** bucket are
    treated as not-green, because the promise this module makes is that a caller
    checking only the exit status cannot mistake a red build for green. An
    unknown bucket is a gh schema change, and defaulting it to green would break
    that promise silently.
    """
    counts: dict[str, int] = {}
    failing: list[dict] = []
    for check in checks:
        bucket = (check.get("bucket") or "unknown").lower()
        counts[bucket] = counts.get(bucket, 0) + 1
        if bucket == "fail":
            link = check.get("link") or ""
            failing.append(
                {
                    "name": check.get("name") or "",
                    "workflow": check.get("workflow") or "",
                    "description": check.get("description") or "",
                    "link": link,
                    "run_id": run_id_from_link(link),
                }
            )
    failing.sort(key=lambda f: (f["workflow"], f["name"]))

    # Name what is still outstanding, not just how many. A pending verdict whose
    # only detail is a count sends the reader back to `gh` to find out which
    # check it is waiting on — which is the hand-diagnosis this tool replaces.
    pending_checks = sorted(
        (check.get("name") or "")
        for check in checks
        if (check.get("bucket") or "").lower() == "pending"
    )

    # Anything that is not `pass` or `skipping` — a cancelled check, or a bucket
    # gh grew since this was written — blocks the build, so it must not fall
    # through to `pass`. Reported under its own conclusion so the caller can tell
    # "red" from "I don't recognize this".
    unresolved = sorted(
        name for name in counts if name not in ("pass", "skipping", "fail", "pending")
    )

    if not checks:
        conclusion = "none"
    elif counts.get("fail"):
        conclusion = "fail"
    elif counts.get("pending"):
        conclusion = "pending"
    elif unresolved:
        conclusion = "blocked"
    else:
        conclusion = "pass"

    return {
        "conclusion": conclusion,
        "counts": counts,
        "failing": failing,
        "pending_checks": pending_checks,
        "unresolved_buckets": unresolved,
    }


def mergeability(pr: int, repo: str) -> dict:
    """``{"mergeable": …, "merge_state_status": …, "error": …}`` for one PR.

    Read **field-selected**, because the only two fields wanted here sit beside
    collection-valued ones (``files``, ``commits``) that would reintroduce the
    payload this tool exists to avoid.
    """
    code, out, err = _gh(
        [
            "pr",
            "view",
            str(pr),
            "--repo",
            repo,
            "--json",
            "mergeable,mergeStateStatus",
        ]
    )
    if code != 0:
        return {"mergeable": None, "merge_state_status": None, "error": err.strip()}
    try:
        payload = json.loads(out or "{}")
    except json.JSONDecodeError as exc:
        return {"mergeable": None, "merge_state_status": None, "error": str(exc)}
    return {
        "mergeable": payload.get("mergeable"),
        "merge_state_status": payload.get("mergeStateStatus"),
        "error": None,
    }


def settle_mergeability(
    pr: int, repo: str, retries: int, delay: float, sleep=time.sleep
) -> tuple[dict, int]:
    """:func:`mergeability`, re-read while it says ``UNKNOWN``.

    GitHub computes mergeability **asynchronously**, and right after a push
    ``UNKNOWN`` is the common answer; concluding from it is the failure being
    avoided. Returns the last read and how many reads it took.
    """
    state = mergeability(pr, repo)
    reads = 1
    while (state.get("mergeable") or "").upper() == "UNKNOWN" and reads <= retries:
        sleep(delay)
        state = mergeability(pr, repo)
        reads += 1
    return state, reads


def wait_mergeable(
    pr: int,
    repo: str = DEFAULT_REPO,
    retries: int = MERGEABLE_SETTLE_RETRIES,
    delay: float = MERGEABLE_SETTLE_DELAY,
    sleep=time.sleep,
) -> dict:
    """The mergeability tri-state as one verdict — ``review-pr`` steps 16 and 20.

    ``conclusion`` keys on ``mergeable``, the gh twin of the MCP
    ``mergeable_state``: ``mergeable`` (any ``mergeStateStatus`` — ``BLOCKED``
    / ``UNSTABLE`` mean checks or review outstanding, ``BEHIND`` that the base
    moved), ``conflicting`` (or ``DIRTY``) — blocking, and never auto-resolved
    — ``unknown`` once the settle reads run out, and ``probe_failed``. Only
    ``mergeable`` exits 0.
    """
    state, reads = settle_mergeability(pr, repo, retries, delay, sleep)
    value = (state.get("mergeable") or "").upper()
    status = state.get("merge_state_status")
    if state.get("error"):
        conclusion = "probe_failed"
        hint = {
            "gate": "unverified",
            "do": "re-run; never read a failed probe as clean",
        }
    elif value == "CONFLICTING" or (status or "").upper() == "DIRTY":
        conclusion = "conflicting"
        hint = {
            "gate": "blocking",
            "do": (
                "do not mark ready or enqueue; rebase onto origin/<base>, "
                "resolve, re-run /review-pr — never auto-resolve"
            ),
        }
    elif value == "MERGEABLE":
        conclusion = "mergeable"
        hint = {"gate": "green", "do": "proceed"}
    else:
        conclusion = "unknown"
        hint = {"gate": "unverified", "do": "still computing after the settle; hold"}
    hint["evidence"] = f"mergeable {state.get('mergeable')}, {status}, {reads} read(s)"
    return {
        "pr": pr,
        "repo": repo,
        "mode": "mergeable",
        "conclusion": conclusion,
        "mergeable": state.get("mergeable"),
        "merge_state_status": status,
        "reads": reads,
        "error": state.get("error"),
        "hint": hint,
    }


def _git(args: list[str], cwd: str | None = None) -> tuple[int, bytes, str]:
    """``(returncode, stdout bytes, stderr text)``. Bytes, because a patch is
    bytes: text mode's universal newlines strip a CRLF file's ``\\r`` and its
    replacement decoding corrupts non-UTF-8 content, and either breaks the
    ``git apply`` the spill exists for."""
    try:
        completed = subprocess.run(
            ["git", *args],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            cwd=cwd,
            check=False,
        )
    except OSError as exc:
        return 127, b"", str(exc)
    return (
        completed.returncode,
        completed.stdout,
        completed.stderr.decode("utf-8", errors="replace"),
    )


def snapshot(path: Path, base: str) -> dict:
    """Write the working diff — committed, staged, unstaged AND untracked —
    against the merge base with ``origin/<base>``, so recovery from a worktree
    deleted under the wait is one ``git apply`` on that base.

    Diffing committed history alone omits exactly the work at risk: a signing
    failure leaves the fixes uncommitted (history.md → "Worktree deleted under
    a wait"). Every git call's status is checked: a failure is reported in
    ``error``, never a silently shorter spill. Git runs from the top level, so
    the untracked listing covers the whole tree whatever the caller's cwd.
    """
    path = Path(path).resolve()
    ref = f"origin/{base}"

    def failed(what: str, err: str, code: int) -> dict:
        return {
            "path": str(path),
            "base": ref,
            "error": f"{what}: {err.strip() or code}",
        }

    code, out, err = _git(["rev-parse", "--show-toplevel"])
    if code != 0:
        return failed("rev-parse", err, code)
    top = out.decode().strip()
    code, out, err = _git(["merge-base", ref, "HEAD"], top)
    if code != 0:
        return failed("merge-base", err, code)
    merge_base = out.decode().strip()
    diff_flags = ["diff", "--binary", "--no-ext-diff", "--no-color"]
    code, diff, err = _git([*diff_flags, merge_base], top)
    if code != 0:
        return failed("diff", err, code)
    code, names, err = _git(["diff", "--name-only", "-z", "HEAD"], top)
    if code != 0:
        return failed("diff --name-only", err, code)
    code, others, err = _git(["ls-files", "-z", "--others", "--exclude-standard"], top)
    if code != 0:
        return failed("ls-files", err, code)
    untracked = [os.fsdecode(p) for p in others.split(b"\0") if p]
    parts = [diff]
    for rel in untracked:
        # `--no-index` exits 1 whenever the two sides differ, which they do;
        # `--` keeps a file named like an option from being parsed as one.
        code, text, err = _git([*diff_flags, "--no-index", "--", "/dev/null", rel], top)
        if code not in (0, 1) or not text:
            return failed(rel, err or "empty diff", code)
        parts.append(text)
    body = b"".join(parts)
    try:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "wb") as fh:
            fh.write(body)
    except OSError as exc:
        return {"path": str(path), "base": ref, "error": str(exc)}
    return {
        "path": str(path),
        "base": ref,
        "merge_base": merge_base,
        "bytes": len(body),
        "uncommitted": [p for p in names.decode(errors="replace").split("\0") if p],
        "untracked": untracked,
        "error": None,
    }


def wait(
    pr: int,
    repo: str = DEFAULT_REPO,
    interval: int = DEFAULT_INTERVAL,
    timeout: int = DEFAULT_TIMEOUT,
    watch: bool = True,
    snapshot_path: Path | None = None,
    base: str = "main",
) -> dict:
    """The whole wait: watch, read, and re-watch while the read says pending.

    With ``snapshot_path``, :func:`snapshot` runs first, before anything can
    block. On ``fail``, ``failed_steps`` carries :func:`diagnose_failures`;
    every verdict carries :func:`action_hint`.

    ``settled`` means *the checks are done*, never merely *gh returned*. Those
    two came apart when a late-registering workflow left a pending check behind
    an exited watch, and reporting the exit as settled produced a verdict that
    contradicted its own counts. So the watch is re-entered while the read still
    says pending, bounded by :data:`MAX_WATCH_ROUNDS` and by the caller's
    ``timeout``; exhausting either reports ``timeout``, which is the honest
    answer, rather than a settled pending, which is not an answer at all.

    ``pending`` and ``none`` count their rounds separately, so the loop's own
    worst case is their sum (8) rather than either cap — still finite, and the
    ``timeout`` deadline bounds it independently. The returned ``watch_rounds``
    is that shared total, which is why it can exceed
    :data:`MAX_WATCH_ROUNDS`.

    **``none`` is re-entered on the same footing, and it is the more dangerous
    of the two.** ``pending`` fails *safe* — keep waiting — while ``none`` fails
    **open**: it declares a commit CI-clean. Measured: a wait started in the
    tool call right after a push returned in **1 second** with
    ``conclusion: "none"``, ``counts: {}``, ``settled: true`` and
    ``mergeable: "MERGEABLE"``, while four runs were in fact already starting
    and ``Semantic PR`` had already completed on that exact SHA. Taking that
    literally treats a CI-unverified commit as green (history.md →
    "Registration race"). A ``none`` that still looks wrong after the rounds
    is told apart from the race by one ``gh run list --branch <branch>``
    filtered to the head SHA.

    A commit that genuinely has no checks still reports ``none`` once the
    rounds are spent, **not** ``timeout`` — the rounds established the absence
    rather than timing out on a wedged run, and collapsing the two would trade
    one wrong answer for another.
    """
    log = log_path_for(pr)
    snap = snapshot(snapshot_path, base) if snapshot_path is not None else None
    started = time.monotonic()
    deadline = started + timeout
    settled = True
    rounds = 0
    summary: dict | None = None
    # Rounds spent PER retryable state, not one shared counter.
    #
    # A shared counter plus a per-state cap is a fail-open: a wait that read
    # `pending` twice and then `none` on the third round would compare
    # rounds(3) against the `none` cap(3), exit with `exhausted_on == "none"`,
    # and the terminal branch would then PRESERVE `none` — reporting a PR whose
    # CI was observed pending twice as having no checks at all. That is exactly
    # the fail-open this retry was added to close, arriving from the pending
    # side, so the counters are separated.
    state_rounds = {state: 0 for state in RETRY_CONCLUSIONS}
    # Which retryable state spent its own rounds, so an exhausted `none` can be
    # reported as `none` rather than collapsed into `timeout`.
    exhausted_on: str | None = None

    while watch:
        remaining = int(deadline - time.monotonic())
        if remaining <= 0:
            settled = False
            break
        rounds += 1
        settled = watch_checks(pr, repo, interval, remaining, log)
        summary = summarize(read_checks(pr, repo))
        state = summary["conclusion"]
        if not settled or state not in RETRY_CONCLUSIONS:
            break
        # The two retryable states get their own cap and pacing: a `none` is a
        # seconds-scale registration race, a `pending` is a whole CI run.
        state_rounds[state] += 1
        cap = MAX_NONE_ROUNDS if state == "none" else MAX_WATCH_ROUNDS
        if state_rounds[state] >= cap:
            # `none` survives the terminal branch only when EVERY round it saw
            # was `none`. A `none` that followed observed `pending` rounds is a
            # checks-disappeared anomaly, not an absence, and must not read as
            # green.
            exhausted_on = state if state_rounds.get("pending", 0) == 0 else None
            settled = False
            break
        # gh exits immediately when it sees nothing left to wait on, so pace the
        # re-entry rather than spinning through the cap in a few milliseconds.
        # Each round re-truncates the log; the last round's is the one that
        # describes the state actually being reported.
        pause = float(NONE_SETTLE_SECONDS if state == "none" else interval)
        time.sleep(max(0.0, min(pause, deadline - time.monotonic())))

    # `--no-watch`, or a timeout budget already spent before the first round.
    if summary is None:
        summary = summarize(read_checks(pr, repo))
    elapsed = int(time.monotonic() - started)

    conclusion = summary["conclusion"]

    # A CONFLICTING PR cannot produce ANY pull_request workflow run, because the
    # merge ref cannot be created. There is no run, no error, and nothing to
    # distinguish it from CI that has not started — one session spent ~20 minutes
    # and ~16 polls on exactly this, including an amend-and-force-push nudge at
    # an event that could never fire. So a bare `none` is ambiguous, and the
    # ambiguity is resolved here rather than left to the caller: `none` means
    # either no-checks-apply, or cannot-run-until-the-conflict-clears, and the
    # second is blocking.
    blocked_by_conflict = False
    merge_state: dict = {"mergeable": None, "merge_state_status": None, "error": None}
    if conclusion == "none":
        # Right after a push — precisely the window this probe runs in —
        # `UNKNOWN` is the common answer, and taking it as "not conflicting"
        # would reproduce the 20-minute dead wait this probe prevents.
        merge_state, _ = settle_mergeability(
            pr, repo, MERGEABLE_RETRIES, MERGEABLE_RETRY_DELAY, time.sleep
        )
        state = (merge_state.get("mergeable") or "").upper()
        if state == "CONFLICTING":
            blocked_by_conflict = True
            conclusion = "conflicting"

    if not settled and conclusion not in ("fail", "conflicting"):
        # A timed-out watch must never claim `pass` off a snapshot it stopped
        # waiting on. But a `fail` it *did* observe is definite and strictly more
        # informative than `timeout`, so that one survives — otherwise a caller
        # branching on `conclusion` can't tell a wedged run from a red one.
        #
        # An exhausted `none` also survives: the rounds were spent establishing
        # that no check ever registered, which is an answer, where `timeout`
        # would claim a wedged run that never existed. The conflict probe above
        # has already run on it, so a conflicting PR has been separated out.
        if not (exhausted_on == "none" and conclusion == "none"):
            conclusion = "timeout"

    verdict = {
        "pr": pr,
        "repo": repo,
        "conclusion": conclusion,
        "settled": settled,
        "elapsed_seconds": elapsed,
        "watch_rounds": rounds,
        "counts": summary["counts"],
        "failing": summary["failing"],
        "pending_checks": summary["pending_checks"],
        "unresolved_buckets": summary["unresolved_buckets"],
        # Only populated when the conclusion was `none`: the cause probe.
        "mergeable": merge_state["mergeable"],
        "merge_state_status": merge_state["merge_state_status"],
        # Surfaced rather than dropped: without it, "the probe failed" and "the
        # PR is not conflicting" both read as `mergeable: null`, so the header's
        # claim that `none` is disambiguated for you would be false with no
        # signal that it had not been.
        "mergeable_error": merge_state["error"],
        "blocked_by_conflict": blocked_by_conflict,
        "failed_steps": (
            diagnose_failures(summary["failing"], repo) if conclusion == "fail" else []
        ),
        "snapshot": snap,
        "log_path": str(log),
    }
    verdict["hint"] = action_hint(verdict)
    return verdict


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="wait_for_checks.py")
    parser.add_argument("--pr", type=int, help="PR number (checks mode)")
    parser.add_argument(
        "--run",
        dest="run_id",
        help="workflow run id to watch instead of a PR's checks (queue mode)",
    )
    parser.add_argument(
        "--queue",
        dest="queue_pr",
        type=int,
        help="enqueued PR number to watch until it merges or leaves the queue",
    )
    parser.add_argument(
        "--mergeable",
        dest="mergeable_pr",
        type=int,
        help="read a PR's mergeability tri-state once it settles",
    )
    parser.add_argument(
        "--snapshot",
        type=Path,
        help="with --pr: write the working diff, uncommitted work included, "
        "here before the watch starts",
    )
    parser.add_argument(
        "--base", default=None, help="with --snapshot: the base branch (main)"
    )
    parser.add_argument(
        "--verdict-out",
        type=Path,
        help="also write the verdict JSON here (review_summary.py reads it)",
    )
    parser.add_argument(
        "--since",
        help="with --queue: the enqueue time (ISO-8601), anchoring the run "
        "window for a fresh call such as a cron firing",
    )
    parser.add_argument("--repo", default=DEFAULT_REPO, help="owner/repo")
    parser.add_argument(
        "--interval",
        type=int,
        default=None,
        help=(
            f"seconds between refreshes (default {DEFAULT_INTERVAL}; "
            f"{QUEUE_INTERVAL} with --queue)"
        ),
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=None,
        help=(
            f"seconds before giving up on the watch (default {DEFAULT_TIMEOUT}; "
            f"{QUEUE_TIMEOUT} with --queue)"
        ),
    )
    parser.add_argument(
        "--no-watch",
        action="store_true",
        help="read the current state once instead of waiting",
    )
    args = parser.parse_args(argv[1:])

    modes = [args.pr, args.run_id, args.queue_pr, args.mergeable_pr]
    if sum(m is not None for m in modes) != 1:
        # Requiring exactly one keeps the modes from silently ranking each
        # other: a caller that passed two would otherwise watch whichever the
        # code happens to check first and report a verdict about the other thing.
        raise WaitForChecksError(
            "pass exactly one of --pr, --run, --queue or --mergeable"
        )
    if args.since is not None and args.queue_pr is None:
        raise WaitForChecksError("--since is meaningful only with --queue")
    if args.snapshot is not None and args.pr is None:
        raise WaitForChecksError("--snapshot is meaningful only with --pr")
    if args.base is not None and args.snapshot is None:
        raise WaitForChecksError("--base is meaningful only with --snapshot")

    if args.mergeable_pr is not None:
        if args.no_watch or args.interval is not None or args.timeout is not None:
            raise WaitForChecksError(
                "--no-watch, --interval and --timeout are meaningless with "
                "--mergeable, whose settle loop is fixed"
            )
        verdict = wait_mergeable(args.mergeable_pr, repo=args.repo)
    elif args.queue_pr is not None:
        if args.no_watch:
            raise WaitForChecksError("--no-watch is meaningless with --queue")
        if args.since is not None and _parse_time(args.since) is None:
            raise WaitForChecksError(f"--since is not an ISO-8601 time: {args.since}")
        verdict = wait_queue(
            args.queue_pr,
            repo=args.repo,
            interval=QUEUE_INTERVAL if args.interval is None else args.interval,
            timeout=QUEUE_TIMEOUT if args.timeout is None else args.timeout,
            since=args.since,
        )
        if verdict["conclusion"] == "merged":
            verdict["notifications"] = dismiss_pr_notifications(
                args.queue_pr, args.repo
            )
    else:
        interval = DEFAULT_INTERVAL if args.interval is None else args.interval
        timeout = DEFAULT_TIMEOUT if args.timeout is None else args.timeout
        if args.run_id is not None:
            if args.no_watch:
                raise WaitForChecksError("--no-watch is meaningless with --run")
            verdict = wait_run(
                args.run_id,
                repo=args.repo,
                timeout=timeout,
                interval=interval,
            )
        else:
            verdict = wait(
                args.pr,
                repo=args.repo,
                interval=interval,
                timeout=timeout,
                watch=not args.no_watch,
                snapshot_path=args.snapshot,
                base=args.base or "main",
            )
    # Stdout first: a verdict can take an hour to earn, and a bad
    # `--verdict-out` path must not throw it away.
    json.dump(verdict, sys.stdout, indent=2)
    sys.stdout.write("\n")
    if args.verdict_out is not None:
        try:
            args.verdict_out.write_text(json.dumps(verdict, indent=2) + "\n")
        except OSError as exc:
            raise WaitForChecksError(f"cannot write {args.verdict_out}: {exc}")
    return 0 if verdict["conclusion"] in ("pass", "merged", "mergeable") else 1


def main() -> int:
    try:
        return run(sys.argv)
    except WaitForChecksError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
