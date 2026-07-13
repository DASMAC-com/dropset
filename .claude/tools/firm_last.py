#!/usr/bin/env python3
"""firm_last.py — firm the single just-approved tool call into the allowlist.

The deterministic core of the ``/f`` fast-firm skill. ``/f`` is typed right
after you one-time-approve a permission prompt, so the just-approved command is
the most recent *executed* tool call in the session transcript. This tool finds
it, generalizes it into a reusable allow-rule (via ``firm_core``), and writes
that rule into this worktree's and the base repo's ``settings.local.json`` —
the worktree copy hot-reloads for the running session, the base copy seeds
future worktrees.

Usage:
    python3 .claude/tools/firm_last.py            # generalize + firm
    python3 .claude/tools/firm_last.py exact      # firm the command verbatim
    python3 .claude/tools/firm_last.py --base-only # skip the worktree write
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
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
    """The transcript file for the running session. Prefers an explicit or
    ``$CLAUDE_SESSION_ID`` session id; otherwise takes the most recently
    modified top-level ``*.jsonl`` under this cwd's project slug (the session
    actively being appended to), scanning every project dir as a fallback.
    """
    projects = claude_home() / "projects"
    session_id = session_id or os.environ.get("CLAUDE_SESSION_ID", "").strip() or None
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
    """Whether a tool call is part of the /f machinery itself (this tool's own
    Bash run, or the skill invocation), which must never be the firm target.
    """
    if name == "Skill":
        # Only the /f and /firm-perms invocations are the firm machinery; other
        # skills are ordinary calls the user might legitimately want to firm.
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
    part of the /f machinery — the one ``/f`` means to firm.
    """
    for entry in reversed(calls):
        if _is_self_call(entry["name"], entry["input"]):
            continue
        if not entry["has_result"] or entry["denied"]:
            continue
        return entry
    return None


def find_base_repo() -> str | None:
    """The path of the worktree whose branch is ``refs/heads/main``."""
    try:
        out = subprocess.run(
            ["git", "worktree", "list", "--porcelain"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError):
        return None
    current = None
    for line in out.splitlines():
        if line.startswith("worktree "):
            current = line[len("worktree ") :].strip()
        elif line.strip() == "branch refs/heads/main":
            return current
    return None


def load_settings(path: Path) -> tuple[dict, list[str]]:
    """Load a settings.local.json into ``(settings_dict, allow_list)``. A missing
    or malformed file yields empty scaffolding so a first firm can create it.
    """
    try:
        settings = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        settings = {}
    if not isinstance(settings, dict):
        settings = {}
    allow = settings.get("permissions", {}).get("allow")
    if not isinstance(allow, list):
        allow = []
    return settings, allow


def write_settings(path: Path, settings: dict, allow: list[str]) -> None:
    settings.setdefault("permissions", {})
    settings["permissions"]["allow"] = allow
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(settings, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )


def firm_into(path: Path, rule: str) -> bool:
    """Add ``rule`` to a settings file's allow array if not already covered.
    Returns whether the file was changed.
    """
    settings, allow = load_settings(path)
    if firm_core.is_covered(rule, allow):
        return False
    # Drop any existing entry the new (broader) rule now subsumes, so firming a
    # generalized rule doesn't leave the redundant narrower ones behind.
    allow = [r for r in allow if not firm_core.is_covered(r, [rule])]
    allow.append(rule)
    write_settings(path, settings, allow)
    return True


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Firm the just-approved tool call.")
    parser.add_argument(
        "exact",
        nargs="?",
        default="",
        help="pass 'exact' to firm the command verbatim instead of generalized",
    )
    parser.add_argument(
        "--base-only", action="store_true", help="skip the worktree write"
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

    worktree_settings = Path.cwd() / ".claude" / "settings.local.json"
    base = find_base_repo()
    targets: list[tuple[str, Path]] = []
    if not args.base_only:
        targets.append(("this worktree", worktree_settings))
    if base:
        base_settings = Path(base) / ".claude" / "settings.local.json"
        if base_settings != worktree_settings:
            targets.append(("base repo", base_settings))
    elif not args.base_only:
        print("firm-last: no main worktree found — firming only this worktree.")

    if not targets:
        # --base-only with no base repo resolvable: nothing to write anywhere.
        print("firm-last: no base repo found — nothing firmed.")
        return 0

    changed = [label for label, path in targets if firm_into(path, rule)]
    if changed:
        print(f"firm-last: firmed {rule} into {', '.join(changed)}.")
    else:
        print(f"firm-last: {rule} already covered — nothing to firm.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
