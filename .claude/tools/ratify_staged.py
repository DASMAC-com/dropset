#!/usr/bin/env python3
"""ratify_staged.py — stage-two ratification of a seat session's filed tasks.

The machinery behind the ``ratify-staged`` skill. A session on the advisor
tier (``architect``, ``explore``; audit findings once their own ratification
lands) files its tasks ``Backlog`` plus
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
  any approved task still holds an open thread or is not staged. A task an
  earlier, partly failed run already moved reports ``ALREADY`` and is
  skipped, so the same command re-runs cleanly.
* ``gate --ids …`` — the close gate: exits 1, naming them, while any of the
  ids is still open under ``Session pending``.

**The pool switch.** Every subcommand takes ``--pool staged|inbox``.
``staged`` (the default, so every existing caller is unchanged) is the
``Session pending`` pool above. ``inbox`` is the operator's review inbox —
every open issue in ``Backlog`` with no project milestone — which the curator
drives through the same machinery:

* ``list --pool inbox`` ranks the pool by priority, marking ``[Operator]``
  items (members like any other) with ``[operator]``.
* ``ask`` takes ``--notes`` (a JSON object, inline or a file path, mapping
  id to the one-line commentary that becomes that checkbox's description; the
  title otherwise) and ``--proposals`` (a JSON list of edges
  ``{"edge": {"blocker": …, "blocked": …}, "reason": …}`` and priority changes
  ``{"priority": {"id": …, "to": …}, "reason": …}``, rendered as a further
  multi-select question whose ticks the session executes through
  ``board_batch.py``). Both work on either pool.
* ``approve --pool inbox`` takes **at most one** ``--ids`` — the
  one-at-a-time rule, enforced here rather than in prose — plus any number of
  ``--cancel`` rejects. The approved id moves to Todo; the rest is as above.
* ``gate --pool inbox`` is refused: nothing is staged in that pool.

An out-of-pool id (Todo, parked under any milestone, closed) is skipped and
named, never silently dropped.

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
    python3 .claude/tools/ratify_staged.py list --pool inbox
    python3 .claude/tools/ratify_staged.py ask --pool inbox --ids ENG-1 ENG-2 \\
        --notes notes.json --proposals proposals.json --out ask.json
    python3 .claude/tools/ratify_staged.py approve --pool inbox --ids ENG-1 \\
        --cancel ENG-2
"""

from __future__ import annotations

import argparse
import json
import sys

import board_batch

MILESTONE = "Session pending"

POOLS = ("staged", "inbox")
# How each pool's membership reads in a refusal or a skip.
POOL_WHERE = {
    "staged": f"open under {MILESTONE}",
    "inbox": "open in Backlog with no milestone",
}
INBOX_STATE = "Backlog"
OPERATOR_TOKEN = "[operator]"

# The verbatim review ask (operator direction, 2026-10-23).
ASK_TEXT = (
    "Have you looked at these? Check each one once you have. Leave a comment "
    "on it and I will revise it; leave nothing and I will take it as approved."
)
ASK_HEADER = "Reviewed"

PROPOSALS_TEXT = (
    "Should I make these board changes? Tick each one to have it written; "
    "leave it unticked and I will not."
)
PROPOSALS_HEADER = "Proposals"
SKIP_PROPOSAL_OPTION = {
    "label": "None",
    "description": "Write none of these; the board stays as it is.",
}

# AskUserQuestion's bounds: 2-4 options per question, 1-4 questions per call.
MAX_OPTIONS = 4
MIN_OPTIONS = 2
MAX_QUESTIONS = 4

# A lone task still needs a second option to make a valid question.
NOT_YET_OPTION = {
    "label": "Not yet",
    "description": "Leave it unticked; it stays parked and is asked again.",
}

# `duplicate` too: a task marked Duplicate is closed, so it must neither block
# the close gate nor be approvable.
RESOLVED_TYPES = ("completed", "canceled", "duplicate")

# One page of comments, replies included. A task with more is refused rather
# than paged: an open thread on a later page would otherwise read as approved.
COMMENTS_PAGE = 250

_COMMENTS_QUERY = """
query($id: String!, $first: Int!) {
  issue(id: $id) {
    comments(first: $first) {
      pageInfo { hasNextPage }
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


def in_pool(issue: dict, pool: str) -> bool:
    """Open and a member of ``pool``: still carrying the parking milestone for
    ``staged``; in Backlog with no milestone at all for ``inbox``."""
    milestone = (issue.get("projectMilestone") or {}).get("name")
    state = issue.get("state") or {}
    if state.get("type") in RESOLVED_TYPES:
        return False
    if pool == "inbox":
        return not milestone and str(state.get("name", "")).lower() == "backlog"
    return milestone == MILESTONE


def is_operator_item(issue: dict) -> bool:
    """An ``[Operator]`` (hand-executed) item, by its title token."""
    return OPERATOR_TOKEN in str(issue.get("title", "")).lower()


def priority_rank(issue: dict) -> tuple[int, int]:
    """Urgent first, no priority last, then by number."""
    priority = issue.get("priority") or 0
    # Linear's scale is 1 (Urgent) to 4 (Low), 0 meaning none.
    return (priority or 5, int(issue.get("number") or 0))


def format_pool_listing(issues: list[dict]) -> list[str]:
    """The inbox listing: ``board_batch.py``'s line format, ranked by priority,
    operator items marked."""
    lines = []
    for issue in sorted(issues, key=priority_rank):
        (line,) = board_batch.format_listing([issue])
        lines.append(f"{line}  {OPERATOR_TOKEN}" if is_operator_item(issue) else line)
    return lines


def load_json_arg(raw: str | None) -> object:
    """A JSON argument given inline or as a file path; ``None`` when absent."""
    if raw is None:
        return None
    text = raw.strip()
    if text[:1] in ("{", "["):
        try:
            return json.loads(text)
        except json.JSONDecodeError as exc:
            raise RatifyError(f"inline JSON does not parse: {exc}") from exc
    return board_batch.load_json_file(raw)


def parse_notes(raw: object, ids: list[str]) -> dict[str, str]:
    """``{id: note}``, ids normalized. A note for an id not in the ask is a
    typo, refused rather than dropped."""
    if raw is None:
        return {}
    if not isinstance(raw, dict):
        raise RatifyError("--notes expects an object of id -> one-line note")
    notes = {str(k).strip().upper(): str(v).strip() for k, v in raw.items()}
    stray = sorted(set(notes) - set(ids))
    if stray:
        raise RatifyError(f"--notes names ids not in the ask: {', '.join(stray)}")
    return notes


def proposal_option(proposal: object) -> dict:
    """One proposal as a checkbox: an edge or a priority change, its reason as
    the description."""
    if not isinstance(proposal, dict) or not str(proposal.get("reason", "")).strip():
        raise RatifyError(f"proposal {proposal!r} needs a one-line reason")
    reason = str(proposal["reason"]).strip()
    edge, change = proposal.get("edge"), proposal.get("priority")
    if isinstance(edge, dict) and edge.get("blocker") and edge.get("blocked"):
        blocker = str(edge["blocker"]).strip().upper()
        blocked = str(edge["blocked"]).strip().upper()
        return {"label": f"{blocker} blocks {blocked}", "description": reason}
    if isinstance(change, dict) and change.get("id") and "to" in change:
        level = board_batch.normalize_priority(change["to"])
        ident = str(change["id"]).strip().upper()
        label = board_batch.PRIORITY_LABELS.get(level, str(level))
        return {"label": f"{ident} to {label}", "description": reason}
    raise RatifyError(
        f"proposal {proposal!r} is neither an edge {{blocker, blocked}} nor a "
        "priority change {id, to}"
    )


def parse_proposals(raw: object) -> list[dict]:
    if raw is None:
        return []
    if not isinstance(raw, list):
        raise RatifyError("--proposals expects a list of edges and priority changes")
    options = [proposal_option(p) for p in raw]
    labels = [o["label"] for o in options]
    repeated = sorted({label for label in labels if labels.count(label) > 1})
    if repeated:
        # Two checkboxes with one label make the operator's ticks ambiguous.
        raise RatifyError(f"--proposals repeats: {', '.join(repeated)}")
    return options


def group_sizes(n: int) -> list[int]:
    """Balanced question sizes for ``n`` checkboxes: as few questions as the
    four-option cap allows, with no question left holding a single task
    unless the whole ask is one task."""
    if n <= 0:
        return []
    questions = -(-n // MAX_OPTIONS)
    base, extra = divmod(n, questions)
    return [base + 1] * extra + [base] * (questions - extra)


def _questions(options: list[dict], text: str, header: str, filler: dict) -> list[dict]:
    """``options`` as balanced multi-select questions, numbered when more than
    one, a lone option padded with ``filler``."""
    sizes = group_sizes(len(options))
    questions = []
    start = 0
    for index, size in enumerate(sizes):
        chunk = options[start : start + size]
        start += size
        if len(chunk) < MIN_OPTIONS:
            chunk.append(dict(filler))
        q_text, q_header = text, header
        if len(sizes) > 1:
            q_text = f"{text} ({index + 1} of {len(sizes)})"
            q_header = f"{header} {index + 1}"
        questions.append(
            {
                "question": q_text,
                "header": q_header,
                "multiSelect": True,
                "options": chunk,
            }
        )
    return questions


def build_ask(
    issues: list[dict],
    notes: dict[str, str] | None = None,
    proposals: list[dict] | None = None,
) -> dict:
    """The review ask as ``AskUserQuestion`` payloads, one per call: a checkbox
    per task (its note as the description, else its title), then any
    proposals as further questions."""
    notes = notes or {}
    options = [
        {
            "label": issue["identifier"],
            "description": notes.get(issue["identifier"]) or issue["title"],
        }
        for issue in issues
    ]
    questions = _questions(options, ASK_TEXT, ASK_HEADER, NOT_YET_OPTION)
    questions += _questions(
        list(proposals or []), PROPOSALS_TEXT, PROPOSALS_HEADER, SKIP_PROPOSAL_OPTION
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
    data = board_batch._post(
        api_key, _COMMENTS_QUERY, {"id": ident, "first": COMMENTS_PAGE}
    )
    issue = data.get("issue")
    if issue is None:
        raise RatifyError(f"{ident}: no such issue")
    comments = issue.get("comments") or {}
    if (comments.get("pageInfo") or {}).get("hasNextPage"):
        raise RatifyError(
            f"{ident}: more than {COMMENTS_PAGE} comments — refusing to judge its "
            "threads from a partial read; review it in Linear by hand"
        )
    return comments.get("nodes") or []


def resolve_all(api_key: str, project: str, ids: list[str]) -> list[dict]:
    """The named issues, resolved within the project — a hard error on any
    reference that does not resolve, never a guess."""
    numbers = board_batch.referenced_numbers(ids)
    by_number = board_batch.index_by_number(
        board_batch.fetch_issues_by_number(api_key, project, numbers)
    )
    return [board_batch.resolve_issue(ident, by_number) for ident in ids]


def review_lines(
    api_key: str, issues: list[dict], pool: str = "staged"
) -> tuple[list[str], list[str]]:
    """``(report lines, held ids)`` for the ticked tasks."""
    lines: list[str] = []
    held: list[str] = []
    outside = "NOT STAGED" if pool == "staged" else "NOT IN INBOX"
    for issue in issues:
        ident = issue["identifier"]
        if not in_pool(issue, pool):
            lines.append(f"{outside} {ident} | {issue['title']}")
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


def already_moved(issue: dict, cancel: bool) -> bool:
    """A task an earlier, partly failed run of the batch already wrote: an
    approval sits in Todo with no milestone, a reject is canceled."""
    state = issue.get("state") or {}
    if cancel:
        return state.get("type") in ("canceled", "duplicate")
    milestone = issue.get("projectMilestone")
    return not milestone and str(state.get("name", "")).lower() == "todo"


def plan_approval(
    approve: list[dict], cancel: list[dict], held: list[str], pool: str = "staged"
) -> tuple[dict[str, dict], list[str]]:
    """``(updates, already)``: the batch ``board_batch.apply_fields`` takes and
    the ids a previous run already moved — or a refusal naming every reason the
    batch cannot run yet. Skipping the already-moved ids is what lets a write
    that failed partway be re-run with the same arguments.

    The inbox pool approves one task per write, and its tasks carry no
    milestone to clear."""
    if pool == "inbox" and len(approve) > 1:
        raise RatifyError(
            "refusing the batch, nothing written — the inbox approves one task "
            f"at a time, got {len(approve)}: "
            + ", ".join(i["identifier"] for i in approve)
        )
    overlap = {i["identifier"] for i in approve} & {i["identifier"] for i in cancel}
    already = [i["identifier"] for i in approve if already_moved(i, cancel=False)]
    already += [i["identifier"] for i in cancel if already_moved(i, cancel=True)]
    approve = [i for i in approve if i["identifier"] not in already]
    cancel = [i for i in cancel if i["identifier"] not in already]
    problems = []
    if overlap:
        problems.append(f"both approved and canceled: {', '.join(sorted(overlap))}")
    if held:
        problems.append(f"still held by an open thread: {', '.join(held)}")
    outside = [i["identifier"] for i in approve + cancel if not in_pool(i, pool)]
    if outside:
        problems.append(f"not {POOL_WHERE[pool]}: {', '.join(outside)}")
    if problems:
        raise RatifyError(
            "refusing the batch, nothing written — " + "; ".join(problems)
        )
    clear = {"milestone": None} if pool == "staged" else {}
    updates = {i["identifier"]: {**clear, "state": "Todo"} for i in approve}
    updates.update({i["identifier"]: {**clear, "state": "Canceled"} for i in cancel})
    return updates, already


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="cmd", required=True)
    listing = sub.add_parser("list", help="every open issue in the pool")
    ask = sub.add_parser("ask", help="the one-checkbox-per-task review ask")
    ask.add_argument("--ids", nargs="+", required=True)
    ask.add_argument("--notes", help="JSON id -> note, inline or a file path")
    ask.add_argument("--proposals", help="JSON list of edges / priority changes")
    ask.add_argument("--out", help="write the JSON here instead of stdout")
    review = sub.add_parser("review", help="approved or held, per ticked task")
    review.add_argument("--ids", nargs="+", required=True)
    approve = sub.add_parser("approve", help="the one batch write at the end")
    approve.add_argument("--ids", nargs="+", default=[])
    approve.add_argument("--cancel", nargs="+", default=[])
    approve.add_argument("--dry-run", action="store_true")
    gate = sub.add_parser("gate", help="fail while any id is still staged")
    gate.add_argument("--ids", nargs="+", required=True)
    for command in (listing, ask, review, approve, gate):
        command.add_argument("--pool", choices=POOLS, default="staged")
    return parser.parse_args(argv)


def run(argv: list[str]) -> int:
    args = _parse_args(argv)
    pool = args.pool
    if args.cmd == "gate" and pool == "inbox":
        raise RatifyError(
            "gate is for the staged pool — nothing is staged in the inbox, so "
            "there is nothing for a close gate to hold"
        )
    key = board_batch.env_var("LINEAR_API_KEY")
    project = board_batch.env_var("LINEAR_PROJECT_ID")

    if args.cmd == "list":
        if pool == "inbox":
            issues = board_batch.fetch_issues(key, project, states=[INBOX_STATE])
            members = [i for i in issues if in_pool(i, pool)]
            lines = format_pool_listing(members) or [f"none {POOL_WHERE[pool]}"]
        else:
            issues = board_batch.fetch_open_in_milestones(key, project, [MILESTONE])
            lines = board_batch.format_listing(issues) or [f"none under {MILESTONE}"]
        for line in lines:
            print(line)
        return 0

    if args.cmd == "ask":
        ids = parse_ids(args.ids)
        issues = resolve_all(key, project, ids)
        staged = [i for i in issues if in_pool(i, pool)]
        skipped = [i["identifier"] for i in issues if not in_pool(i, pool)]
        if not staged:
            raise RatifyError(f"none of these is {POOL_WHERE[pool]}")
        # Checked against the requested ids, so a note for an id that has
        # since left the pool is skipped with it rather than refusing the ask.
        notes = parse_notes(load_json_arg(args.notes), ids)
        proposals = parse_proposals(load_json_arg(args.proposals))
        payload = build_ask(staged, notes, proposals)
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
        issues = resolve_all(key, project, parse_ids(args.ids))
        lines, _ = review_lines(key, issues, pool)
        print("\n".join(lines))
        return 0

    if args.cmd == "approve":
        approve_ids = parse_ids(args.ids) if args.ids else []
        cancel_ids = parse_ids(args.cancel) if args.cancel else []
        if not approve_ids and not cancel_ids:
            raise RatifyError("nothing to approve or cancel")
        approve = resolve_all(key, project, approve_ids) if approve_ids else []
        cancel = resolve_all(key, project, cancel_ids) if cancel_ids else []
        _, held = review_lines(key, [i for i in approve if in_pool(i, pool)], pool)
        updates, already = plan_approval(approve, cancel, held, pool)
        for ident in already:
            print(f"ALREADY {ident} | already in its target state, skipped")
        by_number = {int(i["number"]): i for i in approve + cancel}
        board_batch.apply_fields(key, updates, by_number, dry_run=args.dry_run)
        return 0

    issues = resolve_all(key, project, parse_ids(args.ids))
    still = [i["identifier"] for i in issues if in_pool(i, pool)]
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
