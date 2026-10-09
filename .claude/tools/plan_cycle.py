#!/usr/bin/env python3
"""The planning hub's cycle mechanics — the `plan` skill's "Cycling" section.

A planning session is the fleet's hub, so its prefix only grows: every consult,
board write and handoff is replayed on every later turn, and session cost is
roughly quadratic in length (``docs/conventions/context-economy.md``). Cycling
bounds that: close out, exit, and let the `plan` launcher relaunch a fresh
session in the same tab under the same display name.

Usage::

    python3 .claude/tools/plan_cycle.py check
    python3 .claude/tools/plan_cycle.py snooze
    python3 .claude/tools/plan_cycle.py cycle
    python3 .claude/tools/plan_cycle.py live-name plan-8

* ``check`` prints one JSON line — the session's prefix (input + cache read +
  cache write on the last assistant usage), its inbound peer-message count, the
  two thresholds, and ``due``. Exit 0 either way; the caller branches on ``due``.
* ``snooze`` is the "No" answer: it moves both thresholds past the current
  values by ``SNOOZE_TOKENS`` / ``SNOOZE_MESSAGES``.
* ``cycle`` writes the marker the launcher looks for, then ends this session's
  client. Run it only after the close-out rewrite has verified.
* ``live-name NAME`` is the launcher's guard: exit 0 when a live session already
  holds NAME, 1 when none does. A relaunch under a live session's name would be
  renamed by the client and break every worker's addressing.

The session's own id is computed exactly as the launcher computes it
(``resolve_session.daily_session_id`` plus today's cycle counter), never found by
listing the projects directory.

Standard library only; tests in ``tests/test_plan_cycle.py``, run via
``make tools-tests``.
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import sys
from datetime import datetime
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import resolve_session as rs  # noqa: E402

#: Ratified starting values (the cycle design, 2026-10-05).
TOKEN_THRESHOLD = 250_000
MESSAGE_THRESHOLD = 40
SNOOZE_TOKENS = 50_000
SNOOZE_MESSAGES = 15

#: How much of the transcript's tail is read to find the last usage. The prefix
#: is on the most recent assistant record, so the head is never needed; a miss
#: falls back to the whole file.
TAIL_BYTES = 1 << 20

#: The marker the launcher's loop checks on exit, beside the substrate markers.
MARKER = "plan-cycle-pending"
SNOOZE = "plan-cycle-snooze"


class PlanCycleError(Exception):
    """A user-facing failure: surfaced to stderr, exits 2."""


def default_repo() -> Path:
    # This file sits at <repo>/.claude/tools/, and a planning session runs in
    # the base checkout, so its own location names the state directory.
    return Path(__file__).resolve().parents[2]


def session_id(repo: Path, date: str) -> str:
    return rs.daily_session_id("plan", date, rs.read_cycle(repo, date))


def transcript_path(repo: Path, sid: str) -> Path:
    return rs.claude_home() / "projects" / rs.slugify(repo) / f"{sid}.jsonl"


def _usage_prefix(line: str) -> int | None:
    try:
        entry = json.loads(line)
    except json.JSONDecodeError:
        return None
    if entry.get("type") != "assistant":
        return None
    usage = (entry.get("message") or {}).get("usage")
    if not isinstance(usage, dict):
        return None
    return sum(
        int(usage.get(k) or 0)
        for k in (
            "input_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
        )
    )


def last_prefix(transcript: Path) -> int:
    """The prefix the next turn replays: the last assistant usage's input side."""
    size = transcript.stat().st_size
    with transcript.open("rb") as fh:
        fh.seek(max(size - TAIL_BYTES, 0))
        tail = fh.read().decode("utf-8", errors="replace")
    lines = tail.splitlines()
    if size > TAIL_BYTES:
        lines = lines[1:]  # the first line of a mid-file seek is partial
    for line in reversed(lines):
        prefix = _usage_prefix(line)
        if prefix is not None:
            return prefix
    if size > TAIL_BYTES:
        for line in reversed(transcript.read_text(errors="replace").splitlines()):
            prefix = _usage_prefix(line)
            if prefix is not None:
                return prefix
    return 0


def inbound_messages(transcript: Path) -> int:
    """User records whose origin is a peer session — one per received message."""
    count = 0
    with transcript.open(encoding="utf-8", errors="replace") as fh:
        for line in fh:
            if '"peer"' not in line:
                continue
            try:
                entry = json.loads(line)
            except json.JSONDecodeError:
                continue
            origin = entry.get("origin")
            if entry.get("type") == "user" and isinstance(origin, dict):
                count += origin.get("kind") == "peer"
    return count


def thresholds(repo: Path, sid: str) -> tuple[int, int]:
    """The snoozed thresholds for this session, or the ratified defaults.

    Keyed by session id, so a snooze never outlives the session it was given in:
    a fresh cycle starts from the defaults.
    """
    try:
        saved = json.loads((repo / rs.STATE_DIR / SNOOZE).read_text())
        if saved.get("sid") == sid:
            return int(saved["tokens"]), int(saved["messages"])
    except (OSError, ValueError, KeyError, TypeError):
        pass
    return TOKEN_THRESHOLD, MESSAGE_THRESHOLD


def check(repo: Path, date: str) -> dict:
    sid = session_id(repo, date)
    transcript = transcript_path(repo, sid)
    if not transcript.is_file():
        raise PlanCycleError(
            f"no transcript for this cycle's session {sid} at {transcript} — "
            f"was this session started by the `plan` launcher?"
        )
    prefix = last_prefix(transcript)
    inbound = inbound_messages(transcript)
    tokens, messages = thresholds(repo, sid)
    return {
        "sid": sid,
        "prefix_tokens": prefix,
        "inbound_messages": inbound,
        "token_threshold": tokens,
        "message_threshold": messages,
        "due": prefix >= tokens or inbound >= messages,
    }


def snooze(repo: Path, date: str) -> dict:
    status = check(repo, date)
    saved = {
        "sid": status["sid"],
        "tokens": max(status["prefix_tokens"], status["token_threshold"])
        + SNOOZE_TOKENS,
        "messages": max(status["inbound_messages"], status["message_threshold"])
        + SNOOZE_MESSAGES,
    }
    state = repo / rs.STATE_DIR
    state.mkdir(parents=True, exist_ok=True)
    (state / SNOOZE).write_text(json.dumps(saved) + "\n")
    return saved


def live_sessions() -> list[dict]:
    """Every Claude Code session registered as running, live pid only.

    The client writes ``<claude_home>/sessions/<pid>.json`` while it runs; a
    crashed client can leave one behind, so the pid is probed rather than trusted.
    """
    found = []
    for path in sorted((rs.claude_home() / "sessions").glob("*.json")):
        try:
            entry = json.loads(path.read_text())
            pid = int(entry["pid"])
        except (OSError, ValueError, KeyError, TypeError):
            continue
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            continue
        except PermissionError:
            pass  # alive, owned by someone else
        found.append(entry)
    return found


def cycle(repo: Path, date: str) -> str:
    """Write the marker, then end this session's client. Returns its pid."""
    sid = session_id(repo, date)
    state = repo / rs.STATE_DIR
    state.mkdir(parents=True, exist_ok=True)
    (state / MARKER).write_text(sid + "\n")
    for entry in live_sessions():
        if entry.get("sessionId") == sid:
            os.kill(int(entry["pid"]), signal.SIGTERM)
            return str(entry["pid"])
    raise PlanCycleError(
        f"marker written, but no live client holds session {sid} — type /exit "
        f"and the launcher relaunches."
    )


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="plan_cycle.py", description="The planning hub's cycle mechanics."
    )
    parser.add_argument(
        "--repo", default=None, help="base checkout (default: this file's repo)"
    )
    parser.add_argument(
        "--date",
        default=None,
        metavar="YYYYMMDD",
        help="the launch date (default: today, local time)",
    )
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("check", help="report prefix, inbound count and whether due")
    sub.add_parser("snooze", help="push both thresholds out (the 'No' answer)")
    sub.add_parser("cycle", help="write the marker and end this client")
    live = sub.add_parser("live-name", help="exit 0 iff a live session holds NAME")
    live.add_argument("name")
    args = parser.parse_args(argv[1:])

    repo = Path(os.path.abspath(args.repo)) if args.repo else default_repo()
    date = args.date or datetime.now().strftime("%Y%m%d")

    if args.command == "check":
        print(json.dumps(check(repo, date)))
    elif args.command == "snooze":
        print(json.dumps(snooze(repo, date)))
    elif args.command == "cycle":
        print(f"cycling: ended client pid {cycle(repo, date)}")
    else:
        holders = [s for s in live_sessions() if s.get("name") == args.name]
        return 0 if holders else 1
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except (PlanCycleError, rs.ResolveSessionError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
