#!/usr/bin/env python3
"""Resident-size gate: a hard byte cap on what every session carries.

Three residency classes exist, and only the third is visible to
``session_metrics.py``:

* **Class A** — resident on *every* turn of *every* session: the project
  instructions file and every skill's ``description``. The harness loads all of
  them before the operator types a word.
* **Class B** — a skill's entry file (``SKILL.md``), resident from the moment
  the skill is invoked to the end of the session.
* **Class C** — sibling files, convention docs, tool results: read at a
  trigger, ranked by ``session_metrics.py`` like any other tool result.

This tool caps A and B, and only **reports** C (``--report``): a sibling is a
tool result and is already ranked, and a per-skill *total* would penalize the
progressive loading the budget exists to encourage.

**Bytes, not tokens or lines.** Bytes are what ``wc -c`` reports, need no
tokenizer, and do not move when the model does (measured at ~4 bytes per token
on the largest file). A rendered ``.claude/shared/`` region counts in full:
rendering solves sync, not size.

**One cap, frozen exceptions, no ratchet schedule.** A committed baseline
(``cfg/skill-size-baseline.json``) names every subject over its cap with a
``ceiling`` equal to its size when it was admitted and the issue that retires
it. ``--check`` fails any subject larger than the **greater** of cap and
ceiling, so an over-cap file cannot grow by a byte and an under-cap file — a
stale entry's included — may grow to the cap. (The filing said "lesser", which
would fail every frozen exception on enable day; the two behaviors it
describes need the greater.) ``--write`` only ever *lowers* a ceiling (to the
current size) or drops an entry whose subject is under its cap; it never
raises one and never adds one.
Admitting a new exception is a separate, explicit act (``--admit``) that must
name its retiring issue — and a hand-raised ceiling is a review finding, since
no tool can see the history that would prove it.

Stdlib only. This is a Python skill-tool under ``.claude/tools/`` — deliberately
**not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import NamedTuple

ENTRY_CAP = 32_000
DESCRIPTION_CAP = 1_024
PROJECT_CAP = 32_000

PROJECT_FILE = "CLAUDE.md"
SKILLS_DIR = ".claude/skills"
ENTRY_NAME = "SKILL.md"
DESCRIPTION_SUFFIX = "#description"
DEFAULT_BASELINE = "cfg/skill-size-baseline.json"

# A retiring-issue reference. Data, not prose, so it carries the tag.
_ISSUE_RE = re.compile(r"^ENG-[0-9]+$")

# YAML block-scalar indicators: `>`, `|`, optionally with a chomping and/or
# indentation indicator (`>-`, `|+`, `>2`).
_BLOCK_SCALAR_RE = re.compile(r"^[>|][+-]?[0-9]?$")


class Subject(NamedTuple):
    """One capped thing: a file, or one skill's description."""

    key: str
    size: int
    cap: int


def description_value(text: str) -> str | None:
    """Return the frontmatter ``description`` value of a ``SKILL.md``.

    Handles the plain one-line form every skill uses today and the YAML
    block-scalar forms (``>`` / ``|``) a writer may reach for when a line gets
    long — the latter is folded to the indented lines that follow, which is
    what the harness reads. ``None`` when there is no frontmatter or no key.
    """
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        return None
    for index, line in enumerate(lines[1:], start=1):
        if line.strip() == "---":
            return None
        if not line.startswith("description:"):
            continue
        value = line[len("description:") :].strip()
        if not _BLOCK_SCALAR_RE.match(value):
            return value
        body: list[str] = []
        for follow in lines[index + 1 :]:
            if follow.strip() == "---" or (follow and not follow[0].isspace()):
                break
            body.append(follow.strip())
        return " ".join(part for part in body if part)
    return None


def collect(root: Path) -> list[Subject]:
    """Measure every capped subject under ``root``, in a stable order."""
    subjects: list[Subject] = []
    project = root / PROJECT_FILE
    if project.is_file():
        subjects.append(Subject(PROJECT_FILE, project.stat().st_size, PROJECT_CAP))
    # `*/SKILL.md` only: a sibling (a history ledger, a reference doc) is class
    # C and must never be measured as an entry file.
    for entry in sorted((root / SKILLS_DIR).glob(f"*/{ENTRY_NAME}")):
        rel = entry.relative_to(root).as_posix()
        subjects.append(Subject(rel, entry.stat().st_size, ENTRY_CAP))
        description = description_value(entry.read_text(encoding="utf-8"))
        if description is not None:
            subjects.append(
                Subject(
                    rel + DESCRIPTION_SUFFIX,
                    len(description.encode("utf-8")),
                    DESCRIPTION_CAP,
                )
            )
    return subjects


def load_baseline(path: Path) -> dict[str, dict]:
    """Read the baseline's exceptions; a missing file means no exceptions."""
    if not path.is_file():
        return {}
    data = json.loads(path.read_text(encoding="utf-8"))
    return dict(data.get("exceptions", {}))


def dump_baseline(path: Path, exceptions: dict[str, dict]) -> None:
    """Write the baseline deterministically (sorted keys, trailing newline)."""
    body = {
        "exceptions": {
            key: {
                "ceiling": exceptions[key]["ceiling"],
                "issue": exceptions[key]["issue"],
            }
            for key in sorted(exceptions)
        }
    }
    path.write_text(json.dumps(body, indent=2) + "\n", encoding="utf-8")


def check(
    subjects: list[Subject], exceptions: dict[str, dict]
) -> tuple[list[str], list[str]]:
    """Return ``(failures, notices)`` for the measured subjects.

    A failure is a subject over its limit, a malformed baseline entry, or an
    entry naming a subject that no longer exists. A notice is an entry whose
    subject is now below its ceiling — harmless (the slack can be regrown, but
    never past the ceiling), and cleared by ``--write``.
    """
    failures: list[str] = []
    notices: list[str] = []
    by_key = {subject.key: subject for subject in subjects}

    for key in sorted(exceptions):
        entry = exceptions[key]
        ceiling = entry.get("ceiling")
        issue = entry.get("issue")
        if not isinstance(ceiling, int) or ceiling < 0:
            failures.append(f"{key}: baseline ceiling {ceiling!r} is not a byte count")
            continue
        subject = by_key.get(key)
        if subject is None:
            failures.append(
                f"{key}: baseline entry names nothing that exists — run --write"
            )
            continue
        if ceiling > subject.cap and not (
            isinstance(issue, str) and _ISSUE_RE.match(issue)
        ):
            failures.append(
                f"{key}: ceiling {ceiling:,} is above the {subject.cap:,} cap and "
                f"names no retiring issue (got {issue!r})"
            )

    for subject in subjects:
        entry = exceptions.get(subject.key)
        ceiling = entry.get("ceiling") if entry is not None else None
        if not isinstance(ceiling, int):
            ceiling = subject.cap
        limit = max(subject.cap, ceiling)
        if subject.size > limit:
            if limit == subject.cap:
                why = f"cap {subject.cap:,}"
            else:
                why = f"frozen until {entry.get('issue')}"
            failures.append(
                f"{subject.key}: {subject.size:,} bytes > {limit:,} ({why})"
            )
        elif entry is not None and subject.size < ceiling:
            notices.append(
                f"{subject.key}: {subject.size:,} bytes, below its baseline "
                "ceiling — run --write to tighten it"
            )
    return failures, notices


def write(subjects: list[Subject], exceptions: dict[str, dict]) -> dict[str, dict]:
    """Tighten the baseline: lower ceilings, drop entries no longer needed.

    Never raises a ceiling and never adds an entry. A subject that has grown
    past its ceiling keeps the old one, so ``--check`` keeps failing it — the
    fix is to shrink the file, not to re-baseline it.
    """
    by_key = {subject.key: subject for subject in subjects}
    tightened: dict[str, dict] = {}
    for key, entry in exceptions.items():
        subject = by_key.get(key)
        if subject is None or subject.size <= subject.cap:
            continue
        tightened[key] = {
            "ceiling": min(entry["ceiling"], subject.size),
            "issue": entry["issue"],
        }
    return tightened


def admit(
    subjects: list[Subject], exceptions: dict[str, dict], requests: list[str]
) -> tuple[dict[str, dict], list[str]]:
    """Add explicitly named over-cap subjects at their current size.

    Each request is ``KEY=ENG-###``. Refuses a subject that is under its cap
    (nothing to freeze), one already in the baseline (that would be a raise),
    or a malformed issue reference.
    """
    by_key = {subject.key: subject for subject in subjects}
    admitted = dict(exceptions)
    errors: list[str] = []
    for request in requests:
        key, sep, issue = request.rpartition("=")
        subject = by_key.get(key)
        if not sep or not _ISSUE_RE.match(issue):
            errors.append(f"{request}: expected KEY=ENG-###")
        elif subject is None:
            errors.append(f"{key}: no such subject")
        elif key in exceptions:
            errors.append(f"{key}: already in the baseline; ceilings are never raised")
        elif subject.size <= subject.cap:
            errors.append(
                f"{key}: {subject.size:,} bytes is within its cap; nothing to admit"
            )
        else:
            admitted[key] = {"ceiling": subject.size, "issue": issue}
    return admitted, errors


def report(root: Path, subjects: list[Subject], show_all: bool) -> list[str]:
    """Per-skill sizes, largest entry file first, siblings reported not capped."""
    by_key = {subject.key: subject for subject in subjects}
    rows: list[tuple[int, str]] = []
    for skill_dir in sorted(p for p in (root / SKILLS_DIR).iterdir() if p.is_dir()):
        entry_key = f"{SKILLS_DIR}/{skill_dir.name}/{ENTRY_NAME}"
        entry = by_key.get(entry_key)
        description = by_key.get(entry_key + DESCRIPTION_SUFFIX)
        siblings = [
            p for p in skill_dir.rglob("*") if p.is_file() and p.name != ENTRY_NAME
        ]
        sibling_bytes = sum(p.stat().st_size for p in siblings)
        entry_size = entry.size if entry else 0
        flag = "*" if entry and entry.size > entry.cap else " "
        rows.append(
            (
                entry_size,
                f"{flag} {skill_dir.name:<24} entry {entry_size:>8,}  "
                f"desc {description.size if description else 0:>6,}  "
                f"siblings {sibling_bytes:>8,} ({len(siblings)})",
            )
        )
    rows.sort(key=lambda row: -row[0])
    limit = None if show_all else 10
    lines = [line for _, line in rows[:limit]]
    if limit is not None and len(rows) > limit:
        lines.append(f"  … {len(rows) - limit} more (--all)")
    project = by_key.get(PROJECT_FILE)
    if project is not None:
        lines.append(f"  {PROJECT_FILE:<24} {project.size:>14,}  (cap {project.cap:,})")
    lines.append(
        f"  caps: entry {ENTRY_CAP:,}, description {DESCRIPTION_CAP:,}, "
        f"project {PROJECT_CAP:,}; * = entry over cap"
    )
    return lines


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="skill_size.py",
        description="Hard byte caps on skill entry files, descriptions and CLAUDE.md.",
    )
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--check", action="store_true", help="fail on any over-limit subject (default)"
    )
    mode.add_argument(
        "--write", action="store_true", help="lower ceilings and drop retired entries"
    )
    mode.add_argument(
        "--report", action="store_true", help="per-skill sizes, siblings included"
    )
    parser.add_argument(
        "--admit",
        action="append",
        default=[],
        metavar="KEY=ENG-###",
        help="with --write: freeze a new over-cap subject at its current size",
    )
    parser.add_argument(
        "--all", action="store_true", help="with --report: every skill, not the top 10"
    )
    parser.add_argument(
        "--root", type=Path, default=Path(__file__).resolve().parents[2]
    )
    parser.add_argument("--baseline", type=Path, default=None)
    args = parser.parse_args(argv)

    root: Path = args.root
    baseline_path: Path = args.baseline or root / DEFAULT_BASELINE
    subjects = collect(root)
    exceptions = load_baseline(baseline_path)

    if args.admit and not args.write:
        parser.error("--admit requires --write")

    if args.report:
        print("\n".join(report(root, subjects, args.all)))
        return 0

    if args.write:
        exceptions, errors = admit(subjects, exceptions, args.admit)
        if errors:
            for error in errors:
                print(f"skill-size: {error}", file=sys.stderr)
            return 2
        tightened = write(subjects, exceptions)
        dump_baseline(baseline_path, tightened)
        print(
            f"skill-size: {len(tightened)} frozen exception(s) in {baseline_path.name}"
        )
        return 0

    failures, notices = check(subjects, exceptions)
    for notice in notices:
        print(f"skill-size: note: {notice}", file=sys.stderr)
    if not failures:
        return 0
    for failure in failures:
        print(f"skill-size: {failure}", file=sys.stderr)
    print(
        f"\nskill-size: {len(failures)} subject(s) over their resident-size limit. "
        "Move material into a sibling read at its trigger, or compress it — "
        "never raise a baseline ceiling by hand.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
