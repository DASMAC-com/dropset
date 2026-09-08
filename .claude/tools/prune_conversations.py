#!/usr/bin/env python3
# cspell:word followlinks
"""Reclaim disk from Claude Code's local state — old session transcripts and
adjacent caches — under an age rule with open-PR protection.

This is the deterministic filesystem core of the ``purge-conversations`` skill.
The skill drives the GitHub PR lookups over the MCP and hands this tool the set
of worktree branches with an **open** PR (``--protected-branch``); the tool does
the rest: discover the dropset worktrees, classify every entry under the three
local roots, print a grouped **dry-run manifest** (per-group + total MB), and —
only with ``--apply`` — hard-delete and report bytes freed.

Three roots, two mechanisms:

* **Slug-partitioned** — ``~/.claude/projects`` and
  ``~/Library/Caches/claude-cli-nodejs`` both name a subdirectory per working
  directory with the same ``slugify()`` scheme (every ``/`` and ``.`` → ``-``).
  A slug whose **worktree still exists** is kept unconditionally; a slug whose
  worktree is *gone* gets the age rule; a non-dropset slug is age-only.
* **Session-UUID** — ``~/.claude/file-history`` is one flat subdirectory per
  session UUID, mixing every repo. It is age-ruled by directory mtime, but no
  longer *only* that: the projects tree names each session as
  ``<slug>/<uuid>.jsonl``, so a session belonging to a slug we are keeping is
  kept here too. Both of a session's directories are protected together,
  because the loss below took both and half a fix is not one.

The **dropset set is derived forward** (``git worktree list`` → slug of each
real worktree path), never by string-matching slug prefixes — a prefix would
wrongly catch a sibling repo like ``dropset-beta`` whose slug starts with the
base repo's. The **current session is always kept** in every root (by session
id, and by the current working directory's slug).

**Why this tool refuses to run without a resolvable repo.** It used to treat a
missing ``--dropset-repo`` as "no worktrees exist", degrading to an age-only
sweep in which every live worktree's transcripts were classified — and reported
— as ``non-dropset``. On 2026-09-07 a ``housekeeping`` pass invoked it that way
and hard-deleted a live worktree session's transcript; the approval prompt read
``8.3 MB non-dropset transcripts``, so what was approved bore no resemblance to
what was lost. Three guards close that hole, and they are deliberately
belt-and-braces because the failure is unrecoverable:

1. the repo is **defaulted** from the working directory and the run **aborts**
   when it cannot be resolved, rather than proceeding with an empty set;
2. an existing worktree protects its slug **whatever the PR state** — an
   existing worktree means a session someone intends to resume;
3. the **prompt history** (``~/.claude/history.jsonl``) is cross-checked, so a
   slug with recent activity survives even a total failure of the PR lookup.

Every one of the three fails in the keep-more direction: the cost of a false
keep is disk, the cost of a false delete is a session that cannot be resumed.

One consequence worth stating, since it is a deliberate trade rather than an
oversight: guard 2 protects the **main** worktree too, so the base checkout's
project directory (the largest single slug dir here) is now never age-reclaimed
by this tool. In practice it never was — that directory holds every session ever
started from the base repo, so its mtime is refreshed by the newest of them and
the age rule effectively never fired on it. The change makes the outcome
explicit ("live worktree") instead of incidental ("within age").

Safety invariant: the tool only ever deletes a directory that resolves **under**
one of the three known roots, never follows a symlink, refuses any entry that
escapes its root, and never touches the current session. Dry-run is the default;
deletion requires ``--apply``. Standard library only.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

# Default age threshold (days). An entry whose directory mtime is older than
# this is eligible for the age rule; the skill can override with --age-days.
DEFAULT_AGE_DAYS = 2

SECONDS_PER_DAY = 86_400


class PruneError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


# --------------------------------------------------------------------------
# Roots and the shared slug scheme (ported from session_metrics.py so the two
# tools agree on where transcripts live and how a working dir maps to a slug).
# --------------------------------------------------------------------------


def claude_home() -> Path:
    """The Claude home directory: ``CLAUDE_CONFIG_DIR`` if set, else ``~/.claude``."""
    configured = os.environ.get("CLAUDE_CONFIG_DIR", "").strip()
    if configured:
        return Path(configured)
    home = os.environ.get("HOME")
    if not home:
        raise PruneError("neither CLAUDE_CONFIG_DIR nor HOME is set")
    return Path(home) / ".claude"


def slugify(path: Path) -> str:
    """Claude Code names each project's directory after the working directory,
    replacing every ``/`` and ``.`` with ``-`` — the same scheme the projects
    tree and the CLI cache both use."""
    return "".join("-" if c in "/." else c for c in str(path))


def projects_root() -> Path:
    return claude_home() / "projects"


def file_history_root() -> Path:
    return claude_home() / "file-history"


def history_path() -> Path:
    """``~/.claude/history.jsonl`` — one JSON object per submitted prompt,
    carrying the ``project`` (working directory) and an epoch-**milliseconds**
    ``timestamp``. Read only to establish recent activity; never written."""
    return claude_home() / "history.jsonl"


def cli_cache_root() -> Path:
    """``~/Library/Caches/claude-cli-nodejs`` — the CLI cache, slug-partitioned
    exactly like the projects tree."""
    home = os.environ.get("HOME")
    if not home:
        raise PruneError("HOME is not set")
    return Path(home) / "Library" / "Caches" / "claude-cli-nodejs"


# --------------------------------------------------------------------------
# Pure helpers — worktree parsing, the age/PR decision, and the path guard.
# --------------------------------------------------------------------------


def parse_worktrees(porcelain: str) -> list[tuple[str, str | None]]:
    """Parse ``git worktree list --porcelain`` into ``(path, branch)`` pairs.
    ``branch`` is the short name (``refs/heads/eng-663`` → ``eng-663``) or
    ``None`` for a detached worktree."""
    out: list[tuple[str, str | None]] = []
    path: str | None = None
    branch: str | None = None
    for line in porcelain.splitlines():
        if line.startswith("worktree "):
            if path is not None:
                out.append((path, branch))
            path = line[len("worktree ") :].strip()
            branch = None
        elif line.startswith("branch "):
            ref = line[len("branch ") :].strip()
            branch = ref[len("refs/heads/") :] if ref.startswith("refs/heads/") else ref
    if path is not None:
        out.append((path, branch))
    return out


def dropset_slug_sets(
    worktrees: list[tuple[str, str | None]], protected_branches: set[str]
) -> tuple[set[str], set[str]]:
    """From parsed worktrees, return ``(live_slugs, protected_slugs)``: every
    **existing** worktree path's slug, and the subset whose branch has an open
    PR.

    Both sets are kept, and the distinction is now only about the *reason*
    reported — ``git worktree list`` enumerates worktrees that exist, so
    membership in the first set is itself proof that a checkout is on disk.
    The PR subset survives because "open PR" is the more informative reason to
    show a human, and because it still protects a branch whose worktree has
    already been pruned away.
    """
    live: set[str] = set()
    protected: set[str] = set()
    for path, branch in worktrees:
        slug = slugify(Path(path))
        live.add(slug)
        if branch is not None and branch in protected_branches:
            protected.add(slug)
    return live, protected


def base_worktree(worktrees: list[tuple[str, str | None]]) -> Path:
    """The main worktree — ``git worktree list`` documents it as the first
    entry, ahead of every linked worktree. Preferred over "the one on ``main``"
    because a branch checkout is a convention while the ordering is a
    guarantee."""
    if not worktrees:
        raise PruneError("cannot identify the base repo from an empty worktree list")
    return Path(worktrees[0][0])


def former_worktree_prefix(base_repo: Path) -> str:
    """The slug prefix shared by every worktree under ``<base>/.claude/worktrees``
    — the directory ``claude --worktree`` creates them in.

    This is the one place a prefix comparison is legitimate, and it is safe for
    the reason the blanket ban exists: the ban protects against matching a
    *sibling repo* like ``dropset-beta`` off the base repo's own slug, and this
    prefix reaches a directory **inside** the base repo, which no sibling can
    share. It is used only to label and to name a tag, never to widen deletion:
    a former worktree created outside that directory simply falls through to the
    age-only ``non-dropset`` branch, exactly as it did before.
    """
    return slugify(base_repo / ".claude" / "worktrees") + "-"


def worktree_tag(slug: str, prefix: str | None) -> str | None:
    """The issue tag a worktree slug encodes (``…-worktrees-eng-1192`` →
    ``eng-1192``), or ``None`` when the slug is not a worktree of this repo.
    The manifest uses it so an approval reads as "delete the eng-1192 session"
    rather than as an opaque slug string."""
    if not prefix or not slug.startswith(prefix):
        return None
    return slug[len(prefix) :] or None


def read_active_slugs(cutoff_ts: float, path: Path | None = None) -> set[str]:
    """Slugs of every working directory named as the ``project`` of a prompt
    newer than ``cutoff_ts``, read from the prompt history.

    This is the guard that degrades safely. The open-PR protection is only as
    good as the GitHub lookup that feeds it, and the lookup is the part most
    likely to fail or to be handed an empty list — which is precisely the
    failure that deleted a live session. Recent prompts are local, need no
    network, and are direct evidence that a human was working somewhere.

    Best-effort by construction: a missing file, an unreadable one, or a
    malformed line yields fewer protected slugs but never an error, because a
    guard that can abort the run would itself become a reason to skip it.
    """
    src = path if path is not None else history_path()
    slugs: set[str] = set()
    try:
        text = src.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return slugs
    for line in text.splitlines():
        if not line.strip():
            continue
        try:
            record = json.loads(line)
        except ValueError:
            continue
        if not isinstance(record, dict):
            continue
        project = record.get("project")
        stamp = record.get("timestamp")
        if not isinstance(project, str) or not project:
            continue
        if isinstance(stamp, bool) or not isinstance(stamp, (int, float)):
            continue
        if stamp / 1000.0 < cutoff_ts:  # history timestamps are milliseconds
            continue
        slugs.add(slugify(Path(project)))
    return slugs


@dataclass
class Decision:
    delete: bool
    category: str  # dropset-old | non-dropset | kept
    reason: str


def decide_slug(
    slug: str,
    mtime_ts: float,
    *,
    live_slugs: set[str],
    protected_slugs: set[str],
    current_slug: str | None,
    cutoff_ts: float,
    completed_slugs: set[str] | None = None,
    active_slugs: set[str] | None = None,
    former_prefix: str | None = None,
) -> Decision:
    """Decide a slug-partitioned entry (projects or CLI cache).

    Four keep-rules are tried in order, then the age rule:

    1. the **current session** — always kept;
    2. an **open PR** on the slug's branch;
    3. a **live worktree** — the checkout still exists on disk;
    4. **recent prompt activity** against that working directory.

    Only then does a slug reach the age rule, and only a slug the caller marked
    *completed* skips the grace period within it.

    **Rule 3 is the fix for a data-loss bug and it is unconditional on
    purpose.** It used to be the *weakest* rule rather than a keep-rule at all:
    a dropset slug was age-ruled, and protection came solely from the open-PR
    list. That made every protection contingent on a network lookup landing
    correctly, and when a caller omitted the repo argument the worktree set was
    empty, no slug was recognized, and live sessions were deleted under the
    ``non-dropset`` label. An existing worktree is *local, free and
    unambiguous* evidence that a session is meant to be resumable, so it now
    protects on its own — whatever the PR says, and whatever the age.

    **Rule 4 sits above the completion override deliberately.** Completion is
    the caller's set arithmetic; recent prompts are the operator's own behavior.
    When the two disagree, believing the human costs at most a couple of days of
    disk, while believing the arithmetic can cost a transcript — so the weaker
    evidence does not get to override the stronger one. It shares the age
    cutoff rather than inventing a second window, so "recently active" and
    "within the grace period" mean the same span.

    Rules 1–3 keep their previous relative order, and each is checked before
    ``completed`` for the same reason as before: a slug landing in both sets is
    kept, so a bug in the caller's set arithmetic costs disk rather than data.

    ``former_prefix`` only changes the *label*: past the keep-rules, a slug
    recognizable as a pruned-away worktree of this repo is reported as
    ``dropset-old`` instead of ``non-dropset``. Both are age-ruled identically.
    That distinction exists because the mislabelling is what made the loss
    approvable — "non-dropset transcripts" is exactly what a human waves
    through.
    """
    completed = completed_slugs or set()
    active = active_slugs or set()
    if current_slug is not None and slug == current_slug:
        return Decision(False, "kept", "current session")
    if slug in protected_slugs:
        return Decision(False, "kept", "open PR")
    if slug in live_slugs:
        return Decision(False, "kept", "live worktree")
    if slug in active:
        return Decision(False, "kept", "recent session activity")
    if slug in completed:
        return Decision(True, "completed", "worktree gone, PR merged or closed")
    if former_prefix and slug.startswith(former_prefix):
        if mtime_ts < cutoff_ts:
            return Decision(True, "dropset-old", "worktree gone, older than threshold")
        return Decision(False, "kept", "worktree gone, within age")
    if mtime_ts < cutoff_ts:
        return Decision(True, "non-dropset", "non-dropset, older than threshold")
    return Decision(False, "kept", "non-dropset, within age")


def decide_history(
    name: str,
    mtime_ts: float,
    *,
    current_uuid: str | None,
    cutoff_ts: float,
    protected_uuids: set[str] | None = None,
) -> Decision:
    """Decide a file-history session-UUID directory: age rule, with the current
    session and any **protected session** kept.

    ``file-history`` is one flat directory per session UUID with no repo in the
    name, which is why it was age-only — there was nothing to join on. There is
    now: the projects tree stores each session as ``<slug>/<uuid>.jsonl``, so
    the sessions belonging to a slug we decided to keep can be named exactly,
    and the prompt history supplies the rest.

    This matters because the 2026-09-07 loss took **both** of a session's
    directories. Protecting only the transcript would leave the same session
    half-destroyable by the same blunt rule, which is not a fix.
    """
    protected = protected_uuids or set()
    if current_uuid is not None and name == current_uuid:
        return Decision(False, "kept", "current session")
    if name in protected:
        return Decision(False, "kept", "session of a kept project")
    if mtime_ts < cutoff_ts:
        return Decision(True, "file-history", "older than threshold")
    return Decision(False, "kept", "within age")


def is_within(root: Path, candidate: Path) -> bool:
    """True only when ``candidate`` resolves to a path **under** ``root`` (both
    real-path-resolved) — the guard that keeps deletion inside a known root."""
    try:
        root_resolved = root.resolve()
        candidate_resolved = candidate.resolve()
    except OSError:
        return False
    return root_resolved in candidate_resolved.parents


def dir_size(path: Path) -> int:
    """Total bytes under ``path``, walking without following symlinks and
    skipping anything that errors (a vanished or unreadable file)."""
    total = 0
    for dirpath, _dirnames, filenames in os.walk(path, followlinks=False):
        for name in filenames:
            fp = Path(dirpath) / name
            try:
                if not fp.is_symlink():
                    total += fp.stat().st_size
            except OSError:
                continue
    return total


# --------------------------------------------------------------------------
# Scan — build the manifest of per-entry records for every root.
# --------------------------------------------------------------------------


@dataclass
class Record:
    path: Path
    category: str
    delete: bool
    reason: str
    size: int
    # The issue tag when the slug is a worktree of this repo, else None. Carried
    # so the manifest can name what it proposes to delete; defaulted so the
    # field is additive for every existing caller.
    tag: str | None = None


def _dir_contains_session(entry: Path, current_uuid: str | None) -> bool:
    """True when a projects slug dir holds the current session's transcript —
    an extra guard so the active session is never dropped even if its slug
    differs from the cwd."""
    if current_uuid is None:
        return False
    return (entry / f"{current_uuid}.jsonl").is_file()


def scan_slug_root(
    root: Path,
    *,
    live_slugs: set[str],
    protected_slugs: set[str],
    current_slug: str | None,
    current_uuid: str | None,
    cutoff_ts: float,
    guard_session_file: bool,
    completed_slugs: set[str] | None = None,
    active_slugs: set[str] | None = None,
    former_prefix: str | None = None,
) -> list[Record]:
    """Classify every immediate subdirectory of a slug-partitioned root."""
    records: list[Record] = []
    if not root.is_dir():
        return records
    for entry in sorted(root.iterdir()):
        if not entry.is_dir() or entry.is_symlink():
            continue  # never follow a symlink out of the root
        tag = worktree_tag(entry.name, former_prefix)
        if guard_session_file and _dir_contains_session(entry, current_uuid):
            records.append(Record(entry, "kept", False, "current session", 0, tag))
            continue
        d = decide_slug(
            entry.name,
            entry.stat().st_mtime,
            live_slugs=live_slugs,
            protected_slugs=protected_slugs,
            current_slug=current_slug,
            cutoff_ts=cutoff_ts,
            completed_slugs=completed_slugs,
            active_slugs=active_slugs,
            former_prefix=former_prefix,
        )
        size = dir_size(entry) if d.delete else 0
        records.append(Record(entry, d.category, d.delete, d.reason, size, tag))
    return records


def session_uuids_in(slug_dir: Path) -> set[str]:
    """The session UUIDs a projects slug directory holds, read from its
    ``<uuid>.jsonl`` transcript filenames — the join that lets a ``file-history``
    directory be matched back to the project it belongs to."""
    uuids: set[str] = set()
    try:
        for entry in slug_dir.iterdir():
            if entry.is_file() and entry.suffix == ".jsonl":
                uuids.add(entry.stem)
    except OSError:
        return uuids
    return uuids


def scan_history_root(
    root: Path,
    *,
    current_uuid: str | None,
    cutoff_ts: float,
    protected_uuids: set[str] | None = None,
) -> list[Record]:
    """Classify every session-UUID directory under ``file-history``."""
    records: list[Record] = []
    if not root.is_dir():
        return records
    for entry in sorted(root.iterdir()):
        if not entry.is_dir() or entry.is_symlink():
            continue
        d = decide_history(
            entry.name,
            entry.stat().st_mtime,
            current_uuid=current_uuid,
            cutoff_ts=cutoff_ts,
            protected_uuids=protected_uuids,
        )
        size = dir_size(entry) if d.delete else 0
        records.append(Record(entry, d.category, d.delete, d.reason, size))
    return records


# --------------------------------------------------------------------------
# Report + apply
# --------------------------------------------------------------------------

CATEGORY_LABELS = {
    "completed": "finished work (worktree gone, PR merged or closed)",
    "dropset-old": "dropset transcripts (worktree gone, aged)",
    "non-dropset": "non-dropset transcripts",
    "file-history": "file-history (session UUID dirs)",
    "cli-cache": "CLI cache (aged)",
}


def _mb(n: int) -> str:
    return f"{n / 1_000_000:.1f} MB"


def kept_by_reason(groups: dict[str, list[Record]]) -> dict[str, int]:
    """How many records were kept, per reason.

    One collapsed figure was actively misleading: a dry run reporting "41
    protected" read as open-PR protection, when only four records were actually
    open-PR-protected and the rest were the blunt age rule across three roots.
    Those are different facts — one is work in flight, the other is a grace
    period — and only the first is a reason not to reclaim the space.
    """
    counts: dict[str, int] = {}
    for records in groups.values():
        for record in records:
            if not record.delete:
                counts[record.reason] = counts.get(record.reason, 0) + 1
    return dict(sorted(counts.items()))


def render_manifest(groups: dict[str, list[Record]], protected: int) -> str:
    """The grouped dry-run manifest: per-group count + MB, a total, and the
    kept count broken out by the reason each record was kept for.

    Every dropset slug proposed for deletion is **named by its issue tag**, on
    its own line under the group. The counts alone were what made a real loss
    approvable: a group line reading "non-dropset transcripts: 6 dir(s), 8.3
    MB" is indistinguishable from junk, and the one entry that mattered was a
    live session's transcript. A human can veto "eng-1192"; nobody can veto a
    megabyte count.
    """
    lines = ["purge-conversations — dry run (nothing deleted)\n"]
    total = 0
    for category, label in CATEGORY_LABELS.items():
        recs = [r for r in groups.get(category, []) if r.delete]
        if not recs:
            continue
        size = sum(r.size for r in recs)
        total += size
        lines.append(f"  {label}: {len(recs)} dir(s), {_mb(size)}")
        for r in sorted(recs, key=lambda rec: rec.tag or ""):
            if r.tag:
                lines.append(f"    - {r.tag} ({_mb(r.size)})")
    lines.append(f"  TOTAL to free: {_mb(total)}")
    # Header and breakout from ONE source. `protected` arrives as a
    # caller-computed scalar, and a header that can disagree with the lines
    # under it is the same confusion the per-reason breakout was added to
    # remove — so the total is summed from the breakout rather than taken on
    # trust. The parameter is kept for callers, and asserted against.
    by_reason = kept_by_reason(groups)
    lines.append(f"  kept: {sum(by_reason.values())}")
    for reason, count in by_reason.items():
        lines.append(f"    {reason}: {count}")
    lines.append("\nRe-run with --apply to hard-delete the above.")
    return "\n".join(lines)


def safe_delete(record: Record, roots: list[Path]) -> int:
    """Hard-delete one record's directory after re-checking the safety
    invariant: it must be a real (non-symlink) directory under a known root.
    Returns bytes freed (0 on refusal)."""
    path = record.path
    if path.is_symlink() or not path.is_dir():
        return 0
    if not any(is_within(root, path) for root in roots):
        print(f"refusing to delete outside a known root: {path}", file=sys.stderr)
        return 0
    freed = record.size
    shutil.rmtree(path)
    return freed


def resolve_dropset_repo(explicit: str | None) -> str:
    """The repo whose worktrees are protected: ``--dropset-repo`` when given,
    otherwise the working directory's own repo root.

    Defaulting is the point. The flag was optional and omitting it silently
    disabled **all** worktree protection, which is not a state any invocation
    ever wants — so the tool now derives what it needs and **refuses** when it
    cannot, rather than treating "I don't know the worktrees" as "there are
    none". Any worktree of the repo resolves here, since
    ``git worktree list`` run from a linked worktree enumerates the whole set,
    the main worktree included.
    """
    if explicit:
        return explicit
    try:
        proc = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            capture_output=True,
            text=True,
            check=True,
        )
        root = proc.stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        root = ""
    if not root:
        raise PruneError(
            "cannot resolve the dropset repo: no --dropset-repo was given and "
            "the working directory is not inside a git worktree. Re-run from a "
            "dropset checkout, or pass --dropset-repo <path>. Refusing rather "
            "than running with an empty worktree set, which would age-delete "
            "live sessions' transcripts as 'non-dropset'."
        )
    return root


def read_worktrees(dropset_repo: str) -> list[tuple[str, str | None]]:
    """Run ``git worktree list --porcelain`` for the dropset repo.

    Raises on an empty result. A valid repo always reports at least its main
    worktree, so "no worktrees" means the lookup did not do what the caller
    thinks it did — and continuing from there is precisely the state that
    deleted a live session, since every protection is computed from this list.
    """
    try:
        proc = subprocess.run(
            ["git", "-C", dropset_repo, "worktree", "list", "--porcelain"],
            capture_output=True,
            text=True,
            check=True,
        )
    except (OSError, subprocess.CalledProcessError) as e:
        raise PruneError(f"git worktree list failed for {dropset_repo}: {e}") from e
    worktrees = parse_worktrees(proc.stdout)
    if not worktrees:
        raise PruneError(
            f"git worktree list reported no worktrees for {dropset_repo}; "
            "refusing to run, because every protection is derived from that "
            "list and an empty one protects nothing."
        )
    return worktrees


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description="Prune old Claude Code transcripts and caches (dry-run by "
        "default; --apply to delete).",
    )
    p.add_argument(
        "--dropset-repo",
        help="path to a dropset checkout (any worktree will do); every existing "
        "worktree's slug is kept. Defaults to the working directory's repo "
        "root; the run ABORTS if neither resolves, rather than protecting "
        "nothing.",
    )
    p.add_argument(
        "--protected-branch",
        action="append",
        default=[],
        metavar="BRANCH",
        help="a worktree branch with an OPEN PR (drafts included) — its slug is "
        "kept regardless of age (repeatable; the skill supplies these from the "
        "GitHub MCP). Belt-and-braces: an existing worktree is already kept.",
    )
    p.add_argument(
        "--completed-slug",
        action="append",
        default=[],
        metavar="SLUG",
        help="a slug whose worktree is gone and whose PR is merged or closed — "
        "finished work, deleted without the age grace period (repeatable; the "
        "skill already computes this set one step earlier).",
    )
    p.add_argument(
        "--current-session",
        help="the current session UUID — always kept in every root.",
    )
    p.add_argument(
        "--age-days",
        type=float,
        default=DEFAULT_AGE_DAYS,
        help=f"age threshold in days (default {DEFAULT_AGE_DAYS}).",
    )
    p.add_argument(
        "--apply",
        action="store_true",
        help="hard-delete the manifest (default: dry-run only).",
    )
    p.add_argument(
        "--now",
        type=float,
        default=None,
        help="override the current epoch time (testing only).",
    )
    return p


def run(argv: list[str]) -> int:
    args = build_parser().parse_args(argv[1:])

    now = args.now if args.now is not None else _now()
    cutoff_ts = now - args.age_days * SECONDS_PER_DAY

    repo = resolve_dropset_repo(args.dropset_repo)
    worktrees = read_worktrees(repo)
    live_slugs, protected_slugs = dropset_slug_sets(
        worktrees, set(args.protected_branch)
    )
    former_prefix = former_worktree_prefix(base_worktree(worktrees))
    current_slug = slugify(Path.cwd())
    current_uuid = args.current_session
    completed_slugs = set(args.completed_slug)
    active_slugs = read_active_slugs(cutoff_ts)

    # A protected branch naming no live worktree is legitimate (its worktree
    # may already have been pruned while the PR stayed open), so this warns
    # rather than aborts. It is still worth saying out loud: the same shape is
    # what a broken lookup produces, and the old failure was silent.
    live_branches = {branch for _path, branch in worktrees if branch}
    unmatched = sorted(set(args.protected_branch) - live_branches)
    if unmatched:
        print(
            "warning: --protected-branch matched no live worktree: "
            + ", ".join(unmatched),
            file=sys.stderr,
        )

    proj = projects_root()
    cli = cli_cache_root()
    hist = file_history_root()
    roots = [proj, cli, hist]

    records: list[Record] = []
    project_records = scan_slug_root(
        proj,
        live_slugs=live_slugs,
        protected_slugs=protected_slugs,
        current_slug=current_slug,
        current_uuid=current_uuid,
        cutoff_ts=cutoff_ts,
        guard_session_file=True,
        completed_slugs=completed_slugs,
        active_slugs=active_slugs,
        former_prefix=former_prefix,
    )
    records += project_records
    # Sessions belonging to a project directory we are keeping, so file-history
    # is protected on the same footing as the transcript it belongs to. Derived
    # from the decisions just made rather than re-computed, so the two roots
    # cannot disagree about which sessions matter.
    protected_uuids: set[str] = set()
    for r in project_records:
        if not r.delete:
            protected_uuids |= session_uuids_in(r.path)
    # The CLI cache uses the same slug scheme; re-tag a deletable slug entry as
    # the cli-cache group so the manifest separates it from transcripts.
    for r in scan_slug_root(
        cli,
        live_slugs=live_slugs,
        protected_slugs=protected_slugs,
        current_slug=current_slug,
        current_uuid=current_uuid,
        cutoff_ts=cutoff_ts,
        guard_session_file=False,
        completed_slugs=completed_slugs,
        active_slugs=active_slugs,
        former_prefix=former_prefix,
    ):
        if r.delete:
            r.category = "cli-cache"
        records.append(r)
    records += scan_history_root(
        hist,
        current_uuid=current_uuid,
        cutoff_ts=cutoff_ts,
        protected_uuids=protected_uuids,
    )

    groups: dict[str, list[Record]] = {}
    for r in records:
        groups.setdefault(r.category, []).append(r)
    protected = sum(1 for r in records if not r.delete)

    if not args.apply:
        print(render_manifest(groups, protected))
        return 0

    freed = 0
    deleted = 0
    for r in records:
        if r.delete:
            got = safe_delete(r, roots)
            freed += got
            if got:
                deleted += 1
    print(f"purge-conversations | deleted {deleted} dir(s) | freed {_mb(freed)}")
    return 0


def _now() -> float:
    """Wall-clock epoch seconds, isolated so tests can avoid it (they pass
    ``--now``)."""
    import time

    return time.time()


def main() -> int:
    try:
        return run(sys.argv)
    except PruneError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
