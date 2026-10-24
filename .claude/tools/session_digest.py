#!/usr/bin/env python3
"""``session_metrics.py digest`` — the daily telemetry check-in.

One deterministic digest over every transcript the fleet produced since the
last one, every verb and substrate, written by the upkeep pass in its own
process; no model reads a transcript to make it. It enumerates every
transcript under the base checkout's and its worktrees' project slugs modified
since ``--since`` (upkeep passes its previous run stamp), runs the per-session
aggregation of ``session_metrics.py`` on each, and writes one untracked
Markdown file, ``<base>/.claude/session-metrics/digest-<YYYY-MM-DD>.md``, with
a JSON twin beside it that the allowlist-refresh mining reads.

**The day header leads with rate calibration** — how the rate table matched
the bill on the last settled day, from the reconcile and ``rates`` reads upkeep
already makes and hands in as ``--calibration``, each rate row beside the date
it was last verified, a row past :data:`RATE_TOLERANCE` flagged. Then the
window's figures — sessions, dollars per substrate and per model, the
sub-agent share, the cost of re-warming caches across days — each beside its
rolling :data:`MEDIAN_WINDOW`-digest median, so a day that ballooned is a
flagged line. The medians come from ``history.jsonl`` beside the digests,
which the purge's age rule does not touch.

**One block per session**: verb and issue (the worktree tag, or a base-repo
verb's bootstrap prompt, as ``fleet_cost.py`` attributes), substrate, the
window's per-UTC-day slices beside the cumulative figure, the exit-time
estimate beside it when the launcher left one, the per-model rollup with each
sub-agent's model on its own line, the top sinks and largest results,
resident-prose share, hardening candidates with their cost kind, the session's
own ``Session account`` block lifted verbatim (or "no account"), and a
``live`` flag on a session still running — it is digested again next day.

**A session spanning days** appears in each day's digest with that window's
slice: only records stamped at or after ``--since`` are sliced, so two digests
never count one message twice. The first request of each later UTC day
re-writes the whole cached prefix, and that cache-write cost is its own line.

**Seat sessions are priced at Bedrock rates**, labelled "at Bedrock rates, not
billed" (``session_metrics.NOT_BILLED``); the subscription is those rates
subsidized, so the figure says what the work would have cost.

Stdout is one JSON line — sessions, dollars, flagged lines, digest path — the
line upkeep's ``digest`` step reports. Stdlib only; a Python skill-tool under
``.claude/tools/``, deliberately not a Cargo workspace member. Tests live in
``tests/test_session_digest.py``.
"""

from __future__ import annotations

import argparse
import datetime
import json
import re
import statistics
import sys
from dataclasses import dataclass, field
from pathlib import Path

import fleet_cost
import session_metrics as sm

# Relative to the base checkout, git-ignored with the rest of `.claude/`
# machine state. The purge prunes `digest-*` files here on the transcripts' age
# rule; `history.jsonl` is not one, so the medians outlive the files.
DIGEST_DIR = Path(".claude") / "session-metrics"
HISTORY_NAME = "history.jsonl"
DIGEST_GLOB = "digest-*.json"
# Where the launcher epilogue leaves a session's exit-time estimate, one
# `<session-id>.json` holding `{"dollars": <float>}`.
ESTIMATE_DIR = DIGEST_DIR / "estimates"

# How many earlier digests a header figure's median spans, how many it needs
# before a figure can be judged, and how far above the median flags it.
MEDIAN_WINDOW = 7
MEDIAN_MIN_POINTS = 3
BALLOON_FACTOR = 2.0
HISTORY_KEEP = 30

# A rate row whose priced-against-billed drift on the settled day exceeds this
# is flagged. A verified row reproduces its bill to the cent, so one percent is
# already a row that needs re-seeding, not rounding.
RATE_TOLERANCE = 0.01

# A transcript written this recently is a session still running.
LIVE_SECONDS = 3600
DEFAULT_SINCE_HOURS = 24
# Rows per ranked list in a session block. The digest is read every morning,
# so a block stays a few lines; the full tables are `session_metrics.py`'s.
BLOCK_TOP_N = 3

# The slug segment `claude --worktree` checkouts add after the base's slug.
WORKTREE_SLUG = "--claude-worktrees-"

# The block the shared session tail writes: a heading line naming it, through
# the next heading or the end of that message's text.
ACCOUNT_HEADING_RE = re.compile(r"^\s*(?:#{1,6}\s*|\*\*)Session account\b")
NEXT_HEADING_RE = re.compile(r"^\s*#{1,6}\s")
ACCOUNT_MAX_LINES = 40

COMMAND_RE = re.compile(r"<command-name>/([\w:-]+)</command-name>|^/([\w:-]+)")
# A task session's first command is the bootstrap skill, not the verb's name.
VERB_OF_COMMAND = {"init-pr": "task"}


# --------------------------------------------------------------------------- #
# Discovery and the per-transcript scan.
# --------------------------------------------------------------------------- #


def discover(projects: Path, base: Path, since_ts: float) -> list[Path]:
    """Transcripts of the base checkout and its worktrees written since
    ``since_ts``, oldest first. Slugs are matched exactly or by the worktree
    segment, never by bare prefix, which would catch a sibling repo
    (``dropset-beta``)."""
    slug = sm.slugify(base)
    found: list[tuple[float, Path]] = []
    if not projects.is_dir():
        return []
    for project in projects.iterdir():
        name = project.name
        if not project.is_dir() or not (
            name == slug or name.startswith(slug + WORKTREE_SLUG)
        ):
            continue
        for transcript in project.glob("*.jsonl"):
            try:
                mtime = transcript.stat().st_mtime
            except OSError:
                continue
            if mtime >= since_ts:
                found.append((mtime, transcript))
    return [p for _, p in sorted(found)]


@dataclass
class Head:
    """What a transcript says about itself beyond its tokens."""

    cwd: str | None = None
    verb: str | None = None
    issue: str | None = None
    account: list[str] | None = None


def account_block(text: str) -> list[str] | None:
    """The ``Session account`` block in one message's text, verbatim."""
    lines = text.splitlines()
    for i, line in enumerate(lines):
        if not ACCOUNT_HEADING_RE.match(line):
            continue
        block = [line]
        for following in lines[i + 1 :]:
            if NEXT_HEADING_RE.match(following):
                break
            block.append(following)
        while block and not block[-1].strip():
            block.pop()
        return block[:ACCOUNT_MAX_LINES]
    return None


def _assistant_text(rec: dict) -> str:
    content = (rec.get("message") or {}).get("content")
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return ""
    return "\n".join(
        part["text"]
        for part in content
        if isinstance(part, dict)
        and part.get("type") == "text"
        and isinstance(part.get("text"), str)
    )


def scan_head(path: Path) -> Head:
    """The working directory, the launch verb and issue from the first user
    records, and the LAST session account the transcript carries."""
    head = Head()
    users = 0
    for line in sm._read_lines(path):
        try:
            rec = json.loads(line)
        except ValueError:
            continue
        if not isinstance(rec, dict):
            continue
        if head.cwd is None and isinstance(rec.get("cwd"), str) and rec["cwd"].strip():
            head.cwd = rec["cwd"].strip()
        kind = rec.get("type")
        if kind == "user" and users < fleet_cost.HEAD_USER_RECORDS:
            text = fleet_cost._prompt_text(rec)
            if text.strip():
                users += 1
            head.issue = head.issue or fleet_cost.issue_of_prompt(text)
            match = COMMAND_RE.search(text.strip())
            if head.verb is None and match:
                command = match.group(1) or match.group(2)
                head.verb = VERB_OF_COMMAND.get(command, command)
        elif kind == "assistant":
            text = _assistant_text(rec)
            if "Session account" in text:
                block = account_block(text)
                if block:
                    head.account = block
    return head


def is_live(path: Path, cwd: str | None, base: Path, now_ts: float) -> bool:
    """Written in the last hour, or its worktree is locked."""
    try:
        if now_ts - path.stat().st_mtime < LIVE_SECONDS:
            return True
    except OSError:
        return False
    tag = sm.tag_from_cwd(cwd)
    return bool(tag) and (base / ".git" / "worktrees" / tag / "locked").exists()


def read_estimate(base: Path, session_id: str) -> float | None:
    try:
        data = json.loads((base / ESTIMATE_DIR / f"{session_id}.json").read_text())
        return float(data["dollars"])
    except (OSError, ValueError, KeyError, TypeError):
        return None


# --------------------------------------------------------------------------- #
# Pricing and the per-session record.
# --------------------------------------------------------------------------- #


def price(by_model: dict[str, sm.Tokens]) -> float | None:
    """Dollars at each model's Bedrock rate, or ``None`` when one is unpriced."""
    try:
        return sm.Cost.price_by_model(by_model).total()
    except sm.UnpricedModel:
        return None


def _add(total: float | None, part: float | None) -> float | None:
    return None if total is None or part is None else total + part


@dataclass
class SessionDigest:
    session_id: str
    head: Head
    substrate: str
    live: bool
    report: dict
    estimate: float | None
    # day -> {"dollars", "subagents", "rewarm", "turns"}, the window only.
    days: dict[str, dict] = field(default_factory=dict)
    # model -> dollars over the window, main and sub-agents together.
    window_models: dict[str, float] = field(default_factory=dict)
    unpriced: set[str] = field(default_factory=set)

    @property
    def window_dollars(self) -> float | None:
        total: float | None = 0.0
        for day in self.days.values():
            total = _add(total, day["dollars"])
        return total

    @property
    def cumulative(self) -> float | None:
        cost = self.report["total_cost"]
        return None if cost is None else cost.total()


def digest_session(
    path: Path, base: Path, since: str, now_ts: float
) -> SessionDigest | None:
    """One session's digest record, or ``None`` when nothing it did falls in
    the window and it is not running."""
    session_id = path.stem
    head = scan_head(path)
    report = sm.aggregate(path, session_id, None, since)
    live = is_live(path, head.cwd, base, now_ts)
    if not report["days"] and not live:
        return None
    out = SessionDigest(
        session_id=session_id,
        head=head,
        substrate=report["substrate"],
        live=live,
        report=report,
        estimate=read_estimate(base, session_id),
    )
    for day, day_slice in report["days"].items():
        main, sub = price(day_slice.main), price(day_slice.subagents)
        out.days[day] = {
            "dollars": _add(main, sub),
            "subagents": sub,
            "rewarm": price(day_slice.rewarm),
            "turns": day_slice.turns,
        }
        for by_model in (day_slice.main, day_slice.subagents):
            for model, tokens in by_model.items():
                if not tokens.billable():
                    continue
                dollars = price({model: tokens})
                if dollars is None:
                    out.unpriced.add(model)
                else:
                    out.window_models[model] = (
                        out.window_models.get(model, 0.0) + dollars
                    )
    return out


# --------------------------------------------------------------------------- #
# The day header: figures, medians, calibration.
# --------------------------------------------------------------------------- #


def header_figures(sessions: list[SessionDigest]) -> dict:
    """The window's figures, the numbers the history records."""
    figures: dict = {"sessions": len(sessions), "bedrock": 0.0, "seat": 0.0}
    sub = rewarm = 0.0
    rewarm_sessions = 0
    models: dict[str, float] = {}
    for s in sessions:
        key = "bedrock" if s.substrate == sm.SUBSTRATE_BEDROCK else "seat"
        figures[key] += s.window_dollars or 0.0
        sub += sum(d["subagents"] or 0.0 for d in s.days.values())
        spent = sum(d["rewarm"] or 0.0 for d in s.days.values())
        if spent:
            rewarm += spent
            rewarm_sessions += 1
        for model, dollars in s.window_models.items():
            models[model] = models.get(model, 0.0) + dollars
    figures["total"] = figures["bedrock"] + figures["seat"]
    figures["subagent_share"] = sub / figures["total"] if figures["total"] else 0.0
    figures["rewarm"] = rewarm
    figures["rewarm_sessions"] = rewarm_sessions
    figures["models"] = dict(sorted(models.items(), key=lambda kv: -kv[1]))
    return figures


def read_history(directory: Path, date: str) -> list[dict]:
    """Earlier digests' figures, oldest first; a same-day rerun's own entry
    is left out so it never becomes its own baseline."""
    entries = []
    try:
        lines = (directory / HISTORY_NAME).read_text(encoding="utf-8").splitlines()
    except OSError:
        return []
    for line in lines:
        try:
            entry = json.loads(line)
        except ValueError:
            continue
        if isinstance(entry, dict) and entry.get("date") != date:
            entries.append(entry)
    return entries


def write_history(directory: Path, date: str, figures: dict) -> None:
    entries = read_history(directory, date)
    entries.append({"date": date, **figures})
    text = "".join(json.dumps(e) + "\n" for e in entries[-HISTORY_KEEP:])
    (directory / HISTORY_NAME).write_text(text, encoding="utf-8")


def _figure(
    label: str,
    value: float,
    points: list[float],
    fmt,
    flags: list[str],
) -> str:
    """One header figure beside its rolling median, flagged past
    :data:`BALLOON_FACTOR` of it."""
    points = points[-MEDIAN_WINDOW:]
    if len(points) < MEDIAN_MIN_POINTS:
        return f"{label} {fmt(value)} (median n/a, {len(points)} earlier digest(s))"
    median = statistics.median(points)
    text = f"{label} {fmt(value)} ({len(points)}-digest median {fmt(median)})"
    if median > 0 and value > BALLOON_FACTOR * median:
        text += " FLAGGED"
        flags.append(f"{label} {fmt(value)} is over {BALLOON_FACTOR:g}x its median")
    return text


def _pct(fraction: float | None) -> str:
    return "n/a" if fraction is None else f"{fraction * 100:+.1f}%"


def calibration_lines(calibration: dict | None, flags: list[str]) -> list[str]:
    """The rate-calibration line and one row per rate-table model."""
    if not calibration or calibration.get("error"):
        why = (calibration or {}).get("error") or "no calibration supplied"
        return [f"**Rate calibration**: no reading — {why}."]
    day = calibration.get("day", "?")
    rec = calibration.get("reconcile") or {}
    tolerance = calibration.get("tolerance", fleet_cost.DEFAULT_TOLERANCE)
    line = (
        f"**Rate calibration** ({day}, the last settled day): "
        f"{sm.money(rec.get('metered', 0.0))} metered, "
        f"{sm.money(rec.get('expected', 0.0))} with the classifier, vs "
        f"{sm.money(rec.get('billed_matched', 0.0))} billed — drift "
        f"{_pct(rec.get('drift'))} (tolerance {tolerance:.0%})"
    )
    if rec.get("flagged"):
        line += " FLAGGED"
        flags.append(f"{day} Bedrock drift {_pct(rec.get('drift'))}")
    lines = [line]
    if calibration.get("rates_error"):
        lines.append(f"- rate rows not read — {calibration['rates_error']}")
        return lines
    checks = {}
    for service in (calibration.get("rates") or {}).get("services", {}).values():
        check = service.get("check") or {}
        if check.get("model"):
            checks[check["model"]] = check
    for model, rates in sm.RATES_BY_MODEL.items():
        standing = f"verified {rates.verified}" if rates.verified else "unverified"
        check = checks.get(model)
        if check is None:
            lines.append(f"- `{model}` ({standing}): not billed on {day}")
            continue
        priced, billed = check.get("priced") or 0.0, check.get("billed") or 0.0
        row_drift = (priced - billed) / billed if billed else None
        text = (
            f"- `{model}` ({standing}): priced {sm.money(priced)} vs "
            f"{sm.money(billed)} billed, drift {_pct(row_drift)}"
        )
        if check.get("matches") is None:
            text += " — not judged, an unrecognized usage type billed; FLAGGED"
            flags.append(f"rate row {model} could not be judged on {day}")
        elif row_drift is not None and abs(row_drift) > RATE_TOLERANCE:
            text += " FLAGGED"
            flags.append(f"rate row {model} drift {_pct(row_drift)} on {day}")
        lines.append(text)
    return lines


# --------------------------------------------------------------------------- #
# Rendering.
# --------------------------------------------------------------------------- #


def _dollars(value: float | None, seat: bool) -> str:
    if value is None:
        return "unpriced"
    return sm.money(value) + (f" ({sm.NOT_BILLED})" if seat else "")


def session_block(s: SessionDigest) -> list[str]:
    seat = s.substrate != sm.SUBSTRATE_BEDROCK
    report = s.report
    totals: sm.Totals = report["totals"]
    issue = fleet_cost.issue_of(s.head.cwd) or s.head.issue or fleet_cost.NO_ISSUE
    title = f"### {issue} · {s.head.verb or 'no verb'} · {s.substrate} · `{sm.short_id(s.session_id)}`"
    if s.live:
        title += " · live"
    lines = [title, ""]
    for day, d in s.days.items():
        text = f"- {day}: {_dollars(d['dollars'], seat)}, {d['turns']} turn(s)"
        if d["rewarm"]:
            text += f"; cache re-warm {sm.money(d['rewarm'])}"
        lines.append(text)
    lines.append(
        f"- **Cumulative**: {_dollars(s.cumulative, seat)}, {totals.turns} turn(s), "
        f"peak prefix {sm.human(totals.prefix_max)}"
    )
    if s.estimate is None:
        lines.append("- **Exit estimate**: none recorded")
    else:
        drift = (
            None
            if s.cumulative is None or not s.cumulative
            else (s.estimate - s.cumulative) / s.cumulative
        )
        lines.append(
            f"- **Exit estimate**: {sm.money(s.estimate)} vs "
            f"{_dollars(s.cumulative, False)} here, drift {_pct(drift)}"
        )
    models = [
        f"  - main `{model}` {_dollars(price({model: tokens}), False)}"
        for model, tokens in sorted(totals.by_model.items())
        if tokens.billable()
    ]
    # Sub-agents grouped by the model each billed on, its own line per model:
    # a fan-out of fourteen lanes is one line, and a lane left on a stale model
    # still stands out as a line of its own.
    lanes: dict[str, list[sm.Tokens]] = {}
    agents: dict[str, set[str]] = {}
    for agent in report["subagents"]:
        for model, tokens in agent.by_model.items():
            if tokens.billable():
                lanes.setdefault(model, []).append(tokens)
                agents.setdefault(model, set()).add(agent.agent)
    for model in sorted(lanes):
        dollars: float | None = 0.0
        for tokens in lanes[model]:
            dollars = _add(dollars, price({model: tokens}))
        models.append(
            f"  - sub-agents `{model}` ×{len(agents[model])} {_dollars(dollars, False)}"
        )
    if models:
        lines.append("- **Models** (whole session):")
        lines += models
    tools = report["tools"][:BLOCK_TOP_N]
    if tools:
        lines.append(
            "- **Top sinks**: "
            + ", ".join(
                f"{t.name} ≈{sm.human(t.result_bytes // sm.BYTES_PER_TOKEN)} "
                f"({t.calls} call(s))"
                for t in tools
            )
        )
    sinks = report["top_sinks"][:BLOCK_TOP_N]
    if sinks:
        lines.append(
            "- **Largest results**: "
            + ", ".join(
                f"≈{sm.human(r.bytes // sm.BYTES_PER_TOKEN)} {r.name}"
                + (f" `{r.label}`" if r.label else "")
                for r in sinks
            )
        )
    resident: sm.ResidentProse = report["resident_prose"]
    if resident.token_turns:
        lines.append(
            f"- **Resident prose**: {resident.share * 100:.0f}% of all input"
            + (" — a lever" if resident.is_lever() else "")
        )
    candidates = report["hardening_candidates"][:BLOCK_TOP_N]
    if candidates:
        lines.append(
            "- **Hardening**: "
            + ", ".join(
                f"`{c.signature}` ({c.cost_kind()}, {c.count} call(s))"
                for c in candidates
            )
        )
    if s.head.account:
        lines.append("- **Session account**:")
        lines.append("")
        lines.extend(f"> {ln}" if ln.strip() else ">" for ln in s.head.account)
    else:
        lines.append("- **Session account**: no account")
    lines.append("")
    return lines


def render(
    date: str,
    since: str,
    sessions: list[SessionDigest],
    figures: dict,
    history: list[dict],
    calibration: dict | None,
) -> tuple[str, list[str]]:
    """The digest's Markdown and its flagged lines."""
    flags: list[str] = []
    out = [f"# Session digest — {date}", ""]
    out += calibration_lines(calibration, flags)
    out.append("")

    def points(key: str) -> list[float]:
        return [float(e[key]) for e in history if isinstance(e.get(key), (int, float))]

    def model_points(model: str) -> list[float]:
        return [
            float((e.get("models") or {}).get(model, 0.0))
            for e in history
            if isinstance(e.get("models"), dict)
        ]

    out.append(
        f"**Window**: since {since}Z (UTC), {figures['sessions']} session(s) "
        f"digested, {sum(s.live for s in sessions)} live"
    )
    out.append(
        "- "
        + _figure("Bedrock", figures["bedrock"], points("bedrock"), sm.money, flags)
    )
    out.append(
        "- "
        + _figure("Seat", figures["seat"], points("seat"), sm.money, flags)
        + f" — {sm.NOT_BILLED}"
    )
    out.append(
        "- "
        + _figure(
            "Sub-agent share",
            figures["subagent_share"],
            points("subagent_share"),
            lambda v: f"{v * 100:.0f}%",
            flags,
        )
    )
    for model, dollars in figures["models"].items():
        out.append(
            "- " + _figure(f"`{model}`", dollars, model_points(model), sm.money, flags)
        )
    out.append(
        "- "
        + _figure(
            "Cache re-warm across days",
            figures["rewarm"],
            points("rewarm"),
            sm.money,
            flags,
        )
        + f" over {figures['rewarm_sessions']} session(s)"
    )
    unpriced = sorted({m for s in sessions for m in s.unpriced})
    if unpriced:
        names = ", ".join(f"`{m}`" for m in unpriced)
        out.append(f"- **Unpriced**: {names} — their tokens are in no figure above")
        flags.append(f"unpriced model(s) {', '.join(unpriced)}")
    out.append(f"- **Flagged**: {len(flags)}")
    out += [f"  - {f}" for f in flags]
    out.append("")
    out.append("## Sessions")
    out.append("")
    for s in sorted(sessions, key=lambda s: -(s.window_dollars or 0.0)):
        out += session_block(s)
    return "\n".join(out).rstrip("\n") + "\n", flags


def to_twin(
    date: str, since: str, sessions: list[SessionDigest], figures: dict, flags: list
) -> dict:
    """The JSON twin: what the allowlist-refresh mining and any later
    consumer read instead of re-aggregating transcripts."""
    return {
        "date": date,
        "since": since,
        "figures": figures,
        "flags": flags,
        "sessions": [
            {
                "session_id": s.session_id,
                "verb": s.head.verb,
                "issue": fleet_cost.issue_of(s.head.cwd) or s.head.issue,
                "substrate": s.substrate,
                "live": s.live,
                "days": s.days,
                "window_dollars": s.window_dollars,
                "cumulative_dollars": s.cumulative,
                "estimate_dollars": s.estimate,
                "hardening_candidates": [
                    {
                        "signature": c.signature,
                        "count": c.count,
                        "cost_kind": c.cost_kind(),
                    }
                    for c in s.report["hardening_candidates"]
                ],
            }
            for s in sessions
        ],
    }


# --------------------------------------------------------------------------- #
# The CLI.
# --------------------------------------------------------------------------- #


def _instant(value: str) -> datetime.datetime:
    parsed = datetime.datetime.fromisoformat(value.strip().replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=datetime.timezone.utc)
    return parsed.astimezone(datetime.timezone.utc)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="session_metrics.py digest",
        description="Write the daily session digest; print a one-line summary.",
    )
    parser.add_argument("--base", required=True, help="the base checkout")
    parser.add_argument(
        "--since",
        help=f"ISO instant to digest from (default: {DEFAULT_SINCE_HOURS}h ago)",
    )
    parser.add_argument(
        "--calibration", help="JSON file of the settled day's reconcile and rates"
    )
    parser.add_argument("--now", help="override the current instant (testing)")
    parser.add_argument(
        "--projects", help="the Claude projects directory (default: the Claude home's)"
    )
    args = parser.parse_args(argv)

    try:
        now = (
            _instant(args.now)
            if args.now
            else datetime.datetime.now(datetime.timezone.utc)
        )
        since = (
            _instant(args.since)
            if args.since
            else now - datetime.timedelta(hours=DEFAULT_SINCE_HOURS)
        )
    except ValueError as exc:
        parser.error(f"not an ISO instant: {exc}")
    calibration = None
    if args.calibration:
        try:
            calibration = json.loads(Path(args.calibration).read_text("utf-8"))
        except (OSError, ValueError) as exc:
            calibration = {"error": f"calibration unreadable: {exc}"}

    base = Path(args.base)
    projects = Path(args.projects) if args.projects else sm.claude_home() / "projects"
    since_text = since.strftime("%Y-%m-%dT%H:%M:%S")
    date = now.date().isoformat()
    sessions = []
    for path in discover(projects, base, since.timestamp()):
        try:
            digest = digest_session(path, base, since_text, now.timestamp())
        except OSError:
            continue
        if digest is not None:
            sessions.append(digest)

    directory = base / DIGEST_DIR
    directory.mkdir(parents=True, exist_ok=True)
    figures = header_figures(sessions)
    history = read_history(directory, date)
    text, flags = render(date, since_text, sessions, figures, history, calibration)
    md_path = directory / f"digest-{date}.md"
    md_path.write_text(text, encoding="utf-8")
    twin = to_twin(date, since_text, sessions, figures, flags)
    (directory / f"digest-{date}.json").write_text(
        json.dumps(twin, indent=2), encoding="utf-8"
    )
    write_history(directory, date, figures)
    print(
        json.dumps(
            {
                "sessions": len(sessions),
                "dollars": round(figures["total"], 2),
                "flagged": len(flags),
                "path": str(md_path),
            }
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
