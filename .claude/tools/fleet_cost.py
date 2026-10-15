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
    name). Bedrock spend is also attributed per Linear issue, from the worktree
    tag.

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
cannot account for, reported rather than guessed. A base-repo session is a seat
verb (``plan``, ``housekeeping``, ``explore``) and reads as ``seat``; the one
blind spot is ``plan bedrock``, which writes no marker and so surfaces only as
bill-side drift.

**The bill is matched per service, not by a fixed list of names.** Cost
Explorer files each model as its own Marketplace service, ``Claude <Family>
<version> (Amazon Bedrock Edition)``, under record type ``Usage``; the bare
``Amazon Bedrock`` service dimension reads zero every day and is a convincing
false negative. Each new model adds a service, so the read takes every service
carrying the edition suffix and maps transcript models onto them by name. A
billed service with no transcript tokens that day is **background** traffic
(harness calls on another model, which no transcript records) and is reported
as its own line. The auto-mode permission classifier is the exception: it runs
on the session's own model, so it bills inside the matched service, and is
allowed for by the measured :data:`CLASSIFIER_OVERHEAD` before drift is judged.

**D-1 is provisional.** Cost Explorer lands charges hours to a day late, so the
default window is D-2 (settled) and D-1 (marked provisional). Today's UTC date
is refused.

Stdout is a ranked summary (about ten lines a day); ``--json`` emits the full
breakdown. Exit codes: 0 reported with no drift flag; 1 at least one day
flagged; 2 a usage error or an AWS call that failed.

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
# The share of a day's matched bill no transcript records: the auto-mode
# permission classifier, which since the Opus 5.5 swap runs on the session's
# own model and so bills inside the same service rather than as a background
# line of its own. Measured 2026-10-08, 10-09 and 10-13 at 8.3, 8.1 and 8.3
# percent, with the gap almost entirely fresh input and output (a stateless call
# per tool use) while both cache tiers matched within about 2 percent. The
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

    Tags come in two spellings on disk — bare ``1651`` (``task <n>`` since mid
    October) and ``eng-1073`` (before) — and both name ``ENG-<n>``. A tag that
    is not an issue number is returned as-is, so it still groups.
    """
    tag = sm.tag_from_cwd(cwd)
    if not tag:
        return None
    match = re.fullmatch(r"(?:eng-)?(\d+)", tag, flags=re.IGNORECASE)
    return f"ENG-{match.group(1)}" if match else tag


def service_for_model(model: str) -> str | None:
    """The Cost Explorer service a transcript model id bills under.

    ``claude-opus-5-5`` -> ``Claude Opus 5.5 (Amazon Bedrock Edition)``. A
    dated suffix (``-20251001``) is dropped, as is the Bedrock spelling a
    sub-agent can record (``us.anthropic.claude-haiku-4-5-20251001-v1:0``).
    ``None`` for an id that is not a Claude model (``<synthetic>``, the
    unrecorded sentinel).
    """
    match = re.fullmatch(
        r"(?:[a-z]{2}\.)?(?:anthropic\.)?claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?"
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
    plus the Bedrock share per issue.
    """

    days: set[str]
    by_day: dict[tuple[str, str, str], sm.Tokens] = field(default_factory=dict)
    by_issue: dict[tuple[str, str, str], sm.Tokens] = field(default_factory=dict)
    sessions: dict[tuple[str, str], set[str]] = field(default_factory=dict)
    seen: set[str] = field(default_factory=set)
    parse_errors: int = 0

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
        if substrate == BEDROCK:
            key = (day, issue or "(no issue)", model)
            self.by_issue.setdefault(key, sm.Tokens()).add(msg["usage"])
        self.sessions.setdefault((day, substrate), set()).add(session)


def _read_cwd(path: Path) -> str | None:
    for line in sm._read_lines(path):
        try:
            rec = json.loads(line)
        except ValueError:
            continue
        cwd = rec.get("cwd") if isinstance(rec, dict) else None
        if isinstance(cwd, str) and cwd.strip():
            return cwd.strip()
    return None


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
            cwd = _read_cwd(transcript)
            substrate, issue = classify(cwd), issue_of(cwd)
            session = transcript.stem
            files = [
                transcript,
                *sorted((project / session / "subagents").glob("*.jsonl")),
            ]
            for path in files:
                for line in sm._read_lines(path):
                    meter.ingest(line, substrate, issue, session)
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
    for (day, issue, model), tokens in meter.by_issue.items():
        if day in days:
            totals[issue] = totals.get(issue, 0.0) + (price(tokens, model) or 0.0)
    return sorted(totals.items(), key=lambda kv: (-kv[1], kv[0]))


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
    return json.loads(proc.stdout or "{}")


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
    billed = parse_billed(response)
    return {day: billed.get(day, {}) for day in days}


def fetch_estimate(days: list[str]) -> dict[str, float]:
    """The spend-cap alarms' estimate per UTC day, at the deployed rates."""
    stacks = aws(["cloudformation", "describe-stacks", "--stack-name", SPEND_CAP_STACK])
    params = {
        p["ParameterKey"]: float(p["ParameterValue"])
        for p in stacks["Stacks"][0]["Parameters"]
        if p["ParameterKey"] in SPEND_CAP_METRICS.values()
    }
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
    return estimate_from(response, params, days)


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
    compared; billed services with no metered share are background.
    """
    metered_services = summary["bedrock_by_service"]
    matched = sum(v for k, v in billed.items() if k in metered_services)
    background = sum(v for k, v in billed.items() if k not in metered_services)
    metered = sum(metered_services.values())
    expected = metered / (1 - CLASSIFIER_OVERHEAD)
    d = drift(expected, matched)
    flagged = (d is not None and abs(d) > tolerance) or (matched == 0 and metered > 0)
    return {
        "metered": metered,
        "expected": expected,
        "billed_matched": matched,
        "billed_background": background,
        "raw_drift": drift(metered, matched),
        "drift": d,
        "flagged": flagged,
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


def resolve_days(given: list[str] | None, today: datetime.date) -> list[str]:
    if not given:
        return [(today - datetime.timedelta(days=n)).isoformat() for n in (2, 1)]
    days = sorted(set(given))
    for day in days:
        if datetime.date.fromisoformat(day) >= today:
            raise ValueError(f"{day} is not a complete UTC day yet")
    return days


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
                    flagged |= rec["flagged"]
                    lines.append(
                        "  bill          {} matched, {} background; drift {} "
                        "after classifier, {} raw{}".format(
                            money(rec["billed_matched"]),
                            money(rec["billed_background"]),
                            pct(rec["drift"]),
                            pct(rec["raw_drift"]),
                            "  FLAGGED" if rec["flagged"] else "",
                        )
                    )
                report["days"][day] = summary
            issues = issue_costs(meter, days)
            report["issues"] = issues
            if issues:
                top = ", ".join(f"{i} {money(c)}" for i, c in issues[: args.top])
                lines.append(f"costliest issues: {top}")
        else:
            estimate = fetch_estimate(days)
            billed = fetch_billed(days)
            for day in days:
                gross = sum(billed[day].values())
                d = drift(estimate[day], gross)
                leaking = gross > estimate[day] * (1 + args.tolerance)
                early = d is not None and d > args.tolerance
                flagged |= leaking or early
                verdict = "cap LEAKING" if leaking else "trips EARLY" if early else "ok"
                lines.append(
                    f"{label(day)} (UTC)  estimate {money(estimate[day])}  "
                    f"billed {money(gross)}  {pct(d)}  {verdict}"
                )
                report["days"][day] = {
                    "estimate": estimate[day],
                    "billed": gross,
                    "drift": d,
                    "verdict": verdict,
                }
    except AwsError as exc:
        print(f"fleet_cost: {exc}", file=sys.stderr)
        return 2

    print(json.dumps(report, indent=2, default=list) if args.json else "\n".join(lines))
    return 1 if flagged else 0


if __name__ == "__main__":
    sys.exit(main())
