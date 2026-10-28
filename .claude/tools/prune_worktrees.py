#!/usr/bin/env python3
"""Worktree-prune helper — the deterministic git side of ``upkeep.py``'s prune
step: given the set of branches confirmed finished (the caller reads that from
the board and ``gh pr list``), remove each one's worktree
and local branch, skip anything dirty or locked, tidy stale admin entries, and
return a tally. The caller just reports the counts (per ``CLAUDE.md`` → "Skill
tooling" / "Context economy"), instead of driving the
``git worktree remove`` / ``git branch -D`` / ``git worktree prune`` trio by
hand per merged branch.

Usage (run from the base repo root — you can't remove the worktree you stand in):

    python3 .claude/tools/prune_worktrees.py --merged eng-701 eng-702
    python3 .claude/tools/prune_worktrees.py --merged-file /tmp/merged.txt --dry-run
    python3 .claude/tools/prune_worktrees.py --closed eng-704 --dry-run

Prints JSON ``{removed: [{path, branch}], branches_removed: [branch],
skipped: [{path, branch, reason}], left: [{path, branch}], unmatched: [branch],
ahead: {branch: {ahead, tip}}, pruned: bool, dry_run: bool}``:

* ``removed`` — merged worktrees whose worktree + branch were dropped;
* ``branches_removed`` — merged local branches with no worktree, deleted under
  the same unpushed-commit gate (a skip here carries ``path: None``);
* ``unmatched`` — merged names matching no worktree and no local branch;
* ``skipped`` — merged worktrees held back as unsafe, left in place: either the
  pre-flight found uncommitted changes / unpushed commits / no upstream (see
  ``unsafe_reason``), or ``git worktree remove`` itself **refused** (a dirty or
  locked tree). Both are the safe outcome and both name their reason;
* ``left`` — non-merged worktrees (PR still open / closed-without-merge / no
  PR), untouched;
* ``ahead`` — per matched name, its commits beyond ``origin/main`` and its
  tip, in the dry run and the real one alike;
* ``pruned`` — whether ``git worktree prune`` ran (skipped in ``--dry-run``).

A ``--closed`` name (its issue completed or canceled) skips the safety
pre-flight: only a live session's lock holds it (:func:`prune`).

**The gate is per tier**, read from the launch's substrate marker
(:func:`tier_of`), never from the branch name. An executor worktree (``task``,
or any tree whose tier cannot be read) keeps the full refusal below. An
advisor worktree (``architect``, ``explore``) is read-only toward the repo, so
its untracked spec file is expendable once the caller has confirmed its issue
finished: it is removed with one ``--force``, which still refuses a **locked**
tree. A live session's lock holds it; a Claude Code lock whose recorded pid is
gone is stale (:func:`stale_lock`) and lifted first, since the CLI can leave
one behind at exit. Only a commit of its own beyond ``origin/main`` holds it
back otherwise — a read-only session never makes one, so one is worth a look.

The base worktree (``refs/heads/main``) is never a candidate. Stdlib only; a
Python skill-tool under ``.claude/tools/`` — deliberately **not** a Cargo
workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

MAIN_BRANCH_REF = "refs/heads/main"
# Where the shell's `_ds_substrate_write` records each launch, relative to the
# base checkout: `<key>` holds the substrate, `<key>.tier` the advisor or
# helper tier (absent for a `task`).
SUBSTRATE_DIR = Path(".claude") / "session-substrate"
ADVISOR = "advisor"
# A worktree directory named for an issue: the bare number, or a pre-rename
# `eng-` / `ceo-` / `exp-` spelling — the forms `_ds_worktree_of` resolves.
ISSUE_DIR_RE = re.compile(r"(?:eng-|ceo-|exp-)?(\d+)", re.IGNORECASE | re.ASCII)
# The reason Claude Code writes when it locks a session's worktree.
LOCK_RE = re.compile(r"^claude session \S+ \(pid (\d+) start (.+)\)$")


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
        elif (line == "locked" or line.startswith("locked ")) and current is not None:
            current["locked"] = line[len("locked") :].strip()
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


def unsafe_reason(path: str, branch: str, git=_real_git) -> str | None:
    """Why this worktree must not be removed, or ``None`` if it is safe to.

    ``git worktree remove`` without ``--force`` already refuses a **dirty** tree,
    which is why the tool got by without this check. What it does not refuse is a
    branch carrying commits that were never pushed — and ``git branch -D`` below
    then force-deletes them, because a squash-merged tip is not an ancestor of
    main so ``-d`` would refuse. The result is that committed-but-unpushed work
    can be destroyed with no diagnostic at all.

    That is not hypothetical: two worktrees were removed while their PRs were
    still open, and the second lost a verified, tested review fix that had to be
    reauthored from the session transcript. It survived the first time only
    because every commit happened to have been pushed. This function is the loud
    refusal that makes the safe case explicit rather than incidental.

    A branch with no upstream is treated as unsafe: pushed state cannot be
    proven, and guessing in the destructive direction is the whole failure mode.

    **This gate is not the whole fix, and what remains is not in this repo.**
    Reading the predicate above shows this tool never removed either lost
    worktree: both had *open* PRs, so they were never in ``merged`` to begin
    with. Whatever deleted them ran outside it — the harness's own exit-time
    cleanup is the remaining suspect, and one lead is that an interrupted
    session releases its worktree lock, after which an unlocked worktree with
    an open PR may look prunable to something that sweeps them. That is a
    harness question, not a repo one; this gate closes the hazard for the
    sweeper that *is* committed here, and does so for the case the sweeper
    could genuinely have caused (a merged branch carrying unpushed commits,
    which ``git branch -D`` would have destroyed).

    So the interim mitigation stays **load-bearing** until the harness side is
    settled: the checkpoint-commit convention, which pushes at every
    checkpoint, is the only reason the first instance lost nothing. Anything
    committed but unpushed — or uncommitted because signing failed — is still
    exposed. ``commit-changes`` states that alarm at the point a signing
    failure happens.
    """
    rc, out, _ = git(["-C", path, "status", "--porcelain"])
    if rc != 0:
        return "could not read the worktree's status, so safety is unprovable"
    if out.strip():
        return f"{len(out.strip().splitlines())} uncommitted change(s)"
    return unpushed_reason(branch, git, ["-C", path])


def unpushed_reason(branch: str, git=_real_git, prefix: list[str] = ()) -> str | None:
    """The unpushed-commit half of :func:`unsafe_reason`, alone — the whole gate
    for a local branch with no worktree, which has no status to read."""
    rc, out, _ = git(
        [*prefix, "rev-list", "--count", f"{branch}@{{upstream}}..{branch}"]
    )
    if rc != 0:
        # No upstream. A read-only architect or explore worktree never pushes,
        # so it never has one — and it is meant to be reclaimed once its issue
        # is Done. It is still safe when every commit on it is already on
        # `origin/main`: there is then nothing unpushed to lose. Anything else
        # stays refused, since pushed state cannot be proven. Gitignored files
        # are NOT protected — the dirty check cannot see them and
        # `git worktree remove` discards them — which the convention accepts:
        # these sessions keep durable state in Linear, not in the tree.
        rc, out, _ = git([*prefix, "rev-list", "--count", f"origin/main..{branch}"])
        if rc == 0 and out.strip() == "0":
            return None
        return "no upstream branch, so unpushed commits cannot be ruled out"
    count = out.strip()
    # Fail closed on an unreadable count. Every other branch here refuses when it
    # cannot prove safety, and this was the one place that guessed toward
    # REMOVING: an rc-0 with empty stdout fell through to "safe to delete".
    # `git rev-list --count` always prints a number on success, so this is a
    # belt-and-braces guard — but the whole point of the function is that the
    # guess must never run in the destructive direction.
    if not count.isdigit():
        return f"unreadable unpushed-commit count ({count!r}), so safety is unprovable"
    if count != "0":
        return f"{count} unpushed commit(s)"
    return None


def tier_of(base: str, tree_path: str) -> str | None:
    """The tier a worktree's session launched on, from its substrate marker:
    the ``.tier`` file's ``advisor`` / ``helper``, else ``executor`` when only
    the marker exists, else ``None``.

    The shell reads an ABSENT marker as advisor (the issue-keyed ``explore``
    once wrote none), but this is the destructive side, so absent stays
    unknown and gets the executor gate. Keys mirror ``_ds_marker_key``: the
    bare number first, then a pre-rename ``eng-<n>``.

    An advisor reading counts only for a tree named by the bare number. The
    marker keys on the NUMBER, so a pre-rename ``eng-<n>`` task tree that an
    ``explore <n>`` later reused would otherwise inherit that launch's advisor
    tier and lose its executor gate."""
    name = Path(tree_path).name
    m = ISSUE_DIR_RE.fullmatch(name)
    keys = [str(int(m.group(1))), f"eng-{int(m.group(1))}"] if m else [name]
    markers = Path(base) / SUBSTRATE_DIR
    for key in keys:
        if not (markers / key).is_file():
            continue
        try:
            tier = (markers / f"{key}.tier").read_text(encoding="utf-8").strip()
        except OSError:
            tier = ""
        if tier == ADVISOR and name != keys[0]:
            return None
        return tier if tier in (ADVISOR, "helper") else "executor"
    return None


def advisor_unsafe_reason(branch: str, git=_real_git) -> str | None:
    """The whole gate for an advisor worktree: refuse only a commit of its own
    beyond ``origin/main``. Uncommitted and untracked files are expendable by
    the convention — the session's durable state is in Linear."""
    rc, out, _ = git(["rev-list", "--count", f"origin/main..{branch}"])
    count = out.strip()
    if rc != 0 or not count.isdigit():
        return "could not count commits beyond origin/main, so safety is unprovable"
    if count != "0":
        return f"{count} commit(s) beyond origin/main on a read-only session"
    return None


def _real_alive(pid: int, start: str) -> bool:
    """Whether ``pid`` is running at all. ``start`` is deliberately NOT
    compared: the lock records it in UTC while ``ps`` prints local time, so a
    string compare read every live session as dead (measured). A reused pid
    therefore reads as alive — the tree is skipped, the safe direction.

    Fails CLOSED: only ``ProcessLookupError`` means dead. A ``ps`` probe could
    not tell a dead pid from its own failure (both rc 1, empty output), and a
    permission error means the process exists."""
    del start
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except OSError:
        return True
    return True


def stale_lock(reason: str | None, alive=_real_alive) -> bool:
    """Whether a worktree lock is a Claude Code session lock whose holder is
    gone. The CLI writes ``claude session <tag> (pid N start <time>)`` and,
    measured, can leave it behind when the session exits — which would hold
    every advisor tree forever. Anything else — no lock, a bare ``locked``, a
    hand-written reason, an unparseable one, a live holder — is NOT stale."""
    m = LOCK_RE.search(reason or "")
    if not m:
        return False
    return not alive(int(m.group(1)), m.group(2))


def ahead_of_main(branch: str, git=_real_git) -> dict:
    """``{ahead, tip}`` for one name: its commits beyond ``origin/main`` and the
    tip they hang from, each ``None`` when unreadable. Reported for every
    candidate, so a removal under ``closed`` says what it discarded and the tip
    to restore it from (``git branch <name> <tip>``)."""
    rc, out, _ = git(["rev-list", "--count", f"origin/main..{branch}"])
    ahead = int(out.strip()) if rc == 0 and out.strip().isdigit() else None
    rc, out, _ = git(["rev-parse", "--verify", "--quiet", branch])
    tip = (out.strip() or None) if rc == 0 else None
    return {"ahead": ahead, "tip": tip}


def prune(
    merged: set[str],
    dry_run: bool,
    git=_real_git,
    tier=None,
    alive=_real_alive,
    closed: set[str] = frozenset(),
) -> dict:
    """Remove each merged branch's worktree + local branch, skipping unsafe ones.

    Two gates, and both matter: ``merged`` restricts the candidates to branches
    the caller has confirmed finished — for ``upkeep``, a completed or canceled
    issue with no open PR — and
    :func:`unsafe_reason` then refuses any of
    those still holding uncommitted or unpushed work — or, for an advisor
    worktree, :func:`advisor_unsafe_reason`. ``git`` is an injectable
    ``(args) -> (rc, stdout, stderr)`` runner; ``tier`` an injectable
    ``(path) -> tier | None``, defaulting to :func:`tier_of` on the base.

    ``closed`` names are candidates too, and for them the second gate is
    lifted: their issue is completed or canceled, so its work is settled, and
    an unpushed commit, a missing upstream or a dirty tree no longer holds the
    name forever. Only a live session's lock does — a stale Claude Code lock is
    lifted as for an advisor, and the tree goes with one ``--force``."""
    rc, out, err = git(["worktree", "list", "--porcelain"])
    if rc != 0:
        raise RuntimeError(f"git worktree list failed: {err.strip()}")
    trees = parse_worktrees(out)
    if tier is None:
        base = next((t["path"] for t in trees if is_base(t)), None)
        tier = (lambda p: tier_of(base, p)) if base else (lambda p: None)
    candidates = set(merged) | set(closed)

    removed: list[dict] = []
    skipped: list[dict] = []
    left: list[dict] = []
    ahead: dict[str, dict] = {}

    for tree in trees:
        if is_base(tree) or tree.get("branch") is None:
            continue
        path, branch = tree["path"], tree["branch"]
        if branch not in candidates:
            left.append({"path": path, "branch": branch})
            continue
        ahead[branch] = ahead_of_main(branch, git)
        settled = branch in closed
        advisor = tier(path) == ADVISOR
        # Checked before the dry-run branch, so a dry run reports what it would
        # actually do rather than promising a removal the real run would refuse.
        if settled:
            reason = None
        elif advisor:
            reason = advisor_unsafe_reason(branch, git)
        else:
            reason = unsafe_reason(path, branch, git)
        lock = tree.get("locked")
        # A live session's lock holds every tier. Only an advisor's or a closed
        # name's STALE Claude Code lock — its holder gone — is lifted, since the
        # CLI can leave one behind at exit; an executor keeps git's plain
        # refusal.
        forced = advisor or settled
        unlock = forced and lock is not None and stale_lock(lock, alive)
        if reason is None and lock is not None and forced and not unlock:
            reason = f"locked by a live session ({lock or 'no reason'})"
        if reason is not None:
            skipped.append({"path": path, "branch": branch, "reason": reason})
            continue
        if dry_run:
            removed.append({"path": path, "branch": branch})
            continue
        if unlock:
            # Re-read the lock first: a session relaunched on the tree since
            # the listing holds a fresh lock, which must not be lifted.
            _, now, _ = git(["worktree", "list", "--porcelain"])
            current = next((t for t in parse_worktrees(now) if t["path"] == path), {})
            if current.get("locked") != lock:
                skipped.append(
                    {"path": path, "branch": branch, "reason": "lock changed"}
                )
                continue
            git(["worktree", "unlock", path])
        # An executor gets no --force: a dirty or locked worktree refuses, which
        # is the safe outcome — record it as skipped and move on. An advisor or
        # a closed name gets ONE --force, which discards its expendable files
        # but still refuses a locked tree (that takes two).
        force = ["--force"] if forced else []
        r_rc, _, r_err = git(["worktree", "remove", *force, path])
        if r_rc != 0:
            if unlock:
                # Put the stale lock back as found: the removal did not happen.
                git(["worktree", "lock", "--reason", lock, path])
            skipped.append(
                {"path": path, "branch": branch, "reason": r_err.strip() or "refused"}
            )
            continue
        # Squash/rebase-merged tips aren't ancestors of main, so -d would refuse;
        # the branch is confirmed finished and its commits pushed, so force it.
        git(["branch", "-D", branch])
        removed.append({"path": path, "branch": branch})

    # A merged branch whose worktree is already gone used to survive every pass:
    # only a worktree removal ever deleted a branch. Same gate, minus the status
    # read there is no tree to make.
    in_trees = {t["branch"] for t in trees if t.get("branch")}
    refs_rc, refs, _ = git(["for-each-ref", "--format=%(refname:short)", "refs/heads"])
    local = set(refs.split()) if refs_rc == 0 else set()
    branches_removed: list[str] = []
    for branch in sorted((candidates & local) - in_trees - {"main"}):
        ahead[branch] = ahead_of_main(branch, git)
        reason = None if branch in closed else unpushed_reason(branch, git)
        if reason is not None:
            skipped.append({"path": None, "branch": branch, "reason": reason})
            continue
        if not dry_run:
            d_rc, _, d_err = git(["branch", "-D", branch])
            if d_rc != 0:
                skipped.append(
                    {
                        "path": None,
                        "branch": branch,
                        "reason": (d_err.strip().splitlines() or ["refused"])[0],
                    }
                )
                continue
        branches_removed.append(branch)

    pruned = False
    if not dry_run and removed:
        git(["worktree", "prune"])
        pruned = True

    return {
        "removed": removed,
        "branches_removed": branches_removed,
        "skipped": skipped,
        "left": left,
        # Reported, not dropped: a merged name matching no worktree and no local
        # branch is usually long-finished work, but silence hid the case where
        # the name itself was wrong.
        # Empty when the branch list itself failed: then every name would read
        # as unmatched, which says nothing about the names.
        "unmatched": sorted(candidates - in_trees - local) if refs_rc == 0 else [],
        "ahead": ahead,
        "pruned": pruned,
        "dry_run": dry_run,
    }


def _read_names(names: list[str] | None, path: str | None) -> set[str]:
    out: list[str] = list(names or [])
    if path:
        out.extend(Path(path).read_text(encoding="utf-8").split())
    return {normalize_branch(m) for m in out if m.strip()}


def _read_merged(args) -> set[str]:
    return _read_names(args.merged, args.merged_file)


# What a removal under --closed leaves to recover from. `git branch -D` drops
# the branch's own reflog and `git worktree remove` drops the tree's HEAD log,
# so neither reflog keeps it: the reported tip is the handle, and the commits
# stay in the object store only until `git gc` prunes them — an expiry measured
# from the OBJECT's age, not the deletion, so an old stray's commits can go at
# the very next gc. A dirty tree's uncommitted files have no handle at all.
CLOSED_HELP = (
    "branch names whose issue is completed or canceled: the uncommitted, "
    "unpushed and no-upstream holds are lifted, and only a live session lock "
    "holds. A removed tree's uncommitted and untracked files are discarded "
    "for good. Its commits are not kept by any reflog (the branch's is "
    "deleted with it); restore one promptly with `git branch <name> <tip>` "
    "from the report's `ahead` entry, since `git gc` prunes unreachable "
    "objects older than gc.pruneExpire (two weeks by default), counted from "
    "the commit's own age, so an old branch's can go at the next gc"
)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="prune_worktrees.py")
    parser.add_argument(
        "--merged", nargs="*", default=[], help="branch names whose PR has merged"
    )
    parser.add_argument(
        "--merged-file", default=None, help="file of whitespace-separated branches"
    )
    parser.add_argument("--closed", nargs="*", default=[], help=CLOSED_HELP)
    parser.add_argument(
        "--closed-file",
        default=None,
        help="file of whitespace-separated --closed names",
    )
    parser.add_argument(
        "--dry-run", action="store_true", help="report what would be removed only"
    )
    args = parser.parse_args(argv[1:])

    merged = _read_merged(args)
    closed = _read_names(args.closed, args.closed_file)
    result = prune(merged, args.dry_run, closed=closed)
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
