#!/usr/bin/env python3
"""Check that a spec's open-items index and its inline markers agree.

The architect process marks every open item in a spec file with
``NEEDS-CONFIRM`` or ``NEEDS-FEEDBACK`` **inline**, where the input is
wanted, and lists them in an index at the top. **Both halves are needed**:
the operator reads the file in an editor and jumps from marker to marker, so
an index alone cannot show them where. On 2026-10-26 a second draft carried
its markers in the index only, and the operator could not find where input was
wanted. The rule was prose until then; this makes it mechanical::

    python3 .claude/tools/spec_markers.py 2217-curator.md

Exits 1, naming each failure, when:

- an index entry carries a marker with no matching inline occurrence;
- an inline marker has no matching index entry;
- the file carries markers but no index;
- the index says "None" while an inline marker remains.

**The index** is the body of a heading named ``Open items``, or failing that
the first list carrying a marker before the first section heading (a leading
title heading does not count as a section). **An entry matches** an inline
marker of the same kind when it references the marker's section — the
heading's title, its ``#anchor`` slug, or its ``§N`` number — or when one's
text contains the other's. Fenced code blocks are skipped. A file with no
markers at all is clear.

Exit 0 prints one line, the open-item count; ``--quiet`` suppresses it. Exit 2
is an unreadable file.

Stdlib only, and deliberately **not** a Cargo workspace member — see
``docs/conventions/skill-tooling.md``. Tests live in
``tests/test_spec_markers.py``, run via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass

MARKER = re.compile(r"NEEDS-(CONFIRM|FEEDBACK)")
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")
LIST_ITEM = re.compile(r"^\s*(?:[-*+]|\d+[.)])\s+")
FENCE = re.compile(r"^\s*(```|~~~)")
INDEX_TITLE = re.compile(r"^open items\b", re.IGNORECASE)
SECTION_NUMBER = re.compile(r"^§?(\d+(?:\.\d+)*)\.?\s+")
SECTION_REF = re.compile(r"(?:§|\bsection\s+)(\d+(?:\.\d+)*)", re.IGNORECASE)
ANCHOR_REF = re.compile(r"\(#([^)\s]+)\)")
NONE_ENTRY = re.compile(r"^\W*none\W*$", re.IGNORECASE)

# Below this, a text containment match is too likely to be accidental.
MIN_TEXT_MATCH = 12


@dataclass
class Heading:
    line: int
    level: int
    title: str


@dataclass
class Occurrence:
    """One marker: an index entry, or an inline line."""

    line: int
    kind: str
    text: str
    section: Heading | None = None


def normalize(text: str) -> str:
    return " ".join(re.sub(r"[^0-9a-z]+", " ", text.lower()).split())


def slug(title: str) -> str:
    """GitHub's heading anchor: lowercase, punctuation dropped, spaces dashed."""
    return re.sub(r"[^\w\- ]", "", title.lower()).replace(" ", "-")


def strip_marker(text: str) -> str:
    return normalize(MARKER.sub(" ", LIST_ITEM.sub("", text)))


def scan(lines: list[str]) -> tuple[list[Heading], list[int]]:
    """Headings, and the line numbers (1-based) outside fenced code."""
    headings: list[Heading] = []
    live: list[int] = []
    fenced = False
    for n, line in enumerate(lines, 1):
        if FENCE.match(line):
            fenced = not fenced
            continue
        if fenced:
            continue
        live.append(n)
        match = HEADING.match(line)
        if match:
            headings.append(Heading(n, len(match.group(1)), match.group(2)))
    return headings, live


def find_index(
    lines: list[str], headings: list[Heading], live: list[int]
) -> range | None:
    """The 1-based line range of the open-items index, or None."""
    for i, heading in enumerate(headings):
        if INDEX_TITLE.match(SECTION_NUMBER.sub("", heading.title)):
            end = len(lines) + 1
            for later in headings[i + 1 :]:
                if later.level <= heading.level:
                    end = later.line
                    break
            return range(heading.line + 1, end)

    # No heading: the first marked list before the first section heading. A
    # heading on the first non-blank line is the document's title, not a
    # section.
    first = next((n for n in live if lines[n - 1].strip()), None)
    sections = [h for h in headings if h.line != first]
    limit = sections[0].line if sections else len(lines) + 1
    start = None
    for n in live:
        if n >= limit:
            break
        line = lines[n - 1]
        if LIST_ITEM.match(line):
            if start is None:
                start = n
        elif start is not None and line.strip() and not line[:1].isspace():
            block = range(start, n)
            if any(MARKER.search(lines[m - 1]) for m in block):
                return block
            start = None
    if start is not None:
        block = range(start, limit)
        if any(MARKER.search(lines[m - 1]) for m in block):
            return block
    return None


def section_of(line: int, headings: list[Heading]) -> Heading | None:
    owner = None
    for heading in headings:
        if heading.line > line:
            break
        owner = heading
    return owner


def references(entry: str, section: Heading) -> bool:
    """Whether an index entry's text points at this section."""
    number = SECTION_NUMBER.match(section.title)
    if number and number.group(1) in SECTION_REF.findall(entry):
        return True
    title = SECTION_NUMBER.sub("", section.title)
    if slug(title) in ANCHOR_REF.findall(entry) or slug(
        section.title
    ) in ANCHOR_REF.findall(entry):
        return True
    wanted = strip_marker(title)
    return bool(wanted) and f" {wanted} " in f" {strip_marker(entry)} "


def matches(entry: Occurrence, inline: Occurrence) -> bool:
    if entry.kind != inline.kind:
        return False
    if inline.section is not None and references(entry.text, inline.section):
        return True
    ours, theirs = strip_marker(entry.text), strip_marker(inline.text)
    short = min(len(ours), len(theirs))
    return short >= MIN_TEXT_MATCH and (ours in theirs or theirs in ours)


def check(text: str) -> tuple[int, list[str]]:
    """``(open_item_count, failures)`` for a spec's text."""
    lines = text.splitlines()
    headings, live = scan(lines)
    index = find_index(lines, headings, live)
    in_index = set(index or ())

    entries: list[Occurrence] = []
    inline: list[Occurrence] = []
    says_none = None
    for n in live:
        line = lines[n - 1]
        if n in in_index:
            if says_none is None and NONE_ENTRY.match(LIST_ITEM.sub("", line)):
                says_none = n
            for match in MARKER.finditer(line):
                entries.append(Occurrence(n, match.group(0), line))
            continue
        for match in MARKER.finditer(line):
            inline.append(Occurrence(n, match.group(0), line, section_of(n, headings)))

    failures: list[str] = []
    if not entries and not inline:
        return 0, failures
    if index is None:
        failures.append(
            f"{len(inline)} marker(s) but no open-items index — add an "
            "'Open items' heading, or a marked list before the first section"
        )
        return len(inline), failures
    if says_none is not None and inline:
        failures.append(
            f"line {says_none}: the index says None while "
            f"{len(inline)} inline marker(s) remain"
        )
    for entry in entries:
        if not any(matches(entry, other) for other in inline):
            failures.append(
                f"line {entry.line}: index entry {entry.kind} has no inline "
                f"occurrence: {entry.text.strip()}"
            )
    for other in inline:
        if not any(matches(entry, other) for entry in entries):
            failures.append(
                f"line {other.line}: inline {other.kind} has no index entry: "
                f"{other.text.strip()}"
            )
    return len(entries), failures


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="spec_markers.py",
        description="Check a spec's open-items index against its inline markers.",
    )
    parser.add_argument("file", help="the spec or findings file")
    parser.add_argument(
        "--quiet", action="store_true", help="print nothing when the file is clear"
    )
    return parser


def run(argv: list[str]) -> int:
    args = build_parser().parse_args(argv[1:])
    try:
        with open(args.file, encoding="utf-8") as handle:
            text = handle.read()
    except OSError as exc:
        print(f"error: cannot read {args.file}: {exc}", file=sys.stderr)
        return 2
    count, failures = check(text)
    for failure in failures:
        print(f"spec-markers | {args.file}: {failure}")
    if failures:
        return 1
    if not args.quiet:
        print(f"spec-markers | {count} open item(s); index and inline agree")
    return 0


if __name__ == "__main__":
    sys.exit(run(sys.argv))
