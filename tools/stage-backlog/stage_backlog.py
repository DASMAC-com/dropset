#!/usr/bin/env python3
"""Render the Dropset Linear Backlog as the chips-only Task Staging tree.

This is the deterministic core of the ``stage-backlog`` skill: read the
project's open Backlog (with parents and declared blocking relations), build
the dependency tree from those edges plus file-overlap, and write the rendered
tree to the Task Staging document. The whole path is mechanical — no model
judgment, no issue folding, one Linear read and (on a real run) one write.

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
    parent: str | None = None
    touches: list[str] = field(default_factory=list)
    blocked_by: list[str] = field(default_factory=list)
    blocks: list[str] = field(default_factory=list)

    def is_skill_only(self) -> bool:
        """True when the issue touches **only** the skill suite — files under
        ``.claude/skills/**`` or ``CLAUDE.md``, with no product code — so it
        folds into the consolidated ``# Skills`` PR. An issue with no
        ``touches`` is never skill-only (we can't prove it)."""
        return bool(self.touches) and all(is_skill_glob(g) for g in self.touches)


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


def is_skill_glob(glob: str) -> bool:
    """A glob counts as skill-suite when it names ``CLAUDE.md`` or sits under
    ``.claude/skills/``."""
    g = glob
    while g.startswith("./"):
        g = g[2:]
    return g == "CLAUDE.md" or g.startswith(".claude/skills")


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
# * Within a bucket, issues nest by blocker — declared (``blockedBy`` /
#   ``blocks``) or inferred from a file overlap (the higher-numbered issue
#   nests under the lower).
# * A blocker under a different heading renders as a trailing ``(after …)``
#   note; a second in-heading blocker the nesting can't show renders as
#   ``(also after …)``.
#
# The render is a pure function of its input and fully deterministic: all
# iteration that reaches the output is sorted by issue number.

# A bucket is a tuple: ("skills",), ("parent", "ENG-40"), or ("standalone",).


def missing_touches(issues: list[Issue]) -> list[str]:
    """Identifiers of issues that have no ``**Touches**:`` field — the planner
    can place them only by declared edges / parent, so the caller warns."""
    return [i.id for i in issues if not i.touches]


def block_counts(issues: list[Issue], blockers: dict[str, set[str]]) -> dict[str, int]:
    """How many *other* issues each issue blocks: the number of issues that
    list it in their blocker set. Direct (not transitive) — it matches the
    edges the tree renders, and stays meaningful inside a cycle (a transitive
    count would make every cycle member look equally blocking)."""
    counts = {i.id: 0 for i in issues}
    for i in issues:
        for b in blockers[i.id]:
            if b in counts:
                counts[b] += 1
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


def render(issues: list[Issue], orphans: list[str] | None = None) -> str:
    """Render the full Task Staging document body for ``issues``.

    If ``orphans`` is given, the ids of any bucket members the root-walk could
    not reach (a blocker cycle) are appended to it — the caller turns them into
    a stderr warning. They are still rendered (swept in as additional roots),
    so no issue is ever dropped from the output.
    """
    if not issues:
        return ""

    universe = {i.id for i in issues}
    number_of = {i.id: i.number for i in issues}

    blockers = compute_blockers(issues, universe)
    buckets = compute_buckets(issues)

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
    tally = render_tally(block_counts(issues, blockers), number_of)
    if tally is not None:
        sections.append(tally)

    # # Skills next.
    s = render_bucket(
        "# Skills",
        ("skills",),
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


def compute_blockers(issues: list[Issue], universe: set[str]) -> dict[str, set[str]]:
    """Build each issue's blocker set, restricted to the read universe:
    declared ``blockedBy`` / ``blocks`` (symmetric), then file-overlap edges
    (higher number under lower) for pairs with no declared edge."""
    blockers: dict[str, set[str]] = {i.id: set() for i in issues}

    for i in issues:
        for b in i.blocked_by:
            # Ignore a blocker outside the read set, and a self-edge (a data
            # error) that would otherwise drop the issue from the tree.
            if b != i.id and b in universe:
                blockers[i.id].add(b)
        for b in i.blocks:
            # ``i blocks b`` is the same edge as ``b blockedBy i``.
            if b != i.id and b in universe:
                blockers[b].add(i.id)

    n = len(issues)
    for a in range(n):
        for c in range(a + 1, n):
            ia, ic = issues[a], issues[c]
            if not touches_overlap(ia, ic):
                continue
            # A declared edge between the pair (either direction) wins over the
            # inferred "higher under lower" overlap edge — the human asserted
            # the order on purpose — so don't add a second edge.
            declared = ic.id in blockers[ia.id] or ia.id in blockers[ic.id]
            if declared:
                continue
            lo, hi = (ia, ic) if ia.number <= ic.number else (ic, ia)
            blockers[hi.id].add(lo.id)

    return blockers


def compute_buckets(issues: list[Issue]) -> dict[str, tuple]:
    """Assign each issue a bucket: skill-only → ``# Skills``; otherwise grouped
    under its parent when that parent has 2+ non-skill Backlog subtasks, else
    ``# Standalone``."""
    parent_count: dict[str, int] = {}
    for i in issues:
        if i.is_skill_only():
            continue
        if i.parent:
            parent_count[i.parent] = parent_count.get(i.parent, 0) + 1

    result: dict[str, tuple] = {}
    for i in issues:
        if i.is_skill_only():
            bucket: tuple = ("skills",)
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
    """The trailing ``(after …)`` / ``(also after …)`` note for a node: every
    blocker except the primary **and** except any blocker already an ancestor
    in the tree (the indentation shows those), sorted by number. A top-level
    node's first remaining blocker reads ``after``; everything else reads
    ``also after``."""
    prim = primary.get(ident)
    extra = [b for b in blockers.get(ident, ()) if b != prim and b not in ancestors]
    if not extra:
        return ""
    extra.sort(key=lambda b: number_of.get(b, 0))

    nested = prim is not None
    parts = []
    for i, b in enumerate(extra):
        word = "after" if (not nested and i == 0) else "also after"
        parts.append(f"{word} {b}")
    return f" ({', '.join(parts)})"


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
      identifier
      description
      parent { identifier }
      relations { nodes { type relatedIssue { identifier } } }
      inverseRelations { nodes { type issue { identifier } } }
    }
  }
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
        parent=parent,
        touches=touches,
        blocked_by=blocked_by,
        blocks=blocks,
    )


def fetch_backlog(api_key: str, project_id: str) -> list[Issue]:
    """All open Backlog issues for the project, distilled into :class:`Issue`s.

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
    return [_raw_to_issue(n) for n in conn["nodes"]]


def save_document(api_key: str, doc_id: str, content: str) -> None:
    """Rewrite the Task Staging document's body in full."""
    data = _post(api_key, SAVE_DOC_MUTATION, {"id": doc_id, "content": content})
    if not data["documentUpdate"]["success"]:
        raise StageBacklogError("Linear documentUpdate returned success=false")


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

    issues = fetch_backlog(api_key, project_id)

    for ident in missing_touches(issues):
        print(
            f"warning: {ident} has no **Touches**: field; placed by declared "
            "edges / parent only — backfill one if the placement looks wrong",
            file=sys.stderr,
        )

    orphans: list[str] = []
    document = render(issues, orphans)
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
            f"stage-backlog (dry-run) | {len(issues)} backlog issues", file=sys.stderr
        )
        return 0

    doc_id = env_var("LINEAR_TASK_STAGING_DOC_ID")
    save_document(api_key, doc_id, document)
    print(f"stage-backlog | {len(issues)} backlog issues | staging document updated")
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except StageBacklogError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
