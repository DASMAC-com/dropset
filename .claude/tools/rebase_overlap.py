#!/usr/bin/env python3
"""Triage what a rebase actually pulled in, and whether it can stale anything.

When ``main`` moves under a long-running review, ``review-pr`` rebases and then
has to decide whether its expensive gates — the regeneration checks and the full
local test suite — need re-running against the new base. That decision is pure
path arithmetic, and it was hand-rolled per rebase: fetch the base, ``log`` the
new commits, ``diff --name-only`` in both directions, then intersect the two
file sets by eye. One session ran that identical sequence **three times** as
``main`` moved 15 commits, and re-ran the whole suite (lint + IDL + SDK clients
+ conformance vectors + both Rust targets + SDK node tests) each time — runs 2
and 3 provably redundant, because each rebase delta was TS-only and touched no
input to any generated artifact. The session reasoned that out explicitly and
re-ran anyway.

So this reports the three facts that decision needs:

* **What the base gained** — the commits and the files they touched.
* **What this branch owns** — the files its own commits touch, i.e. since the
  merge base, not since some fixed point.
* **Where those two overlap** — the only files where the base's movement can
  interact with this branch's work, semantically or textually.

Plus the two predicates that say whether a gate can be skipped, delegated to
``review_diff`` so the exclude lists have one owner:

* ``runs_artifact_gates`` — did the base delta touch a **generation input**
  (``programs/**``, ``sdk/idl/**``, ``sdk/codama/**``, ``sdk/math-core/**``,
  ``sdk/interface/**``)? If not, no committed artifact can have gone stale.
* ``runs_rust_suites`` — did it touch anything outside CI's code filter?

Usage::

    # Capture the base the branch is CURRENTLY on, before fetching/rebasing:
    git merge-base HEAD origin/main            # -> <prev>
    git fetch origin main
    git rebase origin/main
    python3 .claude/tools/rebase_overlap.py --from <prev> --to origin/main

**Pass the merge-base, not** ``git rev-parse origin/main``. ``origin/main`` is a
*shared* ref — worktrees have one ``.git``, so a sibling session's fetch can
advance it long before this runs. Reading it "before the fetch" can therefore
capture a tip this branch was never based on, and the comparison then reports a
0-commit delta for a base that demonstrably moved: a false all-clear, in exactly
the place a false all-clear licenses skipping a gate. (This is not hypothetical
— it happened on the tool's first real invocation.) The merge-base is what the
branch actually sits on, whoever fetched what.

``--from`` is required and deliberately so: once the rebase lands, the old
merge-base is no longer derivable from the branch, and guessing it (``ORIG_HEAD``
is the pre-rebase *branch* head, not the old base) would silently compare the
wrong range. ``--to`` defaults to ``origin/main``; ``--branch`` defaults to
``HEAD``.

**And ``--from`` must be an ancestor of ``--to``, which is checked.** The
warning above covers passing a base tip that *moved*; the opposite slip is
passing the **branch tip**, which makes the range a symmetric tree diff instead
of the base's movement. The branch's own new files then land in ``base_files``
and therefore in ``overlap`` — 42 "overlapping" files in one real session,
including ones only that branch had ever created, which momentarily read as the
branch having already merged. Both slips produce a *plausible* report, which is
why neither can be left to the reader to notice: this one is refused outright,
with the correct merge-base named in the error.

Prints JSON on stdout and a one-line human summary on stderr. Read-only: it runs
only ``git merge-base``, ``git log`` and ``git diff --name-only``, and writes
nothing.

Standard library only. A Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling"). Tests live in ``tests/test_rebase_overlap.py``, run via
``make tools-tests``.
"""

from __future__ import annotations

import argparse
import json
import sys

from review_diff import (
    ReviewDiffError,
    _git,
    touches_ci_code,
    touches_generation_input,
)


def changed_files(rev_range: str) -> list[str]:
    """``git diff --name-only <range>``, as a sorted list of repo-relative paths.

    ``--no-renames`` is not optional here. Rename detection is **on by default**
    and reports only the destination path, so a base delta that renamed
    ``sdk/a.rs`` to ``sdk/b.rs`` would yield ``base_files = {sdk/b.rs}`` while a
    branch still editing ``sdk/a.rs`` yields ``{sdk/a.rs}`` — an empty
    intersection, and :func:`summarize` cheerfully printing "no overlap" for the
    single delta shape most likely to have silently dropped the branch's edits.
    Disabling detection surfaces both sides of a rename as changed paths, which
    is what an overlap question actually wants.
    """
    out = _git(["diff", "--no-renames", "--name-only", rev_range])
    return sorted({line.strip() for line in out.splitlines() if line.strip()})


def commit_subjects(rev_range: str) -> list[str]:
    """``git log <range> --oneline`` as ``<sha> <subject>`` lines."""
    out = _git(["log", rev_range, "--oneline"])
    return [line for line in out.splitlines() if line.strip()]


def merge_base(a: str, b: str) -> str:
    """The merge base of two revisions."""
    return _git(["merge-base", a, b]).strip()


def resolve(rev: str) -> str:
    """``rev`` as a full commit sha, or raise if it names no commit."""
    return _git(["rev-parse", "--verify", f"{rev}^{{commit}}"]).strip()


def is_ancestor(maybe_ancestor: str, descendant: str) -> bool:
    """Whether ``maybe_ancestor`` is reachable from ``descendant``.

    Decided by **merge-base identity** rather than
    ``git merge-base --is-ancestor``, which answers through its *exit status*
    (0 yes, 1 no). ``_git`` raises on any non-zero exit, so that form cannot
    distinguish "no" from a bad revision without inspecting the status it has
    already discarded. The identity `merge-base(a, b) == a` holds exactly when
    ``a`` is an ancestor of ``b``, needs only the helper above, and lets a
    genuinely bad revision surface as the error it is.
    """
    return merge_base(maybe_ancestor, descendant) == resolve(maybe_ancestor)


def check_from_is_ancestor(previous_base: str, new_base: str) -> None:
    """Refuse a ``--from`` that is not an ancestor of ``--to``.

    The documented misuse is passing ``git rev-parse origin/main`` — a tip that
    moved. This guards the **opposite** slip, which the docstring above warns
    about in neither direction: passing the *branch tip* instead of the
    merge-base. That yields a symmetric tree diff rather than a
    base-movement diff, so the branch's own new files land in ``base_files``
    **and** therefore in ``overlap``. In one real session that read as 42
    overlapping files including ones only that branch had ever created, which
    momentarily looked like the branch had already merged.

    The failure mode is what makes the guard worth the call: the wrong answer is
    *plausible*. It is a well-formed report with a believable file count, and
    nothing in it says the range was backwards — so it is acted on. Refusing
    outright, and naming the merge-base the caller should have passed, is the
    only outcome that cannot be mistaken for a result.
    """
    if is_ancestor(previous_base, new_base):
        return
    correct = merge_base(previous_base, new_base)
    raise ReviewDiffError(
        f"--from {previous_base} is not an ancestor of --to {new_base}, so the "
        f"range would be a symmetric tree diff rather than the base's movement "
        f"— the branch's own files would appear in `base_files` and `overlap`. "
        f"Their merge-base is {correct}; pass that (capture it with "
        f"`git merge-base HEAD <base>` BEFORE the fetch/rebase)."
    )


def analyze(previous_base: str, new_base: str, branch: str = "HEAD") -> dict:
    """Compare a base's movement against a branch's own changes.

    ``branch_files`` is measured from the **merge base** of ``branch`` and
    ``new_base``, not from ``previous_base``: after a rebase the branch's commits
    sit on top of the new base, so a ``previous_base..branch`` range would fold
    the base's own movement into the branch's file set and report overlap
    everywhere.
    """
    base_files = changed_files(f"{previous_base}..{new_base}")
    commits = commit_subjects(f"{previous_base}..{new_base}")
    branch_files = changed_files(f"{merge_base(branch, new_base)}..{branch}")

    overlap = sorted(set(base_files) & set(branch_files))
    return {
        "previous_base": previous_base,
        "new_base": new_base,
        "base_commits": len(commits),
        "base_commit_subjects": commits,
        "base_files": base_files,
        "branch_files": branch_files,
        "overlap": overlap,
        # Predicates over the BASE DELTA only — the question is whether the
        # base's movement could have staled an artifact this branch already
        # regenerated, not whether this branch touched one. Both are `review_diff`
        # functions, not re-implementations, so the logic has one owner as well
        # as the exclude lists.
        "runs_artifact_gates": touches_generation_input(base_files),
        "runs_rust_suites": touches_ci_code(base_files),
    }


def summarize(result: dict) -> str:
    """One human line: what moved, what it can stale, and where it collides."""
    parts = [
        f"rebase-overlap | base gained {result['base_commits']} commit(s) "
        f"touching {len(result['base_files'])} file(s)"
    ]
    if result["overlap"]:
        parts.append(f"{len(result['overlap'])} overlap with this branch")
    else:
        parts.append("no overlap with this branch")

    gates = []
    if result["runs_artifact_gates"]:
        gates.append("artifact gates")
    if result["runs_rust_suites"]:
        gates.append("Rust suites")
    if gates:
        parts.append("base delta can stale: " + ", ".join(gates))
    else:
        # The whole point of the tool: name the skip explicitly rather than
        # leaving a reader to infer it from two empty lists.
        parts.append(
            "base delta touches no generation input and nothing outside CI's "
            "code filter — assert the gates once rather than re-running them"
        )
    return " | ".join(parts)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="rebase_overlap.py")
    parser.add_argument(
        "--from",
        dest="previous_base",
        required=True,
        help="the base the branch was on: git merge-base HEAD <base>, "
        "captured before the fetch/rebase (NOT git rev-parse origin/<base>)",
    )
    parser.add_argument(
        "--to", dest="new_base", default="origin/main", help="the base tip after"
    )
    parser.add_argument("--branch", default="HEAD", help="the branch under review")
    args = parser.parse_args(argv[1:])

    check_from_is_ancestor(args.previous_base, args.new_base)
    result = analyze(args.previous_base, args.new_base, args.branch)
    json.dump(result, sys.stdout, indent=2)
    print()
    print(summarize(result), file=sys.stderr)
    return 0


def main() -> int:
    # Every failure path here originates in `review_diff._git`, so
    # `ReviewDiffError` is the only exception this module can surface. It
    # deliberately defines no error type of its own: an unraised, unreachable
    # one would just be dead code with a matching dead `except` arm.
    try:
        return run(sys.argv)
    except ReviewDiffError as exc:
        print(f"rebase-overlap: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
