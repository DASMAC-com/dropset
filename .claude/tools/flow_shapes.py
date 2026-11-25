#!/usr/bin/env python3
"""Flow shapes — repeated runs of judgment-free turns a single tool run would
eliminate.

The hardening table in ``session_metrics.py`` ranks repeated **command shapes**
by result size: what one call costs. This ranks the complement, repeated
**flow shapes**: a sequence of consecutive turns with no judgment between
them — a chain of deterministic Bash / MCP acts, a poll-and-react loop, a
lookup-then-act pair — which one tool run would collapse into one turn. Each
eliminated turn re-paid the whole prefix, so the ranking key is the **prefix
replay** those turns paid (`docs/conventions/context-economy.md` → "Session
length is itself a cost lever").

A **turn** is one assistant message (its split content-block records folded
by message id). A turn is a **step** when every call it makes is an act:
a ``Bash`` call (named by the caller's command-shape normalizer, so the two
tables group commands alike) or an ``mcp__*`` call (named by its tool). A run
of steps ends at anything that carries judgment:

* a turn calling any other tool — an edit, a write, a question, a sub-agent,
  a skill;
* a turn with no tool call at all (prose to the operator);
* a user record that is not purely tool results — the operator typing, a
  notification, a cross-session message.

* the same command shape again with new arguments — a search survey, each
  argument chosen from the last result.

A ``ToolSearch`` call is neutral: it neither extends nor ends a run. The
*identical* call repeated folds into one **loop** step (``(loop)``), so a poll
repeated three times and five times is the same shape.

A flow shape is an ordered tuple of steps spanning at least two turns that
recurs at least :data:`MIN_OCCURRENCES` times (non-overlapping, within a run),
in one session or across several. A tool run would leave **one** turn per
occurrence, so the turns it eliminates are the turns spent less the
occurrences, and the replay saved is the prefix of every turn but each
occurrence's first. A shape whose every occurrence lies inside a longer
reported shape is dropped as subsumed.

Stdlib only, and no import of ``session_metrics`` (which imports this): the
caller hands in its normalizer. Tests live in ``tests/test_flow_shapes.py``.
"""

from __future__ import annotations

from collections.abc import Callable, Iterable
import json
import re
from dataclasses import dataclass, field

# A shape must recur this often to be worth a tool: once is an incident.
MIN_OCCURRENCES = 2
# The longest window considered, in steps. The init-pr bootstrap before its
# collapse was about a dozen acts; past this a chain is a session, not a flow.
MAX_STEPS = 16
# The default report's length; the full series is behind a flag.
TOP_N = 5

NEUTRAL_TOOLS = frozenset({"ToolSearch"})
LOOP = " (loop)"

Signature = Callable[[str], str]


@dataclass
class Step:
    """One step of a run: a turn's call signature, each turn's prefix in
    tokens (more than one for a folded loop), and the raw calls that only an
    identical repeat may fold into."""

    sig: str
    prefixes: list[int] = field(default_factory=list)
    raw: str = ""

    @property
    def turns(self) -> int:
        return len(self.prefixes)

    def key(self) -> str:
        return self.sig + (LOOP if self.turns > 1 else "")


@dataclass
class FlowShape:
    """A ranked flow shape and what one tool run in its place would save."""

    steps: tuple[str, ...]
    occurrences: int = 0
    turns: int = 0
    replay_tokens: int = 0
    sessions: set[int] = field(default_factory=set)

    @property
    def turns_eliminated(self) -> int:
        """The turns spent less the one per occurrence a tool run leaves."""
        return self.turns - self.occurrences

    def label(self) -> str:
        return " → ".join(self.steps)

    def to_dict(self) -> dict:
        return {
            "steps": list(self.steps),
            "occurrences": self.occurrences,
            "sessions": len(self.sessions),
            "turns": self.turns,
            "turns_left": self.occurrences,
            "turns_eliminated": self.turns_eliminated,
            "replay_tokens": self.replay_tokens,
        }


def _prefix(usage: dict) -> int:
    return sum(
        int(usage.get(k, 0) or 0)
        for k in (
            "input_tokens",
            "cache_creation_input_tokens",
            "cache_read_input_tokens",
        )
    )


def _call_sig(item: dict, signature: Signature) -> tuple[str, str] | None:
    """An act's signature and its raw call (``("", "")`` for a neutral
    call), or ``None`` for judgment."""
    name = item.get("name")
    if not isinstance(name, str):
        return None
    if name in NEUTRAL_TOOLS:
        return "", ""
    args = item.get("input")
    if name.startswith("mcp__"):
        return name[len("mcp__") :], json.dumps(args, sort_keys=True, default=str)
    if name == "Bash":
        command = args.get("command") if isinstance(args, dict) else None
        if not isinstance(command, str):
            return "bash", ""
        return signature(command) or "bash", command
    return None


class FlowTracker:
    """Streaming: :meth:`observe` each main-transcript record in order, then
    :meth:`finish` into the session's runs (each a list of :class:`Step`).

    ``since`` / ``until`` (``utc_stamp`` strings) window the turns like the
    day slices: a turn stamped outside the window ends the run and is not
    counted, so consecutive digests never count one turn twice."""

    def __init__(
        self, signature: Signature, since: str | None = None, until: str | None = None
    ) -> None:
        self.signature = signature
        self.since = since
        self.until = until
        self.runs: list[list[Step]] = []
        self._run: list[Step] = []
        # The turn being assembled: its message id, stamp, prefix and calls.
        self._turn_id: str | None = None
        self._turn_stamp: str | None = None
        self._turn_prefix = 0
        self._turn_calls: list[tuple[str, str] | None] = []
        self._turn_items: set[str] = set()

    def _in_window(self, stamp: str | None) -> bool:
        if stamp is None:
            return self.since is None and self.until is None
        return (self.since is None or stamp >= self.since) and (
            self.until is None or stamp < self.until
        )

    def _break(self) -> None:
        if self._run:
            self.runs.append(self._run)
        self._run = []

    def _end_turn(self) -> None:
        if self._turn_id is None:
            return
        calls = self._turn_calls
        acts = sorted(c for c in calls if c and c[0])
        if not self._in_window(self._turn_stamp) or any(c is None for c in calls):
            self._break()
        elif not acts:
            # Prose only ends the run; a neutral-only turn leaves it be.
            if not calls:
                self._break()
        else:
            sig = " + ".join(s for s, _ in acts)
            raw = "\n".join(r for _, r in acts)
            last = self._run[-1] if self._run else None
            if last is not None and last.sig == sig and last.raw == raw:
                # The same call again: a poll, folded into one loop step.
                last.prefixes.append(self._turn_prefix)
            else:
                if last is not None and last.sig == sig:
                    # The same shape with new arguments is a survey: each
                    # argument was chosen from the last result.
                    self._break()
                self._run.append(Step(sig, [self._turn_prefix], raw))
        self._turn_id = None
        self._turn_calls = []
        self._turn_items = set()
        self._turn_prefix = 0

    def observe(self, rec: dict, stamp: str | None) -> None:
        if not isinstance(rec, dict):
            return
        msg = rec.get("message")
        if not isinstance(msg, dict):
            return
        role = msg.get("role") or rec.get("type")
        content = msg.get("content")
        items = content if isinstance(content, list) else []
        if role == "assistant":
            mid = msg.get("id") if isinstance(msg.get("id"), str) else None
            if mid is None or mid != self._turn_id:
                self._end_turn()
                self._turn_id = mid or f"anon-{id(rec)}"
                self._turn_stamp = stamp
                usage = msg.get("usage")
                self._turn_prefix = _prefix(usage) if isinstance(usage, dict) else 0
            for item in items:
                if not isinstance(item, dict) or item.get("type") != "tool_use":
                    continue
                tid = item.get("id")
                if isinstance(tid, str):
                    if tid in self._turn_items:
                        continue
                    self._turn_items.add(tid)
                self._turn_calls.append(_call_sig(item, self.signature))
            return
        if role != "user" or rec.get("isMeta"):
            return
        self._end_turn()
        only_results = bool(items) and all(
            isinstance(i, dict) and i.get("type") == "tool_result" for i in items
        )
        if not only_results:
            self._break()

    def finish(self) -> list[list[Step]]:
        self._end_turn()
        self._break()
        return self.runs


def runs_from_lines(
    lines: Iterable[str], signature: Signature, stamp: Callable[[object], str | None]
) -> list[list[Step]]:
    """A flows-only pass over one transcript's lines, for ranking sessions
    other than the one being reported."""
    tracker = FlowTracker(signature)
    for line in lines:
        if not line.strip():
            continue
        try:
            rec = json.loads(line)
        except ValueError:
            continue
        if isinstance(rec, dict):
            tracker.observe(rec, stamp(rec.get("timestamp")))
    return tracker.finish()


def _contains(inner: tuple[str, ...], outer: tuple[str, ...]) -> bool:
    n = len(inner)
    return any(outer[i : i + n] == inner for i in range(len(outer) - n + 1))


def rank(sessions: list[list[list[Step]]]) -> list[FlowShape]:
    """Every flow shape across ``sessions`` (each a list of runs), best
    first: by prefix replay saved, then turns eliminated, then length."""
    shapes: dict[tuple[str, ...], FlowShape] = {}
    for s_idx, runs in enumerate(sessions):
        for run in runs:
            last_end: dict[tuple[str, ...], int] = {}
            for i in range(len(run)):
                for n in range(1, min(MAX_STEPS, len(run) - i) + 1):
                    window = run[i : i + n]
                    turns = sum(s.turns for s in window)
                    if turns < 2:
                        continue
                    key = tuple(s.key() for s in window)
                    # Non-overlapping occurrences: a window starting inside
                    # the last counted one of the same shape is the same act.
                    if i < last_end.get(key, 0):
                        continue
                    last_end[key] = i + n
                    shape = shapes.setdefault(key, FlowShape(key))
                    shape.occurrences += 1
                    shape.turns += turns
                    shape.replay_tokens += (
                        sum(sum(s.prefixes) for s in window) - window[0].prefixes[0]
                    )
                    shape.sessions.add(s_idx)
    recurring = [s for s in shapes.values() if s.occurrences >= MIN_OCCURRENCES]
    kept = [
        s
        for s in recurring
        if not any(
            len(o.steps) > len(s.steps)
            and o.occurrences >= s.occurrences
            and _contains(s.steps, o.steps)
            for o in recurring
        )
    ]
    kept.sort(
        key=lambda s: (-s.replay_tokens, -s.turns_eliminated, -len(s.steps), s.steps)
    )
    return kept


def _human(n: int) -> str:
    if n >= 1_000_000:
        return f"{n / 1_000_000:.1f}M"
    if n >= 1_000:
        return f"{n / 1_000:.1f}k"
    return str(n)


def table(shapes: list[FlowShape], top_n: int | None = TOP_N) -> list[str]:
    """The Markdown table, the first ``top_n`` rows (every row on ``None``),
    with an omitted-count line when truncated."""
    shown = shapes if top_n is None else shapes[:top_n]
    out = [
        "| flow shape | runs | turns | a tool leaves | turns saved | ≈replay saved |",
        "|---|--:|--:|--:|--:|--:|",
    ]
    for s in shown:
        steps = " → ".join(f"`{step}`" for step in s.steps)
        out.append(
            f"| {steps} | {s.occurrences} | {s.turns} | {s.occurrences} | "
            f"{s.turns_eliminated} | {_human(s.replay_tokens)} |"
        )
    omitted = len(shapes) - len(shown)
    if omitted > 0:
        out += [
            "",
            f"_+{omitted} more flow shape(s) omitted; `--flows-all` lists them._",
        ]
    return out


def digest_section(
    sessions: list[list[list[Step]]], top_n: int | None = TOP_N
) -> list[str]:
    """The digest's ``## Flows`` body: the day's sessions ranked together,
    each row carrying a stable ``flow:<slug>`` fingerprint for the fold
    analyst. Empty when nothing recurred."""
    shapes = rank(sessions)
    if not shapes:
        return ["No flow shape recurred."]
    out = table(shapes, top_n)
    shown = shapes if top_n is None else shapes[:top_n]
    out.append("")
    for s in shown:
        out.append(
            f"- `{fingerprint(s)}`: {s.turns_eliminated} turn(s) over "
            f"{s.occurrences} run(s) in {len(s.sessions)} session(s)"
        )
    return out


def fingerprint(shape: FlowShape) -> str:
    """``flow:<first>-to-<last>-<n>``: dotless, lowercase, stable per shape."""
    first, last = shape.steps[0], shape.steps[-1]
    return f"flow:{_slug(first)}-to-{_slug(last)}-{len(shape.steps)}"


def _slug(text: str) -> str:
    return _NON_SLUG.sub("-", text.lower()).strip("-") or "step"


_NON_SLUG = re.compile(r"[^a-z0-9]+")
