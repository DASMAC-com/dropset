"""Permission-rule generalization and coverage — the source of truth.

Pure-stdlib helpers shared by ``firm_last.py`` (the ``/f`` fast-firm tool) and
pointed at by the ``firm-perms`` skill's prose. Turns a just-approved tool call
into the reusable allow-rule it should have been (``generalize``), decides
whether an allowlist already covers a rule (``is_covered``), and flags the one
dangerous outcome the safety floor forbids (``is_bareverb_wildcard``).

The generalization rules mirror ``docs/conventions/shell-commands.md`` and the
``firm-perms`` skill: widen the *variable* parts (worktree tag, trailing args)
while keeping the command + subcommand prefix literal, so a rule never grants
more verb than the approval did.
"""

# cspell:word rustup

from __future__ import annotations

import re
import shlex
from urllib.parse import urlparse

# Programs that take subcommands, so collapsing one to a bare-verb wildcard
# (``git:*``) would grant far more than any single approval. The safety floor
# refuses to auto-firm these without a subcommand; it doubles as the set of
# verbs too dangerous to wildcard (``rm``, ``kill``).
SUBCOMMAND_PROGRAMS = {
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
    "rm",
    "kill",
    "pkill",
    "go",
    "terraform",
}

# Interpreters whose inline-code / eval forms can't reduce to a safe rule.
_INTERPRETERS = {"python", "python3", "node", "ruby", "perl", "bash", "sh", "zsh"}
_INLINE_CODE_FLAGS = {"-c", "-e", "--eval", "--command"}

# Value-taking flags that name a *stable* path/dir, so they stay in the literal
# prefix (with their value) rather than being generalized away — matching rules
# like ``git -C <path> <sub>`` and ``pnpm --dir frontend <sub>``.
_VALUE_FLAGS = {"-C", "--dir"}

# A worktree path segment: `.claude/worktrees/<tag>` -> `.claude/worktrees/*`.
_WORKTREE_RE = re.compile(r"(\.claude/worktrees/)[^/\s]+")

# Non-command-substitution shell operators (checked after quotes are stripped).
_OPERATOR_RE = re.compile(r"\|\||&&|<<|[|;<>&]")


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
    prefix: value-flags with their value, and subcommand words. Stops at the
    first token that looks like a variable argument.
    """
    kept: list[str] = []
    i = 0
    while i < len(rest):
        tok = rest[i]
        if tok in _VALUE_FLAGS and i + 1 < len(rest):
            kept.append(tok)
            kept.append(rest[i + 1])
            i += 2
            continue
        if _is_subcommand_word(tok):
            kept.append(tok)
            i += 1
            continue
        break
    return kept


def generalize_bash(command: str) -> str | None:
    """Generalize a Bash command into a ``Bash(<prefix>:*)`` rule, or ``None`` if
    the command can't reduce to a safe rule (a compound/redirect, a ``cd``, a
    ``jq`` parse, or an interpreter inline-code one-liner).
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
    if prog in {"cd", "jq"}:
        return None
    if prog in _INTERPRETERS and len(tokens) >= 2 and tokens[1] in _INLINE_CODE_FLAGS:
        return None
    literal = " ".join([prog, *_stable_head(tokens[1:])])
    literal = collapse_worktree_tags(literal)
    return f"Bash({literal}:*)"


def _make_verbatim_bash(command: str) -> str | None:
    """The exact-mode rule for a Bash command: the command verbatim (worktree
    tags still collapsed so it isn't pinned to one worktree). ``None`` for a
    compound, which can't be firmed even verbatim.
    """
    command = command.strip()
    if not command or _has_compound(command):
        return None
    if command.split(" ", 1)[0] in {"cd", "jq"}:
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
    forbids — a single subcommand-taking program reduced to ``prog:*`` /
    ``prog *`` with no subcommand kept (``git:*``, ``pnpm:*``, ``rm:*``).
    """
    parsed = _split_rule(rule)
    if parsed is None or parsed[0] != "Bash":
        return False
    prefix = _bash_prefix(parsed[1]).strip()
    return prefix in SUBCOMMAND_PROGRAMS
