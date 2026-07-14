#!/usr/bin/env python3
"""Worktree-prune helper — the deterministic git side of ``housekeeping`` step
2: given the set of branches whose PR has **merged** (the skill reads that from
``gh pr list`` — the one network call), remove each merged branch's worktree
and local branch, skip anything dirty or locked, tidy stale admin entries, and
return a tally. The skill just reports the counts (per ``CLAUDE.md`` → "Skill
tooling" / "Context economy"), instead of driving the
``git worktree remove`` / ``git branch -D`` / ``git worktree prune`` trio by
hand per merged branch.

Usage (run from the base repo root — you can't remove the worktree you stand in):

    python3 .claude/tools/prune_worktrees.py --merged eng-701 eng-702
    python3 .claude/tools/prune_worktrees.py --merged-file /tmp/merged.txt --dry-run

Prints JSON ``{removed: [{path, branch}], skipped: [{path, branch, reason}],
left: [{path, branch}], pruned: bool, dry_run: bool}``:

* ``removed`` — merged worktrees whose worktree + branch were dropped;
* ``skipped`` — merged worktrees whose ``git worktree remove`` **refused** (a
  dirty or locked tree — the safe outcome), left in place;
* ``left`` — non-merged worktrees (PR still open / closed-without-merge / no
  PR), untouched;
* ``pruned`` — whether ``git worktree prune`` ran (skipped in ``--dry-run``).

The base worktree (``refs/heads/main``) is never a candidate. Stdlib only; a
Python skill-tool under ``.claude/tools/`` — deliberately **not** a Cargo
workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

MAIN_BRANCH_REF = "refs/heads/main"


def parse_worktrees(porcelain: str) -> list[dict]:
    """Parse ``git worktree list --porcelain`` into ``[{path, branch}]``. A
    detached or bare worktree gets ``branch: None``. The base (main) worktree is
    included; callers filter it out via ``is_base``."""
    trees: list[dict] = []
    current: dict | None = None
    for line in porcelain.splitlines():
        if line.startswith("worktree "):
            if current is not None:
                trees.append(current)
            current = {"path": line[len("worktree ") :].strip(), "branch": None}
        elif line.startswith("branch ") and current is not None:
            current["ref"] = line[len("branch ") :].strip()
            current["branch"] = current["ref"].removeprefix("refs/heads/")
    if current is not None:
        trees.append(current)
    return trees


def is_base(tree: dict) -> bool:
    return tree.get("ref") == MAIN_BRANCH_REF


def normalize_branch(name: str) -> str:
    """A branch/tag as a bare comparison key: strip a ``refs/heads/`` prefix and
    surrounding whitespace, so ``refs/heads/eng-1`` and ``eng-1`` match."""
    return name.strip().removeprefix("refs/heads/")


def _real_git(args: list[str]) -> tuple[int, str, str]:
    proc = subprocess.run(["git", *args], capture_output=True, text=True, check=False)
    return proc.returncode, proc.stdout, proc.stderr


def prune(merged: set[str], dry_run: bool, git=_real_git) -> dict:
    """Remove each merged branch's worktree + local branch, skipping dirty ones.
    ``git`` is an injectable ``(args) -> (rc, stdout, stderr)`` runner."""
    rc, out, err = git(["worktree", "list", "--porcelain"])
    if rc != 0:
        raise RuntimeError(f"git worktree list failed: {err.strip()}")

    removed: list[dict] = []
    skipped: list[dict] = []
    left: list[dict] = []

    for tree in parse_worktrees(out):
        if is_base(tree) or tree.get("branch") is None:
            continue
        path, branch = tree["path"], tree["branch"]
        if branch not in merged:
            left.append({"path": path, "branch": branch})
            continue
        if dry_run:
            removed.append({"path": path, "branch": branch})
            continue
        # No --force: a dirty or locked worktree refuses, which is the safe
        # outcome — record it as skipped and move on.
        r_rc, _, r_err = git(["worktree", "remove", path])
        if r_rc != 0:
            skipped.append(
                {"path": path, "branch": branch, "reason": r_err.strip() or "refused"}
            )
            continue
        # Squash/rebase-merged tips aren't ancestors of main, so -d would refuse;
        # the PR is confirmed merged, so force the branch delete.
        git(["branch", "-D", branch])
        removed.append({"path": path, "branch": branch})

    pruned = False
    if not dry_run and removed:
        git(["worktree", "prune"])
        pruned = True

    return {
        "removed": removed,
        "skipped": skipped,
        "left": left,
        "pruned": pruned,
        "dry_run": dry_run,
    }


def _read_merged(args) -> set[str]:
    merged: list[str] = list(args.merged or [])
    if args.merged_file:
        text = Path(args.merged_file).read_text(encoding="utf-8")
        merged.extend(text.split())
    return {normalize_branch(m) for m in merged if m.strip()}


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="prune_worktrees.py")
    parser.add_argument(
        "--merged", nargs="*", default=[], help="branch names whose PR has merged"
    )
    parser.add_argument(
        "--merged-file", default=None, help="file of whitespace-separated branches"
    )
    parser.add_argument(
        "--dry-run", action="store_true", help="report what would be removed only"
    )
    args = parser.parse_args(argv[1:])

    merged = _read_merged(args)
    result = prune(merged, args.dry_run)
    json.dump(result, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except (RuntimeError, OSError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
