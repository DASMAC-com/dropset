#!/usr/bin/env python3
"""Rewrap one paragraph, list item or ``#`` comment block to the line cap.

An ``Edit`` that lengthens a line in a wrapped paragraph pushes the overflow
onto the next line, and neither markdownlint nor mdformat rewraps, so each
overflow costs a lint round; an ``old_string`` ending mid-line glues the next
line on and costs another. Five sessions in one digest paid two to four turns
each for it (`docs/conventions/docs-and-style.md` → "Line length"). This
makes the rewrap one deterministic call:

    python3 .claude/tools/rewrap.py docs/conventions/x.md --line 120
    python3 .claude/tools/rewrap.py .github/workflows/lint.yml --anchor 'joined'

The block is the one holding ``--line N`` or the single line containing the
``--anchor`` text, and the cap is the one the file's linter applies:

* **Markdown** (``.md``): 80, markdownlint ``MD013`` (strict, the default
  width). The block is the paragraph or list item around the line, bounded by
  a blank line, a heading, a fence, a table row, an HTML line or the next list
  item. Block-quote and list-marker prefixes are kept, continuation lines
  indented under the marker; a change of quote depth ends the block. A line
  inside a fence or the front matter is refused, as is a block carrying a
  hard line break, a setext heading, a table, an indented code block, or a
  backtick no span closes.
* **Everything else**: the ``#`` comment run around the line, its indent and
  ``# `` kept, split into paragraphs at a bare ``#`` and at a ``-`` / ``*``
  bullet as markdown is. A tool directive (``# cspell:word x``) and an
  indented example line are never joined into the prose. YAML (yamllint
  ``line-length``), a Makefile, a Dockerfile and TOML take 80; any other file
  needs ``--width``.

Words are re-flowed greedily; a code span (`` `a b` ``, or ``` `` a ` b `` ```)
is never broken, even one that crossed a line, and a wrapped line never
starts with a token that would read as a list marker, heading, quote, rule,
fence or HTML. A token longer than the room left sits on its own line. A
CRLF file is refused rather than rewritten with LF endings.

The file is rewritten in place (``--dry-run`` writes nothing) and the output
is only the changed range: one summary line, then the new lines as
``path:N:text``. Exit 0 rewrapped or already wrapped, 2 on a refusal.
Standard library only; tests in ``tests/test_rewrap.py``.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

#: The cap each linter applies, by suffix or basename.
MARKDOWN_WIDTH = 80
COMMENT_WIDTHS = {".yml": 80, ".yaml": 80, ".mk": 80, ".toml": 80}
COMMENT_FILENAMES = {"Makefile": 80, "Dockerfile": 80}

#: A fence line: the run, then the rest (an info string on an opener).
_FENCE = re.compile(r"^\s*(?:>\s?)*\s*(`{3,}|~{3,})(.*)$")
_QUOTE = re.compile(r"^\s*(?:>\s?)+")
_MARKER = re.compile(r"^(\s*)([-*+]|\d{1,9}[.)])( +)(?=\S)")
#: A line that is its own block: a heading, a table row, HTML, a rule, a
#: setext underline.
_STANDALONE = re.compile(r"^\s*(?:#{1,6}(?:\s|$)|\||<|(?:[-*_]\s*){3,}$|=+\s*$|-+\s*$)")
#: A table's delimiter row, leading pipe or not (`--- | ---`).
_TABLE_DELIMITER = re.compile(r"^\s*\|?\s*:?-+:?\s*(?:\|\s*:?-+:?\s*)+\|?\s*$")
#: A token that, starting a line, would change what the line is: a heading,
#: a list marker, a quote, a setext underline or rule, a fence, a table, HTML.
#: Any run of dashes is a setext underline, and any `>` opens a quote.
_RISKY_START = re.compile(
    r"^(?:#{1,6}$|[*+]$|-+$|>|\d{1,9}[.)]$|=+$|[*_]{3,}$|`{3,}|~{3,}|\||<)"
)
#: A word: escapes, code spans (a backtick run closed by a run of the same
#: length) and other non-space characters, glued. A backtick no span closes
#: is matched alone, and refused.
_TOKEN = re.compile(r"(?:\\.|(`+)(?:(?!\1)[\s\S])*?(?<!`)\1(?!`)|[^\s`\\])+|`+|\\")
#: A comment line that is a tool's directive, never prose to join. Each form
#: carries the directive's own punctuation, so a prose line that merely
#: starts with `check` or `ruff` is not one.
_DIRECTIVE = re.compile(
    r"^(?:cspell:|yamllint (?:disable|enable)|shellcheck (?:disable|source)="
    r"|hadolint ignore=|(?:syntax|escape|check)=|(?:type|fmt|pylint|ruff|taplo):"
    r"|noqa\b|(?:prettier|biome|eslint|markdownlint)-)"
)


class RewrapError(Exception):
    """A refusal, printed as ``rewrap: <reason>`` with exit 2."""


def _opaque_lines(lines: list[str]) -> set[int]:
    """Indexes inside the front matter or a fenced code block (fences
    included), which are never rewrapped. CommonMark's fence rules: a
    backtick opener's info string holds no backtick, and a closer is the
    same character, at least as long, with nothing after it."""
    opaque: set[int] = set()
    start = 0
    if lines and lines[0].strip() == "---":
        for i in range(1, len(lines)):
            if lines[i].strip() in ("---", "..."):
                opaque.update(range(0, i + 1))
                start = i + 1
                break
    fence = None
    for i in range(start, len(lines)):
        match = _FENCE.match(lines[i])
        if fence is None:
            if match and not (match.group(1)[0] == "`" and "`" in match.group(2)):
                fence = match.group(1)
                opaque.add(i)
        else:
            opaque.add(i)
            if (
                match
                and match.group(1)[0] == fence[0]
                and len(match.group(1)) >= len(fence)
                and not match.group(2).strip()
            ):
                fence = None
    return opaque


def _depth(text: str) -> int:
    """The line's block-quote depth."""
    quote = _QUOTE.match(text)
    return quote.group(0).count(">") if quote else 0


def _is_boundary(text: str) -> bool:
    body = _QUOTE.sub("", text)
    return not body.strip() or bool(_STANDALONE.match(body))


def _starts_item(text: str) -> bool:
    return bool(_MARKER.match(_QUOTE.sub("", text)))


def find_block(
    lines: list[str], index: int, opaque: set[int], markdown: bool = True
) -> tuple[int, int]:
    """The ``[first, last]`` line indexes of the paragraph or item at ``index``.

    In real ``markdown`` a block under a setext underline or beside a table
    delimiter row is refused; in comment text such a line is only a banner.
    """
    if index in opaque:
        raise RewrapError(f"line {index + 1} is inside a fence or the front matter")
    if _is_boundary(lines[index]):
        raise RewrapError(
            f"line {index + 1} is blank, a heading, a table row or HTML — "
            "not a paragraph"
        )
    depth = _depth(lines[index])
    first = index
    while (
        not _starts_item(lines[first])
        and first > 0
        and first - 1 not in opaque
        and not _is_boundary(lines[first - 1])
        and _depth(lines[first - 1]) == depth
    ):
        first -= 1
    last = index
    while (
        last + 1 < len(lines)
        and last + 1 not in opaque
        and not _is_boundary(lines[last + 1])
        and not _starts_item(lines[last + 1])
        and _depth(lines[last + 1]) == depth
    ):
        last += 1
    if not markdown:
        return first, last
    if last + 1 < len(lines) and re.match(r"^\s*(?:=+|-+)\s*$", lines[last + 1]):
        raise RewrapError(f"lines {first + 1}-{last + 1} are a setext heading")
    if any(_TABLE_DELIMITER.match(line) for line in lines[first : last + 2]):
        raise RewrapError(f"lines {first + 1}-{last + 1} are part of a table")
    return first, last


def _prefixes(block: list[str]) -> tuple[str, str]:
    """The first line's prefix (quote, indent, marker) and the continuation's."""
    head = block[0]
    quote_match = _QUOTE.match(head)
    quote = quote_match.group(0) if quote_match else ""
    rest = head[len(quote) :]
    marker = _MARKER.match(rest)
    if marker:
        first = quote + marker.group(0)
        return first, quote + " " * len(marker.group(0))
    indent = rest[: len(rest) - len(rest.lstrip())]
    return quote + indent, quote + indent


def _strip(line: str) -> str:
    """A line's words, its quote, indent and (first-line) marker removed."""
    body = _QUOTE.sub("", line)
    marker = _MARKER.match(body)
    return (body[marker.end() :] if marker else body).strip()


def fill(words: list[str], first: str, rest: str, width: int) -> list[str]:
    """Greedy fill. A risky line-start token pulls the previous word down
    with it; with no word to spare, it stays on the line, over the cap."""
    out: list[list[str]] = [[]]
    for word in words:
        line = out[-1]
        prefix = first if len(out) == 1 else rest
        used = len(prefix) + len(" ".join(line))
        if line and used + 1 + len(word) > width:
            if _RISKY_START.match(word):
                if len(line) == 1:
                    line.append(word)
                    continue
                out.append([line.pop(), word])
            else:
                out.append([word])
        else:
            line.append(word)
    return [(first if n == 0 else rest) + " ".join(ws) for n, ws in enumerate(out)]


def rewrap_markdown(
    lines: list[str], index: int, width: int, markdown: bool = True
) -> tuple[int, int, list[str]]:
    opaque = _opaque_lines(lines)
    first, last = find_block(lines, index, opaque, markdown)
    block = lines[first : last + 1]
    body = _QUOTE.sub("", block[0])
    if (
        markdown
        and len(body) - len(body.lstrip()) >= 4
        and not _starts_item(block[0])
        and (first == 0 or not lines[first - 1].strip())
    ):
        raise RewrapError(
            f"lines {first + 1}-{last + 1} are an indented code block (or a "
            "list item's later paragraph); rewrap it by hand"
        )
    for n, line in enumerate(block[:-1]):
        if line.endswith("  ") or line.endswith("\\"):
            raise RewrapError(
                f"line {first + n + 1} ends in a hard line break; rewrap it by hand"
            )
    first_prefix, rest_prefix = _prefixes(block)
    # Tokenized as one text, so a code span may cross a line break.
    text = " ".join(_strip(line) for line in block)
    words = [m.group(0) for m in _TOKEN.finditer(text)]
    loose = [w for w in words if w.strip("`") == "" or w == "\\"]
    if loose:
        raise RewrapError(
            f"lines {first + 1}-{last + 1} hold a backtick or backslash no span "
            "closes; rewrap it by hand"
        )
    return first, last, fill(words, first_prefix, rest_prefix, width)


_COMMENT = re.compile(r"^(\s*)#(?: |$)")


def rewrap_comment(
    lines: list[str], index: int, width: int
) -> tuple[int, int, list[str]]:
    match = _COMMENT.match(lines[index])
    if not match:
        raise RewrapError(f"line {index + 1} is not a `#` comment line")
    indent = match.group(1)
    prefix = f"{indent}#"

    def in_run(i: int) -> bool:
        m = _COMMENT.match(lines[i])
        return bool(m) and m.group(1) == indent

    top, bottom = index, index
    while top > 0 and in_run(top - 1):
        top -= 1
    while bottom + 1 < len(lines) and in_run(bottom + 1):
        bottom += 1
    # The comment text, as markdown: a bare `#` is a blank line. A directive
    # (`# cspell:word x`) and an indented example line (`#   make x`) are
    # blanked too, so neither joins the prose — except an indent that is a
    # list item's own continuation.
    texts = [lines[i][len(prefix) + 1 :] for i in range(top, bottom + 1)]
    scan, item_indent = list(texts), None
    for n, text in enumerate(texts):
        marker = _MARKER.match(text)
        lead = len(text) - len(text.lstrip())
        if marker:
            item_indent = len(marker.group(0))
        elif not text.strip():
            item_indent = None
        if _DIRECTIVE.match(text) or (not marker and lead >= 2 and lead != item_indent):
            scan[n] = ""
    if not scan[index - top]:
        raise RewrapError(
            f"line {index + 1} is a directive or an indented example, not prose"
        )
    first, last, new = rewrap_markdown(
        scan, index - top, width - len(prefix) - 1, markdown=False
    )
    return top + first, top + last, [f"{prefix} {line}".rstrip() for line in new]


def width_for(path: Path, override: int | None) -> tuple[str, int]:
    """The block kind and the cap the file's linter applies."""
    if path.suffix in (".md", ".markdown"):
        return "markdown", override or MARKDOWN_WIDTH
    known = COMMENT_WIDTHS.get(path.suffix) or COMMENT_FILENAMES.get(path.name)
    if override or known:
        return "comment", override or known
    raise RewrapError(f"no known line cap for {path.name}; pass --width")


def locate(lines: list[str], line: int | None, anchor: str | None) -> int:
    if anchor is None:
        if not 1 <= line <= len(lines):
            raise RewrapError(f"--line {line} is outside the file (1-{len(lines)})")
        return line - 1
    hits = [i for i, text in enumerate(lines) if anchor in text]
    if len(hits) != 1:
        shown = ", ".join(str(i + 1) for i in hits[:5]) or "none"
        raise RewrapError(
            f"--anchor must match exactly one line; it matched {len(hits)} "
            f"(lines {shown}) — pass --line"
        )
    return hits[0]


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="rewrap.py", description=__doc__.splitlines()[0]
    )
    parser.add_argument("path", type=Path)
    where = parser.add_mutually_exclusive_group(required=True)
    where.add_argument("--line", type=int, help="a 1-based line inside the block")
    where.add_argument("--anchor", help="text on exactly one line of the block")
    parser.add_argument("--width", type=int, default=None, help="override the cap")
    parser.add_argument("--dry-run", action="store_true", help="print, write nothing")
    args = parser.parse_args(argv)

    try:
        with args.path.open(newline="") as handle:  # raw, so CRLF shows
            text = handle.read()
    except OSError as exc:
        raise RewrapError(f"cannot read {args.path}: {exc}") from exc
    if "\r" in text:
        raise RewrapError(f"{args.path} has CRLF line endings; rewrap it by hand")
    lines = text.split("\n")
    trailing = lines[-1] == ""
    if trailing:
        lines.pop()
    kind, width = width_for(args.path, args.width)
    index = locate(lines, args.line, args.anchor)
    rewrap = rewrap_markdown if kind == "markdown" else rewrap_comment
    first, last, new = rewrap(lines, index, width)

    old = lines[first : last + 1]
    span = f"{args.path}:{first + 1}-{last + 1}"
    if new == old:
        print(f"rewrap | {span} already wrapped to {width}")
        return 0
    over = [n for n in new if len(n) > width]
    lines[first : last + 1] = new
    if not args.dry_run:
        args.path.write_text("\n".join(lines) + ("\n" if trailing else ""))
    verb = "would rewrap" if args.dry_run else "rewrapped"
    summary = (
        f"rewrap | {verb} {span} to {width}: {len(old)} -> {len(new)} line(s), "
        f"now {first + 1}-{first + len(new)}"
    )
    if over:
        summary += f" | {len(over)} line(s) still over: a token longer than the room"
    print(summary)
    for n, line in enumerate(new):
        print(f"{args.path}:{first + n + 1}:{line}")
    return 0


def main() -> int:
    try:
        return run(sys.argv[1:])
    except RewrapError as exc:
        print(f"rewrap: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
