#!/usr/bin/env python3
# cspell:word surrogateescape
"""Fingerprint the working tree's CONTENT, and grade recorded evidence against it.

``review-pr`` re-checks base freshness before each expensive stage and re-runs
suites to establish currency. The question it is really asking is **"is this
still green?"**, and today the only way to answer it is to pay for the suite
again — which is why one measured review ran three full lint sweeps, two
complete artifact-gate sets and three Rust suite runs, several of them proving
something already known.

A **content** fingerprint answers it without re-running. The key property is
that it is computed from the *bytes of the tracked files*, so it stays identical
across the operations that change history but not content:

* ``git commit`` — staging then committing the same bytes;
* ``git rebase`` onto a base that touched nothing this branch touches;
* ``git commit --amend``, and squashing;
* changing a commit message.

A commit SHA changes under every one of those; the content does not. That gap is
exactly where the wasted re-runs live.

Evidence is graded three ways, and the three-way answer is the point — a binary
fresh/stale would have to call "no record" stale and re-run:

``fresh``
    Recorded against this exact content. The result stands; assert it.
``stale``
    Recorded against different content. Re-run.
``missing``
    Never recorded. Re-run, and record it this time.

**It fingerprints tracked files plus untracked-not-ignored ones**, because a
new file that is not yet ``git add``-ed is precisely the file a lint or suite
run would fail on — treating it as absent would make the fingerprint claim
currency it does not have.

Usage::

    # What is the tree's content fingerprint right now?
    python3 .claude/tools/tree_fingerprint.py compute

    # Record that a check passed against the current content.
    python3 .claude/tools/tree_fingerprint.py record --check lint

    # Does that recorded result still stand?
    python3 .claude/tools/tree_fingerprint.py check --check lint

``check`` exits 0 when the evidence is ``fresh`` and 1 otherwise, so a skill
step can branch on the status alone.

Stdlib only, and deliberately **not** a Cargo workspace member — see
``docs/conventions/skill-tooling.md``. Tests live in
``tests/test_tree_fingerprint.py``, run via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

# Where the evidence ledger lives. Under `.git/`, deliberately: it is per-
# checkout state about a working tree, it must never be committed, and `.git/`
# is already the one directory guaranteed not to be part of the fingerprint.
LEDGER_RELATIVE = Path("dropset-evidence.json")

# A runaway guard on the file walk. Far above any real source tree; tripping it
# means the fingerprint is being asked to cover something it should not.
MAX_FILES = 50_000


class TreeFingerprintError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def _git(args: list[str]) -> str:
    try:
        completed = subprocess.run(
            ["git", *args],
            capture_output=True,
            text=True,
            errors="replace",
            check=False,
        )
    except OSError as exc:
        raise TreeFingerprintError(f"cannot run git: {exc}") from exc
    if completed.returncode != 0:
        detail = (completed.stderr or "").strip() or f"exit {completed.returncode}"
        raise TreeFingerprintError(f"git {' '.join(args)} failed: {detail}")
    return completed.stdout


def tracked_paths() -> list[str]:
    """Tracked files plus untracked-not-ignored ones, sorted.

    Both halves matter. Tracked files are the obvious content; an
    untracked-not-ignored file is the one a lint or suite run would fail on, so
    omitting it would let the fingerprint claim a currency it has not got —
    the same reason ``lint_paths.py`` includes them.
    """
    out = _git(["ls-files", "--cached", "--others", "--exclude-standard", "-z"])
    paths = sorted(p for p in out.split("\0") if p)
    if len(paths) > MAX_FILES:
        raise TreeFingerprintError(
            f"{len(paths)} files exceeds the {MAX_FILES} cap — refusing to "
            "fingerprint a tree this size"
        )
    return paths


def compute(paths: list[str] | None = None) -> str:
    """A stable hex digest of the working tree's content.

    Each file contributes its **path** and its **bytes**, both length-prefixed
    so no concatenation of one file's content can impersonate another's path.
    Paths are sorted, so the digest does not depend on filesystem order.

    A path that has vanished between the listing and the read is folded in as a
    marker rather than skipped — skipping would make a deletion invisible,
    which is exactly the kind of change that should invalidate evidence.
    """
    digest = hashlib.sha256()
    for path in tracked_paths() if paths is None else paths:
        encoded = path.encode("utf-8", errors="surrogateescape")
        digest.update(f"{len(encoded)}:".encode())
        digest.update(encoded)
        try:
            with open(path, "rb") as handle:
                data = handle.read()
        except (OSError, IsADirectoryError):
            # A submodule entry, a broken symlink, or a file removed mid-walk.
            digest.update(b"0:<unreadable>")
            continue
        digest.update(f"{len(data)}:".encode())
        digest.update(data)
    return digest.hexdigest()


def _ledger_path() -> Path:
    git_dir = _git(["rev-parse", "--git-dir"]).strip()
    if not git_dir:
        raise TreeFingerprintError("not inside a git repository")
    return Path(git_dir) / LEDGER_RELATIVE


def load_ledger() -> dict:
    path = _ledger_path()
    if not path.exists():
        return {}
    try:
        with open(path, encoding="utf-8") as handle:
            data = json.load(handle)
    except (OSError, json.JSONDecodeError):
        # A corrupt ledger grades everything `missing`, which re-runs the
        # checks. That is the safe direction: the failure mode of trusting a
        # damaged ledger is asserting a green that was never established.
        return {}
    return data if isinstance(data, dict) else {}


def save_ledger(ledger: dict) -> None:
    path = _ledger_path()
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(ledger, handle, indent=2, sort_keys=True)
            handle.write("\n")
    except OSError as exc:
        raise TreeFingerprintError(f"cannot write {path}: {exc}") from exc


def grade(ledger: dict, check: str, fingerprint: str) -> str:
    """``"fresh"`` / ``"stale"`` / ``"missing"`` for one recorded check."""
    entry = ledger.get(check)
    if not isinstance(entry, dict) or "fingerprint" not in entry:
        return "missing"
    return "fresh" if entry["fingerprint"] == fingerprint else "stale"


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="tree_fingerprint.py")
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("compute", help="print the working tree's content fingerprint")

    record = sub.add_parser("record", help="record that a check passed")
    record.add_argument("--check", required=True, help="e.g. lint, tools-tests")
    record.add_argument("--note", default="", help="free text kept with the entry")

    check = sub.add_parser("check", help="grade a recorded check; 0 iff fresh")
    check.add_argument("--check", required=True)

    args = parser.parse_args(argv[1:])
    fingerprint = compute()

    if args.command == "compute":
        print(fingerprint)
        return 0

    if args.command == "record":
        ledger = load_ledger()
        ledger[args.check] = {"fingerprint": fingerprint, "note": args.note}
        save_ledger(ledger)
        print(f"RECORDED {args.check} @ {fingerprint[:12]}")
        return 0

    status = grade(load_ledger(), args.check, fingerprint)
    print(f"{status.upper()} {args.check} @ {fingerprint[:12]}")
    return 0 if status == "fresh" else 1


def main() -> int:
    try:
        return run(sys.argv)
    except TreeFingerprintError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
