#!/usr/bin/env python3
"""Resolve which Claude Code session belongs to a worktree tag, and how to reach
it — the addressing `raps` needs and `--continue` alone cannot supply.

**The bug this exists to fix.** `raps <n>` used to `cd` into the `eng-<n>`
worktree and run `claude --continue`, on the reasonable assumption that
`--continue`'s per-directory addressing selects that worktree's session. It does
not, for a session started by `aps`: `aps` runs `claude -w <tag>` **from the base
repo**, so Claude Code files the transcript under the *base* repo's project slug
even though every one of its `cwd` stamps points into the worktree. No project
directory for the worktree ever exists, `--continue` finds nothing there, and
`raps` reports "no conversation found" while the session sits intact under
another slug.

Measured: one session's transcript was recovered by hand at
``~/.claude/projects/<base-slug>/<uuid>.jsonl`` while its worktree was present
and its PR open. Sessions that had been resumed *from inside* their worktree at
some point did have a worktree-slug transcript, which masked the gap — so the
failure looks intermittent and tracks launch history rather than anything the
caller did.

`faps` types `raps`, so fleet resume inherits the same miss for every
`-w`-launched session that has never been resumed from inside its worktree.

Usage::

    python3 .claude/tools/resolve_session.py --tag eng-1051

It prints one JSON object and never opens a session itself — deciding is this
tool's job, launching is the shell verb's:

    {
      "tag": "eng-1051",
      "worktree": "/…/.claude/worktrees/eng-1051",
      "worktree_exists": true,
      "mode": "resume",
      "session_id": "afce0c54-1874-4276-a77b-38df146d0000",
      "run_from": "/…/dropset",
      "reason": "the worktree has no transcript of its own; …"
    }

``mode`` is the instruction to the caller:

* ``continue`` — the worktree has its own transcript; `cd` there and
  ``claude --continue``, the original fast path.
* ``resume`` — a base-slug transcript's ``cwd`` stamps point into this
  worktree; run ``claude --resume <session_id>`` from ``run_from``.
* ``picker`` — nothing resolved deterministically; fall back to
  ``claude --resume <tag>``, which filters the picker rather than resuming.

Reading transcripts happens in **this** process, so a multi-megabyte JSONL never
reaches a transcript of its own (per ``docs/conventions/context-economy.md``).
Only the head of each candidate is read: `claude -w` stamps the cwd from the
first record, so scanning the whole file to find one would be pure cost.

Standard library only. A Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling"). Tests live in ``tests/test_resolve_session.py``, run via
``make tools-tests``.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

# How many leading records of a candidate transcript are scanned for a `cwd`
# stamp. `claude -w` sets the working directory before the first record is
# written, so the answer is in the head or it is not there at all — and these
# files run to hundreds of megabytes, which is the whole reason this scan is
# bounded rather than exhaustive.
CWD_SCAN_LINES = 200

# Keys a transcript record may carry the working directory under. `cwd` is what
# Claude Code writes today; the alternates cost nothing to accept and mean a
# format tweak degrades to the picker instead of to a wrong answer.
CWD_KEYS = ("cwd", "workingDirectory", "working_dir")


class ResolveSessionError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def claude_home() -> Path:
    """The Claude Code state directory — ``CLAUDE_CONFIG_DIR`` or ``~/.claude``.

    Mirrors ``firm_last.py`` and ``prune_conversations.py`` rather than
    re-deciding it: three tools reading the same tree must agree on where it is.
    """
    configured = os.environ.get("CLAUDE_CONFIG_DIR")
    if configured:
        return Path(configured)
    home = os.environ.get("HOME")
    if not home:
        raise ResolveSessionError("neither CLAUDE_CONFIG_DIR nor HOME is set")
    return Path(home) / ".claude"


def slugify(path: Path) -> str:
    """Claude Code names each project's transcript dir after the working dir,
    replacing every ``/`` and ``.`` with ``-``. Same scheme as ``firm_last.py``.
    """
    return "".join("-" if c in "/." else c for c in str(path))


def normalize_tag(raw: str) -> str:
    """``1051``, ``eng-1051`` and ``ENG-1051`` all name the same worktree.

    Matches what ``raps`` and ``cdds`` already accept, so the tool and the verbs
    agree on what a tag is.
    """
    tag = raw.strip().lower()
    if not tag:
        raise ResolveSessionError("--tag was empty")
    return tag if tag.startswith("eng-") else f"eng-{tag.removeprefix('eng-')}"


def transcripts_in(slug_dir: Path) -> list[Path]:
    """Top-level ``*.jsonl`` under one project slug, newest first.

    Top-level only, deliberately: a sub-agent transcript is not a session a
    human can resume, and offering one would be a wrong answer rather than a
    missing one.
    """
    if not slug_dir.is_dir():
        return []
    files = [p for p in slug_dir.glob("*.jsonl") if p.is_file()]
    return sorted(files, key=lambda p: p.stat().st_mtime, reverse=True)


def stamps_into(transcript: Path, target: Path) -> bool:
    """True when this transcript's ``cwd`` stamps point at ``target``.

    This is the actual identification: a `-w`-launched session is filed under
    the base slug, so the slug says nothing and the stamps say everything. A
    malformed or unreadable line is skipped rather than fatal — a transcript
    being written to right now can end mid-record, and that is the common case
    for a live session, which is exactly the one worth resuming.
    """
    target_str = str(target)
    try:
        with transcript.open(encoding="utf-8", errors="replace") as fh:
            for index, line in enumerate(fh):
                if index >= CWD_SCAN_LINES:
                    return False
                line = line.strip()
                if not line:
                    continue
                try:
                    record = json.loads(line)
                except (json.JSONDecodeError, ValueError):
                    continue
                if not isinstance(record, dict):
                    continue
                for key in CWD_KEYS:
                    value = record.get(key)
                    if not isinstance(value, str):
                        continue
                    normalized = os.path.normpath(value)
                    if normalized == target_str or normalized.startswith(
                        target_str + os.sep
                    ):
                        return True
    except OSError:
        return False
    return False


def resolve(tag: str, repo: Path) -> dict:
    """Decide how to reach the session for ``tag``. Pure filesystem inspection."""
    worktree = repo / ".claude" / "worktrees" / tag
    verdict = {
        "tag": tag,
        "worktree": str(worktree),
        "worktree_exists": worktree.is_dir(),
        "mode": "picker",
        "session_id": None,
        "run_from": str(repo),
        "reason": "",
    }

    if not verdict["worktree_exists"]:
        # The pre-existing fallback, and still correct: the worktree was pruned
        # after a merge but the transcript outlives it.
        verdict["reason"] = (
            f"no worktree at {worktree} — it was likely pruned; the picker "
            f"filtered by tag is the only form that still reaches the session"
        )
        return verdict

    projects = claude_home() / "projects"

    # The fast path, unchanged: a worktree with its own transcript is exactly
    # what `--continue` addresses.
    own = transcripts_in(projects / slugify(worktree))
    if own:
        verdict["mode"] = "continue"
        verdict["run_from"] = str(worktree)
        verdict["reason"] = (
            f"{worktree.name} has its own transcript, so --continue addresses "
            f"it from inside the worktree"
        )
        return verdict

    # The miss this tool exists for. Check the base slug first — that is where
    # `aps` files a `-w` session — then every other project dir, since a session
    # could have been launched from somewhere else entirely.
    base_slug = projects / slugify(repo)
    searched = [base_slug]
    others = (
        sorted(
            (p for p in projects.iterdir() if p.is_dir() and p != base_slug),
            key=lambda p: p.stat().st_mtime,
            reverse=True,
        )
        if projects.is_dir()
        else []
    )
    searched.extend(others)

    for slug_dir in searched:
        for transcript in transcripts_in(slug_dir):
            if stamps_into(transcript, worktree):
                verdict["mode"] = "resume"
                verdict["session_id"] = transcript.stem
                verdict["run_from"] = str(repo)
                verdict["reason"] = (
                    f"the worktree has no transcript of its own, but "
                    f"{transcript.name} under {slug_dir.name} stamps its cwd "
                    f"into the worktree — resume it by id from the base repo"
                )
                return verdict

    verdict["reason"] = (
        f"the worktree exists but no transcript anywhere under {projects} "
        f"stamps its cwd into it — the session may never have started"
    )
    return verdict


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="resolve_session.py",
        description="Resolve how to reach a worktree tag's Claude Code session.",
    )
    parser.add_argument("--tag", required=True, help="eng-### or a bare number")
    parser.add_argument(
        "--repo",
        default=None,
        help="base repo checkout (default: the CWD's repo root by convention)",
    )
    parser.add_argument(
        "--format",
        choices=("json", "lines"),
        default="json",
        help="json (default, for humans and jq) or lines — mode, session id "
        "and run-from directory on three lines, for a shell `read`",
    )
    args = parser.parse_args(argv[1:])

    # `abspath`, NOT `resolve()`. Claude Code derives the slug from the working
    # directory **string** it was given, so the slug has to be computed from
    # that same string — and `resolve()` follows symlinks, silently rewriting it.
    # macOS is where this bites: `/tmp` and `/var` are symlinks, so a resolved
    # path yields a slug for a directory that does not exist, and every lookup
    # misses while looking entirely correct. `abspath` normalizes a relative
    # argument without touching symlinks, which is exactly the needed half.
    repo = Path(os.path.abspath(args.repo if args.repo else Path.cwd()))
    verdict = resolve(normalize_tag(args.tag), repo)
    if args.format == "lines":
        # Three fixed lines in a fixed order, so a shell reads them positionally
        # with no JSON parsing. The session id is empty rather than absent when
        # there is none, which keeps the line count constant — a caller doing
        # three `read`s must not have its third read consume a missing second.
        print(verdict["mode"])
        print(verdict["session_id"] or "")
        print(verdict["run_from"])
    else:
        print(json.dumps(verdict, indent=2))
    # 0 when the caller has a deterministic action, 1 when it must fall back to
    # the picker — so the shell can branch on the exit status alone.
    return 0 if verdict["mode"] in ("continue", "resume") else 1


def main() -> int:
    try:
        return run(sys.argv)
    except ResolveSessionError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
