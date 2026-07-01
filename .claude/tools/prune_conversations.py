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
  A dropset slug gets the age rule **unless** its worktree branch has an open PR
  (kept regardless of age); a non-dropset slug is age-only.
* **Session-UUID** — ``~/.claude/file-history`` is one flat subdirectory per
  session UUID, mixing every repo, so it can't be cheaply repo-scoped: age-only
  by directory mtime.

The **dropset set is derived forward** (``git worktree list`` → slug of each
real worktree path), never by string-matching slug prefixes — a prefix would
wrongly catch a sibling repo like ``dropset-beta`` whose slug starts with the
base repo's. The **current session is always kept** in every root (by session
id, and by the current working directory's slug).

Safety invariant: the tool only ever deletes a directory that resolves **under**
one of the three known roots, never follows a symlink, refuses any entry that
escapes its root, and never touches the current session. Dry-run is the default;
deletion requires ``--apply``. Standard library only.
"""

from __future__ import annotations

import argparse
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
    """From parsed worktrees, return ``(dropset_slugs, protected_slugs)``: every
    real worktree path's slug, and the subset whose branch has an open PR (so it
    is kept regardless of age)."""
    dropset: set[str] = set()
    protected: set[str] = set()
    for path, branch in worktrees:
        slug = slugify(Path(path))
        dropset.add(slug)
        if branch is not None and branch in protected_branches:
            protected.add(slug)
    return dropset, protected


@dataclass
class Decision:
    delete: bool
    category: str  # dropset-old | non-dropset | kept
    reason: str


def decide_slug(
    slug: str,
    mtime_ts: float,
    *,
    dropset_slugs: set[str],
    protected_slugs: set[str],
    current_slug: str | None,
    cutoff_ts: float,
) -> Decision:
    """Decide a slug-partitioned entry (projects or CLI cache). The current
    slug is always kept; a dropset slug with an open PR is kept regardless of
    age; otherwise the age rule applies (dropset and non-dropset alike)."""
    if current_slug is not None and slug == current_slug:
        return Decision(False, "kept", "current session")
    if slug in dropset_slugs:
        if slug in protected_slugs:
            return Decision(False, "kept", "open PR")
        if mtime_ts < cutoff_ts:
            return Decision(True, "dropset-old", "dropset, older than threshold")
        return Decision(False, "kept", "dropset, within age")
    if mtime_ts < cutoff_ts:
        return Decision(True, "non-dropset", "non-dropset, older than threshold")
    return Decision(False, "kept", "non-dropset, within age")


def decide_history(
    name: str, mtime_ts: float, *, current_uuid: str | None, cutoff_ts: float
) -> Decision:
    """Decide a file-history session-UUID directory: age-only, current kept."""
    if current_uuid is not None and name == current_uuid:
        return Decision(False, "kept", "current session")
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
    dropset_slugs: set[str],
    protected_slugs: set[str],
    current_slug: str | None,
    current_uuid: str | None,
    cutoff_ts: float,
    guard_session_file: bool,
) -> list[Record]:
    """Classify every immediate subdirectory of a slug-partitioned root."""
    records: list[Record] = []
    if not root.is_dir():
        return records
    for entry in sorted(root.iterdir()):
        if not entry.is_dir() or entry.is_symlink():
            continue  # never follow a symlink out of the root
        if guard_session_file and _dir_contains_session(entry, current_uuid):
            records.append(Record(entry, "kept", False, "current session", 0))
            continue
        d = decide_slug(
            entry.name,
            entry.stat().st_mtime,
            dropset_slugs=dropset_slugs,
            protected_slugs=protected_slugs,
            current_slug=current_slug,
            cutoff_ts=cutoff_ts,
        )
        size = dir_size(entry) if d.delete else 0
        records.append(Record(entry, d.category, d.delete, d.reason, size))
    return records


def scan_history_root(
    root: Path, *, current_uuid: str | None, cutoff_ts: float
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
        )
        size = dir_size(entry) if d.delete else 0
        records.append(Record(entry, d.category, d.delete, d.reason, size))
    return records


# --------------------------------------------------------------------------
# Report + apply
# --------------------------------------------------------------------------

CATEGORY_LABELS = {
    "dropset-old": "dropset transcripts (aged, no open PR)",
    "non-dropset": "non-dropset transcripts",
    "file-history": "file-history (session UUID dirs)",
    "cli-cache": "CLI cache (aged)",
}


def _mb(n: int) -> str:
    return f"{n / 1_000_000:.1f} MB"


def render_manifest(groups: dict[str, list[Record]], protected: int) -> str:
    """The grouped dry-run manifest: per-group count + MB, a total, and the
    protected (kept-by-open-PR / current) count."""
    lines = ["purge-conversations — dry run (nothing deleted)\n"]
    total = 0
    for category, label in CATEGORY_LABELS.items():
        recs = [r for r in groups.get(category, []) if r.delete]
        if not recs:
            continue
        size = sum(r.size for r in recs)
        total += size
        lines.append(f"  {label}: {len(recs)} dir(s), {_mb(size)}")
    lines.append(f"  TOTAL to free: {_mb(total)}")
    lines.append(f"  protected (open PR / current / within age): {protected}")
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


def read_worktrees(dropset_repo: str | None) -> list[tuple[str, str | None]]:
    """Run ``git worktree list --porcelain`` for the dropset repo, or return an
    empty list when no repo was given (every slug is then treated as
    non-dropset — age-only)."""
    if not dropset_repo:
        return []
    try:
        proc = subprocess.run(
            ["git", "-C", dropset_repo, "worktree", "list", "--porcelain"],
            capture_output=True,
            text=True,
            check=True,
        )
    except (OSError, subprocess.CalledProcessError) as e:
        raise PruneError(f"git worktree list failed for {dropset_repo}: {e}") from e
    return parse_worktrees(proc.stdout)


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
        help="path to the dropset base repo; its worktrees' slugs get the "
        "open-PR-protected age rule. Omit to treat every slug as non-dropset.",
    )
    p.add_argument(
        "--protected-branch",
        action="append",
        default=[],
        metavar="BRANCH",
        help="a worktree branch with an OPEN PR — its slug is kept regardless "
        "of age (repeatable; the skill supplies these from the GitHub MCP).",
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

    worktrees = read_worktrees(args.dropset_repo)
    dropset_slugs, protected_slugs = dropset_slug_sets(
        worktrees, set(args.protected_branch)
    )
    current_slug = slugify(Path.cwd())
    current_uuid = args.current_session

    proj = projects_root()
    cli = cli_cache_root()
    hist = file_history_root()
    roots = [proj, cli, hist]

    records: list[Record] = []
    records += scan_slug_root(
        proj,
        dropset_slugs=dropset_slugs,
        protected_slugs=protected_slugs,
        current_slug=current_slug,
        current_uuid=current_uuid,
        cutoff_ts=cutoff_ts,
        guard_session_file=True,
    )
    # The CLI cache uses the same slug scheme; re-tag a deletable slug entry as
    # the cli-cache group so the manifest separates it from transcripts.
    for r in scan_slug_root(
        cli,
        dropset_slugs=dropset_slugs,
        protected_slugs=protected_slugs,
        current_slug=current_slug,
        current_uuid=current_uuid,
        cutoff_ts=cutoff_ts,
        guard_session_file=False,
    ):
        if r.delete:
            r.category = "cli-cache"
        records.append(r)
    records += scan_history_root(hist, current_uuid=current_uuid, cutoff_ts=cutoff_ts)

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
