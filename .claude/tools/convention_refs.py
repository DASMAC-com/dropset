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

#: The citer families whose absence is always a fault rather than a choice.
#: `.claude/shared/*.md` is deliberately excluded — that tree holds optional
#: shared prose fragments and a repo may legitimately have none, so requiring it
#: would make the check cry wolf. The other three are structural to this repo.
REQUIRED_CITER_GLOBS = (
    "CLAUDE.md",
    ".claude/skills/*/SKILL.md",
    "docs/conventions/*.md",
)

#: Where a bare (directory-less) doc name resolves.
CONVENTION_DIR = "docs/conventions"

#: ``·`` `path.md` → "Anchor" ·`` — the house citation form, with either arrow
#: spelling and any amount of space around it.
CITATION_RE = re.compile(r"`([A-Za-z0-9_./-]+\.md)`\s*(?:→|->)\s*[\"“]([^\"”]+)[\"”]")

#: A bold span. Anchors legitimately point at these, not only at headings.
BOLD_RE = re.compile(r"\*\*(.+?)\*\*")

#: The same, allowed to span lines — because under MD013 a bold span wraps just
#: as a citation does. `DOTALL` is what `BOLD_RE` lacks; joining the lines alone
#: does nothing without it, since `.` does not cross a newline. Captures
#: containing a blank line are discarded by the caller: a real bold span never
#: continues across a paragraph break, so that bound keeps one stray `**` from
#: pairing with another half a document away.
BOLD_MULTILINE_RE = re.compile(r"\*\*(.+?)\*\*", re.DOTALL)

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


def paragraphs(text: str) -> list[str]:
    """The unfenced paragraphs of ``text``, each joined into one string.

    Both halves of this tool compare constructs that **wrap** — a citation and
    a bold span alike — so both need to see across a line break, and neither
    ever spans a *paragraph* break. A paragraph is therefore the right unit,
    and it is not merely a convenience:

    Matching with `DOTALL` over the whole document is actively worse than
    per-line matching, which is the trap this function exists to avoid. Bold
    pairing is positional, so a single unpaired `**` anywhere — a literal shown
    as an example, an emphasis inside a code span — shifts every pairing after
    it. Per-line matching *contains* that damage to one line; whole-document
    DOTALL lets it corrupt the rest of the file. Measured while fixing the wrap
    blindness here: joining the whole document turned 2 unresolved citations
    into 5, all of them false, by mispairing spans hundreds of lines away from
    the stray marker. Per paragraph, a desync cannot outlive its own paragraph.
    """
    out: list[str] = []
    current: list[str] = []
    in_fence = False
    for line in text.splitlines():
        if FENCE_RE.match(line):
            in_fence = not in_fence
            if current:
                out.append("\n".join(current))
                current = []
            continue
        if in_fence:
            continue
        if not line.strip():
            if current:
                out.append("\n".join(current))
                current = []
            continue
        current.append(line)
    if current:
        out.append("\n".join(current))
    return out


def anchors(text: str) -> set[str]:
    """Every anchor a citation may legitimately target in one document.

    Headings **and** bold spans, because the second is what heading-only
    matching gets wrong. Fenced blocks are skipped: a doc that *quotes* a
    heading or a bold field name in an example is not defining an anchor, and
    counting one would let a real dangling citation resolve against a sample.

    **Bold spans are matched over the joined text, symmetrically with
    ``citations``.** A bold span wraps across lines under MD013 just as a
    citation does, and matching per line missed those — the mirror image of the
    same defect, on the other side of the comparison. Fixing only the citation
    half surfaced two "dangling" citations whose anchors existed perfectly well
    as multi-line bold spans, so the two halves are one fix.

    Headings stay per line: a heading is a single-line construct by
    definition, and `^#` on joined text would match a `#` mid-paragraph.
    """
    found: set[str] = set()
    for paragraph in paragraphs(text):
        for line in paragraph.splitlines():
            if line.startswith("#"):
                found.add(normalize(line.lstrip("#")))
        for match in BOLD_MULTILINE_RE.finditer(paragraph):
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
    """Every file that may carry a citation, de-duplicated.

    Sorted **within** each glob and concatenated in ``CITER_GLOBS`` order — so
    the result is deterministic but not globally sorted. The docstring used to
    claim "sorted" flatly, which is a different and false statement.
    """
    seen: dict[Path, None] = {}
    for pattern in CITER_GLOBS:
        for path in sorted(repo.glob(pattern)):
            if path.is_file():
                seen.setdefault(path, None)
    return list(seen)


def empty_families(repo: Path) -> list[str]:
    """The ``CITER_GLOBS`` entries that match no file at all.

    The zero-citation refusal in ``run`` cannot catch a renamed or moved
    *family*: four globs feed the scan, so losing the whole skills tree still
    leaves `docs/conventions/*.md` citing and the total non-zero. The count
    stays healthy, the exit stays 0, and the loss is invisible — which is the
    one failure the refusal exists to prevent, arriving one level down.

    Checking for matched **files** rather than a citation floor is deliberate:
    a family may legitimately carry no citations, but one of
    ``REQUIRED_CITER_GLOBS`` matching no files at all is always a moved tree or
    a wrong ``--repo``.
    """
    return [
        pattern
        for pattern in REQUIRED_CITER_GLOBS
        if not any(path.is_file() for path in repo.glob(pattern))
    ]


def citations(text: str) -> list[tuple[str, str]]:
    """``[(cited_path, anchor), …]`` outside fenced blocks.

    Fence-skipping matters on both sides: a skill that shows a citation inside
    a worked example is documenting the *form*, not making a claim that has to
    resolve, and flagging it would train the reader to ignore this tool.

    **Matched over the joined text, not line by line.** Per-line matching
    required the path, the arrow and the quoted anchor to share one line — and
    under MD013's 80-column limit they routinely do not. The live tree carries
    **14** citations whose path and arrow end one line with the anchor on the
    next, in `review-pr`, `plan`, `audit`, `audit-scope`, `housekeeping`,
    `linear-task`, `merge-tasks`, `session-metrics`, `trim-context`,
    `pr-title-description` and `cspell-audit` — the highest-traffic skills,
    which is to say exactly the drift this tool exists to catch.

    Worse than a static blind spot: `mdformat` re-wrapping a paragraph can move
    a currently-checked citation across a line boundary, silently dropping it
    from the count with no signal at all. `CITATION_RE`'s `\\s*` already spans a
    newline, so joining the surviving lines is the whole fix.
    """
    return [
        (match.group(1), match.group(2))
        for paragraph in paragraphs(text)
        for match in CITATION_RE.finditer(paragraph)
    ]


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
            # An anchor that normalizes away entirely resolves against
            # EVERYTHING, because `"" in candidate` is always true — the literal
            # vacuous pass. `normalize` strips emphasis, backticks and trailing
            # punctuation, so `→ "."` and `→ "**"` both reach here empty. Report
            # it rather than letting it count as checked and resolving.
            if not wanted:
                dangling.append(
                    {
                        "citer": rel,
                        "target": str(target.relative_to(repo)),
                        "anchor": anchor,
                        "kind": "empty-anchor",
                    }
                )
                continue
            # Substring, not equality: a citation routinely shortens a long
            # heading, and demanding the whole thing would report drift that is
            # only abbreviation. The floor is that `wanted` is non-empty — a
            # deliberate looseness with one hard bound, rather than an unbounded
            # one.
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

    # A moved or renamed citer family is checked FIRST, because it is the more
    # specific diagnosis: when it fires, the zero-citation refusal below often
    # fires too, and "nothing was checked" sends the reader looking at the
    # citation form when the actual fault is a missing tree.
    #
    # It is also the case the zero-citation refusal cannot reach on its own.
    # Four globs feed the scan, so losing the whole skills tree still leaves
    # `docs/conventions/*.md` citing, the total non-zero and the exit 0 — the
    # loss invisible behind a healthy-looking count.
    missing = empty_families(repo)
    if missing:
        raise ConventionRefsError(
            "these citer families matched no files under "
            f"{repo}: {', '.join(missing)} — a moved tree or a wrong --repo, "
            "either of which hides real drift behind a healthy-looking count"
        )

    # Finding nothing to check is not a clean bill of health either: a changed
    # citation form produces zero citations even with every family present, and
    # returning 0 for that is byte-identical — in the one signal a caller
    # branches on — to "everything resolves".
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
