#!/usr/bin/env python3
"""Enqueue-time guard: does this branch's new migration number collide with
another open PR's?

**Why this cannot be caught in-tree.** The repo's ascend guard and its schema
fence tests only ever see **one** tree, and they start from an **empty**
database — so a duplicate number is invisible until both files coexist, which
first happens on the merge queue's merge-group branch. Until then both PRs ride
green.

**And the dequeue is the cheap half.** The shared dev Postgres may already have
one branch's number applied, and an applied migration is immutable (sqlx records
a checksum), so renumbering the **wrong** branch wedges the shared database. The
only fixes then are manual surgery or a wipe — and a wipe destroys collected
market data outside the venues' backfill windows, which is unrecoverable.

That asymmetry is the whole argument for a check here rather than a fix later,
and it gives the tiebreak its direction:

    **The branch whose number is already applied to the shared dev DB keeps it;
    the other renumbers.**

Real instance: two in-flight PRs each added a migration numbered ``0003`` —
maker telemetry and the pyth roster. Resolved by hand under exactly that rule
(the first kept ``0003``, the second became ``0004``).

Usage::

    # This branch's new migrations, from the merge-base with the base ref:
    python3 .claude/tools/migration_collisions.py --others <file.json>

``--others`` is a JSON array the caller assembles from the GitHub MCP — this
tool makes **no** network call, so the compare stays deterministic and testable
and the network read stays where the convention puts it::

    [{"pr": 351, "files": ["db-schema/migrations/0004_pyth.sql"]}, …]

Prints JSON and exits **non-zero on a collision**, so a caller that checks only
the status still cannot enqueue through one.

Stdlib only. A Python skill-tool under ``.claude/tools/`` — deliberately **not**
a Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

# Where migrations live, relative to the repo root.
DEFAULT_DIR = "db-schema/migrations"

# What `--others` is compared against by default.
DEFAULT_BASE = "origin/main"

# `0004_spot_ticks.sql` -> 4. Anchored at the basename so a directory component
# that happens to start with digits cannot be read as the version.
_NUMBER_RE = re.compile(r"^(\d+)_")


class MigrationCollisionsError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def migration_number(path: str) -> int | None:
    """The version a migration filename encodes, or ``None`` if it encodes none.

    Returns ``None`` rather than raising for a non-migration path, so a caller
    can pass a whole file list without pre-filtering.
    """
    match = _NUMBER_RE.match(Path(path).name)
    return int(match.group(1)) if match else None


def added_migrations(base_ref: str = DEFAULT_BASE, directory: str = DEFAULT_DIR):
    """This branch's **added** migration paths, from the merge-base with
    ``base_ref``.

    ``--diff-filter=A`` is the point: a branch that *edits* an existing
    migration has a different (worse) problem — editing an applied migration
    breaks its checksum — and it is not a numbering collision, so it must not
    be reported as one.
    """
    base = subprocess.run(
        ["git", "merge-base", "HEAD", base_ref],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    out = subprocess.run(
        ["git", "diff", "--name-only", "--diff-filter=A", "-z", base, "--", directory],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    return sorted({p for p in out.split("\0") if p.strip()})


def load_others(path: str) -> list[dict]:
    """Parse the ``--others`` payload, rejecting a shape that would silently
    compare nothing."""
    try:
        raw = Path(path).read_text(encoding="utf-8")
    except OSError as exc:
        raise MigrationCollisionsError(f"cannot read {path}: {exc}") from exc
    try:
        data = json.loads(raw)
    except ValueError as exc:
        raise MigrationCollisionsError(f"{path} is not valid JSON: {exc}") from exc
    if not isinstance(data, list):
        raise MigrationCollisionsError(
            f"{path} must hold a JSON array of "
            '{"pr": <number>, "files": [<path>, …]} objects'
        )
    for entry in data:
        if not isinstance(entry, dict) or "pr" not in entry:
            raise MigrationCollisionsError(
                f"{path}: every entry needs a `pr` key; got {entry!r}"
            )
    return data


def collisions(mine: list[str], others: list[dict]) -> list[dict]:
    """Every (my migration, their migration) pair sharing a version number.

    Compares **numbers**, not filenames: two PRs adding ``0003_telemetry.sql``
    and ``0003_roster.sql`` collide, and comparing paths would miss it — which
    is the actual observed shape.
    """
    mine_by_number: dict[int, list[str]] = {}
    for path in mine:
        number = migration_number(path)
        if number is not None:
            mine_by_number.setdefault(number, []).append(path)

    found = []
    for entry in others:
        for path in entry.get("files") or []:
            number = migration_number(path)
            if number is None or number not in mine_by_number:
                continue
            for ours in mine_by_number[number]:
                found.append(
                    {
                        "number": number,
                        "pr": entry["pr"],
                        "ours": ours,
                        "theirs": path,
                    }
                )
    return sorted(found, key=lambda c: (c["number"], c["pr"], c["theirs"]))


def summarize(result: dict) -> str:
    """One human line — the verdict, and the tiebreak when it is needed."""
    if not result["mine"]:
        return "migration-collisions | this branch adds no migration — nothing to check"
    added = ", ".join(str(n) for n in result["mine_numbers"])
    if not result["collisions"]:
        return (
            f"migration-collisions | adds {added} | no collision across "
            f"{result['prs_checked']} open PR(s) — safe to enqueue"
        )
    pairs = "; ".join(
        f"{c['number']} also in PR #{c['pr']} ({c['theirs']})"
        for c in result["collisions"]
    )
    return (
        f"migration-collisions | adds {added} | COLLISION: {pairs} | do not "
        f"enqueue — the branch whose number is already applied to the shared "
        f"dev DB keeps it, the other renumbers"
    )


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="migration_collisions.py",
        description=(
            "Compare this branch's new migration numbers against other open "
            "PRs' before enqueueing. Exits non-zero on a collision."
        ),
    )
    parser.add_argument(
        "--others",
        required=True,
        help="JSON file: [{'pr': N, 'files': [path, …]}, …], assembled by the "
        "caller from the GitHub MCP (this tool makes no network call)",
    )
    parser.add_argument(
        "--base",
        default=DEFAULT_BASE,
        help=f"the ref to take the merge-base against (default: {DEFAULT_BASE})",
    )
    parser.add_argument(
        "--dir",
        default=DEFAULT_DIR,
        dest="directory",
        help=f"the migrations directory (default: {DEFAULT_DIR})",
    )
    args = parser.parse_args(argv[1:])

    others = load_others(args.others)
    mine = added_migrations(args.base, args.directory)
    found = collisions(mine, others)
    result = {
        "mine": mine,
        "mine_numbers": sorted(
            {n for n in (migration_number(p) for p in mine) if n is not None}
        ),
        "prs_checked": len(others),
        "collisions": found,
        "clear": not found,
    }
    json.dump(result, sys.stdout, indent=2)
    sys.stdout.write("\n")
    print(summarize(result), file=sys.stderr)
    return 0 if result["clear"] else 1


def main() -> int:
    try:
        return run(sys.argv)
    except MigrationCollisionsError as exc:
        print(f"migration-collisions: {exc}", file=sys.stderr)
        return 2
    except subprocess.CalledProcessError as exc:
        detail = (exc.stderr or "").strip() or f"exit {exc.returncode}"
        print(f"migration-collisions: git failed: {detail}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
