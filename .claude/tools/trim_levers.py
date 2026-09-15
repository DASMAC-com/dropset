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

**It writes no relations, ever.** A parked lever is not in the pull queue and
sits outside the meta batch until it is folded — and the batch itself carries no
blocking edge any more, its assembly precondition having replaced the one it
used to need. Blocking edges are human-curated in a planning session. Stdlib only; a Python skill-tool under
``.claude/tools/`` — deliberately **not** a Cargo workspace member. Tests live in
``tests/test_trim_levers.py``, run via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import json
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

# The same listing, plus the bodies. Deliberately a SEPARATE query rather than a
# widened `_PARKED_QUERY`: bodies are the expensive half, so the cheap listing
# must stay cheap and the caller must ask for them on purpose.
_PARKED_BODIES_QUERY = """
query ParkedLeverBodies($filter: IssueFilter, $first: Int!, $after: String) {
  issues(filter: $filter, first: $first, after: $after) {
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


def parked_with_bodies(api_key: str, project_id: str) -> list[dict]:
    """:func:`parked`, but each row carries its ``description`` too.

    **One call for the whole pool**, which is the point. The fold's *reads* had
    become its larger cost: the plain listing prints titles only, so a fold ran
    one ``get_issue`` per lever — 21 per-issue fetches on one pass — and the
    nearest sweep that did carry bodies (a project-wide Todo read) cost 10.6k
    for 65 issues with every description truncated anyway, so five per-issue
    follow-ups ran regardless. The read cost is what sized one fold down to five
    levers.

    The filter is :func:`parked`'s, exactly, so the two cannot disagree about
    what "parked" means.
    """
    return open_parked(
        _paged(
            api_key,
            {
                "project": {"id": {"eq": project_id}},
                "projectMilestone": {"name": {"eq": MILESTONE_NAME}},
                "state": {"type": {"nin": ["completed", "canceled"]}},
            },
            _PARKED_BODIES_QUERY,
        )
    )


ATX_HEADING_RE = re.compile(r"^(#{1,6})(\s+.*)$")

# The depth `render_bodies` forces every lever body's headings to start at, so
# they sit strictly below the `## <identifier>` heading above them.
DUMP_BODY_MIN_DEPTH = 3


def normalize_body_headings(body: str, min_depth: int = DUMP_BODY_MIN_DEPTH) -> str:
    """Shift a lever body's ATX headings so its shallowest sits at ``min_depth``,
    leaving fenced blocks alone.

    Relative structure survives the shift, with one bounded exception: the h6
    clamp below can collapse two originally-distinct depths into one, which
    needs a body spanning depth 1 through 5 or deeper. The cost of that is
    slicing *fidelity* — a former parent section becomes a sibling of its own
    child — never a broken dump, since the property everything else depends on
    (every body heading at ``min_depth`` or deeper) still holds. The clamp is
    the lesser evil; see the comment on it.

    Fence handling matches ``read_result.py``'s ``iter_headings`` exactly: both
    toggle on the same fence syntax and skip what is inside. That agreement is
    the point — a heading this function declines to shift is also one the slicer
    declines to see, so the two cannot disagree about what a heading is. On an
    *unbalanced* fence both therefore ignore the remainder of the body, and the
    slicer already warns about that case.

    **This is what makes the dump sliceable, and its absence was a silent
    correctness bug rather than a cosmetic one.** ``render_bodies`` writes a
    ``## <identifier>`` heading per lever, but a lever body carries its own
    ``# Lever`` / ``# Evidence`` / ``# Proposed edit`` headings one level
    *shallower* than that. A markdown section runs until the next heading of
    the same or shallower depth, so a depth-1 ``# Proposed edit`` section did
    not stop at the next lever's depth-2 identifier — it ran on through it,
    swallowing the following lever whole.

    The damage lands on the reader, not the tool: slicing the dump for the two
    sections a fold wants returned those sections *plus* every intervening
    lever's evidence prose, which is the payload the file exists to keep out of
    a transcript. Measured on a fixture reproducing the real shape: a correct,
    end-anchored two-section pattern still returned 38 of 53 lines, including
    evidence explicitly marked as must-not-be-emitted.

    Normalizing *down* rather than raising the identifier keeps
    ``_DUMP_SECTION_RE`` (`^## …`) matching exactly what it did before, and
    guarantees no body heading can ever collide with an identifier line — a
    body heading is now always depth ≥ 3.
    """
    depths = []
    in_fence = False
    for line in body.splitlines():
        if FENCE_RE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        match = ATX_HEADING_RE.match(line)
        if match:
            depths.append(len(match.group(1)))
    if not depths:
        return body
    shift = min_depth - min(depths)
    if shift <= 0:
        return body
    out: list[str] = []
    in_fence = False
    for line in body.splitlines():
        if FENCE_RE.match(line):
            in_fence = not in_fence
            out.append(line)
            continue
        match = ATX_HEADING_RE.match(line) if not in_fence else None
        if match:
            # Clamp at h6: `iter_headings` only recognizes `#{1,6}`, so pushing
            # past it would stop the line being a heading at all and silently
            # un-sliceable — worse than a slightly flattened hierarchy.
            depth = min(len(match.group(1)) + shift, 6)
            out.append("#" * depth + match.group(2))
        else:
            out.append(line)
    return "\n".join(out)


def render_bodies(levers: list[dict]) -> str:
    """The parked pool as one document, ready to be sliced with read_result.py.

    Written to a FILE rather than printed: the bodies are the payload this whole
    pipeline exists to keep out of a transcript, and the caller wants a few
    sections of it, not all of it. One `## <identifier>` heading per lever makes
    ``read_result.py --headings`` / ``--section`` the natural next call.

    Each body's own headings are normalized to sit **below** that identifier
    (see ``normalize_body_headings``) so a sliced section stops at the next
    lever instead of running through it.
    """
    parts = []
    for lever in sorted(levers, key=lambda m: str(m.get("identifier"))):
        parts.append(f"## {lever.get('identifier')} | {lever.get('title')}")
        parts.append("")
        parts.append(f"{lever.get('url')}")
        parts.append("")
        parts.append(normalize_body_headings((lever.get("description") or "").rstrip()))
        parts.append("")
    return "\n".join(parts).rstrip() + "\n"


_DUMP_SECTION_RE = re.compile(r"^## (\S+) \| (.*)$", re.MULTILINE)
_BARE_URL_RE = re.compile(r"^https://\S+\s*$")

#: Labels a lever body uses for its *statement*, and for its *concrete edit*.
#: Both spellings vary because the producer never constrained them: a filer is
#: told to keep a body "compact — the lever, the sessions and figures, and the
#: concrete edit" and no headings are named, so every variant below is compliant.
STATEMENT_LABELS = ("the lever", "lever")
EDIT_LABELS = (
    "the edit this implies",
    "the concrete edit",
    "proposed edit",
    "the edit",
    "where to edit",
    "the fix",
)


def _label_alternation(labels: tuple[str, ...]) -> str:
    return "|".join(re.escape(label) for label in labels)


def _fenced_line_flags(body: str) -> list[bool]:
    """Per-line "this line is fence syntax or inside a fence" flags.

    Uses the same ``FENCE_RE`` toggle as ``field_values`` and ``demote_headings``,
    so the four sites can never disagree about what a fence is — including how
    they degrade on an *unbalanced* fence (everything after it reads as fenced).
    """
    flags: list[bool] = []
    in_fence = False
    for line in body.split("\n"):
        if FENCE_RE.match(line):
            in_fence = not in_fence
            flags.append(True)
            continue
        flags.append(in_fence)
    return flags


def find_labelled_block(body: str, labels: tuple[str, ...]) -> str | None:
    """The block a lever body introduces with one of ``labels``, or None.

    Matches a **heading** (``## The lever``) or a **bold inline span**
    (``**Lever:**``, ``**The lever.**``, ``**The lever — fix it at the
    producer**``), because the parked pool contains both and always will: only 6
    of one real 18-lever pool carried sub-headings at all, and the other 12 stated
    the lever as a bold span with no heading anywhere. A `--sections` read
    prescribed against the heading form failed outright on that pool — exit 2,
    ``no heading matches`` — and the fold fell back to per-region slices that
    became its dominant cost.

    Matching both here rather than widening the *skill's* regex is the deliberate
    choice: a regex covering bold spans would work today and rot the same way,
    whereas an extraction that runs in this process costs the caller nothing
    whichever shape it finds.
    """
    alternation = _label_alternation(labels)

    # Heading form. Ends at the next heading of any depth — scanned line by line
    # and **fence-aware**, because a `#` inside a fenced block is not a heading.
    # A whole-body regex here was truncating real statements: lever bodies quote
    # shell constantly, and a fenced `# comment` line ended the block early,
    # silently returning a fragment. Under-capture is the dangerous direction —
    # `render_levers` reports a section it cannot find at all, but a section it
    # found and cut short passes as complete.
    lines = body.split("\n")
    fenced = _fenced_line_flags(body)
    label_re = re.compile(rf"^#{{1,6}}\s*(?:{alternation})\s*$", re.IGNORECASE)
    for start, line in enumerate(lines):
        if fenced[start] or not label_re.match(line):
            continue
        end = len(lines)
        for j in range(start + 1, len(lines)):
            if not fenced[j] and ATX_HEADING_RE.match(lines[j]):
                end = j
                break
        text = "\n".join(lines[start + 1 : end]).strip()
        if text:
            return text
        # An empty labelled section falls through to the bold-span form, which is
        # what the whole-body regex did too.
        break

    # Bold-span form. The label may be followed by more bold text on the same
    # line (`**The lever — fix it at the producer, not with a wider regex.**`),
    # so the line itself is kept: it usually carries the statement.
    span = re.search(
        rf"^\*\*(?:{alternation})\b(?P<rest>.*?)(?=^\s*$|^\*\*|^#{{1,6}}\s|\Z)",
        body,
        re.MULTILINE | re.IGNORECASE | re.DOTALL,
    )
    if span:
        text = span.group(0).strip()
        if text:
            return text
    return None


#: A code-span token that looks like a repo path a lever names as its target.
#: Anchored on the trees agent material actually lives in, because that is what a
#: stale-check probes; a looser "contains a slash" test drags in currency pairs,
#: git refs and Action slugs.
_TARGET_RE = re.compile(
    r"`([^`\n]*(?:\.claude/[A-Za-z0-9._/-]+|docs/[A-Za-z0-9._/-]+"
    r"|[A-Za-z0-9_]+\.py|[A-Za-z0-9_-]+\.md)[.,;:]*)`"
)


def lever_targets(body: str) -> list[str]:
    """Repo paths and tool names a lever's prose names, de-duplicated in order.

    The checklist a stale-check needs. It exists because the check has to be run
    per lever and re-reading every body to find the names is the cost that stops
    it being run at all.

    **A rename is the expected case, so this is a starting point, not an answer.**
    One measured lever asked for a committed `query_store.py` and the capability
    had shipped as `localnet_psql.py` — grepping the proposed name finds nothing
    and the lever reads unlanded. So the question a verifier asks is "does the
    described capability exist", not "does this path exist"; these names are
    where to start looking.
    """
    seen: list[str] = []
    for raw in _TARGET_RE.findall(body):
        token = raw.strip().strip("`").rstrip(".,;:")
        # Keep the trailing path segment only when a span wrapped prose around it
        # (`the committed .claude/tools/x.py helper`).
        token = token.split()[-1] if " " in token else token
        if token and token not in seen:
            seen.append(token)
    return seen


def extract_lever(body: str) -> tuple[str | None, str | None]:
    """``(statement, edit)`` from one lever body, either shape."""
    return (
        find_labelled_block(body, STATEMENT_LABELS),
        find_labelled_block(body, EDIT_LABELS),
    )


def render_levers(levers: list[dict]) -> tuple[str, dict]:
    """The pool reduced to what a FOLD consumes, plus a coverage report.

    A fold reads two things from each lever — its statement and its concrete
    edit — and cites the evidence prose by reference rather than inlining it. So
    this writes only those, under a uniform
    ``## <identifier> | <title>`` / ``### Statement`` / ``### Edit`` shape, which
    makes one ``--sections`` call work on the whole pool whatever the producers
    did.

    The report names levers whose statement or edit could not be found, because a
    silent omission here would drop a lever from the fold — the one failure worse
    than a wide read. Fall back to slicing the ``--bodies-out`` dump for those.
    """
    parts: list[str] = []
    missing_statement: list[str] = []
    missing_edit: list[str] = []
    for lever in sorted(levers, key=lambda m: str(m.get("identifier"))):
        identifier = str(lever.get("identifier"))
        body = (lever.get("description") or "").rstrip()
        statement, edit = extract_lever(body)
        if statement is None:
            missing_statement.append(identifier)
        if edit is None:
            missing_edit.append(identifier)
        keys = field_values(body, "Fingerprint")

        parts.append(f"## {identifier} | {lever.get('title')}")
        parts.append("")
        parts.append(f"{lever.get('url')}")
        parts.append("")
        # The dedup key travels with the statement, exactly as it does in the
        # bodies dump — a fold keeps each finding's own Fingerprint line.
        parts.append(f"**Fingerprint**: {keys[0] if keys else '(none)'}")
        parts.append("")
        parts.append("### Statement")
        parts.append("")
        parts.append(demote_headings(statement) if statement else "(not found)")
        parts.append("")
        parts.append("### Edit")
        parts.append("")
        parts.append(demote_headings(edit) if edit else "(not found)")
        parts.append("")
    report = {
        "levers": len(levers),
        "missing_statement": missing_statement,
        "missing_edit": missing_edit,
    }
    return "\n".join(parts).rstrip() + "\n", report


def strip_leading_url(raw: str) -> str:
    """Drop the issue URL ``render_bodies`` writes under each dump heading.

    Anchored to that **known position** — the first non-blank line of the
    section — rather than matched anywhere in the body, which is what it used
    to do (``^https://\\S+\\s*$`` with ``re.MULTILINE``, substituted globally).
    Two problems with the global form, both of which corrupt a fold silently:

    - it is **fence-blind**, so a URL on its own line inside a fenced example
      — a `curl` invocation, a documented endpoint, a citation — was deleted
      from the folded body, and a lever whose whole point is an HTTP surface is
      exactly the kind that carries one;
    - ``\\s*`` matches newlines, so a match could swallow the blank lines after
      it and run two paragraphs together.

    The URL is emitted at one place by one function, so keying on that position
    is both simpler and strictly more accurate than pattern-matching for it.
    """
    lines = raw.split("\n")
    for index, line in enumerate(lines):
        if not line.strip():
            continue
        if _BARE_URL_RE.match(line.strip()):
            del lines[index]
        break
    return "\n".join(lines)


def demote_headings(body: str) -> str:
    """Push every ATX heading down one level, leaving fenced blocks alone.

    Lever bodies carry their own ``#``-level headings (``# Lever``,
    ``# Evidence``, ``# Proposed edit``). Pasted unmodified under a ``# Part N``
    heading they collide at the same level and the aggregated task loses its
    structure — which is silent damage: the body is all there and merely reads
    as one flat document.

    Fence-awareness uses the same ``FENCE_RE`` as ``field_values`` on purpose.
    A lever *about filing conventions* legitimately quotes a fenced markdown
    example, and demoting a ``#`` inside it would corrupt the quoted material.
    """
    out: list[str] = []
    in_fence = False
    for line in body.splitlines():
        if FENCE_RE.match(line):
            in_fence = not in_fence
            out.append(line)
            continue
        if not in_fence and line.startswith("#"):
            out.append("#" + line)
        else:
            out.append(line)
    return "\n".join(out)


def split_dump(dump: str) -> list[tuple[str, str, str]]:
    """``[(identifier, title, body), …]`` from a ``list --bodies-out`` dump."""
    matches = list(_DUMP_SECTION_RE.finditer(dump))
    sections = []
    for i, match in enumerate(matches):
        end = matches[i + 1].start() if i + 1 < len(matches) else len(dump)
        sections.append(
            (match.group(1), match.group(2).strip(), dump[match.end() : end])
        )
    return sections


def _dump_sections(dump: str) -> list[tuple[str, str, str]]:
    """The dump's lever sections, validated — shared by both compose modes."""
    sections = split_dump(dump)
    if not sections:
        raise TrimLeversError(
            "no '## <identifier> | <title>' sections found — expected the "
            "output of `trim_levers.py list --bodies-out`"
        )

    # A repeated identifier means two dumps were concatenated, or one was
    # appended to twice. Folding it would emit the same lever as two numbered
    # parts and then close the single underlying issue once, so the duplicate
    # part survives in the fold with nothing behind it — and `--exclude`, or a
    # group naming that identifier, would silently drop BOTH. Cheap to detect,
    # confusing to unpick afterwards.
    seen: dict[str, int] = {}
    for identifier, _, _ in sections:
        seen[identifier.upper()] = seen.get(identifier.upper(), 0) + 1
    repeated = sorted(name for name, count in seen.items() if count > 1)
    if repeated:
        raise TrimLeversError(
            f"these identifiers appear more than once in the dump: "
            f"{', '.join(repeated)} — two dumps concatenated, or one appended "
            "to twice. Re-run `list --bodies-out` to a fresh file."
        )
    return sections


def compose_fold(
    dump: str, *, start: int = 1, exclude: tuple[str, ...] = ()
) -> tuple[str, dict]:
    """The aggregated fold body, from the ``--bodies-out`` dump.

    ``list --bodies-out`` solved the fold's *fetch*; nothing solved its
    *composition*. Under the whole-pool ruling a fold carries the entire parked
    pool — one pass folded 41 levers — so re-authoring by hand stopped being
    sensible, and the pass that hit it wrote a throwaway script instead. This is
    that script, committed, because it encodes two rules that are easy to get
    wrong by hand and damaging when missed: heading demotion (above) and
    fingerprint preservation (below).

    **Fails loudly on a part with no ``**Fingerprint**:`` line.** Per-lever
    dedup rests on every part keeping its own key, and a hand fold that
    summarizes rather than carries the body drops them — a loss invisible until
    a later pass refiles a lever that was already folded. Silence here would
    defeat the point of the tool.
    """
    dropped = {item.strip().upper() for item in exclude if item.strip()}
    sections = _dump_sections(dump)

    parts: list[str] = []
    folded: list[str] = []
    skipped: list[str] = []
    missing: list[str] = []
    number = start

    for identifier, title, raw in sections:
        if identifier.upper() in dropped:
            skipped.append(identifier)
            continue
        body = strip_leading_url(raw).strip("\n")
        if not field_values(body, "Fingerprint"):
            missing.append(identifier)
        parts.append(f"# Part {number} — {title}\n\n{demote_headings(body)}\n")
        folded.append(identifier)
        number += 1

    if missing:
        raise TrimLeversError(
            "these levers carry no **Fingerprint**: line, so folding them would "
            f"silently break per-lever dedup: {', '.join(missing)}"
        )
    if not parts:
        raise TrimLeversError("every section was excluded — nothing to compose")

    # An exclusion naming a lever the dump does not contain is almost always a
    # typo, and a silent no-op there would fold a lever the caller meant to
    # drop. Reported rather than raised: the caller may legitimately carry one
    # exclusion list across two dumps.
    unknown = dropped - {identifier.upper() for identifier, _, _ in sections}
    return "\n".join(parts), {
        "folded": folded,
        "skipped": skipped,
        "unknown_exclusions": sorted(unknown),
        "next_part": number,
    }


# The fold's size bound (`trim-context` step 3): a task is sized to a short
# session, roughly 4–5 levers. Advisory rather than enforced, because the rule
# is explicitly approximate — a hard limit here would turn "roughly" into a
# refusal and push callers into splitting a coherent theme to satisfy a tool.
_MAX_GROUP_PARTS = 5

# A body this tool emits, for telling one apart from whatever else a caller keeps
# in the output directory. Matches the `f"{index:02d}-{slug}.md"` shape written
# below, allowing three digits so the hundredth task is still recognized as ours.
_TASK_FILE_RE = re.compile(r"^\d{2,3}-[a-z0-9-]*\.md$")


def _slug(text: str) -> str:
    """A filename-safe slug for a task title."""
    cleaned = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
    return (cleaned[:48].strip("-")) or "task"


def parse_groups_spec(raw: str) -> list[tuple[str, tuple[str, ...]]]:
    """The ``--groups-file`` spec: one ``{title, levers}`` entry per task.

    A file rather than repeated flags, matching how ``board_batch.py --updates``
    and ``linear_patch.py --ops`` already take assembled work — it keeps the
    call a single bare command whatever the pool size, and keeps the grouping
    out of the transcript.
    """
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise TrimLeversError(f"groups file is not valid JSON: {exc}") from exc
    if not isinstance(data, list) or not data:
        raise TrimLeversError(
            "groups file must be a non-empty JSON array of "
            '{"title": ..., "levers": [...]} objects, one per task'
        )

    groups: list[tuple[str, tuple[str, ...]]] = []
    for index, entry in enumerate(data, start=1):
        if not isinstance(entry, dict):
            raise TrimLeversError(f"group {index} is not a JSON object")
        title = entry.get("title")
        levers = entry.get("levers")
        if not isinstance(title, str) or not title.strip():
            raise TrimLeversError(f'group {index} has no non-empty "title"')
        if not isinstance(levers, list) or not levers:
            raise TrimLeversError(
                f'group {index} ("{title}") has no non-empty "levers" array'
            )
        # A title is echoed verbatim into the pipe-delimited summary that a skill
        # reads back, so an interior newline or a literal ` | ` can forge a line
        # of machine-shaped output. `_slug` sanitizes the *filename*; nothing
        # sanitizes the echo, so reject it at the boundary instead.
        if any(ch in title for ch in "\n\r|") or any(
            ord(ch) < 32 or ord(ch) == 127 for ch in title
        ):
            raise TrimLeversError(
                f"group {index} has a title containing a newline, a '|', or a "
                "control character — those forge a line in the summary this "
                "tool prints for a caller to parse"
            )

        names: list[str] = []
        for item in levers:
            if not isinstance(item, str) or not item.strip():
                raise TrimLeversError(
                    f'group {index} ("{title}") has a lever entry that is not '
                    "a non-empty string"
                )
            names.append(item.strip().upper())
        groups.append((title.strip(), tuple(names)))
    return groups


def compose_groups(
    dump: str, groups: list[tuple[str, tuple[str, ...]]]
) -> tuple[list[dict], dict]:
    """One composed body per group, from a single ``--bodies-out`` dump.

    The fold now files **several** small themed tasks rather than one (operator
    rule, 2026-09-11), and ``compose_fold`` emits exactly one body per call — so
    N tasks meant N invocations, each having to re-declare every already-folded
    lever in ``--exclude`` and carry the previous call's ``next_part``. That is
    bookkeeping the caller cannot verify and the tool can: a lever dropped from
    every group is invisible, which is the same silent-loss class the
    fingerprint check exists to catch.

    **Parts number from 1 within each task**, not continuously across them.
    ``--start`` exists for a different shape — under the retired whole-pool
    ruling a single task was sometimes composed in halves, and its numbering had
    to continue. A task is now a whole issue, so its parts are Part 1..N *of
    that issue*.
    """
    sections = _dump_sections(dump)
    by_id = {
        identifier.upper(): (identifier, title, raw)
        for identifier, title, raw in sections
    }

    # Assignment is checked in full before anything is composed, so one run
    # names every problem rather than failing on the first group.
    assigned: dict[str, int] = {}
    for index, (_, levers) in enumerate(groups, start=1):
        for name in levers:
            if name in assigned:
                # Same group twice and two different groups are the same defect
                # — the lever lands as a part twice while the issue behind it
                # closes once — but they are different mistakes to go and fix,
                # and reporting "groups 1 and 1" reads as a tool bug and sends
                # the caller hunting through the other groups.
                if assigned[name] == index:
                    raise TrimLeversError(
                        f"{name} is listed twice in group {index} — a lever "
                        "folds into exactly one part, or it lands twice while "
                        "the issue behind it closes once"
                    )
                raise TrimLeversError(
                    f"{name} is assigned to more than one group (groups "
                    f"{assigned[name]} and {index}) — a lever folds into "
                    "exactly one task, or it lands as a part twice while the "
                    "issue behind it closes once"
                )
            assigned[name] = index

    unknown = sorted(name for name in assigned if name not in by_id)
    if unknown:
        raise TrimLeversError(
            f"these grouped levers are not in the dump: {', '.join(unknown)} — "
            "a typo, or a stale grouping against an older dump. Unlike "
            "`--exclude`, a group assignment is authoritative for this dump, so "
            "this is an error rather than a warning."
        )

    unassigned = [
        identifier
        for identifier, _, _ in sections
        if identifier.upper() not in assigned
    ]
    if unassigned:
        raise TrimLeversError(
            "these levers are in the dump but in no group, so folding would "
            f"silently drop them: {', '.join(unassigned)} — assign every lever "
            "to a task, or re-run `list --bodies-out` without the ones you "
            "meant to leave parked"
        )

    tasks: list[dict] = []
    missing: list[str] = []
    oversized: list[tuple[str, int]] = []

    for title, levers in groups:
        parts: list[str] = []
        folded: list[str] = []
        for number, name in enumerate(levers, start=1):
            identifier, part_title, raw = by_id[name]
            body = strip_leading_url(raw).strip("\n")
            if not field_values(body, "Fingerprint"):
                missing.append(identifier)
            parts.append(f"# Part {number} — {part_title}\n\n{demote_headings(body)}\n")
            folded.append(identifier)
        if len(folded) > _MAX_GROUP_PARTS:
            oversized.append((title, len(folded)))
        tasks.append({"title": title, "body": "\n".join(parts), "folded": folded})

    if missing:
        raise TrimLeversError(
            "these levers carry no **Fingerprint**: line, so folding them would "
            f"silently break per-lever dedup: {', '.join(missing)}"
        )

    return tasks, {
        "tasks": len(tasks),
        # The dump's own spelling, not the upper-cased match key — `compose_fold`'s
        # summary and each task's `folded` both carry dump spellings, and one
        # module handing back two normalizations under the same key name is a trap
        # for the next consumer (most plausibly the step that closes the parked
        # originals). Identifiers are `ENG-###`-shaped today, so the two coincide;
        # that is exactly why the divergence would go unnoticed.
        "folded": sorted(by_id[name][0] for name in assigned),
        "oversized": oversized,
    }


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
    except UnicodeDecodeError as e:
        # Not decorative: `--groups-file` is hand-authored, so a file saved in
        # another encoding is a realistic mistake, and a UnicodeDecodeError is a
        # ValueError rather than an OSError — so without this it escapes the CLI
        # as a traceback instead of the one-line error every other failure gets.
        raise TrimLeversError(f"{label} {path} is not valid UTF-8: {e}") from e
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
    lst.add_argument(
        "--fingerprints",
        action="store_true",
        help="also print each lever's Fingerprint — the dedup key, which is what "
        "a sibling lookup actually needs, and which the plain listing omits",
    )
    lst.add_argument(
        "--bodies-out",
        default=None,
        metavar="FILE",
        help="fetch every parked body in ONE call and write them to FILE (one "
        "'## <identifier>' section each); prints sizes only. Slice it with "
        "read_result.py rather than fetching per issue",
    )
    lst.add_argument(
        "--targets",
        action="store_true",
        help="print the repo paths and tool names each lever's prose names — the "
        "checklist for the pre-fold stale check. A rename is the expected case, so "
        "verify the CAPABILITY exists, not the path",
    )
    lst.add_argument(
        "--levers-out",
        default=None,
        metavar="FILE",
        help="like --bodies-out, but write ONLY each lever's statement and "
        "concrete edit, under a uniform Statement/Edit shape — the two things a "
        "fold consumes. Works whether the lever used headings or bold spans, so "
        "one --sections read covers the whole pool; names any lever it could not "
        "extract",
    )
    _add_dry_run(lst, top_level=False)

    comp = sub.add_parser(
        "compose",
        help="the aggregated fold body, from a --bodies-out dump",
    )
    comp.add_argument(
        "--bodies-file",
        required=True,
        metavar="FILE",
        help="the dump written by `list --bodies-out`",
    )
    comp.add_argument(
        "--out",
        metavar="FILE",
        help="single-task mode: where to write the one composed body; prints a "
        "summary only, since the body is the payload this pipeline keeps out "
        "of a transcript",
    )
    comp.add_argument(
        "--groups-file",
        metavar="FILE",
        help='multi-task mode: a JSON array of {"title", "levers"} objects, '
        "one per task — emits one conforming body per group in a single run, "
        "each numbering its parts from 1. Requires --out-dir",
    )
    comp.add_argument(
        "--out-dir",
        metavar="DIR",
        help="multi-task mode: the directory to write the per-task bodies "
        "into (created if absent); the summary names each file",
    )
    comp.add_argument(
        "--start",
        type=int,
        # None, not 1, so an explicit `--start 1` is distinguishable from an
        # absent one. Otherwise the mode check below cannot see it and the
        # documented promise — flags from both modes are refused rather than
        # silently resolved — is not literally true.
        default=None,
        help="single-task mode only: the first `# Part N` number (default 1) — "
        "pass the previous fold's next_part when one task is composed in "
        "halves. Parts always number from 1 per task in --groups-file mode",
    )
    comp.add_argument(
        "--exclude",
        default="",
        help="single-task mode only: comma-separated identifiers to drop "
        "(levers already folded). In --groups-file mode, omitting a lever "
        "from every group is an error, not a silent drop",
    )

    args = parser.parse_args(argv[1:])

    # Two modes, and mixing them silently would be worse than refusing: the
    # single-task flags describe one body with a caller-managed part number,
    # the grouped flags describe N bodies each numbered from 1.
    if args.cmd == "compose":
        if bool(args.out) == bool(args.groups_file):
            parser.error(
                "compose needs exactly one of --out (one task) or "
                "--groups-file (N tasks)"
            )
        if args.groups_file and not args.out_dir:
            parser.error("--groups-file requires --out-dir")
        if args.out and args.out_dir:
            parser.error("--out-dir belongs to --groups-file mode, not --out")
        if args.groups_file and args.start is not None:
            parser.error(
                "--start belongs to --out mode; in --groups-file mode each "
                "task numbers its parts from 1"
            )
        if args.groups_file and args.exclude:
            parser.error(
                "--exclude belongs to --out mode; in --groups-file mode a "
                "lever is dropped by leaving it out of the dump, and omitting "
                "it from every group is an error"
            )

    return args


def run(argv: list[str]) -> int:
    args = _parse_args(argv)

    # `compose` is pure local text work — it reads a dump and writes a body,
    # touching no API. Handled before the credential lookup so it runs in a
    # shell with no Linear secrets resolved, which is also what makes it
    # testable without mocking the transport.
    if args.cmd == "compose" and args.groups_file:
        dump = _read_file(args.bodies_file, "bodies file")
        groups = parse_groups_spec(_read_file(args.groups_file, "groups file"))
        tasks, summary = compose_groups(dump, groups)
        try:
            os.makedirs(args.out_dir, exist_ok=True)
        except OSError as exc:
            raise TrimLeversError(f"cannot create {args.out_dir}: {exc}") from exc
        for index, task in enumerate(tasks, start=1):
            path = os.path.join(args.out_dir, f"{index:02d}-{_slug(task['title'])}.md")
            try:
                # O_NOFOLLOW because the mode below protects only content this
                # call creates: a pre-planted symlink at our filename would
                # redirect the write and leave the 0o600 guarding nothing.
                # Scratchpads live under a shared /tmp root, so that is not
                # purely theoretical. The chmod covers the same gap from the
                # other side — O_CREAT applies its mode only when it creates, so
                # re-running over an existing looser file would truncate and
                # rewrite it while leaving that mode intact.
                handle = os.open(
                    path,
                    os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW,
                    0o600,
                )
                with os.fdopen(handle, "w", encoding="utf-8") as fh:
                    fh.write(task["body"])
                os.chmod(path, 0o600)
            except OSError as exc:
                raise TrimLeversError(f"cannot write {path}: {exc}") from exc
            task["path"] = path
        print(
            f"trim-levers: composed {summary['tasks']} task(s) from "
            f"{len(summary['folded'])} lever(s) into {args.out_dir}"
        )
        for task in tasks:
            print(
                f"trim-levers: {task['path']} | {len(task['folded'])} part(s) "
                f"| {', '.join(task['folded'])} | {task['title']}"
            )
        # A reused --out-dir keeps bodies this run did not write: compose three
        # groups, revise the grouping down to two, re-run, and `03-*.md` survives
        # carrying levers that are now parts of tasks 1-2. A caller who files
        # every body in the directory then files a phantom issue whose parts
        # duplicate the real ones, and the fold closes the parked originals once.
        # Reported, never deleted — a composer that removes files the caller may
        # have put there is worse than the residue it cleans up.
        written = {os.path.basename(task["path"]) for task in tasks}
        stale = sorted(
            name
            for name in os.listdir(args.out_dir)
            if _TASK_FILE_RE.match(name) and name not in written
        )
        if stale:
            print(
                "trim-levers: ADVISORY these files in "
                f"{args.out_dir} are NOT from this run: {', '.join(stale)} — "
                "left by an earlier compose. File only the paths listed above"
            )
        for title, count in summary["oversized"]:
            print(
                f"trim-levers: ADVISORY {count} parts exceeds the {_MAX_GROUP_PARTS}"
                f"-lever short-session bound: {title} — split it, or carry it "
                "deliberately"
            )
        return 0

    if args.cmd == "compose":
        dump = _read_file(args.bodies_file, "bodies file")
        # `--start` defaults to None so grouped mode can tell an explicit 1 from
        # an absent one; this path wants the documented default of 1.
        start = 1 if args.start is None else args.start
        body, summary = compose_fold(
            dump,
            start=start,
            exclude=tuple(args.exclude.split(",")),
        )
        try:
            handle = os.open(args.out, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(handle, "w", encoding="utf-8") as fh:
                fh.write(body)
        except OSError as exc:
            raise TrimLeversError(f"cannot write {args.out}: {exc}") from exc
        print(
            f"trim-levers: composed {len(summary['folded'])} part(s) "
            f"({len(body)} chars) into {args.out}"
        )
        print(f"trim-levers: parts {start}..{summary['next_part'] - 1}")
        if summary["skipped"]:
            print(f"trim-levers: excluded {', '.join(summary['skipped'])}")
        if summary["unknown_exclusions"]:
            print(
                "trim-levers: WARNING these exclusions matched nothing in the "
                f"dump (typo?): {', '.join(summary['unknown_exclusions'])}"
            )
        return 0

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
        # `--targets` is a column of the PRINTED listing, and the rows are
        # suppressed whenever output goes to a file — so pairing it with a
        # `--*-out` dropped it in silence, which reads as "this pool has no
        # targets" rather than "you asked for a column that was never printed".
        # Refuse instead: a flag that cannot take effect is a mistake worth
        # naming, the same way `read_result.py` refuses `--max-depth` without
        # `--headings`.
        #
        # `--fingerprints` is deliberately NOT refused here even though its column
        # is suppressed too: both renderers write a `**Fingerprint**:` line into
        # the file itself, so the key is still delivered and the flag is merely
        # redundant. `--targets` has no such counterpart — neither renderer emits
        # the extracted path list.
        if args.targets and (args.bodies_out or args.levers_out):
            spill = "--bodies-out" if args.bodies_out else "--levers-out"
            raise TrimLeversError(
                f"--targets is a column of the printed listing, which {spill} "
                f"suppresses — so it would have no effect. Drop it, or run "
                f"`list --targets` as a separate call."
            )

        # Bodies are fetched only when asked for, and a fingerprints-only read
        # needs them (the key lives in the body) — so one query serves both.
        wants_bodies = (
            bool(args.bodies_out)
            or bool(args.levers_out)
            or args.targets
            or args.fingerprints
        )
        levers = (
            parked_with_bodies(api_key, project_id)
            if wants_bodies
            else parked(api_key, project_id)
        )
        # `--bodies-out`'s OUTPUT IS THE FILE, so the rows are suppressed. They
        # used to print anyway, which meant a fold following the skill's own
        # step-2 wording bought the same 41-row listing three times — a plain
        # `list` (~1.3k), a `list --fingerprints` (~1.7k, the same rows plus a
        # column), and a `list --bodies-out` (~1.3k, one size line and then the
        # same rows again). ~4.3k for one listing's worth of information, ranks
        # 4, 5 and 7 of that session's largest results, beating every read the
        # fold actually consumed. The third was the clearest waste: it
        # re-printed the listing it had just been asked to spill.
        #
        # `--fingerprints` composes with it, so one call serves the whole read
        # of the pool: the keys ride each `## <identifier>` section in the file.
        if not args.bodies_out and not args.levers_out:
            for m in sorted(levers, key=lambda m: str(m.get("identifier"))):
                state = (m.get("state") or {}).get("name")
                line = (
                    f"{m.get('identifier')} [{state}] {m.get('url')} | {m.get('title')}"
                )
                if args.fingerprints:
                    keys = field_values(m.get("description") or "", "Fingerprint")
                    line += f" | {', '.join(keys) if keys else '(no fingerprint)'}"
                if args.targets:
                    names = lever_targets(m.get("description") or "")
                    line += f" | targets: {', '.join(names) if names else '(none)'}"
                print(line)
        if args.bodies_out:
            rendered = render_bodies(levers)
            try:
                # 0o600 for the same reason the review diff is: a lever body can
                # quote whatever a session had in scope.
                fd = os.open(
                    args.bodies_out, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600
                )
                with os.fdopen(fd, "w", encoding="utf-8") as handle:
                    handle.write(rendered)
            except OSError as exc:
                raise TrimLeversError(f"cannot write {args.bodies_out}: {exc}") from exc
            # Sizes only. Printing the rendered text here would undo the point.
            print(
                f"-- wrote {len(levers)} body(ies), {len(rendered)} chars to "
                f"{args.bodies_out} — slice it with read_result.py --headings",
                file=sys.stderr,
            )
        if args.levers_out:
            rendered, report = render_levers(levers)
            try:
                fd = os.open(
                    args.levers_out, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600
                )
                with os.fdopen(fd, "w", encoding="utf-8") as handle:
                    handle.write(rendered)
            except OSError as exc:
                raise TrimLeversError(f"cannot write {args.levers_out}: {exc}") from exc
            print(
                f"-- wrote {report['levers']} lever(s), {len(rendered)} chars to "
                f"{args.levers_out} — slice it with "
                f"read_result.py --sections '^(Statement|Edit)$'",
                file=sys.stderr,
            )
            # Named out loud, because a silently dropped statement or edit would
            # drop a whole lever from the fold — worse than any wide read.
            for label, ids in (
                ("statement", report["missing_statement"]),
                ("edit", report["missing_edit"]),
            ):
                if ids:
                    print(
                        f"-- WARNING: no {label} found for {', '.join(ids)} — "
                        f"slice those from a --bodies-out dump instead",
                        file=sys.stderr,
                    )
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
