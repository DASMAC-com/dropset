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
- the index says "None" while an inline marker remains;
- a code fence left open hides a marker.

**The index** is the body of a heading named ``Open items``, or failing that
the first list carrying a marker before the first section heading (a leading
title heading does not count as a section). **An entry matches** an inline
marker of the same kind when it references the marker's section or an
ancestor of it — by ``§N`` number or ``#anchor`` slug when the entry carries
one, else by the heading's title — or when one's text contains the other's.
Fenced code blocks are skipped, and a fence left open over a marker is a
failure. A marker in an inline code span still counts, since index entries are
often written that way: to mention the marker literally, write the bare
``NEEDS-`` prefix or use a fence. A file with no markers at all is clear.

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
from dataclasses import dataclass, field

MARKER = re.compile(r"NEEDS-(CONFIRM|FEEDBACK)")
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")
LIST_ITEM = re.compile(r"^\s*(?:[-*+]|\d+[.)])\s+")
FENCE = re.compile(r"^\s*(`{3,}|~{3,})")
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
    sections: list[Heading] = field(default_factory=list)


def normalize(text: str) -> str:
    return " ".join(re.sub(r"[^0-9a-z]+", " ", text.lower()).split())


def slug(title: str) -> str:
    """GitHub's heading anchor: lowercase, punctuation dropped, spaces dashed."""
    return re.sub(r"[^\w\- ]", "", title.lower()).replace(" ", "-")


def strip_marker(text: str) -> str:
    return normalize(MARKER.sub(" ", LIST_ITEM.sub("", text)))


def scan(lines: list[str]) -> tuple[list[Heading], list[int], int | None]:
    """Headings, the line numbers (1-based) outside fenced code, and the line
    of a fence left unclosed at end of file (which would hide every marker
    after it)."""
    headings: list[Heading] = []
    live: list[int] = []
    fence: tuple[int, str] | None = None
    for n, line in enumerate(lines, 1):
        match = FENCE.match(line)
        if fence is None and match:
            fence = (n, match.group(1))
            continue
        if fence is not None:
            # Only a run of the opening character at least as long closes it.
            if match and match.group(1)[0] == fence[1][0]:
                if len(match.group(1)) >= len(fence[1]):
                    fence = None
            continue
        live.append(n)
        match = HEADING.match(line)
        if match:
            headings.append(Heading(n, len(match.group(1)), match.group(2)))
    return headings, live, fence[0] if fence else None


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

    # No heading: the first marked list before the first section heading. The
    # first level-1 heading is the document's title, not a section.
    title = next((h for h in headings if h.level == 1), None)
    sections = [h for h in headings if h is not title]
    limit = sections[0].line if sections else len(lines) + 1
    start = None
    blank = False
    for n in live:
        if n >= limit:
            break
        line = lines[n - 1]
        if LIST_ITEM.match(line):
            if start is None:
                start = n
        elif (
            start is not None
            and line.strip()
            and (line.startswith("#") or (blank and not line[:1].isspace()))
        ):
            # A lazy (unindented) continuation stays in the list, as does an
            # indented one after a blank line; an unindented paragraph after a
            # blank line, or the title heading, ends it.
            block = range(start, n)
            if any(MARKER.search(lines[m - 1]) for m in block):
                return block
            start = None
        blank = not line.strip()
    if start is not None:
        block = range(start, limit)
        if any(MARKER.search(lines[m - 1]) for m in block):
            return block
    return None


def sections_of(line: int, headings: list[Heading]) -> list[Heading]:
    """The heading owning ``line`` and each ancestor, nearest first — so an
    entry citing ``§2`` covers a marker under ``### 2.1``."""
    chain: list[Heading] = []
    for heading in headings:
        if heading.line > line:
            break
        chain = [h for h in chain if h.level < heading.level] + [heading]
    return chain[::-1]


def references(entry: str, section: Heading) -> bool:
    """Whether an index entry's text points at this section.

    An entry carrying an explicit ``§N`` or ``(#anchor)`` reference matches by
    that alone: matching its words against every heading title would let one
    inline marker satisfy an entry meant for another section.
    """
    numbers = SECTION_REF.findall(entry)
    anchors = ANCHOR_REF.findall(entry)
    title = SECTION_NUMBER.sub("", section.title)
    if numbers or anchors:
        number = SECTION_NUMBER.match(section.title)
        return bool(number and number.group(1) in numbers) or bool(
            {slug(title), slug(section.title)} & set(anchors)
        )
    wanted = strip_marker(title)
    return bool(wanted) and f" {wanted} " in f" {strip_marker(entry)} "


def matches(entry: Occurrence, inline: Occurrence) -> bool:
    if entry.kind != inline.kind:
        return False
    if any(references(entry.text, section) for section in inline.sections):
        return True
    ours, theirs = strip_marker(entry.text), strip_marker(inline.text)
    short = min(len(ours), len(theirs))
    return short >= MIN_TEXT_MATCH and (ours in theirs or theirs in ours)


def check(text: str) -> tuple[int, list[str]]:
    """``(open_item_count, failures)`` for a spec's text."""
    lines = text.splitlines()
    headings, live, unclosed = scan(lines)
    index = find_index(lines, headings, live)
    in_index = set(index or ())
    # The title is every section's ancestor; matching an entry against it
    # would pair the entry with every marker in the file.
    title = next((h for h in headings if h.level == 1), None)
    sectioned = [h for h in headings if h is not title]

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
            inline.append(
                Occurrence(n, match.group(0), line, sections_of(n, sectioned))
            )

    failures: list[str] = []
    if unclosed is not None and any(MARKER.search(x) for x in lines[unclosed:]):
        # Everything after it was skipped, markers included.
        failures.append(
            f"line {unclosed}: code fence never closed — every marker after "
            "it is unchecked"
        )
    if not entries and not inline:
        return 0, failures
    if index is None:
        failures.append(
            f"{len(inline)} marker(s) but no open-items index — add an "
            "'Open items' heading, or a marked list before the first section"
        )
        return len(inline), failures
    # A "None" under one group of a grouped index is not the whole index
    # saying None.
    if says_none is not None and inline and not entries:
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
