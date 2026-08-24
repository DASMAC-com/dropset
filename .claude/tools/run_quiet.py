#!/usr/bin/env python3
# cspell:word foobarbaz
"""Run a command quietly: capture its output to a log, surface only a summary.

Per the project's context-economy rule (docs/conventions/context-economy.md),
a tool result is fetched once but replayed as input on every later turn, so a
verbose build log — a ``cargo`` "Compiling …" cascade, a ``make lint`` run, a
``docker`` pull — is paid many times over for a green result that carries zero
information. This wrapper runs
the command with its output redirected to a temp log *inside Python* (so the
model's command line stays free of shell redirects and passes the
``no_compound_bash.py`` guard), then prints:

* on success — a single line naming the command, its exit code, the line count,
  and the log path;
* on failure — first an index of every *failed-hook* result line found anywhere
  in the log (a ``make lint`` / pre-commit run prints one
  ``<hook name>………Failed`` line per hook, and the one that actually failed
  often scrolls off the top past the ``--tail`` window), then an index of the
  distinct spelling offenders (``Unknown word (…)``) with the file each was
  found in, then the last ``--tail`` lines of the log, the exit code, and the
  log path, so the model can ``Read`` more of the log by slice if it needs to.

Note that nothing is printed until the command **exits**: output is captured, so
polling this tool's log while a backgrounded run is still in flight returns
nothing. One session made seven such ``tail`` calls, all empty. Wait for the
completion notification instead — a skill that starts work in the background
should say so rather than prescribe a poll.

One class of line is echoed *while the run is still in flight*: cargo's
``Blocking waiting for file lock on <target>`` status. Buffering it into the log
makes a lock-blocked build indistinguishable from a slow one — the task's output
is simply empty — and that has cost a run several content-free polls plus a
``pgrep`` to work out that a concurrent ``make demo`` held the build lock. So
the child's output is teed through a pipe rather than written straight to the
log, and a lock-wait line is surfaced to stdout the moment it appears. It is
also noted in the final summary, so a run that took minutes because it was
blocked says so rather than looking merely slow.

The failed-hook index exists because trusting the tail alone has bitten us: a
``make lint`` failure in an *early* hook (yamllint, cspell) scrolled off the
50-line tail behind a later hook's output and the run was wrongly judged clean,
costing CI round-trips. Scanning the whole log for ``…Failed`` result lines
surfaces every failing hook regardless of where it sits.

The child's exit code is propagated, so callers (and CI) still see pass/fail.

Usage::

    python3 .claude/tools/run_quiet.py [--tail N] [--label L] -- CMD ARGS...

``--`` separates the wrapper's own options from the command to run. The command
runs with ``shell=False`` — it is exec'd directly, not through a shell, so shell
operators in CMD ARGS are passed verbatim as arguments rather than interpreted.

The tool fails safe: a launch error (missing binary, etc.) prints a clear
message and exits non-zero rather than raising an uncaught traceback.

**Reading a log back**, which is the other half of the job::

    python3 .claude/tools/run_quiet.py inspect LOG [--grep RE] [--context N]
    python3 .claude/tools/run_quiet.py inspect LOG [--tail N]
    python3 .claude/tools/run_quiet.py inspect LOG --failing

With no flags this reprints the failure summary — the failed-hook index, the
spelling index, and the tail — which is what you want after coming back to a
run whose summary has scrolled away.

This subcommand exists because the logs live under the system temp dir, outside
the workspace, and a session that needs more than the summary has to reach in
there. The Grep tool handles it and is the prescribed path — but it is not
always offered, and the sessions that lacked it paid: one made **58 shell**
``grep`` **calls (≈8.7k)**, largely reading these very logs. Filtering in this
process instead of printing the matched region into a tool result is the whole
saving, and it rides the directory-wide ``Bash(python3 .claude/tools/:*)``
allow-rule that every other tool here already uses.

*Correction worth keeping, because the surrounding argument used to be wrong:*
this is justified on **context cost alone**. The "unfirmable prompt churn"
framing once attached to it does not hold on this machine — ``Bash(grep:*)`` is
already in the shared allowlist and the cruft classifier does not flag it, so a
bare ``grep`` prompts for nothing. Do not restate that rationale.

**There is deliberately no ``--latest`` or glob resolution.** Pass the log path
the runner printed for *that* run. A ``make-*.log`` wildcard matches every
historical run in the directory, and resolving "the newest" silently picks a
different run's log when two are in flight — both of which are how a reader ends
up confidently diagnosing the wrong failure (see
``docs/conventions/context-economy.md`` → "Inspect a run_quiet log by its
printed path, not a glob").

Tests live in ``tests/test_run_quiet.py`` (stdlib ``unittest``), run via the
repo's ``make tools-tests``.
"""

from __future__ import annotations

import collections
import os
import re
import subprocess
import sys
import tempfile

# Default number of trailing log lines shown on failure.
DEFAULT_TAIL = 50

# Cap on how many failed-hook index lines to surface, so a pathological log that
# prints "…Failed" thousands of times can't balloon the summary.
MAX_FAILED_LINES = 40

# cspell reports one line per offending token, e.g.
#   docs/foo.md:12:5 - Unknown word (foobarbaz)
# "Forbidden word" is the same shape for a word banned rather than merely absent.
# Indexing these is the fix for the wrapper's worst-behaved case: cspell runs the
# tree in CHUNKS, so the tail window routinely showed a *later, passing* chunk's
# file listing — "Issues found: 0 in 0 files" printed directly beside a Failed
# hook — while the real failure sat in an earlier chunk. Sessions then paid a
# follow-up grep over the log to find the word, one of them four separate times.
UNKNOWN_WORD_RE = re.compile(r"\b(?:Unknown|Forbidden) word \(([^)]+)\)")

# cargo prints its failure detail under a bare `failures:` line, AFTER the run's
# passing `test … ok` lines. So a last-N-lines tail lands on the passing lines
# and the final `test result:` summary, and shows the assertion only if the
# failure happened to be near the end. Measured: 25 wrapped runs spending ~40
# tail lines each to convey one assertion, ~5.5k — a top hardening candidate
# DESPITE already being wrapped. Anchoring on this marker is what makes the
# window land on the detail.
CARGO_FAILURES_RE = re.compile(r"^\s*failures:\s*$")

# Where the window STOPS. A workspace runs several test binaries, so without a
# terminator the window would run from the first failure to end-of-log and
# swallow every later binary's passing output — re-buying exactly the region
# this replaces. `test result:` closes the failing binary's report.
CARGO_RESULT_RE = re.compile(r"^\s*test result:")

# Cap on the failure window, so a suite failing in fifty places still reports a
# bounded amount. Larger than the default tail because this region is the
# actionable payload rather than incidental trailing output.
MAX_FAILURE_LINES = 60

# Cap on distinct spelling offenders surfaced. The first run over a new doc can
# legitimately produce dozens; the index is meant to name the fix, not to
# reproduce the log.
MAX_UNKNOWN_WORDS = 40

# Where captured logs land: a stable subdir of the system temp dir (usually
# /tmp/claude-run-quiet). One file per run, named for the command and pid so
# concurrent runs don't collide.
LOG_DIR = os.path.join(tempfile.gettempdir(), "claude-run-quiet")

# Exit code used when the command can't be launched at all (mirrors the shell's
# 127 "command not found").
LAUNCH_FAILURE_CODE = 127

# Cargo's status line when another cargo process holds the build lock, e.g.
# "Blocking waiting for file lock on build directory". Captured silently it is
# invisible, and a blocked build then reads as a hung one, so this is the one
# line the wrapper echoes live (see the module docstring).
LOCK_WAIT_MARKER = "Blocking waiting for file lock"

# Ceiling on the echoed lock-wait line. The marker is a substring match on child
# output, so the "line" carrying it is attacker-influenced and may have no
# newline for megabytes; `sanitize_for_echo` clamps it to this.
MAX_ECHO_CHARS = 200

# The log-reading subcommand's verb, recognized as the first argument.
INSPECT_VERB = "inspect"

# Cap on matched regions `inspect --grep` prints. The point of filtering in this
# process is that the caller pays for the answer and not the log, so an
# over-broad pattern must degrade into "narrow your pattern" rather than into a
# reproduction of the file.
MAX_GREP_MATCHES = 40


class UsageError(Exception):
    """A malformed invocation: surfaced to stderr, exits non-zero."""


def parse_args(argv):
    """Parse ``[--tail N] [--label L] -- CMD ARGS...`` into (tail, label, cmd).

    Options are read until the ``--`` separator; everything after it is the
    command to run. A missing ``--`` or an empty command is a UsageError.
    """
    tail = DEFAULT_TAIL
    label = None
    i = 0
    n = len(argv)
    while i < n and argv[i] != "--":
        arg = argv[i]
        if arg == "--tail":
            if i + 1 >= n:
                raise UsageError("--tail needs a value")
            tail = _parse_tail(argv[i + 1])
            i += 2
        elif arg.startswith("--tail="):
            tail = _parse_tail(arg[len("--tail=") :])
            i += 1
        elif arg == "--label":
            if i + 1 >= n:
                raise UsageError("--label needs a value")
            label = argv[i + 1]
            i += 2
        elif arg.startswith("--label="):
            label = arg[len("--label=") :]
            i += 1
        else:
            raise UsageError("unknown option: %s" % arg)
    if i >= n:
        raise UsageError("missing '--' separator before the command")
    cmd = argv[i + 1 :]
    if not cmd:
        raise UsageError("no command given after '--'")
    return tail, label, cmd


def _parse_tail(value):
    """Parse a --tail value into a non-negative int, or raise UsageError."""
    try:
        tail = int(value)
    except ValueError:
        raise UsageError("--tail must be an integer, got %r" % value)
    if tail < 0:
        raise UsageError("--tail must be non-negative, got %d" % tail)
    return tail


def sanitize(cmd):
    """Build a filesystem-safe stem from the command tokens.

    Joins the tokens with '-', keeps only alphanumerics / '-' / '_' / '.', and
    truncates so the filename stays short. Empty results fall back to "cmd".
    """
    joined = "-".join(cmd)
    safe = "".join(c if (c.isalnum() or c in "-_.") else "-" for c in joined)
    safe = safe.strip("-._")
    safe = safe[:40].strip("-._")
    return safe or "cmd"


def is_failed_hook_line(line):
    """True for a pre-commit / ``make lint`` per-hook result line that failed.

    Pre-commit prints one ``<hook name>………Failed`` line per hook (passing
    hooks end in ``Passed``); the actual failure detail follows below it.
    Matching the
    result line on a trailing ``Failed`` gives a compact index of *which* hooks
    failed, independent of where in the log they sit.
    """
    return line.rstrip().endswith("Failed")


def is_lock_wait_line(line):
    """True for cargo's "Blocking waiting for file lock on …" status line."""
    return LOCK_WAIT_MARKER in line


def parse_unknown_word(line):
    """``(word, location)`` for a cspell offender line, or ``None``.

    The location is whatever cspell printed before the ``-`` separator — the
    ``path:line:col`` prefix — trimmed. It is kept because the word alone does
    not say which file to escape, and the escape decision is per-file (an inline
    ``cspell:word`` for a single-file term, the shared dictionary for a term in
    two or more; see ``CLAUDE.md`` → "Docs and skills prose").
    """
    match = UNKNOWN_WORD_RE.search(line)
    if not match:
        return None
    word = match.group(1).strip()
    if not word:
        return None
    location = line.split(" - ", 1)[0].strip() if " - " in line else ""
    return word, location


def sanitize_for_echo(line):
    """Make one line of child output safe to print, and bound its length.

    The wrapper's entire value is that child output does **not** reach the
    terminal or the model's context. The lock-wait echo is a deliberate hole in
    that, so what goes through it is scrubbed rather than passed through: any
    build script, vendored dependency, test, or Makefile recipe can emit a line
    containing the marker, and without scrubbing it could carry ANSI/OSC escapes
    (repositioning the cursor, clearing the screen, setting the title) or simply
    append a convincing fake summary line so a failing run reads as clean. The
    same text lands in the tool result, so it is a prompt-injection channel too.

    Control characters are dropped, and the result is truncated — a child can
    emit megabytes with no newline at all, which would otherwise arrive as one
    enormous "line". The full text is still in the log either way.
    """
    cleaned = "".join(c for c in line if c.isprintable())
    cleaned = cleaned.strip()
    if len(cleaned) > MAX_ECHO_CHARS:
        cleaned = cleaned[:MAX_ECHO_CHARS] + "… (truncated; see the log)"
    return cleaned


def stream_to_log(cmd, log_file):
    """Run `cmd`, tee its output into `log_file`, return (exit_code, lock_wait).

    The child writes into a pipe rather than straight to the log so a *blocking*
    status line can be surfaced while the run is still in flight; every other
    line is captured silently, which is the whole point of the wrapper. Output is
    handled a line at a time, so a huge log never sits in memory in full — though
    a child that emits no newline for a long stretch does buffer that stretch as
    one "line", which is the other reason the echo is length-clamped.

    ``lock_wait`` is the first lock-wait line seen, **scrubbed and truncated** by
    ``sanitize_for_echo`` (the echo is a channel out of the capture, so nothing
    reaches it raw), or None. It is echoed to stdout on sight — and flushed, so
    it can't sit in a buffer behind the very wait it is reporting. Only the
    *first* is echoed; cargo repeats the line while it waits.
    """
    lock_wait = None
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        stdin=subprocess.DEVNULL,
        text=True,
        errors="replace",
        bufsize=1,
    )
    with proc:
        for line in proc.stdout:
            log_file.write(line)
            if lock_wait is None and is_lock_wait_line(line):
                lock_wait = sanitize_for_echo(line)
                log_file.flush()
                sys.stdout.write("⏳ %s\n" % lock_wait)
                sys.stdout.flush()
    return proc.returncode, lock_wait


#: What one pass over a captured log yields. A named result rather than a tuple
#: because it has grown twice — the failed-hook index, then the spelling index —
#: and each growth would otherwise have to renumber every unpacking call site.
LogSummary = collections.namedtuple(
    "LogSummary",
    "lines tail_text failed truncated unknown_words unknown_truncated "
    "failure_text failure_truncated",
)


def read_tail_and_count(path, tail):
    """Summarize a captured log in one pass. Returns a :class:`LogSummary`.

    ``failed`` is the list of ``…Failed`` result lines found anywhere in the log
    (newline-stripped, order preserved, capped at MAX_FAILED_LINES) — surfaced on
    failure so a failing hook that scrolled off the tail window is still
    reported. ``truncated`` is True only when *more* than MAX_FAILED_LINES such
    lines were found (so the list holds just the first MAX_FAILED_LINES) — this
    is what distinguishes a genuine cap from a log with exactly
    MAX_FAILED_LINES failures and no more.

    ``unknown_words`` is the same idea one level down, for the hook whose failure
    detail the tail is least likely to hold: ``[(word, location)]`` for each
    *distinct* cspell offender, first occurrence winning, in encounter order.

    Uses a bounded deque so a huge log never sits in memory in full, and scans
    for both indexes in the single pass rather than re-reading the file.
    """
    count = 0
    dq = collections.deque(maxlen=tail if tail > 0 else 0)
    failed = []
    truncated = False
    unknown = {}
    unknown_truncated = False
    failure_lines = []
    failure_started = False
    failure_done = False
    failure_truncated = False
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        for line in fh:
            count += 1
            if tail > 0:
                dq.append(line)
            # Collect from the FIRST `failures:` marker onward. First, not last:
            # cargo prints the marker twice — once before the per-test detail and
            # once before the bare name list — and the detail is the half worth
            # having.
            if (
                not failure_started
                and not failure_done
                and CARGO_FAILURES_RE.match(line)
            ):
                failure_started = True
            if failure_started:
                if len(failure_lines) < MAX_FAILURE_LINES:
                    failure_lines.append(line)
                else:
                    failure_truncated = True
                # `test result:` closes the failing binary's report. Stop there
                # and never restart, so a later binary's output stays out.
                if CARGO_RESULT_RE.match(line):
                    failure_started = False
                    failure_done = True
            if is_failed_hook_line(line):
                if len(failed) < MAX_FAILED_LINES:
                    failed.append(line.rstrip("\n"))
                else:
                    truncated = True
            # Deliberately NOT `continue`. The two matchers are disjoint today
            # only by accident — a failed-hook line ends in "Failed" and an
            # offender line in ")" — so skipping the word scan for a hook line
            # would silently drop a real offender the day cspell appends
            # anything after the parenthesis. The two indexes are independent by
            # intent; one extra regex search per hook line is the whole cost.
            found = parse_unknown_word(line)
            if found is not None and found[0] not in unknown:
                if len(unknown) < MAX_UNKNOWN_WORDS:
                    unknown[found[0]] = found[1]
                else:
                    unknown_truncated = True
    return LogSummary(
        count,
        "".join(dq),
        failed,
        truncated,
        list(unknown.items()),
        unknown_truncated,
        "".join(failure_lines),
        failure_truncated,
    )


def run(tail, label, cmd):
    """Run `cmd`, capture output to a log, print a summary, return its exit code.

    A launch failure (missing binary, permission error) is reported and mapped
    to LAUNCH_FAILURE_CODE rather than raising.
    """
    display = label if label else " ".join(cmd)
    # A captured log can hold secrets a wrapped command surfaced (a token in a
    # failing build, an env dump), so keep the dir and file owner-only.
    os.makedirs(LOG_DIR, mode=0o700, exist_ok=True)
    log_path = os.path.join(LOG_DIR, "%s-%d.log" % (sanitize(cmd), os.getpid()))

    try:
        with open(log_path, "w", encoding="utf-8", errors="replace") as log_file:
            os.chmod(log_path, 0o600)
            code, lock_wait = stream_to_log(cmd, log_file)
    except FileNotFoundError:
        sys.stderr.write("✗ %s — command not found: %s\n" % (display, cmd[0]))
        return LAUNCH_FAILURE_CODE
    except OSError as exc:
        sys.stderr.write("✗ %s — could not launch: %s\n" % (display, exc))
        return LAUNCH_FAILURE_CODE

    summary = read_tail_and_count(log_path, tail)
    # A run that spent minutes queued behind another cargo process should say so
    # in its one-line result, not just look slow.
    waited = " — waited on a cargo file lock" if lock_wait else ""

    if code == 0:
        sys.stdout.write(
            "✓ %s (exit 0, %d lines; log: %s)%s\n"
            % (display, summary.lines, log_path, waited)
        )
        return 0

    sys.stdout.write(
        "✗ %s (exit %d, %d lines; log: %s)%s\n"
        % (display, code, summary.lines, log_path, waited)
    )
    if summary.failed:
        more = " (truncated, more omitted)" if summary.truncated else ""
        sys.stdout.write("--- failed hooks (%d)%s ---\n" % (len(summary.failed), more))
        for fl in summary.failed:
            sys.stdout.write(fl + "\n")
    # Before the tail, not after: the spelling index is the actionable payload,
    # and the tail it would otherwise sit below is precisely the window that
    # tends to show an unrelated passing chunk.
    if summary.unknown_words:
        more = " (truncated, more omitted)" if summary.unknown_truncated else ""
        sys.stdout.write(
            "--- unknown words (%d)%s ---\n" % (len(summary.unknown_words), more)
        )
        for word, location in summary.unknown_words:
            sys.stdout.write("%s%s\n" % (word, " — %s" % location if location else ""))
    write_failure_region(summary, tail)
    return code


def write_failure_region(summary, tail):
    """Print the failures window when there is one, else the tail.

    They are alternatives, not both: the whole point is that on a cargo failure
    the tail shows the *passing* lines that precede the failures block, so
    printing both would keep paying for exactly the region this replaces.
    """
    if summary.failure_text:
        more = " (truncated, more omitted)" if summary.failure_truncated else ""
        sys.stdout.write("--- from cargo's failures block%s ---\n" % more)
        sys.stdout.write(summary.failure_text)
        if not summary.failure_text.endswith("\n"):
            sys.stdout.write("\n")
        return
    if tail > 0 and summary.tail_text:
        shown = min(tail, summary.lines)
        sys.stdout.write("--- last %d line(s) ---\n" % shown)
        sys.stdout.write(summary.tail_text)
        if not summary.tail_text.endswith("\n"):
            sys.stdout.write("\n")


#: What ``inspect`` was asked for.
#:
#: ``grep`` takes precedence: given both, the grep view is printed and
#: ``failing`` is ignored. They are separate views of one log rather than
#: composable filters, and a caller asking for both wants the narrower one.
#: With neither, the full failure summary is reprinted.
InspectArgs = collections.namedtuple("InspectArgs", "path grep context tail failing")


def parse_inspect_args(argv):
    """Parse ``inspect LOG [--grep RE] [--context N] [--tail N] [--failing]``.

    ``argv`` still carries the ``inspect`` verb as its first element, so the
    caller does not have to strip it.
    """
    rest = argv[1:]
    path = None
    grep = None
    context = 0
    tail = DEFAULT_TAIL
    failing = False
    i = 0
    n = len(rest)
    while i < n:
        arg = rest[i]
        if arg == "--grep":
            if i + 1 >= n:
                raise UsageError("--grep needs a pattern")
            grep = rest[i + 1]
            i += 2
        elif arg.startswith("--grep="):
            grep = arg[len("--grep=") :]
            i += 1
        elif arg == "--context":
            if i + 1 >= n:
                raise UsageError("--context needs a value")
            context = _parse_tail(rest[i + 1])
            i += 2
        elif arg.startswith("--context="):
            context = _parse_tail(arg[len("--context=") :])
            i += 1
        elif arg == "--tail":
            if i + 1 >= n:
                raise UsageError("--tail needs a value")
            tail = _parse_tail(rest[i + 1])
            i += 2
        elif arg.startswith("--tail="):
            tail = _parse_tail(arg[len("--tail=") :])
            i += 1
        elif arg == "--failing":
            failing = True
            i += 1
        elif arg.startswith("-"):
            raise UsageError("unknown option: %s" % arg)
        elif path is None:
            path = arg
            i += 1
        else:
            # Refuse rather than silently inspecting the first of several. A
            # second positional is almost always a shell glob that expanded —
            # exactly the mistake the no-glob rule exists to prevent.
            raise UsageError(
                "inspect takes one log path; got a second (%s). Pass the path the "
                "runner printed for that run, never a glob." % arg
            )
    if path is None:
        raise UsageError("inspect needs a log path")
    return InspectArgs(path, grep, context, tail, failing)


def grep_log(path, pattern, context):
    """Matching lines from ``path``, with ``context`` lines either side.

    Returns ``(rendered_lines, match_count, truncated)``. Regions are separated
    by ``--`` and numbered, so a hit can be slice-read from the log afterwards
    without re-grepping. Reads a line at a time and keeps only the context
    window, so an enormous log never sits in memory.
    """
    try:
        compiled = re.compile(pattern)
    except re.error as exc:
        raise UsageError("bad --grep pattern: %s" % exc)

    out = []
    matches = 0
    truncated = False
    before = collections.deque(maxlen=context if context > 0 else 0)
    after_remaining = 0
    last_emitted = 0
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        for lineno, line in enumerate(fh, start=1):
            text = line.rstrip("\n")
            if compiled.search(text):
                if matches >= MAX_GREP_MATCHES:
                    # Test BEFORE counting, so `matches` is what was actually
                    # printed. Counting first reported 41 for a capped run that
                    # emitted 40 regions — a number that was neither what the
                    # caller saw nor what the log holds.
                    truncated = True
                    break
                matches += 1
                start = lineno - len(before)
                if last_emitted and start > last_emitted + 1:
                    out.append("--")
                for offset, prior in enumerate(before):
                    out.append("%d:%s" % (start + offset, prior))
                out.append("%d:%s" % (lineno, text))
                last_emitted = lineno
                before.clear()
                after_remaining = context
            elif after_remaining > 0:
                out.append("%d:%s" % (lineno, text))
                last_emitted = lineno
                after_remaining -= 1
                if context > 0:
                    before.clear()
            elif context > 0:
                before.append(text)
    return out, matches, truncated


def inspect(args):
    """Reprint part of a captured log. Returns an exit code."""
    try:
        summary = read_tail_and_count(args.path, args.tail)
    except OSError as exc:
        sys.stderr.write("run_quiet.py: cannot read %s: %s\n" % (args.path, exc))
        return 2

    if args.grep is not None:
        lines, matches, truncated = grep_log(args.path, args.grep, args.context)
        for line in lines:
            sys.stdout.write(line + "\n")
        note = ""
        if truncated:
            note = "+ (capped at %d — narrow the pattern)" % MAX_GREP_MATCHES
        sys.stdout.write(
            "run-quiet inspect | %d match(es)%s of %d line(s)\n"
            % (matches, note, summary.lines)
        )
        # Non-zero on no match, so a caller checking only the status can tell
        # "not present" from "present" without parsing the summary.
        return 0 if matches else 1

    if summary.failed:
        more = " (truncated, more omitted)" if summary.truncated else ""
        sys.stdout.write("--- failed hooks (%d)%s ---\n" % (len(summary.failed), more))
        for fl in summary.failed:
            sys.stdout.write(fl + "\n")
    if summary.unknown_words:
        more = " (truncated, more omitted)" if summary.unknown_truncated else ""
        sys.stdout.write(
            "--- unknown words (%d)%s ---\n" % (len(summary.unknown_words), more)
        )
        for word, location in summary.unknown_words:
            sys.stdout.write("%s%s\n" % (word, " — %s" % location if location else ""))
    if not args.failing:
        write_failure_region(summary, args.tail)
    sys.stdout.write("run-quiet inspect | %d line(s)\n" % summary.lines)
    return 0


USAGE = (
    "usage: run_quiet.py [--tail N] [--label L] -- CMD ARGS...\n"
    "       run_quiet.py inspect LOG [--grep RE] [--context N] "
    "[--tail N] [--failing]\n"
)


def main(argv):
    if argv and argv[0] == INSPECT_VERB:
        try:
            return inspect(parse_inspect_args(argv))
        except UsageError as exc:
            sys.stderr.write("run_quiet.py: %s\n" % exc)
            sys.stderr.write(USAGE)
            return 2
    try:
        tail, label, cmd = parse_args(argv)
    except UsageError as exc:
        sys.stderr.write("run_quiet.py: %s\n" % exc)
        sys.stderr.write(USAGE)
        return 2
    return run(tail, label, cmd)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
