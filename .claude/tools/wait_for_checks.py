#!/usr/bin/env python3
"""Wait for a PR's CI checks to settle, then print one compact verdict.

This is ``review-pr`` step 17's wait, hardened into a tool. What it replaces was
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

Usage::

    python3 .claude/tools/wait_for_checks.py --pr 285
    python3 .claude/tools/wait_for_checks.py --pr 285 --interval 30 --timeout 1800
    python3 .claude/tools/wait_for_checks.py --run 1234567890

Options: exactly one of ``--pr`` / ``--run``, ``--repo`` (default
``DASMAC-com/dropset``),
``--interval`` (seconds between gh's own refreshes, default 30; honored in
**both** modes), ``--timeout``
(seconds before giving up on the watch, default 3600), ``--no-watch`` (skip the
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
      "log_path": "/…/wait-for-checks-285.log"
    }

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

Exit code is 0 when ``conclusion`` is ``pass``, else 1 — so a caller that only
checks the status still cannot mistake a red build for green.

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


def wait(
    pr: int,
    repo: str = DEFAULT_REPO,
    interval: int = DEFAULT_INTERVAL,
    timeout: int = DEFAULT_TIMEOUT,
    watch: bool = True,
) -> dict:
    """The whole wait: watch, read, and re-watch while the read says pending.

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
    literally treats a CI-unverified commit as green.

    A commit that genuinely has no checks still reports ``none`` once the
    rounds are spent, **not** ``timeout`` — the rounds established the absence
    rather than timing out on a wedged run, and collapsing the two would trade
    one wrong answer for another.
    """
    log = log_path_for(pr)
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
        merge_state = mergeability(pr, repo)
        state = (merge_state.get("mergeable") or "").upper()
        if state == "UNKNOWN":
            # GitHub computes mergeability ASYNCHRONOUSLY, and right after a
            # push — precisely the window this probe runs in — `UNKNOWN` is the
            # common answer. Taking it as "not conflicting" would report a
            # genuinely conflicting PR as `none` and reproduce the exact
            # 20-minute dead wait this probe was added to prevent. So re-probe
            # rather than concluding from it.
            for _ in range(MERGEABLE_RETRIES):
                time.sleep(MERGEABLE_RETRY_DELAY)
                merge_state = mergeability(pr, repo)
                state = (merge_state.get("mergeable") or "").upper()
                if state != "UNKNOWN":
                    break
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

    return {
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
        "log_path": str(log),
    }


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="wait_for_checks.py")
    parser.add_argument("--pr", type=int, help="PR number (checks mode)")
    parser.add_argument(
        "--run",
        dest="run_id",
        help="workflow run id to watch instead of a PR's checks (queue mode)",
    )
    parser.add_argument("--repo", default=DEFAULT_REPO, help="owner/repo")
    parser.add_argument(
        "--interval",
        type=int,
        default=DEFAULT_INTERVAL,
        help=f"seconds between gh's refreshes (default {DEFAULT_INTERVAL})",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=DEFAULT_TIMEOUT,
        help=f"seconds before giving up on the watch (default {DEFAULT_TIMEOUT})",
    )
    parser.add_argument(
        "--no-watch",
        action="store_true",
        help="read the current state once instead of waiting",
    )
    args = parser.parse_args(argv[1:])

    if (args.pr is None) == (args.run_id is None):
        # Requiring exactly one keeps the two modes from silently ranking each
        # other: a caller that passed both would otherwise watch whichever the
        # code happens to check first and report a verdict about the other thing.
        raise WaitForChecksError("pass exactly one of --pr or --run")

    if args.run_id is not None:
        if args.no_watch:
            raise WaitForChecksError("--no-watch is meaningless with --run")
        verdict = wait_run(
            args.run_id,
            repo=args.repo,
            timeout=args.timeout,
            interval=args.interval,
        )
    else:
        verdict = wait(
            args.pr,
            repo=args.repo,
            interval=args.interval,
            timeout=args.timeout,
            watch=not args.no_watch,
        )
    json.dump(verdict, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0 if verdict["conclusion"] == "pass" else 1


def main() -> int:
    try:
        return run(sys.argv)
    except WaitForChecksError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
