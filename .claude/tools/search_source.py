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

Options: ``--ext`` (comma-separated extensions, no dot; default is the source set
below), ``--all-text`` (every extension, not just the source set), ``--dir``
(comma-separated roots to search under; default the repo root), ``--context N``
(lines of context each side), ``--files-only`` (just the matching paths),
``--fixed`` (treat the pattern as a literal, not a regex), ``--ignore-case``,
``--max N`` (cap on reported matches, default 200), ``--root`` (repo root,
default cwd).

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


def iter_files(roots: list[Path], extensions: tuple[str, ...] | None):
    """Yield searchable files under ``roots``, pruning excluded trees.

    ``extensions`` of ``None`` means every extension (``--all-text``); a symlink
    is skipped rather than followed, so a link into ``target/`` cannot smuggle a
    pruned tree back in.
    """
    skip_dirs = excluded_dir_names()
    skip_files = excluded_file_names()
    suffixes = {f".{e.lower().lstrip('.')}" for e in extensions} if extensions else None

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
            if suffixes is not None and entry.suffix.lower() not in suffixes:
                continue
            try:
                if entry.stat().st_size > MAX_FILE_BYTES:
                    continue
            except OSError:
                continue
            yield entry


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
    extensions: tuple[str, ...] | None = SOURCE_EXTENSIONS,
    context: int = 0,
    fixed: bool = False,
    ignore_case: bool = False,
    limit: int = DEFAULT_MAX,
) -> dict:
    """Search and return ``{matches, files, total, truncated}``.

    ``total`` counts every match found, ``matches`` holds at most ``limit`` of
    them, and ``truncated`` is the difference — reported so a capped result is
    never mistaken for a complete one.
    """
    matcher = build_matcher(pattern, fixed, ignore_case)

    if dirs:
        roots = []
        for name in dirs:
            candidate = (root / name).resolve()
            if not candidate.exists():
                raise SearchSourceError(f"no such directory: {name}")
            roots.append(candidate)
    else:
        roots = [root.resolve()]

    matches: list[dict] = []
    files: list[str] = []
    total = 0
    for path in iter_files(roots, extensions):
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
                try:
                    rel = str(path.relative_to(root.resolve()))
                except ValueError:
                    rel = str(path)
                entry = {"path": rel, "line": index + 1, "text": line}
                if context:
                    lo = max(0, index - context)
                    hi = min(len(lines), index + context + 1)
                    entry["context"] = lines[lo:hi]
                    entry["context_start"] = lo + 1
                matches.append(entry)
        if hit_in_file:
            try:
                files.append(str(path.relative_to(root.resolve())))
            except ValueError:
                files.append(str(path))

    files.sort()
    matches.sort(key=lambda m: (m["path"], m["line"]))
    return {
        "matches": matches,
        "files": files,
        "total": total,
        "truncated": max(0, total - len(matches)),
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
    print(summary, file=sys.stderr)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="search_source.py")
    parser.add_argument("pattern", help="regex (or literal with --fixed)")
    parser.add_argument("--root", default=".", help="repo root (default cwd)")
    parser.add_argument("--dir", default=None, help="comma-separated roots to search")
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

    if args.all_text:
        extensions = None
    elif args.ext:
        extensions = tuple(e for e in args.ext.split(",") if e.strip())
    else:
        extensions = SOURCE_EXTENSIONS

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
