#!/usr/bin/env python3
"""Slice a file as it exists at another git ref, without buying the whole blob.

**Every conforming path fails on this shape.** The Grep tool reads only the
working tree; ``git grep`` is blocked by a committed guard with no escape hatch;
piping ``git show`` into ``grep`` is a forbidden compound; and a temp checkout
costs more than the read it avoids. That left exactly one conforming option — a
whole-file ``git show`` — measured at ~4.7k to learn a single ``Duration``
default, and the largest single result of that run.

It is not a rare shape either: the review flow's freshness gates make
cross-reference reads routine in any review that outlives a merge, and comparing
a doc or a constant against ``origin/main`` is ordinary work.

So this is the missing primitive. It reads the blob **inside this process** and
prints only the slice you asked for, reusing ``read_result.py``'s renderers so
the flags, the output shape, and the summary line are identical to the ones used
for a persisted tool result::

    # What does the section look like on main?
    python3 .claude/tools/show_at_ref.py origin/main docs/conventions/x.md \\
        --section 'The levers'

    # Where is the symbol, at the merge base?
    python3 .claude/tools/show_at_ref.py origin/main src/lib.rs --grep '^pub fn'

    # A known line range, and nothing else.
    python3 .claude/tools/show_at_ref.py HEAD~5 src/lib.rs --slice 40:80

    # Size first, when you do not yet know which slice you want.
    python3 .claude/tools/show_at_ref.py origin/main src/lib.rs --count

    # Spill it and slice repeatedly, when you need several regions.
    python3 .claude/tools/show_at_ref.py origin/main src/lib.rs --out /tmp/x.rs

A mode is **required**, deliberately: defaulting to "print it all" would rebuild
the whole-file ``git show`` this replaces.

Stdlib only, and deliberately **not** a Cargo workspace member — see
``docs/conventions/skill-tooling.md``. Tests live in
``tests/test_show_at_ref.py``, run via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys

import read_result

# Refuse a blob larger than this rather than holding it in memory. Far above any
# real source file; this is a runaway guard (a committed binary, a generated
# bundle), not a policy about what is worth reading.
MAX_BLOB_BYTES = 20_000_000


class ShowAtRefError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def read_blob(ref: str, path: str) -> str:
    """The text of ``path`` at ``ref``.

    ``ref`` and ``path`` are joined into git's ``<ref>:<path>`` form and passed
    as a single argv element with ``shell=False``, so neither is interpreted by a
    shell. A leading dash on either would still be read by git as an option, so
    both are refused up front — option injection rather than command injection,
    but refused all the same.
    """
    if ref.startswith("-"):
        raise ShowAtRefError(f"invalid ref: {ref!r}")
    if path.startswith("-"):
        raise ShowAtRefError(f"invalid path: {path!r}")
    try:
        completed = subprocess.run(
            ["git", "show", f"{ref}:{path}"],
            capture_output=True,
            check=False,
        )
    except OSError as exc:
        raise ShowAtRefError(f"cannot run git: {exc}") from exc
    if completed.returncode != 0:
        detail = (completed.stderr or b"").decode("utf-8", errors="replace").strip()
        raise ShowAtRefError(
            f"git show {ref}:{path} failed: {detail or 'exit %d' % completed.returncode}"
        )
    if len(completed.stdout) > MAX_BLOB_BYTES:
        raise ShowAtRefError(
            f"{ref}:{path} is {len(completed.stdout)} bytes, over the "
            f"{MAX_BLOB_BYTES}-byte cap — this tool reads source, not artifacts"
        )
    return completed.stdout.decode("utf-8", errors="replace")


def render(text: str, args: argparse.Namespace) -> tuple[list[str], str]:
    """``(lines_to_print, summary)`` for the chosen mode.

    Delegates to ``read_result``'s renderers rather than reimplementing them, so
    ``--section`` here and ``--section`` there behave identically — two spellings
    of one behavior is exactly how a prescribed command drifts from its tool.
    """
    lines = text.splitlines()
    label = f"{args.ref}:{args.path}"

    if args.count:
        chars = len(text)
        found = len(read_result.headings(lines))
        return (
            [f"{len(lines)} line(s)", f"{chars} character(s)", f"{found} heading(s)"],
            f"count of {label}",
        )
    if args.headings:
        found = read_result.headings(lines)
        return found, f"{len(found)} heading(s) of {len(lines)} line(s) in {label}"
    if args.section:
        out, start = read_result.section(lines, args.section)
        return out, f"section at line {start} of {label}"
    if args.grep:
        out = read_result.grep(lines, args.grep, args.context)
        return out, f"{len(out)} line(s) of {len(lines)} in {label}"
    # `--slice`
    start, end = read_result.parse_slice(args.slice, len(lines))
    numbered = [f"{n}:{lines[n - 1]}" for n in range(start, end + 1)]
    return numbered, f"lines {start}-{end} of {len(lines)} in {label}"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="show_at_ref.py",
        description="Slice a file at another git ref without buying the whole blob.",
    )
    parser.add_argument("ref", help="a git ref (origin/main, HEAD~3, a SHA)")
    parser.add_argument("path", help="repo-relative path at that ref")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--headings", action="store_true", help="markdown heading map")
    mode.add_argument("--section", metavar="RE", help="one heading block")
    mode.add_argument("--grep", metavar="RE", help="matching lines")
    mode.add_argument("--slice", metavar="A:B", help="inclusive line range")
    mode.add_argument("--count", action="store_true", help="size only")
    parser.add_argument(
        "--context", type=int, default=0, help="context lines for --grep (default 0)"
    )
    parser.add_argument(
        "--out",
        default=None,
        metavar="FILE",
        help="also spill the blob to FILE, for when several regions are wanted; "
        "slice it afterwards with read_result.py",
    )
    return parser


def run(argv: list[str]) -> int:
    args = build_parser().parse_args(argv[1:])
    text = read_blob(args.ref, args.path)

    if args.out:
        try:
            # 0o600 for the same reason the review diff is: whatever the file
            # held at that ref lands in a shared temp tree.
            fd = os.open(args.out, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(fd, "w", encoding="utf-8") as handle:
                handle.write(text)
        except OSError as exc:
            raise ShowAtRefError(f"cannot write {args.out}: {exc}") from exc
        print(
            f"show-at-ref | wrote {len(text)} chars to {args.out}",
            file=sys.stderr,
        )

    if not any((args.headings, args.section, args.grep, args.slice, args.count)):
        if args.out:
            # Spilling IS the whole job in this case: the caller asked for the
            # file on disk so they can slice it repeatedly.
            return 0
        raise ShowAtRefError(
            "pick a mode: --headings, --section, --grep, --slice, --count — or "
            "--out to spill it. There is deliberately no print-it-all default: "
            "that is the whole-file `git show` this tool exists to replace."
        )

    out, summary = render(text, args)
    for line in out:
        print(line)
    print(f"show-at-ref | {summary}", file=sys.stderr)
    return 0 if out else 1


def main() -> int:
    try:
        return run(sys.argv)
    except (ShowAtRefError, read_result.ReadResultError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
