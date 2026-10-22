#!/usr/bin/env python3
"""The dispatch gate's two halves, which the Linear MCP does not expose in one read.

Backlog is the operator's review inbox and Todo the approved queue: the
operator's own move from one to the other *is* the approval, since nobody else
may make it. A planning session dispatches a task only once it sits in Todo,
and closes a folded comment thread by resolving it — which the MCP cannot do.

Subcommands
-----------

``approvals`` read each issue's state and comment threads and print one line
              per issue: its state and how many top-level threads are still
              unresolved. Exits **1** if any issue is not in Todo, so a
              dispatch wrapper can refuse mechanically. The open-thread count
              is information for the fold, never a refusal: a thread the
              operator forgot to resolve does not block a launch.
``resolve``   resolve a comment thread via ``commentResolve``, optionally
              naming the comment that resolved it.

Any failure inside a run (transport, GraphQL, a missing variable) exits **2**,
so it can never read as "unapproved".

Stdlib only, and deliberately **not** a Cargo workspace member — see
``docs/conventions/skill-tooling.md``. Tests live in
``tests/test_linear_gate.py``, run via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import sys

import linear_api

# The approved queue. Compared by name, case-insensitively: Todo and Backlog
# share the "unstarted" / "backlog" types with no other distinguishing field.
APPROVED_STATE = "Todo"

EXIT_UNAPPROVED = 1
EXIT_ERROR = 2

# Linear's page ceiling. A spec with more comments than this has bigger
# problems; the count is then printed as a lower bound rather than refused.
_COMMENT_PAGE = 250

_GATE_QUERY = """
query IssueGate($id: String!, $first: Int!) {
  issue(id: $id) {
    identifier
    state { name }
    comments(first: $first) {
      nodes { resolvedAt parent { id } }
      pageInfo { hasNextPage }
    }
  }
}
"""

_RESOLVE_MUTATION = """
mutation ResolveComment($id: String!, $resolvingCommentId: String) {
  commentResolve(id: $id, resolvingCommentId: $resolvingCommentId) {
    success
  }
}
"""


class LinearGateError(Exception):
    """A user-facing failure: surfaced to stderr, exits ``EXIT_ERROR``."""


def open_threads(comments: object) -> str:
    """How many top-level threads are unresolved, as printed.

    A thread is a comment with no parent; a reply's own ``resolvedAt`` means
    nothing. A truncated page prints as a lower bound (``N+``).
    """
    if not isinstance(comments, dict):
        raise LinearGateError("issue comments came back malformed")
    nodes = comments.get("nodes")
    if not isinstance(nodes, list):
        raise LinearGateError("issue comments came back malformed")
    count = sum(
        1
        for node in nodes
        if isinstance(node, dict)
        and node.get("parent") is None
        and node.get("resolvedAt") is None
    )
    truncated = bool((comments.get("pageInfo") or {}).get("hasNextPage"))
    return f"{count}+" if truncated else str(count)


def _post(api_key: str, query: str, variables: dict) -> dict:
    return linear_api.post(api_key, query, variables, error=LinearGateError)


def cmd_approvals(api_key: str, identifiers: list[str]) -> tuple[list[str], bool]:
    """One line per issue, and whether every issue sits in Todo."""
    lines = []
    all_approved = True
    for identifier in identifiers:
        issue = _post(
            api_key, _GATE_QUERY, {"id": identifier, "first": _COMMENT_PAGE}
        ).get("issue")
        if not isinstance(issue, dict):
            raise LinearGateError(
                f"{identifier} did not resolve — it may be archived, deleted, or misspelled"
            )
        state = (issue.get("state") or {}).get("name")
        if not isinstance(state, str) or not state:
            raise LinearGateError(f"{identifier} came back with no state")
        threads = open_threads(issue.get("comments"))
        approved = state.casefold() == APPROVED_STATE.casefold()
        all_approved = all_approved and approved
        # The requested identifier, not the server's: a caller matching lines
        # to the issues it asked about must never see a different name. The
        # state goes last because a state name may contain a space.
        verdict = "APPROVED" if approved else "UNAPPROVED"
        lines.append(f"{verdict} {identifier} open-threads={threads} state={state}")
    return lines, all_approved


def cmd_resolve(
    api_key: str, comment_id: str, resolving_id: str | None, dry_run: bool
) -> str:
    suffix = f" by {resolving_id}" if resolving_id else ""
    if dry_run:
        return f"WOULD RESOLVE {comment_id}{suffix}"
    data = _post(
        api_key,
        _RESOLVE_MUTATION,
        {"id": comment_id, "resolvingCommentId": resolving_id},
    )
    result = data.get("commentResolve")
    if not (isinstance(result, dict) and result.get("success")):
        raise LinearGateError(f"commentResolve failed for {comment_id}")
    return f"RESOLVED {comment_id}{suffix}"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="The dispatch gate: approval probe and thread resolve.",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    approvals = sub.add_parser(
        "approvals",
        help="report each issue's state and open threads; exit 1 if any is not in Todo",
    )
    approvals.add_argument("identifiers", nargs="+", metavar="ENG-###")

    resolve = sub.add_parser("resolve", help="resolve a comment thread")
    resolve.add_argument("comment_id")
    resolve.add_argument(
        "resolving_comment_id",
        nargs="?",
        default=None,
        help="the comment that resolved the thread (optional)",
    )
    resolve.add_argument("--dry-run", action="store_true")

    return parser


def run(argv: list[str]) -> int:
    args = build_parser().parse_args(argv)
    try:
        api_key = linear_api.env_var("LINEAR_API_KEY", error=LinearGateError)
        if args.command == "approvals":
            lines, all_approved = cmd_approvals(api_key, args.identifiers)
        else:
            lines = [
                cmd_resolve(
                    api_key, args.comment_id, args.resolving_comment_id, args.dry_run
                )
            ]
            all_approved = True
    except LinearGateError as exc:
        print(f"linear-gate: {exc}", file=sys.stderr)
        return EXIT_ERROR
    except Exception as exc:  # noqa: BLE001
        # An uncaught exception exits 1, which a dispatch wrapper reads as
        # "unapproved". Anything unforeseen is an error, exit 2. The type alone
        # is printed: an unchecked message could quote a header value.
        print(f"linear-gate: unexpected {type(exc).__name__}", file=sys.stderr)
        return EXIT_ERROR
    for line in lines:
        print(line)
    return 0 if all_approved else EXIT_UNAPPROVED


if __name__ == "__main__":
    sys.exit(run(sys.argv[1:]))
