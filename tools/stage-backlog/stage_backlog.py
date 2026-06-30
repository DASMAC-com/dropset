#!/usr/bin/env python3
"""Render the Dropset Linear Backlog as the chips-only Task Staging tree.

This is the deterministic core of the ``stage-backlog`` skill: read the
project's open Backlog (with parents, declared blocking relations, and the
state of every issue those relations reach), build the dependency tree from
those edges alone, and write the rendered tree to the Task Staging document.

Blocking is **edge-driven**: every blocker is a tagged Linear relation. A
``**Touches**:`` file-overlap between two Backlog issues with no declared edge
is *materialized* into a real ``blocks`` relation (lower number blocks higher)
before the tree is built, rather than inferred in memory — so the staged tree
and Linear never disagree. A blocker is honoured until it reaches a terminal
state (``completed`` / ``canceled``), so a Backlog issue gated by an
In-Progress / In-Review issue keeps showing that blocker as a tag instead of
silently dropping it. The whole path is mechanical — no model judgment, no
issue folding, one Linear read, the overlap-edge writes, and (on a real run)
the document write.

Configuration comes entirely from the environment (no hard-coded ids, never a
committed token):

* ``LINEAR_API_KEY`` — a personal API key (the interactive claude.ai Linear MCP
  rides OAuth and won't authenticate from a script), sent verbatim as the
  ``Authorization`` header.
* ``LINEAR_PROJECT_ID`` — the Dropset project whose Backlog is staged.
* ``LINEAR_TASK_STAGING_DOC_ID`` — the document rewritten each run (not needed
  for ``--dry-run``).

Pass ``--dry-run`` to print the rendered tree to stdout without writing the
document. Standard library only (``urllib`` + ``json``) — no third-party deps.
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

# Overall per-request timeout, so a hung endpoint can't wedge a ``make`` run.
REQUEST_TIMEOUT = 30

INDENT = "    "  # four spaces per nesting level

# Path bases (besides the file CLAUDE.md) that count as agent-infra "meta-work"
# — the surface the ``Claude:`` issue-title prefix batches together.
META_BASES = (".claude", "docs/conventions", "tools")


class StageBacklogError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


# --------------------------------------------------------------------------
# Model — the issue shape and the pure path-glob helpers the planner builds on.
# --------------------------------------------------------------------------


@dataclass
class Issue:
    """One open Backlog issue, reduced to what the planner needs."""

    id: str
    number: int
    uuid: str = ""  # Linear's internal UUID, needed to file a relation
    title: str = ""
    parent: str | None = None
    touches: list[str] = field(default_factory=list)
    blocked_by: list[str] = field(default_factory=list)
    blocks: list[str] = field(default_factory=list)

    def has_claude_prefix(self) -> bool:
        """True when the title carries the ``Claude:`` meta-work prefix (capital
        C, colon, space) — the deterministic signal that batches agent-infra
        work under the ``# Claude`` heading (see
        ``docs/conventions/linear-automation.md`` → "The ``Claude:`` meta-work
        prefix")."""
        return self.title.startswith("Claude: ")

    def is_meta_only(self) -> bool:
        """True when the issue touches **only** the agent-infra surface
        (``.claude/**``, ``CLAUDE.md``, ``docs/conventions/**``, ``tools/**``),
        with no product code — so it *should* carry the ``Claude:`` prefix. An
        issue with no ``touches`` is never meta-only (we can't prove it)."""
        return bool(self.touches) and all(is_meta_glob(g) for g in self.touches)


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


def is_meta_glob(glob: str) -> bool:
    """A glob counts as **meta-work** (agent-infra) when it names ``CLAUDE.md``
    or sits under ``.claude/``, ``docs/conventions/``, or ``tools/`` — the
    surface the ``Claude:`` prefix batches. A glob outside all of these is
    product / on-chain / SDK / frontend code."""
    g = glob
    while g.startswith("./"):
        g = g[2:]
    g = g.rstrip("/")
    if g == "CLAUDE.md":
        return True
    return any(g == base or g.startswith(base + "/") for base in META_BASES)


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


# --------------------------------------------------------------------------
# Planner — from a set of Issues, render the chips-only Task Staging tree.
# --------------------------------------------------------------------------
#
# * Issues bucket under ``# Skills`` (pure skill-suite work), a ``# ENG-###``
#   heading per parent with 2+ Backlog subtasks, or a trailing ``# Standalone``.
# * Within a bucket, issues nest by blocker. Blocking is edge-driven: every
#   blocker is a declared ``blockedBy`` / ``blocks`` relation (a file overlap
#   is materialized into one such relation upstream, before the planner runs).
# * A blocker the nesting can't show — one under a different heading, a sibling,
#   or a *live external* (non-Backlog, not-yet-resolved) issue that has no chip
#   of its own — renders as a trailing bare-tag note ``(ENG-X, ENG-Y)``.
#
# The render is a pure function of its input and fully deterministic: all
# iteration that reaches the output is sorted by issue number.

# A bucket is a tuple: ("claude",), ("parent", "ENG-40"), or ("standalone",).


def missing_touches(issues: list[Issue]) -> list[str]:
    """Identifiers of issues that have no ``**Touches**:`` field — the planner
    can place them only by declared edges / parent, so the caller warns."""
    return [i.id for i in issues if not i.touches]


def prefix_touches_drift(issues: list[Issue]) -> list[tuple[str, str]]:
    """``(identifier, reason)`` pairs where the ``Claude:`` title prefix and the
    ``**Touches**:`` surface disagree — the consistency check that supersedes
    the old glob-only ``# Skills`` bucketing. Two mismatches are flagged:

    * a ``Claude:``-prefixed issue whose touches reach **outside** the meta
      surface (the prefix over-claims), and
    * a meta-only-touches issue with **no** ``Claude:`` prefix (it should have
      one so it batches under ``# Claude``).

    A prefixed issue with no ``**Touches**:`` at all is left alone — there's
    nothing to check it against."""
    out: list[tuple[str, str]] = []
    for i in issues:
        prefixed = i.has_claude_prefix()
        if prefixed and i.touches and not i.is_meta_only():
            out.append(
                (i.id, "Claude: prefix but touches reach outside the meta surface")
            )
        elif i.is_meta_only() and not prefixed:
            out.append((i.id, "meta-only touches but no Claude: prefix"))
    return out


def block_counts(
    issues: list[Issue],
    blockers: dict[str, set[str]],
    extra_nodes: set[str] | None = None,
) -> dict[str, int]:
    """How many *other* issues each issue blocks, counted **transitively**: if
    A blocks B and B blocks C, A blocks 2. The node set is the Backlog
    (``issues``) plus any ``extra_nodes`` — the live external blockers that gate
    Backlog work without a chip of their own — so they rank in the tally too.

    ``blockers[x]`` is the set of issues that block ``x``; we invert it to a
    forward map (who each issue blocks) and count the distinct reach of each
    node with a visited set, so a blocker **cycle** terminates instead of
    looping and no node is counted as blocking itself."""
    forward: dict[str, set[str]] = {}
    for blocked, bs in blockers.items():
        for b in bs:
            forward.setdefault(b, set()).add(blocked)

    nodes = {i.id for i in issues}
    if extra_nodes:
        nodes |= extra_nodes

    counts: dict[str, int] = {}
    for n in nodes:
        reached: set[str] = set()
        stack = list(forward.get(n, ()))
        while stack:
            cur = stack.pop()
            if cur in reached:
                continue
            reached.add(cur)
            stack.extend(forward.get(cur, ()))
        reached.discard(n)  # a cycle can reach back to the node; don't self-count
        counts[n] = len(reached)
    return counts


def render_tally(counts: dict[str, int], number_of: dict[str, int]) -> str | None:
    """The `# Most blocking` tally: every issue that blocks at least one other,
    ranked by how many it blocks (descending), ties broken by lowest number
    first — so the issue to start first sits at the top. ``None`` when nothing
    blocks anything."""
    ranked = [ident for ident, n in counts.items() if n > 0]
    if not ranked:
        return None
    ranked.sort(key=lambda ident: (-counts[ident], number_of.get(ident, 0)))
    out = ["# Most blocking\n\n"]
    for ident in ranked:
        n = counts[ident]
        noun = "issue" if n == 1 else "issues"
        out.append(f"- {ident} — blocks {n} {noun}\n")
    return "".join(out)


def render(
    issues: list[Issue],
    state_of: dict[str, str] | None = None,
    orphans: list[str] | None = None,
) -> str:
    """Render the full Task Staging document body for ``issues``.

    ``state_of`` maps every issue an edge reaches (Backlog and external) to its
    Linear state type, so a blocker that has resolved (``completed`` /
    ``canceled``) is dropped while a live external blocker is kept; when omitted
    every issue is treated as live Backlog (the unit-test default).

    If ``orphans`` is given, the ids of any bucket members the root-walk could
    not reach (a blocker cycle) are appended to it — the caller turns them into
    a stderr warning. They are still rendered (swept in as additional roots),
    so no issue is ever dropped from the output.
    """
    if not issues:
        return ""

    if state_of is None:
        state_of = {i.id: "backlog" for i in issues}

    universe = {i.id for i in issues}

    blockers = compute_blockers(issues, state_of)
    buckets = compute_buckets(issues)

    # Live external blockers — referenced in a blocker set but not a Backlog
    # chip — rank in the tally and number their tie-breaks like any other node.
    external_nodes = {b for bs in blockers.values() for b in bs} - universe
    number_of = {i.id: i.number for i in issues}
    for e in external_nodes:
        number_of[e] = parse_number(e) or 0

    # In-bucket blockers drive nesting; cross-bucket ones become notes.
    def same_bucket(a: str, b: str) -> bool:
        return buckets.get(a) == buckets.get(b)

    # chain_len = longest in-bucket blocker chain below an issue; the primary
    # blocker (the one to nest under) is the in-bucket blocker that settles
    # last, i.e. the deepest chain, tie-broken by highest number.
    primary: dict[str, str | None] = {}
    for i in issues:
        candidates = [b for b in blockers[i.id] if same_bucket(i.id, b)]
        if candidates:
            primary[i.id] = max(
                candidates,
                key=lambda b: (
                    chain_len(b, blockers, same_bucket, set()),
                    number_of.get(b, 0),
                ),
            )
        else:
            primary[i.id] = None

    # children[parent] = issues whose primary is ``parent``.
    children: dict[str, list[str]] = {}
    for i in issues:
        p = primary.get(i.id)
        if p is not None:
            children.setdefault(p, []).append(i.id)

    sections: list[str] = []

    # # Most blocking tally first — ranks the issues to start on first.
    tally = render_tally(block_counts(issues, blockers, external_nodes), number_of)
    if tally is not None:
        sections.append(tally)

    # # Claude (meta-work) next.
    s = render_bucket(
        "# Claude",
        ("claude",),
        issues,
        buckets,
        primary,
        children,
        blockers,
        number_of,
        orphans,
    )
    if s is not None:
        sections.append(s)

    # # ENG-### parent headings, ordered by parent number.
    parent_ids = sorted(
        {b[1] for b in buckets.values() if b[0] == "parent"},
        key=lambda p: parse_number(p) or 0,
    )
    for p in parent_ids:
        s = render_bucket(
            f"# {p}",
            ("parent", p),
            issues,
            buckets,
            primary,
            children,
            blockers,
            number_of,
            orphans,
        )
        if s is not None:
            sections.append(s)

    # # Standalone last.
    s = render_bucket(
        "# Standalone",
        ("standalone",),
        issues,
        buckets,
        primary,
        children,
        blockers,
        number_of,
        orphans,
    )
    if s is not None:
        sections.append(s)

    return "\n".join(sections)


def compute_blockers(
    issues: list[Issue], state_of: dict[str, str]
) -> dict[str, set[str]]:
    """Build each Backlog issue's blocker set from declared edges alone
    (``blockedBy`` / ``blocks``, symmetric). File-overlap is **not** inferred
    here — it arrives upstream as a materialized relation (see
    :func:`materialize_overlap_edges`), so blocking is wholly edge-driven.

    An edge is kept while its blocker is **live**: a blocker that has reached a
    terminal state (``completed`` / ``canceled``) is dropped, so a resolved
    dependency stops gating downstream work. A blocker is kept whether or not it
    is itself a Backlog issue — a live In-Progress / In-Review blocker stays
    visible as a tag. A blocker with no known state is assumed live (never
    silently dropped)."""

    def live(ident: str) -> bool:
        return state_of.get(ident) not in ("completed", "canceled")

    universe = {i.id for i in issues}
    blockers: dict[str, set[str]] = {i.id: set() for i in issues}

    for i in issues:
        for b in i.blocked_by:
            # Drop a self-edge (a data error) that would otherwise pull the
            # issue out of the tree, and a blocker that has already resolved.
            if b != i.id and live(b):
                blockers[i.id].add(b)
        for b in i.blocks:
            # ``i blocks b`` is the same edge as ``b blockedBy i``; only the
            # blocked Backlog issue carries a blocker set, and ``i`` (a Backlog
            # issue) is live by construction.
            if b != i.id and b in universe:
                blockers[b].add(i.id)

    return blockers


def compute_buckets(issues: list[Issue]) -> dict[str, tuple]:
    """Assign each issue a bucket: a ``Claude:``-prefixed (meta-work) issue →
    ``# Claude``; otherwise grouped under its parent when that parent has 2+
    non-``Claude:`` Backlog subtasks, else ``# Standalone``. Bucketing keys on
    the **title prefix** (the deterministic batch signal), not on file globs —
    the glob check is now the :func:`prefix_touches_drift` consistency warning.
    """
    parent_count: dict[str, int] = {}
    for i in issues:
        if i.has_claude_prefix():
            continue
        if i.parent:
            parent_count[i.parent] = parent_count.get(i.parent, 0) + 1

    result: dict[str, tuple] = {}
    for i in issues:
        if i.has_claude_prefix():
            bucket: tuple = ("claude",)
        elif i.parent and parent_count.get(i.parent, 0) >= 2:
            bucket = ("parent", i.parent)
        else:
            bucket = ("standalone",)
        result[i.id] = bucket
    return result


def chain_len(ident, blockers, same_bucket, visiting) -> int:
    """Longest in-bucket blocker chain below ``ident`` (0 if no in-bucket
    blocker), with a ``visiting`` cycle guard so a mutual declared edge can't
    loop. Not memoized: the backlog is small, and a cross-call memo would cache
    cycle-truncated values that depend on the entry point."""
    if ident in visiting:
        return 0  # cycle — stop descending
    visiting.add(ident)
    best = 0
    for b in blockers.get(ident, ()):
        if same_bucket(ident, b):
            best = max(best, 1 + chain_len(b, blockers, same_bucket, visiting))
    visiting.discard(ident)
    return best


def render_bucket(
    heading, bucket, issues, buckets, primary, children, blockers, number_of, orphans
) -> str | None:
    """Render one heading and its tree, or ``None`` if the bucket has no
    members. Members with no in-bucket primary blocker are roots; after the
    sorted-roots walk, any member still unreached (a blocker cycle has no root)
    is swept in as an additional root so it is never silently dropped, and its
    id is recorded in ``orphans``."""
    member_ids = [i.id for i in issues if buckets.get(i.id) == bucket]
    if not member_ids:
        return None
    members_sorted = sorted(member_ids, key=lambda i: number_of.get(i, 0))
    roots = [i for i in members_sorted if primary.get(i) is None]

    out: list[str] = [f"{heading}\n\n"]
    seen: set[str] = set()
    # Proper ancestors of the node currently being rendered, threaded down the
    # descent so a blocker the nesting already expresses isn't repeated as a
    # note. Balanced (insert on descent, remove on backtrack), so it is empty
    # between roots.
    ancestors: set[str] = set()
    for root in roots:
        render_node(
            root, 0, primary, children, blockers, number_of, seen, ancestors, out
        )

    # Orphan sweep: any member the root-walk didn't reach — every member of a
    # blocker cycle has a non-None primary, so none is a root and the whole
    # cycle would otherwise vanish. Record every unreached id (the complete
    # "didn't reach" set, for the warning), then render each still-unseen one
    # as an additional root (lowest number first, breaking the cycle at its
    # lowest-numbered member) so nothing is ever silently dropped.
    if orphans is not None:
        orphans.extend(i for i in members_sorted if i not in seen)
    for ident in members_sorted:
        if ident in seen:
            continue
        render_node(
            ident, 0, primary, children, blockers, number_of, seen, ancestors, out
        )

    return "".join(out)


def render_node(
    ident, depth, primary, children, blockers, number_of, seen, ancestors, out
):
    """Render ``ident`` as a bullet and recurse into its children. ``seen``
    guards against re-rendering a node reached twice. ``ancestors`` holds the
    proper ancestors of ``ident`` on the current descent path; a blocker in
    that set is already shown by the indentation, so ``notes`` drops it."""
    if ident in seen:
        return
    seen.add(ident)
    indent = INDENT * depth
    out.append(
        f"{indent}- {ident}{notes(ident, primary, blockers, number_of, ancestors)}\n"
    )

    kids = children.get(ident)
    if kids:
        kids_sorted = sorted(kids, key=lambda k: number_of.get(k, 0))
        # ``ident`` is a proper ancestor of every node below it; insert before
        # recursing and remove on backtrack, mirroring the ``seen`` guard.
        ancestors.add(ident)
        for kid in kids_sorted:
            render_node(
                kid,
                depth + 1,
                primary,
                children,
                blockers,
                number_of,
                seen,
                ancestors,
                out,
            )
        ancestors.discard(ident)


def notes(ident, primary, blockers, number_of, ancestors) -> str:
    """The trailing bare-tag blocker note for a node: a plain parenthesized
    list ``(ENG-X, ENG-Y)`` of the node's direct live blockers that the nesting
    does **not** already express — i.e. every blocker except the primary (shown
    by what it nests under) and except any blocker already an ancestor on the
    descent path (shown by the indentation), sorted by number. Empty when the
    nesting already shows every blocker."""
    prim = primary.get(ident)
    extra = [b for b in blockers.get(ident, ()) if b != prim and b not in ancestors]
    if not extra:
        return ""
    extra.sort(key=lambda b: number_of.get(b, 0))
    return f" ({', '.join(extra)})"


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
      title
      description
      parent { identifier }
      relations { nodes { type relatedIssue { identifier state { type } } } }
      inverseRelations { nodes { type issue { identifier state { type } } } }
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

SAVE_DOC_MUTATION = """
mutation SaveDoc($id: String!, $content: String!) {
  documentUpdate(id: $id, input: { content: $content }) { success }
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
        raise StageBacklogError(f"Linear API returned HTTP {e.code}: {detail}") from e
    except urllib.error.URLError as e:
        raise StageBacklogError(f"Linear API request failed: {e.reason}") from e

    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError as e:
        raise StageBacklogError(f"decoding Linear GraphQL response: {e}") from e

    errors = parsed.get("errors")
    if errors:
        joined = "; ".join(e.get("message", "") for e in errors)
        raise StageBacklogError(f"Linear GraphQL error: {joined}")
    data = parsed.get("data")
    if data is None:
        raise StageBacklogError("Linear GraphQL response carried no data")
    return data


def _raw_to_issue(raw: dict) -> Issue:
    """Map a raw GraphQL issue into the planner's :class:`Issue`."""
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
    description = raw.get("description") or ""
    touches = parse_touches(description)
    parent = raw["parent"]["identifier"] if raw.get("parent") else None
    ident = raw["identifier"]
    return Issue(
        id=ident,
        number=parse_number(ident) or 0,
        uuid=raw.get("id") or "",
        title=raw.get("title") or "",
        parent=parent,
        touches=touches,
        blocked_by=blocked_by,
        blocks=blocks,
    )


def _collect_states(node: dict, state_of: dict[str, str]) -> None:
    """Record the state type of every issue ``node``'s relations reach, so a
    blocker outside the Backlog (an In-Progress / In-Review / Done issue) has a
    known state. ``setdefault`` keeps a Backlog issue's authoritative
    ``"backlog"`` (set from its own node) over a relation-derived copy."""
    for r in node["relations"]["nodes"]:
        ri = r.get("relatedIssue")
        if ri and ri.get("state"):
            state_of.setdefault(ri["identifier"], ri["state"]["type"])
    for r in node["inverseRelations"]["nodes"]:
        ii = r.get("issue")
        if ii and ii.get("state"):
            state_of.setdefault(ii["identifier"], ii["state"]["type"])


def fetch_backlog(api_key: str, project_id: str) -> tuple[list[Issue], dict[str, str]]:
    """All open Backlog issues for the project, distilled into :class:`Issue`s,
    paired with a ``state_of`` map (identifier → Linear state type) covering
    every Backlog issue and every issue its relations reach.

    Reads one page (``PAGE_SIZE``); rather than silently stage — and overwrite
    the document with — a truncated tree, it refuses if the project has more.
    """
    data = _post(api_key, BACKLOG_QUERY, {"projectId": project_id, "first": PAGE_SIZE})
    conn = data["issues"]
    if conn["pageInfo"]["hasNextPage"]:
        raise StageBacklogError(
            f"project has more than {PAGE_SIZE} open Backlog issues; pagination "
            "is not implemented, so refusing to stage a truncated tree"
        )
    issues = [_raw_to_issue(n) for n in conn["nodes"]]
    # The query filters to backlog state, so every top-level node is "backlog";
    # relations contribute the states of issues outside that set.
    state_of: dict[str, str] = {n["identifier"]: "backlog" for n in conn["nodes"]}
    for n in conn["nodes"]:
        _collect_states(n, state_of)
    return issues, state_of


def save_document(api_key: str, doc_id: str, content: str) -> None:
    """Rewrite the Task Staging document's body in full."""
    data = _post(api_key, SAVE_DOC_MUTATION, {"id": doc_id, "content": content})
    if not data["documentUpdate"]["success"]:
        raise StageBacklogError("Linear documentUpdate returned success=false")


def issue_relation_create(api_key: str, issue_uuid: str, related_uuid: str) -> None:
    """File a ``blocks`` relation: ``issue_uuid`` blocks ``related_uuid`` (both
    Linear internal UUIDs)."""
    data = _post(
        api_key,
        ISSUE_RELATION_CREATE_MUTATION,
        {"issueId": issue_uuid, "relatedIssueId": related_uuid},
    )
    if not data["issueRelationCreate"]["success"]:
        raise StageBacklogError("Linear issueRelationCreate returned success=false")


def materialize_overlap_edges(
    issues: list[Issue], api_key: str | None, dry_run: bool
) -> list[tuple[str, str]]:
    """Turn each undeclared file-overlap into a real Linear ``blocks`` relation.

    For every pair of Backlog issues whose ``**Touches**:`` globs collide and
    that have **no** declared edge in either direction, file the lower-numbered
    issue ``blocks`` the higher-numbered one — materializing the scheduling
    constraint as a tagged edge so the staged tree and Linear agree (this is the
    one place the tool writes a *relation*, a deliberate departure from the
    render-only rewrite). The edge is also added to the in-memory model so the
    same run renders it, in both real and ``--dry-run`` mode; only the Linear
    write is skipped under ``--dry-run``. Returns the ``(lower, higher)`` id
    pairs filed (or that would be filed), lowest-first, for the caller to
    report. A pair already linked (declared, or filed earlier in this pass) is
    skipped, so the relation is never duplicated."""
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
            if not touches_overlap(ia, ic):
                continue
            pair = frozenset((ia.id, ic.id))
            if pair in linked:
                continue
            lo, hi = (ia, ic) if ia.number <= ic.number else (ic, ia)
            filed.append((lo.id, hi.id))
            linked.add(pair)
            # Reflect the new edge in memory so this run renders it.
            lo.blocks.append(hi.id)
            if not dry_run:
                issue_relation_create(api_key, lo.uuid, hi.uuid)
    filed.sort(key=lambda p: (parse_number(p[0]) or 0, parse_number(p[1]) or 0))
    return filed


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------

HELP = """\
Usage:
  stage_backlog.py [--dry-run]
      Render the Dropset Linear Backlog as the Task Staging dependency tree.
      --dry-run  Print the tree to stdout; do not write the document."""


def env_var(name: str) -> str:
    """Read a required, non-empty environment variable."""
    value = os.environ.get(name)
    if value is None:
        raise StageBacklogError(f"{name} is not set")
    if not value.strip():
        raise StageBacklogError(f"{name} is empty")
    return value


def run(argv: list[str]) -> int:
    args = argv[1:]
    if any(a in ("-h", "--help") for a in args):
        print(HELP)
        return 0

    dry_run = False
    for arg in args:
        if arg == "--dry-run":
            dry_run = True
        else:
            raise StageBacklogError(f"unknown argument: {arg} (try --help)")

    api_key = env_var("LINEAR_API_KEY")
    project_id = env_var("LINEAR_PROJECT_ID")
    # Resolve the write target before any relation is filed, so a real run that
    # is missing the doc id fails fast instead of half-writing relations.
    doc_id = env_var("LINEAR_TASK_STAGING_DOC_ID") if not dry_run else None

    issues, state_of = fetch_backlog(api_key, project_id)

    for ident in missing_touches(issues):
        print(
            f"warning: {ident} has no **Touches**: field; placed by declared "
            "edges / parent only — backfill one if the placement looks wrong",
            file=sys.stderr,
        )

    for ident, reason in prefix_touches_drift(issues):
        print(
            f"warning: {ident} {reason} — reconcile the Claude: prefix with "
            "the **Touches**: surface",
            file=sys.stderr,
        )

    filed = materialize_overlap_edges(issues, api_key, dry_run)
    verb = "would file" if dry_run else "filed"
    for lo, hi in filed:
        print(f"{verb}: {lo} blocks {hi} (touch overlap)", file=sys.stderr)

    orphans: list[str] = []
    document = render(issues, state_of, orphans)
    if orphans:
        print(
            f"warning: blocker cycle — {', '.join(orphans)} unreachable from a "
            "bucket root; rendered at the lowest-numbered member (check for a "
            "backwards blockedBy/blocks edge)",
            file=sys.stderr,
        )

    if dry_run:
        sys.stdout.write(document)
        print(
            f"stage-backlog (dry-run) | {len(issues)} backlog issues | "
            f"{len(filed)} overlap edges would be filed",
            file=sys.stderr,
        )
        return 0

    save_document(api_key, doc_id, document)
    print(
        f"stage-backlog | {len(issues)} backlog issues | "
        f"{len(filed)} overlap edges filed | staging document updated"
    )
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except StageBacklogError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
