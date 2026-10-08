#!/usr/bin/env python3
# cspell:word pgdata
# cspell:word fgrep
# cspell:word spoofable
# cspell:word ionice
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
  recursive delete of `/` or the home directory (force flag or not; for an
  `rm` actually being run, the target anywhere among its operands),
  and a force-push to the default branch.

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
import os
import re
import sys

ESCAPE_HATCH = "#destructive-ok"

# --------------------------------------------------------------------------
# git global options. Every git pattern below is built on `_GIT`, never on a
# bare `\bgit\s+`, because a global option between `git` and its subcommand
# otherwise defeats the match at every tier — `git -C /x push --force origin
# main` classified clean, deny tier included. That is not an unusual spelling
# this guard can shrug off as best-effort: `git -C <path>` is a shape the
# repo's own skills prescribe.
#
# The value-taking set is the same table `no_git_grep.py` and
# `no_ai_attribution.py` carry (each hook is a standalone script, so it is
# copied rather than imported). Every other dash token is a valueless global
# (`--no-pager`, `-P`, `--bare`) or a `--name=value` spelling.
#
# The prefix is WIDENED rather than the line rewritten, deliberately: `_matches`
# compares match offsets against quoted-span offsets, and a normalized line
# would no longer share them.
# --------------------------------------------------------------------------

#
# `--config-env` is one this hook adds: it takes a separate `name=envvar` value
# too, so without it `git --config-env x=Y push --force origin main` read the
# value as the subcommand slot and missed.
#
# An unquoted value may not START with a dash. Without that, a `-c` token
# matched both alternatives and the next `-c` could be its value, so a run of
# them backtracked exponentially — thirty took 1.5s, a cheap way to stall the
# hook. Rejecting a dash-led value leaves each token one reading.
GIT_VALUE_FLAGS = (
    "--config-env",
    "--git-dir",
    "--namespace",
    "--super-prefix",
    "--work-tree",
    "-C",
    "-c",
)
_GIT_OPTION_VALUE = r"(?:\"[^\"]*\"|'[^']*'|[^\s\"'-][^\s\"']*)"
_GIT = (
    r"\bgit(?:\s+(?:(?:"
    + "|".join(re.escape(flag) for flag in GIT_VALUE_FLAGS)
    + r")\s+"
    + _GIT_OPTION_VALUE
    + r"|-[^\s\"'=]+(?:="
    + _GIT_OPTION_VALUE
    + r")?))*\s+"
)

# --------------------------------------------------------------------------
# DENY — no override. Kept deliberately tiny: every entry must be something
# with no legitimate use from an agent session in this repo.
# --------------------------------------------------------------------------

# A recursive `rm`, force or not. `[rR]` is deliberate: BSD/macOS `rm` accepts
# `-R` as a first-class recursive flag, and this repo runs on macOS — a
# case-sensitive `r` left `rm -Rf /` unclassified at every tier.
#
# The DENY tier is built on this, not on `_RM_RECURSIVE_FORCE`. Requiring a force
# flag there left `rm -r ~` and `rm -R /` unclassified at every tier, which
# contradicts what this module says the deny set is. And `-f` adds nothing to
# the destructive power from an agent session: the Bash tool's stdin is not a
# terminal, so BSD `rm` removes write-protected files without prompting anyway.
#
# A recursive flag is a SHORT cluster holding `r`/`R`, or `--recursive`. The
# `(?!-)` matters: `-\S*[rR]` alone read the `r` in `--force` and
# `--no-preserve-root` as recursion.
_RM_RECURSIVE = r"\brm\b(?=(?:\s+-\S+)*\s+(?:-(?!-)\S*[rR]|--recursive\b))"

# A recursive-force `rm`, in either flag order — the ASK tier. A plain `rm -r
# <dir>` stays unclassified there; flagging every recursive delete of a build
# directory would train the operator to approve without reading.
_RM_RECURSIVE_FORCE = _RM_RECURSIVE + r"(?=(?:\s+-\S+)*\s+-\S*f)"

# The targets that make a recursive delete catastrophic rather than merely
# destructive. `~/` and `$HOME/` (with the trailing slash) are included because
# `rm -rf ~/` is a trivially plausible slip and reads as no less final.
# `/+` rather than `/`, because a REPEATED slash is the same directory to every
# shell and to `rm`: `rm -rf //` deletes root exactly as `rm -rf /` does, and it
# reached only the marker-liftable ask tier — a one-character defeat of the
# anchor, in a spelling the sibling compound guard also passes. The `+` covers
# `///` and any longer run for free. Longest alternatives come first so the
# greedy match consumes the whole run rather than stopping after one slash.
#
# The home directory's ABSOLUTE spelling is resolved at import time and joins
# the list, because agents here are told to prefer absolute paths:
# `rm -rf /Users/<user>/` reached only the marker-liftable ask tier. A HOME
# that does not resolve to an absolute path other than `/` adds nothing.
_HOME = os.path.expanduser("~").rstrip("/")
_ABSOLUTE_HOME = (
    rf"|{re.escape(_HOME)}/+\*|{re.escape(_HOME)}/+|{re.escape(_HOME)}"
    if _HOME.startswith("/")
    else ""
)
_CATASTROPHIC_CORE = (
    r"(?:/+\*|/+"
    r"|~/+\*|~/+|~"
    r"|\$HOME/+\*|\$HOME/+|\$HOME"
    r"|\$\{HOME\}/+\*|\$\{HOME\}/+|\$\{HOME\}" + _ABSOLUTE_HOME + r")"
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

# At COMMAND POSITION a quoted target may also continue past its closing quote
# with `/`, `/*` or `*`: `rm -r "$HOME"/*` and `rm -rf "$HOME/"*` empty the
# home directory, and were unclassified or asked. Only there, because the
# flags-only shape runs on every line, prose included: a message line reading
# `rm -r "$HOME"/*` must not become an un-overridable deny.
_CATASTROPHIC_COMMAND_TARGET = (
    r"(?:"
    + _CATASTROPHIC_CORE
    + r"|(?:\""
    + _CATASTROPHIC_CORE
    + r"\"|'"
    + _CATASTROPHIC_CORE
    + r"')(?:/+\*|/+|\*)?)"
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
#
# These two tails, with the flags-only head, are the shape that applies on EVERY
# line, prose included.
_RM_FLAGS_HEAD = r"(?:\s+-\S+)*\s+"
_RM_TAIL_STRICT = r"(?:\s+-[^\s\"']+)*\s*$"
_RM_TAIL_SHELL = r"(?:\s+-[^\s\"']+)*[\s\"']*$"

# **At COMMAND POSITION, the target may be ANY operand, not only the last.**
# Tolerating only flags around it meant one more path demoted a root or home
# delete to the liftable ask tier: `rm -rf ~/ .cache` (the stray-space slip for
# `~/.cache`), `rm -rf build /` and `rm -rf / build` all asked. And requiring
# the target to end the line let `rm -r ~ 2>/dev/null` and `rm -r ~; echo done`
# through at every tier, so here the operands may also end at a control
# operator or a redirect.
#
# That shape is confined to an `rm` that is really being RUN — at COMMAND
# POSITION (`_RM_POSITION`) on a line that does not begin inside an open quote,
# or inside a shell's `-c` payload, which `classify` re-classifies as a command
# of its own (`shell_payloads`). The first version applied it on every line,
# and because `classify` splits on newlines, a line of a multi-line commit
# message or PR body reading `rm -r ~ and rm -R / are closed` became an
# un-overridable deny — a message describing this very fix could not be
# committed inline. Elsewhere the flags-only shape above still applies, with
# the force flag now optional.
#
# `_RM_OPERAND` excludes shell control and redirect characters, so a SECOND
# command's path is never read as this one's operand: `rm -rf build; ls /` lists
# root, and must not deny. It admits a quoted segment only when the quote also
# CLOSES on the line, so `rm -rf "a b" /` denies while a stray `then"` — the
# closing line of a stored message — never extends an operand.
_RM_WORD = r"[^\s\"';&|<>()`]"
_RM_OPERAND = r"(?:" + _RM_WORD + r"|\"[^\"\n]*\"|'[^'\n]*')+"
_RM_HEAD = r"(?:\s+" + _RM_OPERAND + r")*\s+"
_RM_COMMAND_END = r"(?:\s*$|\s*(?:[;&|)`]|\d*>))"
_RM_TAIL_COMMAND = r"(?:\s+" + _RM_OPERAND + r")*" + _RM_COMMAND_END

# What may stand between a command position and the `rm` it runs: `VAR=val`
# assignments, and wrappers that run their argv. A wrapper's options may take a
# value (`sudo -u root`, `nice -n 10`), and `timeout` takes a duration.
#
# Every token here has exactly ONE reading, which is what keeps a long run of
# them linear: an option starts with `-`, an assignment holds `=`, and a value
# holds neither and is never a wrapper word. Let a token be read two ways and a
# run of thirty fails in exponential time, as `_GIT_OPTION_VALUE` once did. A
# value or an assignment may carry a quoted segment (`sudo -u "$USER"`,
# `env FOO="a b"`); a quote inside it does not count as its `=`.
_RM_WRAPPER_WORDS = (
    r"(?:sudo|doas|env|command|exec|nohup|nice|time|xargs|ionice|stdbuf)"
)
_RM_QUOTED_SEGMENT = r"\"[^\"\n]*\"|'[^'\n]*'"
_RM_ASSIGNMENT = r"[A-Za-z_]\w*=(?:" + _RM_WORD + r"|" + _RM_QUOTED_SEGMENT + r")*"
_RM_WRAPPER_VALUE = (
    r"(?!(?:"
    + _RM_WRAPPER_WORDS
    + r"|timeout|rm)\s)(?:[^\s\"';&|<>()`=-]|"
    + _RM_QUOTED_SEGMENT
    + r")(?:[^\s\"';&|<>()`=]|"
    + _RM_QUOTED_SEGMENT
    + r")*"
)
_RM_WRAPPER_OPTION = r"-" + _RM_WORD + r"+(?:\s+" + _RM_WRAPPER_VALUE + r")?"
_RM_WRAPPERS = (
    r"(?:(?:"
    + _RM_ASSIGNMENT
    + r"|timeout(?:\s+"
    + _RM_WRAPPER_OPTION
    + r")*\s+\d"
    + _RM_WORD
    + r"*|"
    + _RM_WRAPPER_WORDS
    + r"(?:\s+"
    + _RM_WRAPPER_OPTION
    + r")*)\s+)*"
)

# Command position: a line start, a control operator, an opening `(`, `$(` or
# backtick, or the `)` closing a `case` pattern or a function's `f()`, then any
# of `{`, `!` and the keywords `if then do else elif while until` — so the `rm`
# in `if true; then rm …`, `{ rm …; }`, `(rm …)`, `x=$(rm …)`, `case … *) rm`
# and `f() { rm …; }` is recognized. A keyword counts only right after one of
# those anchors: an `echo then rm -r / x` is not a compound statement.
_RM_POSITION = (
    r"(?:^|[;&|()`])(?:\s*(?:[{!]|(?:if|then|do|else|elif|while|until)(?=\s)))*\s*"
    + _RM_WRAPPERS
)


def _rm_any_operand(position, tail):
    """The any-operand catastrophic delete, after ``position``.

    The `rm` word is captured as group ``rm`` so `classify` can check that it
    sits outside every INERT quoted span (`inert_command_spans`) — an `rm`
    after a `;` inside a quoted message is prose, not a command.
    """
    return re.compile(
        position
        + r"(?P<rm>"
        + _RM_RECURSIVE
        + r")"
        + _RM_HEAD
        + _CATASTROPHIC_COMMAND_TARGET
        + tail
    )


# Programs that hand a quoted argument to a shell for execution. Only these get
# the quote-tolerant tail. An allowlist, not a denylist of "safe" programs: the
# executor long tail (`ssh`, `sudo`, `find -exec`, `xargs`, `timeout`) is what
# a denylist would have to enumerate, and missing one fails open.
SHELL_PROGRAMS = frozenset({"sh", "bash", "zsh", "dash", "ksh", "ash", "fish"})

# The force-push patterns, which `classify` matches over the whole command with
# PROSE quoting honored (see `prose_spans`) rather than line by line. Without
# that gate a commit message that merely QUOTES a push was classified as one:
# the measured instance was a commit whose body described this guard's fix and
# quoted a force-push example, and quoting a push to `main` reached the deny
# tier, which no marker lifts — the only way through was to reword the message.
_PROSE_GATED = set()


def _force_push(tail):
    """A `git push` pattern ending in ``tail``, registered for the prose gate."""
    pattern = re.compile(_GIT + r"push\b" + tail)
    _PROSE_GATED.add(pattern)
    return pattern


# Denies that apply on EVERY line, whatever the program.
DENY_PATTERNS = (
    (
        re.compile(
            _RM_RECURSIVE + _RM_FLAGS_HEAD + _CATASTROPHIC_TARGET + _RM_TAIL_STRICT
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
        _force_push(
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
            _RM_RECURSIVE + _RM_FLAGS_HEAD + _CATASTROPHIC_TARGET + _RM_TAIL_SHELL
        ),
        "a recursive delete of the filesystem root or the home directory",
    ),
)

# Denies for an `rm` at COMMAND POSITION, checked only on a line that does not
# begin inside an open quote, and only where the `rm` word itself is outside
# every inert quote — see `_RM_OPERAND` for why the any-operand shape must not
# reach prose.
COMMAND_DENY_PATTERNS = (
    (
        _rm_any_operand(_RM_POSITION, _RM_TAIL_COMMAND),
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
        #
        # `--force-with-lease` is EXEMPT, and the exemption narrows the guard
        # rather than weakening it. The lease is what makes the form safe: the
        # push aborts if the remote moved unexpectedly, so it cannot clobber
        # another session's work. It is also the form this repo's flow produces
        # on essentially every branch that outlives one main commit — rebase is
        # mandatory at `init-pr` step 5, at `review-pr` step 2, and again at the
        # handoff whenever main moves mid-run (one session's ask fired after
        # main moved twice during a single lint run). An ask that fires on every
        # push of the normal workflow trains the operator to approve without
        # reading, which costs more than it protects.
        #
        # The dangerous case a reviewer wants flagged is the LEASE-LESS force,
        # and every form of it still asks: bare `--force`, the short `-f`
        # INCLUDING INSIDE A CLUSTER, the `+refspec:` spelling, and
        # `--force-if-includes` (which forces nothing on its own, so it can only
        # appear beside a real force). `--force` written alongside a lease also
        # still asks, since the bare flag is matchable in its own right.
        #
        # The cluster half was a pre-existing gap this comment would otherwise
        # have claimed away: the old `(?<!\w)-f(?!\w)` missed `git push -fu` and
        # `git push -uf`, which are ordinary git and exactly the shape a rebased
        # first push produces. `git push` has no short flag other than `-f` that
        # contains an `f`, so widening to a cluster costs no false positive.
        #
        # The cluster branch must not reach `--force-with-lease`, which is why it
        # requires a non-hyphen after the dash and no hyphen before it: without
        # that, the branch matches the `force` inside the lease spelling and
        # silently undoes the exemption above.
        #
        # `\b` alone did not exclude the lease form: it matches between the `e`
        # of `--force` and the following `-`, so the lease spelling tripped this
        # branch on every push. The lookahead is what does the work, and it
        # tolerates the `=<refname>` argument because `\b` sits before the `=`
        # too.
        _force_push(
            r".*(?:--force(?!-with-lease\b)\b"
            r"|(?<![\w-])-(?!-)[A-Za-z]*f[A-Za-z]*(?![\w-])"
            r"|\+[\w./-]+:)"
        ),
        "a force-push without a lease",
    ),
    (re.compile(_GIT + r"reset\s+--hard\b"), "a hard reset, which discards changes"),
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
            _GIT + r"clean\b(?![^\n]*\s(?:-[a-zA-Z]*n|--dry-run\b))"
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
        re.compile(_GIT + r"branch\b(?:\s+-\S+)*\s+-\S*D"),
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

# The characters a command may use OUTSIDE its quotes for quote suppression to
# apply at all, and the one `$` form allowed INSIDE double quotes. Everything
# else — `$`, a backtick, `(`, `{`, `\`, `*`, `<`, `!` — means no suppression.
#
# An ALLOWLIST, because the denylist it replaced lost three rounds of
# adversarial review in a row. Each round named constructs that either run a
# quoted string or make `quoted_spans`, which knows no shell grammar beyond
# plain quoting, pair a quote with the wrong partner — so a real command on a
# later line fell inside a fake "span" and was suppressed. All ran the hidden
# command in bash or zsh:
#
#     echo $(ssh host '…')          unquoted substitution, any executor inside
#     echo "${x:-"'"}"              quotes nested in an expansion
#     echo "${x:-\}"'"}"            ...behind an escaped brace
#     printf -v 'a[$(…)]' x         zsh evaluates the subscript
#     echo *(e:'…':)                zsh glob qualifier runs its string
#     echo =(ssh host '…')          zsh process substitution
#
# plus a heredoc (`<<`), whose unquoted apostrophes open phantom quotes, and
# ANSI-C `$'…'`, whose `\'` is an escape. None of them survives this list, and
# a construct nobody has thought of yet fails CLOSED, into the false positive.
# A plain `$NAME` / `${NAME}` inside double quotes expands to a value and runs
# nothing, which is what keeps `grep -rn "rm -rf ${HOME}"` suppressed.
_PLAIN_UNQUOTED = re.compile(r"[\w\s./:=@%+,-]*")
_PLAIN_EXPANSION = re.compile(r"\$(?:[A-Za-z_]\w*|\{[A-Za-z_]\w*\})")


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


def unquoted_start_lines(cmd):
    """The non-blank lines of ``cmd`` that do NOT begin inside an open quote.

    A line that begins inside a quote is the continuation of a quoted argument —
    typically a multi-line commit message or PR body — so it is prose rather
    than a command, even though `classify` sees it as a line of its own. Quote
    state is carried across newlines with the same rules as `quoted_spans`.

    A HEREDOC body is prose for the same reason, and is skipped through its
    terminator: `git commit -F - <<'EOF'` with a body line reading
    `rm -r / x unclassified` was otherwise an un-overridable deny. Skipping the
    body also stops an apostrophe inside it from opening a quote that never
    closes, which used to hide every real command after the heredoc. The
    exception is a heredoc fed to a SHELL (`bash <<EOF`), whose body is
    commands and is kept.
    """
    return [line for line, command in _scan_lines(cmd) if command and line.strip()]


def without_heredoc_bodies(cmd):
    """``cmd`` with every prose heredoc body, terminator included, removed."""
    return "\n".join(line for line, _ in _scan_lines(cmd))


def _scan_lines(cmd):
    """``[(line, begins_unquoted)]`` for the lines of ``cmd`` outside a prose
    heredoc body — the walk `unquoted_start_lines` documents.

    A heredoc opened inside a still-open `"$(` is prose too: the repo's own
    `git commit -m "$(cat <<'EOF'` … `EOF` / `)"` idiom. Its body used to be
    tracked as the continuation of the double quote, so one stray `"` in it —
    a `5"`, a quoted `then"` — flipped the quote state and exposed every later
    body line as a command, where a markdown code span reached the deny tier.
    The body is skipped with the double quote still open, and the `)"` after
    the terminator closes it.
    """
    result = []
    quote = None
    heredoc = None
    for line in cmd.split("\n"):
        if heredoc is not None:
            if line.strip() == heredoc:
                heredoc = None
            continue
        opener = None
        opened_at = 0
        result.append((line, quote is None))
        if quote is None:
            if program_of(line) not in SHELL_PROGRAMS:
                spans = quoted_spans(line)
                for match in _HEREDOC_RE.finditer(line):
                    if not any(lo <= match.start() < hi for lo, hi, _ in spans):
                        opener = match.group("tag")
                        opened_at = match.start()
                        break
        quote = _carry_quote(line, quote)
        if opener is not None and (
            quote is None or (quote == '"' and '"$(' in line[:opened_at])
        ):
            heredoc = opener
    return result


# A heredoc operator and its terminator word, quoted or not. `<<<` is a
# here-STRING, which has no body, so it is excluded.
_HEREDOC_RE = re.compile(r"(?<!<)<<-?(?!<)\s*(['\"]?)(?P<tag>[A-Za-z_]\w*)\1")


def _carry_quote(line, quote):
    """The quote still open at the end of ``line``, given ``quote`` at its start."""
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
        elif quote == '"' and c == "\\":
            i += 2
            continue
        elif c == quote:
            quote = None
        i += 1
    return quote


def plain_quoting(text):
    """Whether ``text`` uses only quoting ``quoted_spans`` models faithfully.

    True when every character outside the quotes is in ``_PLAIN_UNQUOTED``,
    every quote is closed, and each double-quoted span holds no `$` beyond a
    plain ``_PLAIN_EXPANSION``, no backtick and no backslash. Single-quoted
    content is unrestricted: both bash and zsh take it literally.
    """
    spans = quoted_spans(text)
    unquoted = []
    cursor = 0
    for lo, hi, quote in spans:
        # The span excludes its delimiters, which sit at lo - 1 and hi.
        unquoted.append(text[cursor : lo - 1])
        cursor = hi + 1
        if quote == '"':
            body = _PLAIN_EXPANSION.sub("", text[lo:hi])
            if any(c in body for c in "$`\\"):
                return False
    unquoted.append(text[cursor:])
    # An unterminated quote leaves its quote character here, and fails.
    return bool(_PLAIN_UNQUOTED.fullmatch("".join(unquoted)))


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

    Both now sit behind ``plain_quoting``, which refuses suppression outright
    on a line whose quoting this tracker cannot model, so the first condition
    is defense in depth rather than the line of defense.
    """
    if not plain_quoting(line):
        return []
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
    tail (`_RM_TAIL_SHELL`, so `bash -c "rm -rf /"` is caught) removes that
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


# Commands whose quoted arguments are PROSE — stored or printed, never run: a
# commit, tag or note message, a PR / issue / release body, an `echo`. The
# read-only searches in `READ_ONLY_PROGRAMS` join them in `prose_spans`.
#
# An ALLOWLIST of prose commands, deliberately not a test of "is `git` at
# command position". A position test has to enumerate every executor that runs
# its unquoted argv — `ssh host git push …`, `timeout`, `xargs`, `find -exec`,
# `watch` — and missing one fails open, the same argument `SHELL_PROGRAMS`
# makes. Missing a prose command here fails CLOSED, into the false positive this
# gate exists to remove.
#
# Matched by SUBCOMMAND, because the program alone is too broad: `git rebase
# --exec '…'`, `git submodule foreach '…'` and a `!`-alias under `git -c` all
# run their quoted argument, and `gh codespace ssh` runs one remotely.
#
# And only `-C <path>` may precede a git subcommand, not the full `_GIT`
# option set: `git -c core.editor=dash commit -e -m '…'` hands the message
# file to a program that runs it as a script. `printf` is absent for a
# similar reason — zsh's `printf -v 'a[$(…)]'` evaluates the subscript.
#
# Each prose word must end at whitespace, not at `\b`: a word boundary also
# falls before `=` and `-`, so `echo=1 dash -c '…'` — an assignment prefixing
# a different command — read as `echo`, as did `echo-x` and `gh pr-x`.
_PROSE_COMMAND = re.compile(
    r"\s*(?:git(?:\s+-C\s+"
    + _GIT_OPTION_VALUE
    + r")*\s+(?:commit|tag|notes)"
    + r"|gh\s+(?:pr|issue|release|api)"
    + r"|echo)(?=\s|$)"
)


def prose_spans(cmd):
    """Absolute ``(lo, hi)`` ranges of ``cmd`` that are inert prose arguments.

    Quotes are tracked over the WHOLE command, not per line, because the case
    this exists for is a multi-line commit message: its body lines begin inside
    a quote, and per line they read as commands of their own.

    A span qualifies only when all of these hold, and anything else returns no
    span at all:

    - the whole command passes ``plain_quoting``, so the spans are the ones
      a shell would see and none of them can run;
    - nothing outside the quotes hands text back to a shell — `eval`,
      `xargs`, `sh`, `source` (`_RE_EVALUATES`; its separators cannot pass
      the first check anyway);
    - the command it is an argument of is a prose command (`_PROSE_COMMAND`) or
      a read-only search (`READ_ONLY_PROGRAMS`). With no separator possible, a
      command starts at the last unquoted newline before the span.
    """
    if not plain_quoting(cmd):
        return []
    spans = quoted_spans(cmd)

    def quoted(index):
        return any(lo <= index < hi for lo, hi, _ in spans)

    for match in _RE_EVALUATES.finditer(cmd):
        if not quoted(match.start()):
            return []

    result = []
    for lo, hi, _ in spans:
        newline = cmd.rfind("\n", 0, lo)
        while newline != -1 and quoted(newline):
            newline = cmd.rfind("\n", 0, newline)
        command = cmd[newline + 1 : lo]
        if _PROSE_COMMAND.match(command) or program_of(command) in READ_ONLY_PROGRAMS:
            result.append((lo, hi))
    return result


# A shell at command position, ending in its `-c` flag, right before the quoted
# span that is its payload. Matched against the text BEFORE the span.
_SHELL_C = re.compile(
    _RM_POSITION
    + r"(?:"
    + _RM_WORD
    + r"*/)?(?:"
    + "|".join(sorted(SHELL_PROGRAMS))
    + r")(?:\s+"
    + _RM_WORD
    + r"+)*?\s+-[A-Za-z]*c(?:\s+-"
    + _RM_WORD
    + r"*)*\s*\$?\Z",
    re.MULTILINE,
)
_C_FLAG_TAIL = re.compile(r"\s-[A-Za-z]*c(?:\s+-" + _RM_WORD + r"*)*\s*\$?\Z")


def _live_regions(text, lo, hi):
    """``[(start, end)]`` of the command substitutions in ``text[lo:hi]``.

    The body of a double-quoted span: an unescaped `$(…)`, matched by paren
    depth, or an unescaped backtick pair. A backslash-escaped backtick or `\\$`
    is a literal character inside double quotes, so it opens nothing.
    """
    regions = []
    i = lo
    while i < hi:
        c = text[i]
        if c == "\\":
            i += 2
            continue
        if c == "$" and text.startswith("$(", i):
            depth = 0
            j = i + 1
            while j < hi:
                if text[j] == "\\":
                    j += 2
                    continue
                if text[j] == "(":
                    depth += 1
                elif text[j] == ")":
                    depth -= 1
                    if depth == 0:
                        break
                j += 1
            regions.append((i, min(j + 1, hi)))
            i = j + 1
            continue
        if c == "`":
            j = i + 1
            while j < hi and text[j] != "`":
                j += 2 if text[j] == "\\" else 1
            regions.append((i, min(j + 1, hi)))
            i = j + 1
            continue
        i += 1
    return regions


def inert_command_spans(line):
    """``[(lo, hi)]`` ranges of ``line`` quoted as data rather than run.

    A single-quoted span is inert whole. A double-quoted span is inert except
    for its command substitutions (`_live_regions`), which RUN: in
    `git commit -m "$(rm -r / x)"` the delete happens. Treating the whole span
    as live instead denied prose that merely sits beside a substitution —
    `"Bump $(git describe) (rm -r ~ x is closed)"` — and every message whose
    escaped backticks quote a command, at the tier no marker lifts.
    """
    result = []
    for lo, hi, quote in quoted_spans(line):
        if quote == "'":
            result.append((lo, hi))
            continue
        cursor = lo
        for start, end in _live_regions(line, lo, hi):
            result.append((cursor, start))
            cursor = end
        result.append((cursor, hi))
    return result


# Nested `sh -c` levels `classify` follows before it stops looking deeper.
_MAX_SHELL_DEPTH = 4


def shell_payloads(cmd):
    """The `-c` payloads in ``cmd``, unquoted as the shell would read them.

    A payload is a COMMAND, so `classify` re-classifies it whole rather than
    matching patterns against the quoted string. Matching in place recognized
    only an `rm` that opened the payload: `bash -c "cd x && rm -r ~/ y"` and a
    multi-line payload's later lines both reached only the ask tier. A payload
    sitting inside another quote is an argument, not a payload, so only spans
    at the top level of ``cmd`` qualify — and none in a prose heredoc body.

    The shell and its `-c` are searched for only on the payload's own line:
    continuations are already collapsed, so they cannot sit on an earlier one,
    and searching the whole prefix once per span was quadratic in the length
    of the command. An ANSI-C `$'…'` payload has its `\\n` / `\\t` escapes
    decoded, since `bash -c $'cd x\\nrm …'` runs two lines.
    """
    cmd = without_heredoc_bodies(cmd)
    payloads = []
    for lo, hi, quote in quoted_spans(cmd):
        start = cmd.rfind("\n", 0, lo) + 1
        # A bounded look at what ends just before the quote first: only a span
        # right after a `-c` flag gets the full search, so a line of thousands
        # of ordinary quoted words stays linear.
        if not _C_FLAG_TAIL.search(cmd, max(start, lo - 1 - 256), lo - 1):
            continue
        if not _SHELL_C.search(cmd, start, lo - 1):
            continue
        body = cmd[lo:hi]
        if quote == '"':
            body = re.sub(r"\\([\\\"$`])", r"\1", body)
        elif cmd[lo - 2 : lo - 1] == "$":
            body = re.sub(
                r"\\([nt\\])",
                lambda m: {"n": "\n", "t": "\t"}.get(m.group(1), m.group(1)),
                body,
            )
        payloads.append(body)
    return payloads


def _fires_outside(pattern, cmd, spans):
    """Whether ``pattern`` matches ``cmd`` starting outside every span.

    Searched from each match's start + 1 rather than with `finditer`, so a
    match suppressed inside a span cannot consume an unsuppressed one after it.
    """
    match = pattern.search(cmd)
    while match:
        if not any(lo <= match.start() < hi for lo, hi in spans):
            return True
        match = pattern.search(cmd, match.start() + 1)
    return False


def classify(cmd, _depth=0):
    """``("deny"|"ask"|None, reason)`` for one command string.

    Each **line** is classified independently. Newline is a command separator
    in shell, so a multi-line payload is several commands — and classifying the
    whole blob as one string would let the deny patterns' end-anchors
    (``…\\s*$``) be defeated simply by appending another line. The exception
    is the force-push patterns (``_PROSE_GATED``), matched over the whole
    command so a multi-line message's quotes are tracked; their ``.*`` does not
    cross a newline, so the end-anchor concern does not arise for them.

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
    prose = prose_spans(cmd)

    def fires(pattern):
        if pattern in _PROSE_GATED:
            return _fires_outside(pattern, cmd, prose)
        return any(_matches(pattern, line) for line in lines)

    for pattern, reason in DENY_PATTERNS:
        # The read-only carve-out applies here too — see `_matches`. It has to,
        # now that a trailing quote no longer defeats the end anchor.
        if fires(pattern):
            return "deny", reason
    # Then the shell-only denies, on shell lines only. A closing quote after
    # the target is evidence a shell was handed the string — but only when a
    # shell is what the line invokes.
    shell_lines = [line for line in lines if program_of(line) in SHELL_PROGRAMS]
    for pattern, reason in SHELL_DENY_PATTERNS:
        if any(_matches(pattern, line) for line in shell_lines):
            return "deny", reason
    # Then each shell `-c` payload, classified as the command it is.
    if _depth < _MAX_SHELL_DEPTH:
        for payload in shell_payloads(cmd):
            tier, reason = classify(split_comments(payload)[0], _depth + 1)
            if tier == "deny":
                return tier, reason
    # Then the command-position denies, on lines a shell would read as
    # commands: not a continuation of a quoted message, and with the `rm` word
    # itself outside every inert quote — a substitution inside double quotes
    # runs, so it is not inert (`inert_command_spans`). A `#` comment is not a
    # command either; `evaluate` strips the top level's, but a shell-fed
    # heredoc body's `# 1) rm -r / x` survives to here.
    for line in unquoted_start_lines(cmd):
        line = split_comments(line)[0]
        spans = inert_command_spans(line)
        for pattern, reason in COMMAND_DENY_PATTERNS:
            for match in pattern.finditer(line):
                start = match.start("rm")
                if not any(lo <= start < hi for lo, hi in spans):
                    return "deny", reason
    for pattern, reason in ASK_PATTERNS:
        if fires(pattern):
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
        # A LEASED force-push is the rebase workflow's normal push, so it must
        # not ask. `\b` after `--force` matches before the `-` of `-with-lease`,
        # so these tripped the ask tier on every rebased branch.
        ("git push --force-with-lease origin eng-942", None),
        ("git push --force-with-lease", None),
        ("git push --force-with-lease=eng-942:abc123 origin eng-942", None),
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
        # ...and the lease exemption must not become a bypass for the lease-LESS
        # forms, which are the ones worth flagging. `--force-if-includes` forces
        # nothing by itself, so it only ever appears beside a real force; a bare
        # `--force` written alongside a lease is still matchable on its own.
        ("git push origin +eng-942:eng-942", "ask"),
        ("git push --force-if-includes origin eng-942", "ask"),
        ("git push --force-with-lease --force origin eng-942", "ask"),
        # A clustered short force. Ordinary git, and exactly what a rebased first
        # push produces; the un-clustered `-f` branch missed both spellings.
        ("git push -fu origin eng-942", "ask"),
        ("git push -uf origin eng-942", "ask"),
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
        # The `rm` forms asked until a `)` or backtick could end a command;
        # now the substitution is a command position of its own.
        ('grep "$(git push --force origin main)" f', "deny"),
        ('grep "$(rm -rf ~)" file', "deny"),
        ('grep "`rm -rf ~`" file', "deny"),
        ('grep "${x:-$(rm -rf ~)}" file', "deny"),
        ('rg "$(rm -rf ~)" .', "deny"),
        ('python3 .claude/tools/search_source.py "$(rm -rf ~)"', "deny"),
        # `-m` names a module, not this repo's tool, so it must not reach the
        # allowlist.
        ('python3 -m grep "$(rm -rf ~)"', "deny"),
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
        # A git GLOBAL OPTION between `git` and the subcommand defeated every
        # git pattern, deny tier included — and `git -C <path>` is a shape the
        # repo's own skills prescribe.
        ("git -C /x push --force origin main", "deny"),
        ("git --no-pager push -f origin main", "deny"),
        ('git -C "/a b" -c core.pager=cat push origin +main:main', "deny"),
        ("git --git-dir=/x/.git push --force origin main", "deny"),
        ("git -C /x push --force origin eng-942", "ask"),
        ("git -C /x reset --hard HEAD~1", "ask"),
        ("git -C /x clean -fdx", "ask"),
        ("git -C /x branch -D eng-1", "ask"),
        ("git -C /x clean -ndx", None),
        ("git -C /x push --force-with-lease origin eng-942", None),
        ("git -C /repos/main push -u origin eng-942", None),
        # A recursive delete needs no force flag to be catastrophic: the Bash
        # tool's stdin is not a terminal, so `rm` never prompts. Each of these
        # passed the guard at every tier.
        ("rm -r ~", "deny"),
        ("rm -R /", "deny"),
        ("rm -r $HOME/", "deny"),
        ("rm --recursive ~", "deny"),
        ('bash -c "rm -r /"', "deny"),
        # ...while a plain recursive delete elsewhere stays unclassified, and
        # the `r` inside a LONG option is not recursion.
        ("rm -r build", None),
        ("rm --force notes.txt", None),
        # A catastrophic target is catastrophic as ANY operand, not only the
        # last. These all asked, one marker away from the home directory.
        ("rm -rf ~/ .cache", "deny"),
        ("rm -rf build /", "deny"),
        ("rm -rf / build", "deny"),
        ('bash -c "rm -rf ~/ .cache"', "deny"),
        # ...but a second command's path is not this command's operand, and
        # prose that merely quotes the spelling still never reaches deny.
        ("rm -rf build; ls /", "ask"),
        ('git commit -m "Never run rm -rf / and then walk away"', "ask"),
        ("rm -rf ~/build", "ask"),
        # A target followed by a redirect or a second command is still the
        # target. Requiring it to end the line let both of these through at
        # every tier.
        ("rm -r ~ 2>/dev/null", "deny"),
        ("rm -r ~; echo done", "deny"),
        ("cd /tmp && rm -rf build /", "deny"),
        ("sudo rm -r / x", "deny"),
        ("cd /tmp\nrm -rf build /", "deny"),
        # ...but the any-operand shape is for an `rm` being RUN. A line of a
        # multi-line quoted message is prose, and so is an `rm` after a quoted
        # `;` — the first version denied all of these with no override, which
        # made a commit message describing this very fix impossible to write.
        ('git commit -m "Subject\n\nleft rm -r ~ and rm -R / unclassified\n"', None),
        ('gh pr create --title t --body "Body\nrm -R / unclassified\nend"', None),
        ('git commit -m "Subject\n\nrm -rf build / then more\n"', "ask"),
        ('git commit -m "a; rm -rf / && b"', "ask"),
        ("echo rm -r foo /", None),
        # A heredoc body is prose too — unless a shell is reading it — and an
        # apostrophe inside one must not hide the real command after it.
        ("git commit -F - <<'EOF'\nSubject\n\nrm -r / x unclassified\nEOF", None),
        ("cat <<EOF\nit's here\nEOF\nrm -rf build /", "deny"),
        ("bash <<EOF\nrm -rf ~/ .cache\nEOF", "deny"),
        ("cat <<< 'x'\nrm -rf build /", "deny"),
        # Wrappers whose options take values, and assignment prefixes. Each of
        # these fell back to the flags-only shape, unclassified at every tier.
        ("sudo -u root rm -r / x", "deny"),
        ("env FOO=1 rm -r ~ x", "deny"),
        ("nice -n 10 rm -r / x", "deny"),
        ("timeout 5 rm -r / x", "deny"),
        ("timeout -s KILL 5 rm -r / x", "deny"),
        ("FOO=1 rm -r / x", "deny"),
        ("find . | xargs rm -r / x", "deny"),
        ("sudo env FOO=1 nice -n 5 rm -r ~ x", "deny"),
        # ...but a wrapper's value is never a program it runs.
        ("sudo echo rm -r / x", None),
        ("env FOO=1 echo rm -r / x", None),
        # A wrapper run stays linear: a token read two ways is exponential.
        ("sudo" + " -u x" * 60 + " y", None),
        ("sudo" + " -E sudo" * 40 + " y", None),
        # Compound-statement positions.
        ("if true; then rm -r / x; fi", "deny"),
        ("for f in a; do rm -r ~ x; done", "deny"),
        ("{ rm -r / x; }", "deny"),
        ("(rm -r / x)", "deny"),
        ("x=$(rm -r / x)", "deny"),
        ("! rm -r / x", "deny"),
        ("echo `rm -r / x`", "deny"),
        # ...but a keyword is only a keyword at command position, and an `rm`
        # in an inert quote is prose — while one in a live substitution runs.
        ("echo then rm -r / x", None),
        ('git commit -m "Subject\n\nif x; then rm -r / x; fi\n"', None),
        ("git commit -m 'run (rm -r / x) never'", None),
        ('git commit -m "$(rm -r / x)"', "deny"),
        ("git commit -F - <<'EOF'\nSubject\n\nthen rm -r ~ x\n(rm -r / x)\nEOF", None),
        # A shell's `-c` payload is a command of its own, past its first word
        # and its first line.
        ('bash -c "cd x && rm -r ~/ y"', "deny"),
        ("bash -c 'cd x\nrm -r / y'", "deny"),
        ("sh -lc 'true; rm -r / y'", "deny"),
        ("sudo bash -c 'cd x; rm -r ~ y'", "deny"),
        ("bash -c \"sh -c 'cd x; rm -r / y'\"", "deny"),
        ('bash -c "echo \\"rm -r / x\\""', None),
        # ...but a payload quoted inside a stored message is prose.
        ("git commit -m \"Subject\n\nbash -c 'cd x; rm -r / y'\n\"", None),
        ("git commit -F - <<'EOF'\nbash -c 'cd x; rm -r / y'\nEOF", None),
        ("echo bash -c 'cd x; rm -r / y'", None),
        # Quoted operands next to the target, and a quote mid-token.
        ('rm -rf "a b" /', "deny"),
        ("rm -rf / 'a b'", "deny"),
        ('rm -r "$HOME"/*', "deny"),
        ("rm -r '/'/", "deny"),
        ('rm -r "$HOME"/build', None),
        # ...but a stray closing quote never extends an operand into prose.
        ('git commit -m "Subject\n\nrm -r ~ x then"', None),
        ("git commit -m 'Subject\n\nkeep rm -rf \"a b\" / out'", "ask"),
        ("cat <<'EOF'\nrm -rf \"a b\" /\nEOF", "ask"),
        # The wrapper widening is for an `rm` being run, too.
        ("git commit -m 'Subject\n\nsudo -u root rm -r / x\n'", None),
        ('git commit -m "Subject\n\nFOO=1 rm -r / x\n"', None),
        ("cat <<'EOF'\nsudo -u root rm -r / x\nEOF", None),
        ("cat <<'EOF'\nfind . | xargs rm -r / x\nEOF", None),
        # Quoted wrapper values and assignments, and the remaining wrappers.
        ('sudo -u "$USER" rm -r ~ x', "deny"),
        ('env FOO="a b" rm -r / x', "deny"),
        ('FOO="a b" rm -r ~ x', "deny"),
        ("ionice -c 3 rm -r / x", "deny"),
        ("stdbuf -oL rm -r / x", "deny"),
        # A `case` pattern's `)` and a function body are command positions.
        ("case $x in *) rm -r / x;; esac", "deny"),
        ("f() { rm -r / x; }; f", "deny"),
        # The quoted-target suffix is command-position only: on a prose line
        # the flags-only shape must not deny it.
        ('rm -rf "$HOME/"*', "deny"),
        ("cat <<'EOF'\nrm -r \"$HOME\"/*\nEOF", None),
        ("git commit -m 'Subject\n\nrm -r \"$HOME\"/*\n'", None),
        ("git commit -m 'Subject\n\nrm -r \"/\"/\n'", None),
        # Only a substitution's INTERIOR runs inside double quotes. An escaped
        # backtick or `\$(` is literal, and prose beside a real substitution
        # is still prose — each of these denied, with no override.
        ('git commit -m "Deny \\`rm -r ~ x\\` in payloads"', None),
        ('git commit -m "Deny \\`rm -rf ~ build\\` in payloads"', "ask"),
        ('git commit -m "Document the \\$(rm -r / x) form"', None),
        ('gh pr create --title "t" --body "Close \\`rm -r / x\\` gap"', None),
        ('echo "keep \\`rm -r ~ x\\` out"', None),
        ('git commit -m "Bump $(git describe) (rm -r ~ x is closed)"', None),
        ('git commit -m "Close (rm -r / x was asking) in $(git rev-parse HEAD)"', None),
        ('gh pr comment 1 --body "Let (rm -r ~ x) through; see $(cat ref)"', None),
        ("cat <<'EOF'\necho \"$(rm -r / x)\"\nEOF", None),
        ('echo "a $(rm -r / x) b"', "deny"),
        # More `-c` spellings a shell accepts.
        ("sh -c -- 'cd x; rm -r / y'", "deny"),
        ("bash -c -e 'cd x; rm -r / y'", "deny"),
        ("bash -c $'cd x\\nrm -r / y'", "deny"),
        ("FOO=1 bash -c 'cd x; rm -r ~ y'", "deny"),
        # A heredoc inside `"$(` is prose: a stray `"` in its body must not
        # expose the lines after it, and the command after it still counts.
        (
            "git commit -m \"$(cat <<'EOF'\nKeep prose out\n\nA stray `then\"`"
            ' line\n`rm -r "$HOME"/*` now denies\nEOF\n)"',
            None,
        ),
        (
            "gh pr create --title t --body \"$(cat <<'EOF'\n- a `then\"` line\n"
            '- `case $x in *) rm -r / x;; esac`\n- `sudo -u root rm -r / x`\nEOF\n)"',
            None,
        ),
        ('git commit -m "$(cat <<\'EOF\'\nSubject 5"\nEOF\n)"\nrm -rf build /', "deny"),
        # A comment in a payload or a shell-fed heredoc runs nothing.
        ("bash -c 'make # 1) rm -r / x'", None),
        ("bash -c 'make # note; rm -r / x'", None),
        ("bash <<'EOF'\n# 1) rm -r / x\nmake\nEOF", None),
        ("bash <<'EOF'\nmake\n(rm -r / x)\nEOF", "deny"),
        # Payload extraction stays linear in the command's length.
        ("echo 'a'; " * 4000, None),
        ("echo" + " 'a'" * 8000, None),
        # Long options: `--recursive` still counts, and the `r` inside
        # `--no-preserve-root` does not.
        ("rm --recursive --force build", "ask"),
        ("rm --no-preserve-root notes.txt", None),
        # More global-option spellings, including one the shared table lacked.
        ("git --config-env x=Y push --force origin main", "deny"),
        ('git -C "$PWD" push -f origin main', "deny"),
        # A long run of value-taking options must stay linear: this backtracked
        # exponentially (thirty took 1.5s) before option values were barred
        # from starting with a dash, so a regression shows up as a hang here.
        ("git" + " -c" * 60 + " x", None),
        # A force-push QUOTED in a prose argument is prose. The measured false
        # positive was a commit message quoting one; to `main` it denied.
        ('git commit -S -m "Never git push --force origin main"', None),
        ("git commit -S -m 'Quote git push -fu origin eng-942 here'", None),
        ('git commit -m "Subject\n\ngit push --force origin main\nbody"', None),
        ('git -C /x commit -m "git push -f origin eng-942"', None),
        ('gh pr create --title t --body "Run git push -f origin eng-942"', None),
        ("echo 'git push --force origin main'", None),
        # ...but only an INERT prose argument, and the real command still fires.
        ('git commit -m "$(git push --force origin main)"', "deny"),
        ("git commit -m 'x'\ngit push --force origin main", "deny"),
        ('git commit -m "a\nb"\ngit push -f origin eng-942', "ask"),
        ("echo 'git push -f origin eng-942' | sh", "ask"),
        # A quoted argument that RUNS is not prose: the gate is by subcommand.
        ("git rebase --exec 'git push --force origin main' main", "deny"),
        ("git -c alias.x='!git push -f origin main' x", "deny"),
        ("ssh host 'git push --force origin main'", "deny"),
        # An UNQUOTED substitution is a second command, whatever runs inside
        # it. Each of these classified clean in the gate's first draft.
        ("echo $(ssh host 'git push --force origin main')", "deny"),
        ("git commit -m x `dash -c 'git push --force origin main'`", "deny"),
        ("printf %s <(ssh h 'git push -f origin eng-942')", "ask"),
        ("grep x $(ssh host 'git push --force origin main')", "deny"),
        # Quotes nested in an expansion pair wrongly, which once hid the real push
        # on the middle line; zsh runs a `printf -v` subscript; and an editor
        # set under `git -c` runs the message file.
        ('echo "${x:-"\'"}"\ngit push --force origin main\necho "\'"', "deny"),
        ('echo "$(printf \'"\')"\ngit push -f origin eng-942\necho "\'"', "ask"),
        ("printf -v 'a[$(git push --force origin main)]' x", "deny"),
        ("git -c core.editor=dash commit -e -m 'git push --force origin main'", "deny"),
        # Round three, each closed by the character allowlist rather than by
        # a pattern of its own: an escaped brace, a zsh glob qualifier, zsh's
        # `=(…)`.
        ('echo "${x:-\\}"\'"}"\ngit push --force origin main\necho "\'"', "deny"),
        ("echo *(e:'git push --force origin main':)", "deny"),
        ("echo =(ssh host 'git push --force origin main')", "deny"),
        # An assignment prefix is not the prose word it begins with.
        ("echo=1 dash -c 'git push --force origin main'", "deny"),
        ("echo=1 ksh -c 'git push -f origin eng-942'", "ask"),
        # A suppressed prose match must not hide a real push after it, and a
        # span's command start skips newlines that sit inside earlier quotes.
        ("echo 'git push --force origin main'\ngit push --force origin main", "deny"),
        ('git commit -m "Subject\n\nbody" -m "git push -f origin eng-942"', None),
        # Constructs that would throw off whole-command quote tracking disable
        # the gate, so an apostrophe in them cannot hide the push after them.
        ("git commit -F - <<EOF\nit's\nEOF\ngit push --force origin main 'x'", "deny"),
        ("echo $'it\\'s'\ngit push --force origin main 'x'", "deny"),
    ]
    # The absolute spelling of the home directory, which agents are told to
    # prefer. Built from the real HOME so the case holds on any machine.
    if _HOME.startswith("/"):
        cases += [
            (f"rm -rf {_HOME}/", "deny"),
            (f"rm -rf {_HOME}", "deny"),
            (f"rm -rf {_HOME}/*", "deny"),
            (f"rm -rf {_HOME}/scratch", "ask"),
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
