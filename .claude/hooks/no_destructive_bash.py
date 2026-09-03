#!/usr/bin/env python3
# cspell:word pgdata
# cspell:word fgrep
# cspell:word spoofable
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
# `/+` rather than `/`, because a REPEATED slash is the same directory to every
# shell and to `rm`: `rm -rf //` deletes root exactly as `rm -rf /` does, and it
# reached only the marker-liftable ask tier — a one-character defeat of the
# anchor, in a spelling the sibling compound guard also passes. The `+` covers
# `///` and any longer run for free. Longest alternatives come first so the
# greedy match consumes the whole run rather than stopping after one slash.
_CATASTROPHIC_CORE = (
    r"(?:/+\*|/+"
    r"|~/+\*|~/+|~"
    r"|\$HOME/+\*|\$HOME/+|\$HOME"
    r"|\$\{HOME\}/+\*|\$\{HOME\}/+|\$\{HOME\})"
)

# The same targets, optionally wrapped in **matching** quotes, because that is
# how they arrive inside a shell invocation string. The quoted `$HOME` forms
# used to be enumerated by hand, which closed exactly the cases someone thought
# to list: `rm -rf "$HOME"` was denied while `rm -rf "/"` was not classified at
# any tier. Deriving the wrap from one core list makes that uniform.
_CATASTROPHIC_TARGET = (
    r"(?:"
    + _CATASTROPHIC_CORE
    + r"|\""
    + _CATASTROPHIC_CORE
    + r"\"|'"
    + _CATASTROPHIC_CORE
    + r"')"
)

# What may follow the target at end of line without making the command any less
# final. Without this the end anchor was defeated by a single trailing
# character: `rm -rf /` denied, while `bash -c "rm -rf /"` reached only the
# marker-liftable ask tier — the one tier a caller can lift. See `_matches` for
# the read-only suppression that keeps this from denying a search for the
# literal string.
#
# Three shapes, and the first version of this tolerated only one closing quote
# immediately adjacent to the target, which left the anchor one space or one
# quote from being defeated again:
#
#     bash -c "rm -rf / "          # a space before the closing quote
#     bash -c "sh -c 'rm -rf /'"   # two closing quotes, nested invocations
#     rm -rf / --no-preserve-root  # the spelling GNU rm actually requires
#
# The tail comes in TWO forms, and which one applies depends on whether the
# line's program is a shell. That split is the fix for a false positive the
# single tolerant form created, at the one tier no marker can lift:
#
#     git commit -m "Never run rm -rf / --no-preserve-root"
#     gh pr comment 1 --body "do not rm -rf ~ --force"
#
# Both merely QUOTE a destructive spelling as prose, and both were denied.
# Self-referentially so: `rm -rf / --no-preserve-root` is a literal line in
# this comment block, so writing a commit message about this guard was dead.
# That also contradicts this file's own stated doctrine (see `_SQL_CLIENT`):
# "a guard that blocks `git commit` is a guard that gets turned off, which is a
# worse security outcome than the one it was defending against."
#
# The read-only suppression in `_matches` does not save these — `git` and `gh`
# are not, and should not be, in `READ_ONLY_PROGRAMS`. What separates
# `bash -c "rm -rf /"` from `git commit -m "rm -rf /"` is not the quoting but
# the PROGRAM: one hands the string to a shell, the other stores it. So a
# trailing closing quote is tolerated only for a shell.
#
# Trailing FLAGS are tolerated in both, because `rm -rf / --no-preserve-root`
# is the spelling GNU rm actually requires and a bare `rm` is not a shell.
#
# A trailing flag is `-[^\s"']+`, NOT `-\S+`. `\S` matches a quote, so a greedy
# `-\S+` swallowed the closing quote as part of the flag token and the strict
# tail then matched anyway — which is how `git commit -m "… rm -rf /
# --no-preserve-root"` still denied after the shell split. Excluding quotes from
# the flag class is what makes the two tails actually differ.
_RM_TAIL_STRICT = r"(?:\s+-[^\s\"']+)*\s*$"
_RM_TAIL_SHELL = r"(?:\s+-[^\s\"']+)*[\s\"']*$"

# Programs that hand a quoted argument to a shell for execution. Only these get
# the quote-tolerant tail. An allowlist, not a denylist of "safe" programs: the
# executor long tail (`ssh`, `sudo`, `find -exec`, `xargs`, `timeout`) is what
# a denylist would have to enumerate, and missing one fails open.
SHELL_PROGRAMS = frozenset({"sh", "bash", "zsh", "dash", "ksh", "ash", "fish"})

# Denies that apply on EVERY line, whatever the program.
DENY_PATTERNS = (
    (
        re.compile(
            _RM_RECURSIVE_FORCE
            + r"(?:\s+-\S+)*\s+"
            + _CATASTROPHIC_TARGET
            + _RM_TAIL_STRICT
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
# Denies that apply only on a line whose program is in ``SHELL_PROGRAMS``. The
# quote-tolerant tail lives here rather than in `DENY_PATTERNS` because a
# closing quote after the target means "a shell was handed this string" only
# when a shell is what is being invoked; for `git commit -m` or `gh pr comment`
# the same shape is prose being stored, and denying it at the tier no marker
# lifts was a
# live false positive. See `_RM_TAIL_SHELL`.
SHELL_DENY_PATTERNS = (
    (
        re.compile(
            _RM_RECURSIVE_FORCE
            + r"(?:\s+-\S+)*\s+"
            + _CATASTROPHIC_TARGET
            + _RM_TAIL_SHELL
        ),
        "a recursive delete of the filesystem root or the home directory",
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
# Scoped three ways, and each narrowing is load-bearing.
#
# **By program**, to this allowlist. Suppressing every quoted match would let
# `bash -c "rm -rf /"` and `sh -c '…'` straight through, since there the quoted
# text IS shell.
#
# **By tier** — the ASK tier only; see `classify`. The measured false positive
# was an ask-tier `rm -rf` match, so suppression buys nothing on the deny tier,
# and the deny tier is where being wrong is unrecoverable. It would also break
# outright there: the catastrophic targets are deliberately matched in their
# quoted forms (`_CATASTROPHIC_TARGET` carries `"$HOME"` and `'$HOME'`), so
# `rm -rf "$HOME"` depends on quoted content being scanned.
#
# **By quote kind** — see `inert_spans`. A DOUBLE-quoted argument is not inert:
# `$(…)`, a backtick and `${x:-$(…)}` all execute inside one. Treating it as
# data made `grep "$(git push --force origin main)" f` — which really does run
# the push — classify clean, and that string had previously been a deny. Caught
# in adversarial review of this very change, which is why the comment now
# describes what the code holds rather than the stronger property it read as.
#
# What this buys is narrow and worth stating exactly, because the earlier
# wording claimed more than the code holds: on a line that is a SINGLE SIMPLE
# COMMAND invoking one of these programs, a destructive command name inside a
# quoted argument is data. The code does not verify that the span is the
# *pattern* argument specifically — it suppresses any qualifying quoted span on
# such a line — so the guarantee rests on the line having no second command,
# which `_RE_EVALUATES` is what enforces.
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


# Text that makes a DOUBLE-quoted span executable rather than literal: command
# substitution, in both spellings. `${x:-$(…)}` is covered because it contains
# `$(` — the substitution is what runs, not the braces around it.
#
# **This was a bare `$` and had to be narrowed.** The old comment argued that
# `$` was a cheap over-approximation whose "failure direction is safe — an
# excluded span is simply scanned as before". That was true only while the deny
# tier ignored quoting entirely: a span scanned as before could reach the ask
# tier at worst. Making the deny tier honor suppression AND letting a target be
# followed by a closing quote invalidated the argument in the same change,
# turning the over-approximation into a live regression:
#
#     rg "rm -rf $HOME"        # deletes nothing; searches for a literal string
#
# `program_of` is `rg`, so suppression is attempted — but the span held a `$`,
# so it was not inert, and the deny pattern then matched `$HOME` plus the
# closing quote at end of line. An **un-overridable** deny on a read-only
# search, and self-referentially so: auditing this guard means grepping the
# repo for exactly that string.
#
# A bare parameter expansion (`$HOME`, `${HOME}`) expands to a *value* and runs
# nothing, so it does not belong in this tuple. What runs is a substitution, and
# only these two spellings introduce one. The narrowing cannot weaken a real
# deny: suppression applies only to `READ_ONLY_PROGRAMS`, where the span is an
# argument to a search tool, and `rm -rf "$HOME"` is unaffected because `rm` is
# not on that allowlist.
_LIVE_IN_DOUBLE_QUOTES = ("$(", "`")

# Constructs that make a quoted span executable no matter WHICH quote it used,
# by handing it back to a shell later on the same line. Their presence disables
# suppression for the whole line.
#
# This is the second half of the same lesson as `_LIVE_IN_DOUBLE_QUOTES`, and it
# was missed the first time. Suppression is applied per LINE once token 0 is
# allowlisted — not to a pattern argument — so a *single*-quoted span is literal
# only to the shell's tokenizer, and `eval`, `sh -c` or `xargs sh -c` later on
# the line re-evaluates it. All four of these classified clean before this
# check, and each genuinely runs the destructive operation:
#
#     grep -rl OLD src | xargs -n1 sh -c 'rm -rf build'
#     rg -l OLD src | xargs -n1 sh -c 'git push --force origin feature'
#     grep x f; eval 'rm -rf build'
#     grep -c x f && sh -c 'rm -rf build'
#
# The sibling compound guard blocks every one of them on the separator alone,
# but each guard is wired independently and that one has an escape marker, so
# this guard must not lean on it.
_RE_EVALUATES = re.compile(r"(?:[|;&]|\beval\b|\bxargs\b|\b(?:ba|z)?sh\b|\bsource\b)")


def quoted_spans(line):
    """``[(start, end, quote)]`` index ranges of ``line`` that sit inside quotes.

    An UNTERMINATED quote contributes no span, deliberately. This function only
    ever leads to suppressing a match, so the conservative direction is to
    report less quoting rather than more: a stray quote must not be a way to
    hide a real command behind it.
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
                spans.append((start, i, quote))
                quote = None
        i += 1
    return spans


def inert_spans(line):
    """The quoted spans of ``line`` that no shell on this line will execute.

    Two conditions, and the second is the one that is easy to get wrong.

    A **double**-quoted span is literal only without command substitution or
    expansion: `$(…)`, a backtick and `${x:-$(…)}` all RUN inside double
    quotes, so treating such a span as data is what turned
    `grep "$(git push --force origin main)" f` from a deny into a clean
    verdict.

    And **no** span on the line is literal — single quotes included — once
    something on that line hands a string back to a shell. Suppression applies
    per line, so `eval`, `sh -c` and `xargs sh -c` re-evaluate a span the
    tokenizer treated as literal.

    Both were real, reproducible bypasses found by adversarial review of this
    guard's own change, one round apart. Every variant is pinned in the
    self-test below; the docstring is deliberately explicit about what is
    *not* claimed, because the previous version asserted the first condition
    alone and read as covering both.
    """
    spans = quoted_spans(line)

    # Look for the re-evaluating construct OUTSIDE the quotes only. Scanning the
    # raw line breaks the very case this carve-out exists for: the measured
    # false positive is `search_source.py 'askq|rm -f'`, whose pattern contains
    # a `|` that is data, not a pipe. An unquoted separator means a second
    # command; a quoted one means nothing at all.
    def quoted(index):
        return any(lo <= index < hi for lo, hi, _ in spans)

    for match in _RE_EVALUATES.finditer(line):
        if not quoted(match.start()):
            return []

    return [
        (lo, hi)
        for lo, hi, quote in spans
        if quote == "'" or not any(t in line[lo:hi] for t in _LIVE_IN_DOUBLE_QUOTES)
    ]


def program_of(line):
    """The program a line invokes, for the read-only check.

    ``python3 .claude/tools/search_source.py`` reports ``search_source.py``: the
    interpreter is not the interesting name, the script it runs is. But `-m`
    names a MODULE rather than a path, so `python3 -m grep` is not this repo's
    `grep` and must not reach the allowlist — the interpreter is reported
    instead, which is not allowlisted.
    """
    tokens = line.strip().split()
    if not tokens:
        return ""
    program = tokens[0].rpartition("/")[2]
    if program in ("python", "python3"):
        for token in tokens[1:]:
            if token == "-m":
                return program
            if not token.startswith("-"):
                return token.rpartition("/")[2]
    return program


def _matches(pattern, line, allow_quoted=True):
    """Whether ``pattern`` fires on ``line``.

    ``allow_quoted`` suppresses a match inside an inert quoted span, and only
    on a line whose program is one of ``READ_ONLY_PROGRAMS``.

    **It now applies on the DENY tier too, which it deliberately did not
    before.** The old rationale was that the measured false positive was an
    ask-tier match, so the carve-out bought nothing on deny — true while the
    deny patterns anchored their target at end-of-line, because a search whose
    quoted pattern ended the line could not match anyway. Tolerating a trailing
    tail (`_TRAILING_TAIL`, so `bash -c "rm -rf /"` is caught) removes that
    accidental protection: without suppression, `grep -rn "rm -rf /"` — a
    search for the literal string, deleting nothing — would become an
    **un-overridable** deny. Closing the shell-invocation hole must not buy
    that, so the two changes are one change.

    What keeps this from weakening the deny tier: no shell is in
    ``READ_ONLY_PROGRAMS``, so `bash -c` / `sh -c` payloads are never
    suppressed; a double-quoted span containing a command substitution is not
    inert; an unterminated quote yields no spans; and a line that hands content
    back to a shell disables suppression wholesale. An unquoted destructive
    command sharing a read-only program's line is still matched.

    **The bound worth stating, because it is the one this cannot close.**
    ``program_of`` reports a *basename*, so the allowlist is spoofable: a file
    the agent could itself create at `tools/rg` makes
    `python3 tools/rg 'rm -rf ~'` suppress at both tiers. Trusting the basename
    is not an oversight — it is what lets `python3 .claude/tools/search_source.py`
    reach the allowlist at all, which is the carve-out's entire purpose — and no
    stock program on the list has an exec vector, so a spoof buys silence rather
    than a delete. This guard is best-effort advisory, not a policy boundary
    (`CLAUDE.md` → "Local integrations and guard hooks"), and a session that can
    write arbitrary files and run them is already past it. Recorded so a future
    round does not mistake it for an unexamined hole.
    """
    if not allow_quoted or program_of(line) not in READ_ONLY_PROGRAMS:
        return bool(pattern.search(line))
    spans = inert_spans(line)
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
    ask tier — while the identical un-continued ``rm -rf /`` denies.

    Collapsing must **delete** the continuation, not replace it with a space.
    This used to claim substituting a space was "the conservative direction: it
    can only put more of a command in front of a pattern, never less", which is
    false — a space SPLITS a token the shell would have joined, and a split
    token puts less. ``rm -r\\<newline>f /`` classified clean at every tier
    while both bash and zsh ran ``rm -rf /``.
    """
    # DELETE the continuation, do not replace it with a space. Every shell
    # removes `\`+newline and rejoins the surrounding characters into one
    # token; substituting a space SPLIT the token instead, so the guard saw a
    # different command than the shell would run:
    #
    #     rm -r\<newline>f /     # guard saw `rm -r f /` — unclassified at
    #                           # EVERY tier, while both bash and zsh run
    #                           # `rm -rf /`
    #
    # One character defeated the whole deny tier, and the sibling compound
    # guard passes it too (a continuation is a legal single command). The old
    # docstring claimed collapsing "can only put more of a command in front of
    # a pattern, never less" — false, because splitting a token puts less.
    cmd = _CONTINUATION_RE.sub("", cmd)
    lines = [line for line in cmd.splitlines() if line.strip()]
    for pattern, reason in DENY_PATTERNS:
        # The read-only carve-out applies here too — see `_matches`. It has to,
        # now that a trailing quote no longer defeats the end anchor.
        if any(_matches(pattern, line) for line in lines):
            return "deny", reason
    # Then the shell-only denies, on shell lines only. A closing quote after
    # the target is evidence a shell was handed the string — but only when a
    # shell is what the line invokes.
    shell_lines = [line for line in lines if program_of(line) in SHELL_PROGRAMS]
    for pattern, reason in SHELL_DENY_PATTERNS:
        if any(_matches(pattern, line) for line in shell_lines):
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
        # A shell's quoted payload is shell. These used to land on `ask` — the
        # deny pattern anchored the catastrophic target at end-of-line and the
        # closing quote sat after it, so one trailing character demoted a
        # recursive delete of root into the one tier a marker can lift. The
        # anchor now tolerates that quote.
        ('bash -c "rm -rf /"', "deny"),
        ("sh -c 'rm -rf $HOME'", "deny"),
        ('bash -c "rm -rf ~/"', "deny"),
        ("zsh -c 'rm -rf /'", "deny"),
        # A directly-quoted target, which the hand-enumerated quoted forms
        # missed entirely: `rm -rf "$HOME"` denied while `rm -rf "/"` was
        # unclassified at every tier.
        ('rm -rf "/"', "deny"),
        ("rm -rf '/'", "deny"),
        ('rm -rf "~/"', "deny"),
        # ...and the flip side of tolerating that quote: a read-only SEARCH for
        # the literal string, with the pattern ending the line, must NOT become
        # an un-overridable deny. The end anchor used to protect this case by
        # accident; the read-only span suppression now protects it on purpose.
        ('grep -rn "rm -rf /"', None),
        ("rg 'rm -rf /'", None),
        ("grep -rn \"rm -rf '/'\"", None),
        # The `$`-bearing half of that same case, which the three cases above
        # do NOT cover: every one of them is `$`-free, so they proved the safe
        # half only. A double-quoted span holding `$HOME` was treated as live
        # (the old `_LIVE_IN_DOUBLE_QUOTES` held a bare `$`), suppression was
        # therefore skipped, and the tolerated closing quote let the deny fire —
        # an un-overridable deny on a search that deletes nothing. Parameter
        # expansion runs no command, so only a substitution makes a span live.
        ('rg "rm -rf $HOME"', None),
        ('grep -rn "rm -rf ${HOME}"', None),
        ('grep -rn "rm -rf $HOME/*"', None),
        # ...while a substitution in the same position still disables
        # suppression, because THAT one runs.
        ('grep "$(git push --force origin main)" f', "deny"),
        # The end anchor must not be defeated by one space or one extra quote.
        # Both of these force-delete root and both reached only the ask tier
        # while the tolerance was a single optional adjacent quote.
        ('bash -c "rm -rf / "', "deny"),
        ("bash -c \"sh -c 'rm -rf /'\"", "deny"),
        # The spelling GNU `rm` actually requires in order to delete root, which
        # a bare end-anchor never caught at the deny tier at all.
        ("rm -rf / --no-preserve-root", "deny"),
        ('bash -c "rm -rf / --no-preserve-root"', "deny"),
        # But a trailing bare PATH still is not this pattern: tolerating flags
        # rather than arbitrary words is what keeps a literal-string search from
        # matching even before suppression is consulted.
        ('grep -rn "rm -rf /" notes.txt', None),
        # A repeated slash is the same directory to `rm`, so `//` deletes root
        # exactly as `/` does. It reached only the liftable ask tier — a
        # one-character defeat of the anchor that the compound guard passes too.
        ("rm -rf //", "deny"),
        ("rm -rf ///", "deny"),
        ('bash -c "rm -rf //"', "deny"),
        # A line continuation is DELETED by every shell, not replaced with a
        # space. Collapsing it to a space split the token, so the guard saw
        # `rm -r f /` — unclassified at every tier — while the shell ran
        # `rm -rf /`. One character, total deny-tier bypass.
        ("rm -r\\\nf /", "deny"),
        ("rm -rf \\\n  /", "deny"),
        ("rm -r\\\nf \\\n/", "deny"),
        # PROSE that merely quotes a destructive spelling must never reach the
        # un-overridable tier. `git` and `gh` are not in READ_ONLY_PROGRAMS and
        # must not be, so suppression cannot save these — what separates them
        # from `bash -c "…"` is the PROGRAM, which is why the quote-tolerant
        # tail is shell-only. Self-referential: `rm -rf / --no-preserve-root` is
        # a literal line in this file's own comments, so denying these made it
        # impossible to write a commit message about this guard.
        ('git commit -m "Never run rm -rf / --no-preserve-root"', "ask"),
        ('git commit -m "rm -rf ~ -n"', "ask"),
        ('echo "rm -rf / --no-preserve-root"', "ask"),
        (
            "python3 .claude/tools/linear_issue.py append --text 'rm -rf / --force'",
            "ask",
        ),
        ('gh pr comment 1 --body "do not rm -rf ~ --force"', "ask"),
        ('bash -c "git push --force origin main"', "deny"),
        ("grep pattern file; rm -rf /", "deny"),
        # An unterminated quote must not hide a real command behind it: the
        # span scan reports no quoting rather than swallowing the remainder.
        ("grep 'unclosed rm -rf /", "deny"),
        # A quoted CATASTROPHIC TARGET is still a real deny — the patterns match
        # `"$HOME"` deliberately, so the fix must not blank quoted content.
        # (These pin pre-existing behavior only: `rm` is not an allowlisted
        # program, so `_matches` short-circuits and the span scan never runs.
        # The cases that actually exercise suppression are the `grep`/`rg` ones.)
        ('rm -rf "$HOME"', "deny"),
        ("rm -rf '$HOME'", "deny"),
        # COMMAND SUBSTITUTION inside a double-quoted argument EXECUTES. Treating
        # such a span as inert data was a real bypass — `grep "$(git push
        # --force origin main)" f` really does run the push, and it had
        # previously been an un-overridable deny. Every variant is pinned.
        ('grep "$(git push --force origin main)" f', "deny"),
        ('grep "$(rm -rf ~)" file', "ask"),
        ('grep "`rm -rf ~`" file', "ask"),
        ('grep "${x:-$(rm -rf ~)}" file', "ask"),
        ('rg "$(rm -rf ~)" .', "ask"),
        ('python3 .claude/tools/search_source.py "$(rm -rf ~)"', "ask"),
        # `-m` names a module, not this repo's tool, so it must not reach the
        # allowlist.
        ('python3 -m grep "$(rm -rf ~)"', "ask"),
        # A DOUBLE-quoted pattern with no substitution is still inert, so the
        # carve-out keeps working for the ordinary case.
        ('grep "rm -rf /" /tmp/log.txt', None),
        # A SINGLE-quoted span is literal to the tokenizer but not to a shell
        # invoked later on the same line. Suppression is per line, so `eval`,
        # `sh -c` and `xargs sh -c` re-evaluate it — each of these runs the
        # destructive operation for real and classified clean before the
        # re-evaluation check.
        ("grep -rl OLD src | xargs -n1 sh -c 'rm -rf build'", "ask"),
        ("rg -l OLD src | xargs -n1 sh -c 'git push --force origin feature'", "ask"),
        ("grep x f; eval 'rm -rf build'", "ask"),
        ("grep -c x f && sh -c 'rm -rf build'", "ask"),
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
