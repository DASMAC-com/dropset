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

**One cap, frozen exceptions, a headroom ratchet.** A committed baseline
(``cfg/skill-size-baseline.json``) names every subject over its cap with a
``ceiling`` and the issue that retires it. ``--check`` fails any subject larger
than the **greater** of cap and ceiling, so an over-cap file cannot grow past
its ceiling and an under-cap file — a stale entry's included — may grow to the
cap. (The filing said "lesser", which would fail every frozen exception on
enable day; the two behaviors it describes need the greater.) ``--write`` only
ever *lowers* a ceiling — to ``ceil(size × 1.10)``, never below the old one
— or drops an entry whose subject is under its cap or gone; it never raises
one and never adds one. The 10 percent is **headroom**: a ceiling written at
the shrunk size would leave every compressed file at 100 percent, and the next
writer on it nothing. Admitting a new exception is a separate, explicit act
(``--admit``, at the current size) that must name its retiring issue, and a
review surfaces each one; a *raised* ceiling is a blocking review finding,
since only a hand edit can produce one.

``--utilization`` prints size, limit and percent for every subject and exits
nonzero listing each one above the watch threshold, so a file nearing its
limit becomes a compression task before it blocks a commit.

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

# `--write` sets a lowered ceiling this many percent above the current size.
HEADROOM_PERCENT = 10
# `--utilization` flags a subject above this percent of its limit. Above
# 100 / 1.10 ≈ 90.9, so a freshly ratcheted file reads clean; it flags once
# about half its headroom is spent.
WATCH_PERCENT = 95

PROJECT_FILE = "CLAUDE.md"
SKILLS_DIR = ".claude/skills"
ENTRY_NAME = "SKILL.md"
DESCRIPTION_SUFFIX = "#description"
DEFAULT_BASELINE = "cfg/skill-size-baseline.json"

# A retiring-issue reference. Data, not prose, so it carries the tag.
_ISSUE_RE = re.compile(r"^ENG-[0-9]+$")

# YAML block-scalar header: `>` or `|`, then an optional chomping indicator and
# an optional indentation digit (1-9) in either order (`>-`, `|+`, `>2`, `>2-`,
# `>-2`), then an optional trailing comment.
_BLOCK_SCALAR_RE = re.compile(r"^[>|](?:[1-9][+-]?|[+-][1-9]?)?(?:\s+#.*)?$")


class Subject(NamedTuple):
    """One capped thing: a file, or one skill's description."""

    key: str
    size: int
    cap: int


def headroom_ceiling(size: int) -> int:
    """``ceil(size × (1 + HEADROOM_PERCENT / 100))``, in integer arithmetic."""
    return -(-size * (100 + HEADROOM_PERCENT) // 100)


def description_value(text: str) -> str | None:
    """Return the frontmatter ``description`` value of a ``SKILL.md``.

    Handles the plain one-line form every skill uses today and every way a
    writer may wrap a long one: a block scalar (``>`` / ``|`` header), a plain
    scalar continued on indented lines, or an empty value followed by indented
    text. Each is folded with the indented lines that follow, up to the next
    unindented line or the closing ``---``, because under-measuring a wrapped
    description is the one way this gate could fail open. ``None`` when there
    is no frontmatter or no key.
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
        parts = [] if _BLOCK_SCALAR_RE.match(value) else [value]
        for follow in lines[index + 1 :]:
            if follow.strip() == "---" or (follow and not follow[0].isspace()):
                break
            parts.append(follow.strip())
        return " ".join(part for part in parts if part)
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


def load_baseline(path: Path) -> tuple[dict[str, dict], list[str]]:
    """Read the baseline's exceptions, returning ``(exceptions, errors)``.

    A missing file means no exceptions. Every shape problem — a non-object
    root or entry, a ceiling that is not a non-negative integer, an issue that
    is not a string — is returned as an error and its entry left out, so
    ``--check`` reports it as a failure and ``--write`` refuses to run rather
    than either one crashing. Leaving a malformed entry out fails closed: its
    subject is held to the plain cap.
    """
    if not path.is_file():
        return {}, []
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        return {}, [f"{path.name}: not readable as JSON ({error})"]
    raw = data.get("exceptions", {}) if isinstance(data, dict) else None
    if not isinstance(raw, dict):
        return {}, [f"{path.name}: expected an object with an `exceptions` object"]
    exceptions: dict[str, dict] = {}
    errors: list[str] = []
    for key, entry in raw.items():
        ceiling = entry.get("ceiling") if isinstance(entry, dict) else None
        issue = entry.get("issue") if isinstance(entry, dict) else None
        # `bool` is an `int` subclass, so JSON `true` would read as 1.
        if not isinstance(ceiling, int) or isinstance(ceiling, bool) or ceiling < 0:
            errors.append(f"{key}: baseline ceiling {ceiling!r} is not a byte count")
        elif not isinstance(issue, str):
            errors.append(f"{key}: baseline issue {issue!r} is not a string")
        else:
            exceptions[key] = {"ceiling": ceiling, "issue": issue}
    return exceptions, errors


def dump_baseline(path: Path, exceptions: dict[str, dict]) -> None:
    """Write the baseline deterministically (sorted keys, trailing newline).

    Only ``ceiling`` and ``issue`` are written: a hand-added field does not
    survive ``--write``, so put notes in the retiring issue, not the file.
    """
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

    A failure is a subject over its limit, an over-cap ceiling naming no
    retiring issue, or an entry naming a subject that no longer exists
    (malformed entries are reported by ``load_baseline``). A notice is an entry
    ``--write`` would change: one whose subject is now within its cap (dropped),
    or whose ceiling is above the headroom ceiling of its current size
    (lowered). Both are harmless until then — the slack can be regrown, but
    never past the ceiling.
    """
    failures: list[str] = []
    notices: list[str] = []
    by_key = {subject.key: subject for subject in subjects}

    for key in sorted(exceptions):
        ceiling = exceptions[key]["ceiling"]
        issue = exceptions[key]["issue"]
        subject = by_key.get(key)
        if subject is None:
            failures.append(
                f"{key}: baseline entry names nothing that exists — run --write"
            )
            continue
        if ceiling > subject.cap and not _ISSUE_RE.match(issue):
            failures.append(
                f"{key}: ceiling {ceiling:,} is above the {subject.cap:,} cap and "
                f"names no retiring issue (got {issue!r})"
            )

    for subject in subjects:
        entry = exceptions.get(subject.key)
        ceiling = subject.cap if entry is None else entry["ceiling"]
        limit = max(subject.cap, ceiling)
        if subject.size > limit:
            if limit == subject.cap:
                why = f"cap {subject.cap:,}"
            else:
                why = f"frozen until {entry['issue']}"
            failures.append(
                f"{subject.key}: {subject.size:,} bytes > {limit:,} ({why})"
            )
        elif entry is not None and subject.size <= subject.cap:
            notices.append(
                f"{subject.key}: {subject.size:,} bytes, within its "
                f"{subject.cap:,} cap — run --write to drop its baseline entry"
            )
        elif entry is not None and headroom_ceiling(subject.size) < ceiling:
            notices.append(
                f"{subject.key}: {subject.size:,} bytes, more than "
                f"{HEADROOM_PERCENT}% below its baseline ceiling — run --write "
                "to tighten it"
            )
    return failures, notices


def write(subjects: list[Subject], exceptions: dict[str, dict]) -> dict[str, dict]:
    """Tighten the baseline: lower ceilings, drop entries no longer needed.

    A lowered ceiling is the headroom ceiling of the current size, so a shrink
    leaves room for the next writer. Drops an entry whose subject is within its
    cap or no longer exists. Never raises a ceiling and never adds an entry. A
    subject that has grown past its ceiling keeps the old one, so ``--check``
    keeps failing it — the fix is to shrink the file, not to re-baseline it.
    """
    by_key = {subject.key: subject for subject in subjects}
    tightened: dict[str, dict] = {}
    for key, entry in exceptions.items():
        subject = by_key.get(key)
        if subject is None or subject.size <= subject.cap:
            continue
        tightened[key] = {
            "ceiling": min(entry["ceiling"], headroom_ceiling(subject.size)),
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
        elif key in admitted:
            errors.append(
                f"{key}: already in the baseline; ceilings are never raised"
                if key in exceptions
                else f"{key}: named twice in one --admit run"
            )
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
    skills = root / SKILLS_DIR
    skill_dirs = (
        sorted(p for p in skills.iterdir() if p.is_dir()) if skills.is_dir() else []
    )
    for skill_dir in skill_dirs:
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


def utilization(
    subjects: list[Subject], exceptions: dict[str, dict]
) -> tuple[list[str], list[str]]:
    """Return ``(lines, flagged)``: every subject's use of its limit, and the
    keys above ``WATCH_PERCENT``.

    The limit is the one ``--check`` enforces, the greater of cap and
    ceiling. Lines run highest percent first.
    """
    rows: list[tuple[float, str, Subject, int]] = []
    for subject in subjects:
        entry = exceptions.get(subject.key)
        limit = max(subject.cap, entry["ceiling"] if entry else 0)
        rows.append((100 * subject.size / limit, subject.key, subject, limit))
    rows.sort(key=lambda row: (-row[0], row[1]))
    lines: list[str] = []
    flagged: list[str] = []
    for percent, key, subject, limit in rows:
        over = percent > WATCH_PERCENT
        if over:
            flagged.append(key)
        lines.append(
            f"{'!' if over else ' '} {percent:5.1f}%  {subject.size:>9,} / "
            f"{limit:>9,}  {key}"
        )
    return lines, flagged


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
    mode.add_argument(
        "--utilization",
        action="store_true",
        help=f"size / limit per subject; fail on any above {WATCH_PERCENT}%%",
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
    if args.admit and not args.write:
        parser.error("--admit requires --write")

    subjects = collect(root)
    if args.report:
        print("\n".join(report(root, subjects, args.all)))
        return 0

    exceptions, baseline_errors = load_baseline(baseline_path)
    if args.utilization:
        if baseline_errors:
            for error in baseline_errors:
                print(f"skill-size: {error}", file=sys.stderr)
            return 2
        lines, flagged = utilization(subjects, exceptions)
        print("\n".join(lines))
        if not flagged:
            return 0
        print(
            f"skill-size: {len(flagged)} subject(s) above {WATCH_PERCENT}% "
            "of their limit:",
            file=sys.stderr,
        )
        for key in flagged:
            print(f"skill-size:   {key}", file=sys.stderr)
        return 1

    if args.write:
        if baseline_errors:
            for error in baseline_errors:
                print(f"skill-size: {error}", file=sys.stderr)
            print("skill-size: fix the baseline by hand first", file=sys.stderr)
            return 2
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
    failures = baseline_errors + failures
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
