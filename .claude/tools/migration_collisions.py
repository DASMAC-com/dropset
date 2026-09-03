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

    # Preferred — fetch the open-PR inventory in-process:
    python3 .claude/tools/migration_collisions.py --others-from-gh

    # Or compare against a file the caller assembled:
    python3 .claude/tools/migration_collisions.py --others <file.json>

``--others-from-gh`` runs the `gh pr list` read **inside this process**. It
exists because the two-command form had an unwritable gap: a redirect or pipe
is a compound the shell guard blocks, and capturing the output to re-emit it
with the Write tool routes every open PR's file list through context — the
exact cost this tool exists to avoid (~4.0k for a two-line answer). This is
the same in-process shape ``review_diff.py --overlap`` already uses.

``--others`` remains for a caller that already holds the inventory, and keeps
the compare deterministic and testable with no network call at all::

    [{"pr": 351, "files": ["db-schema/migrations/0004_pyth.sql"]}, …]

Prints JSON and exits **non-zero on a collision**, so a caller that checks only
the status still cannot enqueue through one.

**The bound: this compares against *open* PRs only.** A sibling that already
**merged** is not in the ``--others`` set, and its migration is not in this
branch's tree either until the branch rebases — so a collision with a merged
sibling passes this check cleanly. That gap is covered from the other side and
deliberately not duplicated here: the rebase in ``review-pr`` step 2 pulls the
merged file into the tree, at which point the in-tree ascend guard sees both
numbers and fires, and the merge queue's merge-group branch is a second
backstop. The case this tool exists for is the one neither of those can reach —
two PRs open *simultaneously*, each green, neither containing the other's file.

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

# NOTE ON SCOPE, so the summary line is not read as broader than the check.
# A number already MERGED to the base ref since this branch's merge-base is in
# neither set — not in `mine` (the file is not on this branch) and not in
# `--others` (that PR is closed). The in-tree ascend guard catches that case at
# rebase or on the merge-group branch, which is why it is left uncovered here
# rather than adding a third comparison.

# Where migrations live, relative to the repo root.
DEFAULT_DIR = "db-schema/migrations"

# What `--others` is compared against by default.
DEFAULT_BASE = "origin/main"

# `0004_spot_ticks.sql` -> 4. Anchored at the basename so a directory component
# that happens to start with digits cannot be read as the version, and required
# to end in `.sql` so only an actual migration counts.
#
# The extension is load-bearing, not decoration. The migrations directory also
# holds a `<version>_<name>.fence` manifest beside each migration
# (db-schema/tests/schema_fence.rs), and matching on the version prefix alone
# read all nine of them as added migrations — reporting a branch that touched no
# SQL at all as "adds 1, 2, 3, 4, 5, 6, 7, 8, 9". That direction is fail-safe
# (extra numbers can only invent a collision, never hide one), but a fabricated
# collision blocks an enqueue, and correcting a manifest is a supported edit.
_NUMBER_RE = re.compile(r"^(\d+)_.*\.sql$")


class MigrationCollisionsError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def migration_number(path: str) -> int | None:
    """The version a migration filename encodes, or ``None`` if it encodes none.

    Returns ``None`` rather than raising for a non-migration path, so a caller
    can pass a whole file list without pre-filtering.
    """
    match = _NUMBER_RE.match(Path(path).name)
    return int(match.group(1)) if match else None


def repo_root() -> str:
    """The working tree's top level.

    Every git call below is pinned here rather than trusting the caller's cwd,
    because ``directory`` is a **pathspec** and git resolves a pathspec
    relative to the *current directory*, not the repo root — while
    ``DEFAULT_DIR`` is documented (and written) as root-relative.

    That mismatch fails in the worst possible direction for a gate. Run from
    any subdirectory, ``git diff … -- db-schema/migrations`` matches nothing,
    ``mine`` is empty, nothing can collide, and the tool reports
    ``clear: true`` and **exits 0** — a mis-invoked run is indistinguishable
    from a genuinely clean branch. ``git merge-base`` works from anywhere, so
    nothing fails loudly to give the mistake away.
    """
    return subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()


def added_migrations(base_ref: str = DEFAULT_BASE, directory: str = DEFAULT_DIR):
    """This branch's **added** migration paths, from the merge-base with
    ``base_ref``.

    ``--diff-filter=A`` is the point: a branch that *edits* an existing
    migration has a different (worse) problem — editing an applied migration
    breaks its checksum — and it is not a numbering collision, so it must not
    be reported as one.

    Only paths that actually encode a version are returned. The pathspec is a
    directory, and that directory holds sidecars as well as migrations, so
    filtering here is what keeps ``mine`` a list of migrations rather than a
    list of everything that was added next to one — which the summary line
    reads directly to decide whether the branch adds a migration at all.

    Refuses a ``directory`` that does not exist at the repo root. An absent
    migrations directory means the pathspec is wrong, and the honest answer to
    "did anything collide?" is then an error, not "no".
    """
    root = repo_root()
    if not Path(root, directory).is_dir():
        raise MigrationCollisionsError(
            f"no migrations directory at {directory!r} (relative to {root}) — "
            f"refusing to report 'clear' from a pathspec that matches nothing"
        )
    base = subprocess.run(
        ["git", "merge-base", "HEAD", base_ref],
        capture_output=True,
        text=True,
        check=True,
        cwd=root,
    ).stdout.strip()
    out = subprocess.run(
        ["git", "diff", "--name-only", "--diff-filter=A", "-z", base, "--", directory],
        capture_output=True,
        text=True,
        check=True,
        cwd=root,
    ).stdout
    return sorted(
        {p for p in out.split("\0") if p.strip() and migration_number(p) is not None}
    )


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
        files = entry.get("files")
        # `files` may be absent or empty — a PR touching no migration is the
        # common case, and `collisions` handles both. But a **string** must be
        # refused: it is a plausible slip when hand-assembling a one-file PR
        # from the GitHub read, and it fails silently open — iterating a string
        # yields its characters, each of which `migration_number` maps to None,
        # so nothing collides and the tool reports `clear`.
        if files is not None and not isinstance(files, list):
            raise MigrationCollisionsError(
                f"{path}: `files` must be a list of paths, got "
                f"{type(files).__name__} for PR {entry['pr']} — a bare string "
                f"would be iterated character by character and silently "
                f"collide with nothing"
            )
    return data


GH_OPEN_PRS = (
    "gh",
    "pr",
    "list",
    "--state",
    "open",
    "--json",
    "number,files",
    "--limit",
    "30",
)


def others_from_gh() -> list[dict]:
    """The open-PR file inventory, fetched **inside this process**.

    The step that drives this tool used to prescribe two commands with an
    unwritable gap between them::

        gh pr list --state open --json number,files --limit 30
        python3 .claude/tools/migration_collisions.py --others <file>.json

    Nothing connected them, and every sanctioned way to connect them is
    closed: a `>` redirect is a compound the shell guard blocks (and the
    worktree-isolation guard refused it first), a pipe likewise, and
    capturing the output to re-emit it with the Write tool routes the whole
    per-PR file list **through context** — the precise cost the step's own
    rationale says this tool exists to avoid (~4.0k for a two-line answer).

    So the read moves in here, which is how ``review_diff.py --overlap``
    already solves the identical problem: it intersects open-PR file lists
    in-process, precisely so the per-PR lists never reach context. The two
    steps should not disagree about this.
    """
    try:
        completed = subprocess.run(
            GH_OPEN_PRS, capture_output=True, text=True, check=False
        )
    except OSError as exc:
        raise MigrationCollisionsError(f"could not run `gh pr list`: {exc}") from exc
    if completed.returncode != 0:
        detail = (completed.stderr or "").strip().splitlines()
        raise MigrationCollisionsError(
            "`gh pr list` failed: " + (detail[-1] if detail else "no detail")
        )
    try:
        data = json.loads(completed.stdout or "[]")
    except ValueError as exc:
        raise MigrationCollisionsError(
            f"`gh pr list` did not return JSON: {exc}"
        ) from exc
    if not isinstance(data, list):
        raise MigrationCollisionsError("`gh pr list` did not return an array")

    # gh spells them `number` and a list of {path: …} objects; normalize to the
    # `--others` shape so exactly one comparison path exists downstream.
    out: list[dict] = []
    for entry in data:
        if not isinstance(entry, dict):
            continue
        files = [
            f.get("path")
            for f in (entry.get("files") or [])
            if isinstance(f, dict) and f.get("path")
        ]
        out.append({"pr": entry.get("number"), "files": files})
    return out


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
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument(
        "--others",
        help="JSON file: [{'pr': N, 'files': [path, …]}, …], assembled by the "
        "caller from the GitHub MCP",
    )
    source.add_argument(
        "--others-from-gh",
        action="store_true",
        help="fetch the open-PR file inventory in-process via `gh pr list` — "
        "the preferred form, because it leaves no unwritable gap between a "
        "network read and this compare, and keeps the per-PR file lists out "
        "of context entirely",
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

    others = others_from_gh() if args.others_from_gh else load_others(args.others)
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
