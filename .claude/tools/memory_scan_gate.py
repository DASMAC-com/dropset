#!/usr/bin/env python3
"""Cadence gate for the upkeep pass's auto-memory staleness scan.

The staleness scan reads the entire saved-memory store and repo-verifies every
reference — the dominant compute of an upkeep pass. The store changes
slowly, so re-running that scan on every pass is
wasteful (per ``CLAUDE.md`` → "Context economy"). This tool decides whether a
fresh scan is warranted, so ``upkeep.py`` can skip it when nothing has moved.

The gate scans when **any** of these holds (the union of the two signals the
lever proposed — "at most once per day, *or* when the store changed"):

* no prior scan is recorded (first run, or a fresh machine);
* the memory store changed since the last scan (a memory added / removed /
  edited — detected by a content signature over every ``*.md`` in the store);
* at least ``--min-interval-hours`` have elapsed since the last scan (default
  20h) — the daily floor, so the morning pass still re-scans to catch
  staleness the repo introduced around otherwise-unchanged memories, while an
  early next-day pass still inside that window skips.

Two subcommands, each taking the memory directory as a positional argument:

* ``check MEMORY_DIR`` — print ``{scan, reason, signature, last_scan}`` and
  exit 0. ``scan`` is the boolean the skill branches on.
* ``record MEMORY_DIR`` — write the marker (current signature + now) after a
  scan actually ran, so the next ``check`` measures against it.

The marker is ``.memory-staleness-scan.json`` beside the memory directory (its
parent), so it never clutters the curated ``*.md`` store. Stdlib only; a Python
skill-tool under ``.claude/tools/`` — deliberately **not** a Cargo workspace
member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import hashlib
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

DEFAULT_MIN_INTERVAL_HOURS = 20.0
MARKER_NAME = ".memory-staleness-scan.json"
# The analyst's staleness judge — a sub-agent reading every memory against
# HEAD and the board — runs weekly, on its own marker beside the scan's.
JUDGE_INTERVAL_HOURS = 168.0
JUDGE_MARKER_NAME = ".memory-staleness-judge.json"


class MemoryScanGateError(Exception):
    """A user-facing error (bad args, unreadable marker)."""


def store_signature(memory_dir: Path) -> str:
    """A content signature over every ``*.md`` in the store: a hash of each
    file's name and a hash of its bytes, sorted. Any add / remove / edit
    changes it; a non-``.md`` sidecar (the marker) never affects it. Returns the
    empty-store sentinel when the directory is absent or holds no ``*.md``."""
    parts: list[str] = []
    if memory_dir.is_dir():
        for path in sorted(memory_dir.glob("*.md")):
            if not path.is_file():
                continue
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            parts.append(f"{path.name}:{digest}")
    if not parts:
        return "empty"
    return hashlib.sha256("\n".join(parts).encode("utf-8")).hexdigest()


def marker_path(memory_dir: Path) -> Path:
    """The marker sits beside the store, not inside it, so it never shows up in
    the curated ``*.md`` set."""
    return memory_dir.parent / MARKER_NAME


def _read_marker(path: Path) -> dict | None:
    if not path.is_file():
        return None
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (ValueError, OSError, RecursionError):
        # A corrupt marker is treated as absent — the gate just re-scans.
        #
        # `ValueError`, not `json.JSONDecodeError`, which is a strict SUBSET of
        # it: `read_text(encoding="utf-8")` raises `UnicodeDecodeError` (also a
        # `ValueError`) on one invalid byte, and deeply nested JSON raises
        # `RecursionError`. Neither is a `JSONDecodeError`, so both escaped the
        # narrower clause and crashed the command. Same fix as `allowlist.py`.
        return None
    return data if isinstance(data, dict) else None


def _parse_iso(value: object) -> datetime | None:
    if not isinstance(value, str):
        return None
    try:
        parsed = datetime.fromisoformat(value)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed


def decide(
    signature: str,
    marker: dict | None,
    now: datetime,
    min_interval_hours: float = DEFAULT_MIN_INTERVAL_HOURS,
) -> tuple[bool, str]:
    """Pure decision: should the staleness scan run? Returns ``(scan, reason)``.

    ``now`` and ``min_interval_hours`` are injected so the rule is testable
    without a clock. The empty store still "scans" trivially — there is nothing
    to read, so the caller finds nothing, but the gate never suppresses a first
    look.
    """
    if not isinstance(marker, dict):
        return True, "no prior scan recorded"
    if marker.get("signature") != signature:
        return True, "memory store changed since last scan"
    last = _parse_iso(marker.get("last_scan"))
    if last is None:
        return True, "prior scan has no usable timestamp"
    elapsed_hours = (now - last).total_seconds() / 3600.0
    if elapsed_hours >= min_interval_hours:
        return (
            True,
            f"{elapsed_hours:.1f}h since last scan (>= {min_interval_hours:g}h)",
        )
    return False, f"unchanged and only {elapsed_hours:.1f}h since last scan"


def cmd_check(memory_dir: Path, min_interval_hours: float) -> int:
    signature = store_signature(memory_dir)
    marker = _read_marker(marker_path(memory_dir))
    scan, reason = decide(
        signature, marker, datetime.now(timezone.utc), min_interval_hours
    )
    last_scan = marker.get("last_scan") if isinstance(marker, dict) else None
    print(
        json.dumps(
            {
                "scan": scan,
                "reason": reason,
                "signature": signature,
                "last_scan": last_scan,
            },
            indent=2,
        )
    )
    return 0


def judge_marker_path(memory_dir: Path) -> Path:
    return memory_dir.parent / JUDGE_MARKER_NAME


def decide_judge(
    marker: dict | None,
    now: datetime,
    min_interval_hours: float = JUDGE_INTERVAL_HOURS,
) -> tuple[bool, bool, str]:
    """Pure decision for the analyst's staleness judge: ``(judge, first,
    reason)``. Time alone gates it — a memory saved yesterday must not buy a
    full read of the store every morning — and ``first`` (no usable prior run)
    makes it the uncapped full cull."""
    last = _parse_iso(marker.get("last_judged")) if isinstance(marker, dict) else None
    if last is None:
        return True, True, "no prior judge run recorded"
    elapsed_hours = (now - last).total_seconds() / 3600.0
    if elapsed_hours >= min_interval_hours:
        return True, False, f"{elapsed_hours:.1f}h since last judge run"
    return False, False, f"only {elapsed_hours:.1f}h since last judge run"


def cmd_judge_check(memory_dir: Path, min_interval_hours: float) -> int:
    marker = _read_marker(judge_marker_path(memory_dir))
    judge, first, reason = decide_judge(
        marker, datetime.now(timezone.utc), min_interval_hours
    )
    last = marker.get("last_judged") if isinstance(marker, dict) else None
    print(
        json.dumps(
            {"judge": judge, "first": first, "reason": reason, "last_judged": last},
            indent=2,
        )
    )
    return 0


def cmd_judge_record(memory_dir: Path) -> int:
    payload = {"last_judged": datetime.now(timezone.utc).isoformat()}
    judge_marker_path(memory_dir).write_text(
        json.dumps(payload, indent=2) + "\n", encoding="utf-8"
    )
    print(f"memory-scan-gate: recorded judge run of {memory_dir}")
    return 0


def cmd_record(memory_dir: Path) -> int:
    signature = store_signature(memory_dir)
    path = marker_path(memory_dir)
    payload = {
        "signature": signature,
        "last_scan": datetime.now(timezone.utc).isoformat(),
    }
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    print(f"memory-scan-gate: recorded scan of {memory_dir} ({signature[:12]})")
    return 0


HELP = """\
Usage:
  memory_scan_gate.py check MEMORY_DIR [--min-interval-hours H]
      Print {scan, reason, signature, last_scan} — whether the staleness scan
      is warranted. Exit 0.
  memory_scan_gate.py record MEMORY_DIR
      Record that a scan just ran (marker = current signature + now).
  memory_scan_gate.py judge-check MEMORY_DIR [--min-interval-hours H]
      Print {judge, first, reason, last_judged} — whether the analyst's
      weekly staleness judge is due, and whether it is the first (full) cull.
  memory_scan_gate.py judge-record MEMORY_DIR
      Record that a judge run just finished.
  --min-interval-hours H  Floor for `check` (default 20) or `judge-check`
                          (default 168)."""

MODES = ("check", "record", "judge-check", "judge-record")


def _parse_args(args: list[str]) -> tuple[str, Path, float]:
    """Return ``(mode, memory_dir, min_interval_hours)`` or raise."""
    mode: str | None = None
    memory_dir: Path | None = None
    min_interval_hours: float | None = None
    i = 0
    while i < len(args):
        arg = args[i]
        if arg == "--min-interval-hours":
            i += 1
            if i >= len(args):
                raise MemoryScanGateError("--min-interval-hours requires a value")
            try:
                min_interval_hours = float(args[i])
            except ValueError as e:
                raise MemoryScanGateError(
                    f"--min-interval-hours: not a number: {args[i]}"
                ) from e
        elif arg.startswith("-"):
            raise MemoryScanGateError(f"unknown argument: {arg} (try --help)")
        elif mode is None:
            mode = arg
        elif memory_dir is None:
            memory_dir = Path(arg)
        else:
            raise MemoryScanGateError(f"unexpected extra argument: {arg}")
        i += 1
    if mode not in MODES:
        raise MemoryScanGateError(
            "first argument must be one of " + ", ".join(f"'{m}'" for m in MODES)
        )
    if memory_dir is None:
        raise MemoryScanGateError(f"'{mode}' requires a MEMORY_DIR argument")
    if min_interval_hours is None:
        min_interval_hours = (
            JUDGE_INTERVAL_HOURS
            if mode.startswith("judge")
            else DEFAULT_MIN_INTERVAL_HOURS
        )
    return mode, memory_dir, min_interval_hours


def run(argv: list[str]) -> int:
    args = argv[1:]
    if any(a in ("-h", "--help") for a in args):
        print(HELP)
        return 0
    mode, memory_dir, min_interval_hours = _parse_args(args)
    if mode == "check":
        return cmd_check(memory_dir, min_interval_hours)
    if mode == "judge-check":
        return cmd_judge_check(memory_dir, min_interval_hours)
    if mode == "judge-record":
        return cmd_judge_record(memory_dir)
    return cmd_record(memory_dir)


def main() -> int:
    try:
        return run(sys.argv)
    except MemoryScanGateError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
