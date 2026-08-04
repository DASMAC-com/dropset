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
      "ready": true,               // exactly `not blockers`
      "blockers": []               // why ready is false, if it is
    }

``ready: false`` means **do not fan out**. It is defined as ``not blockers``, so
it can never drift from the reasons. Four things block:

* the base advanced past the rebase (the phantom-deletion failure the skill
  documents, which a line count structurally cannot catch);
* the ``git fetch`` **failed**, so freshness is *unverified* rather than
  verified-fresh — pass ``--no-fetch`` to accept the local ref deliberately;
* nothing changed at all;
* something changed but every changed path is an excluded generated family, so
  there is no source to review (the step 9/10 gates still apply — this is
  reported as its own reason rather than as "nothing to review").

Standard library only. A Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling"). Tests live in ``tests/test_review_diff.py``, run via
``make tools-tests``.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
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


def parse_numstat_z(text: str) -> list[dict]:
    """Parse ``git diff --numstat -z`` into ``[{path, changes}]``, path-sorted.

    ``--numstat`` is used rather than ``--stat`` because it is machine-readable:
    counts in decimal with a raw path, no column padding. **``-z`` is not
    optional**, and this parser only accepts that form. Without it git renders a
    rename as one *pretty* field — ``{infra/aws => cfg}/x.yaml`` — which begins
    with ``{`` and so matches none of the real path prefixes the gates key on;
    a rename that moved a generation input between trees would then read as
    ``runs_artifact_gates: false`` and silently skip the conformance gate. Plain
    output also *quotes* non-ASCII paths (``"na\\303\\257ve dir/f.md"``), quote
    characters and octal escapes included. ``-z`` fixes both.

    The ``-z`` layout is NUL-separated and unambiguous::

        "3\\t4\\tplain/path"        one field: counts and path, tab-separated
        "-\\t-\\tbinary/path"       a binary file reports "-" for both counts
        "0\\t0\\t", "old", "new"    a rename: EMPTY path, then two path fields

    So a rename is detected by the trailing-empty path field, and the two
    following fields are its source and destination; the **destination** is what
    a review cares about. A binary file still changed, so it lands with
    ``changes: 0`` rather than being dropped.
    """
    fields = text.split("\0")
    if fields and fields[-1] == "":
        fields.pop()

    files: list[dict] = []
    i = 0
    while i < len(fields):
        parts = fields[i].split("\t")
        if len(parts) < 3:
            i += 1
            continue
        added, deleted, path = parts[0], parts[1], parts[2]
        if path == "":
            # Rename/copy: consume this record plus its source and destination.
            if i + 2 >= len(fields):
                break  # truncated trailer — nothing trustworthy left to read
            path = fields[i + 2]
            i += 3
        else:
            i += 1
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

    Two deliberate choices:

    * **The excludes are ``:(top,exclude)``, with no positive pathspec.** A bare
      ``"."`` limiter would resolve against the *process* cwd, so running from a
      subdirectory would silently scope the written diff to that subtree while
      the repo-wide ``--numstat`` call disagreed with it. ``top`` anchors each
      exclude at the repo root, and omitting the positive pattern keeps the diff
      repo-wide from anywhere.
    * **Owner-only mode.** A review diff carries whatever the branch changed —
      possibly an added fixture key or a config token — and it lands in a shared
      temp tree, so it gets ``0o600`` for the same reason ``run_quiet.py`` gives
      its captured logs. Truncating an existing ``out_path`` *is* intended: the
      rewrite-every-run behavior is what retires the stale-diff hazard.
    """
    out_path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    pathspec = [f":(top,exclude){p}" for p in DIFF_EXCLUDES]
    try:
        fd = os.open(out_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8", errors="replace") as fh:
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
    """Run the whole preamble and return the verdict dict (see module docs).

    ``ready`` is derived from ``blockers`` — never computed alongside it — so the
    two cannot disagree about whether the fan-out may proceed.
    """
    if base.startswith("-"):
        # `base` is interpolated into argv; a leading dash would be read by git
        # as an option rather than a ref. No shell is involved, so this is option
        # injection rather than command injection, but refuse it regardless.
        raise ReviewDiffError(f"invalid base branch name: {base!r}")

    base_ref = f"origin/{base}"

    fetched = False
    fetch_error = None
    if fetch:
        try:
            _git(["fetch", "origin", base])
            fetched = True
        except ReviewDiffError as exc:
            fetch_error = str(exc)

    base_ahead = oneline_log(f"HEAD..{base_ref}")
    commits = oneline_log(f"{base_ref}..HEAD")
    diff_lines = write_diff(base_ref, out)
    files = parse_numstat_z(_git(["diff", "--numstat", "-z", f"{base_ref}..HEAD"]))

    paths = [f["path"] for f in files]
    runs_rust_suites = any(not matches_any(p, CODE_FILTER_EXCLUDES) for p in paths)
    runs_artifact_gates = any(matches_any(p, GENERATION_INPUTS) for p in paths)

    base_fresh = not base_ahead
    diff_empty = diff_lines == 0

    blockers = []
    if fetch_error is not None:
        # A failed fetch means `base_ahead` was computed against a possibly-stale
        # local ref, so freshness is *unverified* — not verified-fresh. Blocking
        # it is the whole point: an unverified base is exactly the phantom-
        # deletion condition the gate exists to catch. `--no-fetch` is the
        # explicit, deliberate way to review against the local ref.
        blockers.append(
            f"could not fetch {base_ref} ({fetch_error}) — freshness is "
            f"unverified against a possibly-stale local ref; fix the remote, or "
            f"pass --no-fetch to accept the local ref deliberately"
        )
    if not base_fresh:
        blockers.append(
            f"{base_ref} has {len(base_ahead)} commit(s) HEAD lacks — re-fetch, "
            f"rebase onto {base_ref}, and re-run this gate before fanning out"
        )
    if not files:
        blockers.append(
            f"no files changed between {base_ref} and HEAD — nothing to review "
            f"(check the base and the branch)"
        )
    elif diff_empty:
        # Distinct from "nothing changed": every changed path is an excluded
        # generated family, so there is no *source* to fan out over — but the
        # step 9/10 artifact and suite gates still have real work to do.
        blockers.append(
            f"{out} is empty because every changed path is an excluded generated "
            f"family ({', '.join(DIFF_EXCLUDES)}) — no source to review, but the "
            f"regeneration and suite gates still apply"
        )

    return {
        "base": base,
        "base_ref": base_ref,
        "fetched": fetched,
        "fetch_error": fetch_error,
        "base_fresh": base_fresh,
        "base_ahead": base_ahead,
        "commits": commits,
        "diff_path": str(out),
        "diff_lines": diff_lines,
        "diff_empty": diff_empty,
        "files": files,
        "runs_rust_suites": runs_rust_suites,
        "runs_artifact_gates": runs_artifact_gates,
        "ready": not blockers,
        "blockers": blockers,
    }


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
