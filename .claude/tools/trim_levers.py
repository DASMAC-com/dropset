#!/usr/bin/env python3
"""File and fold trim levers as parked Linear issues, without paying a body echo.

The producer/consumer pipeline for session trim levers used to run through a
Linear **document** — ``session-metrics`` appended an entry per session,
``trim-context`` mined the document later. That document outgrew the harness's
tool-result cap between drains (67.0k characters at the last one), so each mining
pass spilled it to disk and picked it apart with a hand-written scratchpad script.
With roughly ten parallel sessions a day it crossed the cap between any two
drains, which made the growth structural rather than a tidiness problem.

The ratified replacement, which this tool implements: **one parked issue per
lever**, keyed by its ``**Fingerprint**:``. A recurring lever accumulates evidence
on the issue that already exists, so cross-session recurrence becomes a fact on
the board instead of a pattern a miner has to re-detect in prose, and the
milestone lifecycle is the state machine — no drain bookkeeping survives.

**Why this is a tool and not MCP calls.** ``save_issue`` echoes the entire stored
body back on every write, even a write that sent no body at all. That is a fixed
cost per call which ``patch`` does not reduce, and it *compounds* on an
accumulator: five touches on one issue measured ~53k, with per-touch cost rising
monotonically because each append enlarged what the next would echo. So every
write here goes through raw GraphQL and prints **one line** — identifier and url.
``append-evidence`` does its read-modify-write entirely inside this process, so
the grown body never enters a transcript at all.

``docs/conventions/linear-automation.md`` deliberately keeps body edits on the MCP
``patch`` path; that rule governs interactive filing and planning flows, where a
human is reading along. This is a high-volume automated pipeline, where the echo
is pure waste — the doc states the carve-out explicitly.

Subcommands::

    # Does a lever already exist? Titles and urls only — never a body.
    python3 .claude/tools/trim_levers.py probe --fingerprint session-metrics:foo

    # File a new parked lever (milestone and state set in the CREATING call).
    python3 .claude/tools/trim_levers.py file \\
        --title 'Narrow a search by scope, not only output form' \\
        --fingerprint session-metrics:search-scope-axis \\
        --touches 'docs/conventions/context-economy.md' \\
        --body-file <scratchpad>/lever.md

    # Same lever seen again: append this session's evidence to the existing one.
    python3 .claude/tools/trim_levers.py append-evidence \\
        --fingerprint session-metrics:search-scope-axis \\
        --evidence-file <scratchpad>/evidence.md

    # The fold: what is parked right now, as one compact listing.
    python3 .claude/tools/trim_levers.py list

Every subcommand takes ``--dry-run``. Reads ``LINEAR_API_KEY``,
``LINEAR_PROJECT_ID``, ``LINEAR_TEAM_ID`` and (for ``file``)
``LINEAR_ASSIGNEE_ID`` from the environment — never a hard-coded UUID, per
``CLAUDE.md`` → "Linear automation".

**It writes no relations, ever.** A parked lever is not in the pull queue and is
exempt from the meta batch and its edge until it is folded; blocking edges are
human-curated in a planning session. Stdlib only; a Python skill-tool under
``.claude/tools/`` — deliberately **not** a Cargo workspace member. Tests live in
``tests/test_trim_levers.py``, run via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import os
import re
import sys

import linear_api

ENDPOINT = linear_api.ENDPOINT

# Overall per-request timeout, so a hung endpoint can't wedge a run.
REQUEST_TIMEOUT = 30

# The parking milestone. Deliberately distinct from "Audit findings" so the
# planning bootstrap's audit-promotion offer stays audit-scoped and does not start
# sweeping up trim levers.
MILESTONE_NAME = "Trim levers"

# Parked findings sit in Todo, never Backlog: Backlog means pullable, and the
# operator's Next view is the unblocked Backlog, so a parked lever there would
# surface as available work. Promotion in a planning session is what moves a lever
# Todo -> Backlog and clears the milestone.
PARKED_STATE = "Todo"

# One page of a listing read. The parked pool is small by construction — it drains
# through folds — but the read follows the cursor anyway, because the sibling
# board tool shipped a one-page guard and every write on it failed the day the
# project crossed that size.
PAGE_SIZE = 100

# Runaway backstop on a cursor-following read.
MAX_PAGES = 40

# A fingerprint is ``<domain-token>:<slug>``. The domain half must be **dotless**:
# Linear linkifies a hostname-valid basename, which silently rewrites the stored
# key and breaks the dedup probe that is this pipeline's only guard against
# refiling. Roughly 40 stored keys were corrupted this way before the rule was
# written down, so it is enforced here rather than trusted to the caller.
FINGERPRINT_RE = re.compile(r"^[a-z0-9][a-z0-9-]*:[a-z0-9][a-z0-9._/-]*$")

# A fenced code block, opening or closing. Lever bodies quote filing examples, so
# a `**Field**:` line inside a fence is an illustration, not a field — see
# `field_values`. Kept identical in shape to `read_result.py`'s guard.
FENCE_RE = re.compile(r"^\s*(```+|~~~+)")


class TrimLeversError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def env_var(name: str) -> str:
    return linear_api.env_var(name, error=TrimLeversError)


def _post(api_key: str, query: str, variables: dict) -> dict:
    """POST a GraphQL operation and return its ``data``.

    Delegates to the shared transport, which refuses redirects so a 3xx can
    never re-send the ``Authorization`` header to another host. Errors surface
    as :class:`TrimLeversError` so the CLI never emits a traceback (which could
    quote the credential).
    """
    return linear_api.post(
        api_key,
        query,
        variables,
        endpoint=ENDPOINT,
        timeout=REQUEST_TIMEOUT,
        error=TrimLeversError,
    )


# --------------------------------------------------------------------------
# Pure helpers
# --------------------------------------------------------------------------


def validate_fingerprint(key: str) -> str:
    """The fingerprint, normalized, or a hard error naming the rule it broke."""
    key = key.strip()
    if not key:
        raise TrimLeversError("--fingerprint is empty")
    if ":" not in key:
        raise TrimLeversError(
            f"fingerprint {key!r} needs a <domain-token>:<slug> shape"
        )
    domain = key.split(":", 1)[0]
    if "." in domain:
        raise TrimLeversError(
            f"fingerprint domain token {domain!r} contains a dot — Linear "
            "linkifies a hostname-valid basename and corrupts the stored key; "
            "use a dotless domain (e.g. 'feeds-http', not 'http.rs')"
        )
    if not FINGERPRINT_RE.match(key):
        raise TrimLeversError(
            f"fingerprint {key!r} must be lowercase <domain-token>:<slug>"
        )
    return key


def field_line_re(field: str, value: str | None = None) -> re.Pattern:
    """A line-anchored matcher for a ``**Field**: value`` line.

    Anchored, and not a substring test. A substring test gets this wrong in both
    directions, which review caught by running it: a fingerprint merely *mentioned
    in prose* suppressed the real field entirely (leaving the lever with no
    machine-parsed key at all), while filing ``a:foo-bar`` onto a body already
    carrying ``a:foo`` appended a **second** field line — one issue owning two
    keys. Both break the dedup this pipeline rests on.
    """
    tail = re.escape(value) + r"\s*$" if value is not None else r".*$"
    return re.compile(rf"^\*\*{re.escape(field)}\*\*:\s*{tail}", re.MULTILINE)


def field_values(body: str, field: str) -> list[str]:
    """Every value carried by a ``**Field**: value`` line, **outside a fence**.

    Fence-awareness is not decoration. A lever body that *quotes* a filing
    example — a fenced block showing ``**Fingerprint**: <domain>:<slug>`` — would
    otherwise read as carrying a second, foreign key, and `compose_body`'s
    refusal would then reject a perfectly valid body outright. Levers *about
    filing conventions* are exactly what this pipeline produces, so that is a
    likely body rather than a contrived one. (Its sibling parser in
    ``read_result.py`` grew the same guard in the same commit; the two must not
    disagree about what a fence is.)
    """
    pattern = re.compile(rf"^\*\*{re.escape(field)}\*\*:\s*(.*)$")
    out: list[str] = []
    in_fence = False
    for line in body.splitlines():
        if FENCE_RE.match(line):
            in_fence = not in_fence
            continue
        if not in_fence:
            m = pattern.match(line)
            if m:
                out.append(m.group(1))
    return out


def compose_body(body: str, fingerprint: str, touches: list[str]) -> str:
    """The stored body: the lever's prose plus its two machine-parsed fields.

    The fields are appended here rather than expected in the prose so every filed
    lever carries them in the same place and spelling — the probe below is only as
    reliable as that consistency.
    """
    # A single parked lever owns exactly ONE key. If the supplied body already
    # carries a *different* `**Fingerprint**:` field, appending ours would store
    # two — and a probe for either would then match this issue, which is the
    # dedup guard failing in the direction hardest to notice. (An aggregated task
    # legitimately carries many; the fold composes those, not this function.)
    foreign = sorted(
        {
            found.strip()
            for found in field_values(body, "Fingerprint")
            if found.strip() != fingerprint
        }
    )
    if foreign:
        raise TrimLeversError(
            f"the supplied body already carries a different **Fingerprint**: "
            f"field ({', '.join(foreign)}) — a lever owns one key, so refusing "
            f"to file it under {fingerprint} as well"
        )

    parts = [body.rstrip()]
    # Anchored presence test, so the field is added exactly once whether or not
    # the prose happens to mention it.
    if not field_line_re("Fingerprint", fingerprint).search(body):
        parts.append(f"**Fingerprint**: {fingerprint}")
    # `**Touches**:` is retired — `session-metrics` passes no `--touches`, so
    # this branch is dead on the normal path and kept only so an explicit
    # caller (or an old script) still composes a valid body rather than
    # erroring. See `CLAUDE.md` -> "Structured filing fields".
    if touches and not field_line_re("Touches").search(body):
        parts.append(f"**Touches**: {', '.join(touches)}")
    # Joined with a blank line, and never leaving a field directly under a
    # paragraph: a bare "---" or a field abutting prose is how Linear's round trip
    # has re-parsed a paragraph as a setext heading before.
    return "\n\n".join(parts) + "\n"


def split_touches(raw: str | None) -> list[str]:
    """``--touches`` as an ordered, de-duplicated glob list."""
    if not raw:
        return []
    out: list[str] = []
    for chunk in raw.split(","):
        glob = chunk.strip()
        if glob and glob not in out:
            out.append(glob)
    return out


# --------------------------------------------------------------------------
# Linear operations
# --------------------------------------------------------------------------

# Only identity fields are selected anywhere a body is not strictly needed. That
# selection *is* the zero-echo property — it is not an optimization detail.
_SEARCH_QUERY = """
query Levers($filter: IssueFilter, $first: Int!, $after: String) {
  issues(filter: $filter, first: $first, after: $after, includeArchived: true) {
    pageInfo { hasNextPage endCursor }
    nodes {
      identifier
      url
      title
      state { name type }
      projectMilestone { name }
    }
  }
}
"""

# The fold's listing query. Deliberately WITHOUT `includeArchived`: the probe
# needs archived rows so a rejection stays permanent, but the fold must not offer
# an archived lever as parked work — and since the selection carries no
# `archivedAt`, nothing downstream could tell the difference.
_PARKED_QUERY = """
query ParkedLevers($filter: IssueFilter, $first: Int!, $after: String) {
  issues(filter: $filter, first: $first, after: $after) {
    pageInfo { hasNextPage endCursor }
    nodes {
      identifier
      url
      title
      state { name type }
      projectMilestone { name }
    }
  }
}
"""

_MILESTONES_QUERY = """
query Milestones($projectId: String!) {
  project(id: $projectId) {
    projectMilestones(first: 100) { nodes { id name } }
  }
}
"""

_STATES_QUERY = """
query States($teamId: String!) {
  team(id: $teamId) {
    states(first: 100) { nodes { id name type } }
  }
}
"""

_CREATE_MUTATION = """
mutation FileLever($input: IssueCreateInput!) {
  issueCreate(input: $input) {
    success
    issue { identifier url }
  }
}
"""

# The probe's own query. Identical to the listing query plus `description`, which
# the exact line-anchored match in `probe` needs — the server-side `contains`
# filter is only a pre-filter. The body is read in this process and never printed.
_PROBE_QUERY = """
query LeverProbe($filter: IssueFilter, $first: Int!, $after: String) {
  issues(filter: $filter, first: $first, after: $after, includeArchived: true) {
    pageInfo { hasNextPage endCursor }
    nodes {
      identifier
      url
      title
      description
      state { name type }
      projectMilestone { name }
    }
  }
}
"""

# The read half of append-evidence. Fetched into this process, never a transcript.
_BODY_QUERY = """
query LeverBody($id: String!) {
  issue(id: $id) { id identifier url description }
}
"""

_UPDATE_MUTATION = """
mutation AppendEvidence($id: String!, $description: String!) {
  issueUpdate(id: $id, input: { description: $description }) {
    success
    issue { identifier url }
  }
}
"""


def _paged(api_key: str, issue_filter: dict, query: str = _SEARCH_QUERY) -> list[dict]:
    """Every issue matching ``issue_filter``, following the cursor."""
    nodes: list[dict] = []
    after: str | None = None
    for _ in range(MAX_PAGES):
        data = _post(
            api_key,
            query,
            {"filter": issue_filter, "first": PAGE_SIZE, "after": after},
        )
        conn = data.get("issues") or {}
        page = conn.get("nodes") or []
        nodes.extend(page)
        info = conn.get("pageInfo") or {}
        if not info.get("hasNextPage"):
            # A full page with no pageInfo at all is not evidence of a complete
            # read — refuse rather than report a possibly-truncated set as whole.
            if not info and len(page) >= PAGE_SIZE:
                raise TrimLeversError(
                    f"read returned a full page of {PAGE_SIZE} with no pageInfo — "
                    "refusing to treat a possibly-truncated result as complete"
                )
            return nodes
        after = info.get("endCursor")
        if not after:
            raise TrimLeversError("Linear reported another page but returned no cursor")
    raise TrimLeversError(f"read did not terminate within {MAX_PAGES} pages")


def probe(api_key: str, project_id: str, fingerprint: str) -> list[dict]:
    """Issues whose body carries ``fingerprint`` as a field, in **any** state.

    Archived and completed issues are included deliberately: a lever that was
    rejected is closed *with its reason*, and dedup-against-resolved is what makes
    that rejection permanent rather than something the next mining pass
    re-proposes on intuition. Nine of thirteen inbox entries carried a
    "do not mine this as waste" note, several written because an earlier pass had
    re-proposed exactly that.

    **The server filter is a substring pre-filter; the exact match happens here.**
    ``description: {contains: …}`` is a substring test, and slugs nest: a probe
    for ``a:search-scope`` matches the issue carrying ``a:search-scope-axis``.
    Left unfiltered that returns one confident wrong match, so
    :func:`append_evidence` would grow the wrong lever and :func:`file_lever`
    would refuse a genuinely new one — on the single guard the whole pipeline
    rests on. So the body is fetched for the (few) candidates and matched
    line-anchored in this process.

    Fetching a description does **not** break the zero-echo property: that
    property is about what reaches a transcript, and no caller prints these.
    """
    matcher = field_line_re("Fingerprint", fingerprint)
    candidates = _paged(
        api_key,
        {
            "project": {"id": {"eq": project_id}},
            "description": {"contains": fingerprint},
        },
        _PROBE_QUERY,
    )
    return [c for c in candidates if matcher.search(c.get("description") or "")]


def open_parked(matches: list[dict]) -> list[dict]:
    """The subset of ``matches`` that are still parked and open.

    Only a **parked** lever — open *and* still carrying the milestone — accepts
    new evidence. Everything else has moved on: **folded** (closed, its content
    copied into an aggregated task) is already queued for action, **rejected**
    (closed with a reason) is settled, and **promoted** (milestone cleared, moved
    to Backlog) is being worked.

    Selecting the parked one is what keeps accumulation working after the first
    fold: the fold copies each ``**Fingerprint**:`` line into the aggregated task,
    so from then on a raw probe legitimately matches two issues, and treating that
    as ambiguous would stop the recurrence-accumulation this pipeline exists for.

    The open test is by **exclusion** (`completed` / `canceled`) rather than by
    listing the open types. Linear's type set is `triage` / `backlog` /
    `unstarted` / `started` / `completed` / `canceled`, and an allow-list would
    turn any type it failed to anticipate into a hard refusal on a lever that is
    genuinely parked — so the fail-direction here is toward *accepting* the
    append, which is the recoverable one.
    """
    return [
        m
        for m in matches
        if (m.get("state") or {}).get("type") not in ("completed", "canceled")
        and ((m.get("projectMilestone") or {}).get("name") == MILESTONE_NAME)
    ]


def rejected(matches: list[dict]) -> list[dict]:
    """The subset of ``matches`` closed as **canceled** — a recorded rejection.

    This is the one disposition that is *permanent*. A lever closed with a reason
    must never be refiled, which is what stops a later pass re-proposing it on
    intuition. Folding is **not** permanent: it means the fix is queued, and a
    lever whose fold has already shipped can legitimately recur.
    """
    return [m for m in matches if (m.get("state") or {}).get("type") == "canceled"]


def describe(matches: list[dict]) -> str:
    """``ENG-1 [State], ENG-2 [State]`` — for a message naming what was found."""
    return ", ".join(
        f"{m.get('identifier')} [{(m.get('state') or {}).get('name')}]" for m in matches
    )


def parked(api_key: str, project_id: str) -> list[dict]:
    """Every lever currently parked under the milestone — **open** and carrying
    it, matching :func:`open_parked`'s definition exactly.

    The state filter is not optional. Filtering on the milestone alone listed
    every lever that had ever carried it, including ones closed as ``Canceled``
    — a recorded *rejection*, which is settled work. On the first real run that
    was **12 rows of which 9 were canceled rejections**, and only 3 were
    foldable. Two things broke in `trim-context`: its "if nothing is parked,
    report that and stop" could never fire once any rejection existed, because
    the pool always looked non-empty; and it chose the bodies to fold from this
    listing, so a fold pass was invited to fold issues that were closed and
    settled.

    Belt and braces, deliberately. The server-side ``nin`` filter keeps the
    pages small as rejections accumulate, and the caller still runs the rows
    through :func:`open_parked` so the definition of "parked" lives in exactly
    one place. If the comparator ever changes name the query fails loudly rather
    than quietly widening. (``nin`` verified against the live schema by
    introspection: ``WorkflowStateFilter.type`` is a ``StringComparator``, which
    accepts it.)

    Uses :data:`_PARKED_QUERY`, which omits ``includeArchived`` — an archived
    lever is not parked work, and the fold would otherwise list it as such.
    """
    return open_parked(
        _paged(
            api_key,
            {
                "project": {"id": {"eq": project_id}},
                "projectMilestone": {"name": {"eq": MILESTONE_NAME}},
                "state": {"type": {"nin": ["completed", "canceled"]}},
            },
            _PARKED_QUERY,
        )
    )


def resolve_milestone_id(api_key: str, project_id: str) -> str:
    data = _post(api_key, _MILESTONES_QUERY, {"projectId": project_id})
    nodes = ((data.get("project") or {}).get("projectMilestones") or {}).get(
        "nodes"
    ) or []
    for node in nodes:
        if (node.get("name") or "").strip() == MILESTONE_NAME:
            return node["id"]
    available = ", ".join(sorted(str(n.get("name")) for n in nodes)) or "(none)"
    raise TrimLeversError(
        f"no {MILESTONE_NAME!r} milestone on this project — create it once, then "
        f"re-run. Available: {available}"
    )


def resolve_state_id(api_key: str, team_id: str) -> str:
    data = _post(api_key, _STATES_QUERY, {"teamId": team_id})
    nodes = ((data.get("team") or {}).get("states") or {}).get("nodes") or []
    for node in nodes:
        if (node.get("name") or "").strip() == PARKED_STATE:
            return node["id"]
    available = ", ".join(sorted(str(n.get("name")) for n in nodes)) or "(none)"
    raise TrimLeversError(
        f"no {PARKED_STATE!r} workflow state on this team. Available: {available}"
    )


def file_lever(
    api_key: str,
    *,
    project_id: str,
    team_id: str,
    assignee_id: str | None,
    title: str,
    body: str,
    fingerprint: str,
    touches: list[str],
    dry_run: bool,
) -> str:
    """Create one parked lever and return its one-line confirmation.

    Milestone, state and assignee all go in the **creating** call. Filing then
    amending costs a second full body echo and buys nothing — one measured session
    filed an issue in two writes purely to add a relation afterwards.
    """
    # Three dispositions, three different answers. Getting this wrong in the
    # cautious direction is what created a dead end: refusing on ANY match meant
    # that once a lever was folded (original closed, aggregate carrying its
    # fingerprint) neither `file` nor `append-evidence` could proceed, so a
    # recurrence after the fold had no available operation at all.
    existing = probe(api_key, project_id, fingerprint)
    parked = open_parked(existing)
    if parked:
        first = parked[0]
        raise TrimLeversError(
            f"fingerprint {fingerprint} is already parked on "
            f"{first.get('identifier')} ({first.get('url')}) — append-evidence "
            "instead of filing a duplicate"
        )
    turned_down = rejected(existing)
    if turned_down:
        raise TrimLeversError(
            f"fingerprint {fingerprint} was REJECTED ({describe(turned_down)}) — "
            "read the closing reason. A rejection is permanent; refiling it is a "
            "human's call, not an unattended one"
        )
    # Anything else (folded, or promoted and shipped) is superseded rather than
    # settled, so filing a fresh lever is correct — the recurrence is real
    # information. Named in the confirmation so the fold can see the lineage.
    superseded = describe(existing) if existing else ""

    description = compose_body(body, fingerprint, touches)
    lineage = f" (supersedes {superseded})" if superseded else ""
    if dry_run:
        return (
            f"WOULD FILE {fingerprint} | {title} | "
            f"{len(description)} char(s), state {PARKED_STATE}, "
            f"milestone {MILESTONE_NAME}{lineage}"
        )

    milestone_id = resolve_milestone_id(api_key, project_id)
    state_id = resolve_state_id(api_key, team_id)
    payload = {
        "teamId": team_id,
        "projectId": project_id,
        "projectMilestoneId": milestone_id,
        "stateId": state_id,
        "title": title,
        "description": description,
    }
    if assignee_id:
        payload["assigneeId"] = assignee_id

    data = _post(api_key, _CREATE_MUTATION, {"input": payload})
    result = data.get("issueCreate") or {}
    if not result.get("success"):
        raise TrimLeversError(f"issueCreate failed for {fingerprint}")
    issue = result.get("issue") or {}
    return f"FILED {issue.get('identifier')} {issue.get('url')}{lineage}"


def append_evidence(
    api_key: str,
    *,
    project_id: str,
    fingerprint: str,
    evidence: str,
    dry_run: bool,
) -> str:
    """Append this session's evidence to the lever that already exists.

    The read-modify-write happens **here**, inside this process: the stored body
    is fetched, grown, and sent back without ever being printed. That is the whole
    point — the accumulator shape is where the MCP echo compounds worst, since
    each append enlarges what the next one echoes.
    """
    matches = probe(api_key, project_id, fingerprint)
    if not matches:
        raise TrimLeversError(
            f"no issue carries fingerprint {fingerprint} — file it first"
        )

    # Only a still-parked lever accumulates. Everything else is a recorded
    # disposition — and that is NOT an error: raising here crashed an unattended
    # `session-metrics` run (rc 2) for the entirely routine case of a lever
    # recurring after its fold. Report it and succeed; the caller decides whether
    # to file a fresh lever.
    live = open_parked(matches)
    if not live:
        return (
            f"NOTED {fingerprint} is no longer parked ({describe(matches)}) — "
            "folded, rejected, or promoted, so there is no parked lever to grow"
        )
    if len(live) > 1:
        names = ", ".join(str(m.get("identifier")) for m in live)
        raise TrimLeversError(
            f"fingerprint {fingerprint} is on {len(live)} parked levers "
            f"({names}) — refusing to guess which one accumulates the evidence"
        )
    identifier = live[0].get("identifier")

    if dry_run:
        return (
            f"WOULD APPEND to {identifier} | {len(evidence.strip())} char(s) "
            "of evidence"
        )

    data = _post(api_key, _BODY_QUERY, {"id": identifier})
    issue = data.get("issue") or {}
    # Guard the read half. Without this, an issue that resolved to null (archived
    # or deleted between the probe and the read) raises a bare KeyError — a
    # traceback this module exists to avoid — and an empty description silently
    # REPLACES the accumulated lever with the evidence alone. The probe matched
    # *on* the description, so an empty read here is a contradiction, not a
    # legitimately blank body.
    if not issue.get("id"):
        raise TrimLeversError(
            f"{identifier} did not resolve on the body read — it may have been "
            "archived or deleted since the probe; re-run rather than writing"
        )
    stored = issue.get("description") or ""
    if not stored.strip():
        raise TrimLeversError(
            f"{identifier} came back with an empty body, but the probe matched "
            "on its description — refusing to overwrite it with the evidence"
        )
    # Two newlines, never one. A single newline before appended text can leave a
    # heading or rule abutting the previous paragraph, which Linear's round trip
    # re-parses — the setext-heading corruption the merge tool hit twice.
    grown = stored.rstrip("\n") + "\n\n" + evidence.strip() + "\n"

    data = _post(api_key, _UPDATE_MUTATION, {"id": issue["id"], "description": grown})
    result = data.get("issueUpdate") or {}
    if not result.get("success"):
        raise TrimLeversError(f"issueUpdate failed for {identifier}")
    # Deliberately reports only the size, not the text: the grown body is exactly
    # what must not reach a transcript.
    return (
        f"APPENDED {identifier} {issue.get('url')} "
        f"({len(stored)} -> {len(grown)} chars)"
    )


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def _read_file(path: str, label: str) -> str:
    try:
        with open(path, "r", encoding="utf-8") as fh:
            text = fh.read()
    except OSError as e:
        raise TrimLeversError(f"cannot read {label} {path}: {e}") from e
    if not text.strip():
        raise TrimLeversError(f"{label} {path} is empty")
    return text


def _add_dry_run(parser: argparse.ArgumentParser, *, top_level: bool) -> None:
    """``--dry-run``, registered on the top level *and* on each subcommand so it
    is accepted in either position.

    The subcommand copy defaults to ``SUPPRESS``, never ``False``. A subparser
    writes its defaults into the SAME namespace after the top-level parse, so a
    plain ``False`` there silently overwrites ``--dry-run file …`` back to a
    **live run** — turning the rehearsal flag into a no-op exactly when it was
    passed correctly, on a tool whose writes are real Linear issues. This was
    shipped wrong once and caught in review; ``board_batch.py`` carries the
    identical helper for the identical reason, and the two should stay the same
    shape.
    """
    parser.add_argument(
        "--dry-run",
        action="store_true",
        default=False if top_level else argparse.SUPPRESS,
        help="report what would happen without writing it",
    )


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="trim_levers.py",
        description="File and fold trim levers as parked Linear issues.",
    )
    _add_dry_run(parser, top_level=True)
    sub = parser.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("probe", help="find the issue carrying a fingerprint")
    p.add_argument("--fingerprint", required=True)
    _add_dry_run(p, top_level=False)

    f = sub.add_parser("file", help="file a new parked lever")
    f.add_argument("--title", required=True)
    f.add_argument("--fingerprint", required=True)
    f.add_argument("--body-file", required=True)
    f.add_argument("--touches", default=None, help="comma-separated path globs")
    _add_dry_run(f, top_level=False)

    a = sub.add_parser("append-evidence", help="grow an existing lever")
    a.add_argument("--fingerprint", required=True)
    a.add_argument("--evidence-file", required=True)
    _add_dry_run(a, top_level=False)

    lst = sub.add_parser("list", help="the parked pool, for the fold")
    _add_dry_run(lst, top_level=False)

    return parser.parse_args(argv[1:])


def run(argv: list[str]) -> int:
    args = _parse_args(argv)
    api_key = env_var("LINEAR_API_KEY")
    project_id = env_var("LINEAR_PROJECT_ID")

    if args.cmd == "probe":
        fingerprint = validate_fingerprint(args.fingerprint)
        matches = probe(api_key, project_id, fingerprint)
        if not matches:
            print(f"NONE {fingerprint}")
            return 1
        for m in matches:
            state = (m.get("state") or {}).get("name")
            milestone = (m.get("projectMilestone") or {}).get("name") or "-"
            print(
                f"MATCH {m.get('identifier')} [{state}] [{milestone}] "
                f"{m.get('url')} | {m.get('title')}"
            )
        return 0

    if args.cmd == "list":
        levers = parked(api_key, project_id)
        for m in sorted(levers, key=lambda m: str(m.get("identifier"))):
            state = (m.get("state") or {}).get("name")
            print(f"{m.get('identifier')} [{state}] {m.get('url')} | {m.get('title')}")
        print(f"-- {len(levers)} parked lever(s)", file=sys.stderr)
        return 0

    if args.cmd == "file":
        fingerprint = validate_fingerprint(args.fingerprint)
        title = args.title.strip()
        if not title:
            raise TrimLeversError("--title is empty")
        print(
            file_lever(
                api_key,
                project_id=project_id,
                team_id=env_var("LINEAR_TEAM_ID"),
                assignee_id=os.environ.get("LINEAR_ASSIGNEE_ID", "").strip() or None,
                title=title,
                body=_read_file(args.body_file, "--body-file"),
                fingerprint=fingerprint,
                touches=split_touches(args.touches),
                dry_run=args.dry_run,
            )
        )
        return 0

    fingerprint = validate_fingerprint(args.fingerprint)
    print(
        append_evidence(
            api_key,
            project_id=project_id,
            fingerprint=fingerprint,
            evidence=_read_file(args.evidence_file, "--evidence-file"),
            dry_run=args.dry_run,
        )
    )
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except TrimLeversError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
