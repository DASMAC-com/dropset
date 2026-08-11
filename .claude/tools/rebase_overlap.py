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

    # Capture the base tip BEFORE fetching, then compare after:
    git rev-parse origin/main                  # -> <prev>
    git fetch origin main
    python3 .claude/tools/rebase_overlap.py --from <prev> --to origin/main

``--from`` is required and deliberately so: the pre-fetch base tip is the one
value that cannot be recovered afterwards, and guessing it (``ORIG_HEAD`` is the
pre-rebase *branch* head, not the old base) would silently compare the wrong
range. ``--to`` defaults to ``origin/main``; ``--branch`` defaults to ``HEAD``.

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
    CODE_FILTER_EXCLUDES,
    GENERATION_INPUTS,
    ReviewDiffError,
    _git,
    matches_any,
)


class RebaseOverlapError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def changed_files(rev_range: str) -> list[str]:
    """``git diff --name-only <range>``, as a sorted list of repo-relative paths."""
    out = _git(["diff", "--name-only", rev_range])
    return sorted({line.strip() for line in out.splitlines() if line.strip()})


def commit_subjects(rev_range: str) -> list[str]:
    """``git log <range> --oneline`` as ``<sha> <subject>`` lines."""
    out = _git(["log", rev_range, "--oneline"])
    return [line for line in out.splitlines() if line.strip()]


def merge_base(a: str, b: str) -> str:
    """The merge base of two revisions."""
    return _git(["merge-base", a, b]).strip()


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
        # regenerated, not whether this branch touched one.
        "runs_artifact_gates": any(
            matches_any(p, GENERATION_INPUTS) for p in base_files
        ),
        "runs_rust_suites": any(
            not matches_any(p, CODE_FILTER_EXCLUDES) for p in base_files
        ),
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
        help="the base tip BEFORE the fetch (capture with git rev-parse)",
    )
    parser.add_argument(
        "--to", dest="new_base", default="origin/main", help="the base tip after"
    )
    parser.add_argument("--branch", default="HEAD", help="the branch under review")
    args = parser.parse_args(argv[1:])

    result = analyze(args.previous_base, args.new_base, args.branch)
    json.dump(result, sys.stdout, indent=2)
    print()
    print(summarize(result), file=sys.stderr)
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except (RebaseOverlapError, ReviewDiffError) as exc:
        print(f"rebase-overlap: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
