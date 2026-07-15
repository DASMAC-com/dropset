#!/usr/bin/env python3
"""Keep the Dropset Linear Backlog's blocking edges in sync with file overlap.

This is the deterministic core of the ``sync-blockers`` skill. Its one job is
**edge maintenance**: given the open Backlog's ``**Touches**:`` globs and its
declared ``blockedBy`` / ``blocks`` relations, find every file-overlap collision
that has *no* declared edge and materialize it into a real Linear ``blocks``
relation (lower ``ENG-###`` blocks higher). Linear's native blocking icons —
not a rendered document — are the source of truth for what gates what; this tool
just makes sure an undeclared same-file collision shows up as one of those
icons. It never renders or writes a document, and it never merges or closes
issues.

Two modes:

* **Incremental** (``--for ENG-###``) — the file-time path. Compares *only* the
  named, just-filed issue's touches against the rest of the open Backlog and
  files its overlap edges. Bounded work (one node vs. the backlog), so each
  filing skill can call it right after ``save_issue`` with no N×N re-scan. If A
  then B are filed, B's file-time check sees A and files the single symmetric
  edge; A's earlier check simply didn't see B yet — the pair is always covered
  by the later filer.
* **Full sweep** (no ``--for``) — compares every pair. No longer load-bearing
  cadence; run it by hand to reconcile after backfilling a ``**Touches**:`` line
  on an *older* issue, or as an occasional catch-up.

Configuration comes entirely from the environment (no hard-coded ids, never a
committed token):

* ``LINEAR_API_KEY`` — a personal API key (the interactive claude.ai Linear MCP
  rides OAuth and won't authenticate from a script), sent verbatim as the
  ``Authorization`` header.
* ``LINEAR_PROJECT_ID`` — the Dropset project whose Backlog is swept.

Pass ``--dry-run`` to print the edges it *would* file without writing anything.
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


# --------------------------------------------------------------------------
# Model — the issue shape and the pure path-glob helpers the sweep builds on.
# --------------------------------------------------------------------------


@dataclass
class Issue:
    """One open Backlog issue, reduced to what edge maintenance needs."""

    id: str
    number: int
    uuid: str = ""  # Linear's internal UUID, needed to file a relation
    touches: list[str] = field(default_factory=list)
    blocked_by: list[str] = field(default_factory=list)
    blocks: list[str] = field(default_factory=list)
    # (blocker ``ENG-###``, blocker state name) for each ``blockedBy`` blocker
    # whose workflow state is the *Todo* (``unstarted``) type. Every issue here
    # is itself Backlog (the fetch filters to it), so a populated list is a
    # Todo→Backlog block — the scheduling smell the report mode surfaces.
    todo_blockers: list[tuple[str, str]] = field(default_factory=list)


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


def touches_overlap(a: Issue, b: Issue) -> bool:
    """Two issues' file sets overlap when any normalized touch-glob of one is
    the same path as, or an ancestor/descendant of, a touch-glob of the
    other."""
    for ga in a.touches:
        na = normalize_glob(ga)
        if not na:
            continue
        for gb in b.touches:
            nb = normalize_glob(gb)
            if not nb:
                continue
            if is_path_prefix(na, nb) or is_path_prefix(nb, na):
                return True
    return False


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


# --------------------------------------------------------------------------
# The sweep — materialize undeclared file-overlaps into ``blocks`` relations.
# --------------------------------------------------------------------------


def materialize_overlap_edges(
    issues: list[Issue],
    api_key: str | None,
    dry_run: bool,
    focus_id: str | None = None,
) -> list[tuple[str, str]]:
    """Turn each undeclared file-overlap into a real Linear ``blocks`` relation.

    For every pair of Backlog issues whose ``**Touches**:`` globs collide and
    that have **no** declared edge in either direction, file the lower-numbered
    issue ``blocks`` the higher-numbered one — materializing the scheduling
    constraint as a tagged edge so Linear's blocking icons reflect the
    same-file collision. Returns the ``(lower, higher)`` id pairs filed (or that
    would be filed under ``--dry-run``), lowest-first, for the caller to report.
    A pair already linked (declared, or filed earlier in this pass) is skipped,
    so the relation is never duplicated.

    When ``focus_id`` is given (incremental mode), only pairs that *include*
    that issue are considered — the bounded one-vs-backlog check a filing skill
    runs right after ``save_issue``.
    """
    universe = {i.id for i in issues}
    linked: set[frozenset[str]] = set()
    for i in issues:
        for b in i.blocked_by:
            if b in universe:
                linked.add(frozenset((i.id, b)))
        for b in i.blocks:
            if b in universe:
                linked.add(frozenset((i.id, b)))

    filed: list[tuple[str, str]] = []
    n = len(issues)
    for a in range(n):
        for c in range(a + 1, n):
            ia, ic = issues[a], issues[c]
            if focus_id is not None and focus_id not in (ia.id, ic.id):
                continue
            if not touches_overlap(ia, ic):
                continue
            pair = frozenset((ia.id, ic.id))
            if pair in linked:
                continue
            lo, hi = (ia, ic) if ia.number <= ic.number else (ic, ia)
            filed.append((lo.id, hi.id))
            linked.add(pair)
            if not dry_run:
                issue_relation_create(api_key, lo.uuid, hi.uuid)
    filed.sort(key=lambda p: (parse_number(p[0]) or 0, parse_number(p[1]) or 0))
    return filed


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
      relations { nodes { type relatedIssue { identifier } } }
      inverseRelations {
        nodes { type issue { identifier state { name type } } }
      }
    }
  }
}
"""

ISSUE_RELATION_CREATE_MUTATION = """
mutation CreateBlocks($issueId: String!, $relatedIssueId: String!) {
  issueRelationCreate(
    input: { type: blocks, issueId: $issueId, relatedIssueId: $relatedIssueId }
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
    todo_blockers = [
        (r["issue"]["identifier"], (r["issue"].get("state") or {}).get("name") or "")
        for r in raw["inverseRelations"]["nodes"]
        if r.get("type") == "blocks"
        and r.get("issue")
        and (r["issue"].get("state") or {}).get("type") == "unstarted"
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
        todo_blockers=todo_blockers,
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


def issue_relation_create(api_key: str, issue_uuid: str, related_uuid: str) -> None:
    """File a ``blocks`` relation: ``issue_uuid`` blocks ``related_uuid`` (both
    Linear internal UUIDs)."""
    data = _post(
        api_key,
        ISSUE_RELATION_CREATE_MUTATION,
        {"issueId": issue_uuid, "relatedIssueId": related_uuid},
    )
    if not data["issueRelationCreate"]["success"]:
        raise SyncBlockersError("Linear issueRelationCreate returned success=false")


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------

HELP = """\
Usage:
  sync_blockers.py [--dry-run]
      Full sweep: file a blocks edge for every undeclared file-overlap in the
      open Dropset Backlog.
  sync_blockers.py --for ENG-### [--dry-run]
      Incremental: file overlap edges for just the named (just-filed) issue.
  sync_blockers.py --report-todo-blocks
      Report-only: print, as JSON, every Todo-state issue that blocks an open
      Backlog issue (a scheduling smell). Writes nothing; cannot combine with
      --for.
  --dry-run  Print the edges that would be filed; write nothing."""


def env_var(name: str) -> str:
    """Read a required, non-empty environment variable."""
    value = os.environ.get(name)
    if value is None:
        raise SyncBlockersError(f"{name} is not set")
    if not value.strip():
        raise SyncBlockersError(f"{name} is empty")
    return value


def _parse_args(args: list[str]) -> tuple[bool, str | None, bool]:
    """Return ``(dry_run, focus_id, report_todo)`` from the CLI args, or raise on
    a bad one. ``focus_id`` is the ``ENG-###`` from ``--for`` in upper case (else
    ``None``); ``report_todo`` is the ``--report-todo-blocks`` report-only mode.
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
    return dry_run, focus_id, report_todo


def run(argv: list[str]) -> int:
    args = argv[1:]
    if any(a in ("-h", "--help") for a in args):
        print(HELP)
        return 0

    dry_run, focus_id, report_todo = _parse_args(args)

    api_key = env_var("LINEAR_API_KEY")
    project_id = env_var("LINEAR_PROJECT_ID")

    issues = fetch_backlog(api_key, project_id)

    if report_todo:
        pairs = todo_blocks_backlog(issues)
        print(
            json.dumps(
                {
                    "todo_blocks_backlog": [
                        {"blocker": b, "blocker_state": s, "blocked": d}
                        for b, s, d in pairs
                    ]
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

    filed = materialize_overlap_edges(issues, api_key, dry_run, focus_id)
    verb = "would file" if dry_run else "filed"
    for lo, hi in filed:
        print(f"{verb}: {lo} blocks {hi} (touch overlap)", file=sys.stderr)

    marker = " (dry-run)" if dry_run else ""
    noun = "edge" if len(filed) == 1 else "edges"
    tail = "would be filed" if dry_run else "filed"
    scope = focus_id if focus_id is not None else f"{len(issues)} backlog issues"
    print(f"sync-blockers{marker} | {scope} | {len(filed)} overlap {noun} {tail}")
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except SyncBlockersError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
