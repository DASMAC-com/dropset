#!/usr/bin/env python3
"""Reload the whole in-flight fleet into iTerm2 — one resumed session per tab.

After a machine restart or an idle stretch, bringing the fleet back is manual:
open the Dropset project view in Linear, find every In Progress / In Review
issue, open a tab for each, and type the resume verb with its number.
Repetitive, error-prone, and dependent on a Linear round trip the operator has
to make by hand.

This does all of it. For each in-flight issue with no live session it opens a
tab, types ``raps <n>``, **presses Enter**, and applies the green attend mark —
so the loaded window is a to-attend list and every session is genuinely
resumed, not merely queued for a keystroke. Nothing is left for the operator to
type.

**What counts as in-flight: state TYPE** ``started``, not the state *names*.
That covers **In Progress and In Review**, which is the set that means "a
session owns this" — and In Review is load-bearing since a merged PR whose
follow-up is outstanding stays there deliberately (see
``docs/conventions/linear-automation.md`` → "The Linear state tracks the
SESSION, not the PR"). Matching the type rather than the names means a workflow
rename cannot silently drop a session from the fleet. The failure direction is
safe either way: this only ever *opens* a tab, so an over-wide match costs a tab
and an under-wide one costs a resumed session.

**Skipping a live session** keys on the iTerm tab's **name**, which carries the
tag because ``aps`` passes ``-n <tag>`` at launch. That is not a coincidence to
rely on loosely — it is the same parity fix that made the committed ``aps``
match the operative one, so the two are coupled: if ``aps`` ever stops setting
a display name, this stops recognizing live sessions and starts double-resuming.

**Read-only by default.** A bare run prints the plan and touches nothing;
``--apply`` opens the tabs. Every AppleScript is emitted as **one** script per
run rather than one per tab, so the whole reload is a single ``osascript``.

Stdlib only. A Python skill-tool under ``.claude/tools/`` — deliberately **not**
a Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import linear_api

ENDPOINT = linear_api.ENDPOINT

# The state type that means "a session owns this issue". Linear's set is
# triage / backlog / unstarted / started / completed / canceled.
IN_FLIGHT_TYPE = "started"

# The shell verb each tab is told to run. `raps <n>` resolves the number to the
# `eng-<n>` worktree and continues that session there.
RESUME_VERB = "raps"

# An `ENG-###` identifier, or the tag inside an iTerm session name. The name
# carries a status glyph prefix ("◐ eng-914"), so this is a search, not a match.
_TAG_RE = re.compile(r"\beng-(\d+)\b", re.IGNORECASE)

_IDENT_RE = re.compile(r"^ENG-(\d+)$", re.IGNORECASE)

# Where the attend-mark script lives, relative to this file.
_ATTEND = Path(__file__).resolve().parent.parent / "scripts" / "iterm-attend.sh"

_IN_FLIGHT_QUERY = """
query InFlight($filter: IssueFilter, $first: Int!, $after: String) {
  issues(filter: $filter, first: $first, after: $after) {
    pageInfo { hasNextPage endCursor }
    nodes {
      identifier
      title
      state { name type }
    }
  }
}
"""

# Enumerate every session's name across every window and tab. Newline-joined so
# the caller parses lines rather than an AppleScript list literal.
_LIST_SESSIONS = """
set out to ""
tell application "iTerm2"
  repeat with w in windows
    repeat with t in tabs of w
      repeat with s in sessions of t
        set out to out & (name of s) & linefeed
      end repeat
    end repeat
  end repeat
end tell
return out
"""


class FleetResumeError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def _env(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value:
        raise FleetResumeError(
            f"{name} is unset — export it in your shell profile "
            f"(see docs/conventions/linear-automation.md)"
        )
    return value


def _post(api_key: str, query: str, variables: dict) -> dict:
    """POST a GraphQL operation and return its ``data``.

    Delegates to the shared transport rather than keeping a third HTTP idiom in
    this directory — which is also what gives this tool the redirect refusal (a
    followed 3xx would re-send the ``Authorization`` header to a new host).
    """
    return linear_api.post(
        api_key,
        query,
        variables,
        endpoint=ENDPOINT,
        error=FleetResumeError,
    )


def in_flight(api_key: str, project_id: str) -> list[dict]:
    """Every issue in the project whose state type is ``started``."""
    nodes: list[dict] = []
    after = None
    while True:
        data = _post(
            api_key,
            _IN_FLIGHT_QUERY,
            {
                "filter": {
                    "project": {"id": {"eq": project_id}},
                    "state": {"type": {"eq": IN_FLIGHT_TYPE}},
                },
                "first": 50,
                "after": after,
            },
        )
        page = data.get("issues") or {}
        nodes.extend(page.get("nodes") or [])
        info = page.get("pageInfo") or {}
        if not info.get("hasNextPage"):
            return nodes
        after = info.get("endCursor")
        if not after:
            # Relay guarantees a non-null endCursor whenever hasNextPage is
            # true, so this is unreachable against a conforming server. It is
            # here because the failure mode if it ever happened is the one
            # thing this module has no other defense against: `after` resets to
            # None, the identical first-page query is re-issued, and the loop
            # never terminates. Every other malformed-response path raises.
            raise FleetResumeError(
                "Linear reported another page but returned no cursor — "
                "refusing to re-issue the same query indefinitely"
            )


def tag_of(identifier: str) -> str | None:
    """``ENG-889`` → ``889``, the argument ``raps`` takes.

    Returns ``None`` for anything that is not an ``ENG-###`` identifier, so a
    differently-shaped one is skipped rather than turned into a bad command.
    """
    match = _IDENT_RE.match(identifier.strip())
    return match.group(1) if match else None


def _osascript(script: str) -> str:
    try:
        completed = subprocess.run(
            ["osascript", "-"],
            input=script,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        raise FleetResumeError(f"cannot run osascript: {exc}") from exc
    if completed.returncode != 0:
        detail = (completed.stderr or "").strip() or f"exit {completed.returncode}"
        raise FleetResumeError(f"osascript failed: {detail}")
    return completed.stdout


def live_tags() -> set[str]:
    """The tags of sessions already open in iTerm, from the **session** names.

    (Session, not tab: `_LIST_SESSIONS` enumerates `name of s`. The two are
    equivalent for the one-pane tabs these helpers create, but the distinction
    matters if a tab is ever split.)

    The name carries a status glyph, so this searches rather than matches.
    A session whose name has no tag (a plain shell, a planning session)
    contributes nothing.
    """
    out = _osascript(_LIST_SESSIONS)
    return {m.group(1) for m in _TAG_RE.finditer(out)}


def _applescript_literal(value: str) -> str:
    """Quote a string for AppleScript source.

    Backslash and double-quote are escaped, which is what the values here
    actually need: every one is an ``eng-<digits>`` tag from :func:`tag_of`, so
    neither character can occur. It is done anyway because the result is
    assembled into a script that gets *executed*, and "the input is
    constrained" is a property of today's caller, not of this function.

    **Not a general AppleScript quoter.** A literal newline or other control
    character would pass through unescaped and break the emitted source —
    AppleScript string literals have no escape for those, so handling them
    would mean splitting into a concatenation of ``character id`` terms. That
    is unnecessary for a digit-run tag and is deliberately not implemented; if
    this ever quotes free-form text, it needs that work first.
    """
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def open_script(tags: list[str]) -> str:
    """One AppleScript that opens a tab per tag and reports each tab's tty.

    Emitted as a single script rather than one per tag: a per-tag `osascript`
    would pay process startup per session and interleave badly with the tabs
    it is creating. The tty comes back so the caller can apply the attend mark
    to each new tab — a coprocess bound to a key can only reach its own
    session, so the mark has to be driven from here.
    """
    lines = ['set out to ""', 'tell application "iTerm2"', "  set w to current window"]
    for tag in tags:
        command = _applescript_literal(f"{RESUME_VERB} {tag}")
        lines += [
            "  tell w",
            "    set t to (create tab with default profile)",
            "  end tell",
            "  set s to current session of t",
            f"  write s text {command} newline yes",
            f'  set out to out & {_applescript_literal(tag)} & " " & (tty of s)'
            " & linefeed",
        ]
    lines += ["end tell", "return out"]
    return "\n".join(lines)


def parse_open_result(out: str) -> list[tuple[str, str]]:
    """``"889 /dev/ttys004"`` lines → ``[(tag, tty)]``."""
    pairs = []
    for line in out.splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[1].startswith("/dev/"):
            pairs.append((parts[0], parts[1]))
    return pairs


def mark_attention(tty: str) -> bool:
    """Apply the green attend mark to ``tty``. True on success.

    Shells out to the committed `iterm-attend.sh` rather than re-emitting the
    escape here, so the palette has one owner. `--mark` sets green outright
    instead of toggling: a toggle's outcome depends on the tab's history, and a
    launcher wants green, not "the other one".
    """
    if not _ATTEND.exists():
        return False
    try:
        completed = subprocess.run(
            [str(_ATTEND), "--tty", tty, "--mark"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        # `.exists()` is not `.access(X_OK)`: a checkout that stripped the exec
        # bit raises PermissionError here — an OSError, not a FleetResumeError,
        # so uncaught it would surface as a raw traceback at the worst possible
        # moment, AFTER every tab is already open and resumed and before
        # `result` is ever printed. The tint is the only casualty; report it
        # through `unmarked` like any other failed mark.
        return False
    return completed.returncode == 0


def plan(api_key: str, project_id: str) -> dict:
    """What a run would do, without doing any of it."""
    issues = in_flight(api_key, project_id)
    live = live_tags()
    resume, skipped, unrecognized = [], [], []
    for issue in issues:
        identifier = issue.get("identifier") or ""
        tag = tag_of(identifier)
        entry = {
            "identifier": identifier,
            "tag": tag,
            "title": issue.get("title"),
            "state": (issue.get("state") or {}).get("name"),
        }
        if tag is None:
            unrecognized.append(entry)
        elif tag in live:
            skipped.append(entry)
        else:
            resume.append(entry)
    return {
        "in_flight": len(issues),
        "live_tags": sorted(live),
        "resume": resume,
        "skipped_already_live": skipped,
        "unrecognized_identifier": unrecognized,
    }


def summarize(result: dict) -> str:
    """One human line."""
    parts = [
        f"fleet-resume | {result['in_flight']} in flight",
        f"{len(result['resume'])} to resume",
        f"{len(result['skipped_already_live'])} already live",
    ]
    if result["unrecognized_identifier"]:
        parts.append(f"{len(result['unrecognized_identifier'])} unrecognized")
    if result.get("opened") is not None:
        parts.append(f"{result['opened']} opened")
        unmarked = result.get("unmarked") or []
        if unmarked:
            # NAME them, and count them correctly. This used to interpolate the
            # list itself into a slot reading "N could not be marked", so the
            # summary printed a raw Python list repr where a count belonged.
            parts.append(f"{len(unmarked)} could not be marked: {', '.join(unmarked)}")
        # The silent path, and the one that actually bit: tabs opened but no
        # tty came back, so nothing was marked and `unmarked` was empty too —
        # a clean-looking summary over a total mark failure. Report the
        # shortfall on its own, because an empty `unmarked` is otherwise
        # indistinguishable from complete success.
        missing = result.get("no_tty") or []
        if missing:
            # Phrased against what was REQUESTED, not asserted as "opened".
            # Absence from `pairs` covers two cases — a tab that opened but
            # whose tty did not parse, and a tab that never opened at all
            # (osascript aborted, or returned nothing) — and this line cannot
            # tell them apart. Claiming "opened" would also contradict the
            # "N opened" it sits beside, which counts only what parsed.
            # Omit the denominator when it is unknown rather than defaulting it
            # to the numerator — "N of N requested" would assert that EVERY
            # requested tab failed, which is the same species of unverified
            # claim this rewording removed from the previous version.
            requested = result.get("requested")
            scope = f" of {requested} requested" if requested else ""
            parts.append(
                f"{len(missing)}{scope} produced no tty, so "
                f"nothing was marked for: {', '.join(missing)}"
            )
    return " | ".join(parts)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="fleet_resume.py",
        description=(
            "Open an iTerm tab per in-flight Linear issue and resume its "
            "session there. Read-only unless --apply is passed."
        ),
    )
    parser.add_argument(
        "--apply",
        action="store_true",
        help="actually open the tabs (without this, prints the plan and exits)",
    )
    args = parser.parse_args(argv[1:])

    api_key = _env("LINEAR_API_KEY")
    project_id = _env("LINEAR_PROJECT_ID")

    result = plan(api_key, project_id)
    if args.apply and result["resume"]:
        tags = [entry["tag"] for entry in result["resume"]]
        pairs = parse_open_result(_osascript(open_script(tags)))
        unmarked = [tag for tag, tty in pairs if not mark_attention(tty)]
        result["opened"] = len(pairs)
        result["unmarked"] = unmarked
        # A tab whose tty never came back is unreachable for marking, and its
        # absence from `pairs` also kept it out of `unmarked` — so a total
        # tty-parse failure reported "0 opened" and no mark complaint at all,
        # over a window full of freshly opened tabs. Track the shortfall
        # explicitly against what was REQUESTED rather than inferring it from
        # what parsed.
        resolved = {tag for tag, _ in pairs}
        result["no_tty"] = [tag for tag in tags if tag not in resolved]
        result["requested"] = len(tags)
    elif args.apply:
        result["opened"] = 0
        result["unmarked"] = []
        result["no_tty"] = []
        result["requested"] = 0

    json.dump(result, sys.stdout, indent=2)
    sys.stdout.write("\n")
    if not args.apply:
        print("(read-only: pass --apply to open the tabs)", file=sys.stderr)
    print(summarize(result), file=sys.stderr)
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except FleetResumeError as exc:
        print(f"fleet-resume: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
