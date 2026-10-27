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

- an index entry carries a marker with no matching inline occurrence of its
  own;
- an inline marker has no matching index entry of its own (pairing is one
  to one both ways: two entries cannot share a marker, nor two markers an
  entry);
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
CLOSE_FENCE = re.compile(r"^\s*(`{3,}|~{3,})\s*$")
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
            # Only a bare run of the opening character, at least as long,
            # closes it; a line with an info string does not.
            close = CLOSE_FENCE.match(line)
            if close and close.group(1)[0] == fence[1][0]:
                if len(close.group(1)) >= len(fence[1]):
                    fence = None
            continue
        live.append(n)
        match = HEADING.match(line)
        if match:
            headings.append(Heading(n, len(match.group(1)), match.group(2)))
    return headings, live, fence[0] if fence else None


def title_of(headings: list[Heading]) -> Heading | None:
    """The document's title: the first heading, when it is level 1 and the
    heading after it is deeper. A spec sectioned by level-1 headings has no
    title; a trailing ``# Appendix`` does not take it away."""
    if headings and headings[0].level == 1:
        if len(headings) == 1 or headings[1].level > 1:
            return headings[0]
    return None


def find_index(
    lines: list[str], headings: list[Heading], live: list[int]
) -> range | None:
    """The 1-based line range of the open-items index, or None."""
    title = title_of(headings)
    sections = [h for h in headings if h is not title]
    for i, heading in enumerate(headings):
        if INDEX_TITLE.match(SECTION_NUMBER.sub("", heading.title)):
            later = headings[i + 1 :]
            level = heading.level
            near_top = all(h is title for h in headings[:i])
            if level == 1 and near_top and later and all(h.level > 1 for h in later):
                # An `# Open items` atop `## 1.` sections would otherwise
                # swallow the whole document: end it at the section level. A
                # trailing one among `#` sections keeps its `##` groups.
                level = min(h.level for h in later)
            end = next((h.line for h in later if h.level <= level), len(lines) + 1)
            return range(heading.line + 1, end)

    # No heading: the first marked list before the first section heading.
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


def keys(section: Heading) -> set[str]:
    """The explicit references that name a section: its number, its slugs."""
    title = SECTION_NUMBER.sub("", section.title)
    found = {f"#{slug(title)}", f"#{slug(section.title)}"}
    number = SECTION_NUMBER.match(section.title)
    if number:
        found.add(number.group(1))
    return found


def explicit(entry: str) -> set[str]:
    return set(SECTION_REF.findall(entry)) | {
        f"#{a}" for a in ANCHOR_REF.findall(entry)
    }


def references(entry: str, section: Heading, known: set[str]) -> bool:
    """Whether an index entry's text points at this section.

    An entry whose explicit ``§N`` or ``(#anchor)`` reference names a real
    section (one in ``known``) matches by that alone: matching its words
    against every heading title would let it claim a marker meant for another
    section. A reference naming no section (``§1`` against unnumbered
    headings) falls back to the title words.
    """
    cited = explicit(entry) & known
    if cited:
        return bool(cited & keys(section))
    wanted = strip_marker(SECTION_NUMBER.sub("", section.title))
    return bool(wanted) and f" {wanted} " in f" {strip_marker(entry)} "


DIRECT, ANCESTOR = 2, 1


def matches(entry: Occurrence, inline: Occurrence, known: set[str]) -> int:
    """``DIRECT`` when the entry names the marker's own section or shares its
    text, ``ANCESTOR`` when it names only an enclosing section, else 0."""
    if entry.kind != inline.kind:
        return 0
    if explicit(entry.text) & known:
        # A real section reference binds: matching text elsewhere must not
        # hide a stale `§3`.
        for depth, section in enumerate(inline.sections):
            if references(entry.text, section, known):
                return ANCESTOR if depth else DIRECT
        return 0
    ours, theirs = strip_marker(entry.text), strip_marker(inline.text)
    short = min(len(ours), len(theirs))
    if short >= MIN_TEXT_MATCH and (ours in theirs or theirs in ours):
        return DIRECT
    for depth, section in enumerate(inline.sections):
        if references(entry.text, section, known):
            return ANCESTOR if depth else DIRECT
    return 0


def pair(direct: list[list[int]], full: list[list[int]]) -> dict[int, int]:
    """A maximum one-to-one pairing of entries to inline markers, as
    ``{entry: marker}`` — so two entries citing one section cannot both be
    satisfied by the single marker one of them owns. Direct matches pair
    first, so when a marker is short it is the entry citing only an enclosing
    section that goes unpaired. Augmenting paths; specs hold a handful of
    items, so the quadratic cost is nothing."""
    owner: dict[int, int] = {}

    def claim(i: int, edges: list[list[int]], seen: set[int]) -> bool:
        for j in edges[i]:
            if j in seen:
                continue
            seen.add(j)
            if j not in owner or claim(owner[j], edges, seen):
                owner[j] = i
                return True
        return False

    for edges in (direct, full):
        paired = set(owner.values())
        for i in range(len(edges)):
            if i not in paired:
                claim(i, edges, set())
    return {i: j for j, i in owner.items()}


def check(text: str) -> tuple[int, list[str]]:
    """``(open_item_count, failures)`` for a spec's text."""
    lines = text.splitlines()
    headings, live, unclosed = scan(lines)
    index = find_index(lines, headings, live)
    in_index = set(index or ())
    # The title is every section's ancestor; matching an entry against it
    # would pair the entry with every marker in the file.
    title = title_of(headings)
    sectioned = [h for h in headings if h is not title]
    known = set().union(*(keys(h) for h in sectioned))

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
    ranks = [[matches(entry, other, known) for other in inline] for entry in entries]
    direct = [[j for j, r in enumerate(row) if r == DIRECT] for row in ranks]
    candidates = [[j for j, r in enumerate(row) if r] for row in ranks]
    claimed = pair(direct, candidates)
    for i, entry in enumerate(entries):
        if i not in claimed:
            failures.append(
                f"line {entry.line}: index entry {entry.kind} has no inline "
                f"occurrence of its own: {entry.text.strip()}"
            )
    paired = set(claimed.values())
    for j, other in enumerate(inline):
        # One entry per marker: a second open item added to an indexed
        # section needs its own entry, or the index hides it.
        if j not in paired:
            failures.append(
                f"line {other.line}: inline {other.kind} has no index entry "
                f"of its own: "
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
