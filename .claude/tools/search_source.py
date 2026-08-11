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

Usage::

    python3 .claude/tools/search_source.py 'WARNING 1' --context 2
    python3 .claude/tools/search_source.py 'fn compute_fill' --ext rs --dir programs,sdk
    python3 .claude/tools/search_source.py 'TODO' --files-only
    python3 .claude/tools/search_source.py '^#' --glob docs/fx-survey.md

Options: ``--ext`` (comma-separated extensions, no dot; default is the source set
below), ``--all-text`` (every extension, not just the source set), ``--dir``
(comma-separated roots to search under; default the repo root), ``--glob``
(comma-separated path globs a file must match — see below), ``--context N``
(lines of context each side), ``--files-only`` (just the matching paths),
``--fixed`` (treat the pattern as a literal, not a regex), ``--ignore-case``,
``--max N`` (cap on reported matches, default 200), ``--root`` (repo root,
default cwd).

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
SOURCE_EXTENSIONS = (
    "rs",
    "ts",
    "tsx",
    "js",
    "jsx",
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


class SearchSourceError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


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
    if extensions is DEFAULT_EXTENSIONS:
        extensions = None if globs else SOURCE_EXTENSIONS

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
    }


def print_result(result: dict, files_only: bool, context: int) -> None:
    """Emit ``grep -n``-shaped lines on stdout and one summary line on stderr."""
    if files_only:
        for path in result["files"]:
            print(path)
    else:
        for match in result["matches"]:
            if context:
                start = match["context_start"]
                for offset, line in enumerate(match["context"]):
                    print(f"{match['path']}:{start + offset}:{line}")
                print("--")
            else:
                print(f"{match['path']}:{match['line']}:{match['text']}")

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
    if result.get("globbed") and not result.get("scanned"):
        # Distinguish the two ways a globbed run can search nothing. Blaming a
        # path typo for an extension mismatch sends the reader to the wrong fix.
        if not result.get("glob_hits"):
            summary += " | WARNING: --glob matched no files, so nothing was searched"
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
    parser.add_argument("--dir", default=None, help="comma-separated roots to search")
    parser.add_argument(
        "--glob",
        default=None,
        help="comma-separated path globs a file must match; searches every "
        "extension unless --ext/--all-text is given. Supports * ? and "
        "segment-wise **, but not [classes] or {braces}",
    )
    parser.add_argument(
        "--ext", default=None, help="comma-separated extensions, no dot"
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

    if args.ext and args.all_text:
        raise SearchSourceError("--ext and --all-text are alternatives")

    # Key on `is not None`, NOT on truthiness: `--glob ''` is falsy, so a
    # truthiness test would skip both this parse and the guard below, silently
    # dropping the filter and sweeping the whole tree — the broad, noisy result
    # `--glob` exists to prevent, delivered without a word of warning.
    globs = None
    if args.glob is not None:
        globs = tuple(g.strip() for g in args.glob.split(",") if g.strip())
        if not globs:
            raise SearchSourceError("--glob was given no patterns")

    if args.all_text:
        extensions = None
    elif args.ext:
        extensions = tuple(e for e in args.ext.split(",") if e.strip())
    else:
        # Let `search` resolve it, so the CLI and the library agree on what an
        # unspecified extension set means under `--glob`.
        extensions = DEFAULT_EXTENSIONS

    dirs = [d.strip() for d in args.dir.split(",") if d.strip()] if args.dir else None

    result = search(
        args.pattern,
        Path(args.root),
        dirs=dirs,
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
