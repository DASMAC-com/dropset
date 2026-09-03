#!/usr/bin/env python3
"""convention_refs.py — report skill/doc citations that no longer resolve.

``CLAUDE.md``'s own preamble requires that changing a convention updates both
its ``docs/conventions/`` file **and** any skill that references it. Nothing
checked that mechanically: ``housekeeping`` step 5 specified the check in prose
("list the headings in ``CLAUDE.md``", "grep the skills for references", "flag
dangling references"), and executing it took **eight ad-hoc greps** — a
101-line citation sweep to yield a 12-name set, a 25-line one for a 6-name set,
four heading listings, and a targeted grep to confirm one anchor by hand — for
a verdict that is one line. ~1.2k per pass, every pass.

Every input is mechanical: the anchor sets, the citation forms, the set
difference. That is exactly the ``CLAUDE.md`` → "Skill tooling" test, and the
precedent is in the same skill — steps 7a and 7b call ``allowlist.py cruft``
and ``hook_wiring.py`` rather than describing their checks in prose.

**Two behaviors the prose did not settle, both of which arose in practice:**

1. **A citation may target a BOLDED PARAGRAPH, not a heading.** Anchoring on
   headings alone reports false drift — the live instance was
   ``"Relations and state belong in the CREATING call"``, a bold lead-in rather
   than a section. So bold spans count as anchors.
1. **Citations come in more than one form** — ``` `CLAUDE.md` → "X" ```,
   ``` `docs/conventions/y.md` → "X" ```, and a bare doc mention — so the
   extractor needs the set, not one regex.

Usage::

    python3 .claude/tools/convention_refs.py            # human-readable
    python3 .claude/tools/convention_refs.py --json     # machine-readable

Exit status is grep-shaped, like ``hook_wiring.py``: ``0`` when every citation
resolves, ``1`` when at least one dangles, ``2`` when the scan could not run.

Standard library only. A Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling"). Tests live in ``tests/test_convention_refs.py``, run via
``make tools-tests``.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

#: Where a citation may live. The skills are the point of the check; the
#: convention docs and `CLAUDE.md` are included because they cite each other,
#: and a dangling cross-reference between two docs is the same defect.
CITER_GLOBS = (
    "CLAUDE.md",
    ".claude/skills/*/SKILL.md",
    ".claude/shared/*.md",
    "docs/conventions/*.md",
)

#: Where a bare (directory-less) doc name resolves.
CONVENTION_DIR = "docs/conventions"

#: ``·`` `path.md` → "Anchor" ·`` — the house citation form, with either arrow
#: spelling and any amount of space around it.
CITATION_RE = re.compile(r"`([A-Za-z0-9_./-]+\.md)`\s*(?:→|->)\s*[\"“]([^\"”]+)[\"”]")

#: A bold span. Anchors legitimately point at these, not only at headings.
BOLD_RE = re.compile(r"\*\*(.+?)\*\*")

FENCE_RE = re.compile(r"^\s*(?:```|~~~)")


class ConventionRefsError(Exception):
    """A user-facing failure: surfaced to stderr, exits 2."""


def normalize(text: str) -> str:
    """Fold an anchor to its comparable form.

    Citations quote loosely — backticks kept or dropped, trailing punctuation
    varying, emphasis included — so comparing raw strings reports drift that is
    only transcription. Case, whitespace, emphasis markers, backticks and
    trailing punctuation are all discarded.
    """
    text = text.replace("**", "").replace("`", "").replace("*", "")
    text = re.sub(r"\s+", " ", text).strip()
    return text.strip(" .:;,—–-").lower()


def anchors(text: str) -> set[str]:
    """Every anchor a citation may legitimately target in one document.

    Headings **and** bold spans, because the second is what heading-only
    matching gets wrong. Fenced blocks are skipped: a doc that *quotes* a
    heading or a bold field name in an example is not defining an anchor, and
    counting one would let a real dangling citation resolve against a sample.
    """
    found: set[str] = set()
    in_fence = False
    for line in text.splitlines():
        if FENCE_RE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        if line.startswith("#"):
            found.add(normalize(line.lstrip("#")))
        for match in BOLD_RE.finditer(line):
            found.add(normalize(match.group(1)))
    found.discard("")
    return found


def resolve_target(repo: Path, cited: str) -> Path | None:
    """The file a citation names, or None when nothing matches.

    A citation may spell the path from the repo root (``docs/conventions/x.md``,
    ``CLAUDE.md``) or by bare name (``x.md``), and both appear in the tree.
    """
    direct = repo / cited
    if direct.is_file():
        return direct
    if "/" not in cited:
        bare = repo / CONVENTION_DIR / cited
        if bare.is_file():
            return bare
    return None


def iter_citers(repo: Path) -> list[Path]:
    """Every file that may carry a citation, de-duplicated and sorted."""
    seen: dict[Path, None] = {}
    for pattern in CITER_GLOBS:
        for path in sorted(repo.glob(pattern)):
            if path.is_file():
                seen.setdefault(path, None)
    return list(seen)


def citations(text: str) -> list[tuple[str, str]]:
    """``[(cited_path, anchor), …]`` outside fenced blocks.

    Fence-skipping matters on both sides: a skill that shows a citation inside
    a worked example is documenting the *form*, not making a claim that has to
    resolve, and flagging it would train the reader to ignore this tool.
    """
    out: list[tuple[str, str]] = []
    in_fence = False
    for line in text.splitlines():
        if FENCE_RE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        for match in CITATION_RE.finditer(line):
            out.append((match.group(1), match.group(2)))
    return out


def scan(repo: Path) -> dict:
    """Check every citation in the tree and report the ones that dangle."""
    anchor_cache: dict[Path, set[str]] = {}
    dangling: list[dict] = []
    checked = 0

    for citer in iter_citers(repo):
        try:
            text = citer.read_text(encoding="utf-8")
        except OSError as exc:
            raise ConventionRefsError(f"could not read {citer}: {exc}") from exc

        for cited, anchor in citations(text):
            checked += 1
            target = resolve_target(repo, cited)
            rel = str(citer.relative_to(repo))
            if target is None:
                dangling.append(
                    {
                        "citer": rel,
                        "target": cited,
                        "anchor": anchor,
                        "kind": "missing-file",
                    }
                )
                continue
            if target not in anchor_cache:
                try:
                    anchor_cache[target] = anchors(target.read_text(encoding="utf-8"))
                except OSError as exc:
                    raise ConventionRefsError(
                        f"could not read {target}: {exc}"
                    ) from exc
            wanted = normalize(anchor)
            # Substring, not equality: a citation routinely shortens a long
            # heading, and demanding the whole thing would report drift that is
            # only abbreviation.
            if not any(wanted in candidate for candidate in anchor_cache[target]):
                dangling.append(
                    {
                        "citer": rel,
                        "target": str(target.relative_to(repo)),
                        "anchor": anchor,
                        "kind": "missing-anchor",
                    }
                )

    return {
        "repo": str(repo),
        "checked": checked,
        "count": len(dangling),
        "dangling": dangling,
    }


def render(result: dict) -> str:
    """The human report: one line per dangling citation, then a verdict."""
    lines = [
        f'  {d["kind"]:<15} {d["citer"]} → {d["target"]} "{d["anchor"]}"'
        for d in result["dangling"]
    ]
    if result["dangling"]:
        lines.append("")
        lines.append(
            f"convention-refs | {result['count']} of {result['checked']} "
            "citation(s) do not resolve. Either the anchor moved (update the "
            "citer) or the section was renamed (update both, per CLAUDE.md's "
            "preamble)."
        )
    else:
        lines.append(f"convention-refs | all {result['checked']} citation(s) resolve")
    return "\n".join(lines)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="convention_refs.py",
        description="Report skill/doc citations whose target or anchor is gone.",
    )
    parser.add_argument("--repo", default=".", help="repo root (default: cwd)")
    parser.add_argument("--json", action="store_true", help="machine-readable")
    args = parser.parse_args(argv[1:])

    repo = Path(args.repo).resolve()
    if not repo.is_dir():
        raise ConventionRefsError(f"--repo is not a directory: {repo}")

    result = scan(repo)

    # Finding nothing to check is not a clean bill of health: a wrong --repo, a
    # renamed skills tree or a changed citation form all produce zero
    # citations, and returning 0 for that is byte-identical — in the one signal
    # a caller branches on — to "everything resolves".
    if not result["checked"]:
        raise ConventionRefsError(
            f"no citations found under {repo} — nothing was checked, which is "
            "not the same answer as everything resolving"
        )

    if args.json:
        print(json.dumps(result, indent=2))
    else:
        print(render(result))
    return 1 if result["dangling"] else 0


def main() -> int:
    try:
        return run(sys.argv)
    except ConventionRefsError as exc:
        print(f"convention-refs: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
