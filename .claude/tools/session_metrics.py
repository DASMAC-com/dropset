#!/usr/bin/env python3
"""``session-metrics`` (`.claude/tools/session_metrics.py`) — account for where
a Claude Code session spent its tokens, so the ``session-metrics`` skill can
recommend concrete trims.

Given a ``--session-id``, the tool resolves the session's on-disk transcript
itself, reads it (and its sub-agent transcripts) in its **own** process — so the
multi-megabyte file never enters the model's context — and prints a compact,
ranked summary: what the session cost, session-wide token totals, how far its
replayed prefix grew, how much of that prefix was resident instruction prose (see
:class:`ResidentLine`), a cache-hit rate, the tools whose results cost the most,
the single largest results, a per-sub-agent rollup, and the repeated command
shapes that are candidates to harden into a tool. Pass ``--json`` for the same
data as JSON.

**Dollars lead, and the report splits by substrate.** A Bedrock worker session
is metered per token, so its headline is a dollar figure computed from its own
transcript at the verified rates, with the prefix growth beside it — together
they make the quadratic visible, since every turn replays the whole prefix and a
session's bill is roughly its average prefix times its turn count. A **seat**
session bills against the Claude subscription, whose internal pricing is not
transparent, so it reports a token profile and deliberately **no** dollar
figure. :func:`resolve_substrate` decides which, from the marker a launch
writes, and fails toward the seat branch — a missing figure is a visible gap,
while a wrong one silently corrupts the daily reconciliation.

Per-session cost comes from the transcript rather than from AWS on purpose: Cost
Explorer has no session dimension and lags up to a day, so AWS is the
reconciliation path, not the source.

The hardening table is ranked by **result size, not call count**, and labels each
candidate with which cost it actually represents — ``context``, ``wall-clock``,
or ``prompt-churn``. Both were reporting defects that recurred across many
sessions: count-ranking put ``grep`` on top five times running while it was
negligible by size (and hoisting a shape *converts many small calls into a few
large ones*, so count-ranking flags the fix as the problem), and a command routed
through ``run_quiet.py`` costs wall-clock rather than tokens — which three
sessions had to disclaim by hand so the candidate wasn't mis-filed as a context
sink. See :class:`HardeningCandidate`.

Nothing about the host is hard-coded. The Claude home is read from
``CLAUDE_CONFIG_DIR`` (falling back to ``~/.claude``), and the per-project
transcript directory is derived from the working directory the same way Claude
Code slugs it — with a scan of every project directory as a fallback, so a
worktree whose slug doesn't match still resolves by session id.

Stdlib only — no third-party dependencies. This is a Python skill-tool under
``.claude/tools/``; it is deliberately **not** a Cargo workspace member (see
``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

# Rough bytes-per-token divisor for approximating a result's token cost from its
# serialized length. Labelled approximate wherever it surfaces.
BYTES_PER_TOKEN = 4

# How many rows the ranked tables keep. The summary is meant to stay a few
# hundred tokens, so the long tail is dropped (and noted when it is).
TOP_N = 8

# Maximum label width before truncation.
LABEL_WIDTH = 56

# A repeated Bash shape is surfaced as a hardening candidate only once it recurs
# at least this many times within the session — a one-off isn't worth a tool.
HARDENING_MIN_COUNT = 2

# How many hardening candidates the summary lists, longest tail dropped.
HARDENING_TOP_N = 8

# Average result bytes per call, above which a shape's cost is genuinely
# **context** (a payload replayed on every later turn) rather than wall-clock or
# permission churn. A `run_quiet.py` success line is ~100-200 bytes and a
# `printenv` answer is a few dozen, so a few hundred separates "this result is
# information" from "this result is an acknowledgement".
CONTEXT_MIN_AVG_BYTES = 400

# The wrapper that routes a verbose command's output to a log instead of into
# context. A shape that goes through it has already had its context cost removed,
# so whatever remains is wall-clock — and saying so is the point: three sessions
# had to hand-annotate this, because a count-ranked table listed `make lint` ×10
# as if it were a token sink when it cost ~20 tokens.
RUN_QUIET_MARKER = "run_quiet.py"

# How an invoked skill's entry file reaches the transcript: an `isMeta` user
# record whose text opens with this line, naming the skill's directory.
SKILL_BODY_PREFIX = "Base directory for this skill: "

# The bytes-per-token band the skill-size gate's byte caps are sound inside. It
# caps in bytes on the `BYTES_PER_TOKEN` proxy, so a calibrated ratio outside
# this band is the signal to revisit the cap, not just a reporting curiosity.
GATE_BYTES_PER_TOKEN_BAND = (3.5, 4.5)

# A skill injection is a calibration sample only when it is at least this share
# of every user-side byte between its two requests, so the prefix delta across
# it is mostly the injection's own tokens rather than a tool result's.
CALIBRATION_MIN_SHARE = 0.8

# The share of all input, in token-turns, that resident instruction prose must
# clear before the report flags it as a trim lever. The 09-15 reference session
# that motivated the line sat near 19%; a tenth is where it stops being noise.
RESIDENT_LEVER_SHARE = 0.10


@dataclass(frozen=True)
class Rates:
    """Bedrock per-million-token rates for one model, in US dollars.

    ``verified`` is the date pricing a session's own transcript at these rates
    was hand-checked against the real bill and agreed, or ``None`` for a rate
    that is only projected. A verification is a single end-to-end agreement,
    not a measured error bound — do not read it as one.

    One-hour cache writes are the only write tier (this fleet's sessions run
    with the 1h TTL), so there is deliberately no 5-minute rate: adding one
    would invite pricing a write at a tier the session did not use.
    """

    input: float
    output: float
    cache_read: float
    cache_write_1h: float
    verified: str | None


# Keyed by ``message.model`` exactly as the transcript records it, so each
# message is priced at its own model's rate and a session that switched models
# mid-way prices both halves correctly. A Bedrock session records the plain
# first-party name (see `resolve_substrate`), which is why the keys carry no
# region or inference-profile prefix.
#
# A model missing here REFUSES to price — the report withholds every dollar
# figure and names the model — rather than falling back to some default row,
# because a wrong figure silently corrupts the daily Cost Explorer
# reconciliation while a missing one is a visible gap. The dates are named so
# that reconciliation knows exactly what to re-check: a rate change, a new
# billed model, or an unexplained line shows up as drift between the billed day
# and the sum of the fleet's transcript-priced estimates.
#
# Opus 5.5 is seeded at 1.10x the first-party list price, the Bedrock premium
# Opus 5 carried. The public Price List API carries no current Anthropic model,
# so it cannot be read programmatically; it stays unverified until the first
# Opus 5.5 fleet day is reconciled against Cost Explorer by usage type.
RATES_BY_MODEL: dict[str, Rates] = {
    "claude-opus-5": Rates(5.50, 27.50, 0.55, 11.00, verified="2026-09-11"),
    "claude-opus-5-5": Rates(4.40, 22.00, 0.22, 8.80, verified=None),
    # The background tier. Same derivation as the Opus 5.5 row from the
    # first-party list price ($0.10 / $0.50, cache read 0.1x and 1h write 2x
    # input), at the up-to-100K-prompt tier, which background calls stay in.
    "claude-haiku-5-5": Rates(0.11, 0.55, 0.011, 0.22, verified=None),
}

# The key a message with no recorded model is accumulated under. It is never
# in `RATES_BY_MODEL`, so billable tokens under it refuse to price by name.
UNRECORDED_MODEL = "<unrecorded>"

# Where a launch records the substrate it started on, relative to the base repo.
# Written by `_ds_substrate_write` in `.claude/shell/init.zsh` and read back by
# the resume verbs; git-ignored, so it is per-machine state, not history.
SUBSTRATE_DIR = Path(".claude") / "session-substrate"

# The path segment that separates a worktree checkout from its base repo, as
# `claude --worktree` lays them out: `<base>/.claude/worktrees/<tag>`.
WORKTREE_SEGMENT = "/.claude/worktrees/"

SUBSTRATE_BEDROCK = "bedrock"
SUBSTRATE_SEAT = "seat"
# What a launch now writes for the subscription substrate. `seat` is the retired
# spelling, still on disk in older markers, so both read as the seat branch.
SUBSTRATE_ANTHROPIC = "anthropic"


# --------------------------------------------------------------------------- #
# Token-cost aggregation (mirrors the former Rust `model.rs`).
# --------------------------------------------------------------------------- #


@dataclass
class Tokens:
    """The four billed token tiers of one model's share of a session."""

    input: int = 0
    output: int = 0
    cache_creation: int = 0
    cache_read: int = 0

    def add(self, usage: dict) -> None:
        self.input += int(usage.get("input_tokens", 0) or 0)
        self.output += int(usage.get("output_tokens", 0) or 0)
        self.cache_creation += int(usage.get("cache_creation_input_tokens", 0) or 0)
        self.cache_read += int(usage.get("cache_read_input_tokens", 0) or 0)

    def billable(self) -> bool:
        """Whether any tier is non-zero. A zero-usage message — Claude Code
        records a failed request as a ``<synthetic>`` model with an all-zero
        usage block — costs nothing, so an unknown model there must not refuse.
        """
        return bool(self.input or self.output or self.cache_creation or self.cache_read)


def _model_of(msg: dict) -> str:
    model = msg.get("model")
    return model if isinstance(model, str) and model else UNRECORDED_MODEL


@dataclass
class Totals:
    """Session-wide token totals, summed across every assistant turn."""

    input: int = 0
    output: int = 0
    cache_creation: int = 0
    cache_read: int = 0
    turns: int = 0
    # The same tokens split by the model each message recorded, which is what
    # gets priced: see `RATES_BY_MODEL`.
    by_model: dict[str, Tokens] = field(default_factory=dict)
    # The prefix carried by the first and last billed request. Every turn
    # re-sends the whole conversation, so this pair is the growth curve that
    # makes the quadratic visible: a session's bill is roughly the average
    # prefix times the turn count, which is why a fat early payload is paid many
    # times over and why session length is itself the largest cost lever.
    prefix_first: int = 0
    prefix_last: int = 0
    prefix_max: int = 0

    def add(self, usage: dict, model: str) -> None:
        self.by_model.setdefault(model, Tokens()).add(usage)
        fresh = int(usage.get("input_tokens", 0) or 0)
        written = int(usage.get("cache_creation_input_tokens", 0) or 0)
        read = int(usage.get("cache_read_input_tokens", 0) or 0)
        self.input += fresh
        self.output += int(usage.get("output_tokens", 0) or 0)
        self.cache_creation += written
        self.cache_read += read
        # This request's prefix: everything the model had to process as input,
        # whether it came fresh, from a cache write, or from a cache read.
        prefix = fresh + written + read
        # An all-zero usage block (an interrupted or errored message) moves
        # NEITHER bound. The two ends fail differently and both badly: pinned as
        # `prefix_first` it reports the whole of the first real prefix as growth,
        # and pinned as `prefix_last` it renders a large negative shrink — which,
        # because the growth line has a `−` branch, reads as a plausible
        # compaction rather than as an error. The turn still counts, and its
        # tokens are still summed above; only the prefix bounds skip it.
        if prefix:
            if not self.prefix_first:
                self.prefix_first = prefix
            self.prefix_last = prefix
            self.prefix_max = max(self.prefix_max, prefix)
        self.turns += 1

    def total_input(self) -> int:
        """Total input the model processed: fresh input plus both cache tiers."""
        return self.input + self.cache_creation + self.cache_read

    def prefix_growth(self) -> int:
        """How much the replayed prefix grew from the first request to the last."""
        return self.prefix_last - self.prefix_first


class UnpricedModel(KeyError):
    """A model carried billable tokens and has no row in `RATES_BY_MODEL`."""

    def __init__(self, model: str) -> None:
        super().__init__(model)
        self.model = model


@dataclass
class Cost:
    """A dollar breakdown of one token profile, at each model's Bedrock rates.

    **Only ever rendered in the Markdown headline for a Bedrock session** — the
    figures are always present in ``--json``, for tooling that wants them, and a
    test pins them there on the seat branch too. It is the rendered report, not
    the data, that withholds a seat session's cost. A seat session bills against
    the Claude subscription, whose internal pricing is not transparent, so
    pricing its tokens at worker rates would invent a number that appears
    nowhere on any bill. :func:`resolve_substrate` decides which branch applies
    and fails toward the seat one, because a wrong dollar figure is a worse
    error than a missing one.
    """

    input: float = 0.0
    output: float = 0.0
    cache_read: float = 0.0
    cache_write: float = 0.0

    @classmethod
    def price(cls, tokens: Tokens | Totals | SubAgentLine, model: str) -> Cost:
        """Price a token profile at ``model``'s rates. Takes anything carrying
        the four token fields; raises :class:`UnpricedModel` for a model with
        no row in `RATES_BY_MODEL` rather than pricing it at some default.
        """
        rates = RATES_BY_MODEL.get(model)
        if rates is None:
            raise UnpricedModel(model)
        per_mtok = 1_000_000.0
        return cls(
            input=tokens.input / per_mtok * rates.input,
            output=tokens.output / per_mtok * rates.output,
            cache_read=tokens.cache_read / per_mtok * rates.cache_read,
            cache_write=tokens.cache_creation / per_mtok * rates.cache_write_1h,
        )

    @classmethod
    def price_by_model(cls, by_model: dict[str, Tokens]) -> Cost:
        """Price each model's share at its own rates and sum. A share with no
        billable tokens is skipped, so a zero-usage ``<synthetic>`` record never
        trips the refusal.
        """
        cost = cls()
        for model, tokens in by_model.items():
            if tokens.billable():
                cost = cost.plus(cls.price(tokens, model))
        return cost

    def total(self) -> float:
        return self.input + self.output + self.cache_read + self.cache_write

    def plus(self, other: Cost) -> Cost:
        return Cost(
            input=self.input + other.input,
            output=self.output + other.output,
            cache_read=self.cache_read + other.cache_read,
            cache_write=self.cache_write + other.cache_write,
        )


@dataclass
class ToolLine:
    """Per-tool rollup: call count and total result bytes contributed."""

    name: str
    calls: int = 0
    result_bytes: int = 0


@dataclass
class SinkLine:
    """A single largest-result entry with a short label drawn from the input."""

    name: str
    label: str
    bytes: int


@dataclass
class SubAgentLine:
    """Per-sub-agent token rollup, summed from that agent's own transcript."""

    agent: str
    turns: int = 0
    input: int = 0
    output: int = 0
    cache_creation: int = 0
    cache_read: int = 0
    # A sub-agent can run on a different model from its parent, so it is priced
    # from its own split, never the parent's.
    by_model: dict[str, Tokens] = field(default_factory=dict)

    def total_input(self) -> int:
        return self.input + self.cache_creation + self.cache_read


@dataclass
class ResidentLine:
    """One piece of instruction prose resident in the replayed prefix: an
    invoked skill's entry file, an instructions file, or a skills listing.

    **Instruction prose is not a tool result**, so the tool and sink tables can
    never see it — yet every request after its injection replays it, which is
    exactly the quadratic the prefix line describes. ``turns`` counts every
    turn after the injection to the end of the session, on the same
    ``Totals.turns`` count the totals line reports (an all-zero usage record
    included). A compaction drops the old copy, so across one this is an upper
    bound.
    """

    kind: str
    label: str
    bytes: int
    # Requests already billed when it was injected; `finish` turns it into
    # `turns`.
    injected_after: int = 0
    turns: int = 0

    def byte_turns(self) -> int:
        return self.bytes * self.turns


@dataclass
class ResidentProse:
    """The resident-instruction-prose line: what the prefix carried in skill
    entry files, instructions files and skill listings, times the turns that
    replayed it.

    ``bytes_per_token`` is calibrated from the session's own skill injections
    when any dominated its turn (``samples``), else it is the
    :data:`BYTES_PER_TOKEN` proxy. ``cost`` prices the token-turns at the
    cache-read rate of the main session's dominant model, and is ``None`` when
    any billable model is unpriced, the same refusal the headline makes.
    """

    lines: list[ResidentLine]
    omitted: int
    bytes_per_token: float
    samples: int
    token_turns: int
    by_kind: dict[str, int]
    share: float
    cost: float | None

    def ratio_in_band(self) -> bool:
        low, high = GATE_BYTES_PER_TOKEN_BAND
        return low <= self.bytes_per_token <= high

    def is_lever(self) -> bool:
        return self.share >= RESIDENT_LEVER_SHARE


def _load_allowlist() -> list[str]:
    """The shared allowlist's rules, or ``[]`` when it cannot be read.

    **This makes the report machine-dependent, deliberately.** Two operators
    mining the same transcript can get different `cost_kind` labels, because
    coverage is a fact about *this* machine's `settings.local.json` and churn is
    a fact about what actually prompted here — which is the question the label
    is answering. Worth knowing when comparing two runs' tables, and worth
    pinning in tests: `test_session_metrics.py` replaces this function so the
    suite never reads operator config.

    Best-effort by design. This only ever *downgrades* a churn claim, so an
    unreadable allowlist reports exactly what this table reported before the
    coverage check existed — the safe direction, and the reason no failure here
    is worth surfacing to the caller.
    """
    try:
        import allowlist

        return allowlist.load_allow(allowlist.resolve_settings_path(None))
    except Exception:
        return []


def _is_allowlisted(signature: str, allow: list[str]) -> bool:
    """Whether the shared allowlist already covers this command shape.

    A false negative is harmless — the shape reports `prompt-churn`, which is
    what it reported before — and `allowlist.covers` is known to miss some
    mid-token globs, so this deliberately fails toward the old behavior rather
    than asserting coverage it cannot prove.
    """
    if not allow or not signature:
        return False
    try:
        import allowlist

        return bool(allowlist.covers(f"Bash({signature}:*)", allow).get("covered"))
    except Exception:
        return False


@dataclass
class HardeningCandidate:
    """A repeated Bash command shape worth porting to a tool.

    Carries ``result_bytes`` because **that**, not ``count``, is what the table is
    ranked on. Ranking by count was actively misleading: ``grep`` topped the
    table by count in five consecutive sessions (×26 / 29 / 63 / 54 / 50) while
    being negligible by size — one session's largest grep result was ~516 bytes —
    and, worse, hoisting a shape *converts many small calls into a few larger
    ones*, so a count-ranked table flags the fix as the new problem.
    """

    signature: str
    count: int
    deterministic: bool
    result_bytes: int = 0
    # True when the shape routed through `run_quiet.py`, i.e. its output was
    # deliberately kept out of context.
    via_run_quiet: bool = False
    # True when the shared allowlist already covers this shape, so its repeats
    # did NOT re-prompt. Set by the caller, which is the layer that can read
    # `settings.local.json`. Defaults False, so an unresolved allowlist reports
    # exactly what this table reported before — the safe direction.
    allowlisted: bool = False

    def avg_bytes(self) -> int:
        return 0 if self.count == 0 else self.result_bytes // self.count

    def cost_kind(self) -> str:
        """Which cost this candidate actually represents.

        Three distinct answers, because conflating them is what made the old table
        unreadable:

        * ``context`` — the result is large, so it is a genuine token sink,
          replayed as input on every later turn.
        * ``context (failures)`` — large *and* quiet-runner wrapped, so the bytes
          are failure tails rather than un-wrapped output. Reported apart from
          plain ``context`` because the lever is different: the wrapping is
          already in place and what costs tokens is how often the command
          failed, so the fix is fewer round trips, not more redirection. A
          session that read this as plain ``context`` concluded the wrapper
          wasn't working and filed a defect against it — the classification was
          right and only the label was ambiguous.
        * ``wall-clock`` — routed through the quiet runner and quiet in practice,
          so its output never entered context; what it costs is *time*, and
          hardening it further buys latency, not tokens.
        * ``prompt-churn`` — cheap and fast, but repeated in slightly different
          shapes, so each variant is a fresh permission prompt. A `printenv` is
          the type case: worth a tool, but not because of tokens.
        * ``covered (no churn)`` — the same shape, except the shared allowlist
          already covers it, so the repeats did **not** re-prompt and there is
          no friction to remove.

        That last one exists because the heuristic cannot see a prompt. It
        infers churn from *many cheap, slightly-varying calls*, which is also
        what a fully-covered shape looks like — and one filed lever argued for
        a whole new tool on that basis before its own author checked coverage
        and withdrew the reasoning. Consulting the allowlist is what stops the
        table making that argument again.

        Size is checked before wrapping, so a quiet-runner command that *did*
        return big failure tails is never reported as merely a latency cost.
        """
        if self.avg_bytes() >= CONTEXT_MIN_AVG_BYTES:
            return "context (failures)" if self.via_run_quiet else "context"
        if self.via_run_quiet:
            return "wall-clock"
        # Coverage is checked LAST, so it can only ever downgrade a churn claim
        # — never mask a real token sink. A covered shape that returns large
        # results is still `context`, because the cost there is the bytes and
        # has nothing to do with prompting.
        return "covered (no churn)" if self.allowlisted else "prompt-churn"


@dataclass
class _ToolCall:
    """A pending tool call awaiting its result, keyed by tool_use_id."""

    name: str
    label: str


@dataclass
class _BashShape:
    """Per-signature accumulator: how often, how many bytes, and how it was run."""

    count: int = 0
    result_bytes: int = 0
    via_run_quiet: bool = False


@dataclass
class _SubAgentAcc:
    turns: int = 0
    input: int = 0
    output: int = 0
    cache_creation: int = 0
    cache_read: int = 0
    by_model: dict[str, Tokens] = field(default_factory=dict)


class SessionAggregator:
    """Streaming accumulator: feed it transcript lines one at a time (so the
    process never holds the whole file), then :meth:`finish` into a report dict.
    """

    def __init__(self) -> None:
        self.totals = Totals()
        self._pending: dict[str, _ToolCall] = {}
        self._by_tool: dict[str, ToolLine] = {}
        self._sinks: list[SinkLine] = []
        self._subagents: dict[str, _SubAgentAcc] = {}
        # Message ids whose usage was already counted, so the
        # one-record-per-content-block split (which repeats usage) is summed
        # once per logical message. Shared across the main and sub-agent
        # transcripts — `msg_…` ids are globally unique.
        self._counted_messages: set[str] = set()
        self._bash_shapes: dict[str, _BashShape] = {}
        # tool_use ids whose Bash signature was already counted. The content
        # array is re-walked on every content-block record of a split message
        # (tool_use items can repeat), so count the signature once per id —
        # otherwise a split message inflates a command's hardening count.
        self._counted_bash_ids: set[str] = set()
        # tool_use id -> signature, so a Bash result's size can be attributed back
        # to the shape that produced it. Result bytes only become known at
        # tool_result time, one or more records after the tool_use.
        self._bash_sig_by_id: dict[str, str] = {}
        # The working directory the session ran in, taken from the first record
        # that carries one. This is what the substrate lookup keys off, and
        # reading it from the transcript rather than from `Path.cwd()` is what
        # lets a session be mined correctly from somewhere else entirely.
        self.cwd: str | None = None
        self.parse_errors = 0
        self._resident: list[ResidentLine] = []
        # Calibration state for the bytes-per-token proxy. Each new request's
        # prefix minus the previous request's prefix and output is what the
        # user-side records between them cost in real tokens; when a skill
        # injection dominates those bytes, the pair is a sample. See
        # `_close_request`.
        self._last_request_end: int | None = None
        self._bytes_since_request = 0
        self._injected_since_request = 0
        self._calibration_bytes = 0
        self._calibration_tokens = 0
        self._calibration_samples = 0

    # -- ingestion -------------------------------------------------------- #

    def ingest_main_line(self, line: str) -> None:
        """Ingest one line of the main session transcript."""
        if not line.strip():
            return
        try:
            rec = json.loads(line)
        except (json.JSONDecodeError, ValueError):
            self.parse_errors += 1
            return
        self._ingest_main_record(rec)

    def ingest_subagent_line(self, agent: str, line: str) -> None:
        """Ingest one line of a sub-agent transcript, attributed to ``agent``."""
        if not line.strip():
            return
        try:
            rec = json.loads(line)
        except (json.JSONDecodeError, ValueError):
            self.parse_errors += 1
            return
        msg = rec.get("message") if isinstance(rec, dict) else None
        if not isinstance(msg, dict):
            return
        usage = msg.get("usage")
        if not isinstance(usage, dict):
            return
        # Count each message's usage once, even though the split repeats it
        # across the message's records.
        if not self._first_usage_sighting(msg.get("id")):
            return
        acc = self._subagents.setdefault(agent, _SubAgentAcc())
        acc.turns += 1
        acc.input += int(usage.get("input_tokens", 0) or 0)
        acc.output += int(usage.get("output_tokens", 0) or 0)
        acc.cache_creation += int(usage.get("cache_creation_input_tokens", 0) or 0)
        acc.cache_read += int(usage.get("cache_read_input_tokens", 0) or 0)
        acc.by_model.setdefault(_model_of(msg), Tokens()).add(usage)

    def _first_usage_sighting(self, msg_id) -> bool:
        """Whether this message's usage has not yet been counted. A message
        without an id can't be deduped, so it always counts (the common path
        always carries one).
        """
        if not isinstance(msg_id, str) or not msg_id:
            return True
        if msg_id in self._counted_messages:
            return False
        self._counted_messages.add(msg_id)
        return True

    def _ingest_main_record(self, rec) -> None:
        if not isinstance(rec, dict):
            return
        if self.cwd is None:
            cwd = rec.get("cwd")
            if isinstance(cwd, str) and cwd.strip():
                self.cwd = cwd.strip()
        attachment = rec.get("attachment")
        if isinstance(attachment, dict):
            self._ingest_attachment(rec, attachment)
        msg = rec.get("message")
        if not isinstance(msg, dict):
            return
        usage = msg.get("usage")
        if isinstance(usage, dict):
            # Sum usage once per logical message, not once per content-block
            # record (which repeats the same usage).
            if self._first_usage_sighting(msg.get("id")):
                self.totals.add(usage, _model_of(msg))
                self._close_request(usage)
        content = msg.get("content")
        if rec.get("type") == "user" and content is not None:
            self._bytes_since_request += prose_len(content)
            if rec.get("isMeta"):
                self._ingest_skill_body(content)
        # The content array is walked on *every* record (tool_use items are
        # idempotent in `pending`; tool_results live in separate user records),
        # so attribution is unaffected by the per-message split.
        if not isinstance(content, list):
            return
        for item in content:
            if isinstance(item, dict):
                self._ingest_content_item(item)

    def _ingest_attachment(self, rec: dict, attachment: dict) -> None:
        """Record the instruction prose an attachment makes resident: the
        instructions files (the project file and the memory index) and the
        skills listing, initial or a later delta. Every attachment's rendered
        text also counts toward the bytes between two requests.
        """
        rendered_list = rec.get("rendered")
        for rendered in rendered_list if isinstance(rendered_list, list) else []:
            if isinstance(rendered, dict):
                self._bytes_since_request += value_len(rendered.get("content", ""))
        kind = attachment.get("type")
        files = attachment.get("files")
        if kind == "instructions" and isinstance(files, list):
            for f in files:
                if not isinstance(f, dict) or not isinstance(f.get("content"), str):
                    continue
                path = f.get("path")
                label = Path(path).name if isinstance(path, str) else "instructions"
                if isinstance(f.get("type"), str):
                    label = f"{label} ({f['type']})"
                self._add_resident("instructions", label, value_len(f["content"]))
        elif kind == "skill_listing":
            content = attachment.get("content")
            if isinstance(content, str):
                label = "initial" if attachment.get("isInitial") else "delta"
                self._add_resident("skill-listing", label, value_len(content))

    def _ingest_skill_body(self, content) -> None:
        """Record an invoked skill's entry file, which arrives as an ``isMeta``
        user record opening with :data:`SKILL_BODY_PREFIX`. The injected text,
        not the file on disk, is what the prefix carried, so it is what's
        measured: it survives the worktree being pruned, and it is exactly
        what the prefix carried, header line and any uncommitted edits
        included.
        """
        if isinstance(content, list):
            texts = [
                i.get("text")
                for i in content
                if isinstance(i, dict) and isinstance(i.get("text"), str)
            ]
            text = texts[0] if texts else ""
        else:
            text = content if isinstance(content, str) else ""
        if not text.startswith(SKILL_BODY_PREFIX):
            return
        skill_dir = text[len(SKILL_BODY_PREFIX) :].split("\n", 1)[0].strip()
        size = value_len(text)
        self._add_resident("skill", Path(skill_dir).name or "skill", size)
        self._injected_since_request += size

    def _add_resident(self, kind: str, label: str, size: int) -> None:
        self._resident.append(
            ResidentLine(
                kind=kind, label=label, bytes=size, injected_after=self.totals.turns
            )
        )

    def _close_request(self, usage: dict) -> None:
        """Take a calibration sample if the gap this request closes was
        dominated by a skill injection, then start the next gap.

        The new prefix less the last request's prefix and output is the real
        token cost of every user-side record in between. An all-zero usage
        block carries no prefix, so like the prefix bounds it is skipped.

        Two biases remain, and the dominance test, comparing bytes with bytes,
        sees neither. Attachments recorded without rendered text (mostly
        one-line hook acknowledgements) add tokens but no bytes. And if the
        last request's thinking is not replayed, subtracting all of its output
        understates the delta. Both are small next to a skill body: 93 output
        tokens against 102.7k on the first measured `review-pr` sample.
        """
        prefix = (
            int(usage.get("input_tokens", 0) or 0)
            + int(usage.get("cache_creation_input_tokens", 0) or 0)
            + int(usage.get("cache_read_input_tokens", 0) or 0)
        )
        if not prefix:
            return
        between = self._bytes_since_request
        injected = self._injected_since_request
        if (
            self._last_request_end is not None
            and injected
            and injected >= CALIBRATION_MIN_SHARE * between
        ):
            delta = prefix - self._last_request_end
            # A shrink is a compaction, not a measurement.
            if delta > 0:
                self._calibration_bytes += between
                self._calibration_tokens += delta
                self._calibration_samples += 1
        self._last_request_end = prefix + int(usage.get("output_tokens", 0) or 0)
        self._bytes_since_request = 0
        self._injected_since_request = 0

    def _ingest_content_item(self, item: dict) -> None:
        kind = item.get("type")
        if kind == "tool_use":
            tid = item.get("id")
            name = item.get("name")
            if not isinstance(tid, str) or not isinstance(name, str):
                return
            label = tool_label(name, item.get("input"))
            self._pending[tid] = _ToolCall(name=name, label=label)
            if name == "Bash" and tid not in self._counted_bash_ids:
                self._counted_bash_ids.add(tid)
                self._record_bash_signature(tid, item.get("input"))
        elif kind == "tool_result":
            tid = item.get("tool_use_id")
            if not isinstance(tid, str):
                return
            content = item.get("content")
            byte_len = value_len(content) if content is not None else 0
            call = self._pending.pop(tid, None)
            if call is not None:
                name, label = call.name, call.label
            else:
                name, label = "unknown", ""
            entry = self._by_tool.get(name)
            if entry is None:
                entry = ToolLine(name=name)
                self._by_tool[name] = entry
            entry.calls += 1
            entry.result_bytes += byte_len
            self._sinks.append(SinkLine(name=name, label=label, bytes=byte_len))
            # Attribute a Bash result's size back to its command shape, so the
            # hardening table can rank by bytes rather than by call count.
            sig = self._bash_sig_by_id.pop(tid, None)
            if sig is not None:
                self._bash_shapes[sig].result_bytes += byte_len

    def _record_bash_signature(self, tool_use_id: str, input_obj) -> None:
        if not isinstance(input_obj, dict):
            return
        command = input_obj.get("command")
        if not isinstance(command, str):
            return
        sig = bash_signature(command)
        if not sig:
            return
        shape = self._bash_shapes.setdefault(sig, _BashShape())
        shape.count += 1
        # Sticky: once any invocation of a shape went through the quiet runner,
        # the shape is a wall-clock candidate. A single unwrapped call among ten
        # wrapped ones is a slip, not a re-classification — and the large result
        # it produced still shows up through the bytes-based `cost_kind` check.
        if RUN_QUIET_MARKER in command:
            shape.via_run_quiet = True
        self._bash_sig_by_id[tool_use_id] = sig

    # -- finishing -------------------------------------------------------- #

    def finish(self, substrate: str | None = None) -> dict:
        """Rank and truncate into the final report dict.

        ``substrate`` overrides the marker lookup; passing ``None`` (the
        default) resolves it from the session's own recorded working directory.
        """
        total_input = self.totals.total_input()
        cache_hit_rate = (
            0.0 if total_input == 0 else self.totals.cache_read / total_input
        )

        tools = sorted(
            self._by_tool.values(),
            key=lambda t: (-t.result_bytes, t.name),
        )
        tools_omitted = max(0, len(tools) - TOP_N)
        tools = tools[:TOP_N]

        sinks = sorted(self._sinks, key=lambda s: (-s.bytes, s.name))
        sinks_omitted = max(0, len(sinks) - TOP_N)
        sinks = sinks[:TOP_N]

        subagents = [
            SubAgentLine(
                agent=agent,
                turns=acc.turns,
                input=acc.input,
                output=acc.output,
                cache_creation=acc.cache_creation,
                cache_read=acc.cache_read,
                by_model=acc.by_model,
            )
            for agent, acc in self._subagents.items()
        ]
        subagents.sort(key=lambda a: (-a.total_input(), a.agent))

        allow = _load_allowlist()
        candidates = [
            HardeningCandidate(
                signature=sig,
                count=shape.count,
                deterministic=is_deterministic_shape(sig),
                result_bytes=shape.result_bytes,
                via_run_quiet=shape.via_run_quiet,
                allowlisted=_is_allowlisted(sig, allow),
            )
            for sig, shape in self._bash_shapes.items()
            if shape.count >= HARDENING_MIN_COUNT
            # The repo's own tools are already the hardened form — see
            # `is_repo_tool_shape`. Their cost shows up in the sinks table.
            and not is_repo_tool_shape(sig)
        ]
        # **By result bytes, not call count.** See `HardeningCandidate` for why
        # count-ranking misled five consecutive sessions. `deterministic` stays a
        # reported column — it says how *portable* a shape is, which is a separate
        # question from how much it cost.
        candidates.sort(key=lambda c: (-c.result_bytes, -c.count, c.signature))
        candidates_omitted = max(0, len(candidates) - HARDENING_TOP_N)
        candidates = candidates[:HARDENING_TOP_N]

        if substrate is None:
            substrate, substrate_reason = resolve_substrate(self.cwd)
        else:
            substrate_reason = "given explicitly"

        # Sub-agent tokens bill exactly like the main session's, so a fan-out
        # session's cost is mostly theirs — reporting only the main line would
        # understate the very sessions the small-PRs convention targets. They are
        # priced separately as well as summed, so the split stays visible.
        #
        # Every billable model across the main session and its sub-agents is
        # checked BEFORE anything is priced: one unknown model withholds all
        # three figures, since a partial sum would read as the whole bill.
        billable: set[str] = set()
        for by_model in [self.totals.by_model, *(s.by_model for s in subagents)]:
            billable.update(m for m, t in by_model.items() if t.billable())
        unpriced_models = sorted(m for m in billable if m not in RATES_BY_MODEL)
        priced_models = sorted(m for m in billable if m in RATES_BY_MODEL)
        session_cost: Cost | None = None
        subagent_cost: Cost | None = None
        total_cost: Cost | None = None
        if not unpriced_models:
            session_cost = Cost.price_by_model(self.totals.by_model)
            subagent_cost = Cost()
            for line in subagents:
                subagent_cost = subagent_cost.plus(Cost.price_by_model(line.by_model))
            total_cost = session_cost.plus(subagent_cost)

        resident = self._resident_prose(total_input, unpriced_models)

        return {
            "totals": self.totals,
            "substrate": substrate,
            "substrate_reason": substrate_reason,
            "cwd": self.cwd,
            "session_cost": session_cost,
            "subagent_cost": subagent_cost,
            "total_cost": total_cost,
            "priced_models": priced_models,
            "unpriced_models": unpriced_models,
            "cache_hit_rate": cache_hit_rate,
            "tools": tools,
            "top_sinks": sinks,
            "subagents": subagents,
            "hardening_candidates": candidates,
            "resident_prose": resident,
            "parse_errors": self.parse_errors,
            "tools_omitted": tools_omitted,
            "sinks_omitted": sinks_omitted,
            "candidates_omitted": candidates_omitted,
        }

    def _resident_prose(
        self, total_input: int, unpriced_models: list[str]
    ) -> ResidentProse:
        if self._calibration_samples:
            ratio = self._calibration_bytes / self._calibration_tokens
        else:
            ratio = float(BYTES_PER_TOKEN)
        lines = []
        for line in self._resident:
            line.turns = max(0, self.totals.turns - line.injected_after)
            lines.append(line)
        by_kind: dict[str, int] = {}
        for line in lines:
            by_kind[line.kind] = by_kind.get(line.kind, 0) + line.byte_turns()
        by_kind = {k: round(v / ratio) for k, v in by_kind.items()}
        token_turns = sum(by_kind.values())
        share = 0.0 if total_input == 0 else token_turns / total_input

        # Resident prose is replayed, never re-written, so the cache-read rate
        # is the one it pays: priced at the model that read the most.
        cost: float | None = None
        billable = {m: t for m, t in self.totals.by_model.items() if t.billable()}
        if billable and not unpriced_models:
            model = max(billable, key=lambda m: (billable[m].cache_read, m))
            cost = token_turns / 1_000_000.0 * RATES_BY_MODEL[model].cache_read

        lines.sort(key=lambda r: (-r.byte_turns(), r.kind, r.label))
        return ResidentProse(
            lines=lines[:TOP_N],
            omitted=max(0, len(lines) - TOP_N),
            bytes_per_token=ratio,
            samples=self._calibration_samples,
            token_turns=token_turns,
            by_kind=by_kind,
            share=share,
            cost=cost,
        )


# --------------------------------------------------------------------------- #
# Labels and command-shape normalization (mirrors the former Rust helpers).
# --------------------------------------------------------------------------- #


def value_len(v) -> int:
    """Serialized **byte** length of a tool result's ``content`` (UTF-8). A bare
    string is measured directly; any other shape is measured by its JSON
    serialization — an approximation, which is all sink *ranking* needs. Bytes,
    not characters, so the "bytes ÷ 4" token proxy holds for non-ASCII results.
    """
    if isinstance(v, str):
        return len(v.encode("utf-8"))
    try:
        serialized = json.dumps(v, separators=(",", ":"), ensure_ascii=False)
        return len(serialized.encode("utf-8"))
    except (TypeError, ValueError):
        return 0


def prose_len(content) -> int:
    """Byte length of a message's ``content`` as the model reads it: a text
    item by its raw text, anything else by :func:`value_len`. Serializing a
    text item would count each escaped newline twice, which on a prose-heavy
    skill body skews the bytes-per-token calibration it feeds.
    """
    if not isinstance(content, list):
        return value_len(content)
    return sum(
        value_len(item["text"])
        if isinstance(item, dict) and isinstance(item.get("text"), str)
        else value_len(item)
        for item in content
    )


def _pick(input_obj: dict, keys: list[str]):
    for key in keys:
        val = input_obj.get(key)
        if isinstance(val, str):
            return val
    return None


def tool_label(name: str, input_obj) -> str:
    """A short, human-readable label for a tool call, drawn from the field of
    its input that identifies the work: the path for file tools, the command for
    Bash, the method/query for an MCP call. Paths keep their tail (the
    filename); everything else keeps its head (the command verb).
    """
    if not isinstance(input_obj, dict):
        return ""
    if name in ("Read", "Edit", "Write", "NotebookEdit"):
        picked = _pick(input_obj, ["file_path", "notebook_path"])
        return shorten_tail(picked) if picked else ""
    if name == "Bash":
        picked = _pick(input_obj, ["command"])
        return shorten_head(picked) if picked else ""
    if name.startswith("mcp__"):
        # MCP inputs vary; the string-valued fields that best identify the call
        # are the method/query/id. (Numeric fields like `pullNumber` can't be a
        # label.)
        picked = _pick(input_obj, ["method", "query", "id"])
        return shorten_head(picked) if picked else ""
    picked = _pick(
        input_obj,
        ["file_path", "command", "pattern", "query", "url", "description"],
    )
    return shorten_head(picked) if picked else ""


def shorten_head(s: str) -> str:
    """Keep the head of a value, marking truncation with a trailing ellipsis."""
    s = " ".join(s.split())
    if len(s) <= LABEL_WIDTH:
        return s
    return s[: LABEL_WIDTH - 1] + "…"


def shorten_tail(s: str) -> str:
    """Keep the tail of a value (the filename of a path), marking truncation
    with a leading ellipsis.
    """
    if len(s) <= LABEL_WIDTH:
        return s
    return "…" + s[len(s) - (LABEL_WIDTH - 1) :]


# Leading `NAME=value` environment assignments to strip before reading the verb.
_ENV_ASSIGN = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")

# How many stable head tokens (program + subcommands) a signature keeps.
_SIGNATURE_TOKENS = 3

# git subcommands that are deterministic local string/path/metadata logic — the
# kind of step worth hardening into a tool — as opposed to network or build ops.
_GIT_LOCAL_SUBCOMMANDS = {
    "worktree",
    "branch",
    "rev-parse",
    "symbolic-ref",
    "config",
    "ls-files",
    "status",
    "show",
    "log",
    "diff",
    "describe",
}

# Programs whose every invocation is deterministic string/env logic.
_DETERMINISTIC_PROGRAMS = {"printenv", "basename", "dirname"}

# Script interpreters, whose *script* names the shape rather than the
# interpreter — otherwise every repo tool collapses into one shape.
# Matched by regex rather than an enumerated set so a new point release
# (`python3.13`) doesn't silently fall back to the collapsing behavior.
_INTERPRETER_RE = re.compile(r"^(?:python(?:\d+(?:\.\d+)?)?|node|deno|bun)$")

# Script suffixes an interpreter can be given. `node`/`deno`/`bun` were in the
# interpreter set while only `.py` was recognized, so a `node foo.js` call
# collapsed to a bare `node` — the exact bug this naming exists to fix, left
# standing for the non-Python entries.
_SCRIPT_SUFFIXES = (".py", ".js", ".mjs", ".cjs", ".ts")


def is_repo_tool_shape(signature: str) -> bool:
    """Whether a signature names one of the repo's own committed skill-tools.

    These are excluded from the hardening table because they are **already the
    hardened form**: the tooling convention says a settled workflow becomes a
    Python tool under ``.claude/tools/``, and these are the result. Nominating
    them reads as "port this into a tool" about a tool, and it crowded out real
    candidates in three separate sessions. Their *results* can still be a
    genuine token sink — that is what the sinks table is for.
    """
    program = signature.split()[0] if signature.split() else ""
    # `.py` only, deliberately narrower than `_SCRIPT_SUFFIXES`. That set
    # exists so every script is *named* by its script; this exclusion is the
    # different question of whether the script is one of the repo's own
    # `.claude/tools/` Python tools. A `node …/build.mjs` should be named
    # after its script AND still be eligible as a hardening candidate.
    return program.endswith(".py")


def _is_subcommand_word(tok: str) -> bool:
    """A stable subcommand word: lowercase ASCII letters and hyphens only, no
    digits — so ``worktree`` / ``list`` / ``pull`` qualify but a specific arg
    (``worktree-eng-1``, ``LINEAR_TEAM_ID``, ``main..HEAD``) does not.
    """
    stripped = tok.replace("-", "")
    return (
        bool(stripped) and stripped.isascii() and stripped.isalpha() and tok.islower()
    )


def _unwrap_run_quiet(tokens: list[str]) -> list[str]:
    """Drop a ``… run_quiet.py --`` prefix, returning the wrapped command.

    Conservative on purpose: it unwraps only when the marker is followed by an
    explicit ``--`` separator, which is the documented invocation. A
    ``run_quiet.py`` appearing as an *argument* to something else (a `git add`
    of the tool, say) has no such separator and is left alone.
    """
    if not any(RUN_QUIET_MARKER in tok for tok in tokens):
        return tokens
    try:
        marker = next(i for i, tok in enumerate(tokens) if RUN_QUIET_MARKER in tok)
        sep = tokens.index("--", marker)
    except (StopIteration, ValueError):
        return tokens
    return tokens[sep + 1 :]


def bash_signature(command: str) -> str:
    """Normalize a Bash command to a stable shape for grouping repeats.

    Strips leading ``NAME=value`` env assignments, then keeps the program name
    and the leading run of subcommand words, skipping flags and path-like
    tokens (a flag value or a ``-C`` target) and stopping at the first concrete
    argument (an uppercase name, a token with digits, a quoted value). So
    ``git worktree list --porcelain`` → ``git worktree list``,
    ``git -C /repo pull --ff-only`` → ``git pull``,
    ``git branch -m worktree-eng-1 eng-1`` → ``git branch``, and
    ``printenv LINEAR_TEAM_ID`` → ``printenv``. Returns "" for an empty command.

    A ``run_quiet.py --`` wrapper is **unwrapped** first, so the shape is the
    command that actually ran: ``python3 .claude/tools/run_quiet.py -- make
    lint`` → ``make lint``. Without that, the normalizer fused the wrapper with
    its payload and emitted nonsense like ``python3 make lint`` /
    ``python3 pnpm frontend`` — the tool path is skipped as a path token and the
    ``--`` as a flag, leaving ``python3`` glued to the wrapped program. Besides
    being unreadable, it grouped unrelated commands together and risked
    nominating the wrapper itself as a hardening candidate. The wall-clock
    ``cost`` label is unaffected: ``via_run_quiet`` is set from the raw command
    text, not from this signature.

    A **script invocation is named by its script**, for the same reason:
    ``python3 .claude/tools/search_source.py 'pat'`` → ``search_source.py``,
    not a bare ``python3``. The interpreter's argument is a path, so the
    generic path-skipping rule would drop it and collapse *every* repo tool
    into one ``python3`` shape — which three separate sessions then reported
    as their top hardening candidate, at ~5k over 19 calls in one and ~10.4k
    over 11 in another. That reads as "harden this" when these already **are**
    the hardened form and the cost lives entirely in their results.
    """
    tokens = command.split()
    # Drop any leading environment assignments (`FOO=bar cmd …`).
    while tokens and _ENV_ASSIGN.match(tokens[0]):
        tokens.pop(0)
    tokens = _unwrap_run_quiet(tokens)
    if not tokens:
        return ""
    if _INTERPRETER_RE.match(tokens[0]):
        module_mode = False
        for i, tok in enumerate(tokens[1:], start=1):
            if tok in ("-m", "--module"):
                module_mode = True
                continue
            if tok.startswith("-"):
                continue  # any other interpreter flag isn't the script
            if module_mode:
                # `python3 -m unittest …` → `unittest`. Without this the head
                # stayed a bare `python3`, so every `-m` invocation collapsed
                # into one shape — the same defect the script case fixes.
                tokens = [tok, *tokens[i + 1 :]]
            elif tok.endswith(_SCRIPT_SUFFIXES):
                tokens = [tok.rsplit("/", 1)[-1], *tokens[i + 1 :]]
            break
    head = [tokens[0]]
    for tok in tokens[1:]:
        if len(head) >= _SIGNATURE_TOKENS:
            break
        if tok.startswith("-"):
            continue  # a flag isn't part of the shape; keep scanning
        if "/" in tok:
            continue  # a path (flag value or target dir); keep scanning
        if _is_subcommand_word(tok):
            head.append(tok)
            continue
        break  # a concrete argument ends the stable head
    return " ".join(head)


def is_deterministic_shape(signature: str) -> bool:
    """Whether a command signature is deterministic string/path/env logic — a
    strong candidate to port into a tool (per ``CLAUDE.md`` → "Skill tooling").
    """
    tokens = signature.split()
    if not tokens:
        return False
    program = tokens[0]
    if program in _DETERMINISTIC_PROGRAMS:
        return True
    if program == "git":
        for tok in tokens[1:]:
            if tok in _GIT_LOCAL_SUBCOMMANDS:
                return True
        return False
    return False


# --------------------------------------------------------------------------- #
# Rendering.
# --------------------------------------------------------------------------- #


def human(n: int) -> str:
    """Format a token count compactly: ``1.2k``, ``3.4M``, or the bare number."""
    if n >= 1_000_000:
        return f"{n / 1_000_000:.1f}M"
    if n >= 1_000:
        return f"{n / 1_000:.1f}k"
    return str(n)


def money(dollars: float) -> str:
    """Format a dollar amount, keeping cents legible for a cheap session."""
    if dollars >= 10.0:
        return f"${dollars:.0f}"
    if dollars >= 1.0:
        return f"${dollars:.2f}"
    return f"${dollars:.3f}"


def _rate_standing(model: str) -> str:
    verified = RATES_BY_MODEL[model].verified
    if verified:
        return f"`{model}` verified {verified}"
    return f"`{model}` projected, unverified"


def _resident_headline(resident: ResidentProse, substrate: str) -> str:
    """The resident-prose line, rendered beside prefix growth. Its dollar
    figure follows the headline's rule: Bedrock only, and never when a model is
    unpriced.
    """
    parts = " · ".join(
        f"{kind} {human(tokens)}"
        for kind, tokens in sorted(resident.by_kind.items(), key=lambda kv: -kv[1])
    )
    line = "**Resident instruction prose**: ≈{} token-turns, {:.0f}% of all input ({})".format(
        human(resident.token_turns), resident.share * 100.0, parts
    )
    if substrate == SUBSTRATE_BEDROCK and resident.cost is not None:
        line += f", about {money(resident.cost)} at the cache-read rate"
    if resident.samples:
        ratio = "{:.2f} bytes/token, calibrated from {} skill injection(s)".format(
            resident.bytes_per_token, resident.samples
        )
    else:
        ratio = f"{BYTES_PER_TOKEN} bytes/token assumed, no injection to calibrate on"
    line += f"; {ratio}\n"
    if not resident.ratio_in_band():
        low, high = GATE_BYTES_PER_TOKEN_BAND
        line += (
            f"**Note**: the measured ratio is outside the {low}–{high} band the "
            "skill-size gate's byte caps assume, so the cap needs revisiting.\n"
        )
    if resident.is_lever():
        line += (
            f"**Lever**: resident prose clears the {RESIDENT_LEVER_SHARE:.0%} "
            "share bar — file it like any other trim lever.\n"
        )
    return line


def to_markdown(report: dict, session_label: str) -> str:
    """Render the compact Markdown summary printed by default."""
    totals: Totals = report["totals"]
    out: list[str] = []
    out.append(f"## Session metrics — {session_label}\n\n")

    # **Dollars first, and only for a Bedrock worker.** The headline is the
    # number the small-PRs convention is trying to drive down, so it leads; a
    # seat session says so instead of being priced at worker rates.
    if report["substrate"] == SUBSTRATE_BEDROCK and report["unpriced_models"]:
        out.append(
            "**Cost withheld**: no Bedrock rate for {} — add a row to "
            "`RATES_BY_MODEL` rather than pricing at another model's rate; "
            "substrate {}.\n".format(
                ", ".join(f"`{m}`" for m in report["unpriced_models"]),
                report["substrate_reason"],
            )
        )
    elif report["substrate"] == SUBSTRATE_BEDROCK:
        total: Cost = report["total_cost"]
        session: Cost = report["session_cost"]
        sub: Cost = report["subagent_cost"]
        out.append(f"**This session cost about {money(total.total())}** ")
        if sub.total() > 0.0:
            out.append(
                "(main {} + sub-agents {}) ".format(
                    money(session.total()), money(sub.total())
                )
            )
        # Name where the substrate came from on this branch too. Without it an
        # operator-asserted `--substrate bedrock` renders byte-identically to a
        # marker-verified one — on precisely the figure the daily
        # reconciliation consumes, where that provenance is the point.
        #
        # Each priced model names its rate's standing, because a projected rate
        # rendered as if verified is exactly the drift the reconciliation hunts.
        out.append(
            "at Bedrock rates — {}; substrate {}.\n".format(
                ", ".join(_rate_standing(m) for m in report["priced_models"])
                or "no billable tokens",
                report["substrate_reason"],
            )
        )
        # **(all agents)** is load-bearing: these four figures price
        # `total_cost`, which includes sub-agents, while the `**Totals**` line
        # just below counts the main session only. Unlabelled, the two adjacent
        # lines invite a dollars-per-token ratio that is wrong on any fan-out.
        out.append(
            "**Cost breakdown** (all agents): cache-read {} · cache-write {} · "
            "output {} · input {}\n".format(
                money(total.cache_read),
                money(total.cache_write),
                money(total.output),
                money(total.input),
            )
        )
    else:
        out.append(
            "**Substrate**: seat — billed to the Claude subscription, whose "
            "internal pricing is not transparent, so this reports a token "
            "profile and no dollar figure ({}).\n".format(report["substrate_reason"])
        )

    out.append(
        "**Totals** (main session): input {} · output {} · cache-write {} · "
        "cache-read {} · {} turns\n".format(
            human(totals.input),
            human(totals.output),
            human(totals.cache_creation),
            human(totals.cache_read),
            totals.turns,
        )
    )
    # The growth curve, which is what makes the quadratic legible: every turn
    # replays the whole prefix, so cost tracks the average prefix times turns.
    out.append(
        "**Prefix**: {} → {} ({}{} across {} turns, peak {})\n".format(
            human(totals.prefix_first),
            human(totals.prefix_last),
            "+" if totals.prefix_growth() >= 0 else "−",
            human(abs(totals.prefix_growth())),
            totals.turns,
            human(totals.prefix_max),
        )
    )
    resident: ResidentProse = report["resident_prose"]
    if resident.token_turns:
        out.append(_resident_headline(resident, report["substrate"]))
    out.append(
        "**Cache-hit rate**: {:.0f}% (cache-read ÷ all input)\n".format(
            report["cache_hit_rate"] * 100.0
        )
    )
    if report["parse_errors"] > 0:
        out.append(
            "**Note**: {} transcript line(s) failed to parse and were skipped.\n".format(
                report["parse_errors"]
            )
        )

    tools: list[ToolLine] = report["tools"]
    if tools:
        out.append("\n### Costliest tools (by result size, ≈tokens = bytes ÷ 4)\n\n")
        out.append("| tool | calls | ≈tokens |\n|---|--:|--:|\n")
        for t in tools:
            out.append(
                f"| {t.name} | {t.calls} | {human(t.result_bytes // BYTES_PER_TOKEN)} |\n"
            )
        if report["tools_omitted"] > 0:
            out.append(f"\n_+{report['tools_omitted']} more tool(s) omitted._\n")

    sinks: list[SinkLine] = report["top_sinks"]
    if sinks:
        out.append("\n### Largest single results (≈tokens)\n\n")
        for i, s in enumerate(sinks):
            label = "" if not s.label else f"  `{s.label}`"
            out.append(
                f"{i + 1}. ≈{human(s.bytes // BYTES_PER_TOKEN)}  {s.name}{label}\n"
            )
        if report["sinks_omitted"] > 0:
            out.append(f"\n_+{report['sinks_omitted']} more result(s) omitted._\n")

    if resident.lines and resident.token_turns:
        out.append("\n### Resident instruction prose (by ≈token-turns)\n\n")
        out.append(
            "| prose | kind | bytes | ≈tokens | turns | ≈token-turns |\n"
            "|---|---|--:|--:|--:|--:|\n"
        )
        for r in resident.lines:
            out.append(
                f"| {r.label} | {r.kind} | {human(r.bytes)} | "
                f"{human(round(r.bytes / resident.bytes_per_token))} | {r.turns} | "
                f"{human(round(r.byte_turns() / resident.bytes_per_token))} |\n"
            )
        if resident.omitted > 0:
            out.append(f"\n_+{resident.omitted} more injection(s) omitted._\n")

    subagents: list[SubAgentLine] = report["subagents"]
    if subagents:
        out.append(f"\n### Sub-agents ({len(subagents)})\n\n")
        out.append("| agent | turns | ≈input | output |\n|---|--:|--:|--:|\n")
        for a in subagents:
            out.append(
                f"| {a.agent} | {a.turns} | {human(a.total_input())} | {human(a.output)} |\n"
            )

    candidates: list[HardeningCandidate] = report["hardening_candidates"]
    if candidates:
        out.append(
            "\n### Hardening candidates (repeated command shapes, by result size)\n\n"
        )
        out.append(
            "| command shape | ≈tokens | calls | cost | deterministic |\n"
            "|---|--:|--:|---|:--:|\n"
        )
        for c in candidates:
            mark = "yes" if c.deterministic else "no"
            out.append(
                f"| `{c.signature}` | {human(c.result_bytes // BYTES_PER_TOKEN)} | "
                f"{c.count} | {c.cost_kind()} | {mark} |\n"
            )
        if report["candidates_omitted"] > 0:
            out.append(f"\n_+{report['candidates_omitted']} more shape(s) omitted._\n")
        out.append(
            "\n_`cost`: **context** = a real token sink; **context (failures)** = "
            "already `run_quiet.py`-wrapped, so the bytes are failure tails — the "
            "lever is fewer failed runs, not more redirection; **wall-clock** = "
            "wrapped and quiet, so hardening it buys latency, not tokens; "
            "**prompt-churn** = cheap and fast, but each variant re-prompts; "
            "**covered (no churn)** = the same shape, but the allowlist "
            "already covers it, so nothing re-prompted and there is no "
            "friction to remove._\n"
        )

    return "".join(out)


def to_json(report: dict) -> str:
    """Serialize the report to pretty JSON (mirrors the former ``--json``)."""

    def encode(obj):
        if isinstance(obj, Totals):
            return {
                "input": obj.input,
                "output": obj.output,
                "cache_creation": obj.cache_creation,
                "cache_read": obj.cache_read,
                "turns": obj.turns,
                "prefix_first": obj.prefix_first,
                "prefix_last": obj.prefix_last,
                "prefix_max": obj.prefix_max,
                "prefix_growth": obj.prefix_growth(),
                "by_model": obj.by_model,
            }
        if isinstance(obj, Tokens):
            return {
                "input": obj.input,
                "output": obj.output,
                "cache_creation": obj.cache_creation,
                "cache_read": obj.cache_read,
            }
        if isinstance(obj, Cost):
            return {
                "input": round(obj.input, 6),
                "output": round(obj.output, 6),
                "cache_read": round(obj.cache_read, 6),
                "cache_write": round(obj.cache_write, 6),
                "total": round(obj.total(), 6),
            }
        if isinstance(obj, ToolLine):
            return {
                "name": obj.name,
                "calls": obj.calls,
                "result_bytes": obj.result_bytes,
            }
        if isinstance(obj, SinkLine):
            return {"name": obj.name, "label": obj.label, "bytes": obj.bytes}
        if isinstance(obj, SubAgentLine):
            return {
                "agent": obj.agent,
                "turns": obj.turns,
                "input": obj.input,
                "output": obj.output,
                "cache_creation": obj.cache_creation,
                "cache_read": obj.cache_read,
                "by_model": obj.by_model,
            }
        if isinstance(obj, HardeningCandidate):
            return {
                "signature": obj.signature,
                "count": obj.count,
                "deterministic": obj.deterministic,
                "result_bytes": obj.result_bytes,
                "avg_bytes": obj.avg_bytes(),
                "via_run_quiet": obj.via_run_quiet,
                "cost_kind": obj.cost_kind(),
            }
        if isinstance(obj, ResidentLine):
            return {
                "kind": obj.kind,
                "label": obj.label,
                "bytes": obj.bytes,
                "turns": obj.turns,
                "byte_turns": obj.byte_turns(),
            }
        if isinstance(obj, ResidentProse):
            return {
                "lines": obj.lines,
                "omitted": obj.omitted,
                "bytes_per_token": round(obj.bytes_per_token, 3),
                "calibration_samples": obj.samples,
                "ratio_in_gate_band": obj.ratio_in_band(),
                "token_turns": obj.token_turns,
                "by_kind": obj.by_kind,
                "share": round(obj.share, 4),
                "cost": None if obj.cost is None else round(obj.cost, 6),
                "lever": obj.is_lever(),
            }
        raise TypeError(f"not serializable: {type(obj)!r}")

    return json.dumps(report, default=encode, indent=2, ensure_ascii=False)


# --------------------------------------------------------------------------- #
# Transcript resolution and the CLI.
# --------------------------------------------------------------------------- #


def claude_home() -> Path:
    """The Claude home directory: ``CLAUDE_CONFIG_DIR`` if set, else ``~/.claude``."""
    configured = os.environ.get("CLAUDE_CONFIG_DIR", "").strip()
    if configured:
        return Path(configured)
    home = os.environ.get("HOME")
    if not home:
        raise RuntimeError("neither CLAUDE_CONFIG_DIR nor HOME is set")
    return Path(home) / ".claude"


def tag_from_cwd(cwd: str | None) -> str | None:
    """The worktree tag a session ran in, or ``None`` for a base-repo session.

    Derived purely from the path — `claude --worktree` lays a worktree out at
    ``<base>/.claude/worktrees/<tag>``, so the tag is the segment after that
    marker. A base-repo session (which is what every seat verb starts) has no
    such segment and so no tag, which is the correct answer rather than a
    failure: it has no marker either.

    **Deliberately not named ``worktree_tag``**, which
    `prune_conversations.py` already uses for a different input domain: that
    one takes a **slug** (a path with ``/`` and ``.`` collapsed to ``-``) plus a
    prefix, this one takes a real path. Two same-named helpers over different
    domains in sibling tools is the kind of collision a reader resolves
    wrongly, so the names are kept distinct instead.
    """
    if not cwd:
        return None
    _, sep, tail = cwd.partition(WORKTREE_SEGMENT)
    if not sep:
        return None
    tag = tail.split("/", 1)[0].strip()
    # The tag is interpolated into a marker path below, so reject outright the
    # names that would leave the marker directory. `split("/")` already rules
    # out a separator, and a NUL raises from the path layer rather than
    # resolving — but both are stated explicitly here rather than relied on as
    # implicit properties, matching how the `--session-id` guard further down
    # this file defends the same shape.
    if tag in {".", ".."} or "\x00" in tag:
        return None
    return tag or None


def base_repo_from_cwd(cwd: str | None) -> Path | None:
    """The base checkout containing a worktree session's cwd, or ``None``.

    The substrate markers live in the **base** repo — one directory shared by
    every worktree, the same way `settings.local.json` resolves — so a worktree
    session has to walk back out to find its own marker.
    """
    if not cwd:
        return None
    head, sep, _ = cwd.partition(WORKTREE_SEGMENT)
    # An empty head is rejected rather than silently reinterpreted: `Path("")`
    # resolves to `.`, which would turn the marker lookup into a RELATIVE read
    # against whatever directory the mining process happens to be run from.
    if not sep or not head:
        return None
    return Path(head)


def read_substrate_marker(cwd: str | None) -> str | None:
    """The substrate recorded for this session's worktree, or ``None``.

    ``None`` means *no marker was found* — a base-repo session, a marker never
    written, or one cleaned up when the worktree was pruned. It is deliberately
    distinct from a marker that says ``seat``, so the report can say which of
    the two it is, even though both land on the same headline.
    """
    base = base_repo_from_cwd(cwd)
    tag = tag_from_cwd(cwd)
    if base is None or not tag:
        return None
    marker = base / SUBSTRATE_DIR / tag
    try:
        recorded = marker.read_text(encoding="utf-8").strip()
    except (OSError, ValueError):
        # `ValueError` is not redundant with `OSError` here, and catching only
        # the latter made this lookup able to kill the whole report: a strict
        # UTF-8 `read_text` raises `UnicodeDecodeError` (a `ValueError`) on a
        # malformed marker, and nothing up the call chain handles it. The writer
        # in `.claude/shell/init.zsh` is a plain truncating redirect rather than
        # an atomic rename, so a torn write is the realistic producer of one.
        # Degrading to seat here is what makes the documented fail-toward-seat
        # policy true for malformed content as well as for a missing file.
        return None
    return recorded or None


def resolve_substrate(cwd: str | None) -> tuple[str, str]:
    """Decide a session's billing substrate, returning ``(substrate, reason)``.

    **The transcript cannot answer this, which is why the marker is consulted
    at all.** Measured on a Bedrock worker session (2026-09-14): its transcript
    records ``message.model`` as the plain ``claude-opus-5``, byte-identical to
    what a seat Opus session records, and no region-prefixed or
    inference-profile form appears in any local transcript. Nor is the model *name* a usable
    proxy, since seat verbs (`housekeeping`, `explore`) also run on Opus. The
    only durable signal is the marker a launch writes, which is exactly what the
    resume verbs already steer by.

    **Absent reads as seat**, matching `_ds_substrate_read` in
    `.claude/shell/init.zsh` and for a sharper reason here: the seat branch
    prints no dollar figure, so an unknown substrate degrades to reporting a
    token profile rather than to inventing a number. A missing figure is a
    visible gap; a wrong one silently corrupts the reconciliation.
    """
    recorded = read_substrate_marker(cwd)
    if recorded == SUBSTRATE_BEDROCK:
        return SUBSTRATE_BEDROCK, "recorded by the launch verb"
    if recorded in (SUBSTRATE_SEAT, SUBSTRATE_ANTHROPIC):
        return SUBSTRATE_SEAT, "recorded by the launch verb"
    if recorded is None:
        if tag_from_cwd(cwd) is None:
            if base_repo_from_cwd(cwd) is None:
                return SUBSTRATE_SEAT, "base-repo session, so a seat verb"
            # Inside the worktrees directory but carrying no tag component. That
            # is a malformed path, not a base-repo session, so it does not get
            # the base-repo reason string.
            return SUBSTRATE_SEAT, "no worktree tag in the path, which reads as seat"
        return SUBSTRATE_SEAT, "no substrate marker found, which reads as seat"
    # A marker holding something unrecognized: treat it as unknown rather than
    # guessing, and fail toward the branch that prints no dollars. Truncated
    # because this string reaches the report, and an unexpected file at that
    # path should not have its whole body echoed into it.
    return SUBSTRATE_SEAT, f"unrecognized marker {recorded[:32]!r}, treated as seat"


def slugify(path: Path) -> str:
    """Claude Code names each project's transcript directory after the working
    directory, replacing every ``/`` and ``.`` with ``-``.
    """
    return "".join("-" if c in "/." else c for c in str(path))


def resolve_transcript(session_id: str) -> Path:
    """Resolve a session id to its transcript file. Tries the slug of the
    current working directory first (the common case), then scans every project
    directory for ``<session-id>.jsonl`` — so a worktree whose slug differs from
    the cwd still resolves.
    """
    projects = claude_home() / "projects"
    file_name = f"{session_id}.jsonl"

    cwd = Path.cwd()
    primary = projects / slugify(cwd) / file_name
    if primary.is_file():
        return primary

    if not projects.is_dir():
        raise FileNotFoundError(f"reading projects directory {projects}")
    for entry in projects.iterdir():
        candidate = entry / file_name
        if candidate.is_file():
            return candidate
    raise FileNotFoundError(f"no transcript {file_name} found under {projects}")


def agent_label(jsonl: Path) -> str:
    """A human label for a sub-agent: the ``description`` (else the
    ``agentType``) from the sibling ``<stem>.meta.json``, falling back to the
    file stem when the sidecar is absent or malformed.
    """
    stem = jsonl.stem or "agent"
    meta_path = jsonl.with_name(f"{stem}.meta.json")
    try:
        meta = json.loads(meta_path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return stem
    if isinstance(meta, dict):
        for key in ("description", "agentType"):
            val = meta.get(key)
            if isinstance(val, str) and val.strip():
                return val
    return stem


def _read_lines(path: Path):
    """Yield a file's lines, skipping any that can't be decoded (a partial
    trailing write, say) so accounting never aborts on a malformed tail.
    """
    with path.open("r", encoding="utf-8", errors="replace") as handle:
        yield from handle


def aggregate(transcript: Path, session_id: str, substrate: str | None = None) -> dict:
    """Stream the main transcript and every sub-agent transcript into a report."""
    agg = SessionAggregator()
    for line in _read_lines(transcript):
        agg.ingest_main_line(line)

    # Sub-agent transcripts live in `<transcript-dir>/<session-id>/subagents/`.
    subagents_dir = transcript.parent / session_id / "subagents"
    if subagents_dir.is_dir():
        for entry in subagents_dir.iterdir():
            if entry.suffix != ".jsonl":
                continue
            label = agent_label(entry)
            for line in _read_lines(entry):
                agg.ingest_subagent_line(label, line)

    return agg.finish(substrate)


def short_id(session_id: str) -> str:
    """The first segment of a UUID — enough to identify the session without
    printing the whole id.
    """
    return session_id.split("-", 1)[0] or session_id


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="session_metrics.py",
        description=(
            "Summarize where a session's tokens went: totals, cache-hit rate, "
            "the costliest tools, the largest single results, a per-sub-agent "
            "rollup, and repeated command shapes worth hardening into a tool. "
            "Reads the transcript in this process; only the summary is printed."
        ),
    )
    parser.add_argument("--session-id", required=True, help="the session UUID")
    parser.add_argument(
        "--json",
        action="store_true",
        help="emit the summary as JSON instead of Markdown",
    )
    parser.add_argument(
        "--substrate",
        choices=(SUBSTRATE_BEDROCK, SUBSTRATE_SEAT),
        default=None,
        help=(
            "override the substrate the launch recorded. The escape hatch for a "
            "session whose marker is gone — pruning a worktree can take its "
            "marker with it, and an absent marker reads as seat, so a historical "
            "Bedrock session would otherwise report no dollar figure"
        ),
    )
    args = parser.parse_args(argv)

    session_id = args.session_id
    # The id is interpolated into a filename, so reject anything that could
    # escape the projects directory (a session id is always a bare UUID).
    if not session_id or "/" in session_id or "\\" in session_id or ".." in session_id:
        parser.error("--session-id must be a bare session id (a UUID), not a path")

    transcript = resolve_transcript(session_id)
    report = aggregate(transcript, session_id, args.substrate)

    if args.json:
        print(to_json(report))
    else:
        sys.stdout.write(to_markdown(report, short_id(session_id)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
