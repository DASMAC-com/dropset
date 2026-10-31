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

So the wait is a **bounded** pair — no model-driven loop:

1. A watch loop over the head SHA's check runs (:func:`watch_checks`) —
   conditional REST reads every :data:`DEFAULT_POLL` seconds, one log line
   per poll, never to stdout, for the same reason ``run_quiet.py`` captures a
   build log. It replaced a ``gh pr checks --watch`` child on a thirty-second
   refresh: an unchanged poll now answers ``304``, which the rate limit does
   not count (see :class:`Conditional`).
2. ``gh pr checks --json`` — one structured read that *is* the verdict, including
   every failing check's name, workflow, and link.

The JSON read, not the watch, is the authority on the outcome: the watch sees
check runs only, while the read also carries commit statuses and gh's
normalized ``bucket``, and a review must distinguish "a check failed", "checks
are still pending" and "there are no checks at all".

**One poll interval for every wait** — this loop, the ``--queue`` watch, and
``verify_head.py``'s CI poll: ``DS_PR_POLL_S`` seconds (default
:data:`DEFAULT_POLL`), ``--interval`` overriding it per call. Each REST
response's ``X-RateLimit-Remaining`` is read, and under
:data:`LOW_QUOTA_FRACTION` of the limit the wait backs off to
:data:`BACKOFF_INTERVAL` (:meth:`Conditional.pace`).

**The pair repeats when the read disagrees with the watch.** The watch settles
on its *own* census of the check set, which is not the same as the PR being
done: a workflow that registers late — one whose trigger matches a path only
some PRs touch — is still pending when the watch returns. Believing that single
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
no id to take). Every poll reads the REST pull (merged / closed) and the PR's
timeline (enqueue / dequeue events), both conditional; the GraphQL queue entry
and the queue-branch runs are corroboration, read once a minute. It prints
nothing until a terminal outcome, and is meant to run as one backgrounded
command so its exit wakes the session. See :func:`wait_queue` and
:class:`QueueProbe`.

A fourth, ``--mergeable <pr>``, is the mergeability tri-state with its settle
loop — one call serving ``review-pr`` steps 16 and 20. See :func:`wait_mergeable`.

A fifth, ``--enqueue <pr> --reviewed-fingerprint <fp>``, is ``review-pr`` step
20's enqueue: it refuses a PR whose diff moved since review, enqueues the way
the branch's rules want, and probes that the queue took it — one verdict where
an exit-0-is-not-proof story used to be. See :func:`enqueue`.

Usage::

    python3 .claude/tools/wait_for_checks.py --pr 285 \
      --snapshot <scratchpad>/pre-wait.diff --verdict-out <scratchpad>/ci.json
    python3 .claude/tools/wait_for_checks.py --pr 285 --interval 30 --timeout 1800
    python3 .claude/tools/wait_for_checks.py --run 1234567890
    python3 .claude/tools/wait_for_checks.py --queue 285
    python3 .claude/tools/wait_for_checks.py --mergeable 285
    python3 .claude/tools/wait_for_checks.py --enqueue 285 \
      --reviewed-fingerprint 3f1c…

Options: exactly one of ``--pr`` / ``--run`` / ``--queue`` / ``--mergeable`` /
``--enqueue`` (which requires ``--reviewed-fingerprint`` and refuses the
watch options),
``--repo`` (default ``DASMAC-com/dropset``), ``--verdict-out <path>`` (also
write the verdict there), ``--snapshot <path>`` with ``--base`` (checks mode:
see :func:`snapshot`),
``--interval`` (seconds between polls, default ``DS_PR_POLL_S`` or
:data:`DEFAULT_POLL`; honored in every watching mode), ``--timeout``
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
      "mergeable": null,         // probed on `none`, or a `dirty` watch read
      "merge_state_status": null,
      "blocked_by_conflict": false,
      "base_gained": null,       // on `conflicting` only, as in `dequeue`
      "base_gained_total": null,
      "base_gained_error": null,
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

A base move **during** the wait is caught too: the watch's own pull read
carries ``mergeable_state``, and a ``dirty`` one ends the watch early, so a
conflict never sits out the CI run it has already made moot. The verdict
settles it (:data:`MERGEABLE_SETTLE_RETRIES` reads, never concluding from
``UNKNOWN``) before reporting ``conflicting``. Every ``conflicting`` verdict
carries ``base_gained`` / ``base_gained_total`` / ``base_gained_error`` (see
:func:`base_gain`), so the rebase starts from it.

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
      "conclusion": "merged",    // merged | dequeued | conflicting | closed |
                                 // not_queued | auto_merge_waiting |
                                 // queued_no_group | timeout |
                                 // probe_failed | stopped
      "settled": true,
      "elapsed_seconds": 1310,
      "polls": 22,
      "seen_queue_entry": true,
      "enqueued_at": "2026-10-22T23:40:27Z",  // earliest enqueue seen, or --since
      "first_run_seen_at": "2026-10-22T23:41:02Z",  // a merge-group run, or null
      "last": {"rest_state": "MERGED", "gql_state": "MERGED",
               "entry_state": null, "entry_position": null,
               "auto_enabled_at": null, "merge_state_status": "CLEAN",
               "pending_checks": [{"name": "Explorer image", "required": false}],
               "removed_at": null, …},
      "queue_runs": [{"run_id": "…", "name": "…", "conclusion": "…", …}],
      "failing_runs": [],
      "requests": {"sent": 140, "not_modified": 131, "rate_remaining": 4710},
      "notifications": {"dismissed": 1, "unsubscribed": 1, "failed": 0,
                        "error": null},
      "todays_merge_count": 4,
      "since": "2026-10-22T23:40:20Z",
      "since_source": "auto_merge",  // flag | auto_merge | queue_entry | null
      "log_path": "/…/wait-for-queue-285.log"
    }

Every queue verdict is written in full to ``wait-for-queue-<pr>.json`` beside
the log. **On** ``merged`` **stdout is one line** instead — the merge, the
``Today's merge count is now N.`` sentence, the dismissal, and that path —
since the post-merge tail needs nothing more.

``since`` defaults, without ``--since``, to the PR's auto-merge time, else its
queue entry's (:func:`default_since`), so a cron firing carries no state of
its own. ``not_queued`` carries the run-list disambiguation
(:func:`not_queued_reading`)::

      "not_queued": {"reading": "never_entered",  // | prior_cycle_only
                     "prior_runs": 0, "newest_prior": null,
                     "prior_failing_runs": [],
                     // only when an earlier cycle failed: classified as a
                     // dequeue's would be, so a re-enqueue is never blind
                     "classification": {"cause": "integration", …}}

``notifications`` is filled on **every exit but** ``probe_failed``: the watch
marks the PR's GitHub notification threads done itself through
``pr_notifications.dismiss``, so no session turn sits between the outcome and
the dismissal. **It unsubscribes only on** ``merged``: a PR still open (or
closed, which can be reopened) keeps its subscription, which is what re-creates
its thread on new activity. That includes ``stopped`` — a SIGTERM or SIGINT
(the session stopping the watch early, having learned the outcome another way)
raises into the same ``finally`` that dismisses, then the verdict prints.
``todays_merge_count`` is filled on ``merged`` only: the PRs merged since local
midnight, this one included. Both are best-effort — a failure is reported there
(``todays_merge_count: null``), never changes the conclusion. See
:func:`dismiss_pr_notifications` and :func:`todays_merge_count`.

``dequeue`` is filled only on ``dequeued``, and is best-effort the same way::

      "dequeue": {"dequeue_reason": "merge_conflict",
                  "dequeued_at": "2026-10-22T23:40:44Z",
                  "base_gained": [{"sha": "8f285869abcd", "subject": "feat(…): …"}],
                  "base_gained_total": 1, "error": null}

``base_gained`` (the base's commits the head lacks) is read only on a conflict
reason, so the rebase starts from the verdict. See :func:`dequeue_cause`. A
dequeue that stays ``dequeued`` also carries ``classification`` — ``cause``
``transient`` / ``integration`` / ``unknown``, read from the queue branch's
failed job steps, with the evidence and the one log command. See
:func:`classify_dequeue`.

or, under ``--enqueue``::

    {
      "pr": 285,
      "mode": "enqueue",
      "conclusion": "queued",    // queued | merged | auto_merge_pending |
                                 // not_queued | enqueue_failed |
                                 // probe_failed | refused
      "reviewed_fingerprint": "3f1c…", "fingerprint": "3f1c…",
      "merge_queue": true,       // the base's rule; null when unreadable
                                 // or never read (refused)
      "attempts": [{"command": "gh pr merge 285 --repo …", "exit": 0,
                    "stderr": null}],
      "entry": {"state": "QUEUED", "enqueuedAt": "…", "position": 3},
      "enqueued_at": "…",
      "error": null
    }

Exit 0 on ``queued`` / ``merged`` / ``auto_merge_pending``, else 1 — and 2,
with no verdict, when the pull read or the fingerprint cannot be had.

A dequeue with **no** queue run also settles the PR's ``mergeable`` (reported
beside ``merge_state_status`` and ``mergeable_error``); a ``CONFLICTING`` / ``DIRTY`` read turns the
conclusion into ``conflicting``, ``dequeue`` filled the same way, with
``base_gained`` read whatever the timeline's reason says.

A conflict is also seen **while the entry is still live**: the once-a-minute
GraphQL read carries ``mergeable`` and ``mergeStateStatus``, and a
``CONFLICTING`` / ``DIRTY`` one is settled the same way and, if it holds,
concludes ``conflicting`` before GitHub removes the PR — ``dequeue`` then
carries a null reason (nothing has dequeued yet) beside ``base_gained``.

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
import signal
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import quote

import pr_notifications
import review_diff

DEFAULT_REPO = "DASMAC-com/dropset"

#: The env knob every PR wait reads its poll interval from, and its default.
#: The operator accepted fifteen to thirty seconds; twenty is the middle. A
#: missing, non-integer or non-positive value falls back to the default.
POLL_ENV = "DS_PR_POLL_S"
DEFAULT_POLL = 20

#: Under this fraction of the REST limit remaining, a wait backs off to
#: :data:`BACKOFF_INTERVAL`. Twenty watchers at twenty seconds send up to
#: 3,600 pull reads an hour (more with the timeline beside them) against a
#: 5,000-an-hour limit — before the 304s, which are not counted, and which an
#: unchanged queue wait mostly is.
LOW_QUOTA_FRACTION = 0.2
BACKOFF_INTERVAL = 60


def poll_interval(env=os.environ) -> int:
    """``DS_PR_POLL_S`` as positive seconds, else :data:`DEFAULT_POLL`."""
    try:
        value = int(env.get(POLL_ENV, "").strip())
    except ValueError:
        return DEFAULT_POLL
    return value if value > 0 else DEFAULT_POLL


# How many times to re-probe a mergeable value of UNKNOWN, and how long to wait
# between. GitHub computes mergeability asynchronously; a couple of seconds is
# usually enough, and concluding from UNKNOWN is the failure being avoided.
MERGEABLE_RETRIES = 3
MERGEABLE_RETRY_DELAY = 2

# The fallback interval for a caller that passes none; the CLI resolves
# `poll_interval()` instead, so the env knob reaches every mode.
DEFAULT_INTERVAL = DEFAULT_POLL

# An outer bound on the watch, so a wedged or cancelled run can't hold the
# session open indefinitely. An hour is well past this repo's slowest suite.
DEFAULT_TIMEOUT = 3600

# A bound on one non-watching `gh` read (a JSON view, a probe). Such a read
# answers in seconds; two minutes is far past a slow API and short enough that
# a hung `gh` becomes an error instead of a verdict that never prints.
GH_CALL_TIMEOUT = 120

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

# What :func:`watch_checks` returns when its pull read says ``dirty``: truthy,
# since the watch did end, but not ``True``, so the caller settles the
# conflict instead of reading the checks it cut short.
CONFLICT_SEEN = "conflict_seen"


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
    return log_dir() / f"wait-for-checks-{pr}.log"


def log_dir() -> Path:
    """The directory every watch log goes in (``verify_head.py``'s too)."""
    base = Path(tempfile.gettempdir()) / "claude-wait-checks"
    base.mkdir(mode=0o700, parents=True, exist_ok=True)
    return base


def run_log_path_for(run_id: str) -> Path:
    """Where a ``--run`` watch's captured output goes.

    Keyed on the run id for the same reason :func:`log_path_for` keys on the PR
    number: a stable, guessable path beats a pid nobody can reconstruct.
    """
    return log_dir() / f"wait-for-run-{run_id}.log"


def _gh(args: list[str]) -> tuple[int, str, str]:
    """Run a ``gh`` command, returning ``(returncode, stdout, stderr)``.

    Never raises on a non-zero exit — ``gh pr checks`` uses non-zero to mean
    "some check failed", which is an outcome to report, not an error. A ``gh``
    that does not answer within :data:`GH_CALL_TIMEOUT` is refused, so a hung
    read cannot hang the verdict.
    """
    try:
        completed = subprocess.run(
            ["gh", *args],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            errors="replace",
            check=False,
            timeout=GH_CALL_TIMEOUT,
        )
    except subprocess.TimeoutExpired as exc:
        raise WaitForChecksError(
            f"gh {' '.join(args[:2])} did not answer within {GH_CALL_TIMEOUT}s"
        ) from exc
    except OSError as exc:
        raise WaitForChecksError(f"cannot run gh: {exc}") from exc
    return completed.returncode, completed.stdout, completed.stderr


_STATUS_RE = re.compile(r"^HTTP/[\d.]+ (\d{3})")


def _split_response(text: str) -> tuple[int | None, dict[str, str], str]:
    """``(status, headers, body)`` from ``gh api -i`` output.

    Header names are lowercased (GitHub sends ``Etag``). ``status`` is ``None``
    when the output carries no status line — gh failed before any response.
    """
    head, _, body = text.replace("\r\n", "\n").partition("\n\n")
    lines = head.split("\n")
    match = _STATUS_RE.match(lines[0]) if lines else None
    if match is None:
        return None, {}, ""
    headers = {}
    for line in lines[1:]:
        name, sep, value = line.partition(":")
        if sep:
            headers[name.strip().lower()] = value.strip()
    return int(match.group(1)), headers, body


class Conditional:
    """Conditional REST GETs through ``gh api -i``, one ETag per path.

    A path read before is re-sent with ``If-None-Match``; a ``304`` returns the
    cached body and is **not counted against the rate limit** (GitHub's REST
    docs, "conditional requests"). Verified against this repo at
    implementation: the pull, timeline and check-runs endpoints all answer a
    repeated ETag with ``304`` and an unchanged ``X-RateLimit-Remaining``;
    the latter two send weak (``W/``) ETags, which are matched as sent. gh
    exits 1 on a ``304``, so the status line, never the exit code, decides.

    ``remaining`` / ``limit`` hold the last response's quota headers, which
    :meth:`pace` turns into the back-off.
    """

    def __init__(self, gh=None):
        # None resolves the module's `_gh` at call time, so a test that
        # patches `wfc._gh` reaches this client too.
        self._gh = gh
        self._cache: dict[str, tuple[str, object]] = {}
        self.remaining: int | None = None
        self.limit: int | None = None
        self.sent = 0
        self.not_modified = 0

    def get(self, path: str):
        """The JSON body at ``path``, from cache on a ``304``."""
        cached = self._cache.get(path)
        args = ["api", "-i", path]
        if cached is not None:
            args += ["-H", f"If-None-Match: {cached[0]}"]
        code, out, err = (self._gh or _gh)(args)
        status, headers, body = _split_response(out)
        if status is None:
            raise WaitForChecksError(
                f"GET {path} failed: {err.strip() or f'exit {code}'}"
            )
        self.sent += 1
        try:
            self.remaining = int(headers["x-ratelimit-remaining"])
            self.limit = int(headers["x-ratelimit-limit"])
        except (KeyError, ValueError):
            pass
        if status == 304 and cached is not None:
            self.not_modified += 1
            return cached[1]
        if status != 200:
            raise WaitForChecksError(f"GET {path}: HTTP {status}")
        try:
            payload = json.loads(body)
        except json.JSONDecodeError as exc:
            raise WaitForChecksError(f"decoding GET {path}: {exc}") from exc
        if headers.get("etag"):
            self._cache[path] = (headers["etag"], payload)
        return payload

    def pace(self, interval: int) -> int:
        """``interval``, or :data:`BACKOFF_INTERVAL` once the quota is low."""
        if self.remaining is not None and self.limit:
            if self.remaining < LOW_QUOTA_FRACTION * self.limit:
                return max(interval, BACKOFF_INTERVAL)
        return interval

    def report(self) -> dict:
        """The verdict's ``requests`` block."""
        return {
            "sent": self.sent,
            "not_modified": self.not_modified,
            "rate_remaining": self.remaining,
        }


# Check runs per page, and the most pages one poll reads: this repo registers
# a few dozen per commit, so a second page is already the unusual case.
CHECK_RUNS_PER_PAGE = 100
CHECK_RUNS_MAX_PAGES = 3


def _check_runs(client: Conditional, repo: str, sha: str) -> list[dict]:
    runs: list[dict] = []
    for page in range(1, CHECK_RUNS_MAX_PAGES + 1):
        payload = client.get(
            f"/repos/{repo}/commits/{sha}/check-runs"
            f"?per_page={CHECK_RUNS_PER_PAGE}&page={page}"
        )
        batch = payload.get("check_runs") if isinstance(payload, dict) else None
        if not isinstance(batch, list):
            raise WaitForChecksError("check-runs read returned no list")
        runs += [r for r in batch if isinstance(r, dict)]
        if len(runs) >= int(payload.get("total_count") or 0):
            break
    return runs


def watch_checks(
    pr: int,
    repo: str,
    interval: int,
    timeout: int,
    log: Path,
    client: Conditional | None = None,
    sleep=None,
    clock=None,
) -> bool | str:
    """Block until the PR head's check runs all complete. ``False`` on timeout.

    Each poll is two conditional reads — the pull, for the head SHA (so a push
    mid-wait moves the watch with it), and that SHA's check runs — and one log
    line. The pull also carries ``mergeable_state``: a ``dirty`` one (the base
    moved under the wait and the PR now conflicts) returns
    :data:`CONFLICT_SEEN` at once, open runs or not, for the caller to settle
    — no extra read, so a clean PR's wait costs what it did. No check run at all settles at once, as gh's watch did: the caller's
    ``none`` rounds own the registration race. A read failing
    :data:`MAX_PROBE_ERRORS` times running also returns, settled, so the
    caller's verdict read surfaces the failure rather than the watch spinning
    out its budget. On timeout the caller still reads the current state, so a
    timed-out wait reports what the checks *were* rather than nothing at all.

    Check runs only, never commit statuses (gh's watch waited on both): this
    repo posts none, and the verdict read still sees one. A status producer
    added later would leave a pending status re-entering this watch at once
    each round, spending :data:`MAX_WATCH_ROUNDS` at ``interval`` pacing.
    """
    client = client or Conditional()
    sleep = sleep or time.sleep
    clock = clock or time.monotonic
    started = clock()
    errors = 0
    try:
        fd = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    except OSError as exc:
        raise WaitForChecksError(f"cannot write {log}: {exc}") from exc
    with os.fdopen(fd, "w", encoding="utf-8", errors="replace") as fh:
        while True:
            line: dict = {"t": int(clock() - started)}
            try:
                pull = client.get(f"/repos/{repo}/pulls/{pr}")
                sha = str(((pull or {}).get("head") or {}).get("sha") or "")
                if not sha:
                    raise WaitForChecksError("pull read carried no head SHA")
                if str((pull or {}).get("mergeable_state") or "").lower() == "dirty":
                    line |= {"sha": sha[:12], "mergeable_state": "dirty"}
                    fh.write(json.dumps(line) + "\n")
                    return CONFLICT_SEEN
                runs = _check_runs(client, repo, sha)
                errors = 0
                open_runs = [r for r in runs if r.get("status") != "completed"]
                line |= {"sha": sha[:12], "runs": len(runs), "open": len(open_runs)}
                settled = not open_runs
            except WaitForChecksError as exc:
                errors += 1
                line["error"] = str(exc)
                settled = errors >= MAX_PROBE_ERRORS
            line["rate_remaining"] = client.remaining
            fh.write(json.dumps(line) + "\n")
            fh.flush()
            if settled:
                return True
            pause = client.pace(interval)
            if clock() - started + pause > timeout:
                return False
            sleep(pause)


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
    return log_dir() / f"wait-for-queue-{pr}.log"


# One GraphQL read answers "landed?", "still queued?" and "dequeued?" at once.
# `mergeQueueEntry` is the queue signal: on a merge-queue repo a genuinely
# queued PR reports `autoMergeRequest: null`, so keying on the latter announced
# a dequeue on every run. A set `autoMergeRequest` beside a null entry is the
# not-yet-queued state: auto-merge waits for every pending check, required or
# not, so the head commit's rollup names what it is waiting on.
QUEUE_QUERY = """
query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      state
      merged
      mergeable
      mergeStateStatus
      mergeQueueEntry { state enqueuedAt position }
      autoMergeRequest { enabledAt }
      commits(last: 1) {
        nodes {
          commit {
            statusCheckRollup {
              contexts(first: 100) {
                nodes {
                  __typename
                  ... on CheckRun {
                    name
                    status
                    isRequired(pullRequestNumber: $number)
                  }
                  ... on StatusContext {
                    context
                    state
                    isRequired(pullRequestNumber: $number)
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
"""

# A queue wait sits behind every branch ahead of it, so it gets a longer outer
# bound than one CI run does.
QUEUE_TIMEOUT = 7200

# How often the corroborating reads — the GraphQL queue entry and the queue
# branch's runs — are re-taken. The pull and the timeline answer every poll.
CORROBORATE_SECONDS = 60

# The silence fallbacks, for when the timeline says nothing. The timeline now
# carries enqueue and dequeue as events, so these no longer have to outwait a
# stale pair of state sources (once several minutes after a real merge).
#
# All-null with no enqueue event and no queue-branch run for this long means
# the enqueue never took; registration lands within seconds of a good one.
NOT_QUEUED_AFTER_SECONDS = 120

# All-null after the entry demonstrably existed, with no removal event and no
# failed queue run to explain it.
AMBIGUOUS_GRACE_SECONDS = 120

# Auto-merge enabled with no queue entry for this long is `auto_merge_waiting`:
# the PR 548 shape, which sat silent to the outer timeout behind a pending
# non-required check. The caller's remedy is the plain `gh pr merge <n>`.
AUTO_MERGE_WAIT_SECONDS = 900

# A queue entry with no merge-group run for this long is `queued_no_group`.
# Observed dispatch lag behind other branches is about 20 minutes.
AWAIT_GROUP_SECONDS = 1200

# How far before the watch's start a removal may sit and still be this
# cycle's, when no `--since` says when the enqueue was: the watch starts
# seconds after the enqueue, and a conflict removal lands seconds after that.
REMOVAL_SLACK_SECONDS = 60

# Timeline events per page, and the most pages one poll reads.
TIMELINE_PER_PAGE = 100
TIMELINE_MAX_PAGES = 5

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


def _iso(epoch: float | None) -> str | None:
    """Epoch seconds as a UTC ``…Z`` timestamp, or ``None``."""
    if epoch is None:
        return None
    return datetime.fromtimestamp(epoch, tz=timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _gh_json(args: list[str], what: str):
    code, out, err = _gh(args)
    if code != 0:
        raise WaitForChecksError(f"{what} failed: {err.strip() or f'exit {code}'}")
    try:
        return json.loads(out)
    except json.JSONDecodeError as exc:
        raise WaitForChecksError(f"decoding {what}: {exc}") from exc


def timeline_state(events: list) -> dict:
    """The queue facts in a PR's REST timeline, oldest event first.

    Event names verified against this repo's API at implementation (PR 685):
    ``added_to_merge_queue`` and ``removed_from_merge_queue``. **A merge also
    writes ``removed_from_merge_queue``**, at the same second as its
    ``merged`` and ``closed`` — so ``removed_at`` is a dequeue only where
    ``merged`` is false, which :func:`wait_queue` checks first.

    ``enqueued_at`` is the newest enqueue; ``removed_at`` the newest removal
    **after** it in timeline order (order, not timestamps: a conflict removal
    can share the enqueue's second), else ``None``.
    """
    names = [
        (str(e.get("event") or ""), e.get("created_at"))
        for e in events
        if isinstance(e, dict)
    ]
    added = [i for i, (name, _) in enumerate(names) if name == "added_to_merge_queue"]
    state: dict = {
        "enqueued_at": names[added[-1]][1] if added else None,
        "removed_at": None,
        "merged": any(name == "merged" for name, _ in names),
    }
    if added:
        for name, at in names[added[-1] + 1 :]:
            if name == "removed_from_merge_queue":
                state["removed_at"] = at
    return state


def _queue_runs(client: Conditional, repo: str, pr: int) -> list[dict]:
    """This PR's ``gh-readonly-queue/…/pr-<n>-…`` runs, newest hundred.

    ``event=merge_group`` narrows server-side, so the page spans queue runs
    only rather than every push on a busy repo.
    """
    payload = client.get(f"/repos/{repo}/actions/runs?event=merge_group&per_page=100")
    runs = payload.get("workflow_runs") if isinstance(payload, dict) else None
    if not isinstance(runs, list):
        raise WaitForChecksError("queue run list returned no list")
    marker = f"/pr-{pr}-"
    return [
        {
            "run_id": str(r.get("id", "")),
            "head_branch": str(r.get("head_branch", "")),
            "name": r.get("name", ""),
            "status": r.get("status", ""),
            "conclusion": r.get("conclusion") or "",
            "created_at": r.get("created_at", ""),
            "url": r.get("html_url", ""),
        }
        for r in runs
        if isinstance(r, dict)
        and str(r.get("head_branch", "")).startswith("gh-readonly-queue/")
        and marker in str(r.get("head_branch", ""))
    ]


def _queue_graphql(pr: int, repo: str) -> dict:
    """The GraphQL queue entry. Not conditional: GraphQL has no ETags, and
    its own points budget, so it is the once-a-minute corroboration."""
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
    return {
        "gql_state": node.get("state"),
        "gql_merged": bool(node.get("merged")),
        "entry": node.get("mergeQueueEntry"),
        "auto": node.get("autoMergeRequest"),
        "mergeable": node.get("mergeable"),
        "merge_state_status": node.get("mergeStateStatus"),
        "pending_checks": pending_contexts(node),
    }


# Rollup states that are still running, for each context type.
PENDING_RUN_STATUSES = frozenset(
    {"QUEUED", "IN_PROGRESS", "WAITING", "PENDING", "REQUESTED"}
)
PENDING_STATUS_STATES = frozenset({"PENDING", "EXPECTED"})


def pending_contexts(node: dict) -> list[dict]:
    """The head commit's still-running checks, as ``{name, required}``.

    Required or not matters: auto-merge waits on a pending non-required
    check too, which is the one a reader cannot otherwise see.
    """
    try:
        commit = node["commits"]["nodes"][-1]["commit"]
        contexts = commit["statusCheckRollup"]["contexts"]["nodes"]
    except (KeyError, IndexError, TypeError):
        return []
    pending = []
    for c in contexts if isinstance(contexts, list) else []:
        if not isinstance(c, dict):
            continue
        if c.get("__typename") == "CheckRun":
            if c.get("status") in PENDING_RUN_STATUSES:
                pending.append({"name": c.get("name"), "required": c.get("isRequired")})
        elif c.get("state") in PENDING_STATUS_STATES:
            pending.append({"name": c.get("context"), "required": c.get("isRequired")})
    return pending


class QueueProbe:
    """One snapshot of a queued PR per call, keeping its ETags across calls.

    Every call reads the timeline (:func:`timeline_state`) and then the REST
    pull (merged / closed), both conditional, so an unchanged poll costs
    two free ``304`` s and a merge or dequeue is seen at the next poll. The
    GraphQL entry and the queue runs are re-read every
    :data:`CORROBORATE_SECONDS`, and **at once on a new removal**, so the
    poll that concludes a dequeue carries a fresh second opinion on the
    merge. Several sources because they have disagreed in **both**
    directions: GraphQL once read ``MERGED`` while the PR view still said
    ``OPEN``, and once still read ``AWAITING_CHECKS`` after it said
    ``MERGED``.
    """

    def __init__(self, client: Conditional | None = None, clock=time.monotonic):
        self.client = client or Conditional()
        self._clock = clock
        self._slow_at: float | None = None
        self._slow: dict = {}
        self._removed_at: str | None = None

    def _timeline(self, pr: int, repo: str) -> list:
        events: list = []
        for page in range(1, TIMELINE_MAX_PAGES + 1):
            batch = self.client.get(
                f"/repos/{repo}/issues/{pr}/timeline"
                f"?per_page={TIMELINE_PER_PAGE}&page={page}"
            )
            if not isinstance(batch, list):
                raise WaitForChecksError("timeline read returned no list")
            events += batch
            if len(batch) < TIMELINE_PER_PAGE:
                break
        return events

    def __call__(self, pr: int, repo: str) -> dict:
        # The timeline first, so a removal it shows is always followed by a
        # pull read: a merge landing between the two reads is then seen
        # merged, never as a fresh removal beside a pre-merge pull.
        events = timeline_state(self._timeline(pr, repo))
        pull = self.client.get(f"/repos/{repo}/pulls/{pr}")
        if not isinstance(pull, dict):
            raise WaitForChecksError("pull read did not return an object")
        now = self._clock()
        # A NEW removal corroborates at once; a standing one (a previous
        # cycle's, which the watch may sit beside for minutes) keeps the
        # once-a-minute pace, since GraphQL reads are never conditional.
        fresh_removal = events["removed_at"] not in (None, self._removed_at)
        self._removed_at = events["removed_at"]
        slow_fresh = (
            fresh_removal
            or self._slow_at is None
            or now - self._slow_at >= CORROBORATE_SECONDS
        )
        if slow_fresh:
            self._slow = _queue_graphql(pr, repo)
            self._slow["queue_runs"] = _queue_runs(self.client, repo, pr)
            self._slow_at = now
        return {
            **self._slow,
            # Whether the GraphQL fields were re-read on this call: a conflict
            # in them is settled once per read, not once per poll.
            "slow_fresh": slow_fresh,
            "rest_state": "MERGED"
            if pull.get("merged")
            else (str(pull.get("state") or "").upper() or None),
            "rest_merged_at": pull.get("merged_at"),
            "events": events,
            "queue_runs": list(self._slow.get("queue_runs", [])),
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
    interval: int = DEFAULT_POLL,
    timeout: int = QUEUE_TIMEOUT,
    probe=None,
    sleep=time.sleep,
    clock=time.monotonic,
    wall=time.time,
    since: str | None = None,
    await_group_seconds: int = AWAIT_GROUP_SECONDS,
    classic_auto_merge: bool = False,
):
    """Watch an enqueued PR until it merges or leaves the queue.

    Built to run as **one backgrounded command** right after a verified
    enqueue: it prints nothing until a terminal outcome, so the harness's
    exit notification is the session's wake-up. That replaces re-probing
    across turns, which nothing woke the session for, and a session-scoped
    cron, which the Bedrock auto-mode classifier can deny as persistence.

    ``conclusion`` is ``merged`` (exit 0), or ``dequeued`` / ``conflicting``
    / ``closed`` / ``not_queued`` / ``auto_merge_waiting`` /
    ``queued_no_group`` / ``timeout`` / ``probe_failed`` (exit 1).
    ``conflicting`` here is a **live** entry whose fresh GraphQL read says
    ``CONFLICTING`` / ``DIRTY`` and whose settled :func:`settle_mergeability`
    read agrees — the queue would remove it, so the watch does not wait. A terminal
    ``MERGED`` from **any** source wins — the REST pull, the timeline's
    ``merged`` event, or GraphQL — and is checked before anything else.

    The timeline decides the rest at the next poll: a removal after the
    newest enqueue is ``dequeued``, and an enqueue with no removal after it
    keeps the watch waiting. A removal counts only when newer than ``since``,
    or without it the watch's start less :data:`REMOVAL_SLACK_SECONDS`: an
    enqueue made while its checks are still pending writes no event until
    they pass, so for minutes the newest add-then-remove pair can be the
    previous cycle's. A fresh call made after an older dequeue therefore needs
    ``since``, or reads it through the fallbacks. A failed run on the newest queue branch still
    dequeues after :data:`DEQUEUE_CONFIRM_POLLS`, and silence still concludes
    after :data:`AMBIGUOUS_GRACE_SECONDS` / :data:`NOT_QUEUED_AFTER_SECONDS` —
    the fallbacks, for a timeline that says nothing.

    ``since`` (the enqueue time, ISO-8601) anchors the run window when no
    entry has been seen in *this* call — which is every firing of a cron
    fallback. Without it a fresh call keeps only runs from the last
    :data:`RUN_WINDOW_SECONDS`; the timeline's own enqueue event now anchors
    it too, so ``since`` matters only where that event is missing.

    Two bounded readings replace a silent wait to the timeout. A set
    ``autoMergeRequest`` with no entry is **not** queued on a merge-queue
    repo — auto-merge defers the enqueue behind every pending check,
    required or not — so after :data:`AUTO_MERGE_WAIT_SECONDS` with no
    required check still running it is ``auto_merge_waiting``,
    ``last.pending_checks`` naming any non-required holdout
    (``classic_auto_merge`` restores waiting, for a repo with no queue). An
    entry with no run on its queue branch for ``await_group_seconds`` is
    ``queued_no_group``; ``first_run_seen_at`` records when a run appeared.
    """
    probe = probe or QueueProbe(clock=clock)
    client = getattr(probe, "client", None)
    log = queue_log_path_for(pr)
    started = clock()
    started_wall = wall()
    removal_floor = (
        _parse_time(since) if since else started_wall - REMOVAL_SLACK_SECONDS
    )
    seen_entry = False
    anchor: float | None = _parse_time(since)
    null_since: float | None = None
    auto_since: float | None = None
    group_since: float | None = None
    first_run_seen_at: float | None = None
    dequeue_streak = 0
    probe_errors = 0
    polls = 0
    snap: dict = {}
    # This PR's queue runs before the window floor drops a previous cycle's:
    # what tells a `not_queued` that never entered from one beside an old run.
    every_run: list[dict] = []
    conflict_read: dict | None = None
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
                events = snap.get("events") or {}
                runs = snap.get("queue_runs", [])
                every_run = list(runs)
                removed = _parse_time(events.get("removed_at"))
                # A timeline enqueue is this cycle's only while nothing has
                # removed it: a removed one is a finished cycle, whose anchor
                # would pull that cycle's failed queue runs back into view.
                current_enqueue = events.get("enqueued_at") if removed is None else None
                for enqueued in (
                    _parse_time((entry or {}).get("enqueuedAt")),
                    _parse_time(current_enqueue),
                ):
                    if enqueued is not None and (anchor is None or enqueued < anchor):
                        anchor = enqueued
                floor = (
                    anchor if anchor is not None else started_wall - RUN_WINDOW_SECONDS
                )
                # A removal older than the removal floor closed a previous
                # cycle, and every run before it is that cycle's: kept, its
                # failure would dequeue a re-enqueue still waiting on checks.
                if removed is not None and removed < removal_floor:
                    floor = max(floor, removed)
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
                            "rest": snap.get("rest_state"),
                            "gql": snap.get("gql_state"),
                            "entry": (entry or {}).get("state"),
                            "auto": bool(snap.get("auto")),
                            "enqueued": events.get("enqueued_at"),
                            "removed": events.get("removed_at"),
                            "runs": len(runs),
                            "failing": len(failing),
                            "rate_remaining": getattr(client, "remaining", None),
                        }
                    )
                    + "\n"
                )
                fh.flush()

                merged = (
                    snap.get("gql_merged")
                    or snap.get("gql_state") == "MERGED"
                    or snap.get("rest_state") == "MERGED"
                    or bool(snap.get("rest_merged_at"))
                    or bool(events.get("merged"))
                )
                if merged:
                    conclusion = "merged"
                    break
                if "CLOSED" in (snap.get("gql_state"), snap.get("rest_state")):
                    conclusion = "closed"
                    break
                # A removal after the newest enqueue, on a poll where no source
                # says merged (a merge writes a removal too), and no older than
                # the floor: before a re-enqueue's own event lands, the newest
                # add-then-remove pair is the previous cycle's.
                if removed is not None and removed >= removal_floor:
                    conclusion = "dequeued"
                    break
                # A timeline enqueue never outranks a failed queue run, since
                # its removal may lag; neither does `autoMergeRequest`.
                if entry or (current_enqueue and not failing):
                    seen_entry = True
                    null_since = None
                    auto_since = None
                    dequeue_streak = 0
                    # A live entry that conflicts is one GitHub will remove
                    # minutes from now; seen here, the rebase starts at once.
                    # Settled first, so an UNKNOWN read is never a verdict.
                    if entry and snap.get("slow_fresh", True) and _is_conflicting(snap):
                        state, _ = settle_mergeability(
                            pr,
                            repo,
                            MERGEABLE_SETTLE_RETRIES,
                            MERGEABLE_SETTLE_DELAY,
                            sleep,
                        )
                        if not state.get("error") and _is_conflicting(state):
                            conflict_read = state
                            conclusion = "conflicting"
                            break
                    # In the queue: a run on its branch is the group, and an
                    # entry that never gets one is told apart from a running
                    # group rather than sitting silent to the timeout.
                    if runs:
                        group_since = None
                        if first_run_seen_at is None:
                            first_run_seen_at = wall()
                    else:
                        if group_since is None:
                            group_since = now
                        if now - group_since >= await_group_seconds:
                            conclusion = "queued_no_group"
                            break
                elif snap.get("auto") and not failing:
                    # On a merge-queue repo this is NOT queued: auto-merge
                    # defers the enqueue until every pending check completes.
                    # Classic auto-merge (no queue) opts back into waiting.
                    null_since = None
                    group_since = None
                    dequeue_streak = 0
                    if classic_auto_merge:
                        seen_entry = True
                    elif any(c.get("required") for c in snap.get("pending_checks", [])):
                        # A required check still running is a legitimate wait,
                        # not the stuck shape: the bound counts only from when
                        # nothing required remains.
                        auto_since = None
                    else:
                        if auto_since is None:
                            auto_since = now
                        if now - auto_since >= AUTO_MERGE_WAIT_SECONDS:
                            conclusion = "auto_merge_waiting"
                            break
                else:
                    auto_since = None
                    group_since = None
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

            pause = client.pace(interval) if client is not None else interval
            if clock() - started + pause > timeout:
                break
            sleep(pause)

    runs = snap.get("queue_runs", [])
    events = snap.get("events") or {}
    entry = snap.get("entry") or {}
    verdict = {
        "pr": pr,
        "repo": repo,
        "mode": "queue",
        "conclusion": conclusion,
        "settled": conclusion not in ("timeout", "probe_failed"),
        "elapsed_seconds": int(clock() - started),
        "polls": polls,
        "seen_queue_entry": seen_entry,
        # The earliest enqueue this watch saw (or `since`), so a dequeue cause
        # older than it can be told apart from this cycle's.
        "enqueued_at": _iso(anchor),
        "first_run_seen_at": _iso(first_run_seen_at),
        "last": {
            "rest_state": snap.get("rest_state"),
            "rest_merged_at": snap.get("rest_merged_at"),
            "gql_state": snap.get("gql_state"),
            "entry_state": entry.get("state"),
            "entry_position": entry.get("position"),
            "auto_enabled_at": (snap.get("auto") or {}).get("enabledAt"),
            "merge_state_status": snap.get("merge_state_status"),
            "pending_checks": snap.get("pending_checks", []),
            "removed_at": events.get("removed_at"),
        },
        "queue_runs": runs,
        "failing_runs": _failing_on_latest(runs),
        "requests": client.report() if client is not None else None,
        # The last probe error, whatever the conclusion: a timeout reached
        # through failed probes must not read as a quiet queue.
        "error": error,
        "log_path": str(log),
    }
    if conclusion == "not_queued":
        verdict["not_queued"] = not_queued_reading(every_run)
    if conflict_read is not None:
        verdict["mergeable"] = conflict_read["mergeable"]
        verdict["merge_state_status"] = conflict_read["merge_state_status"]
        verdict["mergeable_error"] = conflict_read["error"]
        verdict["conflict_read"] = conflict_read
    return verdict


def not_queued_reading(every_run: list[dict]) -> dict:
    """The run-list disambiguation a ``not_queued`` verdict carries.

    ``never_entered``: this PR has no queue run at all, so the enqueue did not
    take. ``prior_cycle_only``: its runs all predate this cycle's window, so
    they are an earlier enqueue's, never this one's — ``newest_prior`` says how
    that cycle ended. Nothing queued this time, but an earlier cycle that
    FAILED may be the dequeue this call started too late to see:
    ``prior_failing_runs`` (its newest queue branch's failures) is what the
    queue watch classifies before any re-enqueue.
    """
    newest = max(
        every_run, key=lambda r: _parse_time(r.get("created_at")) or 0.0, default=None
    )
    return {
        "reading": "prior_cycle_only" if every_run else "never_entered",
        "prior_runs": len(every_run),
        "newest_prior": None
        if newest is None
        else {k: newest.get(k) for k in ("run_id", "conclusion", "created_at")},
        "prior_failing_runs": _failing_on_latest(every_run),
    }


def dismiss_pr_notifications(pr: int, repo: str, merged: bool = True) -> dict:
    """Mark done every notification thread on ``repo``'s PR ``pr``, and
    unsubscribe from them only when ``merged``, through
    ``pr_notifications.dismiss`` (the one rule every caller applies).

    Called on every queue exit but ``probe_failed``. ``failed`` counts failed
    calls (a thread whose ignore and mark-done both fail counts twice). Never
    raises: the outcome already happened, so a failure is reported, and the
    upkeep pass's settled-PR sweep is the fallback.
    """
    try:
        rc, result = pr_notifications.dismiss(
            pr_notifications.real_gh,
            {pr},
            apply=True,
            repo=repo,
            keep_subscription=set() if merged else {pr},
        )
    except Exception as exc:  # noqa: BLE001 — best-effort by contract
        return {
            "dismissed": 0,
            "unsubscribed": 0,
            "failed": 0,
            "error": f"{type(exc).__name__}: {exc}",
        }
    failed = len(result["failed"]) + len(result["unsubscribe_failed"])
    error = result["notice"] if rc else result["error"]
    return {
        "dismissed": result["dismissed"],
        "unsubscribed": result["unsubscribed"],
        "failed": failed,
        "error": error,
    }


# Comfortably above any day's merges; `gh pr list` defaults to 30.
MERGE_COUNT_LIMIT = 500


def local_midnight(now: datetime | None = None) -> str:
    """Today's local midnight as an ISO-8601 time with its UTC offset, the
    form GitHub's ``merged:>=`` search qualifier takes. ``now`` defaults to
    the machine's own zone, localized at midnight itself so a DST change later
    in the day cannot shift the window; an aware ``now`` keeps its zone."""
    if now is None:
        midnight = datetime.now().replace(hour=0, minute=0, second=0, microsecond=0)
        return midnight.astimezone().isoformat()
    return now.replace(hour=0, minute=0, second=0, microsecond=0).isoformat()


def todays_merge_count(pr: int, repo: str, now: datetime | None = None) -> int | None:
    """PRs merged into ``repo`` since local midnight, ``pr`` included even if
    the search index has not caught up with its merge. ``None`` when the
    read fails — the merge already happened, so the count is best-effort.
    """
    try:
        merged = _gh_json(
            [
                "pr", "list", "--repo", repo, "--state", "merged",
                "--search", f"merged:>={local_midnight(now)}",
                "--json", "number", "--limit", str(MERGE_COUNT_LIMIT),
            ],
            "merged PR list",
        )  # fmt: skip
        numbers = {int(p["number"]) for p in merged or []}
    except (WaitForChecksError, KeyError, TypeError, ValueError):
        return None
    return len(numbers | {pr})


class Stopped(Exception):
    """A SIGTERM or SIGINT, raised into the queue watch's ``finally``."""

    def __init__(self, number: int):
        super().__init__(signal.Signals(number).name)
        self.number = number


def _raise_stopped(number, _frame):
    raise Stopped(number)


STOP_SIGNALS = (signal.SIGTERM, signal.SIGINT)


# The newest queue removal event, and the head to compare the base against.
DEQUEUE_QUERY = """
query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      baseRefName
      headRefOid
      timelineItems(last: 1, itemTypes: [REMOVED_FROM_MERGE_QUEUE_EVENT]) {
        nodes { ... on RemovedFromMergeQueueEvent { createdAt reason } }
      }
    }
  }
}
"""

# How many of the base's new commits a merge-conflict verdict lists.
BASE_GAINED_LIMIT = 20


def _subject(commit) -> str:
    """A compare entry's subject line; ``""`` for any unexpected shape."""
    inner = commit.get("commit") if isinstance(commit, dict) else None
    message = inner.get("message") if isinstance(inner, dict) else None
    return message.split("\n")[0] if isinstance(message, str) else ""


def dequeue_cause(
    pr: int, repo: str, enqueued_at: str | None = None, conflicting: bool = False
) -> dict:
    """Why ``repo``'s PR ``pr`` left the queue, and what landed meanwhile.

    Called on a ``dequeued`` queue verdict, so the session reads the cause off
    the verdict instead of spending a timeline query, a mergeability read and a
    ``git log`` to learn it. ``dequeue_reason`` / ``dequeued_at`` come from the
    newest ``RemovedFromMergeQueueEvent`` (GitHub's lowercase reason, e.g.
    ``merge_conflict``); on a conflict reason, ``base_gained``
    lists the base branch's commits the PR head lacks (the last
    :data:`BASE_GAINED_LIMIT` of the compare's first page — up to 250, so the
    newest unless the head is that stale; oldest first; ``base_gained_total``
    counts all) — the PRs that landed first and that the rebase must absorb.
    Not "since the enqueue": a conflict dequeue fires seconds after it, so the
    culprit landed before. ``base_gained`` is ``None`` for any other reason.

    ``enqueued_at`` (the watch's earliest seen enqueue) guards against a stale
    event: a newest removal older than it belongs to an earlier cycle, so its
    reason is withheld and ``error`` says why. ``conflicting`` (the PR's own
    mergeability read ``CONFLICTING``) reads ``base_gained`` whatever the
    timeline says, since the event can lag the dequeue. Never raises: the dequeue
    already happened, so a failed read is reported in ``error`` and the
    session falls back to reading by hand.
    """
    cause: dict = {
        "dequeue_reason": None,
        "dequeued_at": None,
        "base_gained": None,
        "base_gained_total": None,
        "error": None,
    }
    owner, _, name = repo.partition("/")
    try:
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
                f"query={DEQUEUE_QUERY}",
            ],
            "dequeue timeline query",
        )
        node = gql["data"]["repository"]["pullRequest"]
        events = node["timelineItems"]["nodes"]
        base = node["baseRefName"]
        head = node["headRefOid"]
    except (WaitForChecksError, KeyError, TypeError) as exc:
        cause["error"] = f"dequeue timeline query: {exc!r}"
        return cause
    last = events[-1] if events and isinstance(events[-1], dict) else None
    removed = _parse_time((last or {}).get("createdAt"))
    since = _parse_time(enqueued_at)
    if last is None:
        cause["error"] = "no RemovedFromMergeQueueEvent on the timeline"
    elif removed is not None and since is not None and removed < since:
        cause["error"] = (
            f"newest RemovedFromMergeQueueEvent ({last.get('createdAt')}) predates "
            f"this enqueue ({enqueued_at}) — not yet on the timeline"
        )
    else:
        cause["dequeue_reason"] = last.get("reason")
        cause["dequeued_at"] = last.get("createdAt")
    if not conflicting and "conflict" not in str(cause["dequeue_reason"]).lower():
        return cause
    gain = base_gain(repo, head, base)
    cause["base_gained"] = gain["base_gained"]
    cause["base_gained_total"] = gain["base_gained_total"]
    cause["error"] = "; ".join(e for e in (cause["error"], gain["error"]) if e) or None
    return cause


def base_gain(repo: str, head: str | None, base: str | None) -> dict:
    """``{"base_gained", "base_gained_total", "error"}``: what ``base`` has
    that ``head`` lacks — the PRs a conflict's rebase must absorb.

    ``base_gained`` is the last :data:`BASE_GAINED_LIMIT` of the compare's
    first page, oldest first; ``base_gained_total`` counts all. Never raises:
    a failed read (or a missing ref) leaves both ``None`` and says why.
    """
    gain: dict = {"base_gained": None, "base_gained_total": None, "error": None}
    if not head or not base:
        gain["error"] = "base compare: no head SHA or base branch to compare"
        return gain
    try:
        compare = _gh_json(
            ["api", f"/repos/{repo}/compare/{quote(head)}...{quote(base)}"],
            "base compare",
        )
        commits = compare["commits"]
        gain["base_gained_total"] = int(compare["ahead_by"])
    except (WaitForChecksError, KeyError, TypeError, ValueError) as exc:
        gain["error"] = f"base compare: {exc!r}"
        return gain
    gain["base_gained"] = [
        {"sha": str(c.get("sha", ""))[:12], "subject": _subject(c)}
        for c in (commits if isinstance(commits, list) else [])[-BASE_GAINED_LIMIT:]
        if isinstance(c, dict)
    ]
    return gain


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
    """``{"mergeable", "merge_state_status", "head", "base", "error"}`` for one PR.

    Read **field-selected**, because the fields wanted here sit beside
    collection-valued ones (``files``, ``commits``) that would reintroduce the
    payload this tool exists to avoid. ``head`` / ``base`` (the head SHA and
    base branch) ride the same call, for :func:`base_gain` on a conflict.
    """
    failed = {"mergeable": None, "merge_state_status": None, "head": None, "base": None}
    code, out, err = _gh(
        [
            "pr",
            "view",
            str(pr),
            "--repo",
            repo,
            "--json",
            "mergeable,mergeStateStatus,headRefOid,baseRefName",
        ]
    )
    if code != 0:
        return {**failed, "error": err.strip()}
    try:
        payload = json.loads(out or "{}")
    except json.JSONDecodeError as exc:
        return {**failed, "error": str(exc)}
    return {
        "mergeable": payload.get("mergeable"),
        "merge_state_status": payload.get("mergeStateStatus"),
        "head": payload.get("headRefOid"),
        "base": payload.get("baseRefName"),
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


def _is_conflicting(state: dict) -> bool:
    """A :func:`mergeability` read that says the PR cannot merge as it stands."""
    return (state.get("mergeable") or "").upper() == "CONFLICTING" or (
        state.get("merge_state_status") or ""
    ).upper() == "DIRTY"


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
    moved), ``conflicting`` (or ``DIRTY``) — blocking until the session
    resolves it — ``unknown`` once the settle reads run out, and
    ``probe_failed``. Only
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
    elif _is_conflicting(state):
        conclusion = "conflicting"
        hint = {
            "gate": "blocking",
            "do": (
                "do not mark ready or enqueue; rebase onto origin/<base> and "
                "resolve by review-pr step 2's conflict tiers, then re-run"
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
    sleep=None,
) -> dict:
    """The whole wait: watch, read, and re-watch while the read says pending.

    ``sleep`` paces the re-entry between rounds (``time.sleep`` when ``None``)
    — injected so a test records this module's own pacing and nothing else.

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
    merge_state: dict = {"mergeable": None, "merge_state_status": None, "error": None}
    # A base move under the wait: the watch saw `dirty` and this settle agreed.
    conflict_seen = False

    while watch:
        remaining = int(deadline - time.monotonic())
        if remaining <= 0:
            settled = False
            break
        rounds += 1
        settled = watch_checks(pr, repo, interval, remaining, log)
        cut_short = settled == CONFLICT_SEEN
        if cut_short:
            # The REST read can run ahead of a settled answer, so the gate's
            # own settle loop decides; an unconfirmed `dirty` waits on.
            merge_state, _ = settle_mergeability(
                pr, repo, MERGEABLE_SETTLE_RETRIES, MERGEABLE_SETTLE_DELAY,
                sleep or time.sleep,
            )  # fmt: skip
            settled = True
            conflict_seen = not merge_state.get("error") and _is_conflicting(
                merge_state
            )
        summary = summarize(read_checks(pr, repo))
        if conflict_seen:
            break
        state = summary["conclusion"]
        if not settled or state not in RETRY_CONCLUSIONS:
            break
        # The two retryable states get their own cap and pacing: a `none` is a
        # seconds-scale registration race, a `pending` is a whole CI run.
        # A `pending` round an unconfirmed `dirty` cut short was no CI run, so
        # it spends none of the budget: a lagging REST read must not turn a
        # live run into `timeout` (the deadline still bounds the loop).
        if not (cut_short and state == "pending"):
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
        # The watch returns at once when it sees nothing left to wait on, so
        # pace the re-entry rather than spinning through the cap in a few
        # milliseconds. Each round re-truncates the log; the last round's is
        # the one that describes the state actually being reported.
        pause = float(NONE_SETTLE_SECONDS if state == "none" else interval)
        (sleep or time.sleep)(max(0.0, min(pause, deadline - time.monotonic())))

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
    blocked_by_conflict = conflict_seen
    if conflict_seen:
        conclusion = "conflicting"
    elif conclusion == "none":
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
        "base_gained": None,
        "base_gained_total": None,
        "base_gained_error": None,
        "failed_steps": (
            diagnose_failures(summary["failing"], repo) if conclusion == "fail" else []
        ),
        "snapshot": snap,
        "log_path": str(log),
    }
    if conclusion == "conflicting":
        gain = base_gain(repo, merge_state.get("head"), merge_state.get("base"))
        verdict["base_gained"] = gain["base_gained"]
        verdict["base_gained_total"] = gain["base_gained_total"]
        verdict["base_gained_error"] = gain["error"]
    verdict["hint"] = action_hint(verdict)
    return verdict


# A failed step that sets the job up rather than testing the change: a
# toolchain, network or cache failure before any test ran. Matched after the
# lint and test patterns, so `make tools-tests` stays a test.
_INFRA_STEP_RE = re.compile(
    r"(?i)\b(set up|setup|checkout|cache|install|toolchain|platform-tools"
    r"|download|restore)\b"
)
# A locked install fails deterministically when the base moved a manifest or
# lockfile, so it is integration, never re-enqueued as setup noise.
_LOCKED_INSTALL_RE = re.compile(r"--(frozen-lockfile|locked|frozen)\b")
# A failed step that builds the change: compile errors are integration.
_BUILD_STEP_RE = re.compile(r"\b(make|cargo|pnpm|wasm-pack|anchor|node)\b")
# A run that never started a job is infrastructure by definition.
TRANSIENT_RUN_CONCLUSIONS = frozenset({"startup_failure"})


def step_cause(name: str) -> str:
    """``integration`` / ``transient`` / ``unknown`` for one failed step."""
    if classify_step(name) in ("lint_hook", "test_runner"):
        return "integration"
    if _LOCKED_INSTALL_RE.search(name or ""):
        return "integration"
    if _INFRA_STEP_RE.search(name or ""):
        return "transient"
    if _BUILD_STEP_RE.search(name or ""):
        return "integration"
    return "unknown"


def classify_dequeue(failing_runs: list[dict], repo: str) -> dict:
    """Why the queue branch failed, from its jobs' failed steps.

    ``cause`` is ``integration`` when any failed step tested, linted or built
    the change (do not re-enqueue: a real ``main``-integration failure),
    ``transient`` when every failed step was setup — a toolchain, network or
    cache failure before any test ran (re-enqueue) — else ``unknown``. A job
    with no failed step (cancelled beside a sibling's failure) counts only
    when no job has one. Keyed on **job** conclusions, never the run's: a run
    can read ``in_progress`` with a job already failed. ``next_command`` reads
    the first failing run's log. The decision stays with the session; a failed
    read is ``unknown`` evidence in the decision, never dropped, so it holds
    a transient sibling at ``unknown``.
    """
    stepped: list[dict] = []
    no_step: list[dict] = []
    for run in failing_runs:
        run_id = str(run.get("run_id") or "")
        if run.get("conclusion") in TRANSIENT_RUN_CONCLUSIONS:
            stepped.append({"run_id": run_id, "job": None, "step": None,
                            "cause": "transient"})  # fmt: skip
            continue
        payload, error = _fetch_json(
            ["api", f"repos/{repo}/actions/runs/{run_id}/jobs?per_page=100"]
        )
        jobs = payload.get("jobs") if isinstance(payload, dict) else None
        if error is None and not isinstance(jobs, list):
            error = "unexpected jobs payload shape"
        if error is not None:
            # Into the deciding set: an unread run may be the integration
            # failure, so it keeps a transient sibling from reading transient.
            stepped.append({"run_id": run_id, "job": None, "step": None,
                            "cause": "unknown", "error": error})  # fmt: skip
            continue
        for job in jobs:
            if not isinstance(job, dict):
                continue
            if job.get("conclusion") not in FAILED_RUN_CONCLUSIONS:
                continue
            base = {"run_id": run_id, "job": job.get("name") or ""}
            failed = [
                s
                for s in job.get("steps") or []
                if isinstance(s, dict)
                and s.get("conclusion") in FAILED_STEP_CONCLUSIONS
            ]
            if not failed:
                no_step.append({**base, "step": None, "cause": "unknown"})
                continue
            name = failed[0].get("name") or ""
            stepped.append({**base, "step": name, "cause": step_cause(name)})
    evidence = stepped or no_step
    causes = {e["cause"] for e in evidence}
    if "integration" in causes:
        cause = "integration"
    elif causes == {"transient"}:
        cause = "transient"
    else:
        cause = "unknown"
    first = next((str(r.get("run_id")) for r in failing_runs if r.get("run_id")), None)
    return {
        "cause": cause,
        "evidence": stepped + no_step,
        "next_command": None
        if first is None
        else (
            f"python3 .claude/tools/run_quiet.py -- "
            f"gh run view {first} --repo {repo} --log-failed"
        ),
    }


def _settle_dequeue(verdict: dict, pr: int, repo: str) -> None:
    """Fill a ``dequeued`` verdict's cause, turning it ``conflicting`` when a
    no-run removal reads as a conflict."""
    conflicting = False
    if not verdict.get("queue_runs"):
        # Removed before any queue run started: almost always a sibling PR
        # landing first, so ask GitHub instead of leaving it to four session
        # turns.
        state, _ = settle_mergeability(
            pr, repo, MERGEABLE_SETTLE_RETRIES, MERGEABLE_SETTLE_DELAY
        )
        verdict["mergeable"] = state["mergeable"]
        verdict["merge_state_status"] = state["merge_state_status"]
        # As in `wait`: a failed read and a clean one both show
        # `mergeable: null`, so the reason rides beside it.
        verdict["mergeable_error"] = state["error"]
        conflicting = _is_conflicting(state)
        if conflicting:
            verdict["conclusion"] = "conflicting"
    verdict["dequeue"] = dequeue_cause(
        pr, repo, verdict.get("enqueued_at"), conflicting
    )
    if not conflicting:
        # A no-run dequeue has nothing to read; its timeline reason is the
        # whole answer, so it stays `unknown` and the reason rides beside it.
        verdict["dequeue"]["classification"] = (
            classify_dequeue(verdict["failing_runs"], repo)
            if verdict.get("failing_runs")
            else {
                "cause": "unknown",
                "evidence": [],
                "next_command": None,
                "reason": verdict["dequeue"].get("dequeue_reason"),
            }
        )


def default_since(pr: int, repo: str) -> tuple[str | None, str | None]:
    """The watch's ``since`` when none was passed, and where it came from.

    The newer of the PR's auto-merge time (``auto_merge``) and its queue
    entry's (``queue_entry``), so a cron firing carries no state of its own.
    The newer, never a fixed preference: an auto-merge request that survived
    an earlier cycle's dequeue predates that dequeue, and anchoring on it
    would read the old removal as this cycle's. ``(None, None)`` when neither
    exists or the read fails — the watch then falls back to its own start.
    """
    try:
        node = _queue_graphql(pr, repo)
    except WaitForChecksError:
        return None, None
    candidates = [
        (at, source)
        for at, source in (
            ((node.get("auto") or {}).get("enabledAt"), "auto_merge"),
            ((node.get("entry") or {}).get("enqueuedAt"), "queue_entry"),
        )
        if _parse_time(at) is not None
    ]
    if not candidates:
        return None, None
    return max(candidates, key=lambda c: _parse_time(c[0]))


def watch_queue(
    pr: int,
    repo: str,
    interval: int,
    timeout: int,
    since: str | None,
    await_group_seconds: int = AWAIT_GROUP_SECONDS,
    classic_auto_merge: bool = False,
) -> dict:
    """``--queue``: :func:`wait_queue`, its dequeue cause, and the dismissal.

    SIGTERM and SIGINT raise :class:`Stopped` for the length of the watch,
    so a session that stops it early — having learned the outcome another
    way — still reaches the one ``finally`` that dismisses the PR's threads,
    and gets a ``stopped`` verdict back. Every verdict dismisses but
    ``probe_failed``, which never learned the PR's state, and only ``merged``
    unsubscribes; ``todays_merge_count`` is read on ``merged`` alone. With no
    ``since`` it is defaulted first (:func:`default_since`). The signals are
    ignored while the dismissal runs, so a second stop cannot cut it short,
    and restored after.
    """
    since_source = "flag" if since is not None else None
    if since is None:
        since, since_source = default_since(pr, repo)
    previous = {s: signal.signal(s, _raise_stopped) for s in STOP_SIGNALS}
    verdict: dict = {}
    try:
        verdict = wait_queue(
            pr,
            repo=repo,
            interval=interval,
            timeout=timeout,
            since=since,
            await_group_seconds=await_group_seconds,
            classic_auto_merge=classic_auto_merge,
        )
        verdict["since"] = since
        verdict["since_source"] = since_source
        if verdict["conclusion"] == "merged":
            verdict["todays_merge_count"] = todays_merge_count(pr, repo)
        elif verdict["conclusion"] == "dequeued":
            _settle_dequeue(verdict, pr, repo)
        elif verdict["conclusion"] == "conflicting":
            # Still queued, so there is no removal to read a reason from; the
            # same shape keeps step 22's conflicting branch one branch.
            read = verdict.pop("conflict_read", None) or {}
            verdict["dequeue"] = {
                "dequeue_reason": None,
                "dequeued_at": None,
                **base_gain(repo, read.get("head"), read.get("base")),
            }
        elif verdict["conclusion"] == "not_queued":
            # A firing that began after a dequeue sees only that cycle's
            # failed runs: classify them, so a re-enqueue is never blind.
            reading = verdict.get("not_queued") or {}
            if reading.get("prior_failing_runs"):
                reading["classification"] = classify_dequeue(
                    reading["prior_failing_runs"], repo
                )
    except Stopped as exc:
        # A stop after the watch concluded keeps that conclusion; only the
        # cause read it interrupted is missing.
        verdict = verdict or {
            "pr": pr,
            "repo": repo,
            "mode": "queue",
            "conclusion": "stopped",
            "settled": False,
            "log_path": str(queue_log_path_for(pr)),
        }
        verdict["signal"] = str(exc)
    finally:
        for s in STOP_SIGNALS:
            signal.signal(s, signal.SIG_IGN)
        try:
            # No conclusion is a crash, which, like `probe_failed`, never
            # learned the PR's state.
            if verdict.get("conclusion") not in (None, "probe_failed"):
                verdict["notifications"] = dismiss_pr_notifications(
                    pr, repo, merged=verdict["conclusion"] == "merged"
                )
        finally:
            for s, handler in previous.items():
                signal.signal(s, handler)
    return verdict


def merged_line(verdict: dict) -> str:
    """The one line a ``merged`` watch prints; the full verdict goes to its
    file. Carries the exact merge-count sentence the post-merge tail relays."""
    count = verdict.get("todays_merge_count")
    said = (
        f"Today's merge count is now {count}."
        if count is not None
        else "Today's merge count is unavailable (the read failed)."
    )
    notes = verdict.get("notifications") or {}
    error = notes.get("error")
    dismissed = (
        f"notifications dismissed: {notes.get('dismissed', 0)}"
        if not error
        else f"notification dismissal failed: {error}"
    )
    return f"merged: PR #{verdict.get('pr')} | {said} | {dismissed}"


def queue_verdict_path_for(pr: int) -> Path:
    """Where every ``--queue`` verdict is written in full, beside its log."""
    return log_dir() / f"wait-for-queue-{pr}.json"


# How many times, and how far apart, an enqueue probes for its queue entry.
# A good enqueue registers within seconds; twenty is far past a slow API.
ENQUEUE_PROBES = 4
ENQUEUE_PROBE_DELAY = 5

# Enqueue verdicts the caller may proceed past to the queue watch.
ENQUEUE_OK = frozenset({"queued", "merged", "auto_merge_pending"})


def merge_queue_active(repo: str, base: str) -> bool | None:
    """Whether ``merge_queue`` is an active rule on ``base``; ``None`` when the
    rules read fails, so an unknown is never read as either answer."""
    try:
        rules = _gh_json(
            ["api", f"repos/{repo}/rules/branches/{quote(base)}"], "branch rules read"
        )
    except WaitForChecksError:
        return None
    if not isinstance(rules, list):
        return None
    return any(isinstance(r, dict) and r.get("type") == "merge_queue" for r in rules)


def _probe_entry(pr, repo, queue_rule, probe, sleep) -> tuple[str | None, dict]:
    """``(conclusion, node)`` after up to :data:`ENQUEUE_PROBES` reads.

    ``merged`` or ``queued`` concludes at once. Classic auto-merge (no queue
    rule) concludes ``auto_merge_pending`` at once too: it waits on checks
    with no queue to enter. Under a queue rule, auto-merge with no entry is
    the exit-0-never-queued shape, so it keeps probing and concludes
    nothing. ``node`` is the last good read, ``None`` when every read failed.
    """
    node: dict | None = None
    for attempt in range(ENQUEUE_PROBES):
        if attempt:
            sleep(ENQUEUE_PROBE_DELAY)
        try:
            node = probe(pr, repo)
        except WaitForChecksError:
            continue
        if node.get("gql_merged") or node.get("gql_state") == "MERGED":
            return "merged", node
        if node.get("entry"):
            return "queued", node
        if node.get("auto") and queue_rule is not True:
            return "auto_merge_pending", node
    return None, node


def enqueue(
    pr: int,
    repo: str,
    reviewed: str,
    probe=None,
    sleep=time.sleep,
) -> dict:
    """``--enqueue``: the handoff yes to one verdict.

    Refuses before any write (``refused``) when the local HEAD is not the
    PR's pushed head — the review read one tree and the queue would merge
    another — or when the branch's fingerprint
    (``review_diff.diff_fingerprint``) differs from ``reviewed``, the one the
    findings catalogue was reviewed against: the diff moved after review, so
    a scoped re-review of the delta runs first and its fingerprint is passed
    instead.

    Then the enqueue. Where ``merge_queue`` is an active rule on the PR's
    base, a plain ``gh pr merge`` **enqueues** (it cannot bypass the queue),
    so it goes first — ``--auto`` can exit 0 with auto-merge enabled and
    never queue — and ``--auto`` is the fallback when the plain call fails
    or takes nothing. With no queue rule, or an unreadable one, ``--auto``
    alone. Each attempt that exits 0 is probed for its queue entry.

    ``conclusion``: ``queued`` (``entry`` carries its state and position),
    ``merged``, ``auto_merge_pending`` (auto-merge on, no entry yet — the
    queue watch follows it), ``not_queued`` (nothing took), ``enqueue_failed``
    (every attempt exited non-zero), ``probe_failed`` (an attempt took but no
    probe answered), ``refused``. ``enqueued_at`` is the entry's (or
    auto-merge's) time, which the queue watch defaults to.

    Under a queue rule ``auto_merge_pending`` cannot tell checks still
    running (the re-sign's re-run, legitimate) from auto-merge that will
    never queue; it hands both to the watch, whose bounded wait on that
    shape is a separate change. A pull read without a head or base, or a
    fingerprint that cannot be computed, raises (exit 2, no verdict).
    """
    probe = probe or _queue_graphql
    verdict: dict = {
        "pr": pr,
        "repo": repo,
        "mode": "enqueue",
        "conclusion": None,
        "reviewed_fingerprint": reviewed,
        "fingerprint": None,
        "merge_queue": None,
        "attempts": [],
        "entry": None,
        "enqueued_at": None,
        "error": None,
    }
    pull = _gh_json(["api", f"repos/{repo}/pulls/{pr}"], "pull read")
    remote = (pull.get("head") or {}).get("sha") if isinstance(pull, dict) else None
    base = (pull.get("base") or {}).get("ref") if isinstance(pull, dict) else None
    if not remote or not base:
        raise WaitForChecksError("pull read carried no head sha or base ref")
    code, out, err = _git(["rev-parse", "HEAD"])
    local = out.decode(errors="replace").strip() if code == 0 else None
    if local != remote:
        verdict["conclusion"] = "refused"
        verdict["error"] = (
            f"local HEAD {local or err.strip()} is not PR #{pr}'s head {remote} — "
            f"push (or pull) so the reviewed tree is the one enqueued"
        )
        return verdict
    try:
        fingerprint = review_diff.diff_fingerprint(f"origin/{base}")
    except review_diff.ReviewDiffError as exc:
        raise WaitForChecksError(f"fingerprint: {exc}") from exc
    verdict["fingerprint"] = fingerprint
    if fingerprint != reviewed:
        verdict["conclusion"] = "refused"
        verdict["error"] = (
            f"the diff changed since review (fingerprint {fingerprint}, reviewed "
            f"{reviewed}) — run a scoped re-review of the delta, then pass its "
            f"fingerprint"
        )
        return verdict

    queue_rule = merge_queue_active(repo, base)
    verdict["merge_queue"] = queue_rule
    plain = ["pr", "merge", str(pr), "--repo", repo]
    commands = [plain, [*plain, "--auto"]] if queue_rule else [[*plain, "--auto"]]
    node: dict = {}
    read_any = False
    conclusion = None
    for command in commands:
        rc, _, err = _gh(command)
        verdict["attempts"].append(
            {"command": "gh " + " ".join(command), "exit": rc,
             "stderr": err.strip()[-300:] or None}
        )  # fmt: skip
        if rc != 0:
            continue
        conclusion, read = _probe_entry(pr, repo, queue_rule, probe, sleep)
        if read is not None:
            node, read_any = read, True
        if conclusion is not None:
            break
    if conclusion is None:
        if all(a["exit"] != 0 for a in verdict["attempts"]):
            conclusion = "enqueue_failed"
        elif not read_any:
            # An attempt took, but no probe ever answered: unknown, not absent.
            conclusion = "probe_failed"
        elif node.get("auto"):
            conclusion = "auto_merge_pending"
        else:
            conclusion = "not_queued"
    entry = node.get("entry") or None
    verdict["conclusion"] = conclusion
    verdict["entry"] = entry
    verdict["enqueued_at"] = (entry or {}).get("enqueuedAt") or (
        node.get("auto") or {}
    ).get("enabledAt")
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
        "--enqueue",
        dest="enqueue_pr",
        type=int,
        help="enqueue a reviewed PR and probe that it queued",
    )
    parser.add_argument(
        "--reviewed-fingerprint",
        help="with --enqueue: the review_diff.py fingerprint the findings "
        "catalogue was reviewed against",
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
    parser.add_argument(
        "--await-group-seconds",
        type=int,
        default=None,
        help="with --queue: seconds a queue entry may go without a merge-group "
        f"run before the watch returns queued_no_group (default {AWAIT_GROUP_SECONDS})",
    )
    parser.add_argument(
        "--classic-auto-merge",
        action="store_true",
        help="with --queue: the repo has no merge queue, so an enabled "
        "auto-merge is the queued signal rather than auto_merge_waiting",
    )
    parser.add_argument("--repo", default=DEFAULT_REPO, help="owner/repo")
    parser.add_argument(
        "--interval",
        type=int,
        default=None,
        help=f"seconds between polls (default ${POLL_ENV}, else {DEFAULT_POLL})",
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

    modes = [args.pr, args.run_id, args.queue_pr, args.mergeable_pr, args.enqueue_pr]
    if sum(m is not None for m in modes) != 1:
        # Requiring exactly one keeps the modes from silently ranking each
        # other: a caller that passed two would otherwise watch whichever the
        # code happens to check first and report a verdict about the other thing.
        raise WaitForChecksError(
            "pass exactly one of --pr, --run, --queue, --mergeable or --enqueue"
        )
    if (args.reviewed_fingerprint is None) != (args.enqueue_pr is None):
        raise WaitForChecksError(
            "--enqueue and --reviewed-fingerprint are passed together"
        )
    for flag, value in (
        ("--since", args.since),
        ("--await-group-seconds", args.await_group_seconds),
        ("--classic-auto-merge", args.classic_auto_merge or None),
    ):
        if value is not None and args.queue_pr is None:
            raise WaitForChecksError(f"{flag} is meaningful only with --queue")
    if args.await_group_seconds is not None and args.await_group_seconds <= 0:
        raise WaitForChecksError("--await-group-seconds must be positive")
    if args.snapshot is not None and args.pr is None:
        raise WaitForChecksError("--snapshot is meaningful only with --pr")
    if args.base is not None and args.snapshot is None:
        raise WaitForChecksError("--base is meaningful only with --snapshot")
    interval = poll_interval() if args.interval is None else args.interval

    if args.mergeable_pr is not None:
        if args.no_watch or args.interval is not None or args.timeout is not None:
            raise WaitForChecksError(
                "--no-watch, --interval and --timeout are meaningless with "
                "--mergeable, whose settle loop is fixed"
            )
        verdict = wait_mergeable(args.mergeable_pr, repo=args.repo)
    elif args.enqueue_pr is not None:
        if args.no_watch or args.interval is not None or args.timeout is not None:
            raise WaitForChecksError(
                "--no-watch, --interval and --timeout are meaningless with "
                "--enqueue, whose probe is fixed"
            )
        verdict = enqueue(args.enqueue_pr, args.repo, args.reviewed_fingerprint)
    elif args.queue_pr is not None:
        if args.no_watch:
            raise WaitForChecksError("--no-watch is meaningless with --queue")
        if args.since is not None and _parse_time(args.since) is None:
            raise WaitForChecksError(f"--since is not an ISO-8601 time: {args.since}")
        verdict = watch_queue(
            args.queue_pr,
            args.repo,
            interval,
            QUEUE_TIMEOUT if args.timeout is None else args.timeout,
            args.since,
            await_group_seconds=(
                AWAIT_GROUP_SECONDS
                if args.await_group_seconds is None
                else args.await_group_seconds
            ),
            classic_auto_merge=args.classic_auto_merge,
        )
    else:
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
    # The queue file first: it is the one the merged line names, so a bad
    # `--verdict-out` path cannot leave that name pointing at nothing.
    outputs = (
        [queue_verdict_path_for(args.queue_pr)] if args.queue_pr is not None else []
    )
    if args.verdict_out is not None:
        outputs.append(args.verdict_out)
    # Stdout first: a verdict can take an hour to earn, and a bad
    # `--verdict-out` path must not throw it away. A merge is one line: the
    # session's next act is the post-merge tail, which needs nothing more.
    if args.queue_pr is not None and verdict.get("conclusion") == "merged":
        print(f"{merged_line(verdict)} | verdict: {outputs[0]}")
    else:
        json.dump(verdict, sys.stdout, indent=2)
        sys.stdout.write("\n")
    for path in outputs:
        try:
            path.write_text(json.dumps(verdict, indent=2) + "\n")
        except OSError as exc:
            raise WaitForChecksError(f"cannot write {path}: {exc}")
    ok = ("pass", "merged", "mergeable")
    if args.enqueue_pr is not None:
        ok = ENQUEUE_OK
    return 0 if verdict["conclusion"] in ok else 1


def main() -> int:
    try:
        return run(sys.argv)
    except WaitForChecksError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
