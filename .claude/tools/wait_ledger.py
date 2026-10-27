#!/usr/bin/env python3
"""The wait ledger — where a session's wall clock went while it waited.

Every dimension is read from timestamps the transcript already carries, so a
session needs no new instrumentation: a tool call's wait is its ``tool_use``
record's stamp to its ``tool_result``'s, and a backgrounded call's runs on to
the ``<task-notification>`` that names its id. One line per dimension,
ranked largest first, so the next bottleneck is always the top line:

=================  ==========================================================
pool lock          a Bash call whose result carries cargo's ``Blocking
                   waiting for file lock``, by step (the ``make`` target,
                   ``lint_paths.py``'s changed-set lint, or the cargo verb)
CI: checks         ``wait_for_checks.py`` without ``--queue``, and a bare
                   ``gh pr checks --watch`` / ``gh run watch``
CI: merge queue    ``wait_for_checks.py --queue``
verify race        ``verify_head.py``, with the side (``ci`` / ``local``)
                   that settled each check, read from its verdict line
operator           an ``AskUserQuestion`` / ``ExitPlanMode`` to its answer,
                   and a call the operator refused at its permission prompt;
                   by skill point (the skill last invoked, the ask's header)
idle               the session's last output to the operator's next message,
                   while nothing above was pending
sub-agents         the ``Agent`` tool's calls — listed, never counted
=================  ==========================================================

A ledger dominated by the lock is a machine problem; by CI, a GitHub one; by
the operator, an attention one; by idle, a session that should have closed.

**Known limits.** A permission ask the operator *granted* leaves no record,
so only a refused one is measured. A lock wait is the whole call's duration,
the build after the lock included: the transcript holds no stamp for the
moment the lock was granted.

The digest half (:func:`digest_section`) sums the ledgers over a day and adds
the launch records (``sessions/<id>.load``, one JSON line per launch from
``machine_load.py --record``) and the fleet log's queue profile. Its wait
section is written for the fold analyst: each ranked cause carries its hours,
its median per session and a stable ``wait:<dimension>:<cause>``
fingerprint.

Stdlib only; no import of ``session_metrics`` (which imports this), so the
caller hands each record in with its ``utc_stamp`` already parsed. Tests live
in ``tests/test_wait_ledger.py``.
"""

from __future__ import annotations

import datetime
import json
import re
import shlex
import statistics
from dataclasses import dataclass, field
from pathlib import Path

LOCK = "lock"
CHECKS = "ci-checks"
QUEUE = "ci-queue"
RACE = "verify-race"
OPERATOR = "operator"
IDLE = "idle"
SUBAGENTS = "sub-agents"
DIMENSIONS = (LOCK, CHECKS, QUEUE, RACE, OPERATOR, IDLE, SUBAGENTS)
LABEL = {
    LOCK: "pool lock",
    CHECKS: "CI: checks",
    QUEUE: "CI: merge queue",
    RACE: "verify race",
    OPERATOR: "operator",
    IDLE: "idle",
    SUBAGENTS: "sub-agents",
}
# What a dominant dimension says about where the fix lives.
DIAGNOSIS = {
    LOCK: "a machine problem",
    CHECKS: "a GitHub problem",
    QUEUE: "a GitHub problem",
    RACE: "a GitHub problem",
    OPERATOR: "an attention problem",
    IDLE: "a session that should have closed",
}

LOCK_MARKER = "Blocking waiting for file lock"
# Claude Code's reply to a tool call refused at its permission prompt.
REFUSAL_MARKERS = (
    "The user doesn't want to proceed with this tool use",
    "The user doesn't want to take this action right now",
)
OPERATOR_TOOLS = ("AskUserQuestion", "ExitPlanMode")
# Skills another skill calls mid-flow: invoking one does not move the skill
# point, so review-pr's merge-queue ask is not filed under its title helper.
HELPER_SKILLS = ("pr-title-description", "commit-changes", "session-metrics")
AGENT_TOOLS = ("Agent", "Task")
NOTIFICATION_MARKER = "<task-notification>"
# Records that arrive as a user turn but are not the operator typing.
NOT_OPERATOR_MARKERS = (
    NOTIFICATION_MARKER,
    "<cross-session-message",
    "<local-command-stdout>",
    "<local-command-caveat>",
)
_BACKGROUND_ID = re.compile(r"\b(?:ID|id|agentId|task_id)[:=]\s*([\w-]{3,})")
_RACE_SIDE = re.compile(r"\b(ci|local)\b\W{1,3}(green|red)\b")
_COMMAND_NAME = re.compile(r"<command-name>/([\w:-]+)</command-name>")
_SLUG = re.compile(r"[^a-z0-9]+")


def slug(text: str) -> str:
    """A fingerprint-safe token: lowercase, dashes, no dots."""
    return _SLUG.sub("-", text.lower()).strip("-") or "unnamed"


def _seconds(start: str, end: str) -> int:
    fmt = "%Y-%m-%dT%H:%M:%S"
    delta = datetime.datetime.strptime(end, fmt) - datetime.datetime.strptime(
        start, fmt
    )
    return max(0, int(delta.total_seconds()))


def _text(content) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(
            str(i.get("text", "")) if isinstance(i, dict) else str(i) for i in content
        )
    return ""


def _tokens(command: str) -> list[str]:
    """The command's words with leading env assignments and a
    ``run_quiet.py … --`` wrapper looked through."""
    try:
        tokens = shlex.split(command, comments=True)
    except ValueError:
        tokens = command.split()
    while tokens and re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", tokens[0]):
        tokens.pop(0)
    if any(t.endswith("run_quiet.py") for t in tokens[:2]) and "--" in tokens:
        tokens = tokens[tokens.index("--") + 1 :]
    return tokens


def bash_wait(command: str) -> tuple[str, str] | None:
    """The dimension and cause a Bash command waits as by its shape alone, or
    ``None`` when only its result can say (a lock wait)."""
    tokens = _tokens(command)
    script = next((Path(t).name for t in tokens[:2] if t.endswith(".py")), "")
    if script == "wait_for_checks.py":
        if "--queue" in tokens:
            return QUEUE, ""
        return CHECKS, ""
    if script == "verify_head.py":
        return RACE, ""
    if tokens[:3] == ["gh", "pr", "checks"] and "--watch" in tokens:
        return CHECKS, ""
    if tokens[:3] == ["gh", "run", "watch"]:
        return CHECKS, ""
    return None


def lock_step(command: str) -> str:
    """The step a lock-blocked command was: ``make-<target>``,
    ``lint-changed`` or ``cargo-<verb>``."""
    tokens = _tokens(command)
    if not tokens:
        return "unnamed"
    program = Path(tokens[0]).name
    if program == "make":
        target = next(
            (t for t in tokens[1:] if not t.startswith("-") and "=" not in t), ""
        )
        return slug(f"make-{target}" if target else "make")
    if program.startswith("python") and len(tokens) > 1:
        script = Path(tokens[1]).name
        return "lint-changed" if script == "lint_paths.py" else slug(script[:-3])
    if program == "cargo":
        verb = next((t for t in tokens[1:] if not t.startswith(("-", "+"))), "")
        return slug(f"cargo-{verb}" if verb else "cargo")
    return slug(program)


@dataclass
class WaitEvent:
    dimension: str
    cause: str
    seconds: int
    at: str | None
    sides: dict[str, int] = field(default_factory=dict)


@dataclass
class DimensionLine:
    """One dimension's total over the ledger's window."""

    dimension: str
    seconds: int = 0
    calls: int = 0
    causes: dict[str, int] = field(default_factory=dict)
    # The race only: which side settled each check.
    sides: dict[str, int] = field(default_factory=dict)

    @property
    def counted(self) -> bool:
        return self.dimension != SUBAGENTS

    @property
    def label(self) -> str:
        return LABEL[self.dimension]


@dataclass
class _Open:
    dimension: str | None
    cause: str
    at: str
    command: str = ""
    background: bool = False


class WaitLedger:
    """Fed a main transcript's records in order, with each record's
    ``utc_stamp``; :meth:`finish` ranks the dimensions. Only waits that start
    in ``[since, until)`` count, the window the digest slices by."""

    def __init__(self, since: str | None = None, until: str | None = None) -> None:
        self.since = since
        self.until = until
        self.events: list[WaitEvent] = []
        self._open: dict[str, _Open] = {}
        self._background: dict[str, _Open] = {}
        self._seen: set[str] = set()
        self._skill = "session"
        self._last_output: str | None = None

    def _in_window(self, stamp: str | None) -> bool:
        if stamp is None:
            return False
        if self.since is not None and stamp < self.since:
            return False
        return self.until is None or stamp < self.until

    def _close(self, opened: _Open, end: str | None, text: str = "") -> None:
        if opened.dimension is None or end is None:
            return
        if not self._in_window(opened.at):
            return
        sides: dict[str, int] = {}
        if opened.dimension == RACE:
            for side, _ in _RACE_SIDE.findall(text):
                sides[side] = sides.get(side, 0) + 1
        self.events.append(
            WaitEvent(
                opened.dimension,
                opened.cause,
                _seconds(opened.at, end),
                opened.at,
                sides,
            )
        )

    def observe(self, rec: dict, stamp: str | None) -> None:
        msg = rec.get("message")
        if not isinstance(msg, dict) or stamp is None:
            return
        role = msg.get("role") or rec.get("type")
        content = msg.get("content")
        items = content if isinstance(content, list) else []
        if role == "assistant":
            self._last_output = stamp
            for item in items:
                if isinstance(item, dict) and item.get("type") == "tool_use":
                    self._open_call(item, stamp)
            return
        if role != "user":
            return
        results = [
            i for i in items if isinstance(i, dict) and i.get("type") == "tool_result"
        ]
        for item in results:
            self._close_call(item, stamp)
        if results or rec.get("isMeta"):
            return
        text = _text(content)
        if NOTIFICATION_MARKER in text:
            self._close_background(text, stamp)
            return
        found = _COMMAND_NAME.search(text)
        if found:
            self._skill = found.group(1)
        if any(m in text for m in NOT_OPERATOR_MARKERS):
            return
        # The operator typed: idle only when the session was waiting on
        # nothing — no call open, no backgrounded watch still running.
        if self._last_output and not self._open and not self._background:
            self._close(_Open(IDLE, "", self._last_output), stamp)
        self._last_output = None

    def _open_call(self, item: dict, stamp: str) -> None:
        tid = item.get("id")
        if not isinstance(tid, str) or tid in self._seen:
            return
        self._seen.add(tid)
        name = item.get("name")
        args = item.get("input") if isinstance(item.get("input"), dict) else {}
        skill = args.get("skill")
        if name == "Skill" and isinstance(skill, str) and skill not in HELPER_SKILLS:
            self._skill = skill
        dimension, cause, command = None, "", ""
        if name == "Bash" and isinstance(args.get("command"), str):
            command = args["command"]
            found = bash_wait(command)
            if found:
                dimension, cause = found
        elif name in OPERATOR_TOOLS:
            header = ""
            questions = args.get("questions")
            if isinstance(questions, list) and questions:
                first = questions[0]
                if isinstance(first, dict):
                    header = str(first.get("header") or "")
            dimension = OPERATOR
            cause = slug(f"{self._skill}-{header or name}")
        elif name in AGENT_TOOLS:
            dimension = SUBAGENTS
            cause = slug(str(args.get("subagent_type") or "general-purpose"))
        self._open[tid] = _Open(
            dimension, cause, stamp, command, bool(args.get("run_in_background"))
        )

    def _close_call(self, item: dict, stamp: str) -> None:
        opened = self._open.pop(str(item.get("tool_use_id")), None)
        if opened is None:
            return
        text = _text(item.get("content"))
        if any(m in text for m in REFUSAL_MARKERS):
            self._close(
                _Open(OPERATOR, slug(f"{self._skill}-permission"), opened.at), stamp
            )
            return
        if opened.dimension is None and opened.command and LOCK_MARKER in text:
            opened.dimension, opened.cause = LOCK, lock_step(opened.command)
        background = opened.background or opened.dimension == SUBAGENTS
        ids = _BACKGROUND_ID.findall(text) if background else []
        if ids and opened.dimension is not None:
            self._background[ids[0]] = opened
            return
        self._close(opened, stamp, text)

    def _close_background(self, text: str, stamp: str) -> None:
        for key in [k for k in self._background if k in text]:
            self._close(self._background.pop(key), stamp, text)

    def finish(self) -> list[DimensionLine]:
        """Every dimension, the counted ones ranked largest first, the
        sub-agents line last."""
        lines = {d: DimensionLine(d) for d in DIMENSIONS}
        for e in self.events:
            line = lines[e.dimension]
            line.seconds += e.seconds
            line.calls += 1
            line.causes[e.cause] = line.causes.get(e.cause, 0) + e.seconds
            for side, n in e.sides.items():
                line.sides[side] = line.sides.get(side, 0) + n
        counted = sorted(
            (lines[d] for d in DIMENSIONS if d != SUBAGENTS),
            key=lambda line: (-line.seconds, DIMENSIONS.index(line.dimension)),
        )
        return counted + [lines[SUBAGENTS]]


def hours(seconds: float) -> str:
    return f"{seconds / 3600:.2f}h"


def _span(seconds: int) -> str:
    if seconds < 90:
        return f"{seconds}s"
    if seconds < 5400:
        return f"{seconds / 60:.0f}m"
    return hours(seconds)


def ledger_lines(lines: list[DimensionLine]) -> list[str]:
    """The session report's ledger: one bullet per dimension and a verdict."""
    out = ["**Wait ledger** (largest first):", ""]
    for line in lines:
        if not line.calls:
            out.append(f"- {line.label}: none")
            continue
        causes = ", ".join(
            f"{c or 'unnamed'} {_span(s)}"
            for c, s in sorted(line.causes.items(), key=lambda kv: -kv[1])
        )
        text = f"- {line.label}: {_span(line.seconds)} over {line.calls} wait(s)"
        if causes and any(line.causes):
            text += f" — {causes}"
        if line.sides:
            text += " — settled by " + ", ".join(
                f"{s} {n}" for s, n in sorted(line.sides.items())
            )
        if not line.counted:
            text += " (listed, not counted as waiting)"
        out.append(text)
    top = lines[0] if lines else None
    if top is not None and top.seconds:
        out.append("")
        out.append(f"Dominated by {top.label}: {DIAGNOSIS[top.dimension]}.")
    return out + [""]


def to_dict(line: DimensionLine) -> dict:
    return {
        "dimension": line.dimension,
        "seconds": line.seconds,
        "calls": line.calls,
        "counted": line.counted,
        "causes": line.causes,
        "sides": line.sides,
    }


# --------------------------------------------------------------------------- #
# The digest's inputs: launch records and the fleet log.
# --------------------------------------------------------------------------- #


def read_load_records(directory: Path, since: str, until: str) -> list[dict]:
    """Every launch record line under ``directory`` (``<id>.load``) stamped in
    ``[since, until)``. An unreadable line is skipped, not fatal."""
    out = []
    for path in sorted(directory.glob("*.load")) if directory.is_dir() else []:
        try:
            text = path.read_text("utf-8")
        except OSError:
            continue
        for raw in text.splitlines():
            try:
                rec = json.loads(raw)
            except ValueError:
                continue
            at = str(rec.get("at", ""))[:19]
            if isinstance(rec, dict) and since <= at < until:
                out.append(rec)
    return out


_SAMPLE = re.compile(
    r"^(\S+)Z builders=(\d+) builds=(\d+) waiting=(\S+) longest_s=(\d+)"
    r" in_use_gib=([\d.]+)"
)


def queue_profile(text: str, since: str, until: str) -> dict[str, dict]:
    """The fleet log's samples in ``[since, until)`` by UTC hour: the deepest
    build queue, the longest wait and the peak memory seen in it."""
    hours_: dict[str, dict] = {}
    for raw in text.splitlines():
        found = _SAMPLE.match(raw.strip())
        if not found or not since <= found.group(1) < until:
            continue
        at, _, _, waiting, longest, in_use = found.groups()
        depth = sum(
            int(part.rpartition(":")[2])
            for part in waiting.split(",")
            if part != "-" and part.rpartition(":")[2].isdigit()
        )
        hour = hours_.setdefault(
            at[:13], {"samples": 0, "depth": 0, "longest_s": 0, "in_use_gib": 0.0}
        )
        hour["samples"] += 1
        hour["depth"] = max(hour["depth"], depth)
        hour["longest_s"] = max(hour["longest_s"], int(longest))
        hour["in_use_gib"] = max(hour["in_use_gib"], float(in_use))
    return dict(sorted(hours_.items()))


def _median(values: list[float]) -> float | None:
    return statistics.median(values) if values else None


def _ci_cause(ci: dict | None) -> str:
    """The job most often on the merge queue's critical path, the day's
    nameable CI cause; empty when the CI section was not read."""
    if not isinstance(ci, dict) or ci.get("error"):
        return ""
    critical = ci.get("critical_path") or []
    return slug(str(critical[0][0])) if critical else ""


def digest_section(
    ledgers: list[list[DimensionLine]],
    loads: list[dict],
    profile: dict[str, dict],
    ci: dict | None = None,
) -> tuple[list[str], dict]:
    """The digest's ``## Wait`` section and its JSON twin: the daily line,
    then the ranked causes the analyst reads, one fingerprint each."""
    ci_cause = _ci_cause(ci)
    totals = {d: 0 for d in DIMENSIONS}
    # (dimension, cause) -> per-session seconds.
    per_cause: dict[tuple[str, str], list[int]] = {}
    for ledger in ledgers:
        for line in ledger:
            totals[line.dimension] += line.seconds
            for cause, seconds in line.causes.items():
                if line.dimension in (CHECKS, QUEUE) and not cause:
                    cause = ci_cause
                if seconds:
                    per_cause.setdefault((line.dimension, cause), []).append(seconds)
    ranked = sorted(
        (d for d in DIMENSIONS if d != SUBAGENTS and totals[d]),
        key=lambda d: (-totals[d], DIMENSIONS.index(d)),
    )
    builds = sorted(
        ((c, sum(v)) for (d, c), v in per_cause.items() if d == LOCK),
        key=lambda kv: -kv[1],
    )
    longest = [float(r.get("longest_wait_s") or 0) for r in loads]
    building = [float(r.get("building") or 0) for r in loads]
    free = [float(r.get("free") or 0) for r in loads]
    refusals = sum(
        1 for r in loads if "memory" in ((r.get("verdict") or {}).get("reasons") or [])
    )
    in_use = [float(r.get("in_use") or 0) / 1024**3 for r in loads] + [
        h["in_use_gib"] for h in profile.values()
    ]

    def fmt(value: float | None, unit: str = "") -> str:
        return "n/a" if value is None else f"{value:.0f}{unit}"

    out = [
        "**Daily**: "
        + (
            ", ".join(f"{LABEL[d]} {hours(totals[d])}" for d in ranked)
            or "no waiting measured"
        ),
        "- Fleet build wait by target: "
        + (", ".join(f"{c} {hours(s)}" for c, s in builds) or "none"),
        f"- CI wait: {hours(totals[CHECKS] + totals[QUEUE])}"
        f" (checks {hours(totals[CHECKS])}, queue {hours(totals[QUEUE])})",
        "- Queue profile by hour (depth / longest wait): "
        + (
            ", ".join(
                f"{h[11:13]}h {v['depth']}/{v['longest_s']}s"
                for h, v in profile.items()
            )
            or "no fleet-log samples"
        ),
        f"- Launches: {len(loads)}; median longest wait at launch"
        f" {fmt(_median(longest), 's')}; median builders {fmt(_median(building))},"
        f" free {fmt(_median(free))}; memory refusals {refusals};"
        f" peak in_use {(max(in_use) if in_use else 0):.1f} GiB",
        "",
        "**For the analyst** — ranked causes; file a wait lever only for a"
        " recurring named cause, never a one-day spike:",
        "",
    ]
    causes = []
    for (dimension, cause), values in sorted(
        per_cause.items(),
        key=lambda kv: (kv[0][0] == SUBAGENTS, -sum(kv[1]), kv[0]),
    ):
        # Sub-agents are not waiting, and a cause-less wait names no change.
        fingerprint = (
            f"wait:{dimension}:{cause}" if cause and dimension != SUBAGENTS else None
        )
        causes.append(
            {
                "dimension": dimension,
                "cause": cause or None,
                "seconds": sum(values),
                "sessions": len(values),
                "median_session_s": _median([float(v) for v in values]),
                "fingerprint": fingerprint,
            }
        )
        if dimension == SUBAGENTS:
            continue
        tail = (
            f"`**Fingerprint**: {fingerprint}`"
            if fingerprint
            else "no nameable cause — report, don't file"
        )
        out.append(
            f"- {LABEL[dimension]} · {cause or 'unnamed'}: {hours(sum(values))}"
            f" over {len(values)} session(s), median {_span(int(_median(values)))}"
            f" per session — {tail}"
        )
    if not any(c["dimension"] != SUBAGENTS for c in causes):
        out.append("- nothing to rank")
    twin = {
        "totals_s": totals,
        "ranked": ranked,
        "causes": causes,
        "launches": len(loads),
        "median_longest_wait_s": _median(longest),
        "memory_refusals": refusals,
        "peak_in_use_gib": round(max(in_use), 1) if in_use else None,
        "queue_profile": profile,
    }
    return out, twin
