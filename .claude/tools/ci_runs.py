#!/usr/bin/env python3
"""Start a PR's one remote CI run, and cancel the runs a push superseded.

Every session commit carries the ``skip-checks: true`` trailer, so a push
starts no ``pull_request`` workflow and the required checks stay pending (a
trigger-level skip blocks the merge; a job-level ``if:`` would report
success). GitHub reads the trailer only after two empty lines, so the
commit paths write it with ``--cleanup=verbatim`` (with one blank line the
push runs every workflow; measured on PR 771). The remote run starts only
when this tool makes the one commit without it::

    python3 .claude/tools/ci_runs.py trigger --pr 771
    python3 .claude/tools/ci_runs.py cancel-stale --pr 771

``trigger`` checks the PR's head branch is the checked-out one, that the
remote tip is an ancestor of ``HEAD`` (never forcing over another session's
push) and that nothing is staged, then probes the signer
(``signing_probe.py``) and refuses with its ``LOCKED`` line when it does not
answer, so the merge gate can hold the run until the operator unlocks rather
than verify a head the re-sign will rewrite. Then it makes one empty signed
commit, :data:`TRIGGER_SUBJECT`, pushes it as a plain fast-forward (a failed
push drops the commit again), and runs ``cancel-stale``, whose failure is
reported on the line and never undoes the trigger.

``cancel-stale`` lists the branch's ``pull_request`` workflow runs, then reads
the PR's head, and cancels every run not yet completed whose head SHA differs
from it. The head is read **after** the runs, so a listed run can never sit
on a head newer than the one it is compared with: a run on the current head
is never cancelled (a cancelled run reads as a failure, and costs a
re-trigger). After a push the caller passes the pushed SHA, since the PR's
head can trail it: the head is re-read until it matches (bounded), and a run
on that SHA is kept either way. ``resign_branch.py resign`` calls it after
its force-push.

Each prints one verdict line starting ``ci-runs |`` and exits 0 on success,
1 on a refusal (a locked signer, another branch, a failed read or push), 2
on a usage error.

Standard library only; not a Cargo workspace member (``CLAUDE.md`` → "Skill
tooling"). Tests: ``tests/test_ci_runs.py``, via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from typing import Callable

PREFIX = "ci-runs |"
DEFAULT_REPO = "DASMAC-com/dropset"

#: The trigger commit's subject. It carries no ``skip-checks`` trailer.
TRIGGER_SUBJECT = "ci: Run the remote checks"

#: A run in any other status is finished and has nothing to cancel.
DONE = "completed"

#: How often, and how far apart, a read waits for the PR head to show a push.
HEAD_WAIT_TRIES = 5
HEAD_WAIT_S = 2.0

#: One subprocess call: argv in, ``(returncode, stdout, stderr)`` out.
Runner = Callable[[list[str]], "tuple[int, str, str]"]


class Refused(Exception):
    """A refusal; the message is the verdict line's tail."""


def run(argv: list[str]) -> tuple[int, str, str]:
    proc = subprocess.run(
        argv, capture_output=True, text=True, errors="replace", check=False
    )
    return proc.returncode, proc.stdout, proc.stderr


def _ok(runner: Runner, argv: list[str]) -> str:
    code, out, err = runner(argv)
    if code != 0:
        lines = (err or out).strip().splitlines()
        raise Refused(f"`{' '.join(argv[:3])}` failed: {lines[-1] if lines else code}")
    return out


def _json(runner: Runner, argv: list[str]) -> object:
    try:
        return json.loads(_ok(runner, argv))
    except json.JSONDecodeError as exc:
        raise Refused(f"`{' '.join(argv[:3])}` returned no JSON") from exc


def read_pr(runner: Runner, repo: str, pr: int) -> tuple[str, str]:
    """The PR's head branch and head SHA."""
    data = _json(runner, ["gh", "api", f"repos/{repo}/pulls/{pr}"])
    head = data.get("head") if isinstance(data, dict) else None
    if not isinstance(head, dict) or not head.get("ref") or not head.get("sha"):
        raise Refused(f"PR {pr} has no head to read")
    return head["ref"], head["sha"]


def active_runs(runner: Runner, repo: str, branch: str) -> list[dict]:
    """The branch's ``pull_request`` workflow runs not yet completed."""
    data = _json(
        runner,
        [
            "gh",
            "api",
            "-X",
            "GET",
            f"repos/{repo}/actions/runs",
            "-f",
            f"branch={branch}",
            "-f",
            "event=pull_request",
            "-f",
            "per_page=100",
        ],
    )
    runs = data.get("workflow_runs") if isinstance(data, dict) else None
    if not isinstance(runs, list):
        raise Refused("the runs list returned no `workflow_runs`")
    return [r for r in runs if isinstance(r, dict) and r.get("status") != DONE]


def stale(runs: list[dict], kept: set[str]) -> list[dict]:
    """The runs to cancel: every one whose commit is not a ``kept`` SHA."""
    return [r for r in runs if r.get("head_sha") not in kept]


def cancel_stale(
    runner: Runner,
    repo: str,
    pr: int,
    expect: str = "",
    sleep: Callable[[float], None] = time.sleep,
) -> str:
    """Cancel the PR's active runs on any SHA but its head.

    ``expect`` is a SHA just pushed: the PR's head can trail a push for a
    moment, so the head is re-read until it matches (bounded), and a run on
    ``expect`` is kept even if it never does — a lagging read must not
    cancel the newest run.
    """
    branch, _ = read_pr(runner, repo, pr)
    runs = active_runs(runner, repo, branch)
    # Read after the runs, never before: see the module docstring.
    _, head = read_pr(runner, repo, pr)
    for _ in range(HEAD_WAIT_TRIES if expect else 0):
        if head == expect:
            break
        sleep(HEAD_WAIT_S)
        _, head = read_pr(runner, repo, pr)
    kept = {head, expect} - {""}
    doomed = stale(runs, kept)
    failed = []
    for r in doomed:
        code, _, err = runner(
            ["gh", "api", "-X", "POST", f"repos/{repo}/actions/runs/{r['id']}/cancel"]
        )
        # 409: the run finished between the list and the cancel.
        if code != 0 and "409" not in err:
            failed.append(str(r["id"]))
    line = (
        f"cancelled {len(doomed) - len(failed)} of {len(runs)} active "
        f"run(s), head {head[:12]} kept"
    )
    if expect and expect != head:
        line += f" (and pushed {expect[:12]}; the PR head had not caught up)"
    if failed:
        raise Refused(f"{line}; cancel failed for run(s) {', '.join(failed)}")
    return line


def trigger(
    runner: Runner, repo: str, pr: int, probe: Callable[[], tuple[int, str]]
) -> str:
    branch = _ok(runner, ["git", "symbolic-ref", "--short", "HEAD"]).strip()
    pr_branch, _ = read_pr(runner, repo, pr)
    if pr_branch != branch:
        raise Refused(f"PR {pr} heads `{pr_branch}`, not the checked-out `{branch}`")
    remote = _ok(runner, ["git", "ls-remote", "origin", f"refs/heads/{branch}"])
    remote = remote.split()[0] if remote.strip() else ""
    if not remote:
        raise Refused(f"origin/{branch} does not exist; push the branch first")
    # The push below is a plain fast-forward, so a remote holding commits
    # this checkout lacks refuses it; checking first keeps the commit unmade.
    code, _, _ = runner(["git", "merge-base", "--is-ancestor", remote, "HEAD"])
    if code != 0:
        raise Refused(
            f"origin/{branch} ({remote[:9]}) holds commits this checkout lacks; "
            "pull or rebase first"
        )
    if runner(["git", "diff", "--cached", "--quiet"])[0] != 0:
        raise Refused("the index holds staged changes; the trigger commit is empty")
    code, line = probe()
    if code != 0:
        raise Refused(line)
    _ok(
        runner,
        ["git", "commit", "--allow-empty", "-S", "-q", "-m", TRIGGER_SUBJECT],
    )
    sha = _ok(runner, ["git", "rev-parse", "HEAD"]).strip()
    try:
        _ok(runner, ["git", "push", "-q", "origin", branch])
    except Refused:
        # Drop the unpushed trigger commit so a re-run never stacks a second.
        runner(["git", "reset", "-q", "--soft", "HEAD~1"])
        raise
    try:
        cancelled = cancel_stale(runner, repo, pr, expect=sha)
    except Refused as exc:
        # The run started; a stale run left going only holds a runner.
        cancelled = f"cancel-stale failed: {exc}"
    return f"triggered {sha[:12]} on origin/{branch} | {cancelled}"


def _probe() -> tuple[int, str]:
    from signing_probe import probe

    return probe()


def main(argv: list[str] | None = None, runner: Runner = run, probe=_probe) -> int:
    parser = argparse.ArgumentParser(
        prog="ci_runs.py", description=__doc__.splitlines()[0]
    )
    sub = parser.add_subparsers(dest="cmd", required=True)
    for name in ("trigger", "cancel-stale"):
        p = sub.add_parser(name)
        p.add_argument("--pr", type=int, required=True, help="the PR number")
        p.add_argument("--repo", default=DEFAULT_REPO)
    args = parser.parse_args(argv)
    try:
        if args.cmd == "trigger":
            code, line = 0, trigger(runner, args.repo, args.pr, probe)
        else:
            code, line = 0, cancel_stale(runner, args.repo, args.pr)
    except Refused as exc:
        code, line = 1, f"refused | {exc}"
    print(f"{PREFIX} {line}", file=sys.stdout if code == 0 else sys.stderr)
    return code


if __name__ == "__main__":
    raise SystemExit(main())
