#!/usr/bin/env python3
"""Verdict records: does a local green predict a remote one?

``verify_head.py`` and ``wait_for_checks.py`` (checks mode) each append one
JSON line to :data:`LOG` in the base checkout's substrate directory when they
settle, keyed by the **tree** they verified. The tree, not the commit, is the
key because the merge gate moves the commit twice between the two verdicts
and neither move touches the content: the re-sign replays every commit, and
the trigger adds one empty commit. A file, not the transcript, is the source
because the gate runs the race in the background, and a background call's
notification carries no verdict.

:func:`pair` walks the records in time order, holds the latest local race
verdict per tree, and pairs it with the next settled checks verdict on that
tree: a local green the remote run failed (with the failing jobs), the
reverse, or agreement. The digest writes the two disagreement counts beside
the day's pairs (``wait_ledger.digest_section``).

A record is written best-effort: a failed write is one stderr line, never a
changed verdict or exit code. Stdlib only; tests in
``tests/test_ci_verdicts.py``.
"""

from __future__ import annotations

import json
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

import machine_load

LOG = "ci-verdicts.jsonl"
RACE = "race"
CHECKS = "checks"
LOCAL_GREEN_REMOTE_RED = "local-green-remote-red"
LOCAL_RED_REMOTE_GREEN = "local-red-remote-green"
PAIRINGS = (LOCAL_GREEN_REMOTE_RED, LOCAL_RED_REMOTE_GREEN)
# Both sides agreed: no digest line of its own, the denominator of the two.
AGREED = "agreed"


def head_tree(ref: str = "HEAD") -> str | None:
    """``ref``'s tree hash in the current checkout, ``None`` when git cannot
    answer."""
    try:
        out = subprocess.run(
            ["git", "rev-parse", f"{ref}^{{tree}}"],
            capture_output=True,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    tree = out.stdout.strip() if out.returncode == 0 else ""
    return tree or None


def race_record(outcome: str, sha: str, local: dict[str, bool]) -> dict | None:
    """The race verdict's record, or ``None`` when no check settled locally.
    ``local`` maps each locally settled check to green. Local green is a
    GREEN with every local check green; local red is any local red."""
    if not local:
        return None
    if not all(local.values()):
        state = "red"
    elif outcome == "GREEN":
        state = "green"
    else:
        return None
    return {"tool": RACE, "sha": sha[:12], "outcome": outcome, "local": state}


def checks_record(verdict: dict) -> dict | None:
    """The checks verdict's record, or ``None`` unless it settled pass or
    fail."""
    conclusion = verdict.get("conclusion")
    if conclusion not in ("pass", "fail"):
        return None
    failing = sorted(
        str(f.get("name"))
        for f in verdict.get("failing") or []
        if isinstance(f, dict) and f.get("name")
    )
    return {
        "tool": CHECKS,
        "pr": verdict.get("pr"),
        "conclusion": conclusion,
        "failing": failing,
    }


def append(
    record: dict | None,
    tree: str | None,
    state_dir: Path | None = None,
    now: datetime | None = None,
) -> None:
    """Append ``record`` stamped and keyed by ``tree``; a missing record or
    tree writes nothing, and a failed write is reported, never raised."""
    if record is None or not tree:
        return
    at = (now or datetime.now(timezone.utc)).strftime("%Y-%m-%dT%H:%M:%S")
    path = LOG
    try:
        path = (state_dir or machine_load.default_state_dir()) / LOG
        machine_load.append_record(
            path, json.dumps({"at": at, "tree": tree, **record}, sort_keys=True)
        )
    except Exception as exc:  # noqa: BLE001 — a record never changes a verdict
        print(f"ci-verdicts: could not append to {path}: {exc}", file=sys.stderr)


def pr_head(pr: int, repo: str) -> str | None:
    """The PR's head commit as GitHub reports it — the commit its checks ran
    on — or ``None`` when ``gh`` cannot answer."""
    try:
        out = subprocess.run(
            [
                "gh",
                "pr",
                "view",
                str(pr),
                "--repo",
                repo,
                "--json",
                "headRefOid",
                "--jq",
                ".headRefOid",
            ],
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    sha = out.stdout.strip() if out.returncode == 0 else ""
    return sha or None


def read(path: Path, until: str) -> list[dict]:
    """Every record stamped before ``until``, oldest first; an unreadable
    file or line is skipped. Races before the window are kept: a checks
    verdict inside it may pair with one."""
    try:
        text = path.read_text("utf-8")
    except OSError:
        return []
    out = []
    for raw in text.splitlines():
        try:
            rec = json.loads(raw)
        except ValueError:
            continue
        if isinstance(rec, dict) and str(rec.get("at", "")) < until:
            out.append(rec)
    return sorted(out, key=lambda r: str(r.get("at", "")))


def pair(records: list[dict], since: str) -> list[tuple[str, list[str]]]:
    """Each settled checks verdict stamped at or after ``since`` paired with
    the latest earlier race verdict on its tree, which it consumes:
    ``(kind, failing jobs)``."""
    held: dict[str, str] = {}
    pairs = []
    for rec in records:
        tree = rec.get("tree")
        if not isinstance(tree, str):
            continue
        if rec.get("tool") == RACE and rec.get("local") in ("green", "red"):
            held[tree] = rec["local"]
            continue
        if rec.get("tool") != CHECKS or tree not in held:
            continue
        local = held.pop(tree)
        if str(rec.get("at", "")) < since:
            continue
        remote = "green" if rec.get("conclusion") == "pass" else "red"
        if remote == local:
            kind = AGREED
        elif local == "green":
            kind = LOCAL_GREEN_REMOTE_RED
        else:
            kind = LOCAL_RED_REMOTE_GREEN
        pairs.append((kind, list(rec.get("failing") or [])))
    return pairs
