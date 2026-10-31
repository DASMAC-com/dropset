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

**Established facts are a required section.** Every brief must carry the facts
already verified before the run — including the **negatives** ("there is no test
harness here", "this export has zero call sites", "there is no central clock
provider"). This is measured, not a hunch: the two cheapest review fan-outs on
record both credited an ad-hoc block of exactly this shape, one running all five
lenses at or under their turn caps with zero overruns and two lenses reporting
they needed no further reads, the other producing the review's sharpest findings
from a lens that did **zero** cold reads. The excerpt rule covers what you have
already read; this covers what you already know isn't there — and a lens cannot
tell "nobody mentioned it" apart from "I had better go check".

So ``--fact`` (repeatable) or ``--facts-file`` is mandatory, and a run with
genuinely nothing to state must say so with ``--no-facts`` rather than omitting
the section silently.

**``--lens <name>`` composes one review lens's whole brief.** The agent-facing
wording of each ``review-pr`` lens — its dimension, standing sub-questions,
what it is handed, its default cap — lives in one committed file under
``.claude/skills/review-pr/lenses/``, so the reviewing session holds none of it
resident. ``--lens`` appends ``lens-standing.md`` and then that file, and fills
the file's ``CAP_PLACEHOLDER`` code span from ``--cap`` or, absent it, from the
file's own ``<!-- default-cap: ... -->`` line. An unknown name is refused, and
``LENSES`` is the table a unittest holds equal to the directory listing, so a
name the tool accepts cannot be missing its file, nor a file be orphaned.

Usage::

    python3 .claude/tools/lens_preamble.py --out <scratchpad>/lens-preamble.md \\
        --fact '<something you verified, stated as fact>' \\
        --fact '<a NEGATIVE you verified — "there is no X here">'
    python3 .claude/tools/lens_preamble.py --out <scratchpad>/brief-<lens>.md \\
        --facts-file <scratchpad>/facts.md \\
        --lens <lens> [--cap '<= 6 turns / <= 9 tool calls']

The facts above are **placeholders on purpose**. An earlier version of this
docstring used concrete, plausible-looking examples, which is a hazard in a
command meant to be copy-pasted: the composed section tells every lens to treat
its contents as binding and not to re-derive them, so a run that copies the
example verbatim injects *false* established facts into the whole fan-out.

Prints the written path on stdout and a one-line summary on stderr. Standard
library only. A Python skill-tool under ``.claude/tools/`` — deliberately
**not** a Cargo workspace member. Tests live in ``tests/test_lens_preamble.py``,
run via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

# The convention doc that owns the canonical wording. Resolved relative to the
# repo root so the tool works from a worktree or the base checkout alike.
BRIEF_DOC = Path("docs/conventions/sub-agent-brief.md")

# The line that introduces the blockquote. Anchoring on it (rather than "the
# first blockquote in the file") means a later doc edit that adds an earlier
# quoted aside cannot silently swap what gets emitted.
BRIEF_MARKER = "**Prepend this standing brief to *every* `Agent` prompt:**"


# The review-pr lens wording. The standing half every lens shares, then one
# file per lens; both relative to the repo root like ``BRIEF_DOC``.
LENS_STANDING = Path(".claude/skills/review-pr/lens-standing.md")
LENS_DIR = Path(".claude/skills/review-pr/lenses")

# Every review-pr lens with its own brief file in ``LENS_DIR`` (the reduced
# tier's reformat and value-rewiring lenses reuse ``move-fidelity`` and
# ``correctness``). The unittest holds this equal to the directory listing in
# both directions.
LENSES = (
    "ci-skip-list",
    "comment-accuracy",
    "completeness",
    "correctness",
    "cross-check",
    "doc-freshness",
    "move-fidelity",
    "ops-correctness",
    "security",
    "style",
    "test-validity",
)

# Written as a code span so markdown formatters leave it alone (mdformat
# escapes a bare ``{{``); the whole span, backticks included, is replaced.
CAP_PLACEHOLDER = "`{{cap}}`"

# A lens file's own default cap, as an HTML comment so it renders as nothing.
# It is stripped from the composed brief: the agent reads the filled-in cap.
DEFAULT_CAP_RE = re.compile(r"^<!--\s*default-cap:\s*(.+?)\s*-->\s*\n?", re.M)

# The shell rule restated where a lens acts on it, immediately above its task.
FIRST_CALL = (
    "## Your first tool call\n"
    "\n"
    "Make it the Grep tool, or one bare `grep -n '<pattern>' <file>` — never "
    "`;`, `&&`, `|`, `$(…)` or backticks, which the guard blocks and which cost "
    "a call from your cap. Then slice: `Read` with the `offset` / `limit` the "
    "match's line number gives."
)


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


FACTS_HEADING = "## Established facts — do not re-derive"

# The framing around the facts. Written here rather than left to each caller so
# every lens in every run reads the same instruction about how to treat them —
# in particular that a *negative* is a fact, which is the half that gets dropped
# when the block is assembled ad hoc.
FACTS_PREAMBLE = (
    "These were verified before your run. Treat them as given: do not spend "
    "turns re-deriving them, and do not raise a finding that contradicts one "
    "without saying which fact you are contradicting and why. A stated absence "
    "is as binding as a stated presence — if something is listed as not "
    "existing, do not go looking for it."
)

NO_FACTS_NOTE = (
    "Nothing was verified before this run, so treat every claim below as "
    "unestablished. This is stated explicitly rather than left blank: an absent "
    "section reads as an oversight, and a lens that cannot tell the difference "
    "re-derives everything."
)


def facts_section(facts: list[str], no_facts: bool) -> list[str]:
    """The established-facts block, as lines.

    ``no_facts`` is the deliberate empty case and prints its own note, so the
    section is present either way. That is the whole mechanism: the section
    cannot be silently missing, only explicitly empty.
    """
    if no_facts:
        return ["", FACTS_HEADING, "", NO_FACTS_NOTE]
    body = ["", FACTS_HEADING, "", FACTS_PREAMBLE, ""]
    body.extend(f"- {fact.strip()}" for fact in facts)
    return body


def lens_path(lens: str) -> Path:
    """The committed file for ``lens``, refusing a name the table lacks."""
    if lens not in LENSES:
        raise LensPreambleError(
            f"unknown lens {lens!r} — known lenses: {', '.join(LENSES)}"
        )
    return LENS_DIR / f"{lens}.md"


def fill_cap(text: str, cap: str | None, name: str) -> str:
    """``text`` with its default-cap line stripped and ``{{cap}}`` filled.

    ``cap`` wins over the file's default. A file with the placeholder and
    neither is refused rather than shipped with a literal ``{{cap}}``: a lens
    with no stated cap has no hard stop, which is the overrun the cap exists
    to prevent.
    """
    match = DEFAULT_CAP_RE.search(text)
    body = DEFAULT_CAP_RE.sub("", text, count=1) if match else text
    cap = (cap or "").strip() or None
    if CAP_PLACEHOLDER not in body:
        if cap:
            raise LensPreambleError(
                f"--cap was given but {name} has no {CAP_PLACEHOLDER} to fill"
            )
        return body
    chosen = cap or (match.group(1).strip() if match else None)
    if not chosen:
        raise LensPreambleError(
            f"{name} has a {CAP_PLACEHOLDER} placeholder but no default-cap line, "
            "and no --cap was given"
        )
    return body.replace(CAP_PLACEHOLDER, chosen)


def compose(
    root: Path,
    appends: list[Path],
    facts: list[str] | None = None,
    no_facts: bool = False,
    lens: str | None = None,
    cap: str | None = None,
) -> str:
    """The full preamble: the canonical brief, the facts, then each append.

    With ``lens``, ``lens-standing.md`` and then the lens's own file follow the
    explicit appends (``lens-standing.md`` once, even if also appended).
    """
    if cap is not None and lens is None:
        raise LensPreambleError("--cap fills a lens file's cap; pass --lens too")
    lens_file = lens_path(lens) if lens is not None else None
    doc = root / BRIEF_DOC
    try:
        text = doc.read_text(encoding="utf-8")
    except OSError as e:
        raise LensPreambleError(f"cannot read {doc}: {e}") from e

    facts = facts or []
    if not facts and not no_facts:
        raise LensPreambleError(
            "no established facts given — pass --fact/--facts-file with what was "
            "verified before this run (negatives included), or --no-facts to "
            "state on the record that nothing was. The two cheapest review "
            "fan-outs on record both credited this section; omitting it silently "
            "is what this refusal prevents"
        )

    if lens is None:
        intro = [
            "# Standing brief for this sub-agent",
            "",
            "This file is the standing half of your brief — identical for every "
            "sub-agent pointed at it. Your own scope follows in the prompt "
            "that pointed you here.",
        ]
    else:
        intro = [
            f"# Brief for the `{lens}` lens of this review",
            "",
            "This file is your whole standing brief: the shared rules, then your "
            "lens's own section at the end. The material gathered for your lens "
            "and your scope line follow in the prompt that pointed you here.",
        ]
    parts = [
        *intro,
        "",
        "## Shell and tool conventions",
        "",
        extract_brief(text),
    ]

    # Before the skill's own appended scaffolding: the facts change what a lens
    # bothers to read, so they should be read before the scope detail, not after.
    parts.extend(facts_section(facts, no_facts))

    sections = list(appends)
    if lens_file is not None:
        sections = [p for p in sections if p != LENS_STANDING]
        sections.extend([LENS_STANDING, lens_file])

    for extra in sections:
        try:
            body = (root / extra).read_text(encoding="utf-8")
        except OSError as e:
            raise LensPreambleError(f"cannot read {extra}: {e}") from e
        if extra == lens_file:
            body = fill_cap(body, cap, str(extra))
            # The section right before the lens's own task: buried in the
            # brief, the rule lost every lens of one fan-out its first call
            # to the compound guard.
            parts.extend(["", FIRST_CALL])
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
    parser.add_argument(
        "--lens",
        default=None,
        metavar="NAME",
        help=f"append lens-standing.md and the named lens file ({', '.join(LENSES)})",
    )
    parser.add_argument(
        "--cap",
        default=None,
        metavar="TEXT",
        help="the lens's cap, overriding its file's default (requires --lens)",
    )
    parser.add_argument(
        "--fact",
        action="append",
        default=[],
        metavar="TEXT",
        help="an established fact, negatives included (repeatable; required "
        "unless --facts-file or --no-facts is given)",
    )
    parser.add_argument(
        "--facts-file",
        default=None,
        metavar="PATH",
        help="a file of established facts, one per line",
    )
    parser.add_argument(
        "--no-facts",
        action="store_true",
        help="state on the record that nothing was verified before this run",
    )
    args = parser.parse_args(argv[1:])

    root = Path(args.root)
    facts = list(args.fact)
    if args.facts_file:
        try:
            raw = Path(args.facts_file).read_text(encoding="utf-8")
        except OSError as e:
            raise LensPreambleError(f"cannot read {args.facts_file}: {e}") from e
        # Blank lines and comments are dropped so a hand-kept facts file can be
        # annotated without the annotations reaching the brief.
        facts.extend(
            line.strip().lstrip("-").strip()
            for line in raw.splitlines()
            if line.strip() and not line.strip().startswith("#")
        )
    facts = [f for f in facts if f]

    if facts and args.no_facts:
        raise LensPreambleError(
            "--no-facts contradicts the facts given; drop one of them"
        )

    text = compose(
        root,
        [Path(p) for p in args.append],
        facts,
        args.no_facts,
        lens=args.lens,
        cap=args.cap,
    )

    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(text, encoding="utf-8")

    print(out)
    lens_note = f", lens {args.lens}" if args.lens else ""
    print(
        f"lens-preamble | {len(text.splitlines())} line(s), "
        f"{len(facts)} established fact(s), "
        f"{len(args.append)} appended section(s){lens_note}",
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
