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

``trigger`` checks the PR's head branch is the checked-out one, probes the
signer (``signing_probe.py``) and refuses with its ``LOCKED`` line when it
does not answer, so the merge gate can hold the run until the operator
unlocks rather than verify a head the re-sign will rewrite. Then it makes
one empty signed commit, :data:`TRIGGER_SUBJECT`, pushes it with a lease
pinned to the remote tip read before the commit, and runs ``cancel-stale``.

``cancel-stale`` lists the branch's ``pull_request`` workflow runs, then reads
the PR's head, and cancels every run not yet completed whose head SHA differs
from it. The head is read **after** the runs, so a listed run can never sit
on a head newer than the one it is compared with: a run on the current head
is never cancelled (a cancelled run reads as a failure, and costs a
re-trigger). ``resign_branch.py resign`` calls it after its force-push.

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
from typing import Callable

PREFIX = "ci-runs |"
DEFAULT_REPO = "DASMAC-com/dropset"

#: The trigger commit's subject. It carries no ``skip-checks`` trailer.
TRIGGER_SUBJECT = "ci: Run the remote checks"

#: A run in any other status is finished and has nothing to cancel.
DONE = "completed"

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


def stale(runs: list[dict], head: str) -> list[dict]:
    """The runs to cancel: every one whose commit is not ``head``."""
    return [r for r in runs if r.get("head_sha") != head]


def cancel_stale(runner: Runner, repo: str, pr: int) -> str:
    branch, _ = read_pr(runner, repo, pr)
    runs = active_runs(runner, repo, branch)
    # Read after the runs, never before: see the module docstring.
    _, head = read_pr(runner, repo, pr)
    doomed = stale(runs, head)
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
    code, line = probe()
    if code != 0:
        raise Refused(line)
    _ok(
        runner,
        ["git", "commit", "--allow-empty", "-S", "-q", "-m", TRIGGER_SUBJECT],
    )
    sha = _ok(runner, ["git", "rev-parse", "HEAD"]).strip()
    _ok(
        runner,
        [
            "git",
            "push",
            "-q",
            f"--force-with-lease={branch}:{remote}",
            "origin",
            branch,
        ],
    )
    return f"triggered {sha[:12]} on origin/{branch} | {cancel_stale(runner, repo, pr)}"


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
