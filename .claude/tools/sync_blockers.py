#!/usr/bin/env python3
"""Keep the Dropset Linear Backlog's file-overlap links in sync with ``Touches``.

This is the deterministic core of the ``sync-blockers`` skill. Its one job is
**relation maintenance**: given the open Backlog's ``**Touches**:`` globs, find
every file-overlap collision that has no link yet and materialize it into a real
Linear ``related`` relation, naming the paths the two issues collide on. It
never renders or writes a document, and it never merges or closes issues.

**This tool does not write blocking edges, and neither does anything else
automated.** A ``blocks`` edge and a file collision are two different claims:

* A **semantic dependency** — B consumes A's output — genuinely orders work.
* A **mechanical collision** — two PRs touch the same glob — costs at most a
  rebase.

Coarse crate-level ``**Touches**`` globs, binary block semantics, and the
arbitrary lower-number-blocks-higher orientation used to conflate the two, and
the result was giant serial chains: a day-1 mainnet issue sat behind eight
overlap blockers, and a docs-only pair was block-linked because both touched
``docs/market-making-mvp.md`` in unrelated sections.

The deeper reason no automated writer may file one is that the board's
available-vs-blocked view is a **scheduling instrument a human drives** — a
hand-built queue expressing intended order of attack, from which the *available*
set is then sorted by priority. An auto-filed edge silently makes that view
untrustworthy, and a wrongly-blocked issue drops out of the available set
altogether. A missing edge costs a rebase; a spurious one costs scheduling. So
blocking edges are **human-curated end to end**: an agent may *suggest* one with
its evidence, and a human approves or places it. Edges a human placed are
authoritative — this tool never rewrites, redirects, or removes one, with no
exception.

There used to be one. A ``--demote`` mode performed a one-time,
propose-then-confirm migration of pre-existing auto-filed ``blocks`` edges to
``related``. It ran on 2026-08-10 and is **spent**: every candidate it can still
find is a false positive, because the six legitimate hand-placed edges collide
on files and are therefore indistinguishable from artifacts. A second
``--demote --apply`` would delete the intended blocking graph in one command, so
the mode was removed rather than guarded — dead code that is also a foot-gun.
Blocking-edge changes now happen only in a human-directed planning session.

Modes:

* **Incremental** (``--for ENG-###``) — the file-time path. Compares *only* the
  named, just-filed issue's touches against the rest of the open Backlog and
  relates its collisions, printing the paths each one collides on. Bounded work
  (one node vs. the backlog), so each filing skill can call it right after
  ``save_issue`` with no N×N re-scan. If A then B are filed, B's file-time check
  sees A and files the single symmetric link; A's earlier check simply didn't
  see B yet — the pair is always covered by the later filer.
* **Full sweep** (no ``--for``) — compares every pair, then reports three
  sections: **collision clusters** (issues grouped by the paths they share — the
  direct input to ``housekeeping``'s merge-group proposal), **semantic blocks**
  (the surviving human-declared ``blockedBy`` edges), and **smells** (the
  priority-inversion and Todo-blocks-Backlog checks, which now scan those
  human-declared edges only). Run it by hand to reconcile after backfilling a
  ``**Touches**:`` line on an *older* issue.
* **Report-only** (``--report-todo-blocks``) — the two smells as JSON, writing
  nothing.

Configuration comes entirely from the environment (no hard-coded ids, never a
committed token):

* ``LINEAR_API_KEY`` — a personal API key (the interactive claude.ai Linear MCP
  rides OAuth and won't authenticate from a script), sent verbatim as the
  ``Authorization`` header.
* ``LINEAR_PROJECT_ID`` — the Dropset project whose Backlog is swept.

Pass ``--dry-run`` to print the links it *would* file without writing anything.
Standard library only (``urllib`` + ``json``) — no third-party deps.
"""

from __future__ import annotations

import json
import os
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass, field

ENDPOINT = "https://api.linear.app/graphql"

# How many Backlog issues a single query reads. The Dropset Backlog is far
# under this; ``fetch_backlog`` errors rather than truncate if it's exceeded.
PAGE_SIZE = 250

# Overall per-request timeout, so a hung endpoint can't wedge a run.
REQUEST_TIMEOUT = 30


class SyncBlockersError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


# Linear's priority scale, as the API reports it. Note it is **inverted**:
# Urgent is the *lowest* number, and 0 means "No priority".
URGENT_PRIORITY = 1
PRIORITY_NAMES = {0: "No priority", 1: "Urgent", 2: "High", 3: "Medium", 4: "Low"}


def priority_name(value: int | None) -> str:
    """A human label for a Linear priority, for report and warning lines."""
    if value is None:
        return "unknown priority"
    return PRIORITY_NAMES.get(value, f"priority {value}")


def parse_priority(raw_value) -> int | None:
    """Coerce a raw GraphQL ``priority`` into an int, or ``None`` if unreadable.

    ``None`` means *unknown*, and is deliberately distinct from ``0``
    ("No priority") — because the scale is inverted, coercing an absent, null, or
    non-numeric value to ``0`` would make it read as "not Urgent".

    The priority is now read for **reporting only**. There is no filing-time
    priority floor left to disable, because this tool no longer files a blocking
    edge in any direction: a ``related`` link is symmetric and gates nothing, so
    there is no orientation for a priority inversion to get wrong.
    :func:`urgent_gated_by_non_urgent` still reports inversions among the
    human-declared edges, and an unknown priority simply isn't Urgent there.
    """
    if raw_value is None:
        return None
    try:
        return int(raw_value)
    except (TypeError, ValueError):
        return None


# --------------------------------------------------------------------------
# Model — the issue shape and the pure path-glob helpers the sweep builds on.
# --------------------------------------------------------------------------


@dataclass
class Issue:
    """One open Backlog issue, reduced to what relation maintenance needs."""

    id: str
    number: int
    uuid: str = ""  # Linear's internal UUID, needed to file a relation
    touches: list[str] = field(default_factory=list)
    blocked_by: list[str] = field(default_factory=list)
    blocks: list[str] = field(default_factory=list)
    # Identifiers this issue is already `related` to. A collision that is already
    # related-linked needs no second link, so the sweep is idempotent.
    related_to: list[str] = field(default_factory=list)
    # Linear's priority int (see PRIORITY_NAMES), or None when unreadable. Read
    # for reporting only — see `parse_priority`.
    priority: int | None = 0
    # (blocker ``ENG-###``, blocker state name) for each ``blockedBy`` blocker
    # whose workflow state is the *Todo* (``unstarted``) type. Every issue here
    # is itself Backlog (the fetch filters to it), so a populated list is a
    # Todo→Backlog block — the scheduling smell the report mode surfaces.
    todo_blockers: list[tuple[str, str]] = field(default_factory=list)
    # (blocker ``ENG-###``, blocker priority) for every declared blocker, so an
    # already-filed priority inversion can be reported even though this tool
    # would no longer create one. The priority may be None (unknown).
    blocked_by_priority: list[tuple[str, int | None]] = field(default_factory=list)


def parse_number(ident: str) -> int | None:
    """Parse the trailing number out of an ``ENG-###`` identifier."""
    tail = ident.rsplit("-", 1)[-1]
    try:
        return int(tail)
    except ValueError:
        return None


def _strip_field_prefix(line: str, field_name: str) -> str | None:
    """Strip a structured-field prefix (``**Touches**:``) from a line,
    tolerating a single leading list marker (``- `` / ``* ``) and surrounding
    whitespace, and return the remainder. ``None`` when the line isn't that
    field."""
    s = line.strip()
    if s.startswith("- "):
        s = s[2:]
    elif s.startswith("* "):
        s = s[2:]
    if s.startswith(field_name):
        return s[len(field_name) :]
    return None


def parse_touches(description: str) -> list[str]:
    """Pull every glob off an issue description's ``**Touches**:`` line(s). A
    line is ``**Touches**: glob1, glob2, …``; globs are comma-separated,
    trimmed, and stripped of surrounding backticks. Multiple ``**Touches**:``
    lines union."""
    out: list[str] = []
    for line in description.splitlines():
        rest = _strip_field_prefix(line, "**Touches**:")
        if rest is None:
            continue
        for glob in rest.split(","):
            g = glob.strip().strip("`").strip()
            if g:
                out.append(g)
    return out


def normalize_glob(glob: str) -> str:
    """Reduce a glob to a comparable path prefix: drop a trailing ``/**`` or
    ``/*`` and any trailing slash, so ``sdk/rs/**`` and ``sdk/rs/`` both become
    ``sdk/rs``."""
    g = glob.strip()
    while g.startswith("./"):
        g = g[2:]
    if g.endswith("/**"):
        g = g[:-3]
    if g.endswith("/*"):
        g = g[:-2]
    return g.rstrip("/")


def is_path_prefix(a: str, b: str) -> bool:
    """True when ``a`` is ``b`` or a path-segment ancestor of ``b`` (``sdk`` is
    a prefix of ``sdk/rs``, but ``sd`` is not)."""
    return b == a or b.startswith(a + "/")


def overlapping_paths(a: Issue, b: Issue) -> list[str]:
    """The paths two issues' file sets actually collide on, deduped and sorted.

    Two globs collide when one normalizes to the same path as, or a path-segment
    ancestor/descendant of, the other. The colliding *region* is the **deeper** of
    the two — ``tui/**`` against ``tui/pane.rs`` collide on ``tui/pane.rs``, not
    on all of ``tui`` — so that is what's named. Reporting the region rather than
    a bare boolean is what lets a collision line say where the two issues meet,
    and what lets the sweep group issues into clusters by shared path.
    """
    shared: set[str] = set()
    for ga in a.touches:
        na = normalize_glob(ga)
        if not na:
            continue
        for gb in b.touches:
            nb = normalize_glob(gb)
            if not nb:
                continue
            if is_path_prefix(na, nb):
                shared.add(nb)
            elif is_path_prefix(nb, na):
                shared.add(na)
    return sorted(shared)


def touches_overlap(a: Issue, b: Issue) -> bool:
    """Whether two issues' file sets collide at all — ``overlapping_paths`` as a
    predicate."""
    return bool(overlapping_paths(a, b))


def missing_touches(issues: list[Issue]) -> list[str]:
    """Identifiers of issues that have no ``**Touches**:`` field — they can't be
    checked for file overlap, so the caller warns."""
    return [i.id for i in issues if not i.touches]


def todo_blocks_backlog(issues: list[Issue]) -> list[tuple[str, str, str]]:
    """Flag the scheduling smell where a **Todo** (``unstarted``) issue blocks a
    **Backlog** issue: the blocked item sits in the pull queue but can't actually
    be started because a not-yet-pulled or initiative-level item gates it (per
    the Todo/Backlog convention). Every issue in ``issues`` is Backlog (the fetch
    filters to it), so this just walks each one's Todo-state blockers.

    Returns ``(blocker_id, blocker_state_name, blocked_backlog_id)`` triples,
    sorted by blocked then blocker for a stable report. Read-only — the caller
    resolves each pair (move the blocker into Backlog, drop a stale edge, or
    re-prioritize); this never writes.
    """
    pairs: list[tuple[str, str, str]] = []
    for i in issues:
        for blocker_id, blocker_state in i.todo_blockers:
            pairs.append((blocker_id, blocker_state, i.id))
    pairs.sort(key=lambda p: (parse_number(p[2]) or 0, parse_number(p[0]) or 0))
    return pairs


def urgent_gated_by_non_urgent(issues: list[Issue]) -> list[tuple[str, str, str]]:
    """Flag every **declared** edge where a non-Urgent blocker gates an Urgent
    issue — the priority inversion the sweep below now refuses to create.

    An Urgent Backlog issue is meant to be pullable now; an edge from a Medium
    feature makes it unpullable until that feature ships. The sweep declines to
    file such an edge, but edges filed before this guard existed (or added by
    hand) survive, so this reports them for a human to resolve — usually by
    dropping the edge, or by reversing it so the Urgent fix lands first.

    Returns ``(blocker_id, blocker_priority_name, blocked_urgent_id)`` triples,
    sorted by blocked then blocker. Read-only — this never writes.
    """
    pairs: list[tuple[str, str, str]] = []
    for i in issues:
        if i.priority != URGENT_PRIORITY:
            continue
        for blocker_id, blocker_priority in i.blocked_by_priority:
            if blocker_priority != URGENT_PRIORITY:
                pairs.append((blocker_id, priority_name(blocker_priority), i.id))
    pairs.sort(key=lambda p: (parse_number(p[2]) or 0, parse_number(p[0]) or 0))
    return pairs


# --------------------------------------------------------------------------
# The sweep — materialize undeclared file-overlaps into ``related`` relations.
# --------------------------------------------------------------------------


def _pair_sort_key(pair: tuple[str, str]) -> tuple[int, int]:
    """Sort an ``(id, id)`` edge pair by each side's ``ENG-###`` number."""
    return (parse_number(pair[0]) or 0, parse_number(pair[1]) or 0)


def _already_linked(issues: list[Issue]) -> set[frozenset[str]]:
    """Every pair of open Backlog issues that already carries *some* relation.

    A pair linked in any direction — a human-declared ``blocks`` either way, or
    an existing ``related`` — needs no new link, which is what makes the sweep
    idempotent. A declared block already expresses the coupling more strongly
    than a related link would, so stacking one on top of it is pure noise.
    """
    universe = {i.id for i in issues}
    linked: set[frozenset[str]] = set()
    for i in issues:
        for other in (*i.blocked_by, *i.blocks, *i.related_to):
            if other in universe:
                linked.add(frozenset((i.id, other)))
    return linked


def materialize_overlap_relations(
    issues: list[Issue],
    api_key: str | None,
    dry_run: bool,
    focus_id: str | None = None,
) -> list[tuple[str, str, list[str]]]:
    """Turn each unlinked file-overlap into a real Linear ``related`` relation.

    For every pair of Backlog issues whose ``**Touches**:`` globs collide and
    that carry no relation yet, file a ``related`` link and report the paths they
    collide on. A file collision means the two issues would touch the same code —
    useful to see, and the input to a merge-group proposal — but it costs at most
    a rebase, so a symmetric related link is the honest representation of it.

    **It deliberately files no blocking edge.** Ordering work is a scheduling
    decision a human makes (see the module docstring): a spurious block drops an
    issue out of the board's available set, which costs strictly more than the
    rebase a missed collision costs. There is correspondingly no priority floor
    here — a ``related`` link is symmetric and gates nothing, so it has no
    orientation to get wrong.

    Returns ``(a, b, shared_paths)`` triples, lower ``ENG-###`` first purely so
    output is stable. Under ``dry_run`` the list is what *would* be filed.

    When ``focus_id`` is given (incremental mode), only pairs that *include* that
    issue are considered — the bounded one-vs-backlog check a filing skill runs
    right after ``save_issue``.
    """
    linked = _already_linked(issues)

    filed: list[tuple[str, str, list[str]]] = []
    n = len(issues)
    for a in range(n):
        for c in range(a + 1, n):
            ia, ic = issues[a], issues[c]
            if focus_id is not None and focus_id not in (ia.id, ic.id):
                continue
            shared = overlapping_paths(ia, ic)
            if not shared:
                continue
            pair = frozenset((ia.id, ic.id))
            if pair in linked:
                continue
            linked.add(pair)
            lo, hi = (ia, ic) if ia.number <= ic.number else (ic, ia)
            filed.append((lo.id, hi.id, shared))
            if not dry_run:
                issue_relation_create(api_key, lo.uuid, hi.uuid, "related")
    filed.sort(key=lambda t: _pair_sort_key((t[0], t[1])))
    return filed


def collision_clusters(issues: list[Issue]) -> list[tuple[list[str], list[str]]]:
    """Group the open Backlog by the **paths** its issues collide on.

    Returns ``(member_ids, shared_paths)`` per cluster of two or more issues,
    members sorted by ``ENG-###`` and clusters sorted by their lowest member.
    This is the direct input to ``housekeeping``'s merge-group proposal step: a
    cluster is the candidate set for "these would land as one PR".

    **Grouped per shared path, deliberately not by connected component.** The
    transitive reading is the intuitive one and it is useless here: run over the
    real Backlog it put 25 of 27 issues in a single cluster, because coupling
    chains through shared files (everything touches a ``Cargo.toml``, several
    things touch ``bots/maker-bot``). A cluster that says "merge everything" is
    no proposal at all — and the coherence floor forbids it anyway, since a
    component spans separate apps and languages.

    Keying on the path instead yields small, actionable groups: *these three
    issues all touch ``bots/maker-bot/src/model/feeds.rs``*. Paths whose member
    set is identical are merged into one entry listing both paths, so the same
    group isn't proposed twice under two names.

    One consequence worth stating: an issue appears in **every** cluster whose
    path it touches, so the clusters overlap. That is correct for a proposal —
    the reader picks which grouping to act on — but it means the member lists do
    not partition the Backlog.
    """
    order = sorted(issues, key=lambda i: i.number)

    # path -> the issues that collide on it. Accumulated from the pairwise
    # collisions, so it uses exactly the same predicate as the sweep.
    by_path: dict[str, set[str]] = {}
    for a in range(len(order)):
        for b in range(a + 1, len(order)):
            for path in overlapping_paths(order[a], order[b]):
                members = by_path.setdefault(path, set())
                members.add(order[a].id)
                members.add(order[b].id)

    # Merge paths that produce the same member set — one group, several names.
    by_members: dict[frozenset[str], list[str]] = {}
    for path, members in by_path.items():
        if len(members) < 2:
            continue
        by_members.setdefault(frozenset(members), []).append(path)

    clusters = [
        (
            sorted(members, key=lambda x: parse_number(x) or 0),
            sorted(paths),
        )
        for members, paths in by_members.items()
    ]
    # Lowest member first, then by size (a wider group is the bigger proposal),
    # then by path so the order is fully determined.
    clusters.sort(
        key=lambda c: (parse_number(c[0][0]) or 0, -len(c[0]), c[1][0] if c[1] else "")
    )
    return clusters


def semantic_blocks(issues: list[Issue]) -> list[tuple[str, str]]:
    """Every ``blocks`` edge visible from the open Backlog, as ``(blocker,
    blocked)`` pairs sorted by blocker then blocked.

    With no automated writer filing one, every surviving edge is human-declared,
    so this section of the sweep report *is* the intended scheduling order — worth
    printing so a reader can sanity-check it against the collision clusters.
    Collected from both directions, so an edge whose blocker sits outside the open
    Backlog (a Todo-state initiative, say) is still listed.
    """
    pairs: set[tuple[str, str]] = set()
    for i in issues:
        for blocked in i.blocks:
            pairs.add((i.id, blocked))
        for blocker in i.blocked_by:
            pairs.add((blocker, i.id))
    return sorted(pairs, key=_pair_sort_key)


# --------------------------------------------------------------------------
# Linear client — the two GraphQL calls the tool needs.
# --------------------------------------------------------------------------

BACKLOG_QUERY = """
query Backlog($projectId: ID!, $first: Int!) {
  issues(
    filter: { project: { id: { eq: $projectId } }, state: { type: { eq: "backlog" } } }
    first: $first
  ) {
    pageInfo { hasNextPage }
    nodes {
      id
      identifier
      description
      priority
      relations { nodes { type relatedIssue { identifier } } }
      inverseRelations {
        nodes { id type issue { identifier priority state { name type } } }
      }
    }
  }
}
"""

# The relation type is a variable rather than inlined so the mutation stays
# general; every call site passes `related`, the only type this tool writes.
# `IssueRelationType` is Linear's enum (`blocks`, `related`, …).
ISSUE_RELATION_CREATE_MUTATION = """
mutation CreateRelation(
  $issueId: String!
  $relatedIssueId: String!
  $type: IssueRelationType!
) {
  issueRelationCreate(
    input: { type: $type, issueId: $issueId, relatedIssueId: $relatedIssueId }
  ) { success }
}
"""


def _post(api_key: str, query: str, variables: dict) -> dict:
    """POST a GraphQL operation and return its ``data``, surfacing transport
    and GraphQL-level errors with their messages."""
    body = json.dumps({"query": query, "variables": variables}).encode("utf-8")
    req = urllib.request.Request(
        ENDPOINT,
        data=body,
        headers={"Authorization": api_key, "Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=REQUEST_TIMEOUT) as resp:
            raw = resp.read().decode("utf-8")
    except urllib.error.HTTPError as e:
        detail = e.read().decode("utf-8", errors="replace")
        raise SyncBlockersError(f"Linear API returned HTTP {e.code}: {detail}") from e
    except urllib.error.URLError as e:
        raise SyncBlockersError(f"Linear API request failed: {e.reason}") from e

    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError as e:
        raise SyncBlockersError(f"decoding Linear GraphQL response: {e}") from e

    errors = parsed.get("errors")
    if errors:
        joined = "; ".join(e.get("message", "") for e in errors)
        raise SyncBlockersError(f"Linear GraphQL error: {joined}")
    data = parsed.get("data")
    if data is None:
        raise SyncBlockersError("Linear GraphQL response carried no data")
    return data


def _raw_to_issue(raw: dict) -> Issue:
    """Map a raw GraphQL issue into the sweep's :class:`Issue`."""
    blocks = [
        r["relatedIssue"]["identifier"]
        for r in raw["relations"]["nodes"]
        if r.get("type") == "blocks" and r.get("relatedIssue")
    ]
    blocked_by = [
        r["issue"]["identifier"]
        for r in raw["inverseRelations"]["nodes"]
        if r.get("type") == "blocks" and r.get("issue")
    ]
    # `related` is symmetric, but Linear still stores it one-directional, so both
    # sides have to be read or the sweep re-files a link it already made.
    related_to = [
        r["relatedIssue"]["identifier"]
        for r in raw["relations"]["nodes"]
        if r.get("type") == "related" and r.get("relatedIssue")
    ] + [
        r["issue"]["identifier"]
        for r in raw["inverseRelations"]["nodes"]
        if r.get("type") == "related" and r.get("issue")
    ]
    todo_blockers = [
        (r["issue"]["identifier"], (r["issue"].get("state") or {}).get("name") or "")
        for r in raw["inverseRelations"]["nodes"]
        if r.get("type") == "blocks"
        and r.get("issue")
        and (r["issue"].get("state") or {}).get("type") == "unstarted"
    ]
    blocked_by_priority = [
        (r["issue"]["identifier"], parse_priority(r["issue"].get("priority")))
        for r in raw["inverseRelations"]["nodes"]
        if r.get("type") == "blocks" and r.get("issue")
    ]
    description = raw.get("description") or ""
    touches = parse_touches(description)
    ident = raw["identifier"]
    return Issue(
        id=ident,
        number=parse_number(ident) or 0,
        uuid=raw.get("id") or "",
        touches=touches,
        blocked_by=blocked_by,
        blocks=blocks,
        related_to=related_to,
        priority=parse_priority(raw.get("priority")),
        todo_blockers=todo_blockers,
        blocked_by_priority=blocked_by_priority,
    )


def fetch_backlog(api_key: str, project_id: str) -> list[Issue]:
    """All open Backlog issues for the project, distilled into :class:`Issue`s.

    Reads one page (``PAGE_SIZE``); rather than silently sweep a truncated set,
    it refuses if the project has more.
    """
    data = _post(api_key, BACKLOG_QUERY, {"projectId": project_id, "first": PAGE_SIZE})
    conn = data["issues"]
    if conn["pageInfo"]["hasNextPage"]:
        raise SyncBlockersError(
            f"project has more than {PAGE_SIZE} open Backlog issues; pagination "
            "is not implemented, so refusing to sweep a truncated set"
        )
    return [_raw_to_issue(n) for n in conn["nodes"]]


def issue_relation_create(
    api_key: str,
    issue_uuid: str,
    related_uuid: str,
    relation_type: str = "related",
) -> None:
    """File a relation between two issues, by Linear internal UUID.

    Defaults to ``related`` — the only type this tool's sweep writes. ``blocks``
    is reachable through the parameter, but no code path here passes it: blocking
    edges are human-curated (see the module docstring).
    """
    data = _post(
        api_key,
        ISSUE_RELATION_CREATE_MUTATION,
        {
            "issueId": issue_uuid,
            "relatedIssueId": related_uuid,
            "type": relation_type,
        },
    )
    if not data["issueRelationCreate"]["success"]:
        raise SyncBlockersError("Linear issueRelationCreate returned success=false")


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------

HELP = """\
Usage:
  sync_blockers.py [--dry-run]
      Full sweep: relate every unlinked file-overlap in the open Dropset
      Backlog, then report collision clusters, semantic blocks, and smells.
  sync_blockers.py --for ENG-### [--dry-run]
      Incremental: relate the named (just-filed) issue's collisions, naming the
      paths each one collides on.
  sync_blockers.py --report-todo-blocks
      Report-only: print, as JSON, the two scheduling smells — every Todo-state
      issue that blocks an open Backlog issue, and every non-Urgent issue that
      blocks an Urgent one. Writes nothing; cannot combine with --for.

This tool never files, rewrites, or removes a blocking edge. Blocking is
human-curated: an agent may suggest an edge with its evidence, and a human
approves or places it.

  --dry-run  Print the links that would be filed; write nothing."""


def env_var(name: str) -> str:
    """Read a required, non-empty environment variable."""
    value = os.environ.get(name)
    if value is None:
        raise SyncBlockersError(f"{name} is not set")
    if not value.strip():
        raise SyncBlockersError(f"{name} is empty")
    return value


@dataclass(frozen=True)
class Options:
    """The parsed CLI surface. Three mutually-exclusive modes plus ``--dry-run``."""

    dry_run: bool = False
    focus_id: str | None = None
    report_todo: bool = False


def _parse_args(args: list[str]) -> Options:
    """Parse the CLI args into :class:`Options`, or raise on a bad one.

    The mode flags are mutually exclusive. Rejecting a bad combination outright,
    rather than letting one flag silently win, matters here because every mode
    differs in what it *writes*.
    """
    dry_run = False
    focus_id: str | None = None
    report_todo = False
    i = 0
    while i < len(args):
        arg = args[i]
        if arg == "--dry-run":
            dry_run = True
        elif arg == "--report-todo-blocks":
            report_todo = True
        elif arg == "--for":
            i += 1
            if i >= len(args):
                raise SyncBlockersError("--for requires an ENG-### argument")
            focus_id = args[i].upper()
        else:
            raise SyncBlockersError(f"unknown argument: {arg} (try --help)")
        i += 1
    if report_todo and focus_id is not None:
        raise SyncBlockersError("--report-todo-blocks cannot combine with --for")
    return Options(
        dry_run=dry_run,
        focus_id=focus_id,
        report_todo=report_todo,
    )


def _print_sweep_report(issues: list[Issue]) -> None:
    """The full sweep's three report sections, to stderr.

    Text rather than JSON because every consumer is a reader (a human, or the
    model driving ``housekeeping``'s merge-group step); the machine-readable
    smells contract stays in ``--report-todo-blocks``, which housekeeping already
    calls.
    """
    clusters = collision_clusters(issues)
    print("", file=sys.stderr)
    print("collision clusters (candidate merge groups):", file=sys.stderr)
    if not clusters:
        print("  none — no two open issues touch the same paths", file=sys.stderr)
    for members, shared in clusters:
        print(
            f"  {', '.join(members)} — collide on {', '.join(shared)}",
            file=sys.stderr,
        )

    blocks = semantic_blocks(issues)
    print("", file=sys.stderr)
    print("semantic blocks (human-declared — this tool files none):", file=sys.stderr)
    if not blocks:
        print("  none declared", file=sys.stderr)
    for blocker, blocked in blocks:
        print(f"  {blocker} blocks {blocked}", file=sys.stderr)

    todo_pairs = todo_blocks_backlog(issues)
    inversions = urgent_gated_by_non_urgent(issues)
    print("", file=sys.stderr)
    print("smells:", file=sys.stderr)
    if not todo_pairs and not inversions:
        print("  none", file=sys.stderr)
    for blocker, state, blocked in todo_pairs:
        print(
            f"  {blocker} ({state}) blocks Backlog {blocked} — the pullable item "
            f"can't start behind a not-yet-pulled one",
            file=sys.stderr,
        )
    for blocker, blocker_priority, blocked in inversions:
        print(
            f"  {blocker} ({blocker_priority}) blocks Urgent {blocked} — usually "
            f"wants the reverse edge so the Urgent work lands first",
            file=sys.stderr,
        )


def run(argv: list[str]) -> int:
    args = argv[1:]
    if any(a in ("-h", "--help") for a in args):
        print(HELP)
        return 0

    opts = _parse_args(args)
    dry_run, focus_id, report_todo = opts.dry_run, opts.focus_id, opts.report_todo

    api_key = env_var("LINEAR_API_KEY")
    project_id = env_var("LINEAR_PROJECT_ID")

    issues = fetch_backlog(api_key, project_id)

    if report_todo:
        pairs = todo_blocks_backlog(issues)
        inversions = urgent_gated_by_non_urgent(issues)
        print(
            json.dumps(
                {
                    "todo_blocks_backlog": [
                        {"blocker": b, "blocker_state": s, "blocked": d}
                        for b, s, d in pairs
                    ],
                    "urgent_gated_by_non_urgent": [
                        {"blocker": b, "blocker_priority": p, "blocked_urgent": d}
                        for b, p, d in inversions
                    ],
                },
                indent=2,
            )
        )
        return 0

    if focus_id is not None and focus_id not in {i.id for i in issues}:
        raise SyncBlockersError(
            f"{focus_id} is not an open Backlog issue (nothing to sync)"
        )

    for ident in missing_touches(issues):
        # In focus mode only the focus issue's missing field matters.
        if focus_id is not None and ident != focus_id:
            continue
        print(
            f"warning: {ident} has no **Touches**: field; can't check it for "
            "file overlap — backfill one so its edges are maintained",
            file=sys.stderr,
        )

    filed = materialize_overlap_relations(issues, api_key, dry_run, focus_id)
    verb = "would relate" if dry_run else "related-linked"
    for lo, hi, shared in filed:
        print(f"{verb}: {lo} ~ {hi} (overlaps on {', '.join(shared)})", file=sys.stderr)

    # A priority that wouldn't parse no longer suppresses anything — nothing here
    # gates on it — but the smells report reads it, so an unreadable field would
    # silently under-report inversions. Say so rather than look like a clean run.
    unreadable = sorted(i.id for i in issues if i.priority is None)
    if unreadable:
        print(
            f"warning: {len(unreadable)} issue(s) reported no readable priority "
            f"({', '.join(unreadable)}) — the smells report can't judge these for "
            f"a priority inversion. Check the GraphQL `priority` field is still "
            f"being returned.",
            file=sys.stderr,
        )

    # The three report sections are a reconciliation view over the whole board,
    # so they'd be misleading after a bounded one-vs-backlog check.
    if focus_id is None:
        _print_sweep_report(issues)

    marker = " (dry-run)" if dry_run else ""
    noun = "link" if len(filed) == 1 else "links"
    tail = "would be filed" if dry_run else "filed"
    scope = focus_id if focus_id is not None else f"{len(issues)} backlog issues"
    print(f"sync-blockers{marker} | {scope} | {len(filed)} collision {noun} {tail}")
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except SyncBlockersError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
