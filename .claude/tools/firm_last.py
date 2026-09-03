#!/usr/bin/env python3
"""firm_last.py — firm the single just-approved tool call into the allowlist.

The deterministic core of ``firm-perms``' fast firm. That is typed right
after you one-time-approve a permission prompt, so the just-approved command is
the most recent *executed* tool call in the session transcript. This tool finds
it, generalizes it into a reusable allow-rule (via ``firm_core``), and writes
that rule into ``settings.local.json`` at the **main checkout**.

That is the only write target, and deliberately so. Claude Code resolves
``.claude/settings.local.json`` through a worktree to the main checkout for
reads *and* writes, so one file governs every worktree; a rule firmed from
inside a worktree is live everywhere immediately. An earlier version also
wrote a copy under the active worktree, which was redundant at best and
actively misleading at worst — nothing ever reads that path, so a stale copy
there looks live while doing nothing.

Usage:
    python3 .claude/tools/firm_last.py            # generalize + firm
    python3 .claude/tools/firm_last.py exact      # firm the command verbatim
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

import firm_core

# Substrings that mark a tool_result as a *denied* (rejected) call, so it is not
# treated as "approved" and firmed.
_DENIAL_MARKERS = (
    "doesn't want to proceed",
    "tool use was rejected",
    "user rejected",
    "user doesn't want to take this action",
)


def claude_home() -> Path:
    configured = os.environ.get("CLAUDE_CONFIG_DIR", "").strip()
    if configured:
        return Path(configured)
    home = os.environ.get("HOME")
    if not home:
        raise RuntimeError("neither CLAUDE_CONFIG_DIR nor HOME is set")
    return Path(home) / ".claude"


def slugify(path: Path) -> str:
    """Claude Code names each project's transcript dir after the working dir,
    replacing every ``/`` and ``.`` with ``-``.
    """
    return "".join("-" if c in "/." else c for c in str(path))


def resolve_active_transcript(session_id: str | None = None) -> Path:
    """The transcript file for the running session.

    An explicit ``session_id`` (``--session-id``) is exact and is what callers
    should pass. Without one, this takes the most recently modified top-level
    ``*.jsonl`` under this cwd's project slug, scanning every project dir as a
    fallback.

    **Newest-mtime is a documented contract, not an accident.** This used to
    also consult ``$CLAUDE_SESSION_ID``, which reads as the primary mechanism
    and is not one: that variable is **not present in a Bash tool call**
    (``printenv CLAUDE_SESSION_ID`` exits 1), so the branch was unreachable and
    every firm resolved by mtime while appearing not to. The branch is gone
    rather than left as an apparent mechanism.

    Callers should know what mtime costs them. With one planning session plus
    several implementers writing transcripts concurrently, the most recently
    touched file is frequently **not** the session that just approved the
    command — so an unattended firm can harvest another session's approval into
    the shared allowlist. That is recoverable (the write is additive and the
    caller reports it) but it is a race, which is why ``firm-perms`` passes
    ``--session-id`` from its own scratchpad path and why the resolved session
    is named in the output.

    **And state the second fallback's real reach, which is wider than that.**
    When this cwd's slug directory holds no transcript at all, the search
    widens to ``projects.glob("*/*.jsonl")`` — **every project on the machine**,
    other repositories included — and the rule it harvests still lands in
    *this* repo's shared allowlist. That path is rare rather than normal: a
    worktree session's transcript does live under its own worktree slug
    (verified — ``slugify(cwd)`` matches the directory the harness writes), so
    the widening fires only when the slug directory is genuinely absent, as on
    a brand-new checkout. It is documented here because the narrower wording
    read as the whole contract, and because the accounting line now prints the
    transcript's parent directory so a foreign project is legible rather than
    silent.
    """
    projects = claude_home() / "projects"
    session_id = (session_id or "").strip() or None
    if session_id:
        primary = projects / slugify(Path.cwd()) / f"{session_id}.jsonl"
        if primary.is_file():
            return primary
        for entry in projects.iterdir() if projects.is_dir() else []:
            candidate = entry / f"{session_id}.jsonl"
            if candidate.is_file():
                return candidate
        raise FileNotFoundError(f"no transcript for session {session_id}")

    slug_dir = projects / slugify(Path.cwd())
    candidates = list(slug_dir.glob("*.jsonl")) if slug_dir.is_dir() else []
    if not candidates and projects.is_dir():
        candidates = list(projects.glob("*/*.jsonl"))
    if not candidates:
        raise FileNotFoundError(f"no session transcript under {projects}")
    return max(candidates, key=lambda p: p.stat().st_mtime)


def _content_text(content) -> str:
    """A tool_result's content as a lowercased string, for denial detection."""
    if isinstance(content, str):
        return content.lower()
    try:
        return json.dumps(content).lower()
    except (TypeError, ValueError):
        return ""


def _is_self_call(name: str, tool_input: dict) -> bool:
    """Whether a tool call is part of the firm machinery itself (this tool's own
    Bash run, or the skill invocation), which must never be the firm target.
    """
    if name == "Skill":
        # Only the firm-perms invocation is the firm machinery; other skills are
        # ordinary calls the user might legitimately want to firm. The retired
        # `f` shorthand is kept in this set on purpose: it costs nothing, and a
        # transcript written before the retirement still carries that name.
        skill = tool_input.get("skill") or tool_input.get("name")
        return skill in {"f", "firm-perms"}
    if name == "Bash":
        command = tool_input.get("command", "")
        if isinstance(command, str) and "firm_last.py" in command:
            return True
    return False


def iter_tool_calls(lines) -> list[dict]:
    """Walk transcript lines into an ordered list of tool calls, each
    ``{name, input, has_result, denied}``. tool_use items establish order;
    tool_result items (in later user records) fill in the outcome by id.
    """
    calls: list[dict] = []
    by_id: dict[str, dict] = {}
    for line in lines:
        line = line.strip()
        if not line:
            continue
        try:
            rec = json.loads(line)
        except (json.JSONDecodeError, ValueError):
            continue
        msg = rec.get("message") if isinstance(rec, dict) else None
        content = msg.get("content") if isinstance(msg, dict) else None
        if not isinstance(content, list):
            continue
        for item in content:
            if not isinstance(item, dict):
                continue
            if item.get("type") == "tool_use":
                tid = item.get("id")
                name = item.get("name")
                if not isinstance(tid, str) or not isinstance(name, str):
                    continue
                entry = {
                    "name": name,
                    "input": item.get("input")
                    if isinstance(item.get("input"), dict)
                    else {},
                    "has_result": False,
                    "denied": False,
                }
                calls.append(entry)
                by_id[tid] = entry
            elif item.get("type") == "tool_result":
                tid = item.get("tool_use_id")
                entry = by_id.get(tid) if isinstance(tid, str) else None
                if entry is None:
                    continue
                entry["has_result"] = True
                # A denial is structural: the rejection tool_result carries
                # `is_error`. Require it *and* a marker phrase, so an approved
                # call whose output merely contains "user rejected" (a grep hit,
                # a diff, this file's own source) isn't mistaken for a denial.
                is_error = bool(item.get("is_error"))
                text = _content_text(item.get("content"))
                if is_error and any(marker in text for marker in _DENIAL_MARKERS):
                    entry["denied"] = True
    return calls


def most_recent_approved_call(calls: list[dict]) -> dict | None:
    """The most recent executed (has a result), non-denied tool call that isn't
    part of the firm machinery — the one the fast firm means to firm.
    """
    for entry in reversed(calls):
        if _is_self_call(entry["name"], entry["input"]):
            continue
        if not entry["has_result"] or entry["denied"]:
            continue
        return entry
    return None


def find_base_repo() -> str | None:
    """The path of the worktree whose branch is ``refs/heads/main``.

    A thin wrapper over ``firm_core.main_checkout`` — the single owner, so this
    and ``allowlist.py`` can never disagree about where the shared
    ``settings.local.json`` lives. They previously had separate copies of this
    scan, and the copies drifted into a real behavioral divergence.
    """
    base = firm_core.main_checkout()
    return None if base is None else str(base)


# The settings read/write pair lives in ``firm_core`` so ``allowlist.py``'s
# ``add`` writes byte-identically to what this tool writes. Re-exported here under
# the names this module has always used.
firm_into = firm_core.firm_into
load_settings = firm_core.load_settings
write_settings = firm_core.write_settings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Firm the just-approved tool call.")
    parser.add_argument(
        "exact",
        nargs="?",
        default="",
        help="pass 'exact' to firm the command verbatim instead of generalized",
    )
    parser.add_argument(
        "--base-only",
        action="store_true",
        help="accepted and ignored; the base repo is the only write target",
    )
    parser.add_argument("--session-id", default=None, help="override the session id")
    args = parser.parse_args(argv)
    exact = args.exact.strip().lower() == "exact"

    try:
        transcript = resolve_active_transcript(args.session_id)
    except (FileNotFoundError, RuntimeError) as exc:
        print(f"firm-last: {exc}", file=sys.stderr)
        return 1

    with transcript.open("r", encoding="utf-8", errors="replace") as handle:
        calls = iter_tool_calls(handle)
    call = most_recent_approved_call(calls)
    if call is None:
        print("firm-last: no just-approved tool call found — nothing to firm.")
        return 0

    rule = firm_core.generalize(call["name"], call["input"], exact=exact)
    if rule is None:
        print(
            f"firm-last: the last call ({call['name']}) can't reduce to a safe "
            "rule (a compound / heredoc / one-liner) — fix the source, don't "
            "allow-list it."
        )
        return 0
    if firm_core.is_bareverb_wildcard(rule):
        print(
            f"firm-last: generalizing would produce the over-broad rule '{rule}'. "
            "That grants the whole verb — narrow it by hand instead of firming."
        )
        return 0

    # There is exactly ONE settings.local.json: Claude Code resolves it
    # through a worktree to the main checkout, for reads and writes alike.
    # Writing a second copy under the worktree would produce a file nothing
    # ever reads — worse than useless, because it looks live.
    base = find_base_repo()
    if base is None:
        # Non-zero: nothing was firmed, and a fast firm reporting success on a
        # no-op is how a missed rule goes unnoticed.
        print(
            "firm-last: no worktree is on `main`, so the shared "
            "settings.local.json can't be located — nothing firmed.",
            file=sys.stderr,
        )
        return 1
    target = Path(base) / ".claude" / "settings.local.json"

    try:
        changed = firm_into(target, rule)
    except firm_core.SettingsError as exc:
        # An existing settings file that doesn't parse: report it rather than
        # letting the writer replace it (or dying on a raw traceback).
        print(f"firm-last: {exc}", file=sys.stderr)
        return 1
    # Name the session the rule was harvested from. Without an explicit
    # --session-id this is a newest-mtime guess among concurrently-written
    # transcripts, so a surprising firm needs to be diagnosable from the output
    # rather than by re-deriving which session the tool happened to pick.
    provenance = "" if args.session_id else " (resolved by newest transcript)"
    if changed:
        print(f"firm-last: firmed {rule} into {target}.")
        print(f"firm-last: harvested from {transcript.stem}{provenance}.")
    else:
        print(f"firm-last: {rule} already covered — nothing to firm.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
