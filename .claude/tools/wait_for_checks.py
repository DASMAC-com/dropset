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

So the wait is **two** ``gh`` invocations, no loop anywhere:

1. ``gh pr checks --watch`` — gh does the pacing and exits when the checks
   settle. Its live-updating table is captured to a log, never to stdout, for
   the same reason ``run_quiet.py`` captures a build log.
2. ``gh pr checks --json`` — one structured read that *is* the verdict, including
   every failing check's name, workflow, and link.

The JSON read, not the watch's exit code, is the authority on the outcome: ``gh``
overloads its exit status (non-zero covers "a check failed", "checks are still
pending", and "there are no checks at all"), and a review must distinguish those.

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
      "conclusion": "pass",      // pass | fail | pending | none | timeout
      "settled": true,
      "elapsed_seconds": 127,
      "counts": {"pass": 12, "fail": 0, "pending": 0, "skipping": 3},
      "failing": [{"name": "…", "workflow": "…", "link": "…", "run_id": "…"}],
      "log_path": "/…/wait-for-checks-285.log"
    }

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

# gh's own refresh cadence under --watch. 30s matches CI's granularity: the
# shortest job on this repo is several seconds and the longest many minutes, so a
# tighter interval only produces redraws nobody reads.
DEFAULT_INTERVAL = 30

# An outer bound on the watch, so a wedged or cancelled run can't hold the
# session open indefinitely. An hour is well past this repo's slowest suite.
DEFAULT_TIMEOUT = 3600

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
        "unresolved_buckets": unresolved,
    }


def wait(
    pr: int,
    repo: str = DEFAULT_REPO,
    interval: int = DEFAULT_INTERVAL,
    timeout: int = DEFAULT_TIMEOUT,
    watch: bool = True,
) -> dict:
    """The whole wait: watch until settled, read once, summarize."""
    log = log_path_for(pr)
    started = time.monotonic()
    settled = True
    if watch:
        settled = watch_checks(pr, repo, interval, timeout, log)
    elapsed = int(time.monotonic() - started)

    summary = summarize(read_checks(pr, repo))
    conclusion = summary["conclusion"]
    if not settled and conclusion != "fail":
        # A timed-out watch must never claim `pass` off a snapshot it stopped
        # waiting on. But a `fail` it *did* observe is definite and strictly more
        # informative than `timeout`, so that one survives — otherwise a caller
        # branching on `conclusion` can't tell a wedged run from a red one.
        conclusion = "timeout"

    return {
        "pr": pr,
        "repo": repo,
        "conclusion": conclusion,
        "settled": settled,
        "elapsed_seconds": elapsed,
        "counts": summary["counts"],
        "failing": summary["failing"],
        "unresolved_buckets": summary["unresolved_buckets"],
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
