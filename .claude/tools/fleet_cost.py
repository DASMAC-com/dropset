#!/usr/bin/env python3
"""Price the fleet's Bedrock spend from transcripts and reconcile it with the bill.

The fleet cost picture, Bedrock billed and metered, the seat metered alone.
Four subcommands over **complete UTC days** — AWS reports in UTC, so every
day boundary there is a UTC one, on both sides — and ``local``, the one view
bucketed by the machine's own day:

``meter``
    Walk every transcript under the Claude projects directory, sub-agents
    included, count each message's usage **once per message id** (a streamed
    message is written one line per content block, each repeating the same
    usage — pricing lines instead of messages inflated cost 2.2 to 2.6x when it
    was measured), bucket it by the UTC day of the record and the model it
    recorded, and price it at ``session_metrics.RATES_BY_MODEL``. Sessions are
    split by substrate, which comes from the launch marker and **never** from
    the transcript's model id (a seat and a Bedrock session record the same
    name). Every session is also keyed to its Linear issue the same way for
    every substrate — the worktree tag, or for a base-checkout ``chore`` its
    bootstrap prompt, which also keys an issue-less ``plan`` session by its
    verb: Bedrock issues are priced, seat ones are priced too but labelled at
    Bedrock rates, not billed (the subscription is those rates subsidized, and
    never enters the Bedrock total), and unattributed ones are listed by
    session count beside them.

``reconcile``
    ``meter`` beside Cost Explorer's billed gross for the same days, per
    Marketplace service, with the drift and a flag past ``--tolerance``. Both
    print a **friction** line on a day where some session spanned a UTC day,
    rebased or re-enqueued, or stalled: those session counts, and the
    main-session dollars spent after the friction began, so a day's process
    cost sits beside its bill.

``spend-cap``
    The daily spend cap's CloudWatch metric-math **estimate** beside the same
    billed gross: the four Bedrock token metrics summed over the UTC day and
    priced at the deployed stack's ``TokenRate*`` parameters, exactly as the
    alarms do. Billed above the estimate means the cap is leaking; the estimate
    far above billed means it trips early.

``rates``
    For one day, the per-million rate each Bedrock-edition service billed per
    tier (cost over quantity by usage type), every usage type listed by name so
    a long-context or other tier shows rather than averaging in, and whether
    the model's ``RATES_BY_MODEL`` row reproduces the bill to the cent. A
    provisional day is fine: a rate is a ratio, so a partly billed day yields
    it too.

``local``
    The last seven local days (the machine zone, midnight to midnight), the
    last of them yesterday, then today so far: **billed**, from Cost
    Explorer's hourly buckets over the Bedrock-edition services, each hour on
    the local day it starts in, beside **metered**, the transcript meter at
    Bedrock rates over the same windows, with the seat listed apart. Hourly
    data needs the payer account's hourly-granularity preference; while it
    is off the billed column reads ``hourly granularity off``. Hourly data
    lags by hours, so the heading names the last hour billed. The meter is
    only as complete as the transcripts on disk, and the purge ages them out,
    so an older day can meter low where its bill does not. The settled
    ``reconcile`` stays UTC; this view is never reconciled.

**Substrate classes.** ``bedrock`` and ``seat`` come from a marker: the
session's own record (``sessions/<id>``, written when the launch creates
it) first, then its worktree's tag marker. A worktree session with **no**
marker (or an unrecognized one) is ``unattributed`` and is never priced into
the Bedrock total — that is the share the bill comparison cannot account for,
reported rather than guessed. ``explore`` landed here until it wrote a marker;
an older explore worktree still does. A base-repo session with no record reads
as ``seat``. A session launched before the record still classifies the old
way.

**The bill is matched per service, not by a fixed list of names.** Cost
Explorer files each model as its own Marketplace service, ``Claude <Family>
<version> (Amazon Bedrock Edition)``, under record type ``Usage``; the bare
``Amazon Bedrock`` service dimension reads zero every day and is a convincing
false negative. Each new model adds a service, so the read takes every service
carrying the edition suffix and maps transcript models onto them by name. A
billed service with no transcript tokens that day is **background** traffic
(harness calls on another model, which no transcript records) and is reported
as its own line, and flagged past ``--tolerance`` of the day's gross — a
background share that large means the meter missed a session model's own spend.
A model that carried tokens but has no rate bills as **unpriced**, not as
background. The auto-mode permission classifier runs on the session's own
model, so it bills inside the matched service, and is allowed for by the
measured :data:`CLASSIFIER_OVERHEAD` before drift is judged.

**D-1 is provisional.** Cost Explorer lands charges hours to a day late, so the
default window is D-2 (settled) and D-1 (marked provisional, and never counted
toward the exit status, since a partly billed day reads as drift). Today's UTC
date is refused.

Stdout is a ranked summary (about ten lines a day); ``--json`` emits the full
breakdown. Exit codes: 0 reported with no settled day flagged; 1 at least one
settled day flagged (for ``rates``: a row that misses its bill, or cannot be
judged because an unrecognized usage type billed, on any day); 2 a usage error,
or an AWS call that failed or returned a shape this tool does not recognize.

Usage::

    python3 .claude/tools/fleet_cost.py meter [--day YYYY-MM-DD]... [--top N]
    python3 .claude/tools/fleet_cost.py reconcile [--day YYYY-MM-DD]...
    python3 .claude/tools/fleet_cost.py spend-cap [--day YYYY-MM-DD]...
    python3 .claude/tools/fleet_cost.py rates YYYY-MM-DD
    python3 .claude/tools/fleet_cost.py local [--json]

The AWS reads run the CLI under ``DS_AWS_PROFILE`` when it is set, else the
CLI's own default. Stdlib only; a Python skill-tool under ``.claude/tools/`` —
deliberately not a Cargo workspace member. Tests live in
``tests/test_fleet_cost.py``.
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

import session_metrics as sm

BEDROCK = "bedrock"
SEAT = "seat"
UNATTRIBUTED = "unattributed"

EDITION_SUFFIX = " (Amazon Bedrock Edition)"
SPEND_CAP_STACK = "dropset-bedrock-agent"
# The four metrics the spend-cap alarms sum, keyed by the stack parameter that
# prices each. Dimensionless, so every model counts, as in the alarms.
#
# The stack's defaults mirror the Opus 5.5 row of `RATES_BY_MODEL`, so every
# token is estimated at Opus rates whatever model spent it. Against the other
# rows that is conservative for Sonnet 5.5 (0.5x on every tier) and Haiku
# 5.5, but NOT for Fable 5.1: a Fable
# token bills 2.5x the estimate on input, output and cache write, and 1.25x
# on cache read, so a Fable-heavy day under-estimates and can leak past the
# cap. Changing the stack parameters is an operator deploy decision.
SPEND_CAP_METRICS = {
    "InputTokenCount": "TokenRateInput",
    "OutputTokenCount": "TokenRateOutput",
    "CacheReadInputTokenCount": "TokenRateCacheRead",
    "CacheWriteInputTokenCount": "TokenRateCacheWrite",
}
DEFAULT_TOLERANCE = 0.15
# Below this many dollars on both sides of a comparison, a day is reported but
# never judged: a quiet day carrying only cents of harness calls would otherwise
# read as 100 percent background or drift and flag. A fleet day is hundreds.
MIN_JUDGED_DOLLARS = 5.0
# The share of a day's matched bill no transcript records: the auto-mode
# permission classifier, which since the Opus 5.5 swap runs on the session's
# own model and so bills inside the same service rather than as a background
# line of its own. Measured 2026-10-08, 10-09 and 10-13 at 8.3, 8.1 and 8.3
# percent, with the gap mostly fresh input and output (about 85 percent of the
# missing dollars; a stateless call per tool use) while both cache tiers matched
# within about 2 percent. The
# Opus 5.5 rates were verified the same day: billed quantity times
# `RATES_BY_MODEL` reproduced the bill to the cent. Drift is flagged AFTER this
# is applied, so a metering bug of the same size cannot hide inside it;
# re-measure it when the model or the permission mode changes.
CLASSIFIER_OVERHEAD = 0.082


# --------------------------------------------------------------------------- #
# Classification.
# --------------------------------------------------------------------------- #


def classify(cwd: str | None, session_id: str | None = None) -> str:
    """A session's substrate class, from its launch marker.

    Unlike ``session_metrics.resolve_substrate``, which degrades an unknown to
    seat because a single-session report must never invent a dollar figure, a
    fleet total needs the third answer: a worktree session with no marker is
    neither priced nor silently waved through as seat.
    """
    recorded = sm.read_substrate_marker(cwd, session_id)
    if recorded == sm.SUBSTRATE_BEDROCK:
        return BEDROCK
    if recorded in (sm.SUBSTRATE_SEAT, sm.SUBSTRATE_ANTHROPIC):
        return SEAT
    if recorded is None and sm.base_repo_from_cwd(cwd) is None:
        return SEAT
    return UNATTRIBUTED


def issue_of(cwd: str | None) -> str | None:
    """The Linear issue a worktree session ran for, from its tag.

    Every issue-keyed verb names its worktree the bare number (``1651``); the
    pre-rename ``eng-`` / ``ceo-`` / ``exp-<n>`` spellings name ``ENG-<n>``
    too. A tag that is not an issue number is returned as-is, so it still
    groups.
    """
    tag = sm.tag_from_cwd(cwd)
    if not tag:
        return None
    match = re.fullmatch(r"(?:eng-|ceo-|exp-)?(\d+)", tag, flags=re.IGNORECASE)
    return f"ENG-{int(match.group(1))}" if match else tag


# The bootstrap prompt of an issue-keyed launch with no worktree to name it —
# `chore <n>` types `/chore <n>` in the base checkout — as the transcript
# records a slash command, or as typed.
PROMPT_ISSUE_RE = re.compile(
    r"<command-name>/(?:chore|architect|explore)</command-name>\s*"
    r"<command-args>\s*(?:ENG-)?(\d+)\b"
    r"|^/(?:chore|architect|explore)\s+(?:ENG-)?(\d+)\b",
    re.IGNORECASE,
)


def issue_of_prompt(text: str) -> str | None:
    """The issue a base-checkout session ran for, from its first prompt."""
    match = PROMPT_ISSUE_RE.search(text.strip())
    if not match:
        return None
    return f"ENG-{int(match.group(1) or match.group(2))}"


# The bootstrap prompt of a launch with no issue at all: the scheduler's `plan`
# types `/plan` in the base checkout, the curator's `curate` types `/curate`
# and the morning launcher's `launch` types `/launch`, as the retired
# `housekeeping` typed `/housekeeping`.
PROMPT_VERB_RE = re.compile(
    r"<command-name>/(plan|curate|launch|housekeeping)</command-name>"
    r"|^/(plan|curate|launch|housekeeping)(?:\s|$)",
    re.IGNORECASE,
)


def verb_of_prompt(text: str) -> str | None:
    """The verb an issue-less base-checkout session ran as, from its first
    prompt — the key it groups under in the per-issue table, so each seat
    cycle's cost is a line of its own rather than part of ``(no issue)``."""
    match = PROMPT_VERB_RE.search(text.strip())
    if not match:
        return None
    return (match.group(1) or match.group(2)).lower()


def service_for_model(model: str) -> str | None:
    """The Cost Explorer service a transcript model id bills under.

    ``claude-opus-5-5`` -> ``Claude Opus 5.5 (Amazon Bedrock Edition)``. A
    dated suffix (``-20251001``) is dropped, as is the Bedrock spelling a
    sub-agent can record (``us.anthropic.claude-haiku-4-5-20251001-v1:0``).
    ``None`` for an id that is not a Claude model (``<synthetic>``, the
    unrecorded sentinel).
    """
    match = re.fullmatch(
        r"(?:[a-z]+\.)?(?:anthropic\.)?claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?"
        r"(?:-\d{8})?(?:-v\d+:\d+)?",
        model,
    )
    if not match:
        return None
    family, major, minor = match.groups()
    version = f"{major}.{minor}" if minor else major
    return f"Claude {family.capitalize()} {version}{EDITION_SUFFIX}"


# --------------------------------------------------------------------------- #
# The transcript walk.
# --------------------------------------------------------------------------- #


@dataclass
class Meter:
    """Deduplicated token counts, bucketed per UTC day, substrate and model,
    plus the same per issue — every substrate, keyed one way, so a seat
    session carries its issue as a Bedrock one does.
    """

    days: set[str]
    by_day: dict[tuple[str, str, str], sm.Tokens] = field(default_factory=dict)
    # (day, substrate, issue, model)
    by_issue: dict[tuple[str, str, str, str], sm.Tokens] = field(default_factory=dict)
    # (day, substrate, issue) -> session ids
    issue_sessions: dict[tuple[str, str, str], set[str]] = field(default_factory=dict)
    sessions: dict[tuple[str, str], set[str]] = field(default_factory=dict)
    seen: set[str] = field(default_factory=set)
    parse_errors: int = 0
    # Transcripts that vanished or became unreadable mid-walk (a purge running
    # beside a live fleet), skipped rather than allowed to abort the run.
    unreadable: int = 0
    # Per day: sessions that spanned a UTC day, that rebased or re-enqueued,
    # and that stalled, and the main-session tokens spent after each
    # session's friction began.
    spanned: dict[str, set[str]] = field(default_factory=dict)
    rebased: dict[str, set[str]] = field(default_factory=dict)
    stalled: dict[str, set[str]] = field(default_factory=dict)
    # (day, substrate, model)
    friction_after: dict[tuple[str, str, str], sm.Tokens] = field(default_factory=dict)
    # Message and tool-use ids every transcript's `Friction` has counted, one
    # set across the walk for the reason `seen` is.
    friction_seen: set[str] = field(default_factory=set)
    # A record's timestamp -> the day it buckets under: the UTC date unless
    # `local` swaps in the machine zone's. Friction stays UTC-keyed either way.
    day_of: Callable[[str], str] = lambda stamp: stamp[:10]

    def ingest(
        self, line: str, substrate: str, issue: str | None, session: str
    ) -> dict | None:
        """Meter one line; returns the parsed record for any further reader."""
        if not line.strip():
            return None
        try:
            rec = json.loads(line)
        except ValueError:
            self.parse_errors += 1
            return None
        if not isinstance(rec, dict):
            return None
        self._ingest_record(rec, substrate, issue, session)
        return rec

    def add_friction(self, friction: sm.Friction, substrate: str, session: str) -> None:
        """Roll one main transcript's friction into its days."""
        spans = len(friction.days) > 1
        for day in friction.days & self.days:
            if spans:
                self.spanned.setdefault(day, set()).add(session)
        for event in friction.events:
            if event.at and event.at[:10] in self.days:
                self.rebased.setdefault(event.at[:10], set()).add(session)
        if friction.stall_day in self.days:
            self.stalled.setdefault(friction.stall_day, set()).add(session)
        start = friction.start_index()
        if start is None:
            return
        for stamp, model, tokens in friction.timeline[start:]:
            if stamp is None or stamp[:10] not in self.days:
                continue
            acc = self.friction_after.setdefault(
                (stamp[:10], substrate, model), sm.Tokens()
            )
            acc.input += tokens.input
            acc.output += tokens.output
            acc.cache_creation += tokens.cache_creation
            acc.cache_read += tokens.cache_read

    def _ingest_record(
        self, rec: dict, substrate: str, issue: str | None, session: str
    ) -> None:
        msg = rec.get("message")
        if not isinstance(msg, dict) or not isinstance(msg.get("usage"), dict):
            return
        day = self.day_of(str(rec.get("timestamp", "")))
        if day not in self.days:
            return
        # One count per message id across the WHOLE walk, not per file: a
        # resumed session can carry its parent's messages forward into a new
        # transcript, and those are the same billed requests.
        msg_id = msg.get("id")
        if isinstance(msg_id, str) and msg_id:
            if msg_id in self.seen:
                return
            self.seen.add(msg_id)
        model = sm._model_of(msg)
        self.by_day.setdefault((day, substrate, model), sm.Tokens()).add(msg["usage"])
        issue = issue or NO_ISSUE
        key = (day, substrate, issue, model)
        self.by_issue.setdefault(key, sm.Tokens()).add(msg["usage"])
        self.issue_sessions.setdefault((day, substrate, issue), set()).add(session)
        self.sessions.setdefault((day, substrate), set()).add(session)


NO_ISSUE = "(no issue)"


def _prompt_text(rec: dict) -> str:
    content = (rec.get("message") or {}).get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(
            str(part.get("text", ""))
            for part in content
            if isinstance(part, dict) and part.get("type") == "text"
        )
    return ""


def _read_head(path: Path) -> tuple[str | None, str | None]:
    """``(cwd, issue or verb from the bootstrap prompt)`` — the prompt names
    the issue a base-checkout launch ran for, where no worktree tag does, or
    the verb of one with no issue (``plan``). The first few user records are
    tried, not only the first: a caveat or hook record can precede the
    command."""
    cwd = None
    users = 0
    for line in sm._read_lines(path):
        try:
            rec = json.loads(line)
        except ValueError:
            continue
        if not isinstance(rec, dict):
            continue
        if cwd is None and isinstance(rec.get("cwd"), str) and rec["cwd"].strip():
            cwd = rec["cwd"].strip()
        if rec.get("type") == "user":
            text = _prompt_text(rec)
            issue = issue_of_prompt(text) or verb_of_prompt(text)
            users += 1
            if issue or users >= HEAD_USER_RECORDS:
                return cwd, issue
    return cwd, None


# How many leading user records may hold the bootstrap prompt.
HEAD_USER_RECORDS = 3


def _oldest_first(paths) -> list[Path]:
    """``paths`` by last write, then name; one that vanished is dropped."""
    dated = []
    for path in paths:
        try:
            dated.append((path.stat().st_mtime, path.name, path))
        except OSError:
            continue
    return [path for _, _, path in sorted(dated)]


def walk(
    projects: Path,
    days: list[str],
    day_of: Callable[[str], str] | None = None,
    floor: float | None = None,
) -> Meter:
    """Meter every transcript that could hold a record on ``days``.

    A transcript last written before the first day cannot hold one, so it is
    skipped unread; a sub-agent transcript always finishes before its parent's
    last write, so the parent's mtime gates both. ``day_of`` and ``floor``
    re-bucket the walk (``local``): the day a timestamp falls on, and the
    epoch second the first one starts — UTC midnight of the first day when
    not given.

    A project's transcripts are walked oldest-written first. Every count is
    first-seen-wins across the walk, and a resumed session's parent stops
    being written before its child exists, so the parent's records stay the
    parent's. A parent last written before the window is skipped, and its
    child then carries those records as its own: a session resumed across
    days did span them, and the idle time between is a real stall.
    """
    meter = Meter(days=set(days))
    if day_of is not None:
        meter.day_of = day_of
    if floor is None:
        floor = (
            datetime.datetime.fromisoformat(min(days))
            .replace(tzinfo=datetime.timezone.utc)
            .timestamp()
        )
    if not projects.is_dir():
        return meter
    for project in sorted(projects.iterdir()):
        if not project.is_dir():
            continue
        for transcript in _oldest_first(project.glob("*.jsonl")):
            try:
                if transcript.stat().st_mtime < floor:
                    continue
            except OSError:
                continue
            try:
                cwd, prompted = _read_head(transcript)
            except OSError:
                meter.unreadable += 1
                continue
            session = transcript.stem
            substrate, issue = classify(cwd, session), issue_of(cwd) or prompted
            files = [
                transcript,
                *sorted((project / session / "subagents").glob("*.jsonl")),
            ]
            friction = sm.Friction(meter.friction_seen)
            for path in files:
                try:
                    for line in sm._read_lines(path):
                        rec = meter.ingest(line, substrate, issue, session)
                        if rec is not None and path == transcript:
                            friction.observe(rec)
                except OSError:
                    meter.unreadable += 1
            meter.add_friction(friction, substrate, session)
    return meter


# --------------------------------------------------------------------------- #
# Pricing.
# --------------------------------------------------------------------------- #


def price(tokens: sm.Tokens, model: str) -> float | None:
    """Dollars at ``model``'s Bedrock rate, or ``None`` when it has none."""
    try:
        return sm.Cost.price(tokens, model).total()
    except sm.UnpricedModel:
        return None


def day_summary(meter: Meter, day: str) -> dict:
    """One day's metered figures: per-substrate totals, the Bedrock share per
    service, and any model that carried tokens but has no rate.
    """
    out: dict = {
        "day": day,
        "substrates": {},
        "bedrock_by_service": {},
        "bedrock_tokens": {},
        "unpriced": [],
    }
    for (d, substrate, model), tokens in sorted(meter.by_day.items()):
        if d != day or not tokens.billable():
            continue
        if substrate == BEDROCK:
            out["bedrock_tokens"][model] = vars(tokens)
        dollars = price(tokens, model)
        entry = out["substrates"].setdefault(
            substrate, {"dollars": 0.0, "sessions": 0, "tokens": 0}
        )
        entry["sessions"] = len(meter.sessions.get((day, substrate), ()))
        entry["tokens"] += (
            tokens.input + tokens.output + tokens.cache_creation + tokens.cache_read
        )
        if dollars is None:
            if substrate == BEDROCK:
                out["unpriced"].append(model)
            continue
        entry["dollars"] += dollars
        if substrate == BEDROCK:
            service = service_for_model(model) or model
            out["bedrock_by_service"][service] = (
                out["bedrock_by_service"].get(service, 0.0) + dollars
            )
    friction = friction_summary(meter, day)
    if friction:
        out["friction"] = friction
    return out


def friction_summary(meter: Meter, day: str) -> dict | None:
    """The day's friction rollup, or ``None`` on a day without any: sessions
    that spanned a UTC day, that rebased or re-enqueued, and that stalled, and
    the main-session dollars each substrate spent that day after its
    sessions' friction began (the earliest of those events — see
    ``session_metrics.Friction.start_index``)."""
    spanned = meter.spanned.get(day, set())
    rebased = meter.rebased.get(day, set())
    stalled = meter.stalled.get(day, set())
    after_today = any(d == day for d, _, _ in meter.friction_after)
    if not (spanned or rebased or stalled or after_today):
        return None
    after: dict[str, float] = {}
    for (d, substrate, model), tokens in sorted(meter.friction_after.items()):
        if d == day:
            after[substrate] = after.get(substrate, 0.0) + (price(tokens, model) or 0.0)
    return {
        "spanned_sessions": len(spanned),
        "rebased_sessions": len(rebased),
        "stalled_sessions": len(stalled),
        "dollars_after": after,
    }


def render_friction(friction: dict) -> str:
    after = friction["dollars_after"]
    parts = [f"{money(after.get(BEDROCK, 0.0))} Bedrock"]
    if after.get(SEAT):
        parts.append(f"{money(after[SEAT])} seat {sm.NOT_BILLED}")
    if after.get(UNATTRIBUTED):
        parts.append(f"{money(after[UNATTRIBUTED])} unattributed")
    return (
        "{} session(s) spanned a day, {} rebased or re-enqueued, "
        "{} stalled; {} after".format(
            friction["spanned_sessions"],
            friction["rebased_sessions"],
            friction["stalled_sessions"],
            ", ".join(parts),
        )
    )


def issue_costs(
    meter: Meter, days: list[str], substrate: str = BEDROCK
) -> list[tuple[str, float]]:
    """Dollars per issue on one substrate over ``days``, costliest first."""
    totals: dict[str, float] = {}
    for (day, sub, issue, model), tokens in meter.by_issue.items():
        if day in days and sub == substrate:
            totals[issue] = totals.get(issue, 0.0) + (price(tokens, model) or 0.0)
    return sorted(totals.items(), key=lambda kv: (-kv[1], kv[0]))


def issue_session_counts(
    meter: Meter, days: list[str], substrate: str
) -> list[tuple[str, int]]:
    """Sessions per issue on one non-Bedrock class over ``days``, most first.
    Listed rather than priced: seat spend has no bill line to reconcile."""
    by_issue: dict[str, set[str]] = {}
    for (day, sub, issue), sessions in meter.issue_sessions.items():
        if day in days and sub == substrate:
            by_issue.setdefault(issue, set()).update(sessions)
    counts = [(issue, len(s)) for issue, s in by_issue.items()]
    return sorted(counts, key=lambda kv: (-kv[1], kv[0]))


# --------------------------------------------------------------------------- #
# AWS reads, through the CLI.
# --------------------------------------------------------------------------- #


class AwsError(RuntimeError):
    pass


def aws(args: list[str]) -> dict:
    cmd = ["aws", *args, "--output", "json"]
    profile = os.environ.get("DS_AWS_PROFILE", "").strip()
    if profile:
        cmd += ["--profile", profile]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise AwsError(f"aws {args[0]} {args[1]}: {exc}") from exc
    if proc.returncode != 0:
        tail = proc.stderr.strip().splitlines()[-1:] or ["no stderr"]
        raise AwsError(f"aws {args[0]} {args[1]} exited {proc.returncode}: {tail[0]}")
    try:
        return json.loads(proc.stdout or "{}")
    except ValueError as exc:
        raise AwsError(f"aws {args[0]} {args[1]}: output is not JSON") from exc


# What reading a field off a response of the wrong shape raises: a missing key
# or index, a non-dict body (`AttributeError` on `.get`), a non-numeric amount.
SHAPE_ERRORS = (AttributeError, KeyError, IndexError, TypeError, ValueError)


def _shape_error(what: str, exc: Exception) -> AwsError:
    """A response missing a field this tool reads. Raised as `AwsError` so it
    exits 2 rather than escaping as a traceback, whose status 1 would read as a
    flagged day.
    """
    return AwsError(f"unexpected {what} response: {type(exc).__name__}: {exc}")


def next_day(day: str) -> str:
    return (datetime.date.fromisoformat(day) + datetime.timedelta(days=1)).isoformat()


def parse_billed(response: dict) -> dict[str, dict[str, float]]:
    """Cost Explorer's grouped response -> ``{day: {service: dollars}}``,
    keeping only the Bedrock-edition services.
    """
    out: dict[str, dict[str, float]] = {}
    for period in response.get("ResultsByTime", []):
        day = period["TimePeriod"]["Start"]
        services = out.setdefault(day, {})
        for group in period.get("Groups", []):
            service = group["Keys"][0]
            if service.endswith(EDITION_SUFFIX):
                services[service] = float(group["Metrics"]["UnblendedCost"]["Amount"])
    return out


def fetch_billed(days: list[str]) -> dict[str, dict[str, float]]:
    response = aws(
        [
            "ce",
            "get-cost-and-usage",
            "--time-period",
            f"Start={min(days)},End={next_day(max(days))}",
            "--granularity",
            "DAILY",
            "--metrics",
            "UnblendedCost",
            "--filter",
            json.dumps({"Dimensions": {"Key": "RECORD_TYPE", "Values": ["Usage"]}}),
            "--group-by",
            "Type=DIMENSION,Key=SERVICE",
        ]
    )
    try:
        billed = parse_billed(response)
    except SHAPE_ERRORS as exc:
        raise _shape_error("Cost Explorer", exc) from exc
    return {day: billed.get(day, {}) for day in days}


def parse_usage(response: dict) -> dict[str, dict[str, tuple[float, float]]]:
    """Cost Explorer's response grouped by service and usage type ->
    ``{service: {usage_type: (quantity, dollars)}}``, Bedrock-edition services
    only, summed over every period in the response. A response carrying a
    ``NextPageToken`` is refused rather than read as the whole day.
    """
    if response.get("NextPageToken"):
        raise ValueError("response is paginated; only the first page arrived")
    out: dict[str, dict[str, tuple[float, float]]] = {}
    for period in response.get("ResultsByTime", []):
        for group in period.get("Groups", []):
            service, usage_type = group["Keys"]
            if not service.endswith(EDITION_SUFFIX):
                continue
            metrics = group["Metrics"]
            quantity = float(metrics["UsageQuantity"]["Amount"])
            dollars = float(metrics["UnblendedCost"]["Amount"])
            q, d = out.setdefault(service, {}).get(usage_type, (0.0, 0.0))
            out[service][usage_type] = (q + quantity, d + dollars)
    return out


def fetch_usage(day: str) -> dict[str, dict[str, tuple[float, float]]]:
    response = aws(
        [
            "ce",
            "get-cost-and-usage",
            "--time-period",
            f"Start={day},End={next_day(day)}",
            "--granularity",
            "DAILY",
            "--metrics",
            "UsageQuantity",
            "UnblendedCost",
            "--filter",
            json.dumps({"Dimensions": {"Key": "RECORD_TYPE", "Values": ["Usage"]}}),
            "--group-by",
            "Type=DIMENSION,Key=SERVICE",
            "Type=DIMENSION,Key=USAGE_TYPE",
        ]
    )
    try:
        return parse_usage(response)
    except SHAPE_ERRORS as exc:
        raise _shape_error("Cost Explorer", exc) from exc


# A usage type reads `<region>-MP:<region>_<tier>_tokens_standard-Units` (an
# older model files `InputTokenCount` / `OutputTokenCount` instead), and its
# `UsageQuantity` is in MILLIONS of tokens, so cost over quantity is already
# the per-million rate. Only the standard service tier and the 1h write map to a
# `Rates` field; anything else (a 5-minute write, a long-context or priority
# tier) is reported by name and kept out of every rate.
# Matched WHOLE, region prefix included, so a qualifier on either side of the
# tier (`USW2_batch_input_tokens_standard`) falls through to unrecognized.
USAGE_TIER_RE = re.compile(
    r"[A-Z0-9]+-MP:[A-Z0-9]+_(?:"
    r"(?:(input|output|cache_read)_tokens|(cache_write)_tokens_1h)_standard"
    r"|(Input|Output)TokenCount"
    r")-Units"
)


def tier_of(usage_type: str) -> str | None:
    """The `Rates` field a usage type bills, or ``None`` for one this tool
    does not recognize (a long-context or other tier, reported by name)."""
    match = USAGE_TIER_RE.fullmatch(usage_type)
    if not match:
        return None
    plain, write, legacy = match.groups()
    if write:
        return "cache_write_1h"
    return plain or legacy.lower()


def model_for_service(service: str) -> str | None:
    """The `RATES_BY_MODEL` key that bills under ``service``, if any."""
    for model in sm.RATES_BY_MODEL:
        if service_for_model(model) == service:
            return model
    return None


def derive_rates(usage: dict[str, tuple[float, float]]) -> dict:
    """One service's usage types -> the per-million rate each tier billed at,
    as cost over quantity, beside every usage type with its own figures.

    Usage types are summed per tier before dividing, so a tier billed under two
    names (say, one per region) still yields one rate; an unrecognized type is
    left out of every rate and listed, never averaged in.
    """
    tiers: dict[str, list[float]] = {}
    types = []
    for usage_type, (quantity, dollars) in sorted(usage.items()):
        tier = tier_of(usage_type)
        rate = dollars / quantity if quantity else None
        types.append(
            {
                "usage_type": usage_type,
                "tier": tier,
                "quantity": quantity,
                "dollars": dollars,
                "rate": rate,
            }
        )
        if tier:
            q, d = tiers.setdefault(tier, [0.0, 0.0])
            tiers[tier] = [q + quantity, d + dollars]
    rates = {tier: (d / q if q else None) for tier, (q, d) in tiers.items()}
    return {"rates": rates, "usage_types": types}


# "To the cent": each billed line is rounded, so a row that is exactly right
# can still differ from the summed bill by a fraction of a cent.
CENT = 0.01


def check_rates(model: str | None, derived: dict) -> dict:
    """Billed quantity per tier priced at the model's row, against the bill —
    the same check that verified the Opus 5.5 row. ``matches`` is ``None`` when
    there is no row or an unrecognized usage type carried dollars."""
    rates = sm.RATES_BY_MODEL.get(model) if model else None
    unknown = sum(t["dollars"] for t in derived["usage_types"] if t["tier"] is None)
    if rates is None:
        return {"model": model, "priced": None, "billed": None, "matches": None}
    priced = billed = 0.0
    for t in derived["usage_types"]:
        if t["tier"] is None:
            continue
        priced += t["quantity"] * getattr(rates, t["tier"])
        billed += t["dollars"]
    return {
        "model": model,
        "priced": priced,
        "billed": billed,
        "matches": None if unknown else abs(priced - billed) < CENT,
    }


def fetch_estimate(days: list[str]) -> dict[str, float]:
    """The spend-cap alarms' estimate per UTC day, at the deployed rates."""
    stacks = aws(["cloudformation", "describe-stacks", "--stack-name", SPEND_CAP_STACK])
    try:
        params = {
            p["ParameterKey"]: float(p["ParameterValue"])
            for p in stacks["Stacks"][0]["Parameters"]
            if p["ParameterKey"] in SPEND_CAP_METRICS.values()
        }
    except SHAPE_ERRORS as exc:
        raise _shape_error(SPEND_CAP_STACK, exc) from exc
    missing = sorted(set(SPEND_CAP_METRICS.values()) - set(params))
    if missing:
        raise AwsError(f"{SPEND_CAP_STACK} carries no {', '.join(missing)}")
    queries = [
        {
            "Id": f"m{i}",
            "MetricStat": {
                "Metric": {"Namespace": "AWS/Bedrock", "MetricName": name},
                "Period": 86400,
                "Stat": "Sum",
            },
        }
        for i, name in enumerate(SPEND_CAP_METRICS)
    ]
    response = aws(
        [
            "cloudwatch",
            "get-metric-data",
            "--start-time",
            f"{min(days)}T00:00:00Z",
            "--end-time",
            f"{next_day(max(days))}T00:00:00Z",
            "--metric-data-queries",
            json.dumps(queries),
        ]
    )
    try:
        return estimate_from(response, params, days)
    except SHAPE_ERRORS as exc:
        raise _shape_error("CloudWatch", exc) from exc


def estimate_from(
    response: dict, params: dict[str, float], days: list[str]
) -> dict[str, float]:
    rates = list(SPEND_CAP_METRICS.values())
    out = {day: 0.0 for day in days}
    for result in response.get("MetricDataResults", []):
        rate = params[rates[int(result["Id"][1:])]]
        for stamp, value in zip(result.get("Timestamps", []), result.get("Values", [])):
            # The CLI renders these in the machine's LOCAL offset
            # (`2026-10-12T17:00:00-07:00` for the 10-13 UTC bucket), so the
            # date prefix is the wrong day west of Greenwich. Normalize first.
            day = (
                datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00"))
                .astimezone(datetime.timezone.utc)
                .date()
                .isoformat()
            )
            if day in out:
                out[day] += value * rate / 1_000_000
    return out


# --------------------------------------------------------------------------- #
# Comparison and rendering.
# --------------------------------------------------------------------------- #


def drift(metered: float, billed: float) -> float | None:
    return None if billed <= 0 else (metered - billed) / billed


def reconcile_day(summary: dict, billed: dict[str, float], tolerance: float) -> dict:
    """Metered against billed for one day. Services the transcripts priced are
    compared; a service whose model carried tokens but has no rate is
    unpriced; any other billed service is background.
    """
    metered_services = summary["bedrock_by_service"]
    unpriced_services = {service_for_model(m) for m in summary["unpriced"]}
    matched = unpriced = background = 0.0
    for service, dollars in billed.items():
        if service in metered_services:
            matched += dollars
        elif service in unpriced_services:
            unpriced += dollars
        else:
            background += dollars
    metered = sum(metered_services.values())
    expected = metered / (1 - CLASSIFIER_OVERHEAD)
    d = drift(expected, matched)
    gross = matched + unpriced + background
    judged = max(gross, metered) >= MIN_JUDGED_DOLLARS
    flagged = judged and (
        (d is not None and abs(d) > tolerance)
        or (matched == 0 and metered > 0)
        or (gross > 0 and background / gross > tolerance)
    )
    return {
        "metered": metered,
        "expected": expected,
        "billed_matched": matched,
        "billed_unpriced": unpriced,
        "billed_background": background,
        "raw_drift": drift(metered, matched),
        "drift": d,
        "flagged": flagged,
    }


def spend_cap_day(estimate: float, gross: float, tolerance: float) -> dict:
    """The cap's estimate against the billed gross for one day. Billed past the
    estimate by more than ``tolerance`` means the cap leaks; the estimate past
    billed by more than it means the cap trips early — including an estimate
    against a zero bill, which `reconcile_day` flags the same way.
    """
    d = drift(estimate, gross)
    judged = max(estimate, gross) >= MIN_JUDGED_DOLLARS
    leaking = judged and gross > estimate * (1 + tolerance)
    early = judged and (gross == 0 or (d is not None and d > tolerance))
    verdict = "cap LEAKING" if leaking else "trips EARLY" if early else "ok"
    return {
        "estimate": estimate,
        "billed": gross,
        "drift": d,
        "verdict": verdict,
        "flagged": leaking or early,
    }


def money(dollars: float) -> str:
    return f"${dollars:,.2f}"


def pct(fraction: float | None) -> str:
    return "n/a" if fraction is None else f"{fraction * 100:+.1f}%"


def render_meter(summary: dict, label: str) -> list[str]:
    subs = summary["substrates"]
    lines = [f"{label} (UTC)"]
    for name in (BEDROCK, SEAT, UNATTRIBUTED):
        entry = subs.get(name)
        if not entry:
            continue
        if name == BEDROCK:
            figure = money(entry["dollars"])
        elif name == SEAT:
            figure = f"{money(entry['dollars'])} {sm.NOT_BILLED}"
        else:
            figure = f"{money(entry['dollars'])} at Bedrock rates, NOT in the total"
        lines.append(f"  {name:<13} {entry['sessions']:>3} session(s)  {figure}")
    if summary["unpriced"]:
        lines.append("  unpriced: " + ", ".join(sorted(set(summary["unpriced"]))))
    return lines


def flag_mark(flagged: bool, provisional: bool) -> str:
    if provisional:
        return "  (provisional, not counted)"
    return "  FLAGGED" if flagged else ""


def resolve_days(given: list[str] | None, today: datetime.date) -> list[str]:
    if not given:
        return [(today - datetime.timedelta(days=n)).isoformat() for n in (2, 1)]
    # Canonicalized, because `fromisoformat` also accepts the basic `20261013`
    # form on newer Pythons, which would then match no transcript timestamp.
    dates = sorted({datetime.date.fromisoformat(day) for day in given})
    for date in dates:
        if date >= today:
            raise ValueError(f"{date} is not a complete UTC day yet")
    return [date.isoformat() for date in dates]


def render_rates(service: str, derived: dict, check: dict) -> list[str]:
    def rate(tier: str) -> str:
        value = derived["rates"].get(tier)
        return "-" if value is None else f"{value:.4f}"

    lines = [
        f"{service.removesuffix(EDITION_SUFFIX)}  per 1M: input {rate('input')}  "
        f"output {rate('output')}  cache read {rate('cache_read')}  "
        f"cache write {rate('cache_write_1h')}"
    ]
    for t in derived["usage_types"]:
        lines.append(
            f"  {t['usage_type']:<48} {t['tier'] or 'UNRECOGNIZED':<14} "
            f"{t['quantity']:>12,.4f}M  {money(t['dollars'])}"
        )
    if check["matches"] is not None:
        verdict = "reproduces the bill" if check["matches"] else "DOES NOT match"
        lines.append(
            f"  row {check['model']}: {money(check['priced'])} priced vs "
            f"{money(check['billed'])} billed, {verdict}"
        )
    elif check["model"] is None:
        lines.append("  no RATES_BY_MODEL row bills under this service")
    else:
        lines.append(f"  row {check['model']}: not judged, an unrecognized type billed")
    return lines


def rates_main(day: str, as_json: bool) -> int:
    """Per-model rates the bill implies for ``day``. Exit 1 when a model's row
    fails to reproduce its bill or cannot be judged because an unrecognized
    usage type billed, 2 on an AWS error, else 0."""
    try:
        usage = fetch_usage(day)
    except AwsError as exc:
        print(f"fleet_cost: {exc}", file=sys.stderr)
        return 2
    report: dict = {"day": day, "services": {}}
    lines = [f"{day} (UTC) rates billed, by Bedrock-edition service"]
    mismatched = False
    for service in sorted(usage):
        derived = derive_rates(usage[service])
        check = check_rates(model_for_service(service), derived)
        cannot_judge = check["model"] is not None and check["matches"] is None
        mismatched |= check["matches"] is False or cannot_judge
        report["services"][service] = {**derived, "check": check}
        lines += render_rates(service, derived, check)
    if not usage:
        lines.append("  no Bedrock-edition usage billed")
    print(json.dumps(report, indent=2) if as_json else "\n".join(lines))
    return 1 if mismatched else 0


# --------------------------------------------------------------------------- #
# Spend by local day.
# --------------------------------------------------------------------------- #

LOCAL_DAYS = 7
HOURLY_OFF = "hourly granularity off"
HOUR = datetime.timedelta(hours=1)


def local_midnight(date: datetime.date, tz=None) -> datetime.datetime:
    """The aware instant ``date`` starts in ``tz`` (the machine zone when
    None) — computed per day, so a week that crosses a DST change keeps each
    day's own offset."""
    naive = datetime.datetime.combine(date, datetime.time())
    return naive.replace(tzinfo=tz) if tz is not None else naive.astimezone()


def local_windows(
    now: datetime.datetime, tz=None
) -> list[tuple[str, datetime.datetime, datetime.datetime]]:
    """``(day, start, end)`` for the last ``LOCAL_DAYS`` complete local days,
    oldest first, then today from midnight to ``now``."""
    today = now.astimezone(tz).date()
    out = []
    for back in range(LOCAL_DAYS, 0, -1):
        day = today - datetime.timedelta(days=back)
        nxt = day + datetime.timedelta(days=1)
        out.append((day.isoformat(), local_midnight(day, tz), local_midnight(nxt, tz)))
    out.append((today.isoformat(), local_midnight(today, tz), now))
    return out


def local_day_of(tz=None) -> Callable[[str], str]:
    """A transcript timestamp (UTC, ``…Z``) -> its local date. Cached by the
    minute, the finest grain any zone offset has."""
    cache: dict[str, str] = {}

    def day_of(stamp: str) -> str:
        key = stamp[:16]
        if key not in cache:
            try:
                at = datetime.datetime.fromisoformat(f"{key}:00+00:00")
            except ValueError:
                cache[key] = ""
            else:
                cache[key] = at.astimezone(tz).date().isoformat()
        return cache[key]

    return day_of


def _utc(text: str) -> datetime.datetime:
    return datetime.datetime.fromisoformat(text.replace("Z", "+00:00"))


def _ce_hour(at: datetime.datetime) -> str:
    """``at`` on Cost Explorer's hourly grid, rounded down."""
    utc = at.astimezone(datetime.timezone.utc)
    return utc.replace(minute=0, second=0, microsecond=0).strftime("%Y-%m-%dT%H:%M:%SZ")


def parse_hourly(response: dict) -> list[tuple[datetime.datetime, float]]:
    """An hourly Cost Explorer page -> ``(hour start, Bedrock-edition
    dollars)`` per period."""
    out = []
    for period in response.get("ResultsByTime", []):
        dollars = sum(
            float(group["Metrics"]["UnblendedCost"]["Amount"])
            for group in period.get("Groups", [])
            if group["Keys"][0].endswith(EDITION_SUFFIX)
        )
        out.append((_utc(period["TimePeriod"]["Start"]), dollars))
    return out


def fetch_hourly(
    start: datetime.datetime, end: datetime.datetime
) -> list[tuple[datetime.datetime, float]]:
    """Every complete hour from ``start`` to ``end``, paged — ``end`` floored,
    so the read never asks for an hour still running, which has no data yet
    anyway. Needs the payer account's hourly-granularity preference; without
    it the read is refused."""
    args = [
        "ce",
        "get-cost-and-usage",
        "--time-period",
        f"Start={_ce_hour(start)},End={_ce_hour(end)}",
        "--granularity",
        "HOURLY",
        "--metrics",
        "UnblendedCost",
        "--filter",
        json.dumps({"Dimensions": {"Key": "RECORD_TYPE", "Values": ["Usage"]}}),
        "--group-by",
        "Type=DIMENSION,Key=SERVICE",
    ]
    hours: list[tuple[datetime.datetime, float]] = []
    token = None
    while True:
        response = aws(args + (["--next-page-token", token] if token else []))
        try:
            hours += parse_hourly(response)
            token = response.get("NextPageToken")
        except SHAPE_ERRORS as exc:
            raise _shape_error("hourly Cost Explorer", exc) from exc
        if not token:
            return hours


def hourly_off(exc: AwsError) -> bool:
    """The refusal Cost Explorer gives while the hourly preference is off:
    ``Hourly data granularity is an opt-in only feature`` (measured
    2026-10-29)."""
    text = str(exc).lower()
    return "hourly" in text and "opt-in" in text


def bucket_hours(
    hours: list[tuple[datetime.datetime, float]], tz=None
) -> tuple[dict[str, float], datetime.datetime | None]:
    """``({local day: dollars}, end of the last hour that billed)``. Each hour
    lands on the local day it starts in; in a zone whose offset is not whole
    hours, an hour straddling midnight goes to the day it starts in."""
    days: dict[str, float] = {}
    through = None
    for start, dollars in hours:
        day = start.astimezone(tz).date().isoformat()
        days[day] = days.get(day, 0.0) + dollars
        if dollars > 0 and (through is None or start + HOUR > through):
            through = start + HOUR
    return days, through


def local_report(meter: Meter, windows, billed, through, billed_state: str, tz=None):
    """The ``local`` subcommand's structured result."""
    today = windows[-1][0]
    rows = []
    for day, _, _ in windows:
        subs = day_summary(meter, day)["substrates"]
        bedrock, seat = subs.get(BEDROCK) or {}, subs.get(SEAT) or {}
        rows.append(
            {
                "day": day,
                "label": "today so far"
                if day == today
                else "yesterday"
                if day == windows[-2][0]
                else datetime.date.fromisoformat(day).strftime("%a"),
                "billed": None if billed is None else billed.get(day, 0.0),
                "metered": bedrock.get("dollars", 0.0),
                "sessions": bedrock.get("sessions", 0),
                "seat": seat.get("dollars", 0.0),
                "seat_sessions": seat.get("sessions", 0),
            }
        )
    week = rows[:-1]
    return {
        "zone": windows[-1][2].astimezone(tz).tzname(),
        "billed": billed_state,
        "billed_through": None
        if through is None
        else through.astimezone(tz).strftime("%Y-%m-%d %H:%M"),
        "days": rows,
        "week": {
            "billed": None if billed is None else sum(r["billed"] for r in week),
            "metered": sum(r["metered"] for r in week),
            "seat": sum(r["seat"] for r in week),
        },
        "unreadable_transcripts": meter.unreadable,
    }


def local_heading(report: dict) -> str:
    """The legend the rows lean on. Shared with upkeep."""
    state = report["billed"]
    if state == "ok":
        through = report["billed_through"]
        state = f"billed through {through}" if through else "nothing billed yet"
    return (
        f"by local day ({report['zone']}), {state}; metered and seat at Bedrock "
        "rates (sessions), the seat not billed"
    )


def local_row(row: dict, state: str) -> str:
    """One day: billed beside metered, the seat apart. Shared with upkeep."""
    billed = money(row["billed"]) if row["billed"] is not None else state
    return (
        f"{row['day']} {row['label']}: billed {billed} | metered "
        f"{money(row['metered'])} ({row['sessions']}) | seat "
        f"{money(row['seat'])} ({row['seat_sessions']})"
    )


def local_week(report: dict) -> str:
    week = report["week"]
    billed = money(week["billed"]) if week["billed"] is not None else report["billed"]
    return (
        f"last {LOCAL_DAYS} days: billed {billed} | metered "
        f"{money(week['metered'])} | seat {money(week['seat'])}"
    )


def local_main(as_json: bool, tz=None, now: datetime.datetime | None = None) -> int:
    """Spend per local day. A failed billed read is a column state, not an
    exit: the metered side still prints."""
    now = now or datetime.datetime.now(datetime.timezone.utc)
    windows = local_windows(now, tz)
    meter = walk(
        sm.claude_home() / "projects",
        [day for day, _, _ in windows],
        day_of=local_day_of(tz),
        floor=windows[0][1].timestamp(),
    )
    billed, through, state = None, None, "ok"
    try:
        billed, through = bucket_hours(fetch_hourly(windows[0][1], now), tz)
    except AwsError as exc:
        state = HOURLY_OFF if hourly_off(exc) else f"billed read failed: {exc}"
    report = local_report(meter, windows, billed, through, state, tz)
    if as_json:
        print(json.dumps(report, indent=2))
        return 0
    lines = [local_heading(report)]
    lines += [f"  {local_row(row, report['billed'])}" for row in report["days"]]
    lines.append(f"  {local_week(report)}")
    if meter.unreadable:
        lines.append(f"skipped {meter.unreadable} unreadable transcript(s)")
    print("\n".join(lines))
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("meter", "reconcile", "spend-cap"):
        cmd = sub.add_parser(name)
        cmd.add_argument(
            "--day",
            action="append",
            help="a complete UTC day, repeatable (default: D-2 and D-1)",
        )
        cmd.add_argument("--tolerance", type=float, default=DEFAULT_TOLERANCE)
        cmd.add_argument("--top", type=int, default=5, help="issues to list per run")
        cmd.add_argument("--json", action="store_true", help="emit the full breakdown")
    cmd = sub.add_parser("rates")
    cmd.add_argument("day", help="a complete UTC day")
    cmd.add_argument("--json", action="store_true", help="emit the full breakdown")
    cmd = sub.add_parser("local")
    cmd.add_argument("--json", action="store_true", help="emit the full breakdown")
    args = parser.parse_args(argv)
    if args.command == "local":
        return local_main(args.json)

    today = datetime.datetime.now(datetime.timezone.utc).date()
    try:
        days = resolve_days([args.day] if args.command == "rates" else args.day, today)
    except ValueError as exc:
        print(f"fleet_cost: {exc}", file=sys.stderr)
        return 2
    if args.command == "rates":
        return rates_main(days[0], args.json)
    provisional = (today - datetime.timedelta(days=1)).isoformat()

    def label(day: str) -> str:
        return f"{day}{' provisional' if day == provisional else ''}"

    report: dict = {"days": {}, "tolerance": args.tolerance}
    lines: list[str] = []
    flagged = False
    try:
        if args.command in ("meter", "reconcile"):
            meter = walk(sm.claude_home() / "projects", days)
            billed = fetch_billed(days) if args.command == "reconcile" else {}
            for day in days:
                summary = day_summary(meter, day)
                lines += render_meter(summary, label(day))
                if args.command == "reconcile":
                    rec = reconcile_day(summary, billed[day], args.tolerance)
                    summary["reconcile"] = rec
                    flagged |= rec["flagged"] and day != provisional
                    unpriced = rec["billed_unpriced"]
                    lines.append(
                        "  bill          {} matched, {}{} background; drift {} "
                        "after classifier, {} raw{}".format(
                            money(rec["billed_matched"]),
                            f"{money(unpriced)} unpriced, " if unpriced else "",
                            money(rec["billed_background"]),
                            pct(rec["drift"]),
                            pct(rec["raw_drift"]),
                            flag_mark(rec["flagged"], day == provisional),
                        )
                    )
                if "friction" in summary:
                    lines.append(
                        f"  friction      {render_friction(summary['friction'])}"
                    )
                summary["provisional"] = day == provisional
                report["days"][day] = summary
            issues = issue_costs(meter, days)
            report["issues"] = issues
            report["unreadable_transcripts"] = meter.unreadable
            if issues:
                top = ", ".join(f"{i} {money(c)}" for i, c in issues[: args.top])
                lines.append(f"costliest issues: {top}")
            seat_issues = issue_costs(meter, days, SEAT)
            report["seat_issue_costs"] = seat_issues
            if seat_issues:
                top = ", ".join(f"{i} {money(c)}" for i, c in seat_issues[: args.top])
                lines.append(f"seat issues {sm.NOT_BILLED}: {top}")
            for substrate in (SEAT, UNATTRIBUTED):
                counts = issue_session_counts(meter, days, substrate)
                report[f"{substrate}_issues"] = counts
                if counts:
                    top = ", ".join(f"{i} ({n})" for i, n in counts[: args.top])
                    lines.append(f"{substrate} sessions by issue: {top}")
            if meter.unreadable:
                lines.append(f"skipped {meter.unreadable} unreadable transcript(s)")
        else:
            estimate = fetch_estimate(days)
            billed = fetch_billed(days)
            for day in days:
                cap = spend_cap_day(
                    estimate[day], sum(billed[day].values()), args.tolerance
                )
                flagged |= cap["flagged"] and day != provisional
                lines.append(
                    f"{label(day)} (UTC)  estimate {money(cap['estimate'])}  "
                    f"billed {money(cap['billed'])}  {pct(cap['drift'])}  "
                    f"{cap['verdict']}{flag_mark(False, day == provisional)}"
                )
                report["days"][day] = {**cap, "provisional": day == provisional}
    except AwsError as exc:
        print(f"fleet_cost: {exc}", file=sys.stderr)
        return 2

    print(json.dumps(report, indent=2, default=list) if args.json else "\n".join(lines))
    return 1 if flagged else 0


if __name__ == "__main__":
    sys.exit(main())
