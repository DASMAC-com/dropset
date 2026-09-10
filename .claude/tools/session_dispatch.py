#!/usr/bin/env python3
"""Open a new iTerm tab per session verb and type the verb into it.

The planning session's dispatch arm: it names a task that is ready to start,
offers to start it, and on the operator's yes runs this. The ask-then-yes IS
the authorization, the same class as `fleet go` — this tool adds no gate of its
own.

    python3 .claude/tools/session_dispatch.py task 1234
    python3 .claude/tools/session_dispatch.py task local 1234
    python3 .claude/tools/session_dispatch.py task 1234 + plan + housekeeping
    python3 .claude/tools/session_dispatch.py --dry-run plan

**A dispatch opens a TAB in the dispatching session's own window**, never a new
window: the operator drives the fleet from one window, so a dispatched session
has to land beside the planning session that started it rather than as another
window to hunt for. Operator ruling, 2026-09-10, on seeing two resumes arrive as
separate windows. This reverses an earlier "one iTerm window per session is
load-bearing" rationale that used to be stated here, in the `plan` skill's step
5, and in the local-integrations convention doc.

A tab still never falls back to typing into the CURRENT session — the failure
path prints the verb instead. Typing a launch verb into a session that is
already running something remains the one outcome worse than not dispatching at
all, and that hazard was never what the window-per-session rule bought.

Several verbs in one invocation are separated by a bare `+`, which keeps the
one-verb form untouched and costs one driver round trip for the whole batch
rather than one per tab. `+` is safe as a separator precisely because the
grammar below refuses it as a verb or an argument.

WHY PURE PYTHON AND NOT APPLESCRIPT. The machine is Apple-only and single-user,
and iTerm's Python API is its supported automation surface; the AppleScript path
this replaces is one more language in the toolbox for no capability gained. Best
effort is the ratified bar — getting it working beats getting it bulletproof —
which is why every failure path below prints the verb it *would* have typed, so
the operator can paste it by hand and lose nothing but the convenience.

The iTerm driving itself lives in `iterm_api`, shared with `fleet_resume.py`;
see there for the interpreter dance that keeps this repo's own Python
stdlib-only while still reaching a library that is not.
"""

from __future__ import annotations

import argparse
import re
import shlex
import sys

import iterm_api

#: The verbs this can dispatch, each mapped to a validator for its arguments.
#:
#: THIS IS INPUT VALIDATION, NOT POLICY. The ratified design says the dispatcher
#: takes the verb and its arguments verbatim and adds no policy of its own —
#: which it does: `local` passes straight through, so the substrate choice stays
#: at the call site where it belongs. What this table refuses is not a *choice*
#: but a *shape*: the tool types its argument into an interactive shell, so
#: anything that is not one of these forms would be arbitrary command execution
#: wearing a verb's name. A caller wanting a shell command has a shell.
#: `\Z` and `re.ASCII`, both deliberately, and neither is theoretical.
#: Python's `$` matches just before a trailing newline, so `^\d+$` accepts
#: `"1234\n"` — a different shape than the shell's grammar takes, reaching a
#: line that gets typed into a live shell. And bare `\d` is Unicode-aware, so
#: `eng-١٢٣` would validate as a tag. `shlex.quote` downstream keeps either
#: from being exploitable; the point of this table is to refuse the shape.
_TAG = re.compile(r"\A(?:eng-)?\d+\Z", re.ASCII)
_NAME = re.compile(r"\A[a-z0-9][a-z0-9-]*\Z", re.ASCII)


def validate(argv: list[str]) -> list[str]:
    """Return the verb words to type, or raise ``ValueError`` naming the problem.

    Mirrors the grammar in `.claude/shell/init.zsh`. The two must agree, and the
    failure of disagreement is one-directional and cheap: a form this rejects
    that the shell accepts costs the operator a hand-typed launch, while a form
    this accepts that the shell rejects costs a confused error in a fresh
    window. Neither is silent, which is what makes duplicating the grammar
    acceptable rather than a second source of truth to keep in sync.
    """
    if not argv:
        raise ValueError("no verb given")

    verb, args = argv[0], argv[1:]

    if verb == "task":
        if args and args[0] in ("local", "resume"):
            sub, rest = args[0], args[1:]
            # `task resume` alone is the picker, which is a legitimate thing to
            # dispatch; `task local` alone is not, since there is no worktree to
            # open. The shell says the same.
            if sub == "resume" and not rest:
                return ["task", "resume"]
            if len(rest) != 1 or not _TAG.match(rest[0]):
                raise ValueError(f"`task {sub}` takes one eng-### tag or number")
            return ["task", sub, rest[0]]
        if len(args) != 1 or not _TAG.match(args[0]):
            raise ValueError("`task` takes one eng-### tag or number")
        return ["task", args[0]]

    if verb == "explore":
        if not args:
            return ["explore"]
        if args[0] == "resume":
            if len(args) != 2 or not _NAME.match(args[1]):
                raise ValueError("`explore resume` takes one name")
            return ["explore", "resume", args[1]]
        if len(args) != 1 or not _NAME.match(args[0]):
            raise ValueError("`explore` takes at most one lowercase name")
        return ["explore", args[0]]

    if verb == "architect":
        if len(args) != 1 or not _NAME.match(args[0]):
            raise ValueError("`architect` takes one lowercase topic")
        return ["architect", args[0]]

    if verb == "fleet":
        if not args:
            return ["fleet"]
        if args == ["go"]:
            return ["fleet", "go"]
        raise ValueError("`fleet` takes nothing or `go`")

    if verb in ("plan", "housekeeping"):
        if args:
            raise ValueError(f"`{verb}` takes no arguments")
        return [verb]

    raise ValueError(f"unknown verb {verb!r}")


#: Separates one verb from the next in a batch dispatch.
#:
#: A separator rather than inference, deliberately. The verb names are a closed
#: set, so consecutive verbs look as if they could be split apart without one —
#: but `explore` takes an
#: optional lowercase name and `_NAME` matches `plan`, so `explore plan` is
#: genuinely ambiguous between one verb and two. On a boundary that types into a
#: live shell, an explicit separator beats a heuristic that is right most of the
#: time. `+` is not a legal verb or argument under the grammar above, so it can
#: never collide with one; `--` was rejected because argparse strips a lone `--`
#: even out of a REMAINDER.
VERB_SEPARATOR = "+"


def split_verbs(words: list[str]) -> list[list[str]]:
    """Split a flat word list into one word list per verb, on `+`.

    Raises ``ValueError`` on an empty group — a doubled, leading, or trailing
    separator. That is a typo rather than a request, and the alternative (quietly
    dropping the empty group) would dispatch a batch of a different size than the
    caller wrote, which is the sort of near-miss this tool must not paper over.
    """
    groups: list[list[str]] = [[]]
    for word in words:
        if word == VERB_SEPARATOR:
            groups.append([])
        else:
            groups[-1].append(word)

    if len(groups) > 1 and any(not group for group in groups):
        raise ValueError(
            f"empty verb around a `{VERB_SEPARATOR}` separator — "
            f"write `verb {VERB_SEPARATOR} verb`, with a verb on each side"
        )
    return groups


def command_line(words: list[str]) -> str:
    """The exact text to type. Quoted defensively even though `validate` has
    already constrained every word to a shell-safe shape — the quoting is a
    property of this function, not of today's caller."""
    return " ".join(shlex.quote(word) for word in words)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="session_dispatch.py",
        description=(
            "Open an iTerm2 tab per session verb, in this window, and type each. "
            f"Separate several verbs with a bare `{VERB_SEPARATOR}`."
        ),
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print the lines that would be typed, and do nothing else",
    )
    parser.add_argument(
        "verb",
        nargs=argparse.REMAINDER,
        help=f"the session verb(s), separated by `{VERB_SEPARATOR}`",
    )
    args = parser.parse_args(argv)

    # REMAINDER swallows everything after the first positional, so a trailing
    # `--dry-run` never registers as the flag — `session_dispatch.py plan
    # --dry-run` would not dry-run. Today `validate` happens to reject the
    # stray word so nothing is typed, but that is luck rather than design, and
    # dry-run is the look-before-you-type affordance on a boundary that types
    # into a live shell. Catch it explicitly and say where the flag goes.
    if any(word in ("--dry-run", "-n") for word in args.verb):
        print(
            "session-dispatch: --dry-run must come BEFORE the verb "
            "(argparse REMAINDER swallows anything after it)",
            file=sys.stderr,
        )
        return 2

    # Every verb is validated BEFORE any tab is opened, so a batch with one bad
    # verb dispatches nothing rather than half of itself. A partial batch is the
    # bad outcome here: the caller cannot tell which tabs it got without reading
    # them, and re-running to fix the typo would double the ones that worked.
    try:
        commands = [command_line(validate(group)) for group in split_verbs(args.verb)]
    except ValueError as exc:
        print(f"session-dispatch: {exc}", file=sys.stderr)
        return 2

    if args.dry_run:
        for command in commands:
            print(command)
        return 0

    try:
        ttys = iterm_api.open_tabs(commands)
    except iterm_api.ItermUnavailable as exc:
        # Loud, and useful. This is what makes "best effort" an acceptable bar:
        # the operator loses the convenience, never the launch. Every verb is
        # named, not just the first — a batch that could not be typed leaves the
        # whole batch to hand-run.
        print(f"session-dispatch: {exc}", file=sys.stderr)
        listing = "\n".join(f"  {command}" for command in commands)
        print(
            f"session-dispatch: run {'these' if len(commands) > 1 else 'this'} "
            f"by hand instead:\n\n{listing}\n",
            file=sys.stderr,
        )
        return 1

    # The confirmation the operator reads. One line per verb pairs it with the
    # tty its tab landed on, which is what makes a batch auditable rather than
    # merely finished; `open_tabs` keeps that list positional, so a `None` marks
    # the one tab whose tty could not be read instead of shifting the rest.
    for command, tty in zip(commands, ttys):
        print(f"{command} -> {tty or 'tty unknown'}")

    # The roll-call, best-effort and deliberately AFTER the per-verb lines. It is
    # the independent check that the tabs really exist — the ttys above come from
    # the call that made them, so they cannot disprove their own success, whereas
    # a fresh listing can. Losing the roll-call must never turn a dispatch that
    # worked into a non-zero exit, so its failure only annotates.
    try:
        print(f"sessions now open: {', '.join(iterm_api.session_names())}")
    except iterm_api.ItermUnavailable as exc:
        print(f"session-dispatch: tabs opened; roll-call unavailable ({exc})")
    return 0


def main() -> int:
    return run(sys.argv[1:])


if __name__ == "__main__":
    sys.exit(main())
