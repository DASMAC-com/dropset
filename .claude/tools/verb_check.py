#!/usr/bin/env python3
"""The title guard: refuse a launch line the issue's title token contradicts.

    verb_check.py <verb> <substrate> <n>
    verb_check.py explore bedrock 2126    # exit 0: launch
    verb_check.py task bedrock feeds      # exit 0: a name, no check

The shell launcher runs this before ``claude`` starts on every issue-keyed
launch that opens a NEW session (a reopen keeps the session it has). A title's
leading ``[<Verb>/<Substrate>]`` token IS the launch line (the grammar is in
``docs/conventions/linear-automation.md`` → "The verb/substrate title token"),
so a launch that disagrees with it is a typo or a stale title, and either way
the wrong session is about to start.

==========================================  ====================================
title                                       outcome
==========================================  ====================================
token matches verb and substrate            launch
no token, launch is ``task`` on Bedrock     launch (the one elision)
no token, any other launch                  refuse: the title says task/Bedrock
a token naming another verb or substrate    refuse, printing the implied line
``[Operator]``                              refuse: no agent (ruling 27)
a bracket token that is not one of these    refuse: fix the title
Linear unreachable, or no key               warn in one line, launch
a name rather than a number                 no check
==========================================  ====================================

**Fail-closed on a mismatch, fail-open on an unreachable Linear, and no force
flag**: a wrong title is fixed by editing the title, which is one edit the
operator makes in the tab, while a launcher that refused every launch whenever
Linear blinked would hold the fleet hostage to a third party.

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

_TITLE_QUERY = """
query IssueTitle($id: String!) {
  issue(id: $id) { title }
}
"""


class VerbCheckError(Exception):
    """Linear could not answer — the fail-open case."""


def launch_line(verb: str, substrate: str, number: str) -> str:
    """The line to type for a token; ``task`` on Bedrock is the bare form."""
    if (verb, substrate) == ("task", "bedrock"):
        return f"task {number}"
    return f"{verb} {substrate} {number}"


def decide(title: str, verb: str, substrate: str, number: str) -> str | None:
    """``None`` to launch, or the one-line reason to refuse."""
    match = _TOKEN.match(title)
    if match is None:
        if (verb, substrate) == ("task", "bedrock"):
            return None
        return (
            f"the title says task on Bedrock (no token) — launch "
            f"`{launch_line('task', 'bedrock', number)}`, or edit the title"
        )
    token = match.group(0).strip()
    word, half = match.group(1).lower(), (match.group(2) or "").lower()
    if word == "operator" and not half:
        return "this item has no agent; see ruling 27"
    if word not in TOKEN_VERBS or word == "operator" or half not in TOKEN_SUBSTRATES:
        return f"the title leads with {token}, not a launch token — fix the title"
    if (word, half) == (verb, substrate):
        return None
    return (
        f"the title says {token} — launch "
        f"`{launch_line(word, half, number)}`, or edit the title"
    )


def fetch_title(api_key: str, identifier: str) -> str:
    """The issue's title, one GraphQL request; any failure is ``VerbCheckError``."""
    data = linear_api.post(
        api_key, _TITLE_QUERY, {"id": identifier}, error=VerbCheckError
    )
    issue = data.get("issue") or {}
    title = issue.get("title")
    if not isinstance(title, str):
        raise VerbCheckError(f"no issue {identifier}")
    return title


def run(argv: list[str], *, fetch: Callable[[str, str], str] = fetch_title) -> int:
    if len(argv) != 3 or argv[0] not in LAUNCH_VERBS or argv[1] not in TOKEN_SUBSTRATES:
        print(
            f"usage: verb_check.py {{{'|'.join(LAUNCH_VERBS)}}} "
            "{anthropic|bedrock} <n>",
            file=sys.stderr,
        )
        return 2
    verb, substrate, target = argv
    number = target.lower().removeprefix("eng-")
    if not number.isascii() or not number.isdigit():
        return 0
    number = str(int(number))

    try:
        api_key = linear_api.env_var("LINEAR_API_KEY", error=VerbCheckError)
        title = fetch(api_key, f"ENG-{number}")
    except VerbCheckError as exc:
        print(f"{verb}: title not checked ({exc}) — launching", file=sys.stderr)
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
