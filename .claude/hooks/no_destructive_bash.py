#!/usr/bin/env python3
# cspell:word pgdata
"""PreToolUse guard: stop catastrophic and hard-to-reverse Bash commands.

The three committed guards cover shell **form** (compounds, `git grep`) and
edit **path** (a worktree session writing the base repo). None covers command
**danger**, and the failure mode is the expensive one: a recursive delete or a
force-push is discovered after it has run.

Two tiers, deliberately:

* **ASK** — hard to reverse but legitimately wanted sometimes: a recursive
  delete, destructive SQL, a force-push, a hard reset, a volume prune. Blocked
  with a message naming what tripped, and **overridable** with the literal
  marker `#destructive-ok` in the command, so a deliberate one stays possible
  and stays auditable in the transcript.
* **DENY** — a very small catastrophic set that no marker overrides: a
  recursive delete of `/` or the home directory, and a force-push to the
  default branch.

**This is a best-effort advisory stop, not a policy boundary.** It reads one
command string and matches patterns; a determined or unusual spelling gets
through, and it is not a sandbox. Its job is to catch the slip, and it is worth
having on exactly that basis — the upstream equivalent it is modelled on is
honest about the same limit.

One inherited warning, acted on rather than merely noted: the upstream version's
command extractor was originally **grep-based** and silently truncated at an
escaped quote, letting a root delete through when it followed a quoted argument.
This hook — and the sibling compound guard — take the command from a real
`json.loads` of the PreToolUse payload, so the raw string is never re-parsed out
of a larger blob and that bug class cannot arise here. Audited and recorded so
the next editor does not reintroduce a text-extraction step.

Fails **open**: any parse problem returns 0 rather than wedging the session.
"""

import json
import re
import sys

ESCAPE_HATCH = "#destructive-ok"

# --------------------------------------------------------------------------
# DENY — no override. Kept deliberately tiny: every entry must be something
# with no legitimate use from an agent session in this repo.
# --------------------------------------------------------------------------

DENY_PATTERNS = (
    (
        re.compile(
            r"\brm\b(?=(?:\s+-\S+)*\s+-\S*r)(?=(?:\s+-\S+)*\s+-\S*f)"
            r"(?:\s+-\S+)*\s+(?:/|/\*|~|~/\*|\$HOME|\"\$HOME\"|'\$HOME')\s*$"
        ),
        "a recursive delete of the filesystem root or the home directory",
    ),
    (
        re.compile(
            r"\bgit\s+push\b.*(?:--force\b|--force-with-lease\b|(?<!\w)-f(?!\w))"
            r".*\b(?:main|master)\b"
        ),
        "a force-push to the default branch",
    ),
)

# --------------------------------------------------------------------------
# ASK — overridable with the marker.
# --------------------------------------------------------------------------

ASK_PATTERNS = (
    (
        re.compile(r"\brm\b(?=(?:\s+-\S+)*\s+-\S*r)(?=(?:\s+-\S+)*\s+-\S*f)"),
        "a recursive force-delete (`rm -rf`)",
    ),
    (
        re.compile(r"\bgit\s+push\b.*(?:--force\b|(?<!\w)-f(?!\w))"),
        "a force-push",
    ),
    (re.compile(r"\bgit\s+reset\s+--hard\b"), "a hard reset, which discards changes"),
    (
        re.compile(r"\bgit\s+clean\b(?:\s+-\S+)*\s+-\S*[fx]"),
        "a `git clean` that deletes untracked files",
    ),
    (
        re.compile(r"\bdrop\s+(?:table|database|schema)\b", re.IGNORECASE),
        "a destructive SQL DROP",
    ),
    (re.compile(r"\btruncate\s+table\b", re.IGNORECASE), "a SQL TRUNCATE"),
    (
        # DELETE with no WHERE clause anywhere after it.
        re.compile(r"\bdelete\s+from\b(?!.*\bwhere\b)", re.IGNORECASE | re.DOTALL),
        "a SQL DELETE with no WHERE clause",
    ),
    (
        re.compile(r"\bdocker\b.*\b(?:system\s+prune|volume\s+rm|volume\s+prune)\b"),
        "a docker prune or volume removal, which destroys local state",
    ),
    (
        re.compile(r"\bgit\s+branch\b(?:\s+-\S+)*\s+-\S*D"),
        "a forced branch delete",
    ),
)


def unquoted_comment(cmd):
    """The unquoted trailing shell comment (from its ``#``), or ``None``.

    A ``#`` begins a comment only when unquoted and at a word boundary, so a
    quoted or embedded occurrence of the marker cannot silently disable the
    guard. Kept identical in shape to the compound guard's version — two
    spellings of one rule is how an escape hatch drifts.
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


def classify(cmd):
    """``("deny"|"ask"|None, reason)`` for one command string."""
    for pattern, reason in DENY_PATTERNS:
        if pattern.search(cmd):
            return "deny", reason
    for pattern, reason in ASK_PATTERNS:
        if pattern.search(cmd):
            return "ask", reason
    return None, ""


DENY_MESSAGE = (
    "BLOCKED (no override): this command is {reason}.\n\n"
    "This is in the small catastrophic set that the destructive-command guard "
    "refuses outright — the `{hatch}` marker does NOT bypass it. If you truly "
    "intend this, run it yourself outside the agent session."
)

ASK_MESSAGE = (
    "Blocked: this command is {reason}, which is hard or impossible to "
    "reverse.\n\n"
    "If it is not what you meant, use a narrower command:\n"
    "  - Delete specific paths rather than a recursive force-delete.\n"
    "  - Prefer `git restore` / a WIP commit over `git reset --hard`.\n"
    "  - Scope destructive SQL with a WHERE clause, or run it against a "
    "throwaway database.\n\n"
    "If it IS deliberate, confirm with the operator first, then add the marker "
    "`{hatch}` to the command so the intent is auditable in the transcript."
)


def evaluate(payload):
    """Return ``(exit_code, message)``. Exit code 2 blocks; 0 allows."""
    if not isinstance(payload, dict):
        return 0, ""
    if payload.get("tool_name") != "Bash":
        return 0, ""
    tool_input = payload.get("tool_input") or {}
    cmd = tool_input.get("command", "") if isinstance(tool_input, dict) else ""
    if not isinstance(cmd, str) or not cmd.strip():
        return 0, ""

    # Classify the command with its trailing comment REMOVED. A shell comment
    # is inert, so this is faithful — and it closes a real bypass the self-test
    # caught: the deny patterns anchor the target path at end-of-command, so
    # `rm -rf / #destructive-ok` failed to match deny, fell through to ask, and
    # was then lifted by the very marker the deny tier must ignore.
    comment = unquoted_comment(cmd)
    effective = cmd[: len(cmd) - len(comment)] if comment is not None else cmd

    tier, reason = classify(effective)
    if tier is None:
        return 0, ""
    if tier == "deny":
        # Checked BEFORE the escape hatch, on purpose: the deny tier has no
        # override, and reading the marker first would give it one.
        return 2, DENY_MESSAGE.format(reason=reason, hatch=ESCAPE_HATCH)

    if comment is not None and ESCAPE_HATCH in comment:
        return 0, ""
    return 2, ASK_MESSAGE.format(reason=reason, hatch=ESCAPE_HATCH)


def _self_test():
    """Built-in cases, run with ``--self-test`` so it needs no piped stdin."""
    # (command, expected tier)
    cases = [
        ("ls -la", None),
        ("git status --short", None),
        ("cargo test", None),
        ("rm /tmp/one-file.txt", None),
        ("git push -u origin eng-942", None),
        ("psql -c 'DELETE FROM ticks WHERE ts < now()'", None),
        # ask tier
        ("rm -rf /tmp/scratch", "ask"),
        ("rm -fr build", "ask"),
        ("rm -r -f build", "ask"),
        ("git push --force origin eng-942", "ask"),
        ("git push -f origin eng-942", "ask"),
        ("git reset --hard HEAD~1", "ask"),
        ("git clean -fdx", "ask"),
        ("psql -c 'DROP TABLE ticks'", "ask"),
        ("psql -c 'drop database dropset'", "ask"),
        ("psql -c 'TRUNCATE TABLE ticks'", "ask"),
        ("psql -c 'DELETE FROM ticks'", "ask"),
        ("docker system prune -af", "ask"),
        ("docker volume rm dropset_pgdata", "ask"),
        ("git branch -D eng-900", "ask"),
        # deny tier
        ("rm -rf /", "deny"),
        ("rm -rf ~", "deny"),
        ("rm -rf $HOME", "deny"),
        ("git push --force origin main", "deny"),
        ("git push -f origin master", "deny"),
    ]
    failures = []
    for cmd, expected in cases:
        tier, _ = classify(cmd)
        if tier != expected:
            failures.append(f"  {cmd!r}: expected {expected}, got {tier}")

    # The escape hatch lifts `ask` but never `deny`.
    payload = {
        "tool_name": "Bash",
        "tool_input": {"command": "rm -rf build #destructive-ok"},
    }
    if evaluate(payload)[0] != 0:
        failures.append("  escape hatch did not lift the ask tier")
    payload = {
        "tool_name": "Bash",
        "tool_input": {"command": "rm -rf / #destructive-ok"},
    }
    if evaluate(payload)[0] != 2:
        failures.append("  escape hatch WRONGLY lifted the deny tier")
    # A quoted marker must not disable the guard.
    payload = {
        "tool_name": "Bash",
        "tool_input": {"command": "grep '#destructive-ok' log.txt && rm -rf build"},
    }
    if evaluate(payload)[0] != 2:
        failures.append("  a quoted marker disabled the guard")
    # Non-Bash tools are none of this hook's business.
    if evaluate({"tool_name": "Write", "tool_input": {"command": "rm -rf /"}})[0] != 0:
        failures.append("  a non-Bash tool was blocked")

    if failures:
        print("self-test FAILED:", file=sys.stderr)
        for line in failures:
            print(line, file=sys.stderr)
        return 1
    print(f"self-test passed ({len(cases)} cases + 4 hatch/scope checks)")
    return 0


def main():
    if "--self-test" in sys.argv[1:]:
        return _self_test()
    try:
        payload = json.load(sys.stdin)
    except Exception:
        # Fail open: a guard that wedges the session on malformed input is worse
        # than one that misses a command.
        return 0
    code, message = evaluate(payload)
    if message:
        print(message, file=sys.stderr)
    return code


if __name__ == "__main__":
    sys.exit(main())
