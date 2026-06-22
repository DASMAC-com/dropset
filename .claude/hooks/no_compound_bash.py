#!/usr/bin/env python3
"""PreToolUse guard: block compound / redirect shell in Bash tool calls.

CLAUDE.md's shell rules require one bare command per Bash call — no pipes,
redirects, `;`, `&&` / `||`, command substitution, or backticks. The
individual sub-commands are usually already allow-listed; it is only the
*compounding* that makes each invocation unique and un-globbable, so it
re-prompts every time and `firm-perms` cannot firm it (a `*` can't
generalize a compound). This hook enforces that convention mechanically,
so a model slip can't silently produce a forever-re-prompting command.

It reads the PreToolUse payload as JSON on stdin, scans the `command`
string with a quote-aware tokenizer — operators inside single/double
quotes are legitimate text and ignored — and on a match exits 2 with a
reason on stderr, which Claude Code feeds back to the model so it can
split the command. The guard fails *open*: any parse problem returns 0
rather than wedging the session.

Escape hatch: a command carrying the literal marker `#compound-ok` is
let through, so a genuinely-unavoidable compound (rare) stays possible
and auditable in the transcript.
"""

import json
import sys

ESCAPE_HATCH = "#compound-ok"


def find_violation(cmd):
    """Name the first unquoted compound/redirect/substitution operator in
    `cmd`, or return None if the command is a single bare command.

    Quote-aware: `'…'` is fully literal; `"…"` is literal for the word
    operators (`|`, `;`, `&`, `<`, `>`) but command substitution (`$(`
    and a backtick) stays active inside double quotes, mirroring real
    shell. A backslash outside single quotes escapes the next character.
    """
    quote = None  # None | "'" | '"'
    i = 0
    n = len(cmd)
    while i < n:
        c = cmd[i]

        if quote == "'":
            if c == "'":
                quote = None
            i += 1
            continue

        # Outside single quotes, a backslash escapes the next character.
        if c == "\\":
            i += 2
            continue

        if quote == '"':
            # Command substitution is still live inside double quotes.
            if c == "`":
                return "a backtick command substitution (`)"
            if c == "$" and i + 1 < n and cmd[i + 1] == "(":
                return "a command substitution ($(…))"
            if c == '"':
                quote = None
            i += 1
            continue

        # Unquoted.
        if c == "'":
            quote = "'"
        elif c == '"':
            quote = '"'
        elif c == "#" and (i == 0 or cmd[i - 1].isspace()):
            # An unquoted '#' at a word boundary starts a shell comment; the
            # rest of the line is inert. Any operator before it would already
            # have returned, so the command is clean.
            return None
        elif c == "`":
            return "a backtick command substitution (`)"
        elif c == "$" and i + 1 < n and cmd[i + 1] == "(":
            return "a command substitution ($(…))"
        elif c == "|":
            return "a pipe or `||` (|)"
        elif c == ";":
            return "a command separator (;)"
        elif c == "&":
            return "a `&&` or background (&)"
        elif c == ">":
            return "an output redirect (>)"
        elif c == "<":
            return "an input redirect / here-doc (<)"
        i += 1

    return None


def unquoted_comment(cmd):
    """Return the unquoted trailing shell comment (from its `#`), or None.

    A `#` begins a comment only when unquoted and at a word boundary (start
    of string or after whitespace) — `foo#bar` and a quoted `"#x"` are
    literal text, not comments. This is what anchors the escape hatch to a
    genuine comment instead of any substring.
    """
    quote = None
    i = 0
    n = len(cmd)
    while i < n:
        c = cmd[i]
        if quote == "'":
            if c == "'":
                quote = None
            i += 1
            continue
        if c == "\\":
            i += 2
            continue
        if quote == '"':
            if c == '"':
                quote = None
            i += 1
            continue
        if c == "'":
            quote = "'"
        elif c == '"':
            quote = '"'
        elif c == "#" and (i == 0 or cmd[i - 1].isspace()):
            return cmd[i:]
        i += 1
    return None


DENY_MESSAGE = (
    "Blocked: this Bash command contains {op}, a shell compound/redirect "
    "operator that CLAUDE.md forbids. Such a command can't reduce to a "
    "reusable allow-rule, so it re-prompts on every run.\n\n"
    "Run one bare command per Bash call instead:\n"
    "  - Split `&&` / `;` chains into separate tool calls.\n"
    "  - Replace `>` / `>>` redirects with the Write tool, and `<` with "
    "Read.\n"
    "  - Replace pipes into sed/awk/grep/head/tail with the Read or Grep "
    "tool.\n"
    "  - Replace `$(…)` / backticks by computing the value in a prior step "
    "and passing it literally.\n"
    "  - Pass large or special-character arguments through a file "
    "(e.g. `git commit -F /tmp/msg.txt`).\n\n"
    "If a compound is genuinely unavoidable, add the marker "
    "`{hatch}` to the command to bypass this guard."
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
    # Honor the escape hatch only as a genuine unquoted trailing comment, so
    # a quoted/embedded occurrence (e.g. grepping for the literal string)
    # can't silently disable the guard.
    comment = unquoted_comment(cmd)
    if comment is not None and ESCAPE_HATCH in comment:
        return 0, ""
    op = find_violation(cmd)
    if op is None:
        return 0, ""
    return 2, DENY_MESSAGE.format(op=op, hatch=ESCAPE_HATCH)


def _self_test():
    """Built-in cases, run with `--self-test` so it needs no piped stdin."""
    # (command, should_block)
    cases = [
        ("git -C /path/to/repo status --short", False),
        ("git log -n 5", False),
        ("cargo fmt -p dropset", False),
        ('git commit -m "fix: foo; bar | baz"', False),
        ('grep -rn "a|b|c" file.txt', False),
        ("rg --glob '!*.lock' pattern", False),
        ("printf 'a>b\\n'", False),
        ("ls && pwd", True),
        ("ls; pwd", True),
        ("git ls-files | sed -n '1,5p'", True),
        ("git diff > /tmp/f.txt", True),
        ("cat <<EOF", True),
        ("echo $(date)", True),
        ('echo "$(date)"', True),
        ("echo `date`", True),
        ("cargo build &", True),
        ("foo 2>&1", True),
        ("cmd </tmp/in", True),
        ("ls && pwd #compound-ok", False),  # escape hatch (real comment)
        # A quoted/embedded marker must NOT disable the guard.
        ('grep "#compound-ok" log.txt && rm x', True),
        # An unquoted comment is inert — operators inside it don't count.
        ("git log # see notes | here", False),
        ("echo hi # plain trailing comment", False),
        # '#' mid-word is literal, not a comment.
        ("git show HEAD#nope", False),
    ]
    failures = []
    for cmd, should_block in cases:
        payload = {"tool_name": "Bash", "tool_input": {"command": cmd}}
        code, _ = evaluate(payload)
        blocked = code == 2
        if blocked != should_block:
            failures.append(
                "  %-40r expected block=%s got block=%s"
                % (cmd, should_block, blocked)
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
