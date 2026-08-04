"""Permission-rule generalization and coverage — the source of truth.

Pure-stdlib helpers shared by ``firm_last.py`` (the ``/f`` fast-firm tool) and
``allowlist.py`` (the ``firm-perms`` / ``housekeeping`` reader), and pointed at
by the ``firm-perms`` skill's prose. Turns a just-approved tool call into the
reusable allow-rule it should have been (``generalize``), decides whether an
allowlist already covers a rule (``is_covered``), and flags the one dangerous
outcome the safety floor forbids (``is_bareverb_wildcard``).

It also owns the ``settings.local.json`` read/write pair
(``load_settings`` / ``write_settings`` / ``firm_into``) so every tool that
appends a rule does it one way — and, crucially, does it **without a prior
whole-file read** of an allowlist that can run to several hundred entries.

The generalization rules mirror ``docs/conventions/shell-commands.md`` and the
``firm-perms`` skill: widen the *variable* parts (worktree tag, trailing args)
while keeping the command + subcommand prefix literal, so a rule never grants
more verb than the approval did.
"""

# cspell:word chgrp
# cspell:word doas
# cspell:word rustup
# cspell:word setsid

from __future__ import annotations

import json
import os
import re
import shlex
from pathlib import Path
from urllib.parse import urlparse

# Programs whose bare-verb wildcard (``git:*``, ``rm:*``) would grant far more
# than any single approval, so the safety floor refuses to auto-firm one without
# a subcommand kept. Two kinds live here: programs that take subcommands (``git``,
# ``cargo``) and no-subcommand programs whose *arguments* are the whole hazard
# (``rm``, ``dd``, ``chmod``, ``curl``). ``is_bareverb_wildcard`` reads this set.
NO_BARE_WILDCARD = {
    # subcommand-taking programs
    "git",
    "gh",
    "pnpm",
    "npm",
    "npx",
    "yarn",
    "cargo",
    "rustup",
    "docker",
    "kubectl",
    "make",
    "anchor",
    "solana",
    "brew",
    "pip",
    "pip3",
    "apt",
    "apt-get",
    "systemctl",
    "go",
    "terraform",
    # no-subcommand programs whose args are the hazard
    "rm",
    "kill",
    "pkill",
    "dd",
    "chmod",
    "chown",
    "chgrp",
    "cp",
    "mv",
    "ln",
    "tee",
    "rsync",
    "scp",
    "curl",
    "wget",
    "ssh",
    "shred",
    "truncate",
}

# Command-runners / wrappers whose real verb is a *child* command they prefix —
# firming the runner (``Bash(sudo:*)``, ``Bash(env:*)``, ``Bash(xargs:*)``)
# grants arbitrary execution, and reconstructing the child safely is more than a
# fast firm should attempt. Refuse them outright (like ``cd`` / ``jq``).
_COMMAND_RUNNERS = {
    "sudo",
    "doas",
    "env",
    "xargs",
    "nohup",
    "nice",
    "timeout",
    "time",
    "watch",
    "stdbuf",
    "command",
    "setsid",
}

# Programs that never reduce to a safe rule at all.
_REFUSE_PROGRAMS = {"cd", "jq"} | _COMMAND_RUNNERS

# Interpreters: firm the *script path* they run, not a bare ``python3:*`` (which
# would subsume ``python3 -c '<arbitrary>'`` — the inline-code shape refused
# below). Any leading flag (``-c`` / ``-m`` / ``-e`` / …) can't reduce safely.
_INTERPRETERS = {
    "python",
    "python3",
    "node",
    "deno",
    "bun",
    "ruby",
    "perl",
    "bash",
    "sh",
    "zsh",
}

# Value-taking flags that name a *stable* path/dir, so they stay in the literal
# prefix with their value (``git -C <path> <sub>``, ``pnpm --dir frontend <sub>``).
_VALUE_FLAGS = {"-C", "--dir"}

# A worktree path segment: `.claude/worktrees/<tag>` -> `.claude/worktrees/*`.
_WORKTREE_RE = re.compile(r"(\.claude/worktrees/)[^/\s]+")

# Shell compound / redirect operators — a newline is a real separator too, so a
# multi-line command is a compound. (Command substitution is checked separately,
# before quotes are stripped.)
_OPERATOR_RE = re.compile(r"\|\||&&|<<|[|;<>&\n\r]")


def collapse_worktree_tags(text: str) -> str:
    """Replace any ``.claude/worktrees/<tag>`` segment with ``.claude/worktrees/*``."""
    return _WORKTREE_RE.sub(r"\1*", text)


def _has_compound(command: str) -> bool:
    """Whether the command carries a shell compound / redirect that can't reduce
    to an allow-rule. Command substitution (backtick, ``$(``) counts even inside
    quotes; the other operators are checked after quoted spans are removed, so a
    quoted ``;`` or ``|`` in a message is not a false positive.
    """
    if "`" in command or "$(" in command:
        return True
    stripped = re.sub(r"'[^']*'", "", command)
    stripped = re.sub(r'"[^"]*"', "", stripped)
    return bool(_OPERATOR_RE.search(stripped))


def _is_subcommand_word(token: str) -> bool:
    """A stable subcommand word: two-plus lowercase ASCII letters and hyphens,
    no digits — so ``status`` / ``rev-parse`` qualify but ``-A`` / ``eng-1`` /
    ``HEAD`` do not.
    """
    return bool(re.fullmatch(r"[a-z][a-z-]+", token))


def _stable_head(rest: list[str]) -> list[str]:
    """The run of leading tokens (after the program) that belong in the literal
    prefix: value-flags with their value, global long-options that precede the
    subcommand (``git --no-pager <sub>``), and subcommand words. Stops at the
    first token that looks like a variable argument.
    """
    kept: list[str] = []
    i = 0
    seen_subcommand = False
    while i < len(rest):
        tok = rest[i]
        if tok in _VALUE_FLAGS and i + 1 < len(rest):
            kept.append(tok)
            kept.append(rest[i + 1])
            i += 2
            continue
        # A global long-option before the subcommand (e.g. `git --no-pager diff`)
        # must stay in the literal prefix, or the rule won't match the command.
        if not seen_subcommand and tok.startswith("--") and "=" not in tok:
            kept.append(tok)
            i += 1
            continue
        if _is_subcommand_word(tok):
            kept.append(tok)
            seen_subcommand = True
            i += 1
            continue
        break
    return kept


def generalize_bash(command: str) -> str | None:
    """Generalize a Bash command into a ``Bash(<prefix>:*)`` rule, or ``None`` if
    the command can't reduce to a safe rule (a compound/redirect, a ``cd``, a
    ``jq``, a command-runner like ``sudo`` / ``env``, or an interpreter
    inline-code one-liner).
    """
    command = command.strip()
    if not command or _has_compound(command):
        return None
    try:
        tokens = shlex.split(command)
    except ValueError:
        return None
    if not tokens:
        return None
    prog = tokens[0]
    if prog in _REFUSE_PROGRAMS:
        return None
    if prog in _INTERPRETERS:
        # Keep the script path; refuse inline-code / module / bare-REPL forms
        # (any leading flag), which can't reduce to a rule narrower than the
        # whole interpreter.
        if len(tokens) >= 2 and not tokens[1].startswith("-"):
            return f"Bash({collapse_worktree_tags(f'{prog} {tokens[1]}')}:*)"
        return None
    literal = " ".join([prog, *_stable_head(tokens[1:])])
    literal = collapse_worktree_tags(literal)
    return f"Bash({literal}:*)"


def _make_verbatim_bash(command: str) -> str | None:
    """The exact-mode rule for a Bash command: the command verbatim (worktree
    tags still collapsed so it isn't pinned to one worktree). ``None`` for a
    compound or a program that can't be firmed even verbatim.
    """
    command = command.strip()
    if not command or _has_compound(command):
        return None
    if command.split(" ", 1)[0] in _REFUSE_PROGRAMS:
        return None
    return f"Bash({collapse_worktree_tags(command)}:*)"


def _webfetch_rule(tool_input: dict) -> str | None:
    url = tool_input.get("url")
    if not isinstance(url, str) or not url:
        return None
    host = urlparse(url).hostname
    return f"WebFetch(domain:{host})" if host else None


def _path_rule(tool_name: str, tool_input: dict) -> str | None:
    """A file-access rule (``Read(...)`` etc.) with worktree tags collapsed."""
    path = tool_input.get("file_path") or tool_input.get("notebook_path")
    if not isinstance(path, str) or not path:
        return None
    return f"{tool_name}({collapse_worktree_tags(path)})"


def generalize(tool_name: str, tool_input: dict, exact: bool = False) -> str | None:
    """The reusable allow-rule a tool call should have been, or ``None`` if the
    call can't reduce to one.

    ``exact`` keeps a Bash command verbatim (still collapsing worktree tags)
    instead of widening it to the subcommand prefix; the other tool kinds have a
    single canonical rule either way.
    """
    if not isinstance(tool_input, dict):
        tool_input = {}
    if tool_name == "Bash":
        command = tool_input.get("command", "")
        if not isinstance(command, str):
            return None
        return _make_verbatim_bash(command) if exact else generalize_bash(command)
    if tool_name == "WebFetch":
        return _webfetch_rule(tool_input)
    if tool_name == "Skill":
        name = tool_input.get("skill") or tool_input.get("name")
        return f"Skill({name})" if isinstance(name, str) and name else None
    if tool_name.startswith("mcp__"):
        # MCP tool permissions are keyed by the tool name itself, no args.
        return tool_name
    if tool_name in {"Read", "Edit", "Write", "NotebookEdit"}:
        return _path_rule(tool_name, tool_input)
    return None


def _split_rule(rule: str) -> tuple[str, str] | None:
    """Split ``Tool(inner)`` into ``(tool, inner)``; ``None`` if not that shape."""
    match = re.fullmatch(r"([A-Za-z_][\w]*)\((.*)\)", rule, re.DOTALL)
    if not match:
        return None
    return match.group(1), match.group(2)


def _bash_prefix(inner: str) -> str:
    """The literal command prefix of a Bash rule inner, dropping a trailing
    ``:*`` or `` *`` (the canonical any-args markers, which are equivalent).
    """
    if inner.endswith(":*"):
        return inner[:-2]
    if inner.endswith(" *"):
        return inner[:-2]
    return inner


def _glob_to_regex(glob: str) -> re.Pattern:
    """Translate a permission glob (``*`` = any run without ``/``-crossing for a
    single star, ``**`` = any run) into an anchored regex. A conservative
    approximation good enough for subsumption checks.
    """
    out = []
    i = 0
    while i < len(glob):
        ch = glob[i]
        if ch == "*":
            if glob[i : i + 2] == "**":
                out.append(".*")
                i += 2
                continue
            out.append("[^/]*")
            i += 1
            continue
        out.append(re.escape(ch))
        i += 1
    return re.compile("^" + "".join(out) + "$", re.DOTALL)


def is_covered(rule: str, allow_rules: list[str]) -> bool:
    """Whether ``rule`` is already granted by ``allow_rules`` — by an exact
    match, or by a broader existing rule that subsumes it.

    Subsumption handled: a Bash rule whose literal prefix is a prefix of the new
    rule's (an existing ``Bash(git:*)`` covers ``Bash(git status:*)``); a
    file-access glob that matches the new rule's path; and an exact match for the
    verbatim rule kinds (WebFetch / mcp / Skill).
    """
    if rule in allow_rules:
        return True
    parsed = _split_rule(rule)
    if parsed is None:
        return False
    tool, inner = parsed

    if tool == "Bash":
        new_prefix = _bash_prefix(inner)
        for existing in allow_rules:
            ex = _split_rule(existing)
            if ex is None or ex[0] != "Bash":
                continue
            ex_prefix = _bash_prefix(ex[1])
            if new_prefix == ex_prefix or new_prefix.startswith(ex_prefix + " "):
                return True
        return False

    if tool in {"Read", "Edit", "Write", "NotebookEdit"}:
        for existing in allow_rules:
            ex = _split_rule(existing)
            if ex is None or ex[0] != tool:
                continue
            if "*" in ex[1] and _glob_to_regex(ex[1]).match(inner):
                return True
        return False

    return False


def is_bareverb_wildcard(rule: str) -> bool:
    """Whether the rule is an over-broad bare-verb wildcard the safety floor
    forbids — a single hazardous program reduced to ``prog:*`` / ``prog *`` with
    no subcommand kept (``git:*``, ``pnpm:*``, ``rm:*``, ``curl:*``).
    """
    parsed = _split_rule(rule)
    if parsed is None or parsed[0] != "Bash":
        return False
    prefix = _bash_prefix(parsed[1]).strip()
    return prefix in NO_BARE_WILDCARD


class SettingsError(Exception):
    """The settings file exists but can't be safely rewritten."""


def load_settings(path: Path) -> tuple[dict, list[str]]:
    """Load a settings.local.json into ``(settings_dict, allow_list)``.

    A **missing** file yields empty scaffolding so a first firm can create it. A
    file that exists but does not parse raises :class:`SettingsError` instead of
    scaffolding — otherwise a stray trailing comma, or a mistyped ``--settings``
    pointing at some other JSON, would be silently *replaced* by a fresh
    ``{"permissions": {"allow": […]}}`` document, destroying whatever was there.
    The file is git-ignored, so there would be nothing to restore from.
    """
    try:
        raw = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return {}, []
    except OSError as exc:
        raise SettingsError(f"cannot read {path}: {exc}") from exc
    if not raw.strip():
        return {}, []
    try:
        settings = json.loads(raw)
    except ValueError as exc:
        raise SettingsError(
            f"{path} exists but is not valid JSON ({exc}) — refusing to "
            f"overwrite it; fix or move it by hand"
        ) from exc
    if not isinstance(settings, dict):
        raise SettingsError(
            f"{path} is valid JSON but not an object — refusing to overwrite it"
        )
    allow = settings.get("permissions", {}).get("allow")
    if not isinstance(allow, list):
        allow = []
    return settings, allow


def write_settings(path: Path, settings: dict, allow: list[str]) -> None:
    """Write the settings file **atomically**, owner-only.

    A plain ``write_text`` truncates before writing, so an interrupt or a full
    disk between truncate and flush would leave an empty or half-written
    allowlist — and since the file is git-ignored there is no copy to recover
    from. Writing a sibling temp file and ``os.replace``-ing it makes the swap
    atomic: a reader sees either the old file or the new one, never a partial.

    The mode is ``0o600`` because the file holds local configuration (extra
    directories, machine paths, any ``env`` block) and is the authority for this
    agent's own permissions.
    """
    settings.setdefault("permissions", {})
    settings["permissions"]["allow"] = allow
    path.parent.mkdir(parents=True, exist_ok=True)
    body = json.dumps(settings, indent=2, ensure_ascii=False) + "\n"

    tmp = path.with_name(path.name + ".tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(body)
            fh.flush()
            os.fsync(fh.fileno())
        os.replace(tmp, path)
    except BaseException:
        # Never leave the temp file behind to be mistaken for real settings.
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def firm_into(path: Path, rule: str) -> bool:
    """Add ``rule`` to a settings file's allow array if not already covered.
    Returns whether the file was changed.

    Writes **without** any caller having to read the file first, which is the
    whole point: an ``Edit`` requires a prior ``Read``, and reading a
    several-hundred-entry ``settings.local.json`` into context to append one line
    is exactly the cost this tool family exists to avoid.
    """
    settings, allow = load_settings(path)
    if is_covered(rule, allow):
        return False
    # Drop any existing entry the new (broader) rule now subsumes, so firming a
    # generalized rule doesn't leave the redundant narrower ones behind.
    allow = [r for r in allow if not is_covered(r, [rule])]
    allow.append(rule)
    write_settings(path, settings, allow)
    return True
