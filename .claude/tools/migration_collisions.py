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

**Three exit codes, and a caller must distinguish all three.** ``1`` is a
collision, ``0`` is any other ``status``, and ``2`` means the tool **could not
answer** — bad input, a `gh` failure, a truncated listing, a pathspec matching
nothing. So neither naive reading of the status is safe: gating on *non-zero*
reports an operational failure as a collision, and gating on *== 1* treats an
unanswered question as a clean bill of health and enqueues through it. Treat 2 as
a hold.

``status`` names three outcomes, and the exit code is 1 for the first only:

``collision``
    A number is claimed twice. Do not enqueue.
``clear``
    This branch adds a migration and nothing else open claims its number.
``nothing_claimed``
    This branch adds no migration, so **nothing was compared**. Exits 0, because
    the enqueue gate runs on every branch and most add no migration — but it is
    deliberately not spelled ``clear``, because a verdict that compared nothing
    is not a clean bill of health. ``next_free_number`` is what makes this
    outcome useful at branch time.

**Two ways this used to be wrong, both observed live** (ENG-1336):

*It failed closed on the caller's own PR.* Once a branch is pushed its own PR is
in the open-PR listing, so the tool compared the branch's migration against
itself and reported a collision **every time** — exiting non-zero, which is
exactly how a skill gates an enqueue, so it refused provably clean enqueues. At
least five firings across three sessions in one day, each interpreted by hand.
Fixed by dropping the caller's own PR from the comparison, matched on
``headRefName`` against the current branch.

Excluding self cannot hide a real collision: ``mine`` comes from the local tree,
which is authoritative for what is about to be enqueued, and any *other* PR
claiming the same number is still in the set. Where the branch cannot be
resolved (a detached HEAD), the tool reports ``self_branch: null`` and says in
the summary that the exclusion was not applied, rather than going quietly back
to the old behavior.

*It failed open before the file existed.* With no migration on the branch it
returned ``clear: true`` and exit 0 — which is the state ``init-pr`` invokes it
in **on purpose**, to claim a number *before* writing the file. So the all-clear
was vacuous exactly where it was load-bearing, and a real collision surfaced
only on a re-run afterwards. Fixed by the ``nothing_claimed`` status plus
``next_free_number``, so a number-claiming call gets an actionable answer instead
of a reassuring one.

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
        # Optional, and only used to recognize the caller's own PR. A caller
        # assembling the inventory by hand may not have it; without it that PR
        # simply is not self-excluded, which is the pre-fix behavior for that
        # entry alone rather than a failure.
        head = entry.get("headRefName")
        if head is not None and not isinstance(head, str):
            raise MigrationCollisionsError(
                f"{path}: `headRefName` must be a string, got "
                f"{type(head).__name__} for PR {entry['pr']}"
            )
    return data


# `gh pr list` defaults to 30 and truncates SILENTLY past its limit, which in a
# collision checker is a fail-open: the one PR that would have collided is the
# one dropped, and the tool reports "clear". The limit is therefore set well
# above any plausible open-PR count, and `others_from_gh` warns when the result
# comes back exactly at it — at that point truncation cannot be ruled out and
# "clear" is no longer a claim this tool can make quietly.
GH_PR_LIMIT = 200

GH_OPEN_PRS = (
    "gh",
    "pr",
    "list",
    "--state",
    "open",
    "--json",
    # `headRefName` identifies the caller's own PR, which must not be compared
    # against itself — see `exclude_self`. `isCrossRepository` is what keeps that
    # identification from over-reaching: the ref name is unqualified, so a fork's
    # PR from a same-named branch would otherwise read as ours.
    # `changedFiles` is the true file count, which `files` is checked against —
    # see the truncation refusal in `others_from_gh`.
    "number,files,headRefName,isCrossRepository,changedFiles",
    "--limit",
    str(GH_PR_LIMIT),
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
    if len(data) >= GH_PR_LIMIT:
        # Exactly at the limit means gh may have truncated, and the PR it
        # dropped could be the colliding one. Refuse rather than report a
        # "clear" that is no longer supported — a silent truncation in a
        # collision checker fails in the dangerous direction.
        raise MigrationCollisionsError(
            f"`gh pr list` returned {len(data)} PRs, at the --limit of "
            f"{GH_PR_LIMIT}, so the list may be truncated and a dropped PR "
            "could be the colliding one. Raise GH_PR_LIMIT and re-run."
        )

    # gh spells them `number` and a list of {path: …} objects; normalize to the
    # `--others` shape so exactly one comparison path exists downstream.
    out: list[dict] = []
    for entry in data:
        if not isinstance(entry, dict):
            continue
        raw_files = entry.get("files") or []
        # gh resolves a PR's `files` over GraphQL, which pages at 100. Past that
        # the array is silently short — and the migration it drops could be the
        # colliding one, so a truncated entry means this tool cannot answer at
        # all rather than that nothing collided. `changedFiles` is the true
        # count, so the two disagreeing is an exact truncation test.
        #
        # Not reachable in this repo today (the largest PR to date changed 62
        # files) and deliberately guarded anyway: the failure is silent, in the
        # fail-open direction, on a guard whose bypass is unrecoverable.
        changed = entry.get("changedFiles")
        if isinstance(changed, int) and len(raw_files) < changed:
            raise MigrationCollisionsError(
                f"PR #{entry.get('number')} reports {changed} changed files but "
                f"`gh` returned only {len(raw_files)} — the list is truncated, "
                f"so a dropped migration could be the colliding one. This check "
                f"cannot answer for that PR; inspect it by hand before enqueueing."
            )
        files = [
            f.get("path") for f in raw_files if isinstance(f, dict) and f.get("path")
        ]
        out.append(
            {
                "pr": entry.get("number"),
                "files": files,
                "headRefName": entry.get("headRefName"),
                "isCrossRepository": entry.get("isCrossRepository"),
            }
        )
    return out


def current_branch() -> str | None:
    """This worktree's branch name, or ``None`` on a detached HEAD.

    ``git rev-parse --abbrev-ref HEAD`` answers the literal string ``HEAD`` when
    detached, which is not a branch any PR can be open against — so it is mapped
    to ``None`` rather than passed on to match nothing while *looking* like a
    successful resolution.
    """
    name = subprocess.run(
        ["git", "rev-parse", "--abbrev-ref", "HEAD"],
        capture_output=True,
        text=True,
        check=True,
        cwd=repo_root(),
    ).stdout.strip()
    return None if name in ("", "HEAD") else name


def exclude_self(others: list[dict], self_branch: str | None):
    """Drop the caller's own PR from the comparison set.

    Returns ``(others_without_self, excluded_pr_numbers)``.

    **This is the fail-closed half of ENG-1336.** A pushed branch's own PR is in
    the open-PR listing, so comparing against it matched the branch's migration
    with itself and reported a collision on every run — non-zero, which is how a
    skill gates an enqueue.

    Matching on the branch rather than on the file list is deliberate: two PRs
    adding the *same* migration number is precisely the case this tool exists to
    catch, so "their files look like mine" is the one signal that must never mean
    "this is me".

    With ``self_branch`` as ``None`` nothing is excluded — the caller reports that
    in the summary, so an unapplied exclusion is visible rather than silent.
    """
    if self_branch is None:
        return list(others), []
    kept, dropped = [], []
    for entry in others:
        # A fork's PR reports its head ref **unqualified**, so `eng-1336` on a
        # fork compares equal to `eng-1336` here. Excluding it would drop a
        # genuinely-colliding PR rather than the self-match — the one new
        # fail-open this exclusion could introduce — so a cross-repository entry
        # is never us, whatever its branch is called. This repo is public, so
        # that is a reachable input rather than a hypothetical one.
        if entry.get("isCrossRepository"):
            kept.append(entry)
        elif entry.get("headRefName") == self_branch:
            dropped.append(entry.get("pr"))
        else:
            kept.append(entry)
    return kept, dropped


def tree_numbers(directory: str = DEFAULT_DIR) -> list[int]:
    """Every migration version already present in the working tree.

    Read from the directory rather than from git, so a migration this branch has
    written but not yet committed still counts as taken.
    """
    root = repo_root()
    path = Path(root, directory)
    if not path.is_dir():
        raise MigrationCollisionsError(
            f"no migrations directory at {directory!r} (relative to {root})"
        )
    return sorted(
        {
            n
            for n in (migration_number(child.name) for child in path.iterdir())
            if n is not None
        }
    )


def next_free_number(taken) -> int:
    """The lowest number that collides with nothing: one past the highest taken.

    **One past the maximum, not the first gap.** A gap is a number that some tree
    or some PR may still be holding out of this tool's sight — a sibling that
    merged since the merge-base is in neither comparison set (see the bound
    above) — whereas one past the maximum is unclaimed in every set that was
    consulted. Filling a gap is also what produces the out-of-order applied
    state; that state is *tolerated* by the runner
    (`db-schema/tests/schema_fence.rs`), so this is about keeping the claim
    verifiable, not about avoiding a wedge.
    """
    numbers = set(taken)
    return max(numbers) + 1 if numbers else 1


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


def _self_note(result: dict) -> str:
    """How the caller's own PR was treated, so the reader can trust the count.

    Every outcome says something. An exclusion that silently matched nothing is
    what re-creates the original misread — the self-match survives into
    ``collisions`` and prints as an ordinary COLLISION with no hint that it might
    be this branch's own PR.
    """
    if result["self_branch"] is None:
        return (
            " | NOTE: detached HEAD, so this branch's own PR could not be "
            "identified and was not excluded — a collision naming only one PR "
            "whose files match this branch's is probably that self-match"
        )
    override = " (from --self-branch)" if result["self_branch_explicit"] else ""
    if result["self_prs"]:
        excluded = ", ".join(f"#{n}" for n in result["self_prs"])
        return f" | excluding this branch's own PR {excluded}{override}"
    if result["collisions"]:
        return (
            f" | NOTE: no PR was excluded — nothing in the listing has head "
            f"branch {result['self_branch']!r}{override}, so if this branch's own "
            f"PR is open under a different head ref, one of these collisions is "
            f"probably that self-match"
        )
    return f" | no PR excluded (nothing matched this branch){override}"


def summarize(result: dict) -> str:
    """One human line — the verdict, and the tiebreak when it is needed."""
    if result["status"] == "nothing_claimed":
        return (
            "migration-collisions | this branch adds no migration, so nothing "
            f"was compared — NOT a clear verdict | next free number: "
            f"{result['next_free_number']:04d} | if this task adds one, take "
            f"that number and re-run once the file exists"
        )
    added = ", ".join(str(n) for n in result["mine_numbers"])
    if result["status"] == "clear":
        return (
            f"migration-collisions | adds {added} | no collision across "
            f"{result['prs_checked']} open PR(s){_self_note(result)} — safe to "
            f"enqueue"
        )
    pairs = "; ".join(
        f"{c['number']} also in PR #{c['pr']} ({c['theirs']})"
        for c in result["collisions"]
    )
    return (
        f"migration-collisions | adds {added} | COLLISION: {pairs}"
        f"{_self_note(result)} | do not enqueue — the branch whose number is "
        f"already applied to the shared dev DB keeps it, the other renumbers"
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
    parser.add_argument(
        "--self-branch",
        default=None,
        help="the branch whose own PR must be excluded from the comparison "
        "(default: this worktree's current branch). Only needed on a detached "
        "HEAD, or to reproduce the compare offline.",
    )
    args = parser.parse_args(argv[1:])

    all_others = others_from_gh() if args.others_from_gh else load_others(args.others)
    # `is not None`, not `or`: an empty `--self-branch ""` must not read as unset
    # and silently fall back to git, which would make an offline `--others`
    # compare depend on whatever branch the invoking worktree happens to be on.
    self_branch_explicit = args.self_branch is not None
    self_branch = args.self_branch if self_branch_explicit else current_branch()
    others, self_prs = exclude_self(all_others, self_branch)

    mine = added_migrations(args.base, args.directory)
    mine_numbers = sorted(
        {n for n in (migration_number(p) for p in mine) if n is not None}
    )
    found = collisions(mine, others)

    # Every number this tool can see as taken: already in the tree, claimed by
    # an open PR, or added by this branch. `others` here is post-exclusion, so the
    # caller's own PR contributes only through `tree_numbers` (the file is on
    # disk) or `mine_numbers` (it is added against the merge-base). Both lapse if
    # the branch pushed a migration and then deleted it locally, so this can hand
    # back a number that branch's own open PR still claims — a self-claim, so the
    # practical harm is nil, but it is not the unconditional guarantee it looks
    # like.
    taken = set(tree_numbers(args.directory)) | set(mine_numbers)
    for entry in others:
        for path in entry.get("files") or []:
            number = migration_number(path)
            if number is not None:
                taken.add(number)

    result = {
        "mine": mine,
        "mine_numbers": mine_numbers,
        "prs_checked": len(others),
        "self_branch": self_branch,
        "self_branch_explicit": self_branch_explicit,
        "self_prs": self_prs,
        "collisions": found,
        "next_free_number": next_free_number(taken),
        # `clear` is kept for a caller that already reads it, and keeps its
        # original meaning: no collision was found. `status` is what
        # distinguishes "checked and clean" from "checked nothing".
        "clear": not found,
        "status": "collision" if found else ("clear" if mine else "nothing_claimed"),
    }
    json.dump(result, sys.stdout, indent=2)
    sys.stdout.write("\n")
    print(summarize(result), file=sys.stderr)
    # Non-zero for a collision and nothing else. `nothing_claimed` exits 0
    # because the enqueue gate runs on every branch and most add no migration —
    # blocking there would make the guard's adaptation "ignore the exit status",
    # which deletes the protection outright.
    return 1 if result["status"] == "collision" else 0


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
