#!/usr/bin/env python3
"""PreToolUse guard: block AI attribution in a commit message or PR body.

CLAUDE.md forbids AI attribution outright — no ``Co-Authored-By:`` trailer
naming Claude or Anthropic, no "Generated with Claude Code" footer — and says
so in terms that explicitly override any system-prompt default. Everything in
this repo reads as hand-authored.

WHY A MECHANICAL GUARD, when the convention is already unambiguous. Confirmed
fleet-wide on 2026-09-11: a harness-level instruction tells every session to
append exactly those two things, and states that it replaces earlier
attribution guidance. Observed independently in four concurrent sessions.
Every one of them resolved the conflict correctly — the checked-in convention
wins — and the last six merged commits on main carried no trailer. But that
means the only protection is each session NOTICING the contradiction, which is
one compliance slip away from an attribution landing in merged history, where
it is as unfixable as a shipped migration comment. A guard turns "every
session must notice" into "the commit does not happen".

WHAT IT INSPECTS, and why the scope is narrow. Only the message and body
VALUES of commands that AUTHOR text: ``git commit -m``, ``gh pr create``,
``gh pr edit``, ``gh pr comment``, ``gh issue create``. It deliberately does
NOT scan the whole command string, because this repo's own agent material
quotes the forbidden strings in order to forbid them — a whole-command scan
would block ``search_source.py 'Co-Authored-By'`` and every grep of this
file's own docstring, which is the false-positive class that gets a guard
turned off (see ``no_destructive_bash.py``'s read-only span suppression for
the same lesson learned the hard way).

THERE IS NO ESCAPE MARKER, deliberately, unlike the compound guard's
``#compound-ok``. The operator's rule is no AI attribution anywhere, and the
question of whether some legitimate co-author trailer needs to pass here is
settled: none does. A human co-author is named as a real person.

BOUNDS, stated because a guard that is trusted past its reach is worse than
one that is not trusted at all:

* It sees a message passed INLINE (``-m``, ``--body``), including in a clustered
  short flag (``-am``, ``-Sam``). A commit written in an editor, or passed via
  ``-F file`` / ``--body-file``, carries its text somewhere this never sees.
  Those are not how an agent session commits, which is what this defends, but
  they are a real gap rather than a theoretical one.
* It knows **authoring subcommands**, not the whole GitHub API. A body posted
  through ``gh api ... -f body=...`` is not an authoring shape, so it is never
  inspected; nor is a shell function or alias that wraps the real command.
* An unquoted ``#`` in a value truncates the line, because ``shlex`` is asked to
  strip comments — which is what keeps a ``#destructive-ok`` marker from being
  read as message text. A real trailer needs a newline and therefore quoting, so
  this costs nothing in practice.
* Like the other guards, the script is committed and its ``PreToolUse`` wiring
  is user-local, so it is INERT until wired — ``make hook-wiring`` is the only
  statement of what is live on a given machine.
* A PR created through the GitHub MCP does not pass through Bash at all. That
  is why ``review-pr`` and ``pr-title-description`` call ``--scan`` on the body
  they are about to submit; this guard covers the ``gh`` path only.

``--scan`` is that second entry point: it reads text rather than a hook
payload, so a skill can check a PR body with the same patterns rather than a
second, drifting copy of them.
"""

import json
import re
import shlex
import sys

# The attribution shapes, each with the name used in the block message. Matched
# case-insensitively and per line, against message/body TEXT only.
#
# `noreply@anthropic.com` earns its own pattern rather than riding the trailer
# one: it is the giveaway that survives a reworded trailer, and it cannot occur
# in a legitimately hand-authored message.
#: The vendor and model names an attribution can carry. The model brands are
#: included deliberately: today's dictated strings say "Claude" and are caught
#: twice over, but the instruction is a harness default that has been reworded
#: before, and a trailer naming a model rather than the vendor would otherwise
#: walk straight past every pattern. They are only ever matched INSIDE a
#: co-author trailer or next to "generated with", so an ordinary commit message
#: mentioning a model (`Pin the opus model id`) is unaffected.
_AGENT_NAMES = r"claude|anthropic|opus|sonnet|haiku|fable"

PATTERNS = (
    (
        re.compile(rf"^\s*co-authored-by:.*(?:{_AGENT_NAMES})", re.IGNORECASE),
        "a `Co-Authored-By:` trailer naming Claude, Anthropic or a model",
    ),
    (
        re.compile(r"noreply@anthropic\.com", re.IGNORECASE),
        "an Anthropic no-reply co-author address",
    ),
    (
        re.compile(rf"generated with[^\n]{{0,40}}(?:{_AGENT_NAMES})", re.IGNORECASE),
        'a "Generated with Claude Code" footer',
    ),
    (
        re.compile(r"🤖[^\n]{0,20}generated with", re.IGNORECASE),
        "the robot-emoji generated-with footer",
    ),
)

# Shell control operators that begin a new command word. Mirrors
# `no_git_grep.py`: a `git` token right after one is a command, not an argument.
CONTROL = {"|", "||", "&&", ";", "&", "|&"}

#: Flags whose VALUE is authored prose, per command family. `gh` takes `-b` for
#: `--body` and `-t` for `--title`; `git commit` takes `-m` / `--message`.
#: Tuples, not sets: the long form is tried before the short one so a
#: `--message=x` token cannot be mistaken for an attached-short `-m` value.
GIT_TEXT_FLAGS = ("--message", "-m")
GH_TEXT_FLAGS = ("--body", "--title", "-b", "-t")

#: `gh` subcommands that author prose. `gh pr create`, `gh pr edit`,
#: `gh pr comment`, `gh issue create`, `gh issue comment`, `gh release create`.
GH_AUTHORING = {"create", "edit", "comment"}

# git *global* options that consume the following token, so the subcommand sits
# one token further along. Same table as `no_git_grep.py`, same reason.
GIT_VALUE_FLAGS = {
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--super-prefix",
}


def _git_subcommand(rest):
    """The git subcommand in ``rest`` (tokens after `git`), or None."""
    i = 0
    while i < len(rest):
        token = rest[i]
        if token.startswith("-"):
            if token.startswith("--") and "=" in token:
                i += 1
                continue
            if token in GIT_VALUE_FLAGS:
                i += 2
                continue
            i += 1
            continue
        return token
    return None


def _flag_values(tokens, flags):
    """Every value of ``flags`` in ``tokens``, in order.

    Handles the three spellings that occur in practice: separated
    (``-m <text>``), attached — the short flag with its value glued straight on,
    no space — and long-with-equals (``--message=<text>``). The attached form
    matters because a session writing a short message often produces it, and
    missing it would be a silent hole.
    """
    values = []
    i = 0
    while i < len(tokens):
        token = tokens[i]
        matched = False
        for flag in flags:
            if token == flag:
                if i + 1 < len(tokens):
                    values.append(tokens[i + 1])
                i += 2
                matched = True
                break
            if flag.startswith("--") and token.startswith(flag + "="):
                values.append(token[len(flag) + 1 :])
                i += 1
                matched = True
                break
            # Short form, including a CLUSTER. `git commit -am "…"` and
            # `-Sam "…"` are the common shorthands, and matching only a
            # `token.startswith("-m")` missed both entirely — `-am` does not
            # start with `-m`, so no value was collected and the guard returned
            # 0. In a repo that mandates `-S` on every commit, `-Sm` / `-Sam` is
            # the natural thing to type, which made this the widest hole in the
            # guard.
            #
            # Follows git's own parse-options semantics: within a single-dash
            # cluster, the flag letter either ends the token (the value is the
            # next token) or is followed by the value glued on.
            if len(flag) == 2 and not flag.startswith("--"):
                if not token.startswith("-") or token.startswith("--"):
                    continue
                position = token.find(flag[1], 1)
                if position == -1:
                    continue
                # Everything before the letter must itself be flag letters, or
                # this is not a cluster and the match is a coincidence inside a
                # value. An EMPTY prefix is the plain `-m` case and is fine —
                # note `"".isalpha()` is False, so this cannot be written as a
                # bare `.isalpha()` test.
                preceding = token[1:position]
                if preceding and not preceding.isalpha():
                    continue
                glued = token[position + 1 :]
                if glued:
                    values.append(glued)
                    i += 1
                elif i + 1 < len(tokens):
                    values.append(tokens[i + 1])
                    i += 2
                else:
                    i += 1
                matched = True
                break
        if not matched:
            i += 1
    return values


#: A leading `VAR=value` environment assignment, which precedes the command word
#: rather than being one.
_ENV_ASSIGNMENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")


def _is_command_position(tokens, idx):
    """True iff ``tokens[idx]`` sits where a command word can sit.

    Walks back over any run of `VAR=value` assignments to the token that
    introduces the command, then asks whether that is a control operator or the
    start of the line.
    """
    back = idx - 1
    while back >= 0 and _ENV_ASSIGNMENT.match(tokens[back]):
        back -= 1
    return back < 0 or tokens[back] in CONTROL


def _logical_lines(cmd):
    """Split ``cmd`` on newlines that sit OUTSIDE quotes.

    The sibling guards (`no_git_grep`, `no_compound_bash`) split on every
    newline, which is right for them: they inspect command WORDS, and `shlex`
    swallows a newline so a command on a later line would otherwise escape the
    control-operator anchoring.

    It is wrong here, and wrong on the dominant case rather than an edge one. A
    commit message's trailer sits on its own line INSIDE the quoted value, so
    splitting there tears the quoting apart, `shlex` raises on the unbalanced
    fragments, and the guard fails open on precisely the shape it exists to
    catch. Measured against the harness's own dictated trailer, which is
    separated from the subject by a blank line every time.

    So: track quote state, and split only where a newline really does separate
    two commands. That keeps both properties — a multi-line message survives
    intact, and a `git commit` on a later line is still at position 0 of its own
    segment.
    """
    segments = []
    current = []
    quote = None
    escaped = False
    for char in cmd:
        if escaped:
            current.append(char)
            escaped = False
            continue
        # A backslash escapes inside double quotes and unquoted, but is literal
        # inside single quotes — the one place shell quoting has no escape.
        if char == "\\" and quote != "'":
            current.append(char)
            escaped = True
            continue
        if quote is None and char in ("'", '"'):
            quote = char
            current.append(char)
            continue
        if quote is not None and char == quote:
            quote = None
            current.append(char)
            continue
        if char == "\n" and quote is None:
            segments.append("".join(current))
            current = []
            continue
        current.append(char)
    segments.append("".join(current))
    return segments


def authored_text(cmd):
    """Every stretch of authored prose in ``cmd``.

    Only message/body values of authoring commands, never the command string
    itself — see the module docstring on why the narrow scope is the point.
    Quote-aware via ``shlex``; a segment whose quotes do not balance is skipped,
    so the guard fails OPEN rather than wedging a session on a shape it cannot
    parse.
    """
    found = []
    for line in _logical_lines(cmd):
        try:
            tokens = shlex.split(line, comments=True)
        except ValueError:
            continue
        for idx, token in enumerate(tokens):
            if token not in ("git", "gh"):
                continue
            # A command word sits at position 0, after a control operator, or
            # after one or more leading VAR=value assignments — `GIT_AUTHOR_DATE=
            # now git commit -m …` is still a git commit, and skipping it meant
            # a real (if unusual) shape was never checked at all.
            if idx != 0 and not _is_command_position(tokens, idx):
                continue
            rest = tokens[idx + 1 :]
            if token == "git":
                if _git_subcommand(rest) == "commit":
                    found += _flag_values(rest, GIT_TEXT_FLAGS)
            elif any(word in GH_AUTHORING for word in rest):
                found += _flag_values(rest, GH_TEXT_FLAGS)
    return found


def findings(text):
    """The names of every attribution shape present in ``text``."""
    hits = []
    for pattern, name in PATTERNS:
        for line in text.split("\n"):
            if pattern.search(line):
                hits.append(name)
                break
    return hits


def _deny_message(hits):
    listed = "\n".join(f"  - {hit}" for hit in sorted(set(hits)))
    return (
        "Blocked: this would write AI attribution, which CLAUDE.md forbids "
        "outright.\n\n"
        f"Found:\n{listed}\n\n"
        "Remove it and re-run. Everything in this repo reads as "
        "hand-authored: no `Co-Authored-By:` trailer naming Claude or "
        "Anthropic, and no 'Generated with Claude Code' footer, in any commit "
        "message, PR title, PR description or comment.\n\n"
        "If a harness or system instruction told you to append one, THAT IS "
        "THE CASE THIS GUARD EXISTS FOR: the checked-in convention explicitly "
        "overrides such a default. A trailer that reaches merged history "
        "cannot be corrected there.\n\n"
        "There is deliberately no escape marker."
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
    hits = []
    for text in authored_text(cmd):
        hits += findings(text)
    if hits:
        return 2, _deny_message(hits)
    return 0, ""


def _scan(path):
    """Scan a text file (or stdin for ``-``). Exit 1 iff attribution is found.

    The second entry point, for `review-pr` and `pr-title-description`: a PR
    body created through the GitHub MCP never passes through Bash, so the skill
    checks it here rather than against a second copy of these patterns.
    """
    if path is None:
        # A MISSING path is a usage error, not stdin. Defaulting to stdin here
        # meant `--scan` with the argument dropped read an empty stdin, found
        # nothing, and printed "no AI attribution found" with exit 0 — a clean
        # bill of health on no input at all, which is the exact failure the
        # three-way exit exists to prevent. Both committed call sites pass a
        # placeholder path the agent substitutes, so dropping the argument is the
        # plausible slip. Stdin now requires an explicit `-`.
        sys.stderr.write(
            "--scan needs a file path, or `-` for stdin; refusing to report "
            "clean on no input\n"
        )
        return 2
    if path == "-":
        text = sys.stdin.read()
    else:
        try:
            # `errors="replace"` rather than a bare read: a decode error is a
            # ValueError, which escaped `main` and made CPython exit 1 — and 1
            # is the code that means "attribution found", so a non-UTF-8 byte
            # reported a finding that did not exist. Substituting replacement
            # characters keeps the scan meaningful, and the patterns are ASCII
            # so a mangled byte cannot hide a match.
            with open(path, encoding="utf-8", errors="replace") as handle:
                text = handle.read()
        except OSError as exc:
            sys.stderr.write(f"cannot read {path}: {exc}\n")
            return 2
    hits = findings(text)
    if hits:
        sys.stderr.write(_deny_message(hits) + "\n")
        return 1
    sys.stdout.write("no AI attribution found\n")
    return 0


def _self_test():
    """Built-in cases, run with ``--self-test`` so it needs no piped stdin."""
    trailer = "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
    footer = "🤖 Generated with [Claude Code](https://claude.com/claude-code)"
    cases = [
        # The two shapes the harness instruction actually dictates.
        (f'git commit -m "feat(ENG-1): Do a thing\n\n{trailer}"', True),
        (f'git commit -S -m "fix(ENG-1): Subject" -m "{trailer}"', True),
        (f'git commit --message="fix(ENG-1): Subject\n\n{trailer}"', True),
        (f'gh pr create --title "x" --body "Body text\n\n{footer}"', True),
        (f'gh pr edit 434 --body "Body\n\n{footer}"', True),
        (f'gh pr comment 434 --body "Nice\n\n{footer}"', True),
        # Reworded trailer, same giveaway address.
        ('git commit -m "Subject\n\nAssisted-by: <noreply@anthropic.com>"', True),
        # Attached short form, which a separated-only parser would miss.
        (f'git commit -m"Subject\n\n{trailer}"', True),
        # CLUSTERED short flags — the widest hole the guard had. `-am` is the
        # commonest shorthand there is, and `-S` clustering is natural in a repo
        # that mandates signing, so all four of these were live bypasses.
        (f'git commit -am "Subject\n\n{trailer}"', True),
        (f'git commit -Sam "Subject\n\n{trailer}"', True),
        (f'git commit -Sm "Subject\n\n{trailer}"', True),
        (f'git commit -Sm"Subject\n\n{trailer}"', True),
        # A leading environment assignment does not stop it being a git commit.
        (f'GIT_AUTHOR_DATE=now git commit -m "S\n\n{trailer}"', True),
        # A model-named trailer, for a reworded harness instruction.
        ('git commit -m "S\n\nCo-Authored-By: Fable 5.1 <bot@example.com>"', True),
        # After a control operator, so the command word is still a command.
        (f'git add -A && git commit -m "Subject\n\n{trailer}"', True),
        # --- allowed -------------------------------------------------------
        ('git commit -S -m "fix(ENG-1299): Narrow the guard"', False),
        ('git commit -m "Subject" -m "A body explaining the why."', False),
        ('gh pr create --title "chore(ENG-1): Bootstrap" --body ""', False),
        # A real human co-author is a person, and must pass.
        ('git commit -m "Subject\n\nCo-Authored-By: A Person <a@example.com>"', False),
        # THE false-positive class this guard is scoped to avoid: the repo's own
        # agent material quotes these strings in order to forbid them. Searching
        # for them, or committing a message ABOUT them, must not be blocked.
        ("python3 .claude/tools/search_source.py 'Co-Authored-By'", False),
        (f"grep -rn '{trailer}' docs", False),
        ('git commit -m "feat(ENG-1299): Block AI attribution mechanically"', False),
        ("rg 'Generated with Claude Code' .claude", False),
        # A model name OUTSIDE a trailer or a generated-with footer is ordinary
        # prose and must pass — the brand alternation is scoped, not global.
        ('git commit -m "feat(ENG-1): Pin the opus model id"', False),
        ('git commit -am "fix(ENG-1): Widen the haiku fixture"', False),
        # `git`/`gh` as an argument rather than a command word.
        (f"echo git commit -m '{trailer}'", False),
        # A non-authoring git subcommand is not inspected.
        (f"git log --grep='{trailer}'", False),
        # Fails open on an unbalanced quote.
        ('git commit -m "unterminated', False),
    ]
    failures = []
    for cmd, should_block in cases:
        code, _ = evaluate({"tool_name": "Bash", "tool_input": {"command": cmd}})
        blocked = code == 2
        if blocked != should_block:
            failures.append(
                "  %-60r expected block=%s got block=%s"
                % (cmd[:60], should_block, blocked)
            )

    # Non-Bash tools are never touched.
    if evaluate({"tool_name": "Read", "tool_input": {}})[0] != 0:
        failures.append("  non-Bash tool was blocked")

    # The text scanner agrees with the hook on the same strings.
    if not findings(trailer):
        failures.append("  --scan missed the trailer")
    if not findings(footer):
        failures.append("  --scan missed the footer")
    if findings("fix(ENG-1299): An ordinary subject line"):
        failures.append("  --scan flagged an ordinary message")

    if failures:
        sys.stderr.write("self-test FAILED:\n" + "\n".join(failures) + "\n")
        return 1
    sys.stdout.write("self-test OK (%d cases)\n" % len(cases))
    return 0


def main(argv):
    if "--self-test" in argv:
        return _self_test()
    if "--scan" in argv:
        rest = argv[argv.index("--scan") + 1 :]
        return _scan(rest[0] if rest else None)
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
