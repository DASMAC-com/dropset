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
* ``SEARCH_EXCLUDE_DIRS`` — the never-search trees a recursive ``grep`` must skip
  (it does not honor gitignore), exposed with the generated families through
  ``--print-grep-excludes``.

Usage::

    python3 .claude/tools/review_diff.py --base main --out /tmp/review-diff.txt --split
    python3 .claude/tools/review_diff.py --print-grep-excludes

Options: ``--base`` (default ``main``), ``--out`` (where the diff is written —
required except with ``--print-grep-excludes``), ``--no-fetch`` (skip the network
fetch and read the local ref), ``--split`` (also write per-lens
``source`` / ``tests`` / ``docs`` slices beside the diff, reported under
``slices``), ``--print-grep-excludes`` (print the exclude flags a hoisted
repo-wide grep should reuse, and exit).

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
# NOTE for editors: `grep_excludes()` and `search_source.excluded_dir_names()`
# classify each entry as file-vs-directory by whether its last segment contains a
# dot. Every entry below obeys that (`generated` is the only directory, and it
# carries no dot), but a *dotted directory* added here would be read as a file
# and emitted as `--exclude=<name>` instead of being pruned as a tree. Put a
# dotted directory in SEARCH_EXCLUDE_DIRS below, which is taken verbatim.
DIFF_EXCLUDES = (
    "pnpm-lock.yaml",
    "Cargo.lock",
    "sdk/ts/src/generated",
    "sdk/rs/src/generated",
    "sdk/idl/dropset.json",
)

# Directories a recursive source search must never descend into. These are
# *not* in DIFF_EXCLUDES because `git diff` never shows them — they're
# gitignored or VCS internals — but `grep -r` walks them happily and does not
# honor gitignore, so a hoisted grep that omits them is unusable in this repo
# (`target/` alone is multi-GB). `--print-grep-excludes` emits these together
# with the generated families, since a search needs both lists.
SEARCH_EXCLUDE_DIRS = (
    ".git",
    "node_modules",
    "target",
    ".next",
    "dist",
    "__pycache__",
    # Tool caches. These store content-addressed blobs of the very source being
    # searched, so a symbol sweep hits each match twice — once in the file and
    # once in the cache — which is how `.ruff_cache` turned up in a real sweep
    # for `sync_blockers.py` callers.
    ".ruff_cache",
    ".pytest_cache",
    ".mypy_cache",
    # Live worktrees, which sit under the base repo's `.claude/worktrees/` and
    # are each a *full checkout of this same repo*. Searching from the base
    # repo therefore returns every match once per live worktree: one measured
    # planning session paid ~10.7k across two sweeps that were ~6x duplicated.
    # A worktree's own session searches its own root, so nothing is lost by
    # pruning the tree here.
    "worktrees",
)

# Which per-lens slice a changed path belongs to, for ``--split``. Ordered
# most-specific-first: a path is classified by the first list it matches, so
# `docs/x.md` is docs and `sdk/rs/tests/y.rs` is tests even though both would
# also match a broader rule below them.
DOCS_PATTERNS = (
    "**/*.md",
    "docs/**",
    "decks/**",
    "**/*.mdx",
)

# File-level test patterns only. Rust's convention puts unit tests *inline* in
# the source file under `#[cfg(test)]`, so those hunks necessarily land in the
# source slice — the split is by file, not by region. The tests slice therefore
# means "files that exist only to test", which is what a completeness lens wants
# to read end-to-end.
TESTS_PATTERNS = (
    "**/tests/**",
    "**/test_*.py",
    "**/*_test.rs",
    "**/*.test.ts",
    "**/*.test.tsx",
    "**/*.spec.ts",
    "sdk/conformance/**",
)

SLICE_NAMES = ("source", "tests", "docs")

# A mirror of the ``code`` filter in .github/workflows/test.yml, which runs with
# ``predicate-quantifier: every`` — so a diff whose every path matches one of
# these makes all three Tests jobs pass in seconds as path-filtered no-ops, and
# running the Rust suites locally mirrors nothing.
#
# Drift here is silent and its only symptom is wasted wall-clock: the list once
# omitted the frontend workflow file, so a PR touching it got a false
# runs-Rust-suites verdict and `review-pr` ran the 20-to-40-minute local suite to
# mirror a CI job that was never going to run. ``tests/test_review_diff.py`` now
# asserts parity against the workflow itself, which is what makes the next drift
# loud instead — the freshness lens is a second check, not the first.
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
    ".github/workflows/frontend.yml",
    ".github/workflows/lint.yml",
    ".github/workflows/sdk.yml",
    ".github/workflows/semantic-pr.yml",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "cfg/**",
    "infra/**",
)

# Where that filter actually lives, for the parity test.
TESTS_WORKFLOW = ".github/workflows/test.yml"

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


def touches_generation_input(paths) -> bool:
    """Whether any path is a source a committed generated artifact is built from.

    Exported because ``rebase_overlap.py`` asks the same question of a *base
    delta* rather than a review diff. Sharing the **predicate**, not just
    :data:`GENERATION_INPUTS`, is the point: two copies of the ``any(...)`` can
    drift the moment either grows a qualifying clause, and both callers use the
    answer to license skipping an expensive regeneration gate.
    """
    return any(matches_any(p, GENERATION_INPUTS) for p in paths)


def touches_ci_code(paths) -> bool:
    """Whether any path falls outside CI's ``code`` path filter.

    The companion to :func:`touches_generation_input`, shared for the same
    reason — note the inverted sense (a path is relevant when it does *not*
    match an exclude), which is exactly the kind of detail a second copy gets
    wrong.
    """
    return any(not matches_any(p, CODE_FILTER_EXCLUDES) for p in paths)


def matches(path: str, pattern: str) -> bool:
    """Whether ``path`` matches one path-filter pattern.

    Supports the four shapes the lists above use:

    * ``**/seg/**`` — that directory segment at any depth (``**/tests/**``).
      Checked **first**, because it also ends in ``/**`` and would otherwise be
      read as the literal subtree ``**/tests``.
    * ``dir/**`` — a subtree rooted at ``dir``.
    * ``**/*.ext`` — a suffix match at any depth.
    * anything else — a literal path.
    """
    if pattern.startswith("**/") and pattern.endswith("/**"):
        middle = pattern[3:-3]
        return f"/{middle}/" in f"/{path}/"
    if pattern.endswith("/**"):
        prefix = pattern[:-3]
        return path == prefix or path.startswith(prefix + "/")
    if pattern.startswith("**/"):
        return fnmatch.fnmatch(path, pattern[3:]) or fnmatch.fnmatch(path, pattern)
    return path == pattern


def slice_for(path: str) -> str:
    """Which ``--split`` slice a changed path belongs to.

    Docs is checked before tests so a markdown file under a ``tests/`` tree reads
    as docs, and everything unmatched is source — the default a reviewer wants,
    since a misfiled source file is a missed review while a misfiled doc is only
    noise.
    """
    if matches_any(path, DOCS_PATTERNS):
        return "docs"
    if matches_any(path, TESTS_PATTERNS):
        return "tests"
    return "source"


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


def _diff_header_path(line: str) -> str | None:
    """The destination path from a ``diff --git a/X b/Y`` header, or ``None``.

    Takes the ``b/`` side because that is what the change produced — for a rename
    the ``a/`` side no longer exists.

    Note git keeps a **real path on both sides even for a deletion** (``/dev/null``
    appears on the following ``---``/``+++`` lines, which this never inspects), so
    there is no ``/dev/null`` case to handle here.

    Paths may contain spaces, so the pair is split on the ``" b/"`` separator
    rather than on whitespace. That takes the **first** occurrence, so a path that
    itself contains the literal sequence ``" b/"`` mis-splits and the hunk lands in
    the default ``source`` slice. Accepted: the consequence is that one hunk goes
    to the wrong slice on a pathological filename, and ``source`` is the safe side
    to land on.
    """
    if not line.startswith("diff --git "):
        return None
    rest = line[len("diff --git ") :].rstrip("\n")
    marker = rest.find(" b/")
    if marker == -1:
        return None
    return rest[marker + 3 :] or None


def split_diff(diff_path: Path, out_dir: Path) -> dict:
    """Partition a review diff into per-lens slices; return ``{name: {path, lines}}``.

    Five lenses each ``Read`` the *same whole* diff today, which is the structural
    residual after prompt tightening has saturated: a diff carrying a couple of
    hundred lines of reflowed prose makes every lens pay for it, when only the
    freshness lens needs the docs hunks and only completeness needs the tests.
    Slicing by file lets step 5 hand each lens the part it reads.

    Streamed line by line and written straight through, so a large diff never
    enters this process's memory (nor, being a file handoff, the model's context).
    Slices are ``0o600`` for the same reason the full diff is. A slice with no
    hunks is still **written, empty** — a missing file would be ambiguous between
    "nothing in this category" and "the split didn't run".
    """
    out_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    paths = {name: out_dir / f"review-diff-{name}.txt" for name in SLICE_NAMES}
    # The slice handles are opened O_TRUNC *before* the diff is read, so a `--out`
    # that collides with a slice name would have its own input truncated first and
    # every slice would come out empty with no error. Refuse instead.
    if diff_path.resolve() in {p.resolve() for p in paths.values()}:
        raise ReviewDiffError(
            f"--out {diff_path} collides with a --split slice name; choose another "
            f"name (the slices are review-diff-source/tests/docs.txt in that dir)"
        )
    handles = {}
    counts = dict.fromkeys(SLICE_NAMES, 0)
    try:
        for name, path in paths.items():
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            handles[name] = os.fdopen(fd, "w", encoding="utf-8", errors="replace")

        # Anything before the first `diff --git` header (there is normally
        # nothing) goes to source, the default slice.
        current = "source"
        with open(diff_path, "r", encoding="utf-8", errors="replace") as fh:
            for line in fh:
                header_path = _diff_header_path(line)
                if header_path is not None:
                    current = slice_for(header_path)
                handles[current].write(line)
                counts[current] += 1
    except OSError as exc:
        raise ReviewDiffError(f"cannot write diff slices: {exc}") from exc
    finally:
        for handle in handles.values():
            handle.close()

    return {
        name: {"path": str(paths[name]), "lines": counts[name]} for name in SLICE_NAMES
    }


def grep_excludes() -> dict:
    """The exclude list a hoisted repo-wide grep should reuse, in grep's own flags.

    Step 5 asks for repo-scope greps (a uniqueness sweep, a straggler search), and
    an **unscoped** one returns the whole regenerated SDK surface no lens needs —
    a 658-line generated instruction file was the single largest main-loop result
    of one measured session. This tool already owns ``DIFF_EXCLUDES``, so exposing
    it here means one list with one owner rather than a set re-derived per run.

    ``grep --exclude-dir`` matches a directory's **basename**, not its path, so a
    directory-shaped exclude is reduced to its last segment: excluding
    ``sdk/ts/src/generated`` becomes ``--exclude-dir=generated``, which also
    catches its Rust sibling. That is wider than the diff exclude, and correct —
    a search wants no generated tree.
    """
    dirs: list[str] = []
    globs: list[str] = []
    for entry in DIFF_EXCLUDES:
        basename = entry.rsplit("/", 1)[-1]
        # A dotted last segment is a file; anything else is a directory.
        if "." in basename and not entry.endswith("/"):
            if basename not in globs:
                globs.append(basename)
        elif basename not in dirs:
            dirs.append(basename)
    for entry in SEARCH_EXCLUDE_DIRS:
        if entry not in dirs:
            dirs.append(entry)

    args = [f"--exclude-dir={d}" for d in dirs] + [f"--exclude={g}" for g in globs]
    return {
        "exclude_dirs": dirs,
        "exclude_globs": globs,
        "grep_args": " ".join(args),
    }


def gate(base: str, out: Path, fetch: bool = True, split: bool = False) -> dict:
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
    runs_rust_suites = touches_ci_code(paths)
    runs_artifact_gates = touches_generation_input(paths)

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

    verdict = {
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
    if split:
        # Slices live beside the full diff, so one --out choice places everything.
        verdict["slices"] = split_diff(out, out.parent)
    return verdict


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="review_diff.py")
    parser.add_argument("--base", default="main", help="base branch (default main)")
    parser.add_argument(
        "--out", default=None, help="where to write the review diff (required)"
    )
    parser.add_argument(
        "--no-fetch",
        action="store_true",
        help="skip the network fetch and read the local ref",
    )
    parser.add_argument(
        "--split",
        action="store_true",
        help="also write per-lens source/tests/docs slices beside the diff",
    )
    parser.add_argument(
        "--print-grep-excludes",
        action="store_true",
        help="print the exclude flags a hoisted repo-wide grep should reuse, and exit",
    )
    args = parser.parse_args(argv[1:])

    # A pure query about the exclude lists: no repo state, no --out, no git.
    if args.print_grep_excludes:
        json.dump(grep_excludes(), sys.stdout, indent=2)
        sys.stdout.write("\n")
        return 0

    if args.out is None:
        raise ReviewDiffError("--out is required (except with --print-grep-excludes)")

    verdict = gate(args.base, Path(args.out), fetch=not args.no_fetch, split=args.split)
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
