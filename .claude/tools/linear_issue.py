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
resolution would be a place for the two to disagree.

Subcommands::

    linear_issue.py append --id ENG-123 --file notes.md
    linear_issue.py find   --query "reference price" --limit 5

Each prints ONE line of accounting on success; ``find`` prints its rows.
``--dry-run`` reports what would happen and writes nothing.
"""

from __future__ import annotations

import argparse
import sys

import linear_api

#: Cap on a ``find`` page. A title-only projection is cheap in a way the MCP's
#: full issue objects never are, but a cap still beats an unbounded scan.
MAX_FIND = 100


class LinearIssueError(Exception):
    """A failure worth one clean stderr line, never a traceback."""


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

    data = _post(
        api_key,
        _FIND_QUERY,
        {"filter": issue_filter, "first": min(limit, MAX_FIND)},
    )
    nodes = ((data.get("issues") or {}).get("nodes")) or []
    return [f"{n.get('identifier')}  {n.get('title')}" for n in nodes]


def _read_file(path: str) -> str:
    try:
        with open(path, encoding="utf-8") as handle:
            return handle.read()
    except OSError as exc:
        raise LinearIssueError(f"could not read {path}: {exc}") from exc


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="linear_issue.py",
        description="Zero-echo Linear issue body append and title-only search.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="report what would happen and write nothing",
    )
    subs = parser.add_subparsers(dest="command", required=True)

    append = subs.add_parser("append", help="append to an issue body")
    append.add_argument("--id", required=True, help="ENG-### identifier")
    source = append.add_mutually_exclusive_group(required=True)
    source.add_argument("--file", help="read the addition from this file")
    source.add_argument("--text", help="the addition, inline")

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
