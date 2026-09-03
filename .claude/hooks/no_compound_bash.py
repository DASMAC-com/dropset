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

Escape hatch: a command carrying the literal marker `#compound-ok` as a
genuine unquoted comment is let through, so a genuinely-unavoidable
compound (rare) stays possible. The marker is visible in the transcript,
which is what makes a *marked* bypass reviewable — but do not read that
as the guard recording every bypass, because a scan that finds nothing
leaves no trace at all.

**Parser audit, recorded so it is not repeated — including what the
first pass got wrong.** A sibling project's destructive-command guard
shipped a *grep-based* command extractor that silently truncated at an
escaped quote, letting a root delete through when it followed a quoted
argument. That specific bug class does not apply here: the command
string comes from a real `json.loads` of the PreToolUse payload and is
never re-extracted out of a larger blob by pattern, and the scanner
below is character-level and quote-aware rather than regex over the
whole string. If this is ever "simplified" into a regex over the raw
payload, that is the regression to look for.

**But the first audit declared the parser clean and it was not.** The
comment branch returned on the first unquoted `#`, treating the rest of
the *string* as inert while the comment beside it said "the rest of the
line" — so on a multi-line command, `ls # note` followed by
`rm -rf / && pwd` reported clean. Adversarial review of the sibling
destructive guard, where the same defect was an outright bypass of its
no-override deny tier, is what surfaced it. Both are fixed. The lesson
worth keeping: an audit that checks for one named bug class is not an
audit of the parser, and should not be recorded as one.

**And the second audit missed one too.** Fixing the comment branch left
the scanner still looking only for operator *characters*, so `ls\\npwd` —
two commands, no operator between them — was clean by construction. A
later reproduction found it directly and confirmed it live: the harness
passes raw multi-line text through in `tool_input.command` and
normalizes nothing. A newline is now a separator in its own right. The
compounding lesson: this parser has now been declared clean twice and
been wrong twice, both times about what it was not looking *for* rather
than how it looked.
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

    **A newline between two commands is itself a separator.** `ls\\npwd`
    packs two calls into one Bash invocation exactly as `ls; pwd` does, and
    the convention forbids both — but this scanner only ever looked for
    operator *characters*, so a bare two-line call had nothing to find and
    passed. Three things deliberately do not count as a second command: a
    newline inside quotes (ordinary text), a backslash-escaped newline (a
    line continuation, i.e. one command spread over two lines), and a line
    that is blank or holds only a comment.
    """
    quote = None  # None | "'" | '"'
    i = 0
    n = len(cmd)
    # `seen_content` — has the current logical line carried any command text?
    # `pending_newline` — did an earlier line, so that the next real character
    # begins a *second* command? Tracking both is what lets a trailing
    # newline and a comment-only line stay legal.
    seen_content = False
    pending_newline = False
    while i < n:
        c = cmd[i]

        if quote == "'":
            if c == "'":
                quote = None
            i += 1
            continue

        # Outside single quotes, a backslash escapes the next character —
        # including a newline, which is a line continuation (one command
        # spread over two lines) rather than a separator. Consuming both
        # characters here is what keeps a continuation legal.
        #
        # But an escaped NON-newline character is ordinary command content and
        # must run the same bookkeeping as any other content character. This
        # branch used to `i += 2; continue` unconditionally, so it skipped past
        # the `pending_newline` test and never set `seen_content` — which made
        # an all-escaped line invisible to the newline separator in both
        # directions:
        #
        #     ls\n\p\w\d      # `pending_newline` set, but no character tests it
        #     \l\s\npwd       # line 1 sets no content, so the newline is inert
        #
        # Both are two commands in bash (`\l\s` is `ls`, `\p\w\d` is `pwd`).
        # Contrived to write by accident — and this is the parser whose own
        # docstring records being "declared clean twice and wrong twice, both
        # times about what it was not looking for". This was the third, of the
        # same shape, so the bookkeeping is now shared rather than duplicated
        # into a branch that can forget it.
        if c == "\\":
            escaped = cmd[i + 1] if i + 1 < n else ""
            if escaped != "\n":
                if pending_newline:
                    return "a newline separating two commands (\\n)"
                seen_content = True
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
        if c == "\n":
            # End of a logical line. If it carried a command, the next one
            # starts a second command.
            if seen_content:
                pending_newline = True
            seen_content = False
            i += 1
            continue

        if c == "#" and (i == 0 or cmd[i - 1].isspace()):
            # An unquoted '#' starts a comment that ends at the NEWLINE — so
            # skip to it and keep scanning, rather than returning.
            #
            # This used to `return None`, which read the rest of the STRING as
            # inert. The comment beside it already said "the rest of the line",
            # so the code and its own comment disagreed: on a multi-line
            # command, `ls # note` followed by `rm -rf / && pwd` reported clean.
            # Found while auditing the sibling destructive guard, which had the
            # same defect in a place where it mattered far more.
            newline = cmd.find("\n", i)
            if newline == -1:
                return None
            # Skipping the comment also crosses its newline, so close the line
            # here exactly as the branch above would have. Comment text is not
            # command content, so a comment-only line leaves `seen_content`
            # false and adds no second command.
            if seen_content:
                pending_newline = True
            seen_content = False
            i = newline + 1
            continue

        if not c.isspace():
            if pending_newline:
                return "a newline separating two commands (\\n)"
            seen_content = True

        if c == "'":
            quote = "'"
        elif c == '"':
            quote = '"'
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
    """Return the unquoted shell comment text, or None if there is none.

    A `#` begins a comment only when unquoted and at a word boundary (start
    of string or after whitespace) — `foo#bar` and a quoted `"#x"` are
    literal text, not comments. This is what anchors the escape hatch to a
    genuine comment instead of any substring.

    Each comment ends at its NEWLINE, and on a multi-line command every
    comment is collected and joined. Previously this returned everything from
    the first `#` to end of string, which folded ordinary command text into
    "the comment" — harmless for this guard's escape-hatch lookup, but the
    same defect was a real bypass in the sibling destructive guard, so both
    are fixed together rather than left to diverge.
    """
    quote = None
    found = []
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
            newline = cmd.find("\n", i)
            if newline == -1:
                found.append(cmd[i:])
                break
            found.append(cmd[i:newline])
            i = newline + 1
            continue
        i += 1
    if found:
        return "\n".join(found)
    return None


DENY_MESSAGE = (
    "Blocked: this Bash command contains {op}, a shell compound/redirect "
    "operator that CLAUDE.md forbids. Such a command can't reduce to a "
    "reusable allow-rule, so it re-prompts on every run.\n\n"
    "Run one bare command per Bash call instead:\n"
    "  - Split `&&` / `;` chains into separate tool calls.\n"
    "  - Split a multi-line command into one tool call per line (a "
    "backslash line-continuation of a single command is fine).\n"
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
        # A newline between two commands is a separator, with or without a
        # comment in the way — the whole point is that neither line needs to
        # carry an operator for this to be two commands.
        ("ls\npwd", True),
        ("ls #x\nrm -rf /tmp/zzz", True),
        ("git log # note\nls && pwd", True),
        ("echo hi # ok\ncargo build > /tmp/x", True),
        ("ls\ncat /etc/hosts | head", True),
        # A trailing or leading newline adds no second command.
        ("ls\n", False),
        ("\nls", False),
        ("ls\n\n", False),
        # A line holding only a comment is not a second command.
        ("ls\n# note", False),
        ("# note\nls", False),
        # A backslash-escaped newline is a line continuation: one command.
        ("python3 run_quiet.py -- \\\n  python3 lint_paths.py --changed", False),
        # An escaped non-newline character is ordinary content, so it both
        # TRIPS a pending newline and SETS content for the line it sits on.
        # The escape branch used to skip the bookkeeping entirely, leaving an
        # all-escaped line invisible to the separator from either side.
        ("ls\n\\p\\w\\d", True),
        ("\\l\\s\npwd", True),
        ("\\l\\s\n\\p\\w\\d", True),
        # ...while a single escaped command on one line stays one command.
        ("\\l\\s -la", False),
        ("printf %s\\n", False),
        # A continuation whose second line is escaped is still one command.
        ("\\l\\s \\\n  -la", False),
        # A newline inside quotes is ordinary text.
        ('git commit -m "line one\nline two"', False),
        ("printf 'a\nb'", False),
        # The escape hatch still covers a deliberate multi-line command.
        ("ls\npwd #compound-ok", False),
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
