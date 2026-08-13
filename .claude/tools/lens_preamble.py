#!/usr/bin/env python3
"""Compose the standing half of a sub-agent brief, without reading it first.

``review-pr`` step 5 already knows the right shape: every lens brief has a
**standing** half that is byte-identical across all of them and a **per-lens**
half, so the standing half is written to the scratchpad once and each agent is
handed the path. What that still cost was a whole-file ``Read`` of
``docs/conventions/sub-agent-brief.md`` (~1.7k, measured twice) purely to copy
verbatim, unchanging boilerplate into it — paid on every review run, forever.

Both sessions that paid it correctly noted the read was licensed under the
read-whole carve-out (the content is quoted onward into several briefs), which
is precisely the argument for tooling it away: it is deterministic string
assembly over a file with one owner, i.e. the ``CLAUDE.md`` -> "Skill tooling"
shape. This reads the brief in its **own** process and writes the composed
preamble, so the skill names a path and reads nothing.

The canonical brief is the blockquote in ``sub-agent-brief.md``. It is stored
quoted (it is quoted *material* in a convention doc) and emitted unquoted, since
the agent reading the preamble is being addressed directly.

A skill with standing text of its own passes ``--append`` (repeatable), keeping
one owner per half: the convention doc owns the shell rules, the skill's own
committed template owns its review-specific scaffolding. Neither is duplicated
into this tool.

Usage::

    python3 .claude/tools/lens_preamble.py --out <scratchpad>/lens-preamble.md
    python3 .claude/tools/lens_preamble.py --out <path> \\
        --append .claude/skills/review-pr/lens-standing.md

Prints the written path on stdout and a one-line summary on stderr. Standard
library only. A Python skill-tool under ``.claude/tools/`` — deliberately
**not** a Cargo workspace member. Tests live in ``tests/test_lens_preamble.py``,
run via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

# The convention doc that owns the canonical wording. Resolved relative to the
# repo root so the tool works from a worktree or the base checkout alike.
BRIEF_DOC = Path("docs/conventions/sub-agent-brief.md")

# The line that introduces the blockquote. Anchoring on it (rather than "the
# first blockquote in the file") means a later doc edit that adds an earlier
# quoted aside cannot silently swap what gets emitted.
BRIEF_MARKER = "**Prepend this standing brief to *every* `Agent` prompt:**"


class LensPreambleError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def extract_brief(text: str) -> str:
    """The canonical brief, de-quoted.

    Takes the contiguous run of blockquote lines following the marker. A
    blank line inside the quote would end it, which is why the brief is
    authored as one unbroken block — and why a doc edit that breaks it into
    two blocks is caught here as a truncated brief rather than silently
    shipping half the shell rules.
    """
    lines = text.splitlines()
    try:
        start = next(i for i, line in enumerate(lines) if line.strip() == BRIEF_MARKER)
    except StopIteration:
        raise LensPreambleError(
            f"{BRIEF_DOC} no longer contains the marker line "
            f"{BRIEF_MARKER!r} — the brief cannot be located"
        ) from None

    quoted: list[str] = []
    for line in lines[start + 1 :]:
        if not line.strip():
            if quoted:
                break
            continue
        if not line.startswith(">"):
            break
        # Strip the marker and the single space that follows it, preserving
        # any deeper indentation (the brief uses nested list items).
        stripped = line[1:]
        if stripped.startswith(" "):
            stripped = stripped[1:]
        quoted.append(stripped)

    if not quoted:
        raise LensPreambleError(
            f"found the marker in {BRIEF_DOC} but no blockquote after it"
        )
    return "\n".join(quoted).rstrip()


def compose(root: Path, appends: list[Path]) -> str:
    """The full preamble: the canonical brief, then each appended section."""
    doc = root / BRIEF_DOC
    try:
        text = doc.read_text(encoding="utf-8")
    except OSError as e:
        raise LensPreambleError(f"cannot read {doc}: {e}") from e

    parts = [
        "# Standing brief for this review",
        "",
        "This file is the standing half of every lens brief in this run — "
        "identical across lenses. Your own scope follows in the prompt that "
        "pointed you here.",
        "",
        "## Shell and tool conventions",
        "",
        extract_brief(text),
    ]

    for extra in appends:
        try:
            body = (root / extra).read_text(encoding="utf-8")
        except OSError as e:
            raise LensPreambleError(f"cannot read {extra}: {e}") from e
        parts.extend(["", body.rstrip()])

    return "\n".join(parts) + "\n"


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="lens_preamble.py")
    parser.add_argument("--out", required=True, help="path to write the preamble to")
    parser.add_argument("--root", default=".", help="repo root (default cwd)")
    parser.add_argument(
        "--append",
        action="append",
        default=[],
        metavar="PATH",
        help="a committed standing section to append (repeatable)",
    )
    args = parser.parse_args(argv[1:])

    root = Path(args.root)
    text = compose(root, [Path(p) for p in args.append])

    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(text, encoding="utf-8")

    print(out)
    print(
        f"lens-preamble | {len(text.splitlines())} line(s), "
        f"{len(args.append)} appended section(s)",
        file=sys.stderr,
    )
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except LensPreambleError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
