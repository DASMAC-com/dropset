#!/usr/bin/env python3
# cspell:word ppid
"""Atomic session close — write the issue's closing state, then remove the
worktree, as one act a session runs as its **last** tool call on the operator's
explicit ratification in its own tab.

The ordering is the whole point (operator ruling, 2026-10-12): the state write
lands **first** and the worktree goes **second**, never the reverse. A worktree
removed before its issue leaves In Progress / In Review strands the session — a
later resume lands in a different checkout, or cannot resume at all — and the
issue still reads as live work. Before this tool every close was a two-step the
operator finished by hand, in whichever order they happened to reach for.

Usage (run from inside the worktree being closed):

    python3 .claude/tools/close_session.py --state Done
    python3 .claude/tools/close_session.py --state 'In Review' --issue ENG-1571
    python3 .claude/tools/close_session.py --state Done --keep-worktree
    python3 .claude/tools/close_session.py --state Done --dry-run

``--state`` is required and never guessed: an implementation session closes to
Done, an ``architect`` / ``explore`` session hands off at In Review. The issue
is read from an ``eng-###`` branch; any other branch (``worktree-ceo-…``) must
pass ``--issue``. ``--keep-worktree`` is the report-only ending — the same gates
and state write, with the removal left to the operator.

Every gate runs **before** any write, and each fails closed:

* the tree is clean and every commit is pushed — the pruner's own
  :func:`prune_worktrees.unsafe_reason`, except that a branch with no upstream
  passes when it carries nothing beyond ``origin/main`` (an architect worktree
  never pushes, and has nothing to lose);
* no PR on the branch is open (``gh pr list``; an unreadable list refuses);
* the worktree's lock, when removing, is **this** session's: the harness locks
  a live session's worktree as ``claude session <name> (pid N …)``, and the
  pid must be an ancestor of this process. Another live session's lock is never
  lifted.

Then the state is written through ``board_batch`` (zero-echo GraphQL); only on
a confirmed write is the worktree unlocked, removed **without** ``--force``, and
its branch deleted. A removal that fails after the write re-locks the tree and
exits 2: the issue is already resolved, so ``upkeep``'s status-gated sweep is
the backstop.

Prints one JSON object ``{issue, state, worktree, branch, pr, dry_run,
keep_worktree, refused, state_written, removed, notes}``. Exit 0 on a complete
close, 1 on a refusal or error with **nothing** changed, 2 when the state landed
but the removal did not.

The harness's ``ExitWorktree`` is deliberately not the mechanism: it documents
itself as operating only on worktrees made by ``EnterWorktree`` in the same
session, and whether a ``claude --worktree`` launch counts is unverified — the
only non-destructive probe would itself move the session out of its checkout.

Stdlib only; a Python skill-tool under ``.claude/tools/`` — deliberately
**not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Callable

import board_batch
import prune_worktrees

REPO = "DASMAC-com/dropset"
TIMEOUT = 60
ISSUE_BRANCH_RE = re.compile(r"^(?:worktree-)?eng-(\d+)$", re.IGNORECASE)
ISSUE_RE = re.compile(r"^ENG-\d+$")
LOCK_PID_RE = re.compile(r"\(pid (\d+)\b")
NO_UPSTREAM_PREFIX = "no upstream"
MAX_ANCESTRY = 64

Runner = Callable[[list[str], "str | None"], "tuple[int, str, str]"]


class Refusal(Exception):
    """A gate failed; nothing has been written or removed."""


def real_run(cmd: list[str], cwd: str | None = None) -> tuple[int, str, str]:
    """``(rc, stdout, stderr)``; a timeout or a missing binary is an rc."""
    try:
        proc = subprocess.run(
            cmd, capture_output=True, text=True, timeout=TIMEOUT, cwd=cwd, check=False
        )
    except subprocess.TimeoutExpired:
        return 124, "", f"timed out after {TIMEOUT}s"
    except OSError as e:
        return 127, "", str(e)
    return proc.returncode, proc.stdout, proc.stderr


def first_line(text: str) -> str:
    return (text.strip().splitlines() or [""])[0][:200]


# --- pure helpers ----------------------------------------------------------


def issue_for(branch: str, explicit: str | None) -> str:
    """The ``ENG-###`` to close: ``--issue`` wins, else the branch name."""
    if explicit:
        ident = explicit.strip().upper()
        if not ISSUE_RE.match(ident):
            raise Refusal(f"--issue must look like ENG-###, got {explicit!r}")
        return ident
    m = ISSUE_BRANCH_RE.match(branch)
    if not m:
        raise Refusal(f"branch {branch!r} names no eng-### issue — pass --issue")
    return f"ENG-{int(m.group(1))}"


def lock_reasons(porcelain: str) -> dict[str, str | None]:
    """``{worktree path: lock reason}`` from ``git worktree list --porcelain``;
    an unlocked tree maps to ``None``, a reasonless lock to ``""``."""
    out: dict[str, str | None] = {}
    current: str | None = None
    for line in porcelain.splitlines():
        if line.startswith("worktree "):
            current = line[len("worktree ") :].strip()
            out[current] = None
        elif current is not None and (line == "locked" or line.startswith("locked ")):
            out[current] = line[len("locked") :].strip()
    return out


def lock_verdict(reason: str | None, ancestors: set[int]) -> bool:
    """Whether removal must unlock first. Raises :class:`Refusal` on a lock this
    process cannot prove is its own session's."""
    if reason is None:
        return False
    m = LOCK_PID_RE.search(reason)
    if not m:
        raise Refusal(f"worktree is locked with no session pid ({reason!r})")
    if int(m.group(1)) not in ancestors:
        raise Refusal(
            f"worktree is locked by another live process (pid {m.group(1)}), "
            "not this session"
        )
    return True


def pr_verdict(prs: list[dict]) -> dict | None:
    """The PR to report (an open one beats the rest); raises on an open one."""
    open_prs = [p for p in prs if p.get("state") == "OPEN"]
    if open_prs:
        raise Refusal(f"PR #{open_prs[0].get('number')} is still open")
    return prs[0] if prs else None


# --- probes ----------------------------------------------------------------


def ancestor_pids(run: Runner, start: int) -> set[int]:
    """This process and every parent up to init, read through ``ps``."""
    seen: set[int] = set()
    pid = start
    while pid > 1 and pid not in seen and len(seen) < MAX_ANCESTRY:
        seen.add(pid)
        rc, out, _ = run(["ps", "-o", "ppid=", "-p", str(pid)], None)
        text = out.strip()
        if rc != 0 or not text.isdigit():
            break
        pid = int(text)
    return seen


def git_safety(run: Runner, path: str, branch: str) -> None:
    """The pruner's uncommitted/unpushed gate, plus the no-upstream carve-out."""

    def git(args: list[str]) -> tuple[int, str, str]:
        return run(["git", *args], None)

    reason = prune_worktrees.unsafe_reason(path, branch, git)
    if reason is not None and reason.startswith(NO_UPSTREAM_PREFIX):
        # Never pushed is safe only when there is nothing to push.
        rc, out, _ = git(["-C", path, "rev-list", "--count", "origin/main..HEAD"])
        count = out.strip()
        if rc == 0 and count == "0":
            return
        reason = (
            f"no upstream and {count} commit(s) beyond origin/main"
            if rc == 0 and count.isdigit()
            else reason
        )
    if reason is not None:
        raise Refusal(reason)


def read_prs(run: Runner, branch: str) -> list[dict]:
    rc, out, err = run(
        [
            "gh", "pr", "list", "--repo", REPO, "--head", branch,
            "--state", "all", "--json", "number,state",
        ],
        None,
    )  # fmt: skip
    if rc != 0:
        raise Refusal(f"could not read the branch's PRs: {first_line(err)}")
    try:
        prs = json.loads(out or "[]")
    except json.JSONDecodeError as e:
        raise Refusal(f"unreadable gh pr list output: {e}") from e
    if not isinstance(prs, list):
        raise Refusal("unreadable gh pr list output")
    return prs


def locate(run: Runner) -> tuple[str, str, str, str]:
    """``(worktree, branch, base, worktree-list porcelain)`` for the cwd."""
    rc, top, err = run(["git", "rev-parse", "--show-toplevel"], None)
    if rc != 0:
        raise Refusal(f"not inside a git worktree: {first_line(err)}")
    rc, porcelain, err = run(["git", "worktree", "list", "--porcelain"], None)
    if rc != 0:
        raise Refusal(f"git worktree list failed: {first_line(err)}")
    trees = prune_worktrees.parse_worktrees(porcelain)
    bases = [t for t in trees if prune_worktrees.is_base(t)]
    if not bases:
        raise Refusal("main is not checked out in any worktree")
    worktree = str(Path(top.strip()).resolve())
    base = str(Path(bases[0]["path"]).resolve())
    if worktree == base:
        raise Refusal("run from the session's worktree, not the base checkout")
    mine = [t for t in trees if str(Path(t["path"]).resolve()) == worktree]
    if not mine or not mine[0].get("branch"):
        raise Refusal("this worktree has no branch checked out")
    return worktree, mine[0]["branch"], base, porcelain


# --- the act ---------------------------------------------------------------


def write_state(issue: str, state: str, dry_run: bool) -> None:
    """One state write through ``board_batch``; a dry run resolves the state
    name without writing. Raises on any failure."""
    api_key = board_batch.env_var("LINEAR_API_KEY")
    project = board_batch.env_var("LINEAR_PROJECT_ID")
    updates = {issue: {"state": state}}
    by_number = board_batch.index_by_number(
        board_batch.fetch_issues_by_number(
            api_key, project, board_batch.referenced_numbers(updates)
        )
    )
    board_batch.apply_fields(
        api_key, updates, by_number, dry_run=dry_run, emit=lambda _line: None
    )


def remove(run: Runner, base: str, worktree: str, branch: str, lock: str | None):
    """Unlock (when ours), remove without ``--force``, delete the branch.
    Returns ``None`` on success, else the reason the tree is still there."""

    def git(*args: str) -> tuple[int, str, str]:
        return run(["git", "-C", base, *args], None)

    if lock is not None:
        rc, _, err = git("worktree", "unlock", worktree)
        if rc != 0:
            return f"unlock failed: {first_line(err)}"
    rc, _, err = git("worktree", "remove", worktree)
    if rc != 0:
        if lock is not None:
            # Restore the session's protection; the tree is still live.
            git("worktree", "lock", "--reason", lock, worktree)
        return f"worktree remove refused: {first_line(err)}"
    # A squash-merged tip is not an ancestor of main, so -d would refuse; the
    # gates above proved every commit pushed.
    git("branch", "-D", branch)
    git("worktree", "prune")
    return None


def close(
    args: argparse.Namespace,
    run: Runner = real_run,
    writer: Callable[[str, str, bool], None] = write_state,
    pid: int | None = None,
) -> tuple[int, dict]:
    result: dict = {
        "issue": None,
        "state": args.state,
        "worktree": None,
        "branch": None,
        "pr": None,
        "dry_run": args.dry_run,
        "keep_worktree": args.keep_worktree,
        "refused": None,
        "state_written": False,
        "removed": False,
        "notes": [],
    }
    try:
        worktree, branch, base, porcelain = locate(run)
        result.update(worktree=worktree, branch=branch)
        result["issue"] = issue_for(branch, args.issue)
        git_safety(run, worktree, branch)
        result["pr"] = pr_verdict(read_prs(run, branch))
        lock = None
        if not args.keep_worktree:
            reason = lock_reasons(porcelain).get(worktree)
            ancestors = ancestor_pids(run, os.getpid() if pid is None else pid)
            if lock_verdict(reason, ancestors):
                lock = reason
    except Refusal as e:
        result["refused"] = str(e)
        return 1, result

    try:
        writer(result["issue"], args.state, args.dry_run)
    except (board_batch.BoardBatchError, OSError, RuntimeError) as e:
        result["refused"] = f"state write failed, nothing removed: {e}"
        return 1, result
    if args.dry_run:
        result["notes"].append("dry run: gates passed, state name resolved")
        return 0, result
    result["state_written"] = True

    if args.keep_worktree:
        result["notes"].append("worktree kept; the operator removes it")
        return 0, result
    why = remove(run, base, worktree, branch, lock)
    if why is not None:
        result["notes"].append(f"{why}; upkeep's status-gated sweep will collect it")
        return 2, result
    result["removed"] = True
    return 0, result


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(prog="close_session.py")
    parser.add_argument(
        "--state", required=True, help="closing state name, e.g. Done or 'In Review'"
    )
    parser.add_argument("--issue", default=None, help="ENG-### when the branch is not")
    parser.add_argument(
        "--keep-worktree",
        action="store_true",
        help="write the state but leave the worktree for the operator",
    )
    parser.add_argument(
        "--dry-run", action="store_true", help="run every gate, write nothing"
    )
    return parser.parse_args(argv[1:])


def main() -> int:
    rc, result = close(parse_args(sys.argv))
    json.dump(result, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return rc


if __name__ == "__main__":
    sys.exit(main())
