#!/usr/bin/env python3
"""Search the repo's source, skipping generated and never-search trees.

One measured session made **53 bare** ``grep`` **calls** (the Grep tool was
unavailable that run), each re-spelling ``--include="*.rs" --include="*.ts"`` and
a directory list — 53 command shapes, so 53 chances to re-prompt for permission,
and no two quite alike. Separately, the *hoisted* repo-wide sweep that
``review-pr`` step 5 asks for was **unscoped**, and returned the whole regenerated
SDK surface (a 658-line generated instruction file) that no lens needed.

Both are the same missing thing: one search shape, with one owner for the exclude
list. This tool is that shape. It reduces to a single stable allow-rule prefix
(``Bash(python3 .claude/tools/search_source.py:*)``) no matter how the pattern and
filters vary, and it takes its exclusions from ``review_diff.py`` — the module
that already owns ``DIFF_EXCLUDES`` (the generated families) and
``SEARCH_EXCLUDE_DIRS`` (the trees ``grep -r`` would otherwise walk, since it
does not honor gitignore and ``target/`` alone is multi-GB).

Matching is done in Python rather than by shelling out to ``grep``, for three
reasons: no BSD-vs-GNU flag divergence to reason about, the exclude lists are used
directly instead of being translated into flags, and the output can be **capped
with the truncation stated out loud** rather than silently trimmed (per
``CLAUDE.md`` → "Context economy": a cap nobody is told about reads as
"searched everything").

**Prose is not in the default set.** Searching skill or convention text needs
``--ext md`` (or ``--all-text``); a bare sweep looks only at the source
extensions below, so a string that lives only in a ``.md`` comes back as a
confident ``0 match(es)``. That zero now says so on stderr rather than reading
as absence — one measured run took the bare zero at face value and fell back to
a hand-rolled ``grep``, which is exactly what this tool exists to replace.

Usage::

    python3 .claude/tools/search_source.py 'WARNING 1' --context 2
    python3 .claude/tools/search_source.py 'fn compute_fill' --ext rs --dir programs,sdk
    python3 .claude/tools/search_source.py 'TODO' --files-only
    python3 .claude/tools/search_source.py '^#' --glob docs/fx-survey.md
    python3 .claude/tools/search_source.py 'pnpm --dir' --ext md   # prose

Options: ``--ext`` (extensions, no dot; default is the source set below),
``--all-text`` (every extension, not just the source set), ``--dir`` (roots to
search under; default the repo root), ``--glob`` (path globs a file must match —
see below), ``--context N`` (lines of context each side), ``--files-only`` (just
the matching paths), ``--fixed`` (treat the pattern as a literal, not a regex),
``--ignore-case``, ``--max N`` (cap on reported matches, default 200),
``--root`` (repo root, default cwd).

``--ext``, ``--dir`` and ``--glob`` each take a comma-separated list **and**
accumulate across repetitions, so ``--ext ts,tsx`` and ``--ext ts --ext tsx``
are the same request. They were not always: argparse's default ``store`` kept
only the last occurrence, so the repeated spelling searched ``tsx`` alone and
reported a clean ``0 match(es)`` — a false negative indistinguishable from a
true one, on the tool ``review-pr`` leans on to prove repo-wide negatives.
Passing one of them *empty* is now refused rather than silently widening the
search back to the default.

``--dir`` and ``--glob`` narrow along different axes, and the gap between them
was measured: getting the section map of **three named docs** cost 3.0k because
``--dir docs --ext md`` was the narrowest scope available, so it returned all
~200 headings across 18 files — ~60 of them from one architecture spec nobody
wanted. ``--dir`` picks *subtrees*; ``--glob`` picks *files*::

    --glob docs/fx-survey.md,docs/indexer.md    # exactly these two
    --glob 'programs/**/state.rs'               # by shape, at any depth
    --glob '*.toml'                             # a bare pattern also
                                                # matches on basename

A ``--glob`` run searches **every** extension unless ``--ext`` / ``--all-text``
is passed explicitly. The default source-extension set is a heuristic for "I
don't know which files"; naming a file already answers that, and silently
dropping the file the caller named — because ``.md`` isn't in the source set —
is precisely the under-report this tool's truncation reporting exists to
prevent. The exclude lists still apply: a glob cannot reach into ``target/`` or
a generated family.

Prints plain ``path:line:text`` lines — the shape a reader already knows from
``grep -n`` — then a one-line summary on stderr. When the cap truncates, the
summary says how many matches were dropped.

Standard library only. A Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling"). Tests live in ``tests/test_search_source.py``, run via
``make tools-tests``.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

from review_diff import DIFF_EXCLUDES, SEARCH_EXCLUDE_DIRS

# Sentinel for "the caller expressed no preference", so `search` can apply the
# same default the CLI does. A plain `SOURCE_EXTENSIONS` default could not tell
# an explicit pass from an omission, which let the library and the CLI disagree
# about a `--glob`ed `.md` — the library silently dropped the named file.
DEFAULT_EXTENSIONS = object()

# The extensions "source" means by default. Deliberately excludes `.md` — a doc
# search is a different question, and mixing prose into a symbol sweep is what
# made the unscoped hoisted grep noisy. Pass `--ext md` or `--all-text` for docs.
#
# That exclusion is a design choice, but it is also a trap, so it is announced:
# a bare sweep for a string that lives only in prose returns a confident
# `0 match(es)` that means "not in the source set", not "not in the repo". One
# measured run read that as absence and fell back to a bare `grep` — the very
# thing this tool replaces. `print_result` now says so whenever a defaulted run
# comes back empty.
#
# `mjs` / `cjs` are here because their absence was a silent under-report of the
# same shape: a sweep for a symbol present six times in a `.mjs` config file
# returned zero hits, and ESM/CJS config files are ordinary source in this repo.
SOURCE_EXTENSIONS = (
    "rs",
    "ts",
    "tsx",
    "js",
    "jsx",
    "mjs",
    "cjs",
    "py",
    "toml",
    "json",
    "yml",
    "yaml",
    "sql",
    "sh",
)

# Default cap on reported matches. High enough that an ordinary symbol sweep is
# never truncated, low enough that a pattern matching half the repo can't dump it
# into context.
DEFAULT_MAX = 200

# A file this large is a generated or vendored blob whatever its extension says.
MAX_FILE_BYTES = 2_000_000

# A matched line longer than this is truncated. Real source lines sit well
# under it; what runs past are minified bundles and embedded data URIs, where
# the whole line can cost thousands of tokens to answer a yes/no question.
MAX_LINE_CHARS = 400


class SearchSourceError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def accumulate_flag(values: list[str] | None) -> tuple[str, ...] | None:
    """Flatten a repeatable comma-separated flag into one ordered tuple.

    ``--ext ts --ext tsx`` and ``--ext ts,tsx`` name the same request, so both
    spellings have to reach the search as ``("ts", "tsx")``. They did not:
    argparse's default ``store`` kept only the **last** occurrence, so the
    repeated form searched ``tsx`` alone and reported a clean
    ``0 match(es) in 0 file(s)`` with grep's non-zero exit — a false negative
    **indistinguishable from a true one**, on the very tool `CLAUDE.md`
    prescribes for proving a repo-wide negative (`review-pr`'s straggler and
    uniqueness sweeps exist to do exactly that, and would have shipped a
    dangling reference without a word of warning).

    Returns ``None`` only when the flag was never given, so a caller can still
    distinguish "unset" from "given, but empty" and refuse the latter loudly
    rather than silently widening the search.
    """
    if values is None:
        return None
    out: list[str] = []
    for value in values:
        for part in value.split(","):
            part = part.strip()
            # Dedupe, preserving first-seen order: `--ext ts --ext ts,tsx` is a
            # plausible thing to type and must not search `ts` twice.
            if part and part not in out:
                out.append(part)
    return tuple(out)


def excluded_dir_names() -> set[str]:
    """Directory **base names** never descended into.

    Both lists contribute. From ``DIFF_EXCLUDES`` the directory-shaped entries
    reduce to their last segment, so excluding ``sdk/ts/src/generated`` also
    covers its Rust sibling — wider than the diff exclude, and correct here,
    since a search wants no generated tree at all.
    """
    names = set(SEARCH_EXCLUDE_DIRS)
    for entry in DIFF_EXCLUDES:
        basename = entry.rsplit("/", 1)[-1]
        if "." not in basename:
            names.add(basename)
    return names


def excluded_file_names() -> set[str]:
    """File base names never searched — the file-shaped generated families."""
    names = set()
    for entry in DIFF_EXCLUDES:
        basename = entry.rsplit("/", 1)[-1]
        if "." in basename:
            names.add(basename)
    return names


def iter_files(
    roots: list[Path],
    extensions: tuple[str, ...] | None,
    oversized: list[Path] | None = None,
    globs: tuple[str, ...] | None = None,
    base: Path | None = None,
    stats: dict | None = None,
):
    """Yield searchable files under ``roots``, pruning excluded trees.

    ``extensions`` of ``None`` means every extension (``--all-text``); a symlink
    is skipped rather than followed, so a link into ``target/`` cannot smuggle a
    pruned tree back in.

    Files over :data:`MAX_FILE_BYTES` are skipped and appended to ``oversized``
    when a list is given, so the caller can *say* it skipped them rather than
    silently under-reporting.

    **Filter order is load-bearing**, which is why ``globs`` is applied here
    rather than by the caller. It runs *before* the extension and size checks so
    that:

    * ``oversized`` only ever names a file the glob actually selected — a
      ``--glob docs/*.md`` run must not report skipping some huge ``.wasm`` it
      never intended to read; and
    * ``stats["glob_hits"]`` counts files the **glob** matched, independently of
      how many survived ``extensions``. Without that split, "the glob matched
      nothing" and "``--ext`` filtered out everything the glob matched" are
      indistinguishable, and the caller blames a path typo for an extension
      mismatch.
    """
    skip_dirs = excluded_dir_names()
    skip_files = excluded_file_names()
    suffixes = {f".{e.lower().lstrip('.')}" for e in extensions} if extensions else None

    def relative(path: Path) -> str:
        if base is None:
            return path.name
        try:
            return str(path.relative_to(base))
        except ValueError:
            return str(path)

    stack = list(roots)
    while stack:
        current = stack.pop()
        try:
            entries = sorted(current.iterdir())
        except (OSError, PermissionError):
            continue
        for entry in entries:
            if entry.is_symlink():
                continue
            if entry.is_dir():
                if entry.name not in skip_dirs:
                    stack.append(entry)
                continue
            if not entry.is_file():
                continue
            if entry.name in skip_files:
                continue
            if globs is not None and not path_matches_globs(relative(entry), globs):
                continue
            if stats is not None:
                stats["glob_hits"] = stats.get("glob_hits", 0) + 1
            if suffixes is not None and entry.suffix.lower() not in suffixes:
                continue
            try:
                if entry.stat().st_size > MAX_FILE_BYTES:
                    if oversized is not None:
                        oversized.append(entry)
                    continue
            except OSError:
                continue
            yield entry


def path_matches_globs(rel_path: str, globs: tuple[str, ...]) -> bool:
    """Whether ``rel_path`` (repo-relative, ``/``-separated) matches any glob.

    Two conveniences, both because a caller reaches for ``--glob`` to *name*
    files rather than to write a precise pattern:

    * ``**`` is honored across separators — ``programs/**/state.rs`` matches at
      any depth — which plain :func:`fnmatch.fnmatchcase` does not do, since its
      ``*`` already spans ``/``. Translating the pattern ourselves keeps a
      single-star segment from silently behaving like a double-star one.
    * A pattern with **no** separator is also tried against the basename, so
      ``--glob '*.toml'`` finds them at any depth instead of only at the root.
    """
    for raw in globs:
        pattern = raw.strip().strip("/")
        if not pattern:
            continue
        if re.fullmatch(_glob_to_regex(pattern), rel_path):
            return True
        if "/" not in pattern and re.fullmatch(
            _glob_to_regex(pattern), rel_path.rsplit("/", 1)[-1]
        ):
            return True
    return False


def _glob_to_regex(pattern: str) -> str:
    """Translate a path glob to a regex where ``*`` stops at a separator.

    ``fnmatch`` maps ``*`` to ``.*``, which crosses ``/`` — so ``docs/*.md``
    would match ``docs/a/b.md``. Here ``*`` is ``[^/]*``, ``?`` is a single
    non-separator character, and ``**`` spans separators **only when it is a
    whole path segment** (``a/**/b``, a leading ``**/``, a trailing ``/**``).

    That last condition matters: a mid-segment ``docs/fx**.md`` is *not* a
    globstar, and treating it as one would make it match ``docs/fx/a/b.md`` —
    silently contradicting the "stops at a separator" promise above. Bash's
    ``globstar``, gitignore, and ``pathlib`` all degrade a non-boundary ``**``
    to a single ``*``, so this does too.

    Character classes (``[ab]``) and brace expansion (``{ts,tsx}``) are **not**
    supported — their characters are escaped literally. A pattern using them
    matches nothing, which the caller surfaces as "matched no files" rather
    than silently returning an empty result.
    """
    out: list[str] = []
    i = 0
    while i < len(pattern):
        char = pattern[i]
        if char == "*":
            if pattern[i : i + 2] == "**":
                at_segment_start = i == 0 or pattern[i - 1] == "/"
                after = pattern[i + 2 : i + 3]
                at_segment_end = after in ("", "/")
                if at_segment_start and at_segment_end:
                    # `a/**/b` should also match `a/b`, so swallow the trailing
                    # slash into the optional group rather than requiring an
                    # empty segment.
                    if after == "/":
                        out.append("(?:.*/)?")
                        i += 3
                    else:
                        out.append(".*")
                        i += 2
                    continue
                # Not a whole segment: degrade to a single `*`, consuming both
                # stars so the second isn't re-read as another wildcard.
                out.append("[^/]*")
                i += 2
                continue
            out.append("[^/]*")
        elif char == "?":
            out.append("[^/]")
        else:
            out.append(re.escape(char))
        i += 1
    return "".join(out)


def build_matcher(pattern: str, fixed: bool, ignore_case: bool):
    """Compile the pattern, or raise a readable error for a bad regex."""
    flags = re.IGNORECASE if ignore_case else 0
    text = re.escape(pattern) if fixed else pattern
    try:
        return re.compile(text, flags)
    except re.error as exc:
        raise SearchSourceError(
            f"invalid pattern {pattern!r}: {exc} (pass --fixed to search it literally)"
        ) from exc


def search(
    pattern: str,
    root: Path,
    dirs: list[str] | None = None,
    extensions: tuple[str, ...] | None = DEFAULT_EXTENSIONS,
    context: int = 0,
    fixed: bool = False,
    ignore_case: bool = False,
    limit: int = DEFAULT_MAX,
    globs: tuple[str, ...] | None = None,
) -> dict:
    """Search and return ``{matches, files, total, truncated}``.

    ``total`` counts every match found, ``matches`` holds at most ``limit`` of
    them, and ``truncated`` is the difference — reported so a capped result is
    never mistaken for a complete one.

    ``globs`` narrows to files whose repo-relative path matches one of them; it
    composes with ``dirs`` (a file must satisfy both) and is applied *after* the
    exclude lists, so it can never reach into a pruned tree.

    Left at :data:`DEFAULT_EXTENSIONS`, ``extensions`` resolves to the source set
    — except under ``globs``, where it resolves to *every* extension: a named
    glob has already answered "which files?", and applying the source-extension
    heuristic on top would silently drop a ``.md`` the caller asked for by name.
    Pass an explicit tuple (or ``None``) to override either way.
    """
    # Remember that the caller expressed no preference *before* resolving it, so
    # an empty result can distinguish "searched everywhere and found nothing"
    # from "never looked outside the source set".
    defaulted = extensions is DEFAULT_EXTENSIONS
    if extensions is DEFAULT_EXTENSIONS:
        extensions = None if globs else SOURCE_EXTENSIONS
    narrowed_by_default = defaulted and extensions is not None

    matcher = build_matcher(pattern, fixed, ignore_case)

    base = root.resolve()
    if not base.is_dir():
        # A wrong --root would otherwise be silent: `iter_files` swallows the
        # `iterdir` OSError, so the run prints "0 match(es)" and exits 1 — a
        # "searched everything, found nothing" of exactly the kind this tool's
        # truncation reporting exists to prevent.
        raise SearchSourceError(f"--root is not a directory: {root}")

    if dirs:
        roots = []
        for name in dirs:
            candidate = (root / name).resolve()
            if not candidate.exists():
                raise SearchSourceError(f"no such directory: {name}")
            # Containment. `Path("/repo") / "/etc"` is `/etc`, so without this an
            # absolute (or `../..`) --dir searches outside the tree and prints
            # matching *lines* from it. This tool reduces to one blanket
            # allow-rule, so a --dir that escaped the tree would turn that rule
            # into an unbounded host-filesystem read behind a single approval.
            if not candidate.is_relative_to(base):
                raise SearchSourceError(
                    f"--dir must stay under --root ({base}): {name} resolves to "
                    f"{candidate}"
                )
            roots.append(candidate)
        # Drop a root nested inside another, and any duplicate, so an overlapping
        # --dir can't walk the same tree twice and double-count every match.
        roots = [
            c
            for c in dict.fromkeys(roots)
            if not any(c != other and c.is_relative_to(other) for other in roots)
        ]
    else:
        roots = [base]

    def relative(path: Path) -> str:
        try:
            return str(path.relative_to(base))
        except ValueError:
            return str(path)

    matches: list[dict] = []
    files: list[str] = []
    total = 0
    scanned = 0
    oversized: list[Path] = []
    stats: dict = {"glob_hits": 0}
    for path in iter_files(roots, extensions, oversized, globs, base, stats):
        scanned += 1
        try:
            lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
        except OSError:
            continue
        hit_in_file = False
        for index, line in enumerate(lines):
            if not matcher.search(line):
                continue
            total += 1
            hit_in_file = True
            if len(matches) < limit:
                entry = {"path": relative(path), "line": index + 1, "text": line}
                if context:
                    lo = max(0, index - context)
                    hi = min(len(lines), index + context + 1)
                    entry["context"] = lines[lo:hi]
                    entry["context_start"] = lo + 1
                matches.append(entry)
        if hit_in_file:
            files.append(relative(path))

    files.sort()
    matches.sort(key=lambda m: (m["path"], m["line"]))

    # A *fourth* way a globbed run can search nothing, and the one that reads
    # most like a typo: the glob names a path that really is there, and the
    # exclude lists prune it. `--glob sdk/idl/dropset.json` is the live case —
    # a generated family, so it is answered "matched no files", which sends the
    # reader off to re-check a path sitting in plain sight. Resolving only
    # wildcard-free patterns keeps this to one `stat` apiece and no second walk.
    pruned: list[str] = []
    if globs and not stats["glob_hits"]:
        excluded_files = excluded_file_names()
        excluded_dirs = excluded_dir_names()
        for pattern in globs:
            if "*" in pattern or "?" in pattern:
                continue
            candidate = base / pattern
            if not candidate.exists():
                continue
            try:
                parts = candidate.resolve().relative_to(base).parts
            except ValueError:
                continue
            if candidate.name in excluded_files or any(
                part in excluded_dirs for part in parts
            ):
                pruned.append(pattern)

    return {
        "matches": matches,
        "files": files,
        "total": total,
        "truncated": max(0, total - len(matches)),
        # The size cap is the tool's *other* cap, and the same rule applies: a cap
        # nobody is told about reads as "searched everything".
        "skipped_oversized": sorted(relative(p) for p in oversized),
        # How many files the pattern was actually run against, and how many the
        # glob alone selected. Both are needed because three different failures
        # otherwise print an identical `0 match(es)`: the glob named nothing
        # (usually a typo'd path), the glob matched but `--ext` filtered every
        # hit out, or the files were searched and genuinely held no match. Only
        # the third is a real negative, and conflating them is exactly the
        # under-report this tool exists to avoid.
        "scanned": scanned,
        "glob_hits": stats["glob_hits"],
        "globbed": globs is not None,
        # Which of those named paths exist but are pruned as a generated family
        # or a never-search tree. Empty unless the glob selected nothing at all.
        "glob_pruned": sorted(pruned),
        # True when the run fell back to `SOURCE_EXTENSIONS` because the caller
        # named no extension. Only meaningful on an empty result, where it is the
        # difference between a real negative and an unasked question.
        "narrowed_by_default": narrowed_by_default,
    }


def clip(line: str) -> str:
    """A matched line, truncated past ``MAX_LINE_CHARS`` with the cut stated.

    A single line can be arbitrarily long — a minified bundle, or a generated
    SVG carrying a base64 data URI — and echoing one whole costs far more than
    the question was worth: one existence check under ``--all-text`` paid ~5.2k
    for exactly that. Truncating keeps the match visible (you can still see
    *that* it matched, and where) while bounding what it can cost.
    """
    if len(line) <= MAX_LINE_CHARS:
        return line
    return f"{line[:MAX_LINE_CHARS]}… [+{len(line) - MAX_LINE_CHARS} chars]"


def merge_context_blocks(matches: list[dict]) -> list[tuple[str, int, list[str]]]:
    """Collapse overlapping context windows into one block each.

    Several matches a few lines apart otherwise emit near-identical windows
    that re-quote the same source: one ``--context 12`` call against a file
    with five nearby matches returned five overlapping blocks, ~2.2k for
    roughly 400 tokens of distinct content. Adjacent or overlapping windows in
    the same file become a single block covering their union.

    Returns ``(path, start_line, lines)`` triples in input order.
    """
    blocks: list[tuple[str, int, list[str]]] = []
    for match in matches:
        path = match["path"]
        start = match["context_start"]
        lines = list(match["context"])
        end = start + len(lines) - 1
        if blocks:
            prev_path, prev_start, prev_lines = blocks[-1]
            prev_end = prev_start + len(prev_lines) - 1
            # `+ 1` so strictly adjacent windows merge too — a one-line gap
            # between two blocks costs a separator worth more than the line.
            if prev_path == path and start <= prev_end + 1:
                if end > prev_end:
                    prev_lines.extend(lines[prev_end - start + 1 :])
                continue
        blocks.append((path, start, lines))
    return blocks


def print_result(result: dict, files_only: bool, context: int) -> None:
    """Emit ``grep -n``-shaped lines on stdout and one summary line on stderr."""
    if files_only:
        for path in result["files"]:
            print(path)
    elif context:
        for path, start, lines in merge_context_blocks(result["matches"]):
            for offset, line in enumerate(lines):
                print(f"{path}:{start + offset}:{clip(line)}")
            print("--")
    else:
        for match in result["matches"]:
            print(f"{match['path']}:{match['line']}:{clip(match['text'])}")

    summary = (
        f"search-source | {result['total']} match(es) in {len(result['files'])} file(s)"
    )
    if result["truncated"]:
        # Say it out loud: a silent cap reads as "searched everything".
        summary += (
            f" | {result['truncated']} match(es) NOT shown (raise --max to see them)"
        )
    skipped = result.get("skipped_oversized") or []
    if skipped:
        # The size cap is the other silent-cap risk, so it is announced too.
        summary += (
            f" | {len(skipped)} file(s) skipped as oversized: {', '.join(skipped)}"
        )
    if result.get("narrowed_by_default"):
        # The third silent-cap risk. It fires on **every** default-narrowed
        # run, not just an empty one: a search that finds some matches while
        # silently skipping every .md file reads as a complete answer, and a
        # non-empty result is *more* misleading than an empty one, not less —
        # an empty result at least prompts a second look. This exact shape
        # produced a wrong "referenced by nothing" conclusion about a tool that
        # three .md files reference.
        if result["total"]:
            summary += (
                " | NOTE: source set only (no --ext/--all-text), so .md and "
                "other prose was NOT searched — these matches may be partial"
            )
        else:
            summary += (
                " | NOTE: searched the source set only (no --ext/--all-text "
                "given), so .md and other prose was not looked at — retry "
                "with --ext md"
            )
    if result.get("globbed") and not result.get("scanned"):
        # Distinguish the two ways a globbed run can search nothing. Blaming a
        # path typo for an extension mismatch sends the reader to the wrong fix.
        if not result.get("glob_hits"):
            pruned = result.get("glob_pruned") or []
            if pruned:
                # Naming the reason turns a dead end into a decision: the path
                # is there, it is excluded on purpose, and no spelling of
                # --glob will reach it.
                summary += (
                    f" | WARNING: --glob named {len(pruned)} path(s) that exist "
                    f"but are excluded as a generated family or never-search "
                    f"tree, so nothing was searched: {', '.join(pruned)}"
                )
            else:
                summary += (
                    " | WARNING: --glob matched no files, so nothing was searched"
                )
        else:
            summary += (
                f" | WARNING: --glob matched {result['glob_hits']} file(s), but "
                f"--ext excluded all of them — nothing was searched"
            )
    print(summary, file=sys.stderr)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="search_source.py")
    parser.add_argument("pattern", help="regex (or literal with --fixed)")
    parser.add_argument("--root", default=".", help="repo root (default cwd)")
    parser.add_argument(
        "--dir",
        action="append",
        default=None,
        help="roots to search; comma-separated, and repeatable — values "
        "accumulate rather than the last one winning",
    )
    parser.add_argument(
        "--glob",
        action="append",
        default=None,
        help="path globs a file must match; comma-separated, and repeatable — "
        "values accumulate. Searches every extension unless --ext/--all-text "
        "is given. Supports * ? and segment-wise **, but not [classes] or "
        "{braces}",
    )
    parser.add_argument(
        "--ext",
        action="append",
        default=None,
        help="extensions, no dot; comma-separated, and repeatable — values "
        "accumulate",
    )
    parser.add_argument(
        "--all-text",
        action="store_true",
        help="search every extension, not just the source set",
    )
    parser.add_argument("--context", type=int, default=0, help="context lines")
    parser.add_argument("--files-only", action="store_true", help="print paths only")
    parser.add_argument("--fixed", action="store_true", help="literal, not regex")
    parser.add_argument("--ignore-case", action="store_true")
    parser.add_argument(
        "--max",
        type=int,
        default=DEFAULT_MAX,
        help=f"cap on reported matches (default {DEFAULT_MAX})",
    )
    args = parser.parse_args(argv[1:])

    # Each of these three is repeatable, so flatten every occurrence into one
    # ordered tuple before anything reads it.
    exts = accumulate_flag(args.ext)
    globs = accumulate_flag(args.glob)
    dirs = accumulate_flag(args.dir)

    # Key on `is not None`, NOT on truthiness: `--glob ''` flattens to an empty
    # tuple, so a truthiness test would skip the guard and silently drop the
    # filter, sweeping the whole tree — the broad, noisy result `--glob` exists
    # to prevent, delivered without a word of warning. The same silent-widening
    # trap applies to an empty `--ext` (falls back to the default set) and an
    # empty `--dir` (searches every root), so all three are refused alike.
    if exts is not None and not exts:
        raise SearchSourceError("--ext was given no extensions")
    if globs is not None and not globs:
        raise SearchSourceError("--glob was given no patterns")
    if dirs is not None and not dirs:
        raise SearchSourceError("--dir was given no roots")

    if exts and args.all_text:
        raise SearchSourceError("--ext and --all-text are alternatives")

    if args.all_text:
        extensions = None
    elif exts:
        extensions = exts
    else:
        # Let `search` resolve it, so the CLI and the library agree on what an
        # unspecified extension set means under `--glob`.
        extensions = DEFAULT_EXTENSIONS

    result = search(
        args.pattern,
        Path(args.root),
        dirs=list(dirs) if dirs is not None else None,
        extensions=extensions,
        context=args.context,
        fixed=args.fixed,
        ignore_case=args.ignore_case,
        limit=args.max,
        globs=globs,
    )
    print_result(result, args.files_only, args.context)
    # 0 when something matched, 1 when nothing did — grep's convention, so a
    # caller can branch on it.
    return 0 if result["total"] else 1


def main() -> int:
    try:
        return run(sys.argv)
    except SearchSourceError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
