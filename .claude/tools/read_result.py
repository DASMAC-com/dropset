#!/usr/bin/env python3
# cspell:word lineterm
# cspell:word tofile
"""Slice-reader over a persisted tool-result file — read a huge payload in *this*
process and emit only the part that answers the question.

When a tool result overflows the harness's inline cap it is spilled to a JSON
file under the session's ``tool-results/`` directory and only a 2KB preview
reaches the transcript. The payload is then stuck: ``Read`` on the file pulls the
whole thing into context, which is exactly what the spill avoided, so sessions
have twice now written a throwaway scratchpad script to grep it instead — a
>49KB Linear issue body mined with hand-rolled slices, then a 67.0k-character
inbox document mined the same way. This is that script, committed once, so the
shape reduces to a single allow-rule instead of being re-authored per session.

The envelope is unwrapped for you: a persisted file holds a JSON array of
``{"type": "text", "text": ...}`` blocks, and for an MCP result that inner text
is itself JSON. ``--field`` walks into it, so a Linear issue body is
``--field description`` rather than a hand-written ``json.loads`` chain.

Usage::

    # What sections does this 40-part issue have, and where?
    python3 .claude/tools/read_result.py <file> --field description --headings

    # Just one section, by heading pattern
    python3 .claude/tools/read_result.py <file> --field description \\
        --section 'Part 24'

    # Narrow searches and explicit line ranges
    python3 .claude/tools/read_result.py <file> --grep 'PAGE_SIZE' --context 2
    python3 .claude/tools/read_result.py <file> --slice 120:180

    # What changed between two fetches of the same object?
    python3 .claude/tools/read_result.py <new> --field description --diff <old>

Every mode prints to stdout and a one-line summary to stderr. Prefer the
narrowest mode that answers the question — ``--headings`` to navigate,
``--section`` or ``--slice`` to read, ``--grep`` to locate, and ``--count`` when
the question is merely *how many* (per ``docs/conventions/context-economy.md`` →
"The levers": match the search shape to the question type). Standard library
only. A Python skill-tool under ``.claude/tools/`` — deliberately **not** a Cargo
workspace member (see ``CLAUDE.md`` → "Skill tooling"). Tests live in
``tests/test_read_result.py``, run via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import difflib
import json
import re
import sys
from pathlib import Path

# ATX markdown headings only. A persisted payload is machine-written, so the
# setext form (an underline of ``=`` or ``-``) shows up mostly as an artifact of
# a body whose paragraph abuts a rule — which is a bug to find, not a heading to
# navigate by, so treating it as prose here is deliberate.
HEADING_RE = re.compile(r"^(#{1,6})\s+(.*)$")

# A fenced code block, opening or closing. Tracking fences is not optional here:
# these payloads are Linear issue bodies full of ```sh / ```json blocks whose
# lines routinely start with `#` (a shell comment). Read as depth-1 headings,
# those end a section early — so `--section` returned a SILENTLY TRUNCATED block
# and `--headings` listed phantom entries. Caught in review.
FENCE_RE = re.compile(r"^\s*(```+|~~~+)")


class ReadResultError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def unwrap(raw: str) -> str:
    """The text payload of a persisted tool-result file.

    Accepts the harness's block-array envelope, a bare JSON string, or plain
    text — so the tool is equally usable on a spilled result and on any other
    oversized file. Unparseable input is returned verbatim rather than raising:
    a truncated spill is still worth grepping.
    """
    try:
        doc = json.loads(raw)
    except (json.JSONDecodeError, ValueError):
        return raw

    if isinstance(doc, str):
        return doc
    if isinstance(doc, list):
        texts = [
            block["text"]
            for block in doc
            if isinstance(block, dict) and isinstance(block.get("text"), str)
        ]
        if texts:
            # Announce a dropped block rather than silently returning a subset:
            # an image block or a null `text` beside real text would otherwise
            # make a partial payload read as the whole one, which is the same
            # silent-truncation failure this tool exists to avoid.
            dropped = len(doc) - len(texts)
            if dropped:
                print(
                    f"read-result | NOTE: {dropped} non-text block(s) in the "
                    "envelope were not part of this payload",
                    file=sys.stderr,
                )
            return "\n".join(texts)
    return raw


def extract_field(text: str, path: str) -> str:
    """Walk a dotted ``path`` into ``text`` parsed as JSON.

    A list index is written as a bare integer (``attachments.0.url``). The value
    is returned as-is when it is a string and re-serialized when it is not, so
    ``--field labels`` prints readable JSON instead of a Python ``repr``.
    """
    try:
        node = json.loads(text)
    except (json.JSONDecodeError, ValueError) as e:
        raise ReadResultError(
            f"--field {path} needs a JSON payload, but the text is not JSON: {e}"
        ) from e

    for part in path.split("."):
        if isinstance(node, list):
            try:
                node = node[int(part)]
            except (ValueError, IndexError) as e:
                raise ReadResultError(
                    f"--field {path}: {part!r} is not a valid index into a "
                    f"{len(node)}-element list"
                ) from e
        elif isinstance(node, dict):
            if part not in node:
                available = ", ".join(sorted(node)[:12]) or "(none)"
                raise ReadResultError(
                    f"--field {path}: no key {part!r}; available: {available}"
                )
            node = node[part]
        else:
            raise ReadResultError(
                f"--field {path}: cannot descend into {type(node).__name__} at {part!r}"
            )

    return node if isinstance(node, str) else json.dumps(node, indent=2)


def iter_headings(lines: list[str]):
    """Yield ``(index, depth, text)`` for each ATX heading outside a code fence.

    One scanner, used by both :func:`headings` and :func:`section`, so the two
    cannot disagree about what counts as a heading.
    """
    in_fence = False
    for i, line in enumerate(lines):
        if FENCE_RE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        m = HEADING_RE.match(line)
        if m:
            yield i, len(m.group(1)), m.group(2).strip()
    if in_fence:
        # An unclosed fence swallows every heading after it, so `--headings`
        # would return a short list that reads as complete and `--section` would
        # report "no heading matches" for a heading that is really there. Say so:
        # a truncated spill is a documented input to this tool (see `unwrap`), so
        # this is reachable, and silent truncation is the failure it exists to
        # prevent.
        print(
            "read-result | WARNING: unclosed code fence — headings after it were "
            "not scanned; the payload may be truncated",
            file=sys.stderr,
        )


def headings(lines: list[str]) -> list[str]:
    """Every ATX heading as ``<line>:<indent><text>``, indented by depth.

    This is the navigation mode: it turns a 40-part body into a table of
    contents a few hundred bytes wide, which is what makes the follow-up
    ``--section`` read narrow instead of speculative.
    """
    return [
        f"{i + 1}:{'  ' * (depth - 1)}{text}" for i, depth, text in iter_headings(lines)
    ]


def section(lines: list[str], pattern: str) -> tuple[list[str], int]:
    """The heading block whose heading matches ``pattern``, and its start line.

    Runs to the next heading of the same or shallower depth, so asking for a
    part returns its sub-bullets and checklist with it. Matching is a
    case-insensitive search, which is why ``--section 'Part 24'`` is enough.
    """
    try:
        rx = re.compile(pattern, re.IGNORECASE)
    except re.error as e:
        raise ReadResultError(f"--section {pattern!r} is not a valid regex: {e}") from e

    found = [(i, d, text) for i, d, text in iter_headings(lines) if rx.search(text)]
    if not found:
        raise ReadResultError(
            f"no heading matches {pattern!r} — run --headings to see what is there"
        )
    if len(found) > 1:
        # Report rather than guess. `--section 'Part 2'` would otherwise return
        # `Part 24` if it came first, silently.
        where = ", ".join(f"line {i + 1} ({text})" for i, _, text in found)
        raise ReadResultError(
            f"{len(found)} headings match {pattern!r} ({where}) — narrow the "
            "pattern (anchor it, e.g. 'Part 2 —') so the section is unambiguous"
        )

    start, depth, _ = found[0]
    end = len(lines)
    for i, d, _ in iter_headings(lines):
        if i > start and d <= depth:
            end = i
            break
    return lines[start:end], start + 1


def grep(lines: list[str], pattern: str, context: int) -> list[str]:
    """Matching lines as ``<line>:<text>``, with ``--`` between context runs.

    ``context`` defaults to 0 on purpose: a persisted payload is exactly the
    place where a wide context window buys the file twice over, which is the
    density rule in ``context-economy.md``.
    """
    try:
        rx = re.compile(pattern)
    except re.error as e:
        raise ReadResultError(f"--grep {pattern!r} is not a valid regex: {e}") from e

    keep: set[int] = set()
    hits = 0
    for i, line in enumerate(lines):
        if rx.search(line):
            hits += 1
            lo = max(0, i - context)
            hi = min(len(lines), i + context + 1)
            keep.update(range(lo, hi))

    if not hits:
        return []

    out: list[str] = []
    previous = None
    for i in sorted(keep):
        if previous is not None and i != previous + 1:
            out.append("--")
        out.append(f"{i + 1}:{lines[i]}")
        previous = i
    return out


def parse_slice(spec: str, total: int) -> tuple[int, int]:
    """``START:END`` as a 1-indexed inclusive range clamped to the payload.

    Either side may be empty (``:40``, ``400:``). Clamping rather than raising
    means a range aimed past the end still returns the tail.
    """
    if ":" not in spec:
        raise ReadResultError(f"--slice {spec!r} must look like START:END")
    lo_s, hi_s = spec.split(":", 1)
    try:
        lo = int(lo_s) if lo_s.strip() else 1
        hi = int(hi_s) if hi_s.strip() else total
    except ValueError as e:
        raise ReadResultError(f"--slice {spec!r} needs integer bounds: {e}") from e
    # Negative bounds are refused rather than clamped: `1:-1` most likely means
    # "all but the last line" (a Python habit) and clamping it to an empty range
    # reported "start is past end", which is not what went wrong.
    if lo < 0 or hi < 0:
        raise ReadResultError(
            f"--slice {spec!r} has a negative bound; use 1-indexed line numbers "
            "(an open side is allowed: ':40', '400:')"
        )
    lo = max(1, lo)
    hi = min(total, hi)
    if lo > hi:
        raise ReadResultError(f"--slice {spec!r} is empty (start is past end)")
    return lo, hi


def diff(old: list[str], new: list[str], context: int, old_name: str) -> list[str]:
    """A unified diff of two payloads.

    The mode that answers "what changed since I last read this" — the case that
    prompted this tool, where a peer session reports an object was amended and
    re-reading it whole would cost the same as the first read.
    """
    return list(
        difflib.unified_diff(
            old, new, fromfile=old_name, tofile="(this file)", n=context, lineterm=""
        )
    )


def payload(path: Path, field: str | None) -> str:
    try:
        raw = path.read_text(encoding="utf-8")
    except OSError as e:
        raise ReadResultError(f"cannot read {path}: {e}") from e
    text = unwrap(raw)
    return extract_field(text, field) if field else text


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="read_result.py",
        description="Emit a slice of a persisted tool-result file.",
    )
    parser.add_argument("path", help="the persisted tool-result (or any text) file")
    parser.add_argument(
        "--field",
        default=None,
        metavar="DOTTED",
        help="dotted path into the payload parsed as JSON (e.g. description)",
    )
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--headings",
        action="store_true",
        help="list markdown headings with line numbers",
    )
    mode.add_argument("--section", default=None, metavar="RE", help="one heading block")
    mode.add_argument("--grep", default=None, metavar="RE", help="matching lines")
    mode.add_argument(
        "--slice", default=None, metavar="A:B", help="inclusive line range"
    )
    mode.add_argument(
        "--diff", default=None, metavar="FILE", help="unified diff against FILE"
    )
    mode.add_argument(
        "--count",
        action="store_true",
        help="print size only — lines, characters, headings",
    )
    parser.add_argument(
        "--context",
        type=int,
        default=0,
        help="context lines for --grep (default 0) and --diff (default 3)",
    )
    args = parser.parse_args(argv[1:])

    text = payload(Path(args.path), args.field)
    lines = text.splitlines()
    summary: str

    if args.count:
        out = [
            f"{len(lines)} line(s)",
            f"{len(text)} character(s)",
            f"{len(headings(lines))} heading(s)",
        ]
        summary = "count"
    elif args.headings:
        out = headings(lines)
        summary = f"{len(out)} heading(s) of {len(lines)} line(s)"
    elif args.section is not None:
        block, start = section(lines, args.section)
        out = block
        summary = f"section at line {start}, {len(block)} line(s)"
    elif args.grep is not None:
        out = grep(lines, args.grep, max(0, args.context))
        shown = sum(1 for line in out if line != "--")
        summary = f"{shown} line(s) of {len(lines)}"
    elif args.slice is not None:
        lo, hi = parse_slice(args.slice, len(lines))
        out = [f"{i}:{lines[i - 1]}" for i in range(lo, hi + 1)]
        summary = f"lines {lo}-{hi} of {len(lines)}"
    elif args.diff is not None:
        # --diff is the one mode where zero context is unhelpful: a bare changed
        # line gives no anchor for where in the body it landed.
        n = args.context if args.context else 3
        old = payload(Path(args.diff), args.field).splitlines()
        out = diff(old, lines, n, args.diff)
        summary = f"{len(out)} diff line(s)" if out else "identical"
    else:
        raise ReadResultError(
            "pick a mode: --headings, --section, --grep, --slice, --diff, or --count"
        )

    for line in out:
        print(line)
    print(f"read-result | {summary}", file=sys.stderr)
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except ReadResultError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
