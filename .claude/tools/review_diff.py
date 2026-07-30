#!/usr/bin/env python3
"""Run ``review-pr`` step 5's diff-and-freshness preamble and print one verdict.

The preamble was six fixed commands in a fixed order on **every** review —
``git fetch origin <base>``, ``git log HEAD..origin/<base>``, the
``git diff --output`` carrying five generated-family excludes,
``git log origin/<base>..HEAD``, ``git diff --stat``, and a ``wc -l`` — all of
it pure string/path logic resolving to a mechanical verdict: is the base fresh,
is the diff non-empty, which files changed, and which gates does that imply?
That is the settled-and-repeated shape ``docs/conventions/skill-tooling.md``
names, so it lives here and the skill drives it: six tool results collapse into
one compact JSON object (per ``CLAUDE.md`` → "Context economy").

The second reason it is a tool: three path lists that used to sit as prose in
the skill, re-typed by hand each run, are now **data with one owner** —

* ``DIFF_EXCLUDES`` — the generated families kept out of the review diff.
* ``CODE_FILTER_EXCLUDES`` — a mirror of ``.github/workflows/test.yml``'s
  ``code`` path filter, which decides whether CI runs the Rust suites at all.
* ``GENERATION_INPUTS`` — the sources a committed generated artifact is built
  from, which decides whether an artifact can even be stale.

Usage::

    python3 .claude/tools/review_diff.py --base main --out /tmp/review-diff.txt

Options: ``--base`` (default ``main``), ``--out`` (required — where the diff is
written), ``--no-fetch`` (skip the network fetch and read the local ref).

Prints JSON::

    {
      "base": "main",
      "base_ref": "origin/main",
      "fetched": true,
      "base_fresh": true,          // false iff base_ahead is non-empty
      "base_ahead": [],            // commits the base has that HEAD lacks
      "commits": ["abc1234 Subject", …],
      "diff_path": "/tmp/review-diff.txt",
      "diff_lines": 1234,
      "diff_empty": false,
      "files": [{"path": "…", "changes": 12}],
      "runs_rust_suites": false,   // any path outside the CI code filter?
      "runs_artifact_gates": false,// any generation input touched?
      "ready": true,               // base_fresh and not diff_empty
      "blockers": []               // why ready is false, if it is
    }

``ready: false`` means **do not fan out**: either the base advanced past the
rebase (the phantom-deletion failure the skill documents, which a line count
structurally cannot catch) or the diff is empty. ``blockers`` names which.

Standard library only. A Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling"). Tests live in ``tests/test_review_diff.py``, run via
``make tools-tests``.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import subprocess
import sys
from pathlib import Path

# Generated families excluded from the review diff: machine-authored, reviewed
# by their regeneration gate (step 9) rather than by eye, and large enough to
# swamp the diff the lenses actually read.
DIFF_EXCLUDES = (
    "pnpm-lock.yaml",
    "Cargo.lock",
    "sdk/ts/src/generated",
    "sdk/rs/src/generated",
    "sdk/idl/dropset.json",
)

# A mirror of the ``code`` filter in .github/workflows/test.yml, which runs with
# ``predicate-quantifier: every`` — so a diff whose every path matches one of
# these makes all three Tests jobs pass in seconds as path-filtered no-ops, and
# running the Rust suites locally mirrors nothing. Keep in sync with that
# workflow; the step-5 freshness lens covers exactly this kind of drift.
CODE_FILTER_EXCLUDES = (
    "frontend/**",
    "decks/**",
    "brand-assets/**",
    "docs/**",
    "**/*.md",
    "sdk/ts/**",
    "sdk/codama/**",
    ".claude/**",
    ".github/workflows/explorer-image.yml",
    ".github/workflows/lint.yml",
    ".github/workflows/sdk.yml",
    ".github/workflows/semantic-pr.yml",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "cfg/**",
    "infra/**",
)

# The sources each committed generated artifact is built from. A diff touching
# none of these cannot have staled any of them, so the three regeneration gates
# are provably no-ops:
#   programs/**                      -> sdk/idl/dropset.json  (anchor idl build)
#   sdk/idl/**, sdk/codama/**        -> the TS + Rust clients (codama)
#   sdk/math-core/**, sdk/interface/** -> sdk/conformance/*.json (generators)
GENERATION_INPUTS = (
    "programs/**",
    "sdk/idl/**",
    "sdk/codama/**",
    "sdk/math-core/**",
    "sdk/interface/**",
)


class ReviewDiffError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def matches(path: str, pattern: str) -> bool:
    """Whether ``path`` matches one path-filter pattern.

    Supports the three shapes the lists above use: a ``dir/**`` subtree, a
    ``**/*.ext`` suffix match at any depth, and a literal path.
    """
    if pattern.endswith("/**"):
        prefix = pattern[:-3]
        return path == prefix or path.startswith(prefix + "/")
    if pattern.startswith("**/"):
        return fnmatch.fnmatch(path, pattern[3:]) or fnmatch.fnmatch(path, pattern)
    return path == pattern


def matches_any(path: str, patterns) -> bool:
    """Whether ``path`` matches at least one of ``patterns``."""
    return any(matches(path, p) for p in patterns)


def _git(args: list[str], cwd: Path | None = None) -> str:
    """Run a read-only git command and return its stdout, or raise."""
    try:
        completed = subprocess.run(
            ["git", *args],
            cwd=str(cwd) if cwd else None,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            errors="replace",
            check=False,
        )
    except OSError as exc:  # git missing entirely
        raise ReviewDiffError(f"cannot run git: {exc}") from exc
    if completed.returncode != 0:
        detail = (completed.stderr or "").strip() or f"exit {completed.returncode}"
        raise ReviewDiffError(f"git {' '.join(args)} failed: {detail}")
    return completed.stdout


def oneline_log(rev_range: str) -> list[str]:
    """``git log <range> --oneline`` as a list of ``<sha> <subject>`` lines."""
    out = _git(["log", rev_range, "--oneline"])
    return [line for line in out.splitlines() if line.strip()]


def parse_numstat(text: str) -> list[dict]:
    """Parse ``git diff --numstat`` into ``[{path, changes}]``, path-sorted.

    ``--numstat`` is used rather than ``--stat`` because it is machine-readable:
    tab-separated added/deleted counts and a raw path, with no column padding or
    ``=>`` rename graph to unpick. A binary file reports ``-`` for both counts;
    it still changed, so it lands with ``changes: 0`` rather than being dropped.
    """
    files: list[dict] = []
    for line in text.splitlines():
        parts = line.split("\t")
        if len(parts) < 3:
            continue
        added, deleted, path = parts[0], parts[1], parts[-1]
        changes = 0
        for count in (added, deleted):
            if count.isdigit():
                changes += int(count)
        files.append({"path": path, "changes": changes})
    files.sort(key=lambda f: f["path"])
    return files


def write_diff(base_ref: str, out_path: Path) -> int:
    """Write the excluded review diff to ``out_path``; return its line count.

    The diff is streamed straight to the file — it never passes through this
    process's memory, and never through the model's context.
    """
    out_path.parent.mkdir(parents=True, exist_ok=True)
    pathspec = [".", *(f":(exclude){p}" for p in DIFF_EXCLUDES)]
    try:
        with open(out_path, "w", encoding="utf-8", errors="replace") as fh:
            completed = subprocess.run(
                ["git", "diff", f"{base_ref}..HEAD", "--", *pathspec],
                stdout=fh,
                stderr=subprocess.PIPE,
                text=True,
                errors="replace",
                check=False,
            )
    except OSError as exc:
        raise ReviewDiffError(f"cannot write {out_path}: {exc}") from exc
    if completed.returncode != 0:
        detail = (completed.stderr or "").strip() or f"exit {completed.returncode}"
        raise ReviewDiffError(f"git diff failed: {detail}")
    return count_lines(out_path)


def count_lines(path: Path) -> int:
    """Line count of a file, read a line at a time so a huge diff stays cheap."""
    total = 0
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        for _ in fh:
            total += 1
    return total


def gate(base: str, out: Path, fetch: bool = True) -> dict:
    """Run the whole preamble and return the verdict dict (see module docs)."""
    base_ref = f"origin/{base}"

    fetched = False
    fetch_error = None
    if fetch:
        try:
            _git(["fetch", "origin", base])
            fetched = True
        except ReviewDiffError as exc:
            # Offline or a missing remote must not sink the review: fall back to
            # the local ref and say so, rather than pretending the base is fresh.
            fetch_error = str(exc)

    base_ahead = oneline_log(f"HEAD..{base_ref}")
    commits = oneline_log(f"{base_ref}..HEAD")
    diff_lines = write_diff(base_ref, out)
    files = parse_numstat(_git(["diff", "--numstat", f"{base_ref}..HEAD"]))

    paths = [f["path"] for f in files]
    runs_rust_suites = any(
        not matches_any(p, CODE_FILTER_EXCLUDES) for p in paths
    )
    runs_artifact_gates = any(matches_any(p, GENERATION_INPUTS) for p in paths)

    base_fresh = not base_ahead
    diff_empty = diff_lines == 0

    blockers = []
    if not base_fresh:
        blockers.append(
            f"{base_ref} has {len(base_ahead)} commit(s) HEAD lacks — re-fetch, "
            f"rebase onto {base_ref}, and re-run this gate before fanning out"
        )
    if diff_empty:
        blockers.append(
            f"{out} is empty — nothing to review (check the base and the branch)"
        )

    verdict = {
        "base": base,
        "base_ref": base_ref,
        "fetched": fetched,
        "base_fresh": base_fresh,
        "base_ahead": base_ahead,
        "commits": commits,
        "diff_path": str(out),
        "diff_lines": diff_lines,
        "diff_empty": diff_empty,
        "files": files,
        "runs_rust_suites": runs_rust_suites,
        "runs_artifact_gates": runs_artifact_gates,
        "ready": base_fresh and not diff_empty,
        "blockers": blockers,
    }
    if fetch_error is not None:
        verdict["fetch_error"] = fetch_error
    return verdict


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="review_diff.py")
    parser.add_argument("--base", default="main", help="base branch (default main)")
    parser.add_argument("--out", required=True, help="where to write the review diff")
    parser.add_argument(
        "--no-fetch",
        action="store_true",
        help="skip the network fetch and read the local ref",
    )
    args = parser.parse_args(argv[1:])

    verdict = gate(args.base, Path(args.out), fetch=not args.no_fetch)
    json.dump(verdict, sys.stdout, indent=2)
    sys.stdout.write("\n")
    # Exit non-zero when the gate says don't fan out, so a caller that only
    # checks the status still can't proceed on a stale base.
    return 0 if verdict["ready"] else 1


def main() -> int:
    try:
        return run(sys.argv)
    except ReviewDiffError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
