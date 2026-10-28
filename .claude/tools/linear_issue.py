#!/usr/bin/env python3
"""Zero-echo Linear issue BODY operations: append, and a title-only search.

**Why this exists.** The Linear MCP echoes the *entire stored issue body* on
every write — including a write that sends no body at all. The cost is fixed
per call, ``patch`` does not shrink it, and it scales with the body as stored,
so it compounds on an issue that accumulates. Measured across sessions that
were each fully compliant with the per-call conventions:

===========  =====================  =====================================
session      writes                 approx cost
===========  =====================  =====================================
a684162a     7 ``save_issue``       20.5k — top sink; ~3.6k each
17a8f772     6 ``save_issue``       6.7k — five of the eight largest
90b46d55     6 ``save_issue``       19.7k — five of the six largest
9e685c03     6 appends, one issue   echoes growing to 6.2k, 6.9k, 7.7k
===========  =====================  =====================================

Every one of those sessions followed the rules: bodies fetched once, ticks
folded into one call, ``patch`` used for partial edits. The cost is structural,
so it wants a tool rather than more discipline.

**The mechanism**, and the carve-out ``trim_levers.py`` already holds and
``docs/conventions/linear-automation.md`` already licenses: do the
read-modify-write *inside this process*, so the stored body is fetched, grown
and sent back without ever reaching a transcript. An ``append`` enlarges the
body, so through the MCP each append echoes everything the previous ones added
— the cost is quadratic in the number of appends, not linear. Batching is not
always available, because appends are often genuinely separated in time.

**Scope boundary — this tool does BODIES.** Non-body field writes, the
workflow state included, belong to ``board_batch.py``, which already resolves
a state name to its UUID and validates the team prefix::

    board_batch.py state --id ENG-123 --state "In Review"

Splitting on that line is deliberate: a second implementation of the state
resolution would be a place for the two to disagree. The one exception is
``tick``, whose state move must ride the **same** write as its ticks — a tick
that landed without its state move, or the reverse, is the half-applied save
the MCP's atomic ``patch`` existed to prevent — and which resolves the name
with the ``create`` path's resolver, not a third one.

Subcommands::

    linear_issue.py append --id ENG-123 --file notes.md
    linear_issue.py tick   --id ENG-123 --box "Part 1 text" --box "Part 2 text"
    linear_issue.py tick   --id ENG-123 --box-file boxes.txt   # one per line
    linear_issue.py find   --query "reference price" --limit 5
    linear_issue.py create --title "…" --body-file body.md \
        --state Todo --milestone "Audit findings"
    linear_issue.py create --title "…" --body-file body.md \
        --verb chore --substrate anthropic     # → "[Chore/Anthropic] …"

``create`` is the filer for the **bulk** callers, whose bodies are long by
design because the fold convention asks for a part and a fingerprint per
finding — which is exactly where the echo is worst. One audit session's nine
``save_issue`` calls cost ≈22.8k and supplied six of its eight largest single
results; a planning session absorbing follow-ups made 27 for ≈30.1k. State,
milestone and priority are parameters so one filer serves `audit-scope`
(``Todo`` + ``Audit findings``), a planning follow-up (Backlog + a priority),
and anything else that files in bulk.

Each prints ONE line of accounting on success; ``find`` prints its rows.
``--dry-run`` reports what would happen and writes nothing.
"""

from __future__ import annotations

import argparse
import re
import sys

import linear_api

#: Cap on a ``find`` page. A title-only projection is cheap in a way the MCP's
#: full issue objects never are, but a cap still beats an unbounded scan.
MAX_FIND = 100

#: The verb half of a ``[<Verb>/<Substrate>]`` title token, lowercased input to
#: canonical spelling. ``Operator`` is the one verb with no substrate half.
TOKEN_VERBS = {
    "task": "Task",
    "chore": "Chore",
    "explore": "Explore",
    "architect": "Architect",
    "operator": "Operator",
}
TOKEN_SUBSTRATES = {"bedrock": "Bedrock", "anthropic": "Anthropic"}


class LinearIssueError(Exception):
    """A failure worth one clean stderr line, never a traceback."""


def title_token(verb: str | None, substrate: str | None) -> str:
    """The ``[<Verb>/<Substrate>]`` token a title leads with, or ``""``.

    A token is always **whole**: a half-token would silently change meaning
    the day a verb's default substrate flips, so one of the pair without the
    other is refused. The one elision is ``[Task/Bedrock]``, the default of
    defaults, which is never written — the caller omits both flags instead.
    ``[Operator]`` marks work with no agent, so it takes no substrate.
    """
    if verb is None and substrate is None:
        return ""
    canon_verb = TOKEN_VERBS.get((verb or "").strip().lower())
    if verb is not None and canon_verb is None:
        raise LinearIssueError(
            f"unknown --verb {verb!r} — have: {', '.join(TOKEN_VERBS.values())}"
        )
    canon_sub = TOKEN_SUBSTRATES.get((substrate or "").strip().lower())
    if substrate is not None and canon_sub is None:
        raise LinearIssueError(
            f"unknown --substrate {substrate!r} — have: "
            f"{', '.join(TOKEN_SUBSTRATES.values())}"
        )
    if canon_verb == "Operator":
        if canon_sub is not None:
            raise LinearIssueError(
                "--verb operator takes no --substrate: [Operator] has no agent"
            )
        return "[Operator]"
    if canon_verb is None or canon_sub is None:
        raise LinearIssueError(
            "--verb and --substrate go together: a token is always whole"
        )
    if (canon_verb, canon_sub) == ("Task", "Bedrock"):
        raise LinearIssueError(
            "[Task/Bedrock] is the default and is never written — omit both "
            "--verb and --substrate"
        )
    return f"[{canon_verb}/{canon_sub}]"


def _post(api_key: str, query: str, variables: dict) -> dict:
    return linear_api.post(api_key, query, variables, error=LinearIssueError)


_BODY_QUERY = """
query IssueBody($id: String!) {
  issue(id: $id) { id identifier url description }
}
"""

_BODY_MUTATION = """
mutation SetBody($id: String!, $description: String!) {
  issueUpdate(id: $id, input: { description: $description }) {
    success
    issue { identifier url }
  }
}
"""

_FIND_QUERY = """
query FindIssues($filter: IssueFilter, $first: Int!) {
  issues(filter: $filter, first: $first) {
    nodes { identifier title }
  }
}
"""


def fetch_issue_body(api_key: str, identifier: str) -> dict:
    """The stored body, fetched into this process and never printed.

    ``issue(id:)`` accepts the human ``ENG-###`` identifier as well as the
    UUID, which is what lets a session pass the tag it already knows instead of
    resolving one first.
    """
    data = _post(api_key, _BODY_QUERY, {"id": identifier})
    issue = data.get("issue") or {}
    if not issue.get("id"):
        raise LinearIssueError(f"no issue {identifier}")
    return issue


def append_body(api_key: str, identifier: str, text: str, *, dry_run: bool) -> str:
    """Append to an issue body without the stored body reaching a transcript."""
    addition = text.strip("\n")
    if not addition.strip():
        raise LinearIssueError("refusing to append empty text")

    if dry_run:
        return f"WOULD APPEND to {identifier} | {len(addition)} char(s)"

    issue = fetch_issue_body(api_key, identifier)
    stored = issue.get("description") or ""
    # One blank line between the stored body and the addition, never two, and
    # none at all when the issue was filed without a body.
    body = f"{stored.rstrip()}\n\n{addition}\n" if stored.strip() else f"{addition}\n"

    data = _post(api_key, _BODY_MUTATION, {"id": issue["id"], "description": body})
    result = data.get("issueUpdate") or {}
    if not result.get("success"):
        raise LinearIssueError(f"the body write for {identifier} reported failure")
    return (
        f"APPENDED to {issue['identifier']} | {len(addition)} char(s) | "
        f"body now {len(body)} char(s)"
    )


_TICK_QUERY = """
query TickIssue($id: String!) {
  issue(id: $id) {
    id identifier description
    state { name type }
    team { states { nodes { id name } } }
  }
}
"""

_TICK_MUTATION = """
mutation Tick($id: String!, $input: IssueUpdateInput!) {
  issueUpdate(id: $id, input: $input) { success }
}
"""


class _WriteFailed(LinearIssueError):
    """The tick's write failed — the one failure a re-fetch can cure."""


#: State types a tick never moves the issue out of.
TERMINAL_STATE_TYPES = frozenset({"completed", "canceled"})

_BOX = re.compile(r"^(\s*[-*] \[)([ xX])(\] )(.*)$")
# Linear stores a typed `ENG-###` as a link to the issue; a box is matched on
# its text with that markup folded back to the bare tag, so a tag-bearing box
# needs no special path.
# A `--box` text pasted with its own checkbox marker, in any form.
_BOX_PREFIX = re.compile(r"^\s*[-*] \[[ xX]\] ")
_MENTION_LINK = re.compile(r"\[([A-Z]+-\d+)\]\([^)]*\)")
_MENTION_URL = re.compile(r"https://linear\.app/[^/\s]+/issue/([A-Z]+-\d+)[^\s)]*")


def _normalize(text: str) -> str:
    text = _MENTION_LINK.sub(r"\1", text)
    text = _MENTION_URL.sub(r"\1", text)
    return " ".join(text.split())


def tick_boxes(body: str, boxes: list[str]) -> tuple[str, int, int]:
    """Tick each named checklist box in ``body``.

    Returns ``(new body, ticked, already ticked)``. Matching is per line on the
    box's text, mention markup folded and whitespace collapsed — an exact match
    first, else a prefix. A text names as many boxes as it is passed times, and
    must match exactly that many: more matches is ambiguous, fewer is a box
    that is not there, and two texts may not claim one box. A prefix counts
    ticked boxes too, so ``Part 1`` never lands on ``Part 12`` because the
    ``Part 1`` box was already ticked. Every box is resolved before any is
    ticked, so a refusal leaves the body untouched.
    """
    lines = body.split("\n")
    parsed = [(i, _BOX.match(line)) for i, line in enumerate(lines)]
    found = [(i, m) for i, m in parsed if m]

    wanted: dict[str, int] = {}
    for box in boxes:
        key = _normalize(_BOX_PREFIX.sub("", box))
        if not key:
            raise LinearIssueError("refusing to tick a blank box text")
        wanted[key] = wanted.get(key, 0) + 1

    to_tick: list[int] = []
    claimed: dict[int, str] = {}
    already = 0
    for key, count in wanted.items():
        hits = [(i, m) for i, m in found if _normalize(m.group(4)) == key]
        if not hits:
            hits = [(i, m) for i, m in found if _normalize(m.group(4)).startswith(key)]
        if not hits:
            raise LinearIssueError(f"no checklist box reads {key!r}")
        if len(hits) != count:
            raise LinearIssueError(
                f"{len(hits)} box(es) read {key!r} but it was named {count} "
                "time(s) — name it once per box, or quote more of it"
            )
        for i, _ in hits:
            if i in claimed:
                raise LinearIssueError(
                    f"{key!r} and {claimed[i]!r} both name one box — quote more of it"
                )
            claimed[i] = key
        open_hits = [i for i, m in hits if m.group(2) == " "]
        already += len(hits) - len(open_hits)
        to_tick.extend(open_hits)

    for i in to_tick:
        m = _BOX.match(lines[i])
        lines[i] = f"{m.group(1)}x{m.group(3)}{m.group(4)}"
    return "\n".join(lines), len(to_tick), already


def _tick_once(
    api_key: str, identifier: str, boxes: list[str], state: str | None, dry_run: bool
) -> str:
    data = _post(api_key, _TICK_QUERY, {"id": identifier})
    issue = data.get("issue") or {}
    if not issue.get("id"):
        raise LinearIssueError(f"no issue {identifier}")
    stored = issue.get("description") or ""
    body, ticked, already = tick_boxes(stored, boxes) if boxes else (stored, 0, 0)

    current = issue.get("state") or {}
    plan = [f"{ticked} box(es) ticked", f"{already} already ticked"]
    update: dict = {}
    if ticked:
        update["description"] = body
    if state and current.get("name", "").lower() != state.lower():
        if current.get("type") in TERMINAL_STATE_TYPES:
            plan.append(f"state left at {current.get('name')} (terminal)")
        else:
            nodes = (((issue.get("team") or {}).get("states")) or {}).get("nodes") or []
            update["stateId"] = _resolve_named(nodes, state, "workflow state")
            plan.append(f"state {current.get('name')} → {state}")
    elif state:
        plan.append(f"state already {current.get('name')}")

    verb = "WOULD TICK" if dry_run else "TICKED"
    if not update:
        return f"NOTHING TO WRITE for {issue['identifier']} | {', '.join(plan)}"
    if dry_run:
        return f"{verb} {issue['identifier']} | {', '.join(plan)}"
    try:
        result = _post(api_key, _TICK_MUTATION, {"id": issue["id"], "input": update})
    except LinearIssueError as exc:
        raise _WriteFailed(str(exc)) from exc
    if not (result.get("issueUpdate") or {}).get("success"):
        raise _WriteFailed(f"the tick write for {identifier} reported failure")
    return f"{verb} {issue['identifier']} | {', '.join(plan)}"


def tick(
    api_key: str,
    identifier: str,
    boxes: list[str],
    *,
    state: str | None = "In Progress",
    dry_run: bool = False,
) -> str:
    """Tick checklist boxes and move the state, in one zero-echo write.

    The body and the state change ride one ``issueUpdate``, so a tick never
    lands without its state move or the reverse. A box that cannot be resolved
    is refused before anything is written. A failed **write** (a transport
    error or ``success: false``) re-fetches once, rebuilds from the fresh body
    and retries once, then stops and leaves the issue alone; a write that
    landed but whose answer was lost re-reads as ``NOTHING TO WRITE``. A
    resolution refusal is not retried: re-reading the body it was just read
    from cannot change it.

    **Concurrency is ``append``'s, not compare-and-swap.** The body is read and
    written back in one call, milliseconds apart, and Linear's API has no
    conditional write — so an edit landing inside that window is overwritten,
    as with ``append``. That window replaces a worse one: the MCP ``patch``
    flow built its ops from a body held since session start. A write that
    moves only the state never sends ``description`` at all.
    """
    try:
        return _tick_once(api_key, identifier, boxes, state, dry_run)
    except _WriteFailed:
        pass
    try:
        return (
            _tick_once(api_key, identifier, boxes, state, dry_run) + " | retried once"
        )
    except _WriteFailed as second:
        raise LinearIssueError(
            f"{second} — failed twice; leaving {identifier} alone"
        ) from second


def find(
    api_key: str,
    *,
    query: str,
    project_id: str | None,
    state: str | None,
    limit: int,
) -> list[str]:
    """``identifier + title`` lines, for a dedup probe.

    The MCP has no title-only projection, so a *compliant* dedup probe — a
    distinctive phrase, a project and state filter, ``limit: 5`` — still cost
    ~1.9k: five complete issue objects, each carrying a truncated description
    plus the full scalar set, to answer a question the five titles answered on
    their own. ``limit`` bounds the row count and nothing bounds the row width,
    so ~1.9k was the floor for a careful call rather than the price of a
    careless one.
    """
    issue_filter: dict = {"title": {"containsIgnoreCase": query}}
    if project_id:
        issue_filter["project"] = {"id": {"eq": project_id}}
    if state:
        issue_filter["state"] = {"name": {"eq": state}}

    # Floored as well as capped. `--limit 0` sent `first: 0` and a negative sent
    # a negative, both of which come back as a raw GraphQL error rather than the
    # single clean stderr line this module promises.
    data = _post(
        api_key,
        _FIND_QUERY,
        {"filter": issue_filter, "first": max(1, min(limit, MAX_FIND))},
    )
    nodes = ((data.get("issues") or {}).get("nodes")) or []
    return [f"{n.get('identifier')}  {n.get('title')}" for n in nodes]


_CREATE_MUTATION = """
mutation Create($input: IssueCreateInput!) {
  issueCreate(input: $input) {
    success
    issue { identifier url }
  }
}
"""

_STATES_QUERY = """
query States($teamId: String!) {
  team(id: $teamId) { states { nodes { id name } } }
}
"""

_MILESTONES_QUERY = """
query Milestones($projectId: String!) {
  project(id: $projectId) { projectMilestones { nodes { id name } } }
}
"""


def _resolve_named(nodes: list, wanted: str, kind: str) -> str:
    """An id for ``wanted``, matched case-insensitively by name.

    An **ambiguous** name is refused rather than silently resolved to the first
    hit. Linear does not enforce unique state or milestone names, and the
    filing skills address both by name — so two milestones called
    `Audit findings` (a stale one and a live one, say) would have filed every
    parked finding into whichever the API happened to return first, with no
    signal at all. Refusing costs one clear error; guessing costs a milestone's
    worth of misfiled work that only surfaces when a sweep comes up short.
    """
    lowered = wanted.strip().lower()
    matches = [
        node for node in nodes if str(node.get("name", "")).strip().lower() == lowered
    ]
    if len(matches) > 1:
        ids = ", ".join(sorted(str(node.get("id")) for node in matches))
        raise LinearIssueError(
            f"{len(matches)} {kind}s are named {wanted!r} ({ids}) — resolve the "
            "duplicate in Linear, or pass an id, rather than letting this pick one"
        )
    if matches:
        return str(matches[0]["id"])
    names = ", ".join(sorted(str(n.get("name")) for n in nodes)) or "(none)"
    raise LinearIssueError(f"no {kind} named {wanted!r} — have: {names}")


# A filing naming state Backlog and no milestone enters the curator's review
# pool, and the curator learns of it by message, never by polling the board.
# Every filing skill passes --state, so an omitted one is not guessed at. The
# tool only reminds; messaging is the filing session's act.
CURATOR_REMINDER = (
    "REMINDER: message the curator (curate-<day>, via ListAgents) this "
    "filing; with none live, message the scheduler (plan-<day>)"
)


def create_issue(
    api_key: str,
    *,
    team_id: str,
    project_id: str,
    title: str,
    body: str,
    state: str | None = None,
    milestone: str | None = None,
    priority: int | None = None,
    assignee_id: str | None = None,
    verb: str | None = None,
    substrate: str | None = None,
    dry_run: bool = False,
) -> str:
    """File one issue in a single zero-echo call, and return one line.

    **Why a filer belongs here.** The MCP `save_issue` echoes the whole stored
    body back on a *create* too, which makes it worst for the bulk filers — the
    ones whose bodies are long by design because the fold convention asks for a
    part and a fingerprint per finding. Measured: an audit session's 9
    `save_issue` calls cost ≈22.8k and supplied six of its eight largest single
    results, roughly half of all main-loop result cost; a planning session
    absorbing PR follow-ups made 27 calls for ≈30.1k, its top sink, against ≈7k
    for all 26 planning-document writes combined. In every case the echoed
    bytes were content the session had **just authored**, so nothing
    decision-relevant is lost by suppressing them.

    State, milestone and priority are **parameters** rather than constants
    precisely so one filer serves every bulk caller: `audit-scope` files
    `Todo` + `Audit findings`, a planning follow-up files Backlog + a
    priority, and `trim_levers.py` keeps its own lever-specific writer for its
    fingerprint lifecycle. All of them go in the **creating** call — filing and
    then amending costs a second full echo and buys nothing.

    Names, not ids, for state and milestone: both are resolved here, which is
    the one thing `board_batch.py fields` deliberately refuses to do for
    `milestone`. That refusal is right for a *field write* (a name reaching
    Linear dies as an unnamed validation error), and a filer that cannot name
    its own milestone would just push the lookup back into the transcript.

    ``verb`` / ``substrate`` prefix the title with its launch-line token
    (`title_token`), built here so the token and the filing stay consistent by
    construction, as the ``Claude:`` prefix does.
    """
    if not title.strip():
        raise LinearIssueError("--title must not be blank")
    if not body.strip():
        raise LinearIssueError("--body-file must not be empty")
    token = title_token(verb, substrate)
    if token:
        title = f"{token} {title.strip()}"

    plan = [f"{len(body)} char(s)"]
    if state:
        plan.append(f"state {state}")
    if milestone:
        plan.append(f"milestone {milestone}")
    if priority is not None:
        plan.append(f"priority {priority}")
    if dry_run:
        return f"WOULD FILE {title} | {', '.join(plan)}"

    payload: dict = {
        "teamId": team_id,
        "projectId": project_id,
        "title": title,
        "description": body,
    }
    if state:
        data = _post(api_key, _STATES_QUERY, {"teamId": team_id})
        nodes = (((data.get("team") or {}).get("states")) or {}).get("nodes") or []
        payload["stateId"] = _resolve_named(nodes, state, "workflow state")
    if milestone:
        data = _post(api_key, _MILESTONES_QUERY, {"projectId": project_id})
        nodes = (((data.get("project") or {}).get("projectMilestones")) or {}).get(
            "nodes"
        ) or []
        payload["projectMilestoneId"] = _resolve_named(nodes, milestone, "milestone")
    if priority is not None:
        payload["priority"] = priority
    if assignee_id:
        payload["assigneeId"] = assignee_id

    data = _post(api_key, _CREATE_MUTATION, {"input": payload})
    result = data.get("issueCreate") or {}
    if not result.get("success"):
        raise LinearIssueError(f"issueCreate failed for {title!r}")
    issue = result.get("issue") or {}
    line = f"FILED {issue.get('identifier')} {issue.get('url')} | {', '.join(plan)}"
    if (state or "").strip().lower() == "backlog" and not milestone:
        line += "\n" + CURATOR_REMINDER
    return line


def _read_file(path: str) -> str:
    try:
        with open(path, encoding="utf-8") as handle:
            return handle.read()
    except OSError as exc:
        raise LinearIssueError(f"could not read {path}: {exc}") from exc


def _add_dry_run(parser: argparse.ArgumentParser, *, top_level: bool) -> None:
    """``--dry-run``, registered on the top level *and* on the write subcommand
    so it is accepted in either position.

    Registered only on the top-level parser, ``append --id X --text y
    --dry-run`` — the form this module's own Usage block teaches — exits 2 with
    "unrecognized arguments". For the one flag whose entire job is to rehearse a
    write safely, failing on the natural spelling is the wrong default.

    The subcommand copies default to ``SUPPRESS`` rather than ``False``: a
    subparser writes its defaults into the SAME namespace after the top-level
    parse, so a plain ``False`` there would silently overwrite
    ``--dry-run append …`` back into a live write — turning the rehearsal flag
    into a no-op exactly when it was passed correctly.

    This mirrors ``board_batch.py`` deliberately. The two tools disagreeing on
    where a flag of the same name and meaning may sit is its own defect.
    """
    parser.add_argument(
        "--dry-run",
        action="store_true",
        default=False if top_level else argparse.SUPPRESS,
        help="report what would happen and write nothing",
    )


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="linear_issue.py",
        description="Zero-echo Linear issue body append, checklist tick, filing "
        "and title-only search.",
    )
    _add_dry_run(parser, top_level=True)
    subs = parser.add_subparsers(dest="command", required=True)

    append = subs.add_parser("append", help="append to an issue body")
    _add_dry_run(append, top_level=False)
    append.add_argument("--id", required=True, help="ENG-### identifier")
    source = append.add_mutually_exclusive_group(required=True)
    source.add_argument("--file", help="read the addition from this file")
    source.add_argument("--text", help="the addition, inline")

    tick_cmd = subs.add_parser(
        "tick", help="tick checklist boxes and move the state, zero echo"
    )
    _add_dry_run(tick_cmd, top_level=False)
    tick_cmd.add_argument("--id", required=True, help="ENG-### identifier")
    tick_cmd.add_argument(
        "--box",
        action="append",
        default=[],
        help="a box's text (repeatable; name a duplicate box once per tick)",
    )
    tick_cmd.add_argument(
        "--box-file",
        default=None,
        help="read box texts from this file, one per line (blank lines skipped) "
        "— for a box carrying a backtick the shell guard refuses inline",
    )
    tick_cmd.add_argument(
        "--state",
        default="In Progress",
        help="target state NAME (default In Progress; never leaves Done/Canceled)",
    )
    tick_cmd.add_argument(
        "--keep-state", action="store_true", help="tick only; write no state"
    )

    create = subs.add_parser("create", help="file one issue, zero echo")
    _add_dry_run(create, top_level=False)
    create.add_argument("--title", required=True)
    create.add_argument("--body-file", required=True, help="the issue description")
    create.add_argument(
        "--team", default=None, help="team id (default $LINEAR_TEAM_ID)"
    )
    create.add_argument(
        "--project", default=None, help="project id (default $LINEAR_PROJECT_ID)"
    )
    create.add_argument("--state", default=None, help="workflow state NAME")
    create.add_argument("--milestone", default=None, help="project milestone NAME")
    create.add_argument("--priority", type=int, default=None, help="0-4")
    create.add_argument(
        "--assignee", default=None, help="assignee id (default $LINEAR_ASSIGNEE_ID)"
    )
    create.add_argument(
        "--verb",
        default=None,
        help="title-token verb: task, chore, explore, architect, operator",
    )
    create.add_argument(
        "--substrate",
        default=None,
        help="title-token substrate: bedrock, anthropic (omit with operator)",
    )

    find_cmd = subs.add_parser("find", help="title-only search")
    find_cmd.add_argument("--query", required=True, help="text to match in titles")
    find_cmd.add_argument("--project", help="restrict to this project id")
    find_cmd.add_argument("--state", help="restrict to this workflow state name")
    find_cmd.add_argument(
        "--limit", type=int, default=10, help=f"max rows (cap {MAX_FIND})"
    )

    return parser.parse_args(argv[1:])


def run(argv: list[str]) -> int:
    args = _parse_args(argv)
    api_key = linear_api.env_var("LINEAR_API_KEY", error=LinearIssueError)

    if args.command == "append":
        text = args.text if args.text is not None else _read_file(args.file)
        print(append_body(api_key, args.id, text, dry_run=args.dry_run))
        return 0

    if args.command == "tick":
        state = None if args.keep_state else args.state
        boxes = list(args.box)
        if args.box_file is not None:
            boxes += [ln for ln in _read_file(args.box_file).splitlines() if ln.strip()]
        print(tick(api_key, args.id, boxes, state=state, dry_run=args.dry_run))
        return 0

    if args.command == "create":
        # Resolved from the environment by default, per the standing rule that
        # team / project / assignee are never hard-coded — and each via its own
        # bare lookup, since a combined `printenv A B C` returns only the first
        # on macOS.
        team = args.team or linear_api.env_var("LINEAR_TEAM_ID", error=LinearIssueError)
        project = args.project or linear_api.env_var(
            "LINEAR_PROJECT_ID", error=LinearIssueError
        )
        assignee = args.assignee
        if assignee is None:
            try:
                assignee = linear_api.env_var(
                    "LINEAR_ASSIGNEE_ID", error=LinearIssueError
                )
            except LinearIssueError:
                assignee = None  # Unassigned is a legitimate filing.
        print(
            create_issue(
                api_key,
                team_id=team,
                project_id=project,
                title=args.title,
                body=_read_file(args.body_file),
                state=args.state,
                milestone=args.milestone,
                priority=args.priority,
                assignee_id=assignee,
                verb=args.verb,
                substrate=args.substrate,
                dry_run=args.dry_run,
            )
        )
        return 0

    rows = find(
        api_key,
        query=args.query,
        project_id=args.project,
        state=args.state,
        limit=args.limit,
    )
    for row in rows:
        print(row)
    # The count goes to stderr so a caller can consume the rows alone, and an
    # empty result says so rather than looking like a dropped payload.
    print(f"linear-issue | {len(rows)} match(es)", file=sys.stderr)
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except LinearIssueError as exc:
        print(f"linear-issue: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
