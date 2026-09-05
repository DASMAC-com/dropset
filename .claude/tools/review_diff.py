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
``slices``), ``--gate-only`` (emit just the verdict fields — for a mid-review
re-check), ``--print-grep-excludes`` (print the exclude flags a hoisted
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
      "crates": {"sdk": {"source": 12, "docs": 0, "tests": 0,
                         "has_source": true}},
      "code_crates": 1,           // trees with an actual source change
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

``--gate-only`` keeps the verdict fields — including the two skip predicates
``runs_rust_suites`` / ``runs_artifact_gates``, which answer "what may I skip?"
for two booleans — and drops the unbounded inventory (``commits``, ``files``,
``slices``, ``diff_path``, ``diff_lines``). The full payload is what a
**fan-out** needs; a mid-review *re-check* consumes only ``base_fresh`` /
``ready`` / ``blockers``, and one measured run printed a 70-file ``files`` array
to answer exactly that — for a diff that had just been rebased away. The gating
still runs in full and the exit status is unchanged; only the projection
narrows. Making the re-check nearly free is the point, because a check that
costs a payload is a check that quietly stops being run.

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
import re
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
    # The Grafana dashboard SQL mirror, emitted by `dashboard_sql.py` from the
    # dashboard JSON. Machine-authored and gated by its own regeneration check
    # (`make dashboard-sql-check`), so reviewing it by eye adds nothing — and it
    # is one file per panel, so it would otherwise swamp the diff the lenses
    # read. The JSON it is generated FROM stays in the diff, which is where a
    # query change should be reviewed.
    "market-data/grafana/sql",
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
    # for a tool's callers.
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
# `decks/` is narrowed to its PROSE rather than listed as a whole tree. The
# whole tree was on the wrong side of this module's own stated rationale — that
# everything unmatched is source, because a misfiled source file is a *missed
# review* while a misfiled doc is only a wasted read. Listing `decks/**`
# traded the safe failure for the unsafe one across a package: one diff of six
# `.tsx` / `.ts` / `.css` files carrying state, effects, URL parsing and a
# third-party selector produced `source: 0 lines` / `docs: 224 lines`, so the
# correctness, security and style lenses were each handed an **empty file** and
# the run had to notice and pass them the full diff instead. A slide-heavy deck
# component now lands in source, which costs a lens a little prose to read.
DOCS_PATTERNS = (
    "**/*.md",
    "docs/**",
    "decks/**/*.md",
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
    ".github/workflows/token-icon-audit.yml",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "cfg/**",
    "infra/**",
    "market-data/grafana/**",
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


# The single bucket every repository-root file shares. Angle brackets keep it
# from ever colliding with a real directory name.
ROOT_TREE = "<root>"


def crate_of(path: str) -> str:
    """The top-level tree a changed path belongs to.

    A coarse stand-in for "crate": the first path segment, which is what the
    workspace layout makes it (``programs/…``, ``sdk/rs/…``, ``feeds/…``). It
    exists to answer *how many trees does this diff span*, not to resolve a
    Cargo package, so a coarse answer is the right one.

    **A repository-ROOT file is not a tree, and treating it as one defeated the
    metric.** `Makefile` and `Cargo.toml` have no first segment, so returning
    the path itself made each root file its own "crate" and a diff touching two
    of them read as spanning two crates. Measured on the very diff that added
    this function: `cfg/dictionary.txt` and `.claude/**` gave `code_crates: 2`,
    where one of the two was a two-line dictionary entry — the same
    over-escalation the rollup was written to stop, arriving through the
    denominator instead of the numerator. Root files therefore share one
    bucket.
    """
    head, sep, _ = path.partition("/")
    if not sep:
        return ROOT_TREE
    return head or ROOT_TREE


def crate_rollup(files) -> dict:
    """``{crate: {"source": n, "docs": n, "tests": n, "has_source": bool}}``.

    Why this exists: the review tier's multi-crate trigger weighed the
    **presence** of a second crate rather than what changed in it, so a
    seven-line docs-only change in a second tree escalated a diff to the full
    fan-out. `--split` already separates docs from source per file; what was
    missing was the per-crate rollup that turns that into a predicate.

    Note **docs-only** above means a docs *path* — a `.md` file, or anything
    under `docs/**`. It used to read "doc-comment fix", which the bound below
    contradicts four lines later: a `///` comment lives in a `.rs` file, so it
    classifies as source and still escalates. Stating the motivating case as
    the tool actually decides it matters more than it sounds, because a reader
    tiering a diff takes that sentence as license and then the tool disagrees.

    ``has_source`` is the field a tier decision wants: a crate with no source
    **or test** changes at all should not count toward "spans crates". Note the
    deliberate asymmetry — this discounts such a crate from the *crate count*
    and nothing else. The change is still fully reviewed (one such seven-line
    doc fix was itself a real finding from an earlier PR that had gone stale),
    so the docs and completeness lenses must still see it.

    **Tests count as code.** This was `source > 0` alone, which discounted a
    crate whose only change was to its tests — and a second crate's test change
    is exactly the kind of cross-tree coupling the multi-crate trigger exists
    to catch. The docstring justified the discount for the *docs* case only, so
    excluding tests was scope the rationale never covered, and it failed open on
    the tier decision. Docs remain the only discounted slice.

    **The bound, stated rather than papered over:** this classifies by PATH,
    so it catches a crate whose changes are non-code *files* and not a crate
    whose `.rs` changes happen to be entirely doc comments. Line-level
    classification is not computed here, so a doc-comment-only source change
    still reads as source. That is the conservative direction — it over-counts
    crates rather than under-reviewing one — and closing it would need a
    line-level pass this tool does not do.
    """
    rollup: dict[str, dict] = {}
    for entry in files:
        path = entry["path"] if isinstance(entry, dict) else str(entry)
        changes = entry.get("changes", 0) if isinstance(entry, dict) else 0
        bucket = rollup.setdefault(crate_of(path), {"source": 0, "docs": 0, "tests": 0})
        bucket[slice_for(path)] += max(1, changes)
    for bucket in rollup.values():
        bucket["has_source"] = bucket["source"] > 0 or bucket["tests"] > 0
    return rollup


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


def write_diff(base_ref: str, out_path: Path, only: list[str] | None = None) -> int:
    """Write the excluded review diff to ``out_path``; return its line count.

    The diff is streamed straight to the file — it never passes through this
    process's memory, and never through the model's context.

    ``only`` restricts the diff to the given path globs. That is how an oversized
    slice gets subdivided: the review step asks for slices past ~1k lines to be
    broken up, and until this existed ``--split`` could only cut by *category*,
    so 3,156- and 4,148-line source slices went to every lens whole (the
    costliest single agent on one such run took 634.7k of input). The hand-rolled
    alternative was three separate ``git diff`` calls with literal path lists.

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
    if only:
        # Positive limiters are anchored at the repo root for the same reason the
        # excludes are: an unanchored pattern would resolve against the process
        # cwd and silently scope the diff differently depending on where the tool
        # was run from.
        pathspec = [f":(top,glob){p}" for p in only] + pathspec
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


_HUNK_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")

# The attribute that marks an inline Rust test module.
_CFG_TEST_RE = re.compile(r"^\s*#\[cfg\(test\)\]")


def rust_test_ranges(text: str) -> list[tuple[int, int]]:
    """1-based inclusive line ranges of every ``#[cfg(test)]`` item in ``text``.

    Rust keeps its unit tests **inside the source file**, so a category split
    that keys on the path alone produces a **zero-line tests slice** on a diff
    that is full of test changes. Measured: a 4,351-line Rust diff split to
    exactly zero test lines, so the test-adequacy lens had to read three source
    slices and became that run's costliest agent at 473.6k.

    Brace matching rather than regex-per-line, because a test module contains
    arbitrary nesting. String and char literals are skipped so a ``"}"`` inside
    a test fixture cannot close the module early; line comments are skipped for
    the same reason. Raw strings (``r#"..."#``) are handled too — they are
    common in test fixtures, which is exactly where this runs.

    **A brace-less annotated item ends at its semicolon.** `#[cfg(test)]` also
    guards declarations with no block at all — `#[cfg(test)] mod test_support;`
    and `#[cfg(test)] use …;` are both common, and the repo contains the first.
    Walking forward to "the next `{`" on those swallows whatever braced item
    happens to follow, which routes **production code into the tests slice**.
    So the walk stops at a `;` that closes the item before any brace opens.

    Block comments (``/* … */``) are **not** handled — a `{` inside one still
    counts. Stated rather than claimed otherwise: it needs multi-line state,
    and an unbalanced brace only ever over-claims a range, which errs toward
    tests rather than losing them.
    """
    lines = text.splitlines()
    ranges: list[tuple[int, int]] = []
    index = 0
    while index < len(lines):
        if not _CFG_TEST_RE.match(lines[index]):
            index += 1
            continue
        start = index
        # Walk forward to the first `{` that opens the annotated item, then brace
        # match to its close. An attribute may be followed by more attributes, a
        # doc comment, or the `mod`/`fn` line itself.
        depth = 0
        opened = False
        cursor = index
        while cursor < len(lines):
            depth, opened_here = _scan_braces(lines[cursor], depth)
            opened = opened or opened_here
            if opened and depth <= 0:
                break
            if not opened and _ends_item_without_block(lines[cursor]):
                # `#[cfg(test)] mod test_support;` — the item is complete and
                # no block ever opens. Claim these lines and nothing more.
                break
            cursor += 1
        if cursor >= len(lines):
            # Unbalanced (a truncated or unparsable file): claim to the end
            # rather than dropping the region, so tests are never lost to source.
            ranges.append((start + 1, len(lines)))
            break
        ranges.append((start + 1, cursor + 1))
        index = cursor + 1
    return ranges


# An item the `#[cfg(test)]` attribute guards that has no block: a `;` closes
# it. Checked only before any brace has opened, so a `;` inside a test body is
# irrelevant. Attribute and comment lines are skipped so `#[cfg(test)]` itself
# (and a doc comment under it) does not end the walk.
def _ends_item_without_block(line: str) -> bool:
    stripped = line.strip()
    if not stripped or stripped.startswith("#[") or stripped.startswith("//"):
        return False
    # A TRAILING comment has to come off before the `;` test, or
    # `#[cfg(test)] mod test_support; // helpers only` does not end the walk and
    # the scan runs on to swallow the next braced item — routing production code
    # into the tests slice, the exact failure this helper exists to prevent.
    # Splitting on `//` can misread a `//` inside a string literal on such a
    # line; that is a vanishingly rare shape, and erring here costs at most one
    # extra line of an already-correct range.
    code = stripped.split("//", 1)[0].rstrip()
    return code.endswith(";")


def _char_literal_end(line: str, start: int) -> int | None:
    """Index just past a Rust char literal at ``start``, or ``None``.

    ``None`` means the `'` is a lifetime, not a literal. The forms accepted are
    `'x'` and an escape `'\\n'` / `'\\''` / `'\\u{1F600}'`.
    """
    if start + 1 >= len(line):
        return None
    if line[start + 1] == "\\":
        close = line.find("'", start + 2)
        return close + 1 if close != -1 else None
    # A plain char literal is exactly one character wide.
    if start + 2 < len(line) and line[start + 2] == "'":
        return start + 3
    return None


def _scan_braces(line: str, depth: int) -> tuple[int, bool]:
    """``(new_depth, saw_open_brace)`` for one line, ignoring braces in literals."""
    opened = False
    i = 0
    while i < len(line):
        ch = line[i]
        if ch == "/" and i + 1 < len(line) and line[i + 1] == "/":
            break  # line comment: nothing after it counts
        if ch == "r" and i + 1 < len(line) and line[i + 1] in '#"':
            # Raw string: r"..." or r#"..."#. Skip to its terminator.
            hashes = 0
            j = i + 1
            while j < len(line) and line[j] == "#":
                hashes += 1
                j += 1
            if j < len(line) and line[j] == '"':
                close = '"' + "#" * hashes
                end = line.find(close, j + 1)
                if end == -1:
                    break  # runs past end of line; the rest cannot hold braces
                i = end + len(close)
                continue
        if ch == "'":
            # A `'` in Rust is a char literal ONLY in `'x'` or `'\n'` form —
            # otherwise it is a LIFETIME (`&'static str`, `where T: 'a`), which
            # has no closing quote. Treating a lifetime as an open literal made
            # the scan run to end-of-line and skip the trailing `{`, so `depth`
            # never incremented while the matching `}` still decremented it —
            # ending the test range an item early and routing the rest of the
            # module back to `source`.
            end = _char_literal_end(line, i)
            if end is None:
                i += 1  # a lifetime: ordinary text, keep scanning this line
                continue
            i = end
            continue
        if ch == '"':
            j = i + 1
            while j < len(line):
                if line[j] == "\\":
                    j += 2
                    continue
                if line[j] == '"':
                    break
                j += 1
            i = j + 1
            continue
        if ch == "{":
            depth += 1
            opened = True
        elif ch == "}":
            depth -= 1
        i += 1
    return depth, opened


def _in_any_range(line_no: int, ranges: list[tuple[int, int]]) -> bool:
    return any(start <= line_no <= end for start, end in ranges)


def _read_post_image(path: str) -> str | None:
    """The working-tree text of ``path``, or ``None`` if it is gone or binary."""
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            return fh.read()
    except OSError:
        return None


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
    # Namespaced off the `--out` STEM, not a fixed prefix. Fixed names made two
    # runs in one scratchpad silently destroy each other's slices: a
    # `--only '.claude/tools/**' --split` run has no docs hunks, so it wrote an
    # empty `review-diff-docs.txt` over the full run's 1684-line one — and the
    # skill's own instruction ("run it once per sub-slice, and hand each lens its
    # own `--out` path") reads as protection against exactly that while providing
    # none, because `--out` did not reach these names. The failure is silent and
    # total for the affected lens: it receives a 0-line slice and correctly
    # reports nothing to review.
    #
    # Deriving from the stem also makes the old `--out`-collides-with-a-slice
    # case structurally impossible (`<stem>-<name>.txt` can never equal
    # `<stem>.txt`), so the guard that used to sit here is gone rather than left
    # as a second, unreachable source of truth.
    stem = diff_path.stem
    paths = {name: out_dir / f"{stem}-{name}.txt" for name in SLICE_NAMES}
    # One hazard the stem derivation does NOT close: an `--out` that is itself
    # spelled like a slice (`--out review-diff-docs.txt`) writes the whole diff
    # over a previous run's docs slice. The old guard refused that filename as a
    # side effect; keep refusing it deliberately, since it is the same
    # silent-overwrite failure by a different route.
    if any(stem.endswith(f"-{name}") for name in SLICE_NAMES):
        raise ReviewDiffError(
            f"--out {diff_path.name} is spelled like a --split slice, so it "
            f"would overwrite one; choose a name not ending in "
            f"-{'/-'.join(SLICE_NAMES)}"
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
        # State for the inline-Rust-tests split. `header` buffers a file's whole
        # preamble so it can be replayed into whichever slice first receives one
        # of that file's hunks — a hunk without its file header is not readable
        # as a diff.
        #
        # Everything before the first `@@` is buffered, rather than only the
        # line shapes a whitelist happened to name. The whitelist form missed
        # `new file mode`, `rename from`/`rename to` and `copy from`/`copy to`,
        # which fell through and were emitted early — so an ADDED .rs file with
        # inline tests produced a tests slice whose header was `diff --git`
        # immediately followed by `@@`, with no `---`/`+++` at all. It also
        # dropped a header-only diff (a mode change) from every slice.
        test_ranges: list[tuple[int, int]] = []
        header: list[str] = []
        header_written: set[str] = set()
        in_preamble = False
        file_slice = "source"

        def emit(name: str, text: str) -> None:
            handles[name].write(text)
            counts[name] += 1

        def flush_header(name: str) -> None:
            """Replay the buffered preamble into ``name`` at most once."""
            if name in header_written:
                return
            for head_line in header:
                emit(name, head_line)
            header_written.add(name)

        with open(diff_path, "r", encoding="utf-8", errors="replace") as fh:
            for line in fh:
                header_path = _diff_header_path(line)
                if header_path is not None:
                    # A preceding file whose diff was header-only (a mode change,
                    # a pure rename) never reached a hunk, so flush it now rather
                    # than dropping it.
                    if header and not header_written:
                        flush_header(file_slice)
                    file_slice = slice_for(header_path)
                    current = file_slice
                    header = [line]
                    header_written = set()
                    in_preamble = True
                    # Only a Rust file landing in `source` can hide test hunks.
                    test_ranges = []
                    if file_slice == "source" and header_path.endswith(".rs"):
                        text = _read_post_image(header_path)
                        if text is not None:
                            test_ranges = rust_test_ranges(text)
                    continue

                match = _HUNK_RE.match(line)
                if match:
                    in_preamble = False
                    if test_ranges:
                        start = int(match.group(1))
                        # A hunk counts as tests when its post-image lines fall
                        # inside a cfg(test) region. Judged on the START line:
                        # a hunk straddling the boundary is rare, and putting it
                        # with its opening context is the readable choice.
                        current = (
                            "tests" if _in_any_range(start, test_ranges) else file_slice
                        )
                    else:
                        current = file_slice
                    flush_header(current)
                    emit(current, line)
                    continue

                if in_preamble:
                    header.append(line)
                    continue

                # `current`, NOT `file_slice`. A body line belongs to whichever
                # slice its hunk went to, and flushing to `file_slice` here
                # wrote the whole preamble into `source` for a Rust file whose
                # hunks were ALL test hunks — a phantom "file changed" entry
                # with no hunks after it, and a non-zero source count that a
                # caller reads as "spawn a source lens".
                flush_header(current)
                emit(current, line)

            # And the same flush for the LAST file in the diff.
            if header and not header_written:
                flush_header(file_slice)
    except OSError as exc:
        raise ReviewDiffError(f"cannot write diff slices: {exc}") from exc
    finally:
        for handle in handles.values():
            handle.close()

    return {
        name: {"path": str(paths[name]), "lines": counts[name]} for name in SLICE_NAMES
    }


def overlapping_prs(paths: list[str], limit: int = 30) -> dict:
    """Open PRs whose changed files intersect ``paths``.

    **The hazard this catches is in-flight overlap, not base drift.** The
    freshness gate compares HEAD against the base and is satisfied whenever the
    base has not moved — so a known-overlapping PR that lands *during* the
    fan-out passes every check and still invalidates the review. Measured: a
    five-lens pass (~1.23M input) was invalidated exactly that way, and the
    overlap had been written in the issue days earlier.

    **Reports, never blocks.** Waiting is sometimes right and sometimes not; the
    decision belongs to the caller, who can see the priorities. This returns the
    evidence.

    The per-PR file lists are collection-valued and large — a ``gh pr list``
    carrying ``files`` measured ~4.0k for a two-line answer across eleven PRs.
    They are fetched **into this process** and only the intersection is
    returned, which is the whole reason this lives in a tool.

    A missing or unauthenticated ``gh`` is not an error: overlap is advisory, so
    the failure is reported in ``error`` and the caller proceeds.
    """
    mine = set(paths)
    if not mine:
        return {"checked": False, "error": "no changed paths", "prs": []}
    try:
        completed = subprocess.run(
            [
                "gh",
                "pr",
                "list",
                "--state",
                "open",
                "--limit",
                str(limit),
                "--json",
                "number,title,headRefName,files",
            ],
            capture_output=True,
            text=True,
            errors="replace",
            check=False,
        )
    except OSError as exc:
        return {"checked": False, "error": f"gh unavailable: {exc}", "prs": []}
    if completed.returncode != 0:
        detail = (completed.stderr or "").strip() or f"exit {completed.returncode}"
        return {"checked": False, "error": detail, "prs": []}
    try:
        rows = json.loads(completed.stdout or "[]")
    except json.JSONDecodeError as exc:
        return {"checked": False, "error": f"decoding gh output: {exc}", "prs": []}

    try:
        current = _git(["rev-parse", "--abbrev-ref", "HEAD"]).strip()
    except ReviewDiffError:
        current = ""

    hits = []
    for row in rows:
        if row.get("headRefName") == current:
            continue  # this PR
        theirs = {f.get("path") for f in (row.get("files") or [])}
        shared = sorted(mine & theirs)
        if shared:
            hits.append(
                {
                    "number": row.get("number"),
                    "title": row.get("title"),
                    "branch": row.get("headRefName"),
                    "shared_files": shared,
                    "shared_count": len(shared),
                }
            )
    hits.sort(key=lambda h: h["shared_count"], reverse=True)
    return {"checked": True, "error": None, "prs": hits}


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


def gate(
    base: str,
    out: Path,
    fetch: bool = True,
    split: bool = False,
    only: list[str] | None = None,
    overlap: bool = False,
) -> dict:
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
    diff_lines = write_diff(base_ref, out, only=only)
    # `--only` narrows the INVENTORY as well as the diff. Letting git apply the
    # same limiters keeps one owner for the glob semantics — a second matcher
    # here would drift from the pathspec `write_diff` uses, and a single-segment
    # glob like `dir/*.py` is exactly where a hand-rolled one gets it wrong.
    #
    # Note this applies the positive limiters ONLY, never DIFF_EXCLUDES: the
    # inventory stays unfiltered with respect to the generated families.
    numstat = ["diff", "--numstat", "-z", f"{base_ref}..HEAD"]
    if only:
        numstat += ["--", *[f":(top,glob){p}" for p in only]]
    files = parse_numstat_z(_git(numstat))

    # ...but the GATE PREDICATES are computed from the UNLIMITED file list, and
    # that distinction is the whole point of this pair of calls. `--only` is a
    # statement about what to *review*, never about what the branch changed:
    # narrowing to `.claude/**` on a branch that also touched a generation input
    # would otherwise report `runs_artifact_gates: false` and step 9 would skip
    # a gate CI is about to run. One extra cheap git call, only when `--only` is
    # passed, buys a predicate that cannot go blind.
    if only:
        gate_paths = [
            f["path"]
            for f in parse_numstat_z(
                _git(["diff", "--numstat", "-z", f"{base_ref}..HEAD"])
            )
        ]
    else:
        gate_paths = [f["path"] for f in files]
    runs_rust_suites = touches_ci_code(gate_paths)
    runs_artifact_gates = touches_generation_input(gate_paths)

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
    if not files and only:
        # Checked BEFORE the bare no-files case: with `--only` the inventory is
        # limited too, so an empty one means the GLOB matched nothing — not that
        # the branch is empty. Reporting "no files changed between the base and
        # HEAD" here would name the wrong cause and send the reader to check the
        # branch instead of the pattern.
        blockers.append(
            f"--only {', '.join(only)} matched none of this branch's changed "
            f"paths — check the glob (it is anchored at the repo root, and `*` "
            f"does not cross a `/`)"
        )
    elif not files:
        blockers.append(
            f"no files changed between {base_ref} and HEAD — nothing to review "
            f"(check the base and the branch)"
        )
    elif diff_empty and only:
        # The limiters matched paths, but every one of them is also an excluded
        # generated family, so the diff came out empty.
        blockers.append(
            f"{out} is empty: --only {', '.join(only)} matched "
            f"{len(files)} path(s), but all of them are excluded generated "
            f"families ({', '.join(DIFF_EXCLUDES)})"
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

    crates = crate_rollup(files)

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
        "only": only or [],
        "files": files,
        # The tier decision's crate inputs. `code_crates` is the one to read:
        # it counts only trees with an actual source change, so a docs-only
        # second crate no longer escalates a diff to the full fan-out on
        # presence alone.
        "crates": crates,
        "code_crates": sum(1 for b in crates.values() if b["has_source"]),
        "runs_rust_suites": runs_rust_suites,
        "runs_artifact_gates": runs_artifact_gates,
        "ready": not blockers,
        "blockers": blockers,
    }
    if split:
        # Slices live beside the full diff, so one --out choice places everything.
        verdict["slices"] = split_diff(out, out.parent)
    if overlap:
        # Deliberately NOT a blocker: in-flight overlap is a decision for the
        # caller (wait, or proceed with a documented re-run cost), not a gate.
        # `gate_paths`, for the same reason the predicates use it: overlap with
        # another PR is a property of what this BRANCH changed, not of the slice
        # `--only` asked to review.
        verdict["overlapping_prs"] = overlapping_prs(gate_paths)
    return verdict


# The fields a freshness re-check actually consumes. Everything else in the
# verdict is inventory for the fan-out.
GATE_ONLY_FIELDS = (
    "base",
    "base_ref",
    "fetched",
    "fetch_error",
    "base_fresh",
    "base_ahead",
    "diff_empty",
    "ready",
    "blockers",
    "runs_rust_suites",
    "runs_artifact_gates",
)


def gate_only(verdict: dict) -> dict:
    """``verdict`` reduced to the fields that answer "may I proceed?".

    The mid-review freshness re-checks the skill prescribes read `base_fresh`,
    `ready` and `blockers` and nothing else — but the full verdict carries the
    whole `files` array, which on a 70-file branch is the bulk of the payload.
    One measured run printed all 70 entries on a re-check whose only consumed
    fields were those three, for a diff that had just been rebased away.

    The point is not the bytes on that one call: it is that a check nobody wants
    to pay for is a check that stops being run. Making it nearly free is what
    makes it actually happen, which is the real payoff.

    `base_ahead` and `fetch_error` come along because they are the *reasons*
    behind a verdict, and both are short (a commit list that is empty on the
    happy path, and a string that is normally absent). `commits` and `files` do
    not: they are unbounded in the branch's size.

    `runs_rust_suites` and `runs_artifact_gates` come along too, and the
    question they answer is why. "May I proceed?" has a second half — "and what
    may I skip?" — which is exactly what those two predicates decide at steps 9
    and 11. Dropping them would mean a re-check that reports `ready` but leaves
    the caller reaching for the full verdict to learn which suites the diff
    still forces, which defeats the purpose. They are two booleans, so unlike
    `commits` / `files` they cost nothing to carry.
    """
    return {k: verdict[k] for k in GATE_ONLY_FIELDS if k in verdict}


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
        "--only",
        action="append",
        default=None,
        metavar="GLOB",
        help="restrict the diff to these path globs; repeatable. Use it to "
        "subdivide an oversized slice instead of handing a lens the whole thing",
    )
    parser.add_argument(
        "--overlap",
        action="store_true",
        help="also report open PRs whose files intersect this diff — the "
        "in-flight hazard the base-freshness gate cannot see. Advisory, never a "
        "blocker",
    )
    parser.add_argument(
        "--gate-only",
        action="store_true",
        help="emit just the verdict fields (base freshness, ready, blockers) — "
        "for a mid-review re-check that consumes no inventory",
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

    verdict = gate(
        args.base,
        Path(args.out),
        fetch=not args.no_fetch,
        split=args.split,
        only=args.only,
        overlap=args.overlap,
    )
    # Narrow AFTER gating, never instead of it: the exit status below and the
    # blockers both have to reflect the full computation, so --gate-only is a
    # projection of the answer and not a cheaper way of reaching one.
    json.dump(gate_only(verdict) if args.gate_only else verdict, sys.stdout, indent=2)
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
