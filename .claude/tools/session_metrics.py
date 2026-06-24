#!/usr/bin/env python3
"""``session-metrics`` (`.claude/tools/session_metrics.py`) — account for where
a Claude Code session spent its tokens, so the ``session-metrics`` skill can
recommend concrete trims.

Given a ``--session-id``, the tool resolves the session's on-disk transcript
itself, reads it (and its sub-agent transcripts) in its **own** process — so the
multi-megabyte file never enters the model's context — and prints a compact,
ranked summary: session-wide token totals, a cache-hit rate, the tools whose
results cost the most, the single largest results, a per-sub-agent rollup, and
the repeated, deterministic command shapes that are candidates to harden into a
tool. Pass ``--json`` for the same data as JSON.

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
from dataclasses import dataclass
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


# --------------------------------------------------------------------------- #
# Token-cost aggregation (mirrors the former Rust `model.rs`).
# --------------------------------------------------------------------------- #


@dataclass
class Totals:
    """Session-wide token totals, summed across every assistant turn."""

    input: int = 0
    output: int = 0
    cache_creation: int = 0
    cache_read: int = 0
    turns: int = 0

    def add(self, usage: dict) -> None:
        self.input += int(usage.get("input_tokens", 0) or 0)
        self.output += int(usage.get("output_tokens", 0) or 0)
        self.cache_creation += int(usage.get("cache_creation_input_tokens", 0) or 0)
        self.cache_read += int(usage.get("cache_read_input_tokens", 0) or 0)
        self.turns += 1

    def total_input(self) -> int:
        """Total input the model processed: fresh input plus both cache tiers."""
        return self.input + self.cache_creation + self.cache_read


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

    def total_input(self) -> int:
        return self.input + self.cache_creation + self.cache_read


@dataclass
class HardeningCandidate:
    """A repeated, deterministic Bash command shape worth porting to a tool."""

    signature: str
    count: int
    deterministic: bool


@dataclass
class _ToolCall:
    """A pending tool call awaiting its result, keyed by tool_use_id."""

    name: str
    label: str


@dataclass
class _SubAgentAcc:
    turns: int = 0
    input: int = 0
    output: int = 0
    cache_creation: int = 0
    cache_read: int = 0


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
        self._bash_signatures: dict[str, int] = {}
        # tool_use ids whose Bash signature was already counted. The content
        # array is re-walked on every content-block record of a split message
        # (tool_use items can repeat), so dedupe the signature count by id —
        # otherwise a split message inflates a command's hardening count.
        self._counted_bash_ids: set[str] = set()
        self.parse_errors = 0

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
        msg = rec.get("message")
        if not isinstance(msg, dict):
            return
        usage = msg.get("usage")
        if isinstance(usage, dict):
            # Sum usage once per logical message, not once per content-block
            # record (which repeats the same usage).
            if self._first_usage_sighting(msg.get("id")):
                self.totals.add(usage)
        # The content array is walked on *every* record (tool_use items are
        # idempotent in `pending`; tool_results live in separate user records),
        # so attribution is unaffected by the per-message split.
        content = msg.get("content")
        if not isinstance(content, list):
            return
        for item in content:
            if isinstance(item, dict):
                self._ingest_content_item(item)

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
                self._record_bash_signature(item.get("input"))
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

    def _record_bash_signature(self, input_obj) -> None:
        if not isinstance(input_obj, dict):
            return
        command = input_obj.get("command")
        if not isinstance(command, str):
            return
        sig = bash_signature(command)
        if sig:
            self._bash_signatures[sig] = self._bash_signatures.get(sig, 0) + 1

    # -- finishing -------------------------------------------------------- #

    def finish(self) -> dict:
        """Rank and truncate into the final report dict."""
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
            )
            for agent, acc in self._subagents.items()
        ]
        subagents.sort(key=lambda a: (-a.total_input(), a.agent))

        candidates = [
            HardeningCandidate(
                signature=sig,
                count=count,
                deterministic=is_deterministic_shape(sig),
            )
            for sig, count in self._bash_signatures.items()
            if count >= HARDENING_MIN_COUNT
        ]
        # Deterministic shapes first (the real port candidates), then by count.
        candidates.sort(key=lambda c: (not c.deterministic, -c.count, c.signature))
        candidates_omitted = max(0, len(candidates) - HARDENING_TOP_N)
        candidates = candidates[:HARDENING_TOP_N]

        return {
            "totals": self.totals,
            "cache_hit_rate": cache_hit_rate,
            "tools": tools,
            "top_sinks": sinks,
            "subagents": subagents,
            "hardening_candidates": candidates,
            "parse_errors": self.parse_errors,
            "tools_omitted": tools_omitted,
            "sinks_omitted": sinks_omitted,
            "candidates_omitted": candidates_omitted,
        }


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


def _is_subcommand_word(tok: str) -> bool:
    """A stable subcommand word: lowercase ASCII letters and hyphens only, no
    digits — so ``worktree`` / ``list`` / ``pull`` qualify but a specific arg
    (``worktree-eng-1``, ``LINEAR_TEAM_ID``, ``main..HEAD``) does not.
    """
    stripped = tok.replace("-", "")
    return (
        bool(stripped) and stripped.isascii() and stripped.isalpha() and tok.islower()
    )


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
    """
    tokens = command.split()
    # Drop any leading environment assignments (`FOO=bar cmd …`).
    while tokens and _ENV_ASSIGN.match(tokens[0]):
        tokens.pop(0)
    if not tokens:
        return ""
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


def to_markdown(report: dict, session_label: str) -> str:
    """Render the compact Markdown summary printed by default."""
    totals: Totals = report["totals"]
    out: list[str] = []
    out.append(f"## Session metrics — {session_label}\n\n")
    out.append(
        "**Totals**: input {} · output {} · cache-write {} · cache-read {} · {} turns\n".format(
            human(totals.input),
            human(totals.output),
            human(totals.cache_creation),
            human(totals.cache_read),
            totals.turns,
        )
    )
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
        out.append("\n### Hardening candidates (repeated command shapes)\n\n")
        out.append("| command shape | count | deterministic |\n|---|--:|:--:|\n")
        for c in candidates:
            mark = "yes" if c.deterministic else "no"
            out.append(f"| `{c.signature}` | {c.count} | {mark} |\n")
        if report["candidates_omitted"] > 0:
            out.append(f"\n_+{report['candidates_omitted']} more shape(s) omitted._\n")

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
            }
        if isinstance(obj, HardeningCandidate):
            return {
                "signature": obj.signature,
                "count": obj.count,
                "deterministic": obj.deterministic,
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


def aggregate(transcript: Path, session_id: str) -> dict:
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

    return agg.finish()


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
    args = parser.parse_args(argv)

    session_id = args.session_id
    # The id is interpolated into a filename, so reject anything that could
    # escape the projects directory (a session id is always a bare UUID).
    if not session_id or "/" in session_id or "\\" in session_id or ".." in session_id:
        parser.error("--session-id must be a bare session id (a UUID), not a path")

    transcript = resolve_transcript(session_id)
    report = aggregate(transcript, session_id)

    if args.json:
        print(to_json(report))
    else:
        sys.stdout.write(to_markdown(report, short_id(session_id)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
