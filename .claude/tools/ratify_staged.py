#!/usr/bin/env python3
"""ratify_staged.py — stage-two ratification of a seat session's filed tasks.

The machinery behind the ``ratify-staged`` skill. A session on the advisor
tier (``architect``, ``explore``; audit findings once their own ratification
lands) files its tasks ``Backlog`` plus
the ``Session pending`` milestone on the operator's filing yes; this tool
drives the review that follows, so the language and the writes live in one
place rather than in each consumer skill.

Five subcommands, every one but ``list`` naming the session's own tasks —
as ids (``--ids``, ``ENG-###``, space- or comma-separated) or as the
sub-issues of its governing issue (``--parent``, see "The parent switch"):

* ``list`` — every open issue under ``Session pending``, one compact line
  each. The milestone is shared by every seat session, so this is the
  backstop view; a session reviews only its own ids.
* ``ask --ids …`` — the ``AskUserQuestion`` payload for the review ask: one
  checkbox per staged task, never grouped, at most four per question and four
  questions per call (so sixteen tasks fit one ask; more spill into further
  calls). Each description leads with the task's current priority — for a
  freshly staged task, the filer's proposal — so the operator reviews it in
  the same tick. Printed as JSON,
  ``{"calls": [{"questions": [...]}, ...]}``.
* ``review --ids …`` — for the tasks the operator ticked: each one is
  ``APPROVED`` (no open comment thread — the tick is the approval) or
  ``HELD`` (an open thread, printed with its id and body so the session can
  fold it by the fold protocol).
* ``approve --ids … [--cancel …] [--dry-run]`` — the one batch write at the
  end: every approved task loses the milestone and moves to Todo, every
  rejected one is canceled. Refuses the whole batch, writing nothing, while
  any approved task still holds an open thread or is not staged. Priority is
  never written here; it stands as already set on the issue. A task an
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
  ``{"edge": {"blocker": …, "blocked": …}, "reason": …}``, and the launcher's
  hand-off kills ``{"kill": …, "reason": …}``, merges
  ``{"merge": [… two or more], "reason": …}`` and priority changes
  ``{"priority": {"id": …, "to": …}, "reason": …}``, rendered as a further
  multi-select question whose ticks the session executes — an edge or a
  priority through ``board_batch.py``, a kill as a cancel, a merge through
  ``merge-tasks``). Both work on either pool.
* ``approve --pool inbox`` takes **at most one** ``--ids`` — the
  one-at-a-time rule, enforced here rather than in prose — plus any number of
  ``--cancel`` rejects. The approved id moves to Todo; the rest is as above.
  ``--title`` retitles that one task in the same write, through
  ``board_batch.py``'s title guard: a title that changes the leading
  ``[Verb/Substrate]`` / ``[Operator]`` / ``Claude:`` token run is refused,
  and this tool has no ``--retoken``.
* ``gate --pool inbox`` is refused: nothing is staged in that pool.

**The operator walk.** ``--pool operator`` is the Todo ``[Operator]`` items
with no milestone — the hand-executed work a tick moved to Todo, which the
curator carries one per batch until Done. A chore-token item is never a
member: it is Todo work the scheduler dispatches.

* ``list --pool operator`` ranks it Urgent first, then oldest (lowest
  number) first, so its first line is the item the next ask carries.
* ``ask --operator <id>`` adds that one item as its own question after the
  review checkboxes — the "fifth box", since a question holds four. ``--ids``
  may then be omitted: the operator-items-only ask. The tool reads the item's
  comments and counts the curator's dated deferral replies (a comment whose
  body starts ``Deferred YYYY-MM-DD:``), so the description says how often
  and how long it has waited, and from the third deferral on it recommends
  cancel under the necessity gate.
* ``approve --pool operator`` takes at most one ``--ids`` and moves it to
  Done; ``--cancel`` cancels as above.
* ``approve --chore <id>`` retitles the operator item's ``[Operator]`` token to
  ``[Chore/Bedrock]`` and changes nothing else, leaving it in Todo for the
  scheduler to dispatch under the fleet gate. The operator's ``attended
  chore`` comment is what licenses that one token change; there is still no
  general ``--retoken``.
* ``review --pool operator`` reads the item's open threads like any pool;
  ``gate`` is refused for it.

**The curator's mark.** An inbox member carrying the ``Curator pending``
milestone is still an inbox member: ``list --pool inbox`` lists it (marked
``[curator pending]``) and ``approve --pool inbox`` accepts it, clearing the
milestone in the same write. Two inbox-only subcommands write it, the
milestone resolved by name:

* ``mark --ids … [--dry-run]`` — stamp the batch before its ask; an
  out-of-pool id is skipped and named.
* ``unmark (--ids … | --all) [--dry-run]`` — clear it; ``--all`` is the close
  sweep and the next bootstrap's leftover sweep. Clears only that milestone.

An out-of-pool id (Todo, parked under any other milestone, closed) is skipped and
named, never silently dropped.

**The parent switch.** A seat session files its tasks as sub-issues of its
governing issue (``linear_issue.py create --parent``), so ``--parent ENG-###``
names the set wherever ``--ids`` does — ``ask``, ``review``, ``approve`` and
``gate`` — and narrows ``list`` to it: a session that died mid-review resumes
from its issue number alone. ``review`` and ``approve`` take the sub-issues
still staged (``approve --parent`` approves every one not given to
``--cancel``, reports one an earlier run moved as ``ALREADY``, names any
other as ``SKIPPED``, and refuses a ``--cancel`` id outside the set); ``gate``
judges them all. ``ask --parent`` records the children it carried in the base
checkout's substrate directory, and ``approve --parent`` refuses the batch,
naming them, while any child it would move is missing from that last record
— one filed after the ask, or left by an earlier session — unless ``--allow``
names it. ``--cancel`` and ``--allow`` take ``ENG-###`` or a bare number. A parent with no sub-issues is refused, never a vacuous
``CLEAR``, and ``--parent`` is refused on every pool but ``staged``: the
inbox and operator pools approve by id.

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
    python3 .claude/tools/ratify_staged.py review --parent ENG-9
    python3 .claude/tools/ratify_staged.py approve --parent ENG-9 --cancel ENG-2
    python3 .claude/tools/ratify_staged.py approve --parent ENG-9 --allow ENG-4
    python3 .claude/tools/ratify_staged.py gate --parent ENG-9
    python3 .claude/tools/ratify_staged.py list --pool inbox
    python3 .claude/tools/ratify_staged.py ask --pool inbox --ids ENG-1 ENG-2 \\
        --notes notes.json --proposals proposals.json --out ask.json
    python3 .claude/tools/ratify_staged.py approve --pool inbox --ids ENG-1 \\
        --cancel ENG-2
    python3 .claude/tools/ratify_staged.py mark --ids ENG-1 ENG-2
    python3 .claude/tools/ratify_staged.py unmark --all
    python3 .claude/tools/ratify_staged.py list --pool operator
    python3 .claude/tools/ratify_staged.py ask --pool inbox --ids ENG-1 \\
        --operator ENG-9 --notes notes.json --out ask.json
    python3 .claude/tools/ratify_staged.py approve --pool operator --ids ENG-9
    python3 .claude/tools/ratify_staged.py approve --chore ENG-9
"""

from __future__ import annotations

import argparse
import datetime
import json
import re
import sys
from pathlib import Path

import board_batch
import machine_load

MILESTONE = "Session pending"
# The curator's under-review mark: stamped on a batch before its ask, cleared by
# the ticked move, the close sweep or the next bootstrap. Only the curator
# writes it, and an inbox member carrying it is still an inbox member.
CURATOR_MILESTONE = "Curator pending"

POOLS = ("staged", "inbox", "operator")
# How each pool's membership reads in a refusal or a skip.
POOL_WHERE = {
    "staged": f"open under {MILESTONE}",
    "inbox": f"open in Backlog with no milestone or {CURATOR_MILESTONE}",
    "operator": "an [Operator] item open in Todo with no milestone",
}
INBOX_STATE = "Backlog"
OPERATOR_STATE = "Todo"
OPERATOR_TOKEN = "[operator]"
# The state an approval moves a task to, per pool: a staged or inbox task is
# approved into Todo, an operator item's tick is its Done.
APPROVED_STATE = {"staged": "Todo", "inbox": "Todo", "operator": "Done"}
# The one retoken this tool writes: an operator item the operator's `attended
# chore` comment hands to the scheduler as a chore.
CHORE_TOKEN = "[Chore/Bedrock]"

# The fifth box: one operator item, its own question after the review ticks.
OPERATOR_TEXT = (
    "Is this one done? Tick it once it is. Leave a comment to put it off, "
    "`drop` to cancel it, or `attended chore` to hand it to the scheduler."
)
OPERATOR_HEADER = "Operator"
NOT_TODAY_OPTION = {
    "label": "Not today",
    "description": "Leave it unticked; it stays in Todo and comes back next batch.",
}
# The curator's dated deferral reply, which the next ask counts.
DEFERRAL = re.compile(r"\s*Deferred (\d{4}-\d{2}-\d{2}):")
# The deferral count from which the ask recommends cancel (ruling 3).
CANCEL_AT_DEFERRALS = 3

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
    ``staged``; in Backlog with no milestone, or the curator's own
    ``Curator pending`` mark, for ``inbox``; an ``[Operator]`` item in Todo
    with no milestone, never a chore-token one, for ``operator``."""
    milestone = (issue.get("projectMilestone") or {}).get("name")
    state = issue.get("state") or {}
    if state.get("type") in RESOLVED_TYPES:
        return False
    if pool == "inbox":
        return milestone in (None, "", CURATOR_MILESTONE) and (
            str(state.get("name", "")).lower() == "backlog"
        )
    if pool == "operator":
        tokens = [t.lower() for t in board_batch.title_token(issue.get("title", ""))]
        return (
            not milestone
            and str(state.get("name", "")).lower() == OPERATOR_STATE.lower()
            and OPERATOR_TOKEN in tokens
            and not any(t.startswith("[chore/") for t in tokens)
        )
    return milestone == MILESTONE


def curator_marked(issue: dict) -> bool:
    return (issue.get("projectMilestone") or {}).get("name") == CURATOR_MILESTONE


def is_operator_item(issue: dict) -> bool:
    """An ``[Operator]`` (hand-executed) item, by its title token."""
    return OPERATOR_TOKEN in str(issue.get("title", "")).lower()


def chore_title(issue: dict) -> str:
    """``issue``'s title with its leading ``[Operator]`` token swapped for
    ``[Chore/Bedrock]``, the rest kept byte for byte; refused when the leading
    token run carries no ``[Operator]``, or a ``[Verb/Substrate]`` token beside
    it that the swap would leave as a second launch line."""
    title = str(issue.get("title") or "")
    end = board_batch._TITLE_TOKEN.match(title).end(1)
    run = title[:end]
    if any("/" in t for t in board_batch.title_token(title)):
        raise RatifyError(
            f"{issue.get('identifier')}: the title already carries a "
            "[Verb/Substrate] token — fix the title by hand, not by a swap"
        )
    swapped = re.sub(r"\[operator\]", CHORE_TOKEN, run, count=1, flags=re.I)
    if swapped == run:
        raise RatifyError(
            f"{issue.get('identifier')}: no leading [Operator] token to hand over"
        )
    return swapped + title[end:]


def deferrals(comments: list[dict]) -> list[datetime.date]:
    """The dates of the curator's deferral replies, oldest first."""
    dates = []
    for comment in comments:
        match = DEFERRAL.match(comment.get("body") or "")
        if match:
            try:
                dates.append(datetime.date.fromisoformat(match.group(1)))
            except ValueError:
                continue
    return sorted(dates)


def operator_note(base: str, dates: list[datetime.date], today: datetime.date) -> str:
    """The fifth box's description: the curator's line (or the title), then
    the deferral history and, from the third deferral on, the cancel
    recommendation."""
    if not dates:
        return base
    waited = (today - dates[0]).days
    note = (
        f"{base} — deferred {len(dates)} time{'s' if len(dates) > 1 else ''} "
        f"since {dates[0].isoformat()} (waited {waited} day"
        f"{'' if waited == 1 else 's'})"
    )
    if len(dates) >= CANCEL_AT_DEFERRALS:
        note += (
            "; I recommend cancel under the necessity gate — comment `drop` "
            "and I will cancel it"
        )
    return note


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
        if is_operator_item(issue):
            line = f"{line}  {OPERATOR_TOKEN}"
        if curator_marked(issue):
            line = f"{line}  [{CURATOR_MILESTONE.lower()}]"
        lines.append(line)
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


PROPOSAL_KINDS = ("edge", "kill", "merge", "priority")
ISSUE_ID = re.compile(r"ENG-(\d+)")


def _issue_id(raw: object) -> tuple[str, int]:
    """A hand-off id as ``("ENG-<n>", n)``, leading zeros dropped so
    ``ENG-09`` and ``ENG-9`` are one issue; anything else is refused."""
    match = ISSUE_ID.fullmatch(str(raw).strip().upper())
    if not match:
        raise RatifyError(f"hand-off id {raw!r} is not ENG-###")
    number = int(match.group(1))
    return f"ENG-{number}", number


def _hand_off_ids(proposal: dict) -> list[str]:
    """The issues a kill or merge consumes; an edge consumes none."""
    if "kill" in proposal:
        return [_issue_id(proposal["kill"])[0]]
    if "merge" in proposal:
        return sorted({_issue_id(i)[0] for i in proposal["merge"]})
    return []


def _priority_target(proposal: dict) -> str | None:
    """The issue a priority proposal re-ranks; ``None`` for any other kind."""
    if "priority" not in proposal:
        return None
    change = proposal["priority"]
    if not isinstance(change, dict) or "id" not in change:
        raise RatifyError(f"priority {change!r} needs an object with an id")
    return _issue_id(change["id"])[0]


def priority_name(value: object) -> str:
    """A priority, by name or Linear's integer, as the ask shows it."""
    try:
        number = board_batch.normalize_priority(value)
    except board_batch.BoardBatchError as exc:
        raise RatifyError(str(exc)) from exc
    return "No priority" if number == 0 else board_batch.PRIORITY_LABELS[number].title()


def proposal_option(proposal: object) -> dict:
    """One proposal as a checkbox: an edge, a priority change, or a kill or
    merge from the launcher's hand-off, its reason (the evidence) as the
    description."""
    if not isinstance(proposal, dict) or not str(proposal.get("reason", "")).strip():
        raise RatifyError(f"proposal {proposal!r} needs a one-line reason")
    kinds = [k for k in PROPOSAL_KINDS if k in proposal]
    if len(kinds) > 1:
        raise RatifyError(f"proposal {proposal!r} mixes {' and '.join(kinds)}")
    reason = str(proposal["reason"]).strip()
    edge, kill, merge, _ = (proposal.get(k) for k in PROPOSAL_KINDS)
    target = _priority_target(proposal)
    if target is not None:
        if "to" not in proposal["priority"]:
            raise RatifyError(f"priority for {target} needs a target 'to'")
        to = priority_name(proposal["priority"]["to"])
        return {"label": f"{target} to {to}", "description": reason}
    if isinstance(edge, dict) and edge.get("blocker") and edge.get("blocked"):
        blocker = str(edge["blocker"]).strip().upper()
        blocked = str(edge["blocked"]).strip().upper()
        return {"label": f"{blocker} blocks {blocked}", "description": reason}
    if kill is not None:
        return {"label": f"Cancel {_issue_id(kill)[0]}", "description": reason}
    if isinstance(merge, list) and len(merge) >= 2:
        # merge-tasks folds into the lowest-numbered survivor; name it.
        numbers = sorted({_issue_id(i)[1] for i in merge})
        if len(numbers) < 2:
            raise RatifyError(f"merge {merge!r} needs two distinct ids")
        ids = [f"ENG-{n}" for n in numbers]
        return {
            "label": f"Merge {' '.join(ids[1:])} into {ids[0]}",
            "description": reason,
        }
    raise RatifyError(
        f"proposal {proposal!r} is neither an edge {{blocker, blocked}}, a "
        "priority {id, to}, a kill (an id) nor a merge (two or more ids)"
    )


def parse_proposals(raw: object) -> list[dict]:
    if raw is None:
        return []
    if not isinstance(raw, list):
        raise RatifyError(
            "--proposals expects a list of edges, priorities, kills and merges"
        )
    options = [proposal_option(p) for p in raw]
    labels = [o["label"] for o in options]
    repeated = sorted({label for label in labels if labels.count(label) > 1})
    if repeated:
        # Two checkboxes with one label make the operator's ticks ambiguous.
        raise RatifyError(f"--proposals repeats: {', '.join(repeated)}")
    consumed = [i for p in raw for i in _hand_off_ids(p)]
    contested = sorted({i for i in consumed if consumed.count(i) > 1})
    if contested:
        # Ticking both a kill and a merge of one issue cannot both execute.
        raise RatifyError(
            f"--proposals names an issue in two kills or merges: {', '.join(contested)}"
        )
    ranked = [t for t in (_priority_target(p) for p in raw) if t]
    twice = sorted({t for t in ranked if ranked.count(t) > 1})
    if twice:
        # Two ticked targets for one issue: whichever ran last would win.
        raise RatifyError(f"--proposals re-ranks an issue twice: {', '.join(twice)}")
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
    operator: dict | None = None,
) -> dict:
    """The review ask as ``AskUserQuestion`` payloads, one per call: a checkbox
    per task (its priority, then its note or else its title, as the
    description), then the one ``operator`` option (the fifth box, already
    described) as its own question, then any proposals as further
    questions."""
    notes = notes or {}
    options = [
        {
            "label": issue["identifier"],
            "description": (
                # Linear types priority as a Float; the scale is integral.
                f"{priority_name(int(issue.get('priority') or 0))} · "
                f"{notes.get(issue['identifier']) or issue['title']}"
            ),
        }
        for issue in issues
    ]
    questions = _questions(options, ASK_TEXT, ASK_HEADER, NOT_YET_OPTION)
    if operator is not None:
        questions += _questions(
            [operator], OPERATOR_TEXT, OPERATOR_HEADER, NOT_TODAY_OPTION
        )
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


_PARENT_QUERY = """
query($id: String!) {
  issue(id: $id) { id identifier }
}
"""

PARENT_ID = re.compile(r"[A-Z]+-\d+")
NOT_FOUND = "Entity not found"


def parent_ident(parent: str) -> str:
    ident = parent.strip().upper()
    if not PARENT_ID.fullmatch(ident):
        raise RatifyError(f"--parent wants an ENG-### identifier, got {parent!r}")
    return ident


def ask_record_dir() -> Path:
    """Where ``ask --parent`` records the set it carried: the base checkout's
    substrate directory, so the record outlives a temporary worktree."""
    return machine_load.default_state_dir()


def ask_record_path(parent: str) -> Path:
    return ask_record_dir() / f"ratify-ask-{parent_ident(parent)}.json"


def write_ask_record(parent: str, ids: list[str]) -> None:
    """Record the children the last ``ask --parent`` carried, replacing any
    earlier round's record."""
    path = ask_record_path(parent)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"parent": parent_ident(parent), "ids": ids}) + "\n")


def read_ask_record(parent: str) -> list[str] | None:
    """The ids the last ``ask --parent`` carried, or ``None`` with no record."""
    path = ask_record_path(parent)
    if not path.is_file():
        return None
    try:
        return list(json.loads(path.read_text())["ids"])
    except (ValueError, KeyError, TypeError) as exc:
        raise RatifyError(f"unreadable ask record {path}: {exc}") from exc


def resolve_among(
    raw: list[str], children: list[dict], flag: str, parent: str
) -> list[dict]:
    """The children ``raw`` names, by ``ENG-###`` or bare number as on the ids
    path; one that is not a sub-issue of ``parent`` refuses the batch. Two
    spellings of one child (``3`` and ``ENG-3``) name it once."""
    by_number = board_batch.index_by_number(children)
    found, stray = [], []
    for ident in raw:
        try:
            issue = board_batch.resolve_issue(ident, by_number, label=flag)
        except board_batch.BoardBatchError:
            stray.append(ident)
            continue
        if issue not in found:
            found.append(issue)
    if stray:
        raise RatifyError(
            f"refusing the batch, nothing written — {flag} names ids that are "
            f"not sub-issues of {parent_ident(parent)}: {', '.join(stray)}"
        )
    return found


def not_asked(
    moving: list[dict], asked: list[str] | None, allowed: set[str]
) -> list[str]:
    """The ids about to move that the last ask did not carry and ``--allow``
    did not name: a child filed after the ask, or left by an earlier session,
    that the operator never ticked."""
    carried = set(asked or [])
    return [
        i["identifier"]
        for i in moving
        if i["identifier"] not in carried and i["identifier"] not in allowed
    ]


def fetch_children(api_key: str, project: str, parent: str) -> list[dict]:
    """The sub-issues of ``parent`` within the project, by number — the set a
    seat session filed with ``linear_issue.py create --parent``.

    This is what makes a session that died mid-review resumable from its
    governing issue's number alone. A parent that does not resolve, or has no
    children, is refused: an empty set would read as a vacuous ``CLEAR``.
    """
    ident = parent_ident(parent)
    try:
        issue = board_batch._post(api_key, _PARENT_QUERY, {"id": ident}).get("issue")
    except board_batch.BoardBatchError as exc:
        # Linear answers an unknown id with a GraphQL error, not a null node;
        # surface it as this tool's own refusal, not the transport's.
        if NOT_FOUND not in str(exc):
            raise
        issue = None
    if not (issue or {}).get("id"):
        raise RatifyError(f"parent {ident}: no such issue")
    children = board_batch._fetch_filtered(
        api_key,
        {"project": {"id": {"eq": project}}, "parent": {"id": {"eq": issue["id"]}}},
    )
    if not children:
        raise RatifyError(f"{ident} has no sub-issues in this project")
    return sorted(children, key=lambda i: int(i.get("number") or 0))


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
    outside = {
        "staged": "NOT STAGED",
        "inbox": "NOT IN INBOX",
        "operator": "NOT AN OPERATOR ITEM",
    }[pool]
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


def already_moved(issue: dict, cancel: bool, target: str = "Todo") -> bool:
    """A task an earlier, partly failed run of the batch already wrote: an
    approval sits in ``target`` with no milestone, a reject is canceled."""
    state = issue.get("state") or {}
    if cancel:
        return state.get("type") in ("canceled", "duplicate")
    milestone = issue.get("projectMilestone")
    return not milestone and str(state.get("name", "")).lower() == target.lower()


def plan_approval(
    approve: list[dict], cancel: list[dict], held: list[str], pool: str = "staged"
) -> tuple[dict[str, dict], list[str]]:
    """``(updates, already)``: the batch ``board_batch.apply_fields`` takes and
    the ids a previous run already moved — or a refusal naming every reason the
    batch cannot run yet. Skipping the already-moved ids is what lets a write
    that failed partway be re-run with the same arguments.

    The inbox and operator pools approve one task per write; the inbox clears
    a task's milestone only when it carries the curator's ``Curator pending``
    mark, and an operator item's approval is its Done."""
    if pool in ("inbox", "operator") and len(approve) > 1:
        raise RatifyError(
            f"refusing the batch, nothing written — the {pool} pool approves one "
            f"task at a time, got {len(approve)}: "
            + ", ".join(i["identifier"] for i in approve)
        )
    target = APPROVED_STATE[pool]
    overlap = {i["identifier"] for i in approve} & {i["identifier"] for i in cancel}
    already = [
        i["identifier"]
        for i in approve
        if already_moved(i, cancel=False, target=target)
    ]
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

    def clear(issue: dict) -> dict:
        if pool == "staged" or curator_marked(issue):
            return {"milestone": None}
        return {}

    updates = {i["identifier"]: {**clear(i), "state": target} for i in approve}
    updates.update({i["identifier"]: {**clear(i), "state": "Canceled"} for i in cancel})
    return updates, already


_MILESTONES_QUERY = """
query($projectId: String!) {
  project(id: $projectId) { projectMilestones { nodes { id name } } }
}
"""


def milestone_id(api_key: str, project: str, name: str) -> str:
    """The id of the project milestone named ``name``; zero or several matches
    are refused rather than guessed."""
    data = board_batch._post(api_key, _MILESTONES_QUERY, {"projectId": project})
    nodes = ((data.get("project") or {}).get("projectMilestones") or {}).get(
        "nodes"
    ) or []
    matches = [n for n in nodes if n.get("name") == name]
    if len(matches) != 1:
        raise RatifyError(
            f"{len(matches)} milestones are named {name!r} in this project — "
            "expected exactly one"
        )
    return str(matches[0]["id"])


def plan_mark(issues: list[dict], mark_id: str) -> tuple[dict[str, dict], list[str]]:
    """``(updates, skipped)`` stamping ``Curator pending`` on the inbox members
    of ``issues``; a member already carrying it needs no write, and an
    out-of-pool id is skipped and named."""
    updates: dict[str, dict] = {}
    skipped: list[str] = []
    for issue in issues:
        if not in_pool(issue, "inbox"):
            skipped.append(issue["identifier"])
        elif not curator_marked(issue):
            updates[issue["identifier"]] = {"milestone": mark_id}
    return updates, skipped


def plan_unmark(issues: list[dict]) -> dict[str, dict]:
    """The writes clearing ``Curator pending`` from whichever of ``issues``
    carry it — never another milestone."""
    return {i["identifier"]: {"milestone": None} for i in issues if curator_marked(i)}


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="cmd", required=True)
    listing = sub.add_parser("list", help="every open issue in the pool")
    listing.add_argument("--parent", help="only the sub-issues of this ENG-###")
    ask = sub.add_parser("ask", help="the one-checkbox-per-task review ask")
    ask.add_argument(
        "--operator",
        help="the one operator item, its own question after the review ticks; "
        "with no --ids, the operator-items-only ask",
    )
    ask.add_argument("--notes", help="JSON id -> note, inline or a file path")
    ask.add_argument("--proposals", help="JSON list of edges / kills / merges")
    ask.add_argument("--out", help="write the JSON here instead of stdout")
    review = sub.add_parser("review", help="approved or held, per ticked task")
    approve = sub.add_parser("approve", help="the one batch write at the end")
    approve.add_argument("--cancel", nargs="+", default=[])
    approve.add_argument(
        "--allow",
        nargs="+",
        default=[],
        help="--parent only: approve these sub-issues though the last ask "
        "--parent did not carry them (its record: ratify-ask-<parent>.json in "
        "the base checkout's .claude/session-substrate)",
    )
    approve.add_argument(
        "--title",
        help="inbox only: retitle the one approved task in the same write; the "
        "leading token run must not change",
    )
    approve.add_argument(
        "--chore",
        help=f"retoken this one operator item {CHORE_TOKEN}, leaving it in Todo "
        "for the scheduler; on the operator's `attended chore` comment only",
    )
    approve.add_argument("--dry-run", action="store_true")
    gate = sub.add_parser("gate", help="fail while any id is still staged")
    for command in (ask, review, approve, gate):
        # The set is named either way: the ids, or the governing issue whose
        # sub-issues they are. approve may name neither (cancels only), and
        # ask neither when it carries only `--operator`; `run` checks those.
        which = command.add_mutually_exclusive_group(
            required=command not in (ask, approve)
        )
        which.add_argument("--ids", nargs="+", default=[])
        which.add_argument("--parent", help="the sub-issues of this ENG-###")
    for command in (listing, ask, review, approve, gate):
        command.add_argument("--pool", choices=POOLS, default="staged")
    mark = sub.add_parser("mark", help=f"stamp an inbox batch {CURATOR_MILESTONE}")
    mark.add_argument("--ids", nargs="+", required=True)
    mark.add_argument("--dry-run", action="store_true")
    unmark = sub.add_parser("unmark", help=f"clear {CURATOR_MILESTONE}")
    which = unmark.add_mutually_exclusive_group(required=True)
    which.add_argument("--ids", nargs="+")
    which.add_argument(
        "--all",
        action="store_true",
        help="every open issue carrying it: the close and bootstrap sweeps",
    )
    unmark.add_argument("--dry-run", action="store_true")
    return parser.parse_args(argv)


def run_mark(args: argparse.Namespace, key: str, project: str) -> int:
    """``mark`` / ``unmark``: the curator's ``Curator pending`` writes, one
    ``board_batch`` field batch each."""
    if args.cmd == "unmark" and args.all:
        issues = board_batch.fetch_open_in_milestones(key, project, [CURATOR_MILESTONE])
    else:
        issues = resolve_all(key, project, parse_ids(args.ids))
    if args.cmd == "mark":
        updates, skipped = plan_mark(
            issues, milestone_id(key, project, CURATOR_MILESTONE)
        )
        for ident in skipped:
            print(f"SKIPPED {ident} | not {POOL_WHERE['inbox']}")
    else:
        updates = plan_unmark(issues)
    if not updates:
        print(f"NOTHING to write — {CURATOR_MILESTONE} already as asked")
        return 0
    by_number = {int(i["number"]): i for i in issues}
    board_batch.apply_fields(key, updates, by_number, dry_run=args.dry_run)
    return 0


def run(argv: list[str]) -> int:
    args = _parse_args(argv)
    if args.cmd in ("mark", "unmark"):
        return run_mark(
            args,
            board_batch.env_var("LINEAR_API_KEY"),
            board_batch.env_var("LINEAR_PROJECT_ID"),
        )
    pool = args.pool
    if args.cmd == "gate" and pool != "staged":
        raise RatifyError(
            f"gate is for the staged pool — nothing is staged in the {pool} "
            "pool, so there is nothing for a close gate to hold"
        )
    if args.cmd == "ask" and pool == "operator":
        raise RatifyError(
            "an operator item rides --operator on an inbox ask, never --ids"
        )
    if args.cmd == "ask" and args.operator and pool != "inbox":
        raise RatifyError(
            "--operator rides the curator's inbox batch — pass --pool inbox"
        )
    # `is not None` throughout, never truthiness: `--parent ""` must reach
    # `fetch_children`'s refusal, not fall back to an empty `--ids` set.
    parent = args.parent
    if parent is not None and pool != "staged":
        # Every other pool approves by id: on the operator pool an implicit
        # sweep of a parent's children would move an operator item to Done
        # unnamed.
        raise RatifyError(
            f"--parent names a seat session's staged set — the {pool} pool is "
            "reviewed by id, one task at a time"
        )
    key = board_batch.env_var("LINEAR_API_KEY")
    project = board_batch.env_var("LINEAR_PROJECT_ID")

    def named() -> list[dict]:
        """The set the command names: the ids, or the parent's sub-issues."""
        if parent is not None:
            return fetch_children(key, project, parent)
        return resolve_all(key, project, parse_ids(args.ids))

    if args.cmd == "list":
        if parent is not None:
            members = [
                i for i in fetch_children(key, project, parent) if in_pool(i, pool)
            ]
            lines = board_batch.format_listing(members) or [
                f"none of {parent.upper()}'s sub-issues is {POOL_WHERE[pool]}"
            ]
        elif pool == "inbox":
            issues = board_batch.fetch_issues(key, project, states=[INBOX_STATE])
            members = [i for i in issues if in_pool(i, pool)]
            lines = format_pool_listing(members) or [f"none {POOL_WHERE[pool]}"]
        elif pool == "operator":
            issues = board_batch.fetch_issues(key, project, states=[OPERATOR_STATE])
            members = sorted((i for i in issues if in_pool(i, pool)), key=priority_rank)
            # One line at a time: format_listing re-sorts by number, and the
            # order here is the walk's.
            lines = [line for i in members for line in board_batch.format_listing([i])]
            lines = lines or [f"none is {POOL_WHERE[pool]}"]
        else:
            issues = board_batch.fetch_open_in_milestones(key, project, [MILESTONE])
            lines = board_batch.format_listing(issues) or [f"none under {MILESTONE}"]
        for line in lines:
            print(line)
        return 0

    if args.cmd == "ask":
        if not args.ids and parent is None and not args.operator:
            raise RatifyError(
                "nothing to ask: pass --ids or --parent, --operator, or both"
            )
        if parent is not None:
            issues = fetch_children(key, project, parent)
            ids = [i["identifier"] for i in issues]
        else:
            ids = parse_ids(args.ids) if args.ids else []
            issues = resolve_all(key, project, ids) if ids else []
        staged = [i for i in issues if in_pool(i, pool)]
        skipped = [i["identifier"] for i in issues if not in_pool(i, pool)]
        if ids and not staged:
            raise RatifyError(f"none of these is {POOL_WHERE[pool]}")
        operator_ids = parse_ids([args.operator]) if args.operator else []
        if len(operator_ids) > 1:
            raise RatifyError("--operator takes one item: the fifth box is one box")
        # Checked against the requested ids, so a note for an id that has
        # since left the pool is skipped with it rather than refusing the ask.
        notes = parse_notes(load_json_arg(args.notes), ids + operator_ids)
        proposals = parse_proposals(load_json_arg(args.proposals))
        operator = None
        if operator_ids:
            (item,) = resolve_all(key, project, operator_ids)
            ident = item["identifier"]
            if not in_pool(item, "operator"):
                raise RatifyError(f"{ident} is not {POOL_WHERE['operator']}")
            dates = deferrals(fetch_comments(key, ident))
            base = notes.pop(ident, None) or item["title"]
            operator = {
                "label": ident,
                "description": operator_note(base, dates, datetime.date.today()),
            }
        payload = build_ask(staged, notes, proposals, operator)
        if parent is not None:
            # What `approve --parent` checks its set against.
            write_ask_record(parent, [i["identifier"] for i in staged])
        if skipped:
            payload["skipped"] = skipped
        text = json.dumps(payload, indent=2)
        if args.out:
            with open(args.out, "w", encoding="utf-8") as handle:
                handle.write(text + "\n")
            print(f"WROTE {args.out} | {len(staged)} task(s)", end="")
            print(f", operator {operator['label']}" if operator else "", end="")
            print(f", skipped {', '.join(skipped)}" if skipped else "")
        else:
            print(text)
        return 0

    if args.cmd == "review":
        issues = named()
        if parent is not None:
            # A parent names the whole set, already-moved tasks included; only
            # the ones still staged have anything to review.
            issues = [i for i in issues if in_pool(i, pool)] or issues
        lines, _ = review_lines(key, issues, pool)
        print("\n".join(lines))
        return 0

    if args.cmd == "approve" and args.chore is not None:
        if args.ids or args.cancel or args.title is not None or parent is not None:
            raise RatifyError(
                "--chore is its own write: pass it alone, with no --ids, "
                "--parent, --cancel or --title"
            )
        chore_ids = parse_ids([args.chore])
        if len(chore_ids) != 1:
            raise RatifyError("--chore hands over one operator item at a time")
        (item,) = resolve_all(key, project, chore_ids)
        ident = item["identifier"]
        if not in_pool(item, "operator"):
            raise RatifyError(f"{ident} is not {POOL_WHERE['operator']}")
        board_batch.apply_fields(
            key,
            {ident: {"title": chore_title(item)}},
            {int(item["number"]): item},
            dry_run=args.dry_run,
            retoken=True,
        )
        return 0

    if args.cmd == "approve":
        cancel_ids = parse_ids(args.cancel) if args.cancel else []
        approve_ids = parse_ids(args.ids) if args.ids else []
        # `--parent` is refused on every pool but staged, so this refuses a
        # `--title` beside it too.
        if args.title is not None and (pool != "inbox" or len(approve_ids) != 1):
            raise RatifyError(
                "--title retitles the one task an inbox approve moves — "
                "pass it with --pool inbox and exactly one --ids"
            )
        if args.allow and parent is None:
            raise RatifyError(
                "--allow admits a sub-issue the last ask did not carry — pass "
                "it with --parent"
            )
        if parent is not None:
            children = fetch_children(key, project, parent)
            cancel = resolve_among(cancel_ids, children, "--cancel", parent)
            cancel_ids = [i["identifier"] for i in cancel]
            allow_ids = parse_ids(args.allow) if args.allow else []
            allowed = {
                i["identifier"]
                for i in resolve_among(allow_ids, children, "--allow", parent)
            }
            # Every staged sub-issue not rejected is approved — the final
            # answer covers the whole set. One an earlier run already moved
            # rides along so `plan_approval` reports it ALREADY and the same
            # command re-runs cleanly; any other child is named, never
            # silently dropped.
            approve = []
            for issue in children:
                if issue["identifier"] in cancel_ids:
                    continue
                if in_pool(issue, pool) or already_moved(issue, cancel=False):
                    approve.append(issue)
                else:
                    print(f"SKIPPED {issue['identifier']} | not {POOL_WHERE[pool]}")
            # The set was never typed, so it is held to the last ask: a child
            # the operator never saw is not theirs to have approved.
            asked = read_ask_record(parent)
            unseen = not_asked([i for i in approve if in_pool(i, pool)], asked, allowed)
            if unseen:
                why = (
                    f"no ask --parent {parent_ident(parent)} is recorded, so "
                    "none was reviewed"
                    if asked is None
                    else "the last ask did not carry"
                )
                raise RatifyError(
                    f"refusing the batch, nothing written — {why}: "
                    f"{', '.join(unseen)}; ask over them, or pass --allow "
                    "naming each the operator did review"
                )
        else:
            approve = resolve_all(key, project, approve_ids) if approve_ids else []
            cancel = resolve_all(key, project, cancel_ids) if cancel_ids else []
        if not approve and not cancel:
            raise RatifyError("nothing to approve or cancel")
        _, held = review_lines(key, [i for i in approve if in_pool(i, pool)], pool)
        updates, already = plan_approval(approve, cancel, held, pool)
        if args.title is not None:
            ident = approve[0]["identifier"]
            if ident in updates:
                updates[ident]["title"] = args.title
            else:
                # Already moved by another hand: the move is skipped, and so
                # is the retitle that rode with it — say so, never drop it.
                print(f"TITLE NOT APPLIED {ident} | retitle it with board_batch.py")
        for ident in already:
            print(f"ALREADY {ident} | already in its target state, skipped")
        by_number = {int(i["number"]): i for i in approve + cancel}
        board_batch.apply_fields(key, updates, by_number, dry_run=args.dry_run)
        return 0

    issues = named()
    still = [i["identifier"] for i in issues if in_pool(i, pool)]
    if still:
        print(f"OPEN — still under {MILESTONE}: {', '.join(still)}")
        return 1
    # A parent's set was never typed by the caller, so it is named.
    named_set = (
        f" ({', '.join(i['identifier'] for i in issues)})" if parent is not None else ""
    )
    print(f"CLEAR — none of {len(issues)} task(s){named_set} is under {MILESTONE}")
    return 0


def main() -> int:
    try:
        return run(sys.argv[1:])
    except (RatifyError, board_batch.BoardBatchError) as exc:
        print(f"ratify_staged: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
