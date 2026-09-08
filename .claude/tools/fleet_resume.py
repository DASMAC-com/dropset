#!/usr/bin/env python3
"""Reload the whole in-flight fleet into iTerm2 — one resumed session per tab.

After a machine restart or an idle stretch, bringing the fleet back is manual:
open the Dropset project view in Linear, find every In Progress / In Review
issue, open a tab for each, and type the resume verb with its number.
Repetitive, error-prone, and dependent on a Linear round trip the operator has
to make by hand.

This does all of it. For each in-flight issue with no live session it opens a
tab, types ``task resume <n>``, **presses Enter**, and applies the green attend
mark — so the loaded window is a to-attend list and every session is genuinely
resumed, not merely queued for a keystroke. Nothing is left for the operator to
type.

``task resume`` reads that session's own substrate marker, so a mixed fleet of
Bedrock and seat sessions comes back on the right provider per session and this
tool needs no substrate knowledge of its own.

**What counts as in-flight: state TYPE** ``started``, not the state *names*.
That covers **In Progress and In Review**, which is the set that means "a
session owns this" — and In Review is load-bearing since a merged PR whose
follow-up is outstanding stays there deliberately (see
``docs/conventions/linear-automation.md`` → "The Linear state tracks the
SESSION, not the PR"). Matching the type rather than the names means a workflow
rename cannot silently drop a session from the fleet. The failure direction is
safe either way: this only ever *opens* a tab, so an over-wide match costs a tab
and an under-wide one costs a resumed session.

**Skipping a live session** keys on the iTerm session's **name**, which carries
the tag because ``task`` passes ``-n <tag>`` at launch. That is not a
coincidence to rely on loosely — it is the same parity fix that made the
committed launcher match the operative one, so the two are coupled: if ``task``
ever stops setting a display name, this stops recognizing live sessions and
starts double-resuming.

**Read-only by default.** A bare run prints the plan and touches nothing;
``--apply`` opens the tabs. The whole reload is **one** driver round trip rather
than one per tab, so it pays interpreter startup once and cannot interleave with
the tabs it is creating.

**No AppleScript.** iTerm is driven through its Python API, via the shared
``iterm_api`` module — the one owner of iTerm automation in this repo. This tool
and ``session_dispatch.py`` were the only two AppleScript callers, and
consolidating them there retired the language from the toolbox entirely rather
than leaving a second copy to drift.

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

import iterm_api
import linear_api

ENDPOINT = linear_api.ENDPOINT

# The state type that means "a session owns this issue". Linear's set is
# triage / backlog / unstarted / started / completed / canceled.
IN_FLIGHT_TYPE = "started"

# The shell verb each tab is told to run. `task resume <n>` resolves the number
# to the `eng-<n>` worktree, continues that session there, and re-exports the
# substrate that session launched on.
RESUME_VERB = "task resume"

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
    """``ENG-889`` → ``889``, the argument ``task resume`` takes.

    Returns ``None`` for anything that is not an ``ENG-###`` identifier, so a
    differently-shaped one is skipped rather than turned into a bad command.
    """
    match = _IDENT_RE.match(identifier.strip())
    return match.group(1) if match else None


def live_tags() -> set[str]:
    """The tags of sessions already open in iTerm, from the **session** names.

    (Session, not tab: the two are equivalent for the one-pane tabs these
    helpers create, but the distinction matters if a tab is ever split.)

    The name carries a status glyph — `"◐ eng-914"` — so this searches rather
    than matches. A session whose name has no tag (a plain shell, a planning
    session) contributes nothing.

    An iTerm that cannot be reached yields an EMPTY set rather than an error,
    and the direction of that failure is deliberate: an empty set means nothing
    looks live, so `--apply` would open a tab for every in-flight issue. That
    costs duplicate tabs, which is visible and cheap to close. The opposite
    default — treating unreachable as "everything is live" — would silently
    resume nothing at all and report a clean run, which is the failure nobody
    notices.
    """
    try:
        names = iterm_api.session_names()
    except iterm_api.ItermUnavailable as exc:
        print(f"fleet-resume: cannot read live sessions ({exc})", file=sys.stderr)
        return set()
    return {m.group(1) for m in _TAG_RE.finditer("\n".join(names))}


def resume_command(tag: str) -> str:
    """The exact line typed into a freshly opened tab."""
    return f"{RESUME_VERB} {tag}"


def open_tabs(tags: list[str]) -> list[tuple[str, str]]:
    """Open a tab per tag, type its resume verb, and pair each tag with its tty.

    The tty comes back so the caller can apply the attend mark to each new tab —
    a coprocess bound to a key can only reach its own session, so the mark has
    to be driven from here.

    Tags whose tty could not be read are OMITTED from the returned pairs rather
    than carried with a placeholder, which keeps the pair list meaning "these
    can be marked". The caller reconstructs the shortfall by difference against
    what it requested; see the `no_tty` handling in :func:`run`, which exists
    because an earlier version reported a clean summary over a total failure.
    """
    ttys = iterm_api.open_tabs([resume_command(tag) for tag in tags])
    return [(tag, tty) for tag, tty in zip(tags, ttys) if tty]


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
        try:
            pairs = open_tabs(tags)
        except iterm_api.ItermUnavailable as exc:
            # Best effort is the bar, and the operator loses the convenience
            # rather than the information: name every verb that went untyped so
            # the fleet can be brought back by hand.
            raise FleetResumeError(
                f"{exc}\n  run these by hand:\n"
                + "\n".join(f"    {resume_command(tag)}" for tag in tags)
            ) from exc
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
