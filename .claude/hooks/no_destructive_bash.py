#!/usr/bin/env python3
# cspell:word pgdata
# cspell:word fgrep
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

# A recursive-force `rm`, in either flag order. `[rR]` is deliberate: BSD/macOS
# `rm` accepts `-R` as a first-class recursive flag, and this repo runs on
# macOS — a case-sensitive `r` left `rm -Rf /` unclassified at every tier.
_RM_RECURSIVE_FORCE = r"\brm\b(?=(?:\s+-\S+)*\s+-\S*[rR])(?=(?:\s+-\S+)*\s+-\S*f)"

# The targets that make a recursive delete catastrophic rather than merely
# destructive. `~/` and `$HOME/` (with the trailing slash) are included because
# `rm -rf ~/` is a trivially plausible slip and reads as no less final.
_CATASTROPHIC_TARGET = (
    r"(?:/|/\*|~|~/|~/\*|\$HOME|\$HOME/|\$HOME/\*"
    r"|\$\{HOME\}|\$\{HOME\}/|\$\{HOME\}/\*"
    r"|\"\$HOME\"|'\$HOME'|\"\$HOME/\"|'\$HOME/')"
)

DENY_PATTERNS = (
    (
        re.compile(
            _RM_RECURSIVE_FORCE + r"(?:\s+-\S+)*\s+" + _CATASTROPHIC_TARGET + r"\s*$"
        ),
        "a recursive delete of the filesystem root or the home directory",
    ),
    (
        # Order-independent: the force flag may follow the refname just as
        # naturally as precede it, and `git push origin main --force` used to
        # fall through to the marker-liftable ask tier. The `+ref` refspec is a
        # force push with no flag at all — and it has to be matched in its FULL
        # form, not just the bare `+main`: `+refs/heads/main:refs/heads/main`
        # and `+HEAD:main` carry no flag either, and matching only the short
        # spelling left the deny tier half-closed. The optional `[\w./-]*[:/]`
        # prefix covers both a qualified refname and a `src:dst` pair; the
        # trailing lookahead keeps `+main-thing:x` (a differently-named branch)
        # out of a tier no marker can lift.
        re.compile(
            r"\bgit\s+push\b"
            r"(?=.*(?:--force\b|--force-with-lease\b|(?<!\w)-f(?!\w)"
            r"|\+(?:[\w./-]*[:/])?(?:main|master)(?=[:\s]|$)))"
            r"(?=.*\b(?:main|master)\b)"
        ),
        "a force-push to the default branch",
    ),
)

# --------------------------------------------------------------------------
# ASK — overridable with the marker.
# --------------------------------------------------------------------------

# Destructive SQL is only recognized when a SQL CLIENT is being invoked. Without
# that gate the patterns match ordinary English and block the commands this repo
# runs constantly: `git commit -m "Drop table borders in the report"` and
# `git commit -m "Delete from the dictionary the single-file words"` both tripped
# the un-gated form. A guard that blocks `git commit` is a guard that gets turned
# off, which is a worse security outcome than the one it was defending against.
_SQL_CLIENT = r"\b(?:psql|mysql|sqlite3|sqlx|pg_dump|cockroach|clickhouse)\b"

ASK_PATTERNS = (
    (
        re.compile(_RM_RECURSIVE_FORCE),
        "a recursive force-delete (`rm -rf`)",
    ),
    (
        # `[\w./-]`, not `(?:\w|/)`: a hyphen is legal in a refname and every
        # branch in this repo has one (`+eng-942:eng-942`), so the narrower
        # class matched no real refspec force-push here at all.
        re.compile(r"\bgit\s+push\b.*(?:--force\b|(?<!\w)-f(?!\w)|\+[\w./-]+:)"),
        "a force-push",
    ),
    (re.compile(r"\bgit\s+reset\s+--hard\b"), "a hard reset, which discards changes"),
    (
        # `-n` is git clean's DRY RUN. `git clean -ndx` is the recommended
        # preview and deletes nothing, so blocking it is a pure false positive.
        #
        # The exemption is deliberately narrow: it must be a SHORT-flag cluster
        # containing `n`, or the long `--dry-run`. A looser `\s-\S*n` reads any
        # dash-token with an `n` anywhere as a dry run, so
        # `git clean -fdx --exclude=node_modules` and `git clean --interactive
        # -fdx` both went unclassified — the false-positive fix opening a real
        # hole, which is the failure mode to watch for in this whole file.
        re.compile(
            r"\bgit\s+clean\b(?![^\n]*\s(?:-[a-zA-Z]*n|--dry-run\b))"
            r"(?:\s+-\S+)*\s+-\S*[fx]"
        ),
        "a `git clean` that deletes untracked files",
    ),
    (
        re.compile(
            _SQL_CLIENT + r"[^\n]*\bdrop\s+(?:table|database|schema)\b", re.IGNORECASE
        ),
        "a destructive SQL DROP",
    ),
    (
        re.compile(_SQL_CLIENT + r"[^\n]*\btruncate\s+table\b", re.IGNORECASE),
        "a SQL TRUNCATE",
    ),
    (
        # DELETE with no WHERE clause anywhere after it, in a SQL client call.
        re.compile(
            _SQL_CLIENT + r"[^\n]*\bdelete\s+from\b(?![^\n]*\bwhere\b)", re.IGNORECASE
        ),
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


def split_comments(cmd):
    """``(effective, comments)`` — the command with its comments removed.

    A ``#`` begins a comment only when unquoted and at a word boundary, so a
    quoted or embedded occurrence of the escape marker cannot silently disable
    the guard.

    **A comment ends at the NEWLINE, not at the end of the string**, and that
    distinction is load-bearing rather than pedantic. An earlier version
    returned everything from the first ``#`` onward as "the comment", so on a
    multi-line command every line after a first-line comment was stripped
    before classification:

        ls # check
        rm -rf /

    classified as ``ls`` and was **allowed** — defeating even the deny tier,
    which no marker is supposed to lift. That is the ordinary shape of a
    commented script block, not an adversarial one.

    Quote state is tracked across the whole string (faithful to a real shell,
    where a quoted string may span lines); only the comment span is bounded by
    the newline.
    """
    quote = None
    out = []
    comments = []
    i = 0
    n = len(cmd)
    while i < n:
        c = cmd[i]
        if quote == "'":
            out.append(c)
            if c == "'":
                quote = None
            i += 1
            continue
        if c == "\\":
            out.append(cmd[i : i + 2])
            i += 2
            continue
        if quote == '"':
            out.append(c)
            if c == '"':
                quote = None
            i += 1
            continue
        if c == "'":
            quote = "'"
        elif c == '"':
            quote = '"'
        elif c == "#" and (i == 0 or cmd[i - 1].isspace()):
            end = cmd.find("\n", i)
            if end == -1:
                comments.append(cmd[i:])
                break
            comments.append(cmd[i:end])
            # Keep the newline so the following line stays its own line.
            out.append("\n")
            i = end + 1
            continue
        out.append(c)
        i += 1
    return "".join(out), "\n".join(comments)


_CONTINUATION_RE = re.compile(r"\\\n[ \t]*")

# Programs whose quoted arguments are DATA — a search pattern, a regex, a format
# string — rather than shell to be run. For these, and ONLY these, a
# destructive-looking match that begins inside a quoted argument is ignored.
#
# The motivating false positive was a read-only search: a `search_source.py`
# call whose pattern happened to contain `rm -f` was denied as a recursive
# force-delete, because `rm` inside the quoted pattern paired with the `-f'` in
# it and the `r` in a later `--dir` flag to satisfy both lookaheads of
# `_RM_RECURSIVE_FORCE`. The command deletes nothing and touches nothing.
#
# Scoped to an allowlist rather than applied to all quoted spans, and the
# distinction is the whole safety argument. Suppressing every quoted match would
# let `bash -c "rm -rf /"` and `sh -c '…'` straight through, since there the
# quoted text IS shell. It would also break the deny tier outright: the
# catastrophic targets are deliberately matched in their quoted forms
# (`_CATASTROPHIC_TARGET` carries `"$HOME"` and `'$HOME'`), so `rm -rf "$HOME"`
# depends on quoted content being scanned. What is never legitimate is a
# destructive command NAME appearing inside an argument to `grep`.
READ_ONLY_PROGRAMS = frozenset(
    {
        "ack",
        "ag",
        "egrep",
        "fgrep",
        "grep",
        "read_result.py",
        "rg",
        "search_source.py",
        "show_at_ref.py",
    }
)


def quoted_spans(line):
    """``[(start, end)]`` index ranges of ``line`` that sit inside quotes.

    An UNTERMINATED quote contributes no span, deliberately. This function only
    ever suppresses a match, so the conservative direction is to report less
    quoting rather than more: a stray quote must not be a way to hide a real
    command behind it.
    """
    spans = []
    quote = None
    start = 0
    i = 0
    n = len(line)
    while i < n:
        c = line[i]
        if quote is None:
            if c == "\\":
                i += 2
                continue
            if c in "'\"":
                quote = c
                start = i + 1
        else:
            if quote == '"' and c == "\\":
                i += 2
                continue
            if c == quote:
                spans.append((start, i))
                quote = None
        i += 1
    return spans


def program_of(line):
    """The program a line invokes, for the read-only check.

    ``python3 .claude/tools/search_source.py`` reports ``search_source.py``: the
    interpreter is not the interesting name, the script it runs is.
    """
    tokens = line.strip().split()
    if not tokens:
        return ""
    program = tokens[0].rpartition("/")[2]
    if program in ("python", "python3"):
        for token in tokens[1:]:
            if not token.startswith("-"):
                return token.rpartition("/")[2]
    return program


def _matches(pattern, line):
    """Whether ``pattern`` fires on ``line``, ignoring quoted-argument text for
    the read-only programs above."""
    if program_of(line) not in READ_ONLY_PROGRAMS:
        return bool(pattern.search(line))
    spans = quoted_spans(line)
    for match in pattern.finditer(line):
        if not any(lo <= match.start() < hi for lo, hi in spans):
            return True
    return False


def classify(cmd):
    """``("deny"|"ask"|None, reason)`` for one command string.

    Each **line** is classified independently. Newline is a command separator
    in shell, so a multi-line payload is several commands — and classifying the
    whole blob as one string would let the deny patterns' end-anchors
    (``…\\s*$``) be defeated simply by appending another line.

    **A trailing backslash is the exception, so it is collapsed first.** There
    the newline is *not* a separator, and splitting on it split one command in
    two::

        rm -rf \\
          / #destructive-ok

    left ``rm -rf \\`` on its own line, which reaches only the marker-liftable
    ask tier — while the identical un-continued ``rm -rf /`` denies. Collapsing
    is also the conservative direction: it can only put more of a command in
    front of a pattern, never less.
    """
    cmd = _CONTINUATION_RE.sub(" ", cmd)
    lines = [line for line in cmd.splitlines() if line.strip()]
    for pattern, reason in DENY_PATTERNS:
        if any(_matches(pattern, line) for line in lines):
            return "deny", reason
    for pattern, reason in ASK_PATTERNS:
        if any(_matches(pattern, line) for line in lines):
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

    # Classify the command with its comments REMOVED. A shell comment is inert,
    # so this is faithful — and it closes a real bypass: the deny patterns
    # anchor the target path at end-of-line, so `rm -rf / #destructive-ok`
    # failed to match deny, fell through to ask, and was then lifted by the very
    # marker the deny tier must ignore.
    effective, comments = split_comments(cmd)

    tier, reason = classify(effective)
    if tier is None:
        return 0, ""
    if tier == "deny":
        # Checked BEFORE the escape hatch, on purpose: the deny tier has no
        # override, and reading the marker first would give it one.
        return 2, DENY_MESSAGE.format(reason=reason, hatch=ESCAPE_HATCH)

    if ESCAPE_HATCH in comments:
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
        # False positives that would get the guard turned off. `-n` is git
        # clean's DRY RUN, and destructive SQL words occur constantly in this
        # repo's commit messages.
        ("git clean -ndx", None),
        ("git clean -nx", None),
        ('git commit -m "Drop table borders in the report"', None),
        ('git commit -m "Delete from the dictionary the single-file words"', None),
        ('git commit -m "Truncate table headers to two lines"', None),
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
        # A read-only SEARCH whose quoted pattern merely CONTAINS destructive
        # text. The measured false positive: `rm` inside the pattern paired
        # with the `-f'` in it and the `r` in a later `--dir` to satisfy both
        # lookaheads of _RM_RECURSIVE_FORCE, denying a command that deletes
        # nothing.
        ("python3 .claude/tools/search_source.py 'askq|rm -f' --dir .claude", None),
        ("grep -e 'rm -rf /' -e trap /tmp/log.txt", None),
        ('grep -rn "git push --force" docs', None),
        ("rg 'git reset --hard' .claude", None),
        # ...but the allowlist must not become a bypass. The quoted text of a
        # SHELL is shell, and an unquoted destructive command on a read-only
        # program's line is still that command.
        #
        # These two land on `ask` rather than `deny` for a PRE-EXISTING reason
        # unrelated to quoted-argument scanning: the deny pattern anchors the
        # catastrophic target at end-of-line, and the closing quote sits after
        # it. Pinned at `ask` here so the case still proves the point that
        # matters — a shell's quoted payload is never suppressed — and so a
        # later change to that anchor is noticed here rather than silently.
        ('bash -c "rm -rf /"', "ask"),
        ("sh -c 'rm -rf $HOME'", "ask"),
        ('bash -c "git push --force origin main"', "deny"),
        ("grep pattern file; rm -rf /", "deny"),
        # An unterminated quote must not hide a real command behind it: the
        # span scan reports no quoting rather than swallowing the remainder.
        ("grep 'unclosed rm -rf /", "deny"),
        # A quoted CATASTROPHIC TARGET is still a real deny — the patterns match
        # `"$HOME"` deliberately, so the fix must not blank quoted content.
        ('rm -rf "$HOME"', "deny"),
        ("rm -rf '$HOME'", "deny"),
        # deny tier
        ("rm -rf /", "deny"),
        ("rm -rf ~", "deny"),
        ("rm -rf $HOME", "deny"),
        ("git push --force origin main", "deny"),
        ("git push -f origin master", "deny"),
        # BSD/macOS `rm` takes -R as a recursive flag, and this repo runs on
        # macOS. A case-sensitive `r` left every one of these unclassified.
        ("rm -Rf /", "deny"),
        ("rm -Rf ~", "deny"),
        ("rm -fR $HOME", "deny"),
        ("rm -Rf /tmp/scratch", "ask"),
        # `rm -rf ~/` is as final as `rm -rf ~` and was only `ask`.
        ("rm -rf ~/", "deny"),
        ("rm -rf $HOME/", "deny"),
        ("rm -rf ${HOME}", "deny"),
        # Flag-last is at least as natural as flag-first, and used to fall
        # through to the marker-liftable ask tier.
        ("git push origin main --force", "deny"),
        ("git push origin master -f", "deny"),
        # A refspec force-push carries no flag at all — in ANY of its
        # spellings. Matching only the short one left the rest on the
        # marker-liftable ask tier, which is the tier this rule exists to
        # escape.
        ("git push origin +main:main", "deny"),
        ("git push origin +refs/heads/main:refs/heads/main", "deny"),
        ("git push origin +HEAD:main", "deny"),
        # ...but a branch that merely STARTS with `main` is a different branch.
        ("git push origin +main-thing:main-thing", "ask"),
        # The globbed home targets, which the un-globbed pair already denied.
        ("rm -rf $HOME/*", "deny"),
        ("rm -rf ${HOME}/*", "deny"),
        ("rm -rf ~/*", "deny"),
        # The dry-run exemption must not swallow an ordinary flag containing
        # `n`: both of these DELETE, and a loose `-\\S*n` read them as previews.
        ("git clean -fdx --exclude=node_modules", "ask"),
        ("git clean --interactive -fdx", "ask"),
        ("git clean --dry-run -fdx", None),
        # A trailing backslash continues the command; the newline is not a
        # separator, so this must classify as the one command it actually is.
        ("rm -rf \\\n  /", "deny"),
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

    # A comment ends at the NEWLINE. Treating it as running to end-of-string
    # let a first-line comment swallow every following line, so an ordinary
    # commented script block bypassed the guard entirely — deny tier included.
    multiline = [
        ("ls # check\nrm -rf /", 2, "a multi-line deny slipped past a comment"),
        ("ls # check\nrm -rf build", 2, "a multi-line ask slipped past a comment"),
        ("echo one # note\necho two", 0, "a benign multi-line command was blocked"),
        # The end-anchored deny target must not be defeated by a trailing line.
        ("rm -rf /\necho done", 2, "a trailing line defeated the deny anchor"),
        # And the marker still must not lift a deny on any line.
        ("rm -rf / #destructive-ok\necho done", 2, "the marker lifted a deny"),
        # A `#` inside quotes is not a comment, so it must not hide the tail.
        ("echo '# not a comment'\nrm -rf /", 2, "a quoted '#' was read as a comment"),
    ]
    for command, expected, message in multiline:
        got = evaluate({"tool_name": "Bash", "tool_input": {"command": command}})[0]
        if got != expected:
            failures.append(f"  {message}: {command!r} -> {got}")
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
