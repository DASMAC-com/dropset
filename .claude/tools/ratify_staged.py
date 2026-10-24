#!/usr/bin/env python3
"""ratify_staged.py — stage-two ratification of a seat session's filed tasks.

The machinery behind the ``ratify-staged`` skill. A session on the advisor
tier (``architect``, ``explore``, an audit) files its tasks ``Backlog`` plus
the ``Session pending`` milestone on the operator's filing yes; this tool
drives the review that follows, so the language and the writes live in one
place rather than in each consumer skill.

Five subcommands, every one taking the session's own task ids (``ENG-###``,
space- or comma-separated) except ``list``:

* ``list`` — every open issue under ``Session pending``, one compact line
  each. The milestone is shared by every seat session, so this is the
  backstop view; a session reviews only its own ids.
* ``ask --ids …`` — the ``AskUserQuestion`` payload for the review ask: one
  checkbox per staged task, never grouped, at most four per question and four
  questions per call (so sixteen tasks fit one ask; more spill into further
  calls). Printed as JSON, ``{"calls": [{"questions": [...]}, ...]}``.
* ``review --ids …`` — for the tasks the operator ticked: each one is
  ``APPROVED`` (no open comment thread — the tick is the approval) or
  ``HELD`` (an open thread, printed with its id and body so the session can
  fold it by the fold protocol).
* ``approve --ids … [--cancel …] [--dry-run]`` — the one batch write at the
  end: every approved task loses the milestone and moves to Todo, every
  rejected one is canceled. Refuses the whole batch, writing nothing, while
  any approved task still holds an open thread or is not staged.
* ``gate --ids …`` — the close gate: exits 1, naming them, while any of the
  ids is still open under ``Session pending``.

**The write is the operator's, executed.** ``approve`` runs only on the
operator's final answer in the session's own tab (ruling 12's carve-out); it
never decides that a task is ratified. A tick with no comment is the
approval, an open thread is the per-task "not yet", and an unticked task is
simply not passed to ``review`` or ``approve``.

An "open thread" is a top-level comment that is not resolved. The fold
protocol resolves each thread once its fold lands, so a task the session has
fully folded reads ``APPROVED`` on the next round.

Configuration: ``LINEAR_API_KEY`` and ``LINEAR_PROJECT_ID``, as for
``board_batch.py``, whose resolver and field writer this reuses.

Usage:
    python3 .claude/tools/ratify_staged.py list
    python3 .claude/tools/ratify_staged.py ask --ids ENG-1 ENG-2 --out ask.json
    python3 .claude/tools/ratify_staged.py review --ids ENG-1 ENG-2
    python3 .claude/tools/ratify_staged.py approve --ids ENG-1 --cancel ENG-2
    python3 .claude/tools/ratify_staged.py gate --ids ENG-1 ENG-2
"""

from __future__ import annotations

import argparse
import json
import sys

import board_batch

MILESTONE = "Session pending"

# The verbatim review ask (operator direction, 2026-10-23).
ASK_TEXT = (
    "Have you looked at these? Check each one once you have. Leave a comment "
    "on it and I will revise it; leave nothing and I will take it as approved."
)
ASK_HEADER = "Reviewed"

# AskUserQuestion's bounds: 2-4 options per question, 1-4 questions per call.
MAX_OPTIONS = 4
MIN_OPTIONS = 2
MAX_QUESTIONS = 4

# A lone task still needs a second option to make a valid question.
NOT_YET_OPTION = {
    "label": "Not yet",
    "description": "Leave it unticked; it stays parked and is asked again.",
}

RESOLVED_TYPES = ("completed", "canceled")

_COMMENTS_QUERY = """
query($id: String!) {
  issue(id: $id) {
    comments(first: 100) {
      nodes { id body resolvedAt parentId quotedText user { name } }
    }
  }
}
"""


class RatifyError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def parse_ids(raw: list[str]) -> list[str]:
    """``ENG-###`` ids from space- and/or comma-separated arguments, in order,
    de-duplicated."""
    out: list[str] = []
    for chunk in raw:
        for part in chunk.split(","):
            ident = part.strip().upper()
            if ident and ident not in out:
                out.append(ident)
    if not out:
        raise RatifyError("no task ids given")
    return out


def is_staged(issue: dict) -> bool:
    """Open and still carrying the parking milestone."""
    milestone = (issue.get("projectMilestone") or {}).get("name")
    state_type = (issue.get("state") or {}).get("type")
    return milestone == MILESTONE and state_type not in RESOLVED_TYPES


def group_sizes(n: int) -> list[int]:
    """Balanced question sizes for ``n`` checkboxes: as few questions as the
    four-option cap allows, with no question left holding a single task
    unless the whole ask is one task."""
    if n <= 0:
        return []
    questions = -(-n // MAX_OPTIONS)
    base, extra = divmod(n, questions)
    return [base + 1] * extra + [base] * (questions - extra)


def build_ask(issues: list[dict]) -> dict:
    """The review ask as ``AskUserQuestion`` payloads, one per call."""
    sizes = group_sizes(len(issues))
    questions = []
    start = 0
    for index, size in enumerate(sizes):
        chunk = issues[start : start + size]
        start += size
        options = [
            {"label": issue["identifier"], "description": issue["title"]}
            for issue in chunk
        ]
        if len(options) < MIN_OPTIONS:
            options.append(dict(NOT_YET_OPTION))
        text = ASK_TEXT
        if len(sizes) > 1:
            text = f"{ASK_TEXT} ({index + 1} of {len(sizes)})"
        questions.append(
            {
                "question": text,
                "header": ASK_HEADER,
                "multiSelect": True,
                "options": options,
            }
        )
    calls = [
        {"questions": questions[i : i + MAX_QUESTIONS]}
        for i in range(0, len(questions), MAX_QUESTIONS)
    ]
    return {"calls": calls}


def open_threads(comments: list[dict]) -> list[dict]:
    """Top-level comments not yet resolved: the per-task "not yet"."""
    return [c for c in comments if not c.get("parentId") and not c.get("resolvedAt")]


def format_thread(comment: dict) -> list[str]:
    author = (comment.get("user") or {}).get("name") or "unknown"
    lines = [f"  thread {comment['id']} ({author})"]
    if comment.get("quotedText"):
        lines.append(f"    on: {comment['quotedText'].strip()}")
    lines.extend(f"    {line}" for line in (comment.get("body") or "").splitlines())
    return lines


def fetch_comments(api_key: str, ident: str) -> list[dict]:
    data = board_batch._post(api_key, _COMMENTS_QUERY, {"id": ident})
    issue = data.get("issue")
    if issue is None:
        raise RatifyError(f"{ident}: no such issue")
    return (issue.get("comments") or {}).get("nodes") or []


def resolve_all(api_key: str, project: str, ids: list[str]) -> list[dict]:
    """The named issues, resolved within the project — a hard error on any
    reference that does not resolve, never a guess."""
    numbers = board_batch.referenced_numbers(ids)
    by_number = board_batch.index_by_number(
        board_batch.fetch_issues_by_number(api_key, project, numbers)
    )
    return [board_batch.resolve_issue(ident, by_number) for ident in ids]


def review_lines(api_key: str, issues: list[dict]) -> tuple[list[str], list[str]]:
    """``(report lines, held ids)`` for the ticked tasks."""
    lines: list[str] = []
    held: list[str] = []
    for issue in issues:
        ident = issue["identifier"]
        if not is_staged(issue):
            lines.append(f"NOT STAGED {ident} | {issue['title']}")
            continue
        threads = open_threads(fetch_comments(api_key, ident))
        if threads:
            held.append(ident)
            lines.append(f"HELD {ident} | {len(threads)} open thread(s)")
            for thread in threads:
                lines.extend(format_thread(thread))
        else:
            lines.append(f"APPROVED {ident} | {issue['title']}")
    return lines, held


def plan_approval(
    approve: list[dict], cancel: list[dict], held: list[str]
) -> dict[str, dict]:
    """The batch ``board_batch.apply_fields`` takes, or a refusal naming every
    reason the batch cannot run yet."""
    overlap = {i["identifier"] for i in approve} & {i["identifier"] for i in cancel}
    problems = []
    if overlap:
        problems.append(f"both approved and canceled: {', '.join(sorted(overlap))}")
    if held:
        problems.append(f"still held by an open thread: {', '.join(held)}")
    unstaged = [i["identifier"] for i in approve + cancel if not is_staged(i)]
    if unstaged:
        problems.append(f"not open under {MILESTONE}: {', '.join(unstaged)}")
    if problems:
        raise RatifyError(
            "refusing the batch, nothing written — " + "; ".join(problems)
        )
    updates = {i["identifier"]: {"milestone": None, "state": "Todo"} for i in approve}
    updates.update(
        {i["identifier"]: {"milestone": None, "state": "Canceled"} for i in cancel}
    )
    return updates


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="cmd", required=True)
    sub.add_parser("list", help=f"every open issue under {MILESTONE}")
    ask = sub.add_parser("ask", help="the one-checkbox-per-task review ask")
    ask.add_argument("--ids", nargs="+", required=True)
    ask.add_argument("--out", help="write the JSON here instead of stdout")
    review = sub.add_parser("review", help="approved or held, per ticked task")
    review.add_argument("--ids", nargs="+", required=True)
    approve = sub.add_parser("approve", help="the one batch write at the end")
    approve.add_argument("--ids", nargs="+", default=[])
    approve.add_argument("--cancel", nargs="+", default=[])
    approve.add_argument("--dry-run", action="store_true")
    gate = sub.add_parser("gate", help="fail while any id is still staged")
    gate.add_argument("--ids", nargs="+", required=True)
    return parser.parse_args(argv)


def run(argv: list[str]) -> int:
    args = _parse_args(argv)
    key = board_batch.env_var("LINEAR_API_KEY")
    project = board_batch.env_var("LINEAR_PROJECT_ID")

    if args.cmd == "list":
        issues = board_batch.fetch_open_in_milestones(key, project, [MILESTONE])
        for line in board_batch.format_listing(issues) or [f"none under {MILESTONE}"]:
            print(line)
        return 0

    if args.cmd == "ask":
        issues = resolve_all(key, project, parse_ids(args.ids))
        staged = [i for i in issues if is_staged(i)]
        skipped = [i["identifier"] for i in issues if not is_staged(i)]
        if not staged:
            raise RatifyError(f"none of these is open under {MILESTONE}")
        payload = build_ask(staged)
        if skipped:
            payload["skipped"] = skipped
        text = json.dumps(payload, indent=2)
        if args.out:
            with open(args.out, "w", encoding="utf-8") as handle:
                handle.write(text + "\n")
            print(f"WROTE {args.out} | {len(staged)} task(s)", end="")
            print(f", skipped {', '.join(skipped)}" if skipped else "")
        else:
            print(text)
        return 0

    if args.cmd == "review":
        lines, _ = review_lines(key, resolve_all(key, project, parse_ids(args.ids)))
        print("\n".join(lines))
        return 0

    if args.cmd == "approve":
        approve_ids = parse_ids(args.ids) if args.ids else []
        cancel_ids = parse_ids(args.cancel) if args.cancel else []
        if not approve_ids and not cancel_ids:
            raise RatifyError("nothing to approve or cancel")
        approve = resolve_all(key, project, approve_ids) if approve_ids else []
        cancel = resolve_all(key, project, cancel_ids) if cancel_ids else []
        _, held = review_lines(key, [i for i in approve if is_staged(i)])
        updates = plan_approval(approve, cancel, held)
        by_number = {int(i["number"]): i for i in approve + cancel}
        board_batch.apply_fields(key, updates, by_number, dry_run=args.dry_run)
        return 0

    issues = resolve_all(key, project, parse_ids(args.ids))
    still = [i["identifier"] for i in issues if is_staged(i)]
    if still:
        print(f"OPEN — still under {MILESTONE}: {', '.join(still)}")
        return 1
    print(f"CLEAR — none of {len(issues)} task(s) is under {MILESTONE}")
    return 0


def main() -> int:
    try:
        return run(sys.argv[1:])
    except (RatifyError, board_batch.BoardBatchError) as exc:
        print(f"ratify_staged: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
