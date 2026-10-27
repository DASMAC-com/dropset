#!/usr/bin/env python3
"""Race the abridged local checks against CI for one HEAD, in one call.

A verification point used to be turns: run ``make lint-abridged``, read it,
run ``make test-abridged``, read it, then wait on CI for the same commit and
read that. Each wait was a turn replayed on every later one. This tool does
the whole fall-through inside one Bash call::

    python3 .claude/tools/verify_head.py
    python3 .claude/tools/verify_head.py --pr 656 --timeout 2400

1. **Record HEAD's SHA** and start the local run: ``make lint-abridged``,
   then ``make test-abridged`` — sequential, since both want the cargo pool
   lock. Each stage is its own process group, logged under the checks
   watcher's log directory (``wait_for_checks.py``), never to the session.
2. **Poll CI when HEAD is pushed and the tree clean** (HEAD is its
   upstream, nothing uncommitted — CI checks the commit, the local stages
   the tree): every ``--interval`` seconds, one
   ``gh pr view --json headRefOid`` and the watcher's own
   ``gh pr checks --json`` read, believed only while the PR's head is this
   SHA. Otherwise the run is local only and the verdict says why.
3. **Each check settles on whichever side finishes first.** CI's ``Lint``
   green kills the local lint; every ``Tests*`` check green — or skipping,
   which is what CI's paths-filter does — kills or skips the local test. A
   local pass is a pass: CI's full sweep still completes behind it, and the
   merge queue runs it again, so nothing reaches main on an abridged run.
4. **A red on either side ends the race.** A CI failure kills the local run
   and names the job; a local failure names the log line and leaves CI
   running.
5. **HEAD moving** ends the race (commit, then call again); ``--timeout``
   bounds it; a CI read failing :data:`MAX_CI_ERRORS` times in a row stops
   the polling, and the local side settles alone.
6. **The kill is a process-group signal** (``killpg``, never the leader
   alone), so no orphan ``rustc`` keeps the pool lock. That lock is a file
   lock released on process death, and cargo discards a partial artifact on
   its next run.

Prints one verdict line, then appends the machine-load probe's ``--sample``
line to the fleet log (best-effort)::

    verify-head: GREEN sha=0123456789ab lint=ci:green[Lint] \
test=local:green[/…/verify-head-0123456789ab-test.log] wall=212s \
killed=lint ci=consulted

``outcome`` is ``GREEN`` / ``RED`` / ``HEAD-MOVED`` / ``TIMEOUT``; each check
is ``<side>:<state>[<detail>]``, ``side`` ``ci`` / ``local`` / ``-`` (never
settled), detail the job name or the log path (``path:line: text`` on a
local red). ``ci`` is ``consulted`` or ``not-consulted(<why>)``: on
``unpushed`` or ``dirty`` push for the full sweep, and on ``unreachable`` run
``make lint`` / ``make test`` before trusting a local green. **Compare the
``sha`` with HEAD before acting on the verdict.**

Exit 0 on ``GREEN``, 1 on anything else, 2 on a usage error.

Standard library only; not a Cargo workspace member (``CLAUDE.md`` → "Skill
tooling"). Tests: ``tests/test_verify_head.py``, via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import signal
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

import wait_for_checks as wfc

#: The two checks, in local run order, and the make target each runs.
STAGES = (("lint", ["make", "lint-abridged"]), ("test", ["make", "test-abridged"]))

#: CI's check for each stage: ``Lint`` exactly, and every ``Tests*`` job.
CI_LINT = re.compile(r"^Lint$")
CI_TEST = re.compile(r"^Tests\b")

DEFAULT_INTERVAL = 30
DEFAULT_TIMEOUT = 3600

#: Consecutive failed CI reads before the race stops consulting CI.
MAX_CI_ERRORS = 3

#: Seconds between local-process polls; CI is polled every ``interval``.
TICK = 1.0

#: Seconds a killed group gets after SIGTERM before SIGKILL.
KILL_GRACE = 5.0

#: The first line of a local log that names a failure, else its last line.
_FAIL_LINE = re.compile(r"\.Failed$|FAILED|^error(\[|:)|panicked at")

#: The ``ci=`` field when CI was not consulted, last on the line so it can
#: carry the instruction that goes with it.
NOT_PUSHED = "not-consulted(unpushed) - push for the full sweep"
DIRTY = "not-consulted(dirty) - commit and push for the full sweep"
UNREACHABLE = "not-consulted(unreachable) - run make lint and make test"


@dataclass
class Settled:
    side: str  # ci | local
    green: bool
    detail: str


@dataclass
class Race:
    sha: str
    settled: dict[str, Settled] = field(default_factory=dict)
    killed: list[str] = field(default_factory=list)
    outcome: str = ""
    ci_note: str = ""


def log_path_for(sha: str, stage: str) -> Path:
    """A stage's log, in the checks watcher's directory, keyed on the SHA."""
    return wfc.log_dir() / f"verify-head-{sha[:12]}-{stage}.log"


def failure_line(log: Path) -> str:
    """``path:line: text`` for the line a local red should be read at."""
    try:
        lines = log.read_text(errors="replace").splitlines()
    except OSError:
        return str(log)
    hit = next(
        (i for i, text in enumerate(lines) if _FAIL_LINE.search(text.strip())), None
    )
    if hit is None:
        hit = max((i for i, text in enumerate(lines) if text.strip()), default=-1)
    if hit < 0:
        return str(log)
    return f"{log}:{hit + 1}: {lines[hit].strip()[:160]}"


class Stage:
    """One local stage, run as its own process group."""

    def __init__(self, name: str, cmd: list[str], log: Path):
        self.name = name
        self.log = log
        with open(log, "wb") as out:
            self.proc = subprocess.Popen(
                cmd, stdout=out, stderr=subprocess.STDOUT, start_new_session=True
            )

    def poll(self) -> int | None:
        return self.proc.poll()

    def kill(self) -> None:
        for sig, grace in ((signal.SIGTERM, KILL_GRACE), (signal.SIGKILL, None)):
            try:
                os.killpg(self.proc.pid, sig)
            except ProcessLookupError:
                return
            try:
                self.proc.wait(timeout=grace)
                return
            except subprocess.TimeoutExpired:
                continue


def _git(args: list[str]) -> str:
    done = subprocess.run(["git", *args], capture_output=True, text=True, check=False)
    return done.stdout.strip() if done.returncode == 0 else ""


def read_head() -> str:
    return _git(["rev-parse", "HEAD"])


def local_only_reason(sha: str) -> str:
    """Why CI cannot speak for this tree, or ``""`` when it can.

    CI checks the pushed commit and the local stages check the working tree,
    so a CI verdict settles a check only when the two are the same bytes:
    HEAD is its upstream and nothing is uncommitted.
    """
    if _git(["rev-parse", "@{upstream}"]) != sha:
        return NOT_PUSHED
    if _git(["status", "--porcelain"]):
        return DIRTY
    return ""


def bucket_state(checks: list[dict], pattern: re.Pattern) -> tuple[str, str]:
    """``green`` / ``red`` / ``pending`` for the checks ``pattern`` names.

    ``skipping`` counts as green: it is how CI's paths-filter reports a PR
    no test can see. No matching check registered yet reads as pending.
    """
    mine = [c for c in checks if pattern.search(c.get("name") or "")]
    red = [c for c in mine if c.get("bucket") in ("fail", "cancel")]
    if red:
        return "red", ",".join(c["name"] for c in red)
    if mine and all(c.get("bucket") in ("pass", "skipping") for c in mine):
        return "green", ",".join(c["name"] for c in mine)
    return "pending", ""


def read_ci(sha: str, pr: str, repo: str) -> dict[str, tuple[str, str]] | None:
    """Each stage's CI state for ``sha``, or ``None`` while CI is on another head.

    Raises :class:`wfc.WaitForChecksError` when either read fails.
    """
    # `gh pr view --repo` needs an argument, so with no PR name the branch.
    target = pr or _git(["rev-parse", "--abbrev-ref", "HEAD"])
    view = ["pr", "view", target, "--repo", repo, "--json", "number,headRefOid"]
    code, out, err = wfc._gh(view)
    if code != 0:
        raise wfc.WaitForChecksError(f"gh pr view failed: {err.strip()}")
    head = json.loads(out)
    if head.get("headRefOid") != sha:
        return None
    checks = wfc.read_checks(int(head["number"]), repo)
    return {
        "lint": bucket_state(checks, CI_LINT),
        "test": bucket_state(checks, CI_TEST),
    }


def race(
    sha: str,
    local_only: str,
    *,
    start: Callable[[str], object],
    ci: Callable[[], dict[str, tuple[str, str]] | None],
    head: Callable[[], str],
    clock: Callable[[], float],
    sleep: Callable[[float], None],
    interval: float,
    timeout: float,
) -> Race:
    """The race itself, with every side effect injected (see the tests).

    ``local_only`` is :func:`local_only_reason`'s answer: non-empty, CI is
    never read and the reason is the verdict's ``ci=`` field.
    """
    r = Race(sha=sha, ci_note=local_only or "consulted")
    polling = not local_only
    errors = 0
    began = clock()
    next_ci = began
    order = [name for name, _ in STAGES]
    current = None  # (name, Stage)

    def stop_local(name: str | None = None) -> None:
        nonlocal current
        if current and (name is None or current[0] == name):
            current[1].kill()
            r.killed.append(current[0])
            current = None

    while True:
        if head() != sha:
            stop_local()
            r.outcome = "HEAD-MOVED"
            return r
        if clock() - began >= timeout:
            stop_local()
            r.outcome = "TIMEOUT"
            return r

        if polling and clock() >= next_ci:
            next_ci = clock() + interval
            try:
                states = ci()
                errors = 0
            except (wfc.WaitForChecksError, ValueError, KeyError):
                states = None
                errors += 1
                if errors >= MAX_CI_ERRORS:
                    polling = False
                    r.ci_note = UNREACHABLE
            for name, (state, jobs) in (states or {}).items():
                if name in r.settled or state == "pending":
                    continue
                r.settled[name] = Settled("ci", state == "green", jobs)
                if state == "red":
                    stop_local()
                    r.outcome = "RED"
                    return r
                stop_local(name)

        if current and current[0] in r.settled:
            stop_local(current[0])
        if current:
            code = current[1].poll()
            if code is not None:
                name, stage = current
                current = None
                if code == 0:
                    r.settled[name] = Settled("local", True, str(stage.log))
                else:
                    r.settled[name] = Settled("local", False, failure_line(stage.log))
                    r.outcome = "RED"
                    return r
        if current is None:
            todo = [n for n in order if n not in r.settled]
            if not todo:
                r.outcome = "GREEN"
                return r
            current = (todo[0], start(todo[0]))
        sleep(TICK)


def render(r: Race, wall: float) -> str:
    def check(name: str) -> str:
        s = r.settled.get(name)
        if s is None:
            return f"{name}=-:unsettled"
        return f"{name}={s.side}:{'green' if s.green else 'red'}[{s.detail}]"

    return " ".join(
        [
            f"verify-head: {r.outcome}",
            f"sha={r.sha[:12]}",
            *(check(name) for name, _ in STAGES),
            f"wall={round(wall)}s",
            f"killed={','.join(r.killed) or '-'}",
            f"ci={r.ci_note}",
        ]
    )


def sample_fleet() -> None:
    """Append the machine-load probe's sample line; best-effort, silent."""
    probe = Path(__file__).with_name("machine_load.py")
    try:
        subprocess.run(
            [sys.executable, str(probe), "--sample"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
            timeout=60,
        )
    except (OSError, subprocess.SubprocessError):
        pass


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--pr", default="", help="PR number (default: the branch's)")
    parser.add_argument("--repo", default=wfc.DEFAULT_REPO)
    parser.add_argument("--interval", type=int, default=DEFAULT_INTERVAL)
    parser.add_argument("--timeout", type=int, default=DEFAULT_TIMEOUT)
    opts = parser.parse_args(argv)
    if opts.interval < 1 or opts.timeout < 1:
        parser.error("--interval and --timeout must be positive")

    sha = read_head()
    if not sha:
        print("verify-head: cannot read HEAD", file=sys.stderr)
        return 2
    commands = dict(STAGES)
    started: list[Stage] = []

    def start(name: str) -> Stage:
        started.append(Stage(name, commands[name], log_path_for(sha, name)))
        return started[-1]

    began = time.monotonic()
    try:
        result = race(
            sha,
            local_only_reason(sha),
            start=start,
            ci=lambda: read_ci(sha, opts.pr, opts.repo),
            head=read_head,
            clock=time.monotonic,
            sleep=time.sleep,
            interval=opts.interval,
            timeout=opts.timeout,
        )
    except BaseException:
        # An interrupt or a crash must not leave a stage holding the pool lock.
        for stage in started:
            if stage.poll() is None:
                stage.kill()
        raise
    print(render(result, time.monotonic() - began), flush=True)
    sample_fleet()
    return 0 if result.outcome == "GREEN" else 1


if __name__ == "__main__":
    raise SystemExit(main())
