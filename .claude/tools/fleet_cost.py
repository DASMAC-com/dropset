#!/usr/bin/env python3
"""Price the fleet's Bedrock spend from transcripts and reconcile it with the bill.

The Bedrock side of the fleet cost picture (``subscription_usage.py`` is the
seat side). Three subcommands, each over **complete UTC days** — AWS reports
in UTC, so every day boundary here is a UTC one, on both sides:

``meter``
    Walk every transcript under the Claude projects directory, sub-agents
    included, count each message's usage **once per message id** (a streamed
    message is written one line per content block, each repeating the same
    usage — pricing lines instead of messages inflated cost 2.2 to 2.6x when it
    was measured), bucket it by the UTC day of the record and the model it
    recorded, and price it at ``session_metrics.RATES_BY_MODEL``. Sessions are
    split by substrate, which comes from the launch marker and **never** from
    the transcript's model id (a seat and a Bedrock session record the same
    name). Every session is also keyed to its Linear issue the one way — the
    worktree tag, or for a base-checkout ``chore`` its bootstrap prompt —
    whatever its substrate: Bedrock issues are priced, seat and unattributed
    ones are listed by session count beside them.

``reconcile``
    ``meter`` beside Cost Explorer's billed gross for the same days, per
    Marketplace service, with the drift and a flag past ``--tolerance``.

``spend-cap``
    The daily spend cap's CloudWatch metric-math **estimate** beside the same
    billed gross: the four Bedrock token metrics summed over the UTC day and
    priced at the deployed stack's ``TokenRate*`` parameters, exactly as the
    alarms do. Billed above the estimate means the cap is leaking; the estimate
    far above billed means it trips early.

**Substrate classes.** ``bedrock`` and ``seat`` come from a marker. A worktree
session with **no** marker (or an unrecognized one) is ``unattributed`` and is
never priced into the Bedrock total — that is the share the bill comparison
cannot account for, reported rather than guessed. ``explore`` landed here
until it wrote a marker; an older explore worktree still does. A base-repo
session (``plan``, ``housekeeping``, ``chore``) reads as ``seat``.

Two blind spots, both visible only as bill-side drift: ``plan bedrock`` is a
base-repo launch that writes no marker; and a marker is one file per worktree
tag, overwritten by each launch, so relaunching a worktree on the other
substrate reclassifies that worktree's EARLIER sessions too.

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
settled day flagged; 2 a usage error, or an AWS call that failed or returned a
shape this tool does not recognize.

Usage::

    python3 .claude/tools/fleet_cost.py meter [--day YYYY-MM-DD]... [--top N]
    python3 .claude/tools/fleet_cost.py reconcile [--day YYYY-MM-DD]...
    python3 .claude/tools/fleet_cost.py spend-cap [--day YYYY-MM-DD]...

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

import session_metrics as sm

BEDROCK = "bedrock"
SEAT = "seat"
UNATTRIBUTED = "unattributed"

EDITION_SUFFIX = " (Amazon Bedrock Edition)"
SPEND_CAP_STACK = "dropset-bedrock-agent"
# The four metrics the spend-cap alarms sum, keyed by the stack parameter that
# prices each. Dimensionless, so every model counts, as in the alarms.
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


def classify(cwd: str | None) -> str:
    """A session's substrate class, from its launch marker.

    Unlike ``session_metrics.resolve_substrate``, which degrades an unknown to
    seat because a single-session report must never invent a dollar figure, a
    fleet total needs the third answer: a worktree session with no marker is
    neither priced nor silently waved through as seat.
    """
    recorded = sm.read_substrate_marker(cwd)
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

    def ingest(
        self, line: str, substrate: str, issue: str | None, session: str
    ) -> None:
        if not line.strip():
            return
        try:
            rec = json.loads(line)
        except ValueError:
            self.parse_errors += 1
            return
        if not isinstance(rec, dict):
            return
        msg = rec.get("message")
        if not isinstance(msg, dict) or not isinstance(msg.get("usage"), dict):
            return
        day = str(rec.get("timestamp", ""))[:10]
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
    """``(cwd, issue from the bootstrap prompt)`` — the prompt names the issue
    a base-checkout launch ran for, where no worktree tag does. The first few
    user records are tried, not only the first: a caveat or hook record can
    precede the command."""
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
            issue = issue_of_prompt(_prompt_text(rec))
            users += 1
            if issue or users >= HEAD_USER_RECORDS:
                return cwd, issue
    return cwd, None


# How many leading user records may hold the bootstrap prompt.
HEAD_USER_RECORDS = 3


def walk(projects: Path, days: list[str]) -> Meter:
    """Meter every transcript that could hold a record on ``days``.

    A transcript last written before the first day cannot hold one, so it is
    skipped unread; a sub-agent transcript always finishes before its parent's
    last write, so the parent's mtime gates both.
    """
    meter = Meter(days=set(days))
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
        for transcript in sorted(project.glob("*.jsonl")):
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
            substrate, issue = classify(cwd), issue_of(cwd) or prompted
            session = transcript.stem
            files = [
                transcript,
                *sorted((project / session / "subagents").glob("*.jsonl")),
            ]
            for path in files:
                try:
                    for line in sm._read_lines(path):
                        meter.ingest(line, substrate, issue, session)
                except OSError:
                    meter.unreadable += 1
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
    return out


def issue_costs(meter: Meter, days: list[str]) -> list[tuple[str, float]]:
    """Bedrock dollars per issue over ``days``, costliest first."""
    totals: dict[str, float] = {}
    for (day, substrate, issue, model), tokens in meter.by_issue.items():
        if day in days and substrate == BEDROCK:
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
            figure = "subscription, not billed here"
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
    args = parser.parse_args(argv)

    today = datetime.datetime.now(datetime.timezone.utc).date()
    try:
        days = resolve_days(args.day, today)
    except ValueError as exc:
        print(f"fleet_cost: {exc}", file=sys.stderr)
        return 2
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
                summary["provisional"] = day == provisional
                report["days"][day] = summary
            issues = issue_costs(meter, days)
            report["issues"] = issues
            report["unreadable_transcripts"] = meter.unreadable
            if issues:
                top = ", ".join(f"{i} {money(c)}" for i, c in issues[: args.top])
                lines.append(f"costliest issues: {top}")
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
