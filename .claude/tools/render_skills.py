#!/usr/bin/env python3
# cspell:word keepends
"""Render shared prose blocks into skill files, and gate their freshness.

Changing a convention today means editing the convention doc **and** every
skill that restates it, enforced by a review lens plus a housekeeping pass —
which is to say, by an agent remembering to look. That hand-sync tax is paid on
every meta batch, and it is measurable: one batch updated the same
search-shape rule in three separate files, and the same cspell rule in two.

This makes a repeated block have **one source**. A skill marks the region it
wants filled, and this tool fills it:

    <!-- render:begin fable-model-guard verb=paps -->
    ...generated content, do not edit by hand...
    <!-- render:end fable-model-guard -->

``--check`` re-renders in memory and fails on any difference, so a hand-edited
generated region is caught rather than silently kept. It also fails on a
**dangling** marker — an unclosed region, or one naming a source that does not
exist — because a region that never renders is the same silent failure as a
committed-but-unwired guard hook.

Deliberately a **narrow** extraction, not a general template engine. Blocks
live in ``.claude/shared/``, substitution is a flat ``{{name}}`` replace, and
there is no logic, no inheritance and no partials. If a block needs a
conditional it is not a shared block — it is two blocks.

**On what actually repeats.** Less than expected, and that is worth recording
so the next pass does not over-build: the `plan` and `init-pr` model guards
point in *opposite* directions and are a complementary pair rather than a
duplicate, and the several "runs in the base repo" mentions each say something
different about why. Extract a block only when the prose is genuinely
verbatim in two or more places.

Usage::

    python3 .claude/tools/render_skills.py --check
    python3 .claude/tools/render_skills.py --write

Stdlib only, and deliberately **not** a Cargo workspace member — see
``docs/conventions/skill-tooling.md``. Tests live in
``tests/test_render_skills.py``, run via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

SHARED_DIR = Path(".claude/shared")
SKILLS_DIR = Path(".claude/skills")

# `<!-- render:begin <source> k=v k=v -->` … `<!-- render:end <source> -->`
BEGIN_RE = re.compile(
    r"^(?P<indent>[ \t]*)<!-- render:begin (?P<source>[a-z0-9-]+)(?P<args>[^>]*)-->[ \t]*$"
)
END_RE = re.compile(r"^[ \t]*<!-- render:end (?P<source>[a-z0-9-]+) -->[ \t]*$")

# A `{{name}}` placeholder inside a shared block.
PLACEHOLDER_RE = re.compile(r"\{\{([a-z0-9_]+)\}\}")


class RenderError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def parse_args_spec(raw: str) -> dict:
    """``k=v k=v`` from a begin marker, as a dict."""
    values = {}
    for token in raw.split():
        if "=" not in token:
            raise RenderError(f"malformed marker argument {token!r} (want k=v)")
        key, value = token.split("=", 1)
        values[key] = value
    return values


def substitute(text: str, values: dict, source: str) -> str:
    """Replace every ``{{name}}``, refusing an unresolved one.

    Refusing rather than leaving it in place: a `{{verb}}` that reached a
    rendered skill would be read by an agent as literal text, which is a
    silent instruction defect rather than a visible failure.
    """
    missing = sorted(set(PLACEHOLDER_RE.findall(text)) - set(values))
    if missing:
        raise RenderError(
            f"{source}: no value given for {', '.join(missing)} — add it to the "
            f"render:begin marker as {missing[0]}=<value>"
        )
    return PLACEHOLDER_RE.sub(lambda m: values[m.group(1)], text)


def load_block(root: Path, source: str) -> str:
    path = root / SHARED_DIR / f"{source}.md"
    if not path.is_file():
        raise RenderError(
            f"no shared block {source!r} at {path} — a render:begin marker names "
            "a file that does not exist, so that region would never render"
        )
    return path.read_text(encoding="utf-8")


def render_text(root: Path, text: str, label: str) -> str:
    """Return ``text`` with every marked region refilled from its source."""
    lines = text.splitlines(keepends=True)
    out: list[str] = []
    index = 0
    while index < len(lines):
        line = lines[index]
        begin = BEGIN_RE.match(line.rstrip("\n"))
        if begin is None:
            if END_RE.match(line.rstrip("\n")):
                raise RenderError(
                    f"{label}: a render:end marker with no matching begin at line "
                    f"{index + 1}"
                )
            out.append(line)
            index += 1
            continue

        source = begin.group("source")
        indent = begin.group("indent")
        values = parse_args_spec(begin.group("args"))

        # Find the matching end, and refuse an unclosed region rather than
        # swallowing the rest of the file.
        close = None
        for probe in range(index + 1, len(lines)):
            end = END_RE.match(lines[probe].rstrip("\n"))
            if end is not None:
                if end.group("source") != source:
                    raise RenderError(
                        f"{label}: render:begin {source!r} closed by render:end "
                        f"{end.group('source')!r} at line {probe + 1}"
                    )
                close = probe
                break
        if close is None:
            raise RenderError(
                f"{label}: unclosed render:begin {source!r} at line {index + 1}"
            )

        block = substitute(load_block(root, source), values, source)
        out.append(line)
        # A blank line on each side of the content, because `mdformat` inserts
        # one between an HTML comment and an adjacent paragraph. Emitting it
        # here makes rendering a FIXED POINT of the formatter — without it,
        # every render is immediately reformatted and `--check` then reports
        # the file as stale forever, which would make the gate useless.
        out.append("\n")
        for block_line in block.splitlines():
            # An empty line stays empty: trailing whitespace on a blank line is
            # what the end-of-file / whitespace hooks strip, so emitting it here
            # would make the rendered output fail lint by construction.
            out.append(f"{indent}{block_line}\n" if block_line else "\n")
        out.append("\n")
        out.append(lines[close])
        index = close + 1

    return "".join(out)


def skill_files(root: Path) -> list[Path]:
    directory = root / SKILLS_DIR
    if not directory.is_dir():
        return []
    return sorted(directory.glob("*/SKILL.md"))


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="render_skills.py")
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument(
        "--check",
        action="store_true",
        help="fail if any rendered region differs from its source",
    )
    mode.add_argument("--write", action="store_true", help="rewrite each file in place")
    parser.add_argument("--root", default=".", help="repo root (default cwd)")
    args = parser.parse_args(argv[1:])

    root = Path(args.root)
    stale: list[str] = []
    rendered_count = 0

    for path in skill_files(root):
        original = path.read_text(encoding="utf-8")
        if "render:begin" not in original and "render:end" not in original:
            continue
        rendered_count += 1
        updated = render_text(root, original, str(path))
        if updated == original:
            continue
        if args.write:
            path.write_text(updated, encoding="utf-8")
            print(f"rendered {path}")
        else:
            stale.append(str(path))

    if args.check and stale:
        for name in stale:
            print(f"STALE {name}", file=sys.stderr)
        print(
            f"render-skills | {len(stale)} file(s) differ from their shared "
            "source. Run `--write` and commit the result; do not hand-edit a "
            "generated region.",
            file=sys.stderr,
        )
        return 1

    print(
        f"render-skills | {rendered_count} file(s) carry rendered regions, all in sync",
        file=sys.stderr,
    )
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except RenderError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
