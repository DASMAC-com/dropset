#!/usr/bin/env python3
"""The thumbs-up dispatch gate's two halves, which the Linear MCP does not expose.

A planning session dispatches a task only once the operator has ratified its
spec with a thumbs-up on the issue, and closes a folded comment thread by
resolving it. Neither is reachable through the MCP — ``get_issue`` and
``list_comments`` return no reactions, and nothing resolves a thread — so both
ran for two days as scratchpad scripts, a dozen times each, before the ruling
that a working shape is committed rather than re-typed. These are those scripts
on the shared transport, with one change: they now fail closed, refusing where
the scripts printed whatever came back.

Subcommands
-----------

``approvals`` read each issue's ``reactionData`` and print one line per issue:
              whether a ``+1`` by the configured assignee exists, and when.
              Exits **1** if any issue lacks one, so a dispatch wrapper can
              refuse mechanically.
``resolve``   resolve a comment thread via ``commentResolve``, optionally
              naming the comment that resolved it.

The assignee is ``LINEAR_ASSIGNEE_ID`` from the environment — never a
hard-coded UUID, per ``docs/conventions/linear-automation.md``. Any failure
inside a run (transport, GraphQL, a missing variable) exits **2**, so it can never read as
"unapproved".

Stdlib only, and deliberately **not** a Cargo workspace member — see
``docs/conventions/skill-tooling.md``. Tests live in
``tests/test_linear_gate.py``, run via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import sys

import linear_api

# Linear stores the thumbs-up as its shortcode. The bare glyph is accepted too,
# in case another client writes it; any other form reads as unapproved, which
# fails closed.
APPROVAL_EMOJI = frozenset({"+1", "\N{THUMBS UP SIGN}"})

EXIT_UNAPPROVED = 1
EXIT_ERROR = 2

_REACTIONS_QUERY = """
query IssueReactions($id: String!) {
  issue(id: $id) { identifier reactionData }
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


def approval_time(reaction_data: object, user_id: str) -> str | None:
    """The ``reactedAt`` of ``user_id``'s thumbs-up, or ``None`` if absent.

    ``reactionData`` is a list of ``{emoji, reactions: [{userId, reactedAt}]}``.
    Anything else — ``null`` on an issue nobody reacted to — is no approval.
    """
    if not isinstance(reaction_data, list):
        return None
    for entry in reaction_data:
        if not isinstance(entry, dict) or entry.get("emoji") not in APPROVAL_EMOJI:
            continue
        for reaction in entry.get("reactions") or []:
            if isinstance(reaction, dict) and reaction.get("userId") == user_id:
                return str(reaction.get("reactedAt") or "unknown time")
    return None


def _post(api_key: str, query: str, variables: dict) -> dict:
    return linear_api.post(api_key, query, variables, error=LinearGateError)


def cmd_approvals(
    api_key: str, user_id: str, identifiers: list[str]
) -> tuple[list[str], bool]:
    """One line per issue, and whether every issue carried the approval."""
    lines = []
    all_approved = True
    for identifier in identifiers:
        issue = _post(api_key, _REACTIONS_QUERY, {"id": identifier}).get("issue")
        if not isinstance(issue, dict):
            raise LinearGateError(
                f"{identifier} did not resolve — it may be archived, deleted, or misspelled"
            )
        when = approval_time(issue.get("reactionData"), user_id)
        # The requested identifier, not the server's: a caller matching lines
        # to the issues it asked about must never see a different name.
        if when is None:
            all_approved = False
            lines.append(f"UNAPPROVED {identifier}")
        else:
            lines.append(f"APPROVED {identifier} {when}")
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
        description="The thumbs-up dispatch gate: approval probe and thread resolve.",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    approvals = sub.add_parser(
        "approvals",
        help="report each issue's thumbs-up by $LINEAR_ASSIGNEE_ID; exit 1 if any lacks one",
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
            user_id = linear_api.env_var("LINEAR_ASSIGNEE_ID", error=LinearGateError)
            lines, all_approved = cmd_approvals(api_key, user_id, args.identifiers)
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
