#!/usr/bin/env python3
"""hook_wiring.py — report committed guard hooks that nothing wires.

A guard script under ``.claude/hooks/`` does **nothing** until a ``PreToolUse``
entry in a settings file points at it. The scripts are committed; the wiring
deliberately is not (``settings.json`` and ``settings.local.json`` are both
git-ignored, per ``docs/conventions/local-integrations.md``). That combination
is exactly the condition under which a guard sits committed and inert
indefinitely with nobody noticing — the repo documents a protection, the script
is right there, and **nothing anywhere checks that the two are connected**.

It happened: as of 2026-08-14 all three guards were committed and only
``no_compound_bash.py`` was wired. The worktree edit-path guard — which exists
to stop a worktree session mutating a base-repo path, a slip the conventions
call recurring and expensive — had been documented as active protection while
providing none.

CI cannot catch this. The settings files are git-ignored, so a PR cannot
install wiring and a CI runner has none to inspect; the check has to run on the
machine that owns the settings. ``housekeeping`` is the natural driver — it
already runs day-to-day upkeep from the base repo, where the settings resolve.

**This tool reports; it never writes.** Wiring a guard grants a hook the right
to block tool calls, which is the operator's decision, not an agent's — the
same posture as the rest of the local-integrations story. Output names the
unwired scripts plainly rather than printing a settings diff.

Usage::

    python3 .claude/tools/hook_wiring.py            # human-readable report
    python3 .claude/tools/hook_wiring.py --json     # machine-readable

Exit status is grep-shaped, so a caller can branch on it without parsing:
``0`` when every committed hook is wired, ``1`` when at least one is not, and
``2`` when the scan itself could not run (no main checkout, unreadable
settings). A clean scan and a broken scan must never look alike — that is the
failure this tool exists to prevent, and it would be ironic to reproduce it.

Standard library only. A Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling"). Tests live in ``tests/test_hook_wiring.py``, run via
``make tools-tests``.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import firm_core

# Every settings file that can carry a `hooks` key, in the order Claude Code
# layers them. All three are git-ignored or user-local, so none is guaranteed to
# exist — a missing file is a normal outcome, not an error.
#
# The two repo-scoped files resolve **through a worktree to the main checkout**
# (see local-integrations.md → "How settings files resolve across worktrees"),
# so they are read from the base repo rather than the cwd. Reading them from a
# worktree would find nothing and report every guard unwired — a false alarm
# that would train the reader to ignore this tool.
REPO_SETTINGS = ("settings.json", "settings.local.json")
USER_SETTINGS = Path.home() / ".claude" / "settings.json"

# Hook events that can point at a guard script. `PreToolUse` is the one this
# repo's guards use, but a script wired to any event is wired — reporting a
# `PostToolUse` hook as "unwired" would be a false positive.
HOOK_EVENTS = (
    "PreToolUse",
    "PostToolUse",
    "UserPromptSubmit",
    "Notification",
    "Stop",
    "SubagentStop",
    "SessionStart",
    "SessionEnd",
    "PreCompact",
)


class HookWiringError(Exception):
    """A user-facing failure: surfaced to stderr, exits 2."""


def iter_hook_commands(settings: dict) -> list[str]:
    """Every hook ``command`` string in one settings mapping.

    Shape-tolerant on purpose: this walks a user-authored, git-ignored file
    that no schema validates, so anything unexpected is skipped rather than
    raised on. A malformed entry should cost one missed match, not a crashed
    upkeep run.
    """
    commands: list[str] = []
    hooks = settings.get("hooks")
    if not isinstance(hooks, dict):
        return commands
    for event in HOOK_EVENTS:
        for matcher_entry in hooks.get(event) or []:
            if not isinstance(matcher_entry, dict):
                continue
            for hook in matcher_entry.get("hooks") or []:
                if not isinstance(hook, dict):
                    continue
                command = hook.get("command")
                if isinstance(command, str):
                    commands.append(command)
    return commands


def load_settings(path: Path) -> dict:
    """Parse one settings file. Missing is ``{}``; malformed is an error.

    The asymmetry is deliberate. A missing file genuinely means "nothing wired
    here" and is the common case. A file that exists but does not parse means
    the scan's answer is unknown — and reporting "all wired" from a file the
    tool could not read is precisely the silent-clean-result this exists to
    prevent.
    """
    if not path.exists():
        return {}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise HookWiringError(f"could not read {path}: {exc}") from exc
    return data if isinstance(data, dict) else {}


def committed_hooks(repo: Path) -> list[str]:
    """Base names of every ``.py`` under ``.claude/hooks/``.

    Tracked-ness is deliberately not checked: the directory holds committed
    guards, so listing it is the same answer without shelling out to git. The
    residual is that an untracked scratch script dropped in there reports as
    ``UNWIRED`` — noise in a report whose value depends on not crying wolf, but
    a cheaper failure than a git call that has to work in a worktree.
    """
    hooks_dir = repo / ".claude" / "hooks"
    if not hooks_dir.is_dir():
        return []
    return sorted(
        p.name for p in hooks_dir.iterdir() if p.is_file() and p.suffix == ".py"
    )


def scan(repo: Path, user_settings: Path = USER_SETTINGS) -> dict:
    """Report which committed hooks are wired, and by which settings file.

    Matching is by **base name** appearing anywhere in a hook's command string.
    That is deliberately loose: the wiring in this repo is
    ``python3 "$CLAUDE_PROJECT_DIR/.claude/hooks/no_compound_bash.py"``, and an
    operator may reasonably spell the path absolutely, relatively, or through a
    different variable. A loose match risks calling a guard wired when a typo'd
    path means it never runs; a strict one risks crying wolf on every valid
    spelling. Between a rare false negative and routine false positives, the
    false positives are worse — they are what makes a checker get ignored.
    """
    sources: dict[str, list[str]] = {}
    for name in REPO_SETTINGS:
        sources[f".claude/{name}"] = iter_hook_commands(
            load_settings(repo / ".claude" / name)
        )
    sources[str(user_settings)] = iter_hook_commands(load_settings(user_settings))

    wired: dict[str, list[str]] = {}
    unwired: list[str] = []
    for script in committed_hooks(repo):
        by = [
            source
            for source, commands in sources.items()
            if any(script in command for command in commands)
        ]
        if by:
            wired[script] = by
        else:
            unwired.append(script)

    return {
        "repo": str(repo),
        "wired": wired,
        "unwired": unwired,
        "scanned_settings": [
            source for source, commands in sources.items() if commands
        ],
    }


def render(result: dict) -> str:
    """The human report. Names the unwired scripts; prints no settings diff."""
    lines = []
    for script, sources in sorted(result["wired"].items()):
        lines.append(f"  wired    {script}  ({', '.join(sources)})")
    for script in result["unwired"]:
        lines.append(f"  UNWIRED  {script}  — committed, but nothing points at it")
    if not lines:
        lines.append("  no committed hooks found under .claude/hooks/")

    if result["unwired"]:
        # Say what to do about it. A report that only states a fact gets read
        # as noise; the whole point is that the operator, not this tool, wires
        # it — so name where the block to copy lives.
        lines.append("")
        lines.append(
            f"hook-wiring | {len(result['unwired'])} committed guard(s) never "
            "fire. Each guard's section in "
            "docs/conventions/local-integrations.md carries the PreToolUse "
            "block to paste into the main checkout's settings; wiring is the "
            "operator's call, so nothing was written."
        )
    else:
        lines.append("")
        lines.append("hook-wiring | every committed hook is wired")
    return "\n".join(lines)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="hook_wiring.py")
    parser.add_argument(
        "--repo",
        default=None,
        help="main checkout to scan (default: the worktree that is on `main`)",
    )
    parser.add_argument("--json", action="store_true", help="machine-readable output")
    args = parser.parse_args(argv[1:])

    if args.repo:
        repo = Path(args.repo)
    else:
        base = firm_core.main_checkout()
        if base is None:
            raise HookWiringError(
                "no worktree is on `main`, so the shared settings files can't "
                "be located — nothing scanned"
            )
        repo = Path(base)

    if not repo.is_dir():
        raise HookWiringError(f"--repo is not a directory: {repo}")

    result = scan(repo)
    if args.json:
        print(json.dumps(result, indent=2))
    else:
        print(render(result))
    return 1 if result["unwired"] else 0


def main() -> int:
    try:
        return run(sys.argv)
    except HookWiringError as exc:
        print(f"hook-wiring: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
