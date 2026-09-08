#!/usr/bin/env python3
"""Open a new iTerm window and type one session verb into it.

The planning session's dispatch arm: it names a task that is ready to start,
offers to start it, and on the operator's yes runs this. The ask-then-yes IS
the authorization, the same class as `fleet go` — this tool adds no gate of its
own.

    python3 .claude/tools/session_dispatch.py task 1234
    python3 .claude/tools/session_dispatch.py task local 1234
    python3 .claude/tools/session_dispatch.py --dry-run plan

**One iTerm window per session is load-bearing**, not cosmetic: it is how the
operator talks to the fleet. So this creates a window rather than a tab, and a
dispatch that cannot create one fails rather than falling back to the current
session — typing a launch verb into a window that is already running something
is the one outcome worse than not dispatching at all.

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
_TAG = re.compile(r"^(?:eng-)?\d+$")
_NAME = re.compile(r"^[a-z0-9][a-z0-9-]*$")


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


def command_line(words: list[str]) -> str:
    """The exact text to type. Quoted defensively even though `validate` has
    already constrained every word to a shell-safe shape — the quoting is a
    property of this function, not of today's caller."""
    return " ".join(shlex.quote(word) for word in words)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="session_dispatch.py",
        description="Open a new iTerm2 window and type one session verb.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print the line that would be typed, and do nothing else",
    )
    parser.add_argument("verb", nargs=argparse.REMAINDER, help="the session verb")
    args = parser.parse_args(argv)

    try:
        words = validate(args.verb)
    except ValueError as exc:
        print(f"session-dispatch: {exc}", file=sys.stderr)
        return 2

    command = command_line(words)

    if args.dry_run:
        print(command)
        return 0

    try:
        iterm_api.open_window(command)
    except iterm_api.ItermUnavailable as exc:
        # Loud, and useful. This is what makes "best effort" an acceptable bar:
        # the operator loses the convenience, never the launch.
        print(f"session-dispatch: {exc}", file=sys.stderr)
        print(
            f"session-dispatch: run this by hand instead:\n\n  {command}\n",
            file=sys.stderr,
        )
        return 1
    return 0


def main() -> int:
    return run(sys.argv[1:])


if __name__ == "__main__":
    sys.exit(main())
