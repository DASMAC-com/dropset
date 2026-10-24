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
  cache write on the last assistant usage), two dollar figures priced at that
  usage's model's ``session_metrics.RATES_BY_MODEL`` row, the thresholds, and
  ``due``. ``replay_usd`` is the prefix at the cache-read rate — the replay a
  cycle saves; ``next_turn_usd`` adds the last turn's cache write, the expected
  next one, at the 1h write rate; ``session_usd`` is the session to date,
  sub-agents included, priced as session metrics prices it. ``due`` fires on
  ``replay_usd``: a fresh session pays the write too, so cycling does not save
  it, and at 40-80x the read rate one tool result's write would otherwise swamp
  the length signal. A seat session records the same model id as a Bedrock one, so it is
  priced at Bedrock rates too, as ``fleet_cost.py`` prices it; only a model
  with no row falls back to the token threshold, and the line says so. Exit 0
  either way; the caller branches on ``due``.
* ``snooze`` is the "No" answer: it moves both thresholds past the current
  values by ``SNOOZE_USD`` / ``SNOOZE_TOKENS``.
* ``cycle`` writes the marker the launcher looks for, then ends this session's
  client. Run it only after the close-out rewrite has verified.
* ``live-name NAME`` is the launcher's guard: exit 0 when a live session already
  holds NAME, 1 when none does. A relaunch under a live session's name would
  give the fleet two sessions answering to it and break worker addressing.

The session's own id is computed exactly as the launcher computes it
(``resolve_session.daily_session_id`` plus the LAUNCH day's cycle counter, the
day arriving as ``DS_PLAN_DATE``), never found by listing the projects
directory.

Standard library only; tests in ``tests/test_plan_cycle.py``, run via
``make tools-tests``.
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import resolve_session as rs  # noqa: E402
import session_metrics as sm  # noqa: E402

#: Ratified starting values (the cycle design, 2026-10-05). Now only the
#: fallback for a model with no rate row, and the source of the dollar values.
TOKEN_THRESHOLD = 250_000
SNOOZE_TOKENS = 50_000

PER_MTOK = 1_000_000

#: The advisor tier's cache-read rate per million tokens when the dollar
#: threshold replaced the token one. The dollar values below are the token
#: points priced at it, so on the advisor model the replay trips at exactly
#: 250k and day-one behavior is unchanged. Re-derive both after about a week of priced planning
#: days from the measured bootstrap cost: a cycle pays when a fresh bootstrap
#: costs less than the replay it saves.
ADVISOR_CACHE_READ = 0.275
USD_THRESHOLD = TOKEN_THRESHOLD * ADVISOR_CACHE_READ / PER_MTOK
SNOOZE_USD = SNOOZE_TOKENS * ADVISOR_CACHE_READ / PER_MTOK

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


def _assistant_usage(line: str) -> tuple[dict, str] | None:
    """An assistant record's usage block and the model it recorded.

    A zero-usage record — a failed request, recorded as a ``<synthetic>`` model
    — is skipped, so the search walks back to the last real prefix. Failures
    cluster in long sessions, exactly where a zero would hide a due cycle.
    """
    try:
        entry = json.loads(line)
    except json.JSONDecodeError:
        return None
    if entry.get("type") != "assistant":
        return None
    message = entry.get("message") or {}
    usage = message.get("usage")
    if not isinstance(usage, dict) or not prefix_of(usage):
        return None
    return usage, sm._model_of(message)


def last_usage(transcript: Path) -> tuple[dict, str]:
    """The last assistant usage — the prefix the next turn replays — and its model."""
    size = transcript.stat().st_size
    with transcript.open("rb") as fh:
        fh.seek(max(size - TAIL_BYTES, 0))
        tail = fh.read().decode("utf-8", errors="replace")
    lines = tail.splitlines()
    if size > TAIL_BYTES:
        lines = lines[1:]  # the first line of a mid-file seek is partial
    for line in reversed(lines):
        found = _assistant_usage(line)
        if found is not None:
            return found
    if size > TAIL_BYTES:
        for line in reversed(transcript.read_text(errors="replace").splitlines()):
            found = _assistant_usage(line)
            if found is not None:
                return found
    return {}, sm.UNRECORDED_MODEL


def _tokens(usage: dict, key: str) -> int:
    return int(usage.get(key) or 0)


def prefix_of(usage: dict) -> int:
    return sum(
        _tokens(usage, k)
        for k in (
            "input_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
        )
    )


def replay_usd(usage: dict, rates: sm.Rates) -> float:
    """The whole prefix read back from cache: what grows with length."""
    return prefix_of(usage) * rates.cache_read / PER_MTOK


def next_turn_usd(usage: dict, rates: sm.Rates) -> float:
    """The next turn's cost: the replay plus a cache write the size of the last
    one. Output is left out — it is not known yet, and it is not what grows
    with the prefix.
    """
    written = _tokens(usage, "cache_creation_input_tokens")
    return replay_usd(usage, rates) + written * rates.cache_write_1h / PER_MTOK


def session_usd(transcript: Path, sid: str) -> float | None:
    """The session to date, sub-agents included, or ``None`` when any billable
    model in it has no rate row (session metrics withholds a partial sum).
    """
    cost = sm.aggregate(transcript, sid)["total_cost"]
    return None if cost is None else cost.total()


def thresholds(repo: Path, sid: str) -> tuple[float, int]:
    """The snoozed thresholds for this session, or the ratified defaults.

    Keyed by session id, so a snooze never outlives the session it was given in:
    a fresh cycle starts from the defaults. A file in the retired message-count
    shape reads as no snooze.
    """
    try:
        saved = json.loads((repo / rs.STATE_DIR / SNOOZE).read_text())
        if saved.get("sid") == sid:
            return float(saved["usd"]), int(saved["tokens"])
    except (OSError, ValueError, KeyError, TypeError, AttributeError):
        pass
    return USD_THRESHOLD, TOKEN_THRESHOLD


def check(repo: Path, date: str) -> dict:
    sid = session_id(repo, date)
    transcript = transcript_path(repo, sid)
    if not transcript.is_file():
        raise PlanCycleError(
            f"no transcript for this cycle's session {sid} at {transcript} — "
            f"was this session started by the `plan` launcher?"
        )
    usage, model = last_usage(transcript)
    prefix = prefix_of(usage)
    usd, tokens = thresholds(repo, sid)
    status = {
        "sid": sid,
        "model": model,
        "prefix_tokens": prefix,
        "token_threshold": tokens,
        "usd_threshold": round(usd, 6),
    }
    rates = sm.RATES_BY_MODEL.get(model)
    if rates is None:
        status.update(
            replay_usd=None,
            next_turn_usd=None,
            session_usd=None,
            fallback=f"no rate row for {model}: due on the token threshold",
            due=prefix >= tokens,
        )
        return status
    replay = replay_usd(usage, rates)
    session = session_usd(transcript, sid)
    status.update(
        replay_usd=round(replay, 6),
        next_turn_usd=round(next_turn_usd(usage, rates), 6),
        session_usd=None if session is None else round(session, 6),
        due=replay >= usd,
    )
    return status


def snooze(repo: Path, date: str) -> dict:
    status = check(repo, date)
    replay = status["replay_usd"] or 0.0
    saved = {
        "sid": status["sid"],
        "usd": max(replay, status["usd_threshold"]) + SNOOZE_USD,
        "tokens": max(status["prefix_tokens"], status["token_threshold"])
        + SNOOZE_TOKENS,
    }
    state = repo / rs.STATE_DIR
    state.mkdir(parents=True, exist_ok=True)
    (state / SNOOZE).write_text(json.dumps(saved) + "\n")
    return saved


def live_sessions() -> list[dict]:
    """Every Claude Code session registered as running, live pid only.

    The client writes ``<claude_home>/sessions/<pid>.json`` while it runs; a
    crashed client can leave one behind, so the pid is probed rather than trusted.
    A pid owned by another user is not live for this purpose: the registry is
    per-user, so such a pid can only be a stale file's number reused.
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
        except (ProcessLookupError, PermissionError):
            continue
        found.append(entry)
    return found


def _started(entry: dict) -> int:
    """A registry entry's start time; malformed reads as oldest, never raises."""
    try:
        return int(entry.get("startedAt") or 0)
    except (TypeError, ValueError):
        return 0


def cycle(repo: Path, date: str) -> str:
    """Write the marker, then end this session's client. Returns its pid.

    The id is deterministic, so a crashed earlier client of the same session
    can leave a registry file naming it; when several live entries match, the
    most recently started is this one.
    """
    sid = session_id(repo, date)
    state = repo / rs.STATE_DIR
    state.mkdir(parents=True, exist_ok=True)
    (state / MARKER).write_text(sid + "\n")
    mine = [e for e in live_sessions() if e.get("sessionId") == sid]
    if mine:
        entry = max(mine, key=_started)
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
        help="the launch date (default: $DS_PLAN_DATE, which the launcher "
        "sets, else today)",
    )
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("check", help="report prefix, dollar figures and whether due")
    sub.add_parser("snooze", help="push both thresholds out (the 'No' answer)")
    sub.add_parser("cycle", help="write the marker and end this client")
    live = sub.add_parser("live-name", help="exit 0 iff a live session holds NAME")
    live.add_argument("name")
    args = parser.parse_args(argv[1:])

    repo = Path(os.path.abspath(args.repo)) if args.repo else default_repo()
    date = args.date or rs.launch_date()

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
