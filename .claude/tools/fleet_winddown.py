#!/usr/bin/env python3
"""The fleet wind-down's listing and roll-call — behind the ``fleet down`` verb.

When the operator says the fleet is closing out, the scheduler (or the
operator) runs this. It lists every in-flight issue (state type ``started``,
the set ``fleet_resume`` reopens tomorrow), splits it with **no time
argument** — **finish** when the issue is In Review (``review-pr`` has handed
off at the queue), **pause** for everything else, In Progress and chores
included — and prints the two message texts the scheduler sends, one
``SendMessage`` per live session.

Each row carries whether the session is **live** (its iTerm session name is
the tag, as ``fleet_resume.live_tags`` reads it) and, from one GraphQL call
over every branch, its open PR's ``mergeable``, ``mergeStateStatus`` and merge
queue entry. A re-run after the messages is therefore the roll-call: who has
parked (no longer live), who is live, and which live finisher is already
**doomed** — its path to merge is no longer linear, so the message's rule
stands it down at once. Nothing polls; the scheduler re-runs the listing when a
stand-down message arrives.

The rule and the stand-down act live in ``docs/conventions/fleet-winddown.md``;
the message texts below carry them whole, since an explore session's skill
has no room for a pointer.

Usage:

    python3 .claude/tools/fleet_winddown.py           # ranked summary
    python3 .claude/tools/fleet_winddown.py --json    # every row, as JSON

Read-only: it opens, closes and messages nothing. Stdlib only; a Python
skill-tool under ``.claude/tools/`` — deliberately **not** a Cargo workspace
member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import json
import sys

import fleet_resume

FINISH, PAUSE = "finish", "pause"

# What makes a finisher's path non-linear before anything else happens: a
# rebase is owed. CI red and a dequeue are the session's own to see.
DOOMED_MERGEABLE = {"CONFLICTING"}
DOOMED_MERGE_STATE = {"DIRTY"}

STAND_DOWN = (
    "Stand down: append a **Resume brief** on the issue naming your next step "
    "(and the PR, when one is open) with linear_issue.py append; confirm the "
    "tree is clean and pushed (an unsigned checkpoint is fine); print "
    '"Standing down; ready to resume tomorrow." as your last line and send '
    "the scheduler that same line; then run "
    "`close_session.py --park --session-id <id>` as your last tool call. It "
    "keeps your state, worktree and branch, marks your tab green, moves it "
    "leftmost and ends your client; `fleet go` resumes you tomorrow."
)

FINISH_MESSAGE = (
    "Fleet wind-down: the fleet is closing out for the day, and you are In "
    "Review, so finish only while your path to merge stays linear: CI green "
    "or running, enqueue, queue run, merged, with no local work in between. "
    "The first event needing local work or a retry is grounds to stand down "
    "at once: a rebase (base moved, mergeable CONFLICTING or DIRTY), CI red "
    "on the PR's own checks, a dequeue of any kind or any re-enqueue, a lens "
    "finding that reopens the review. That rebase and second review are "
    "tomorrow's, in one pass. Once merged, run the post-merge tail up to its "
    "Done question, leave the issue In Review, and stand down. " + STAND_DOWN
)

PAUSE_MESSAGE = (
    "Fleet wind-down: the fleet is closing out for the day. Start nothing "
    "new; reach your first safe point (let a running build finish, so its "
    "lane lock frees), push a checkpoint, and write your next step as the "
    "resume brief. " + STAND_DOWN
)


def split(state: str | None) -> str:
    """``finish`` for In Review, ``pause`` for everything else."""
    return FINISH if state == "In Review" else PAUSE


def open_pr(prs: list[dict]) -> dict | None:
    """The newest open PR on the branch, or None."""
    return next((p for p in prs if p.get("state") == "OPEN"), None)


def doomed(row: dict) -> bool:
    """Whether a live finisher's path to merge is already non-linear."""
    if row["group"] != FINISH or not row["live"]:
        return False
    if row["pr"] is None:
        # Merged (its tail is all that is left) or PR-less by design (an
        # architect or chore handed off at In Review): nothing to doom.
        return False
    return (
        row["mergeable"] in DOOMED_MERGEABLE or row["merge_state"] in DOOMED_MERGE_STATE
    )


def rows(issues: list[dict], live: set[str], prs: dict[str, list[dict]] | None):
    """One row per recognized in-flight issue; ``prs`` None means unread."""
    out = []
    for issue in issues:
        tag = fleet_resume.tag_of(issue.get("identifier") or "")
        if tag is None:
            continue
        state = (issue.get("state") or {}).get("name")
        branch = (prs or {}).get(tag, [])
        pr = open_pr(branch)
        queue = (pr or {}).get("mergeQueueEntry") or {}
        row = {
            "tag": tag,
            "title": issue.get("title"),
            "state": state,
            "group": split(state),
            "live": tag in live,
            "pr": (pr or {}).get("number"),
            "merged": fleet_resume.merged_only(branch),
            "mergeable": (pr or {}).get("mergeable") if prs is not None else "?",
            "merge_state": (pr or {}).get("mergeStateStatus")
            if prs is not None
            else "?",
            "queue": queue.get("state"),
        }
        row["doomed"] = prs is not None and doomed(row)
        out.append(row)
    # Live first, finishers before pausers, then by number.
    out.sort(key=lambda r: (not r["live"], r["group"] != FINISH, int(r["tag"])))
    return out


def line(row: dict) -> str:
    """One row as a terse line."""
    if row["pr"] is not None:
        pr = f"PR #{row['pr']} {row['mergeable']}/{row['merge_state']}"
        if row["queue"]:
            pr += f" queue {row['queue']}"
    else:
        pr = "PR merged" if row["merged"] else "no PR"
    flag = " DOOMED: stands down at once" if row["doomed"] else ""
    where = "live" if row["live"] else "parked"
    return f"  {row['tag']} [{row['state']}] {where} | {pr}{flag}"


def summary(listed: list[dict], note: str | None) -> list[str]:
    """The ranked roll-call, then the message texts."""
    live = [r for r in listed if r["live"]]
    head = (
        f"fleet-down | {len(listed)} in flight | {len(live)} live | "
        f"{len(listed) - len(live)} parked | "
        f"{sum(r['doomed'] for r in listed)} doomed"
    )
    out = [head]
    if note:
        out.append(f"  note: {note}")
    for group in (FINISH, PAUSE):
        members = [r for r in listed if r["group"] == group]
        out.append(f"{group} ({len(members)}):")
        out += [line(r) for r in members] or ["  (none)"]
    out += ["", "finish message:", FINISH_MESSAGE, "", "pause message:", PAUSE_MESSAGE]
    return out


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="fleet_winddown.py",
        description="List the fleet for a wind-down: finish In Review, pause the rest.",
    )
    parser.add_argument("--json", action="store_true", help="every row, as JSON")
    args = parser.parse_args(argv[1:])

    api_key = fleet_resume._env("LINEAR_API_KEY")
    project_id = fleet_resume._env("LINEAR_PROJECT_ID")
    issues = fleet_resume.in_flight(api_key, project_id)
    live = fleet_resume.live_tags()
    tags = [t for i in issues if (t := fleet_resume.tag_of(i.get("identifier") or ""))]
    note = None
    try:
        prs = fleet_resume.branch_prs(tags)
    except fleet_resume.FleetResumeError as exc:
        prs, note = None, f"PR columns unread ({exc})"
    listed = rows(issues, live, prs)
    if args.json:
        payload = {
            "rows": listed,
            "note": note,
            "finish_message": FINISH_MESSAGE,
            "pause_message": PAUSE_MESSAGE,
        }
        json.dump(payload, sys.stdout, indent=2)
        sys.stdout.write("\n")
    else:
        print("\n".join(summary(listed, note)))
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except fleet_resume.FleetResumeError as exc:
        print(f"fleet-down: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
