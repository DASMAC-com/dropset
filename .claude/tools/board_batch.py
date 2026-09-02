#!/usr/bin/env python3
"""board_batch.py — batched Linear board writes and a compact board read.

The planning session's hands. Every subcommand here exists because the
equivalent MCP call echoes an issue's **entire stored body** back for each
write, which dominates a planning session's token profile: one measured
session made 21 writes of which **17 touched no body at all** — a priority
change, some parent/state moves, eleven milestone stamps and a relation
removal — and paid roughly 40k echoing bodies to confirm changes that fit on
17 lines. Linear's ``issueUpdate`` returns whatever the caller selects, so
requesting ``success`` alone makes the echo vanish.

Five subcommands, all reading their config from the environment (no hard-coded
ids, never a committed token):

* ``list`` — compact open-Backlog listing: ``number | priority | title``, one
  line each. Roughly 600 tokens where the MCP equivalent measured ~11k.
* ``fields --updates FILE`` — every non-body **issue field**: priority, state,
  parent, milestone, labels, assignee. Takes an explicit map of issue number
  to field values and prints one line per issue.
* ``priorities --updates FILE`` — a thin alias of ``fields`` for the
  priority-only case, kept because a priority sweep is the common shape.
* ``state --id ENG-### --state NAME`` — a thin alias of ``fields`` for the
  single-issue lifecycle transition, so ``init-pr``'s move to In Progress and
  ``review-pr``'s to In Review are one command rather than a JSON file
  composed to carry one enum. Through the MCP those cost ~3.6k each.
* ``edges --pairs FILE [--remove]`` — add or remove **operator-directed**
  blocking edges.

**Relations are not issue fields, and ``fields`` does not take them.** They
are a separate mutation pair (``issueRelationCreate`` /
``issueRelationDelete``), so they live in ``edges`` — which is also where the
human-curated policy below belongs. Passing a relation key to ``fields``
raises ``unknown field(s)``, deliberately.

Configuration:

* ``LINEAR_API_KEY`` — a personal API key (the interactive claude.ai Linear MCP
  rides OAuth and won't authenticate from a script), sent verbatim as the
  ``Authorization`` header.
* ``LINEAR_PROJECT_ID`` — the Dropset project. Issue numbers are resolved
  against this project alone; there is no team-key setting, because a
  reference that carries a team prefix is validated against the resolved
  issue's own ``identifier`` instead (see :func:`resolve_issue`).

Two constraints that are **policy, not implementation detail**:

**``edges`` executes a human's decision; it never derives one.** Blocking
edges are human-curated end to end (``CLAUDE.md`` → "Blocking relations"), and
this subcommand exists only because there is no MCP path for relations at all.
So it takes an **explicit pair list**, has **no discovery mode**, and
**refuses an empty list**. It is never called by a filing skill or by
automation, and since the automated file-collision machinery retired it is
the only relation writer left in the repo at all. The
no-automated-blocking-edges rule is unchanged — this tool is how a human's
decision gets *executed*.

**Body edits stay on the MCP ``patch`` path, deliberately.** Linear's API has
no patch primitive: ``description`` is a whole string, so a Python body-writer
would have to fetch, apply locally, and write back wholesale — which costs the
read anyway and reintroduces the round-trip corruption hazard that the ``plan``
skill's close-out step documents (code spans coming back mangled). The MCP
``patch`` does anchor matching with atomic abort on ambiguity, and that safety
is load-bearing: it has correctly refused writes whose anchor matched twice
rather than guessing. Do not "finish the job" by adding a body writer here.

The one exception is an **append**, and the distinction is anchoring: an
append has no anchor, so nothing can match ambiguously and the atomic abort
protects nothing. It therefore leaves the MCP — but it lives in
``linear_issue.py``, not here, because this tool is the non-body writer and
mixing the two would blur exactly the boundary the paragraph above draws.

Usage:
    python3 .claude/tools/board_batch.py list
    python3 .claude/tools/board_batch.py list --state Todo
    python3 .claude/tools/board_batch.py fields --updates updates.json
    python3 .claude/tools/board_batch.py priorities --updates priorities.json
    python3 .claude/tools/board_batch.py state --id ENG-123 --state "In Review"
    python3 .claude/tools/board_batch.py edges --pairs edges.json
    python3 .claude/tools/board_batch.py edges --pairs edges.json --remove

``--dry-run`` prints what each write subcommand *would* do and writes nothing.
Rehearse an ``edges`` run that way first: a blocking edge drops an issue out
of the operator's available set, so a wrong one is expensive. It is accepted
in either position (``--dry-run edges …`` or ``edges … --dry-run``).
"""

from __future__ import annotations

import argparse
import json
import re
import sys

import linear_api

ENDPOINT = linear_api.ENDPOINT

# How many issues one page of a listing query reads. Reads follow the cursor, so
# this is a page size and not a board ceiling. It used to be both, with a comment
# asserting the board was "far under this" — that went false in August 2026 at
# roughly 575 issues, and because the resolver read was project-wide and
# unfiltered, every write subcommand failed outright until these reads paged.
PAGE_SIZE = 250

# How many issue numbers one resolver query names. A write path resolves only the
# issues its own payload references, so this bounds a chunk of that lookup rather
# than the board. The reason to keep it modest is **query size**, not truncation:
# `_fetch_filtered` pages, so a chunk cannot truncate however many cross-team
# number collisions it contains. (An earlier comment here claimed the opposite
# and was wrong in both directions.)
RESOLVE_CHUNK = 100

# Runaway backstop on a cursor-following read: 40 pages is ~10k issues, far above
# any real project, so tripping this means an endpoint returning a cursor that
# never terminates rather than a board that grew.
MAX_PAGES = 40

# Overall per-request timeout, so a hung endpoint can't wedge a run.
REQUEST_TIMEOUT = 30

# Linear's priority scale, accepted by name so an updates file stays readable.
PRIORITY_NAMES = {
    "none": 0,
    "urgent": 1,
    "high": 2,
    "medium": 3,
    "low": 4,
}
PRIORITY_LABELS = {v: k for k, v in PRIORITY_NAMES.items()}

# The field names `fields` accepts, mapped to their issueUpdate argument.
# Anything not listed here is rejected rather than passed through, so a typo
# fails loudly instead of silently updating nothing.
SCALAR_FIELDS = {
    "priority": "priority",
    "state": "stateId",
    "parent": "parentId",
    "milestone": "projectMilestoneId",
    "assignee": "assigneeId",
    "labels": "labelIds",
}


class BoardBatchError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def env_var(name: str) -> str:
    return linear_api.env_var(name, error=BoardBatchError)


def _post(api_key: str, query: str, variables: dict) -> dict:
    """POST a GraphQL operation and return its ``data``, surfacing transport
    and GraphQL-level errors with their messages.

    Delegates to the shared transport rather than keeping a second HTTP idiom
    in this directory — which is also what gives this tool the redirect refusal
    (a followed 3xx would re-send the ``Authorization`` header to a new host).
    """
    return linear_api.post(
        api_key,
        query,
        variables,
        endpoint=ENDPOINT,
        timeout=REQUEST_TIMEOUT,
        error=BoardBatchError,
    )


_ISSUES_QUERY = """
query($filter: IssueFilter, $first: Int!, $after: String) {
  issues(filter: $filter, first: $first, after: $after) {
    pageInfo { hasNextPage endCursor }
    nodes {
      id
      identifier
      number
      title
      priority
      state { name type }
      projectMilestone { name }
      team { id }
    }
  }
}
"""

# The team's workflow states, for resolving a state NAME to its UUID. Linear's
# `stateId` takes an id and nothing else: a name reaches it as an opaque string
# and dies as an unnamed `Argument Validation Error`, which cost one session six
# round trips to diagnose because the documented example prescribed a name.
_STATES_QUERY = """
query TeamStates($teamId: String!) {
  team(id: $teamId) { states(first: 100) { nodes { id name } } }
}
"""

# A Linear id is a UUID. Used to tell "already an id" from "a name to resolve",
# and to refuse a name on the fields this tool cannot resolve for you.
UUID_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$", re.IGNORECASE
)

# Fields this tool cannot resolve for you: their value must already be an id.
# Named individually so the refusal below can say which one.
ID_ONLY_FIELDS = ("milestone", "assignee")


def _fetch_filtered(api_key: str, issue_filter: dict) -> list[dict]:
    """Every issue matching ``issue_filter``, following the cursor to the end.

    Selects only the fields a listing or a number-to-id lookup needs — never
    ``description``, which is the whole point of this tool.

    Paging rather than refusing is the fix for the failure this tool shipped
    with: one page plus a guard meant that crossing the page size turned every
    write into a hard error, and the documented fallback for that error was the
    full-body MCP ``save_issue`` — the exact echo cost the tool exists to remove.
    """
    nodes: list[dict] = []
    after: str | None = None
    for _ in range(MAX_PAGES):
        data = _post(
            api_key,
            _ISSUES_QUERY,
            {"filter": issue_filter, "first": PAGE_SIZE, "after": after},
        )
        conn = data.get("issues") or {}
        page = conn.get("nodes") or []
        nodes.extend(page)
        info = conn.get("pageInfo") or {}
        if not info.get("hasNextPage"):
            # The old code carried a truncation backstop independent of the
            # server's own claim; paging replaced it with trust in `hasNextPage`.
            # Keep a floor for the one case where that trust has nothing behind
            # it: a FULL page with no `pageInfo` at all. On the resolver path a
            # short read degrades to a loud `resolve_issue` refusal, but `list`
            # has no such net and would print a truncated board as the whole one.
            if not info and len(page) >= PAGE_SIZE:
                raise BoardBatchError(
                    f"read returned a full page of {PAGE_SIZE} with no pageInfo "
                    "— refusing to report a possibly-truncated board as complete"
                )
            return nodes
        after = info.get("endCursor")
        if not after:
            raise BoardBatchError(
                "Linear reported another page but returned no cursor — refusing "
                "to loop or to report a truncated read"
            )
    raise BoardBatchError(
        f"read did not terminate within {MAX_PAGES} pages of {PAGE_SIZE} — "
        "refusing to keep paging; narrow the filter"
    )


def fetch_issues(
    api_key: str, project_id: str, states: list[str] | None = None
) -> list[dict]:
    """Issues in the project, optionally filtered to named workflow states.

    The **listing** read. Write paths must not use this: they want
    :func:`fetch_issues_by_number`, whose cost tracks the payload rather than
    the board.
    """
    issue_filter: dict = {"project": {"id": {"eq": project_id}}}
    if states:
        issue_filter["state"] = {"name": {"in": states}}
    return _fetch_filtered(api_key, issue_filter)


def fetch_issues_by_number(
    api_key: str, project_id: str, numbers: list[int]
) -> list[dict]:
    """Just the issues a write payload names, resolved for number-to-id lookup.

    The resolver read. Indexing the whole project to resolve a handful of
    numbers is what coupled every write to board size; filtering on the numbers
    actually referenced removes the cliff instead of relocating it, and chunking
    keeps a large updates file working too.

    A number absent from the result is left absent — :func:`resolve_issue` turns
    that into a hard error naming the reference, which is the same outcome as
    before and still never a guess.
    """
    unique = sorted({int(n) for n in numbers})
    if not unique:
        return []
    nodes: list[dict] = []
    for start in range(0, len(unique), RESOLVE_CHUNK):
        chunk = unique[start : start + RESOLVE_CHUNK]
        nodes.extend(
            _fetch_filtered(
                api_key,
                {
                    "project": {"id": {"eq": project_id}},
                    "number": {"in": chunk},
                },
            )
        )
    return nodes


def index_by_number(issues: list[dict]) -> dict[int, dict]:
    """Issues keyed by their integer number, for number-to-id resolution.

    Raises on a duplicate number rather than letting the last one win. Linear
    numbers are per-**team**, so a project spanning two teams can legitimately
    contain two issues numbered 123 — and silently resolving to whichever came
    back last is exactly the wrong-issue mutation this tool must not make.
    """
    index: dict[int, dict] = {}
    for issue in issues:
        if issue.get("number") is None:
            continue
        number = int(issue["number"])
        existing = index.get(number)
        if existing is not None:
            raise BoardBatchError(
                f"issue number {number} is ambiguous in this project "
                f"({existing.get('identifier')} and {issue.get('identifier')}) "
                "— refusing to guess which one a reference means"
            )
        index[number] = issue
    return index


def format_listing(issues: list[dict], *, show_milestone: bool = False) -> list[str]:
    """One compact ``number | priority | title`` line per issue.

    This is the whole reason `list` exists: the same information the MCP
    listing carries, minus the bodies nobody asked for.
    """
    lines = []
    for issue in sorted(issues, key=lambda i: int(i.get("number") or 0)):
        priority = PRIORITY_LABELS.get(issue.get("priority") or 0, "none")
        row = (
            f"{issue.get('identifier', '?')} | {priority:<6} | {issue.get('title', '')}"
        )
        if show_milestone:
            milestone = (issue.get("projectMilestone") or {}).get("name") or "-"
            row = f"{row}  [{milestone}]"
        lines.append(row)
    return lines


def drop_milestoned(issues: list[dict]) -> list[dict]:
    """Issues carrying no project milestone.

    A milestone means **parked** (see the `plan` skill's board schema), and
    Linear's issue-list API has no milestone filter — so the drop is
    client-side, which is exactly why it lives here rather than in a query.
    """
    return [i for i in issues if not (i.get("projectMilestone") or {}).get("name")]


_UPDATE_MUTATION = """
mutation($id: String!, $input: IssueUpdateInput!) {
  issueUpdate(id: $id, input: $input) { success }
}
"""


def normalize_priority(value) -> int:
    """A priority given as a name or a number, as Linear's integer scale."""
    if isinstance(value, bool):
        raise BoardBatchError(f"priority {value!r} is not a priority")
    if isinstance(value, int):
        if value not in PRIORITY_LABELS:
            raise BoardBatchError(
                f"priority {value} out of range — expected 0-4 or one of "
                f"{', '.join(PRIORITY_NAMES)}"
            )
        return value
    if isinstance(value, str):
        key = value.strip().lower()
        if key in PRIORITY_NAMES:
            return PRIORITY_NAMES[key]
    raise BoardBatchError(
        f"priority {value!r} unrecognized — expected 0-4 or one of "
        f"{', '.join(PRIORITY_NAMES)}"
    )


def build_update_input(fields: dict) -> dict:
    """Translate a caller's field map into an ``IssueUpdateInput``.

    Rejects unknown field names outright: a typo that silently updated nothing
    would be reported as success, which is worse than failing.
    """
    if not isinstance(fields, dict):
        raise BoardBatchError(
            f"expected an object of fields, got {type(fields).__name__}"
        )
    unknown = set(fields) - set(SCALAR_FIELDS)
    if unknown:
        raise BoardBatchError(
            f"unknown field(s) {', '.join(sorted(unknown))} — supported: "
            f"{', '.join(sorted(SCALAR_FIELDS))}"
        )
    update: dict = {}
    for name, value in fields.items():
        arg = SCALAR_FIELDS[name]
        if name == "priority":
            update[arg] = normalize_priority(value)
        elif name == "labels":
            if not isinstance(value, list):
                raise BoardBatchError("labels must be a list of label ids")
            for item in value:
                _refuse_obvious_name("labels", item)
            update[arg] = value
        elif name in ID_ONLY_FIELDS:
            _refuse_obvious_name(name, value)
            update[arg] = value
        else:
            # `null` clears the field — which is how a milestone is cleared to
            # un-park a finding, so it must pass through rather than be dropped.
            # `state` and `parent` are resolved later, once the issue (and so its
            # team) is known; see `_resolve_field_values`.
            update[arg] = value
    if not update:
        raise BoardBatchError("no fields to update")
    return update


def _refuse_obvious_name(field: str, value) -> None:
    """Refuse a value that is plainly a human-readable name, not an id.

    Linear takes an id here and nothing else, and a name reaches it as an opaque
    string that dies as an unnamed ``Argument Validation Error`` — the same trap
    that cost one session six round trips on ``state``. ``state`` and ``parent``
    are *resolved* rather than refused; these fields have no cheap lookup, so the
    next best thing is failing with a message that names the problem.

    The test is deliberately **whitespace**, not UUID-shape: the ids callers pass
    are opaque strings this tool must not second-guess, while every real instance
    of the trap (``"Audit findings"``, ``"In Review"``, a person's name) contains
    a space. That keeps the guard free of false positives.
    """
    if isinstance(value, str) and value.strip() and any(c.isspace() for c in value):
        raise BoardBatchError(
            f"{field} was given {value!r}, which is a name rather than an id. Linear "
            f"accepts only ids for {field}, and a name fails as an unnamed "
            "Argument Validation Error — look the id up and pass that."
        )


def resolve_state_name(nodes: list, wanted: str) -> str:
    """A state id for ``wanted``, matched case-insensitively by name."""
    lowered = wanted.strip().lower()
    for node in nodes:
        if str(node.get("name", "")).strip().lower() == lowered:
            return str(node["id"])
    names = ", ".join(sorted(str(n.get("name")) for n in nodes))
    raise BoardBatchError(f"no state named {wanted!r} on this team — have: {names}")


def _team_states(api_key: str, team_id: str, cache: dict) -> list:
    """The team's workflow states, read once per team per run."""
    if team_id not in cache:
        data = _post(api_key, _STATES_QUERY, {"teamId": team_id})
        cache[team_id] = (((data.get("team") or {}).get("states")) or {}).get(
            "nodes"
        ) or []
    return cache[team_id]


def _resolve_field_values(
    api_key: str,
    issue: dict,
    update: dict,
    by_number: dict[int, dict],
    cache: dict,
) -> None:
    """Turn caller-friendly ``state`` / ``parent`` values into ids, in place.

    Resolution happens in the pre-flight, **including on a dry run**. That is
    the point: ``--dry-run`` used to resolve issue numbers but never field
    values, so a state name gave a confident green and then failed for real —
    a false green is worse than no check.
    """
    state = update.get("stateId")
    if isinstance(state, str) and not UUID_RE.match(state):
        team_id = (issue.get("team") or {}).get("id")
        if not team_id:
            raise BoardBatchError(
                f"{issue['identifier']} carried no team, so state {state!r} cannot "
                "resolve — pass a state UUID instead"
            )
        update["stateId"] = resolve_state_name(
            _team_states(api_key, team_id, cache), state
        )

    parent = update.get("parentId")
    if parent is not None and not (isinstance(parent, str) and UUID_RE.match(parent)):
        # An issue number or `ENG-###` — resolved through the same lookup the
        # tool already uses for the issues being updated.
        update["parentId"] = resolve_issue(parent, by_number)["id"]


def apply_fields(
    api_key: str,
    updates: dict,
    by_number: dict[int, dict],
    *,
    dry_run: bool = False,
    emit=None,
) -> list[str]:
    """Apply each issue's field map. Returns one report line per issue.

    **Validates the whole batch before issuing any write.** Resolution and
    input-building are pure, so doing them up front means a typo in the fifth
    entry fails with nothing mutated — rather than leaving four writes applied
    and the rest not. ``emit`` (default ``print``) reports each write as it
    lands, so an error partway through still leaves an audit trail of what
    actually happened; the returned list is the same lines, for callers that
    want them.
    """
    emit = print if emit is None else emit
    # Pre-flight: resolve and build everything, mutating nothing. Field VALUES
    # are resolved here too, so a dry run validates them rather than reporting a
    # green it has not earned.
    planned = []
    state_cache: dict = {}
    for raw_number, fields in updates.items():
        issue = resolve_issue(raw_number, by_number)
        update = build_update_input(fields)
        _resolve_field_values(api_key, issue, update, by_number, state_cache)
        summary = ", ".join(f"{k}={fields[k]!r}" for k in sorted(fields))
        planned.append((issue, update, summary))

    lines = []
    for issue, update, summary in planned:
        if dry_run:
            line = f"WOULD SET {issue['identifier']} | {summary}"
        else:
            data = _post(
                api_key, _UPDATE_MUTATION, {"id": issue["id"], "input": update}
            )
            if not (data.get("issueUpdate") or {}).get("success"):
                raise BoardBatchError(
                    f"{issue['identifier']}: issueUpdate reported failure"
                )
            line = f"SET {issue['identifier']} | {summary}"
        emit(line)
        lines.append(line)
    return lines


def _as_ref(raw) -> tuple[str | None, int]:
    """An issue reference as ``(team_prefix_or_None, number)``.

    Accepts ``123``, ``"123"``, or ``"ENG-123"``. The prefix is **kept** rather
    than discarded: Linear numbers are per-team, so throwing it away would let
    ``"FIN-123"`` silently resolve to ``ENG-123`` and mutate the wrong issue.
    :func:`resolve_issue` is what enforces it.
    """
    if isinstance(raw, bool):
        raise BoardBatchError(f"cannot read an issue number from {raw!r}")
    if isinstance(raw, int):
        number = raw
        prefix = None
    else:
        text = str(raw).strip().upper()
        prefix = None
        if "-" in text:
            prefix, _, text = text.rpartition("-")
            prefix = prefix or None
        try:
            number = int(text)
        except ValueError as e:
            raise BoardBatchError(f"cannot read an issue number from {raw!r}") from e
    if number <= 0:
        raise BoardBatchError(f"issue number must be positive, got {raw!r}")
    return prefix, number


def resolve_issue(raw, by_number: dict[int, dict], label: str = "issue") -> dict:
    """The issue ``raw`` names, or a hard error — never a guess.

    Validates a caller-supplied team prefix against the resolved issue's own
    ``identifier``, so a reference from another team fails loudly instead of
    mutating this project's issue of the same number.
    """
    prefix, number = _as_ref(raw)
    issue = by_number.get(number)
    if issue is None:
        raise BoardBatchError(
            f"{label} {raw!r} is not in this project — refusing to "
            "guess; check the number and the project"
        )
    if prefix is not None:
        actual = str(issue.get("identifier", "")).rpartition("-")[0].upper()
        if actual and prefix != actual:
            raise BoardBatchError(
                f"{label} {raw!r} names team {prefix}, but issue {number} in "
                f"this project is {issue.get('identifier')} — refusing to "
                "mutate a different team's issue"
            )
    return issue


def referenced_numbers(refs) -> list[int]:
    """The issue numbers an iterable of references names, to bound a read.

    Deliberately lenient: a reference it cannot parse is skipped rather than
    rejected, because :func:`apply_fields` and :func:`place_edges` already
    pre-flight the whole payload and their errors name the offending entry
    precisely. Raising here would duplicate that validation in a second place
    and change those messages; the only consequence of a skip is that the
    resolver does not fetch an issue the pre-flight is about to reject anyway.
    """
    numbers: list[int] = []
    for raw in refs:
        try:
            numbers.append(_as_ref(raw)[1])
        except BoardBatchError:
            continue
    return numbers


#: The reference keys an ``edges`` pair carries. Named once so `pair_refs` and
#: `place_edges` cannot disagree: a third key added to one and not the other
#: would be silently under-fetched, surfacing as a spurious "not in this project"
#: error on a payload that is actually valid.
EDGE_REF_KEYS = ("blocker", "blocked")


def pair_refs(pairs: list) -> list:
    """Every issue reference in an ``edges`` payload, in order.

    Skips a malformed pair for the same reason :func:`referenced_numbers` skips
    a malformed reference — :func:`place_edges` owns that error.
    """
    return [
        pair[key]
        for pair in pairs
        if isinstance(pair, dict)
        for key in EDGE_REF_KEYS
        if key in pair
    ]


_RELATION_MUTATION = """
mutation($input: IssueRelationCreateInput!) {
  issueRelationCreate(input: $input) { success }
}
"""

# Removal needs the relation's own id, which the pair does not carry — so the
# edge is looked up on the blocker first. Selecting only what identifies the
# relation keeps this as cheap as the rest of the tool.
_RELATIONS_QUERY = """
query($id: String!) {
  issue(id: $id) {
    relations {
      nodes { id type relatedIssue { id identifier } }
    }
  }
}
"""

_RELATION_DELETE = """
mutation($id: String!) {
  issueRelationDelete(id: $id) { success }
}
"""


def find_relation_id(
    api_key: str, blocker: dict, blocked: dict, relation_type: str = "blocks"
) -> str | None:
    """The id of the ``blocker -> blocked`` relation, or ``None`` if absent."""
    data = _post(api_key, _RELATIONS_QUERY, {"id": blocker["id"]})
    nodes = ((data.get("issue") or {}).get("relations") or {}).get("nodes") or []
    for node in nodes:
        related = node.get("relatedIssue") or {}
        if node.get("type") == relation_type and related.get("id") == blocked["id"]:
            return node.get("id")
    return None


def place_edges(
    api_key: str,
    pairs: list,
    by_number: dict[int, dict],
    *,
    dry_run: bool = False,
    remove: bool = False,
    emit=None,
) -> list[str]:
    """Add (or, with ``remove``, delete) operator-directed ``blocks`` edges.

    Refuses an empty list on purpose. This subcommand has no discovery mode and
    must never be handed a list it computed itself: an edge that nobody decided
    is exactly the spurious edge the human-curated rule exists to prevent.
    Removal is the same policy in reverse — a human decided the edge should go.
    """
    if not pairs:
        raise BoardBatchError(
            "edges refuses an empty pair list — it executes an operator's "
            "decision and has no discovery mode"
        )
    # Pre-flight the whole list before placing any edge, for the same reason
    # apply_fields does: a bad pair halfway down must not leave the earlier
    # edges placed. A half-applied set of blocking edges is worse than none —
    # it silently drops issues out of the available set.
    planned = []
    for pair in pairs:
        if not isinstance(pair, dict) or any(k not in pair for k in EDGE_REF_KEYS):
            raise BoardBatchError(
                f"each pair needs 'blocker' and 'blocked' keys, got {pair!r}"
            )
        blocker = resolve_issue(pair["blocker"], by_number, "blocker")
        blocked = resolve_issue(pair["blocked"], by_number, "blocked")
        if blocker["id"] == blocked["id"]:
            raise BoardBatchError(f"{blocker['identifier']} cannot block itself")
        planned.append((blocker, blocked))

    emit = print if emit is None else emit
    lines = []
    for blocker, blocked in planned:
        arrow = f"{blocker['identifier']} blocks {blocked['identifier']}"
        if dry_run:
            line = f"WOULD {'UNLINK' if remove else 'LINK'} {arrow}"
        elif remove:
            relation_id = find_relation_id(api_key, blocker, blocked)
            if relation_id is None:
                # Report rather than raise: removing an edge that is already
                # gone is the operator's intended end state either way, and
                # aborting would strand the rest of the batch.
                line = f"ABSENT {arrow} (no such edge — nothing removed)"
            else:
                data = _post(api_key, _RELATION_DELETE, {"id": relation_id})
                if not (data.get("issueRelationDelete") or {}).get("success"):
                    raise BoardBatchError(
                        f"{arrow}: issueRelationDelete reported failure"
                    )
                line = f"UNLINKED {arrow}"
        else:
            data = _post(
                api_key,
                _RELATION_MUTATION,
                {
                    "input": {
                        "issueId": blocker["id"],
                        "relatedIssueId": blocked["id"],
                        "type": "blocks",
                    }
                },
            )
            if not (data.get("issueRelationCreate") or {}).get("success"):
                raise BoardBatchError(f"{arrow}: issueRelationCreate reported failure")
            line = f"LINKED {arrow}"
        emit(line)
        lines.append(line)
    return lines


def load_json_file(path: str) -> object:
    try:
        with open(path, "r", encoding="utf-8") as handle:
            return json.load(handle)
    except FileNotFoundError as e:
        raise BoardBatchError(f"no such file: {path}") from e
    except (OSError, json.JSONDecodeError) as e:
        raise BoardBatchError(f"cannot read {path}: {e}") from e


def _add_dry_run(parser: argparse.ArgumentParser, *, top_level: bool) -> None:
    """``--dry-run``, registered on the top level *and* on each write
    subcommand so it is accepted in either position.

    Registering it only on the top-level parser made
    ``edges --pairs f --dry-run`` — the form anyone would type — exit 2 on an
    unrecognized argument. For the one flag whose entire job is to rehearse a
    destructive write safely, failing on the natural spelling is the wrong
    default.

    The subcommand copies default to ``SUPPRESS`` rather than ``False``: a
    subparser writes its defaults into the SAME namespace after the top-level
    parse, so a plain ``False`` default there would silently overwrite
    ``--dry-run edges …`` back to a live run — turning the rehearsal flag into
    a no-op exactly when it was passed correctly.
    """
    parser.add_argument(
        "--dry-run",
        action="store_true",
        default=False if top_level else argparse.SUPPRESS,
        help="print what would be written without writing it",
    )


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(prog="board_batch.py")
    _add_dry_run(parser, top_level=True)
    sub = parser.add_subparsers(dest="cmd", required=True)

    listing = sub.add_parser("list", help="compact board listing")
    listing.add_argument(
        "--state",
        default="Backlog",
        help="workflow state to list (default Backlog); 'all' for every state",
    )
    listing.add_argument(
        "--include-milestoned",
        action="store_true",
        help="include parked (milestoned) issues, which are dropped by default",
    )
    listing.add_argument(
        "--show-milestone",
        action="store_true",
        help="append each issue's milestone name",
    )

    for name, help_text in (
        ("fields", "batch non-body issue-field updates"),
        ("priorities", "batch priority updates (a thin alias of fields)"),
    ):
        p = sub.add_parser(name, help=help_text)
        p.add_argument("--updates", required=True, help="path to the updates JSON")
        _add_dry_run(p, top_level=False)

    state_cmd = sub.add_parser(
        "state",
        help="transition ONE issue's workflow state (a thin alias of fields)",
    )
    state_cmd.add_argument("--id", required=True, help="ENG-### identifier")
    state_cmd.add_argument("--state", required=True, help="target workflow state name")
    _add_dry_run(state_cmd, top_level=False)

    edges = sub.add_parser(
        "edges", help="add or remove operator-directed blocking edges"
    )
    edges.add_argument("--pairs", required=True, help="path to the pair-list JSON")
    edges.add_argument(
        "--remove",
        action="store_true",
        help="delete the named edges instead of creating them",
    )
    _add_dry_run(edges, top_level=False)

    return parser.parse_args(argv[1:])


def _normalize_priority_updates(raw: object) -> dict:
    """`priorities` takes a flat ``{number: priority}`` map; widen it to the
    ``{number: {field: value}}`` shape `fields` applies."""
    if not isinstance(raw, dict):
        raise BoardBatchError("priorities expects an object of number -> priority")
    return {number: {"priority": value} for number, value in raw.items()}


def _normalize_state_update(identifier: str, state: str) -> dict:
    """``state`` takes one ``ENG-###`` and a state name; widen it to the
    ``{ref: {field: value}}`` shape `fields` applies.

    It exists because the per-session lifecycle transitions — `init-pr`'s move
    to In Progress at bootstrap, `review-pr`'s to In Review at the merge-queue
    handoff — are single-issue, single-enum writes, and routing them through
    `fields` meant composing a JSON file to carry one value. Through the MCP
    those writes echo the entire stored body back: measured at ~3.6k to
    transmit one enum, on a session where the Linear writes cost roughly three
    times every shell command combined.

    The identifier is passed straight through as the key rather than parsed to
    an int, so it inherits :func:`_as_ref`'s team-prefix validation — a
    ``FIN-123`` cannot silently resolve to ``ENG-123``.
    """
    return {identifier: {"state": state}}


def run(argv: list[str]) -> int:
    args = _parse_args(argv)
    api_key = env_var("LINEAR_API_KEY")
    project_id = env_var("LINEAR_PROJECT_ID")

    if args.cmd == "list":
        states = None if args.state == "all" else [args.state]
        issues = fetch_issues(api_key, project_id, states)
        if not args.include_milestoned:
            issues = drop_milestoned(issues)
        lines = format_listing(issues, show_milestone=args.show_milestone)
        for line in lines:
            print(line)
        print(f"-- {len(lines)} issue(s)", file=sys.stderr)
        return 0

    # Every write path loads its payload FIRST, then resolves numbers to ids from
    # a read scoped to what that payload names — so a write's cost tracks the
    # handful of issues it touches rather than the size of the board.
    if args.cmd in ("fields", "priorities", "state"):
        if args.cmd == "state":
            updates = _normalize_state_update(args.id, args.state)
        else:
            raw = load_json_file(args.updates)
            updates = (
                _normalize_priority_updates(raw) if args.cmd == "priorities" else raw
            )
        if not isinstance(updates, dict):
            raise BoardBatchError("fields expects an object of number -> fields")
        if not updates:
            raise BoardBatchError("nothing to update — the updates file is empty")
        by_number = index_by_number(
            fetch_issues_by_number(api_key, project_id, referenced_numbers(updates))
        )
        # apply_fields reports each write as it lands (see its `emit`), so the
        # caller must not re-print the returned lines.
        apply_fields(api_key, updates, by_number, dry_run=args.dry_run)
        return 0

    pairs = load_json_file(args.pairs)
    if not isinstance(pairs, list):
        raise BoardBatchError("edges expects a list of {blocker, blocked} objects")
    by_number = index_by_number(
        fetch_issues_by_number(
            api_key, project_id, referenced_numbers(pair_refs(pairs))
        )
    )
    place_edges(api_key, pairs, by_number, dry_run=args.dry_run, remove=args.remove)
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except BoardBatchError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
