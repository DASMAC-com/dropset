#!/usr/bin/env python3
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
  often scrolls off the top past the ``--tail`` window), then the last
  ``--tail`` lines of the log, the exit code, and the log path, so the model
  can ``Read`` more of the log by slice if it needs to.

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

Tests live in the sibling ``test_run_quiet.py`` (stdlib ``unittest``), runnable
as ``python3 .claude/tools/test_run_quiet.py``.
"""

from __future__ import annotations

import collections
import os
import subprocess
import sys
import tempfile

# Default number of trailing log lines shown on failure.
DEFAULT_TAIL = 50

# Cap on how many failed-hook index lines to surface, so a pathological log that
# prints "…Failed" thousands of times can't balloon the summary.
MAX_FAILED_LINES = 40

# Where captured logs land: a stable subdir of the system temp dir (usually
# /tmp/claude-run-quiet). One file per run, named for the command and pid so
# concurrent runs don't collide.
LOG_DIR = os.path.join(tempfile.gettempdir(), "claude-run-quiet")

# Exit code used when the command can't be launched at all (mirrors the shell's
# 127 "command not found").
LAUNCH_FAILURE_CODE = 127


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


def read_tail_and_count(path, tail):
    """Return (line_count, last-`tail`-lines-as-text, failed_hook_lines, truncated).

    ``failed_hook_lines`` is the list of ``…Failed`` result lines found anywhere
    in the log (newline-stripped, order preserved, capped at MAX_FAILED_LINES) —
    surfaced on failure so a failing hook that scrolled off the tail window is
    still reported. ``truncated`` is True only when *more* than
    MAX_FAILED_LINES such lines were found (so the list holds just the first
    MAX_FAILED_LINES) — this is what distinguishes a genuine cap from a log with
    exactly MAX_FAILED_LINES failures and no more. Uses a bounded deque so a
    huge log never sits in memory in full.
    """
    count = 0
    dq = collections.deque(maxlen=tail if tail > 0 else 0)
    failed = []
    truncated = False
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        for line in fh:
            count += 1
            if tail > 0:
                dq.append(line)
            if is_failed_hook_line(line):
                if len(failed) < MAX_FAILED_LINES:
                    failed.append(line.rstrip("\n"))
                else:
                    truncated = True
    return count, "".join(dq), failed, truncated


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
            completed = subprocess.run(
                cmd,
                stdout=log_file,
                stderr=subprocess.STDOUT,
                stdin=subprocess.DEVNULL,
                text=True,
            )
        code = completed.returncode
    except FileNotFoundError:
        sys.stderr.write("✗ %s — command not found: %s\n" % (display, cmd[0]))
        return LAUNCH_FAILURE_CODE
    except OSError as exc:
        sys.stderr.write("✗ %s — could not launch: %s\n" % (display, exc))
        return LAUNCH_FAILURE_CODE

    lines, tail_text, failed, truncated = read_tail_and_count(log_path, tail)

    if code == 0:
        sys.stdout.write(
            "✓ %s (exit 0, %d lines; log: %s)\n" % (display, lines, log_path)
        )
        return 0

    sys.stdout.write(
        "✗ %s (exit %d, %d lines; log: %s)\n" % (display, code, lines, log_path)
    )
    if failed:
        more = " (truncated, more omitted)" if truncated else ""
        sys.stdout.write("--- failed hooks (%d)%s ---\n" % (len(failed), more))
        for fl in failed:
            sys.stdout.write(fl + "\n")
    if tail > 0 and tail_text:
        shown = min(tail, lines)
        sys.stdout.write("--- last %d line(s) ---\n" % shown)
        sys.stdout.write(tail_text)
        if not tail_text.endswith("\n"):
            sys.stdout.write("\n")
    return code


def main(argv):
    try:
        tail, label, cmd = parse_args(argv)
    except UsageError as exc:
        sys.stderr.write("run_quiet.py: %s\n" % exc)
        sys.stderr.write("usage: run_quiet.py [--tail N] [--label L] -- CMD ARGS...\n")
        return 2
    return run(tail, label, cmd)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
