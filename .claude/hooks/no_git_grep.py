#!/usr/bin/env python3
"""PreToolUse guard: block `git grep` in Bash tool calls.

CLAUDE.md's shell rules (docs/conventions/shell-commands.md) forbid
`git grep` outright: use the Grep tool, or a bare single `grep` where the
Grep tool is absent. `git grep` looks blessed — it's a git subcommand, so
it seems covered by the `git -C <path> <sub>` cross-checkout allow-rule —
but it isn't: a clean single pattern re-prompts until firmed, and a quoted
`\\|` alternation trips the harness's per-subcommand `|` guard and can't be
firmed *at all*. Prose enforcement loses to muscle memory (one recent
session ran `git -C <path> grep` eight times), so this hook enforces the
rule mechanically, mirroring `.claude/hooks/no_compound_bash.py`.

It reads the PreToolUse payload as JSON on stdin, tokenizes the `command`
string with the guards' shared quote-aware tokenizer (`_shell_tokens.py`
beside this file), and blocks (exit 2, reason on stderr) when a command's
git subcommand is `grep` — including `git -C <path> grep`,
`git --no-pager grep`, `git -c core.pager=cat grep`, and `--flag=value`
global-option variants; after any control operator, glued or spaced, or a
newline; behind a wrapper or an assignment (`env`, `sudo`, `X=1`); and
inside a live command substitution, a `-c` payload or `eval`. A `git grep`
that is only quoted text (a commit message, a single-quoted `$(…)`) or
sits in a comment passes. A line whose quotes do not balance is scanned
word by word rather than skipped, as the shared tokenizer does for every
guard. The guard fails *open* on an unparseable payload, an internal
error, or a shared module that does not import.

There is deliberately **no escape hatch** (unlike the compound guard's
`#compound-ok`): the Grep tool — or a bare `grep` — covers every
legitimate content search, so there is no `git grep` worth letting
through.
"""

import json
import os
import sys

try:
    import _shell_tokens as sh
except ImportError:  # fail open: a missing parse must never wedge the session
    sh = None

# Nesting bound for substitutions and `-c` payloads, as in the private-key
# guard.
MAX_DEPTH = 6

# git *global* options (before the subcommand) that consume the FOLLOWING
# token as their value, so the subcommand sits one token further along. The
# `--flag=value` long form is self-contained and handled separately.
VALUE_FLAGS = {
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--super-prefix",
}


def _subcommand_is_grep(rest):
    """Given the tokens *after* a `git` word, return True iff the git
    subcommand is `grep`, skipping any leading global options."""
    i = 0
    n = len(rest)
    while i < n:
        t = rest[i]
        if t.startswith("-"):
            # `--flag=value` is self-contained.
            if t.startswith("--") and "=" in t:
                i += 1
                continue
            # `-C <path>` / `-c <cfg>` / `--git-dir <path>` eat the next token.
            if t in VALUE_FLAGS:
                i += 2
                continue
            # Any other bare flag (`--no-pager`, `-p`, `--paginate`, `--bare`).
            i += 1
            continue
        # First non-flag token is the subcommand.
        return t == "grep"
    return False


def is_git_grep(cmd, depth=0):
    """Return True iff `cmd` invokes `git grep` as a command.

    Each command the shared tokenizer finds is unwrapped to the argv that
    runs; one whose program is `git` with a `grep` subcommand blocks. The
    substitutions the shell would run, and the nested strings a wrapper
    hands a shell, are scanned the same way, to a bounded depth.
    """
    if sh is None or depth > MAX_DEPTH:
        return False
    rewritten, nested, _ = sh.raw_pass(cmd.replace("\\\n", ""))
    nested = list(nested)
    for seg, _ in sh.segments(sh.tokenize(rewritten)):
        argv, inner, _, _ = sh.unwrap(seg)
        nested.extend(inner)
        if argv and os.path.basename(argv[0]) == "git":
            if _subcommand_is_grep(argv[1:]):
                return True
    return any(
        is_git_grep(payload, depth + 1)
        for payload in nested
        if payload and payload != cmd
    )


DENY_MESSAGE = (
    "Blocked: this Bash command runs `git grep`, which CLAUDE.md forbids "
    "(docs/conventions/shell-commands.md).\n\n"
    "Use the **Grep tool** instead — it takes a real regex (alternation is "
    "`a|b|c`, not a shell-quoted `a\\|b\\|c`), searches any path you point "
    "it at, and prompts zero times. Where the Grep tool is absent (native "
    "macOS builds), a bare single `grep` is the fallback.\n\n"
    "`git grep` can't reduce to a durable allow-rule: a plain pattern "
    "re-prompts until firmed, and a quoted `\\|` alternation trips the "
    "harness's per-subcommand `|` guard and can't be firmed at all — so "
    "reach for Grep (or bare `grep`), never `git grep`."
)


def evaluate(payload):
    """Return (exit_code, message). exit_code 2 blocks; 0 allows."""
    if not isinstance(payload, dict):
        return 0, ""
    if payload.get("tool_name") != "Bash":
        return 0, ""
    tool_input = payload.get("tool_input") or {}
    cmd = tool_input.get("command", "") if isinstance(tool_input, dict) else ""
    if not isinstance(cmd, str) or not cmd.strip():
        return 0, ""
    if is_git_grep(cmd):
        return 2, DENY_MESSAGE
    return 0, ""


def _self_test():
    """Built-in cases, run with `--self-test` so it needs no piped stdin."""
    # (command, should_block)
    cases = [
        ("git grep foo", True),
        ("git grep 'a\\|b'", True),
        ("git -C /path/to/repo grep foo", True),
        ("git --no-pager grep foo", True),
        ("git -c core.pager=cat grep foo", True),
        ("git --git-dir=/x/.git grep foo", True),
        ("git --git-dir /x/.git grep foo", True),
        ("git -C /path/to/repo --no-pager grep -n bar", True),
        ("ls && git grep foo", True),
        ("git grep -n 'x' | head", True),
        # Newline-separated command on a later line is still caught.
        ("ls\ngit grep foo", True),
        # Shapes the shared tokenizer reaches: glued operators, wrappers,
        # assignments, live substitutions, nested shells.
        ("ls;git grep foo", True),
        ("(git grep foo)", True),
        ("env git grep foo", True),
        ("sudo -u x /usr/bin/git grep foo", True),
        ("GIT_PAGER=cat git grep foo", True),
        ("echo $(git grep -l foo)", True),
        ('echo "`git grep -l foo`"', True),
        ("bash -c 'git grep foo'", True),
        ("xargs git grep foo", True),
        # An unbalanced line is scanned word by word, never skipped.
        ("git grep 'unterminated", True),
        # Not git grep.
        # A git grep inside a trailing comment is inert, not a real command.
        ("git log # note ; git grep foo", False),
        # Quoted text is not a command: a commit message, a literal `$(…)`.
        ("git commit -m 'never run git grep; use Grep'", False),
        ("echo '$(git grep foo)'", False),
        ("command -v git grep", False),
        ("grep -rn foo .", False),
        ("git log --grep=foo", False),
        ("git log -n 5", False),
        ("git -C /path/to/repo status --short", False),
        ("git show HEAD", False),
        ("rg foo", False),
        ("cargo test grep", False),
        # `git` is an argument here, not a command word.
        ("echo git grep", False),
    ]
    failures = []
    for cmd, should_block in cases:
        payload = {"tool_name": "Bash", "tool_input": {"command": cmd}}
        code, _ = evaluate(payload)
        blocked = code == 2
        if blocked != should_block:
            failures.append(
                "  %-40r expected block=%s got block=%s" % (cmd, should_block, blocked)
            )
    # Non-Bash tools are never touched.
    if evaluate({"tool_name": "Read", "tool_input": {}})[0] != 0:
        failures.append("  non-Bash tool was blocked")
    if failures:
        sys.stderr.write("self-test FAILED:\n" + "\n".join(failures) + "\n")
        return 1
    sys.stdout.write("self-test OK (%d cases)\n" % len(cases))
    return 0


def main(argv):
    if "--self-test" in argv:
        return _self_test()
    try:
        payload = json.loads(sys.stdin.read())
    except Exception:
        # Fail open: any read or parse problem must never wedge the session.
        return 0
    code, message = evaluate(payload)
    if message:
        sys.stderr.write(message + "\n")
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
