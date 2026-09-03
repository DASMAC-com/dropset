#!/usr/bin/env python3
"""hook_wiring.py — report committed guard hooks that cannot fire.

Scope is the **guard** hooks: every ``.py`` under ``.claude/hooks/``. The
iTerm2 tab-color scripts live in ``.claude/scripts/`` and are deliberately
outside it, so read every "every committed hook" claim about this tool — here
or in the conventions — as *every committed guard hook*.

A guard script under ``.claude/hooks/`` does **nothing** until a ``PreToolUse``
entry in a settings file points at it. The scripts are committed; the wiring
deliberately is not (``settings.json`` and ``settings.local.json`` are both
git-ignored, per ``docs/conventions/local-integrations.md``). That combination
is exactly the condition under which a guard sits committed and inert
indefinitely with nobody noticing — the repo documents a protection, the script
is right there, and **nothing anywhere checks that the two are connected**.

It happened: as of 2026-08-14, of the guards committed at the time only
``no_compound_bash.py`` was wired. The worktree edit-path guard — which exists
to stop a worktree session mutating a base-repo path, a slip the conventions
call recurring and expensive — had been documented as active protection while
providing none. (That is history, not current state; the guard set has grown
since. ``make hook-wiring`` is the only statement of what is live on a given
machine, which is the whole reason this tool exists.)

**Wired is not the same as able to fire**, which is the second thing this
reports. A hook entry carries a ``matcher`` naming the tools it applies to, so
``worktree_edit_guard.py`` filed under ``"matcher": "Bash"`` is pointed at
tools it can never see — the likeliest operator slip, because each guard's
documented paste block invites copying the compound guard's ``Bash`` block and
swapping the script path. A command path that resolves to nothing is inert the
same way. Both used to report ``wired``. They now report ``MISMATCHED`` and
``MISDIRECTED``, and matching is anchored to the command's **executable
position** so a script merely *mentioned* in an argument (the shape a guard
disabled by commenting it into an ``echo`` takes) no longer counts as wiring.

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
``0`` when every committed guard hook can fire, ``1`` when at least one cannot
(unwired, mismatched, or misdirected), and ``2`` when the scan itself could
not run (no main checkout, unreadable settings). A clean scan and a broken
scan must never look alike — that is the failure this tool exists to prevent,
and it would be ironic to reproduce it.

Standard library only. A Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling"). Tests live in ``tests/test_hook_wiring.py``, run via
``make tools-tests``.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
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


# The tools each guard must be able to see. A guard filed under a matcher that
# selects none of these is inert however correct its path is — and unlike the
# deliberately-loose path matching below, this is cheaply checkable, because
# each guard's required tool set is knowable from what the guard inspects.
#
# A script not listed here is unconstrained: a new guard should be added to the
# table, but until it is, "no expectation" must mean "don't cry wolf" rather
# than a spurious MISMATCHED.
EXPECTED_TOOLS = {
    "no_compound_bash.py": frozenset({"Bash"}),
    "no_git_grep.py": frozenset({"Bash"}),
    "no_destructive_bash.py": frozenset({"Bash"}),
    "worktree_edit_guard.py": frozenset({"Edit", "Write", "MultiEdit", "NotebookEdit"}),
}

# Programs that run a script passed as an argument. Used only to decide whether
# a script name sits in executable position, never to validate the interpreter.
INTERPRETERS = frozenset({"python", "python3", "env", "uv", "sh", "bash", "zsh"})


class HookWiringError(Exception):
    """A user-facing failure: surfaced to stderr, exits 2."""


def matcher_selects(matcher, tools) -> bool:
    """Whether a hook ``matcher`` can select at least one of ``tools``.

    Claude Code treats the matcher as a regex over the tool name, with an
    absent or ``*`` matcher meaning every tool. An unparseable matcher is
    treated as selecting — this tool's job is to catch the operator slip of
    filing a guard under the wrong tool, not to lint regex syntax, and a false
    MISMATCHED is exactly the noise that gets a checker ignored.
    """
    if not tools:
        return True
    if matcher is None:
        return True
    if not isinstance(matcher, str) or matcher.strip() in ("", "*"):
        return True
    try:
        pattern = re.compile(matcher)
    except re.error:
        return True
    return any(pattern.search(tool) for tool in tools)


def script_reference(command: str, script: str) -> str | None:
    """The token that would **execute** ``script``, or None for a mere mention.

    This is the tightening that stops ``echo skipping no_compound_bash.py``
    from reporting three guards wired. A script counts when it is the command
    itself, or the argument of an interpreter; named anywhere else it is text.
    """
    if not isinstance(command, str):
        return None
    try:
        tokens = shlex.split(command, comments=False)
    except ValueError:
        # An unbalanced quote is a user-authored typo, not a reason to crash.
        tokens = command.split()
    if not tokens:
        return None
    named = [t for t in tokens if os.path.basename(t) == script]
    if not named:
        return None
    token = named[0]
    if tokens[0] == token:
        return token
    program = os.path.basename(tokens[0])
    if program in INTERPRETERS or program.startswith("python"):
        return token
    return None


def path_status(token: str, repo: Path) -> str:
    """``ok`` / ``missing`` / ``unknown`` for the script path a command names.

    ``$CLAUDE_PROJECT_DIR`` is the documented spelling and resolves to the
    scanned checkout. Any *other* unresolved variable yields ``unknown``, which
    is treated as fine — the loose-path tradeoff the original docstring owned
    is kept exactly where it was earned, and narrowed only where the answer is
    knowable.
    """
    expanded = token
    for var in ("${CLAUDE_PROJECT_DIR}", "$CLAUDE_PROJECT_DIR"):
        expanded = expanded.replace(var, str(repo))
    if "$" in expanded:
        return "unknown"
    path = Path(os.path.expanduser(expanded))
    if not path.is_absolute():
        path = repo / path
    return "ok" if path.is_file() else "missing"


def iter_hook_entries(settings: dict) -> list[tuple[object, str]]:
    """Every ``(matcher, command)`` pair in one settings mapping.

    The matcher rides along because a command alone cannot say whether the
    guard can ever fire. It is returned raw (possibly ``None``, possibly not a
    string) and interpreted by ``matcher_selects``.

    Shape-tolerant on purpose: this walks a user-authored, git-ignored file
    that no schema validates, so anything unexpected is skipped rather than
    raised on. A malformed entry should cost one missed match, not a crashed
    upkeep run.
    """
    entries: list[tuple[object, str]] = []
    hooks = settings.get("hooks")
    if not isinstance(hooks, dict):
        return entries
    for event in HOOK_EVENTS:
        for matcher_entry in hooks.get(event) or []:
            if not isinstance(matcher_entry, dict):
                continue
            matcher = matcher_entry.get("matcher")
            for hook in matcher_entry.get("hooks") or []:
                if not isinstance(hook, dict):
                    continue
                command = hook.get("command")
                if isinstance(command, str):
                    entries.append((matcher, command))
    return entries


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

    Matching is by **base name in executable position** within a hook's command
    string. The path *spelling* stays deliberately loose: the wiring in this
    repo is
    ``python3 "$CLAUDE_PROJECT_DIR/.claude/hooks/no_compound_bash.py"``, and an
    operator may reasonably spell it absolutely, relatively, or through a
    different variable. A loose match risks calling a guard wired when a typo'd
    path means it never runs; a strict one risks crying wolf on every valid
    spelling. Between a rare false negative and routine false positives, the
    false positives are worse — they are what makes a checker get ignored.

    Three outcomes are separated out from the old binary, each because the
    answer *is* knowable and reporting ``wired`` for it was false assurance:

    * ``MISMATCHED`` — a reference filed under a matcher that selects none of
      the tools the guard inspects, so it can never fire.
    * ``MISDIRECTED`` — a reference whose script path resolves to nothing.
    * ``UNWIRED`` — no reference at all (a bare mention in an argument is not
      a reference).
    """
    sources: dict[str, list[tuple[object, str]]] = {}
    for name in REPO_SETTINGS:
        sources[f".claude/{name}"] = iter_hook_entries(
            load_settings(repo / ".claude" / name)
        )
    sources[str(user_settings)] = iter_hook_entries(load_settings(user_settings))

    wired: dict[str, list[str]] = {}
    mismatched: dict[str, list[str]] = {}
    misdirected: dict[str, list[str]] = {}
    unwired: list[str] = []

    for script in committed_hooks(repo):
        expected = EXPECTED_TOOLS.get(script, frozenset())
        ok: list[str] = []
        bad_path: list[str] = []
        bad_matcher: list[str] = []
        for source, entries in sources.items():
            for matcher, command in entries:
                token = script_reference(command, script)
                if token is None:
                    continue
                if not matcher_selects(matcher, expected):
                    bad_matcher.append(source)
                elif path_status(token, repo) == "missing":
                    bad_path.append(source)
                else:
                    ok.append(source)
        # One good reference is enough — a guard wired correctly once fires,
        # whatever else also names it.
        if ok:
            wired[script] = sorted(set(ok))
        elif bad_path:
            misdirected[script] = sorted(set(bad_path))
        elif bad_matcher:
            mismatched[script] = sorted(set(bad_matcher))
        else:
            unwired.append(script)

    return {
        "repo": str(repo),
        "wired": wired,
        "mismatched": mismatched,
        "misdirected": misdirected,
        "unwired": unwired,
        "scanned_settings": [source for source, entries in sources.items() if entries],
    }


def inert_count(result: dict) -> int:
    """How many committed guards cannot fire, for any of the three reasons."""
    return (
        len(result["unwired"]) + len(result["mismatched"]) + len(result["misdirected"])
    )


def render(result: dict) -> str:
    """The human report. Names the affected scripts; prints no settings diff."""
    lines = []
    for script, sources in sorted(result["wired"].items()):
        lines.append(f"  wired    {script}  ({', '.join(sources)})")
    for script, sources in sorted(result["mismatched"].items()):
        expected = "/".join(sorted(EXPECTED_TOOLS.get(script, ())))
        lines.append(
            f"  MISMATCHED  {script}  ({', '.join(sources)}) — wired under a "
            f"matcher that never selects {expected}, so it cannot fire"
        )
    for script, sources in sorted(result["misdirected"].items()):
        lines.append(
            f"  MISDIRECTED {script}  ({', '.join(sources)}) — the command's "
            "script path resolves to nothing"
        )
    for script in result["unwired"]:
        lines.append(f"  UNWIRED  {script}  — committed, but nothing points at it")
    if not lines:
        lines.append("  no committed hooks found under .claude/hooks/")

    inert = inert_count(result)
    if inert:
        # Say what to do about it. A report that only states a fact gets read
        # as noise; the whole point is that the operator, not this tool, wires
        # it — so name where the block to copy lives.
        lines.append("")
        lines.append(
            f"hook-wiring | {inert} committed guard(s) never fire. Each "
            "guard's section in docs/conventions/local-integrations.md "
            "carries the PreToolUse block to paste into the main checkout's "
            "settings; wiring is the operator's call, so nothing was written."
        )
    else:
        lines.append("")
        lines.append("hook-wiring | every committed guard hook is wired")
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

    # Finding nothing is not a clean bill of health. Point `--repo` at a typo'd
    # path, a checkout predating `.claude/hooks/`, or a future rename, and the
    # scan legitimately has nothing to say — but returning 0 for it would be
    # byte-identical, in the one signal a caller branches on, to "every guard is
    # wired". This module's whole premise is that those two must never look
    # alike, so refusing here is not defensive padding; it is the tool declining
    # to reproduce the exact bug it was written to catch.
    if not result["wired"] and not inert_count(result):
        raise HookWiringError(
            f"no hook scripts found under {repo}/.claude/hooks/ — nothing was "
            "checked, which is not the same answer as everything being wired"
        )

    if args.json:
        print(json.dumps(result, indent=2))
    else:
        print(render(result))
    return 1 if inert_count(result) else 0


def main() -> int:
    try:
        return run(sys.argv)
    except HookWiringError as exc:
        print(f"hook-wiring: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
