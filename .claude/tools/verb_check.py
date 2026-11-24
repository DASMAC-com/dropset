#!/usr/bin/env python3
"""The title guard: read the launch line from the issue's title token.

    verb_check.py <verb> <substrate> <n>
    verb_check.py explore bedrock 2126    # exit 0: launch
    verb_check.py task bedrock feeds      # exit 0: a name, no check
    verb_check.py --print chore 2360      # stdout `chore anthropic`: launch

The shell launcher runs this before ``claude`` starts on every issue-keyed
launch that opens a NEW session (a reopen keeps the session it has). A title's
leading ``[<Verb>/<Substrate>]`` token IS the launch line (the grammar is in
``docs/conventions/linear-automation.md`` → "The verb/substrate title token"),
so the launcher reads the substrate from it rather than asking the operator to
type it again.

**Two modes.** The print mode (``--print <verb> <n>``) serves a bare launch,
one with no substrate word typed: it writes ``<verb> <substrate>`` to stdout,
and the launcher takes that substrate as its override. The check mode (a
typed word) refuses a word the title contradicts — the word stays the override
and the outage path, so a typo or a stale title is still caught.

==========================================  ====================================
title                                       outcome
==========================================  ====================================
token matches the verb                      print: launch on its substrate;
                                            check: launch if the word agrees
no token, the verb is ``task``              launch on Bedrock (the one elision);
                                            check: a typed ``anthropic`` refuses
no token, any other verb                    print: refuse, add the token or
                                            type a word; check: launch, the
                                            typed word being the override
a token naming another verb or substrate    refuse, printing the implied line
``[Operator]``, first or after the token    refuse: no agent (ruling 27)
a bracket token that is not one of these    refuse: fix the title
Linear unreachable, or no key               check: warn in one line, launch;
                                            print: ``task`` warns and launches
                                            on its configured substrate, any
                                            other verb refuses naming the word
a name rather than a number                 no check (print: empty stdout)
==========================================  ====================================

**Fail-closed on a mismatch, fail-open on an unreachable Linear where a typed
word or the task default can stand in, and no force flag**: a wrong title is
fixed by editing the title, one edit the operator makes in the tab, while a
launcher that refused every launch whenever Linear blinked would hold the fleet
hostage to a third party. A bare chore, explore or architect has no default to
fall open to — the operator wants the title required — so its outage path is
the typed word, and the refusal says so.

Exit status: 0 launch, 1 refuse (one reason on stderr), 2 usage.
"""

from __future__ import annotations

import re
import sys
from typing import Callable

import linear_api
from linear_issue import TOKEN_SUBSTRATES, TOKEN_VERBS

#: The launch verbs that take an issue number. ``plan`` and ``fleet`` carry
#: none, so the launcher never calls this for them.
LAUNCH_VERBS = ("task", "chore", "explore", "architect")

#: A leading bracket token: ``[Verb/Substrate]`` or a bare ``[Word]``.
_TOKEN = re.compile(r"\A\s*\[([A-Za-z]+)(?:/([A-Za-z]+))?\]")

#: An ``[Operator]`` mark among the title's leading bracket tokens. A
#: hand-executed item may lead with a launch token ahead of it
#: (``[Chore/Anthropic] [Operator] …``), which must not read as a chore to
#: launch.
_OPERATOR = re.compile(r"\A(?:\s*\[[^\]]*\])*?\s*\[operator\]", re.IGNORECASE)

_TITLE_QUERY = """
query IssueTitle($id: String!) {
  issue(id: $id) { title }
}
"""

_NO_AGENT = "this item has no agent; see ruling 27"


class VerbCheckError(Exception):
    """Linear could not answer — the fail-open case."""


def launch_line(verb: str, number: str) -> str:
    """The line to type for a title: the bare form, which reads the token."""
    return f"{verb} {number}"


def token(title: str) -> tuple[str, str, str] | str | None:
    """The title's launch token as ``(verb, substrate, text)``.

    ``None`` when the title carries no token, or the one-line reason to
    refuse when it leads with a bracket that is not a launch token.
    """
    if _OPERATOR.match(title):
        return _NO_AGENT
    match = _TOKEN.match(title)
    if match is None:
        return None
    text = match.group(0).strip()
    word, half = match.group(1).lower(), (match.group(2) or "").lower()
    if word not in TOKEN_VERBS or word == "operator" or half not in TOKEN_SUBSTRATES:
        return f"the title leads with {text}, not a launch token — fix the title"
    return (word, half, text)


def _redirect(said: str, verb: str, number: str) -> str:
    return (
        f"the title says {said} — launch `{launch_line(verb, number)}`, "
        "or edit the title"
    )


def decide(title: str, verb: str, substrate: str, number: str) -> str | None:
    """Check mode: ``None`` to launch, or the one-line reason to refuse.

    A token-less title implies task on Bedrock, so a typed ``task anthropic``
    contradicts it; for the other verbs it names nothing, and the typed word
    is the override the print mode's refusal offers.
    """
    found = token(title)
    if isinstance(found, str):
        return found
    if found is None:
        if verb != "task" or substrate == "bedrock":
            return None
        return _redirect("task on Bedrock (no token)", "task", number)
    if found[:2] == (verb, substrate):
        return None
    return _redirect(found[2], found[0], number)


def infer(title: str, verb: str, number: str) -> tuple[str, str] | str:
    """Print mode: the ``(verb, substrate)`` to launch, or the reason to refuse."""
    found = token(title)
    if isinstance(found, str):
        return found
    if found is None:
        if verb == "task":
            return ("task", "bedrock")
        return (
            "the title carries no launch line; add "
            f"`[{TOKEN_VERBS[verb]}/Bedrock]` or type the substrate "
            f"(`{verb} bedrock {number}`)"
        )
    if found[0] != verb:
        return _redirect(found[2], found[0], number)
    return found[:2]


def fetch_title(api_key: str, identifier: str) -> str:
    """The issue's title, one GraphQL request; any failure is ``VerbCheckError``."""
    data = linear_api.post(
        api_key, _TITLE_QUERY, {"id": identifier}, error=VerbCheckError
    )
    issue = data.get("issue")
    title = issue.get("title") if isinstance(issue, dict) else None
    if not isinstance(title, str):
        raise VerbCheckError(f"no issue {identifier}")
    return title


def _usage() -> int:
    verbs = "|".join(LAUNCH_VERBS)
    print(
        f"usage: verb_check.py {{{verbs}}} {{anthropic|bedrock}} <n>\n"
        f"       verb_check.py --print {{{verbs}}} <n>",
        file=sys.stderr,
    )
    return 2


def _number(target: str) -> str | None:
    """The issue number a target names, unpadded, or ``None`` for a name."""
    number = target.lower().removeprefix("eng-")
    if not number.isascii() or not number.isdigit():
        return None
    return str(int(number))


def _title(number: str, fetch: Callable[[str, str], str]) -> tuple[str | None, str]:
    """The title, or ``None`` and the printable reason Linear did not answer."""
    try:
        api_key = linear_api.env_var("LINEAR_API_KEY", error=VerbCheckError)
        return fetch(api_key, f"ENG-{number}"), ""
    except VerbCheckError as exc:
        # The reason can quote a server's error body, so only printable
        # characters reach the operator's terminal.
        return None, "".join(c for c in str(exc) if c.isprintable())


def _print_mode(argv: list[str], fetch: Callable[[str, str], str]) -> int:
    if len(argv) != 2 or argv[0] not in LAUNCH_VERBS:
        return _usage()
    verb, target = argv
    number = _number(target)
    if number is None:
        return 0
    title, why = _title(number, fetch)
    if title is None:
        if verb == "task":
            print(
                f"task: title not read ({why}) — launching on the configured "
                f"substrate; type it (`task bedrock {number}`) to choose",
                file=sys.stderr,
            )
            return 0
        print(
            f"{verb}: title not read ({why}) — type the substrate "
            f"(`{verb} bedrock {number}` or `{verb} anthropic {number}`) "
            "to launch",
            file=sys.stderr,
        )
        return 1
    line = infer(title, verb, number)
    if isinstance(line, str):
        print(f"{verb}: refusing ENG-{number} — {line}", file=sys.stderr)
        return 1
    print(f"{line[0]} {line[1]}")
    return 0


def run(argv: list[str], *, fetch: Callable[[str, str], str] = fetch_title) -> int:
    if argv[:1] == ["--print"]:
        return _print_mode(argv[1:], fetch)
    if len(argv) != 3 or argv[0] not in LAUNCH_VERBS or argv[1] not in TOKEN_SUBSTRATES:
        return _usage()
    verb, substrate, target = argv
    number = _number(target)
    if number is None:
        return 0

    title, why = _title(number, fetch)
    if title is None:
        print(
            f"{verb}: title not checked ({why}) — launching on the typed `{substrate}`",
            file=sys.stderr,
        )
        return 0

    reason = decide(title, verb, substrate, number)
    if reason is None:
        return 0
    print(f"{verb}: refusing ENG-{number} — {reason}", file=sys.stderr)
    return 1


def main() -> int:
    return run(sys.argv[1:])


if __name__ == "__main__":
    sys.exit(main())
