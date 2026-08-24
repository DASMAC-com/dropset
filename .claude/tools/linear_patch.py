#!/usr/bin/env python3
"""Zero-echo writes against a Linear issue: patch ops, comments, state.

The MCP write path echoes the **whole stored description** on every call. That
is a fixed per-call cost which ``patch`` does not shrink — patching reduces what
you *send*, never what you *receive* — and on an accumulator issue it compounds
until the echoes dominate a session. Measured, repeatedly:

* a planning session paid **44 saves for ~153.2k**, its eight largest results
  all echoes of one growing meta batch;
* ten saves against a single large-bodied issue cost **~41.1k** to add ~2k of
  new content, *including a bare state transition* that paid the full echo for
  a one-field write;
* folding the batch this tool was written for required one full-body echo of
  the very issue being folded.

``trim_levers.py`` already carved out a zero-echo path for one narrow case. This
is that carve-out generalized: any issue, any patch op, one line of output.

**The pre-read stays.** Anchors must match the stored text exactly, so ``patch``
still fetches the body — but it fetches it *into this process*, applies the ops
here, and prints a size delta. The body never enters a transcript.

**Prefer ``comment`` for narrative.** New prose on a large issue belongs in a
comment: additive, chronological, and with no body round-trip at all. Reserve
``patch`` for what genuinely must live in the body — checklists, structured
fields, supersessions.

Subcommands
-----------

``read``    fetch the body to a file; print its size only.
``patch``   apply an ops file to the description, atomically.
``comment`` add a comment from a file.
``state``   move the issue to a named state, with no body echo.

Ops file — a JSON array, applied in order, capped at 50::

    [{"op": "append",        "text": "..."},
     {"op": "prepend",       "text": "..."},
     {"op": "insert_before", "anchor": "...", "text": "..."},
     {"op": "insert_after",  "anchor": "...", "text": "..."},
     {"op": "replace",       "anchor": "...", "text": "..."},
     {"op": "replace_range", "start": "...", "end": "...", "text": "..."}]

Stdlib only, and deliberately **not** a Cargo workspace member — see
``docs/conventions/skill-tooling.md``. Tests live in
``tests/test_linear_patch.py``, run via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import json
import re
import sys

import linear_api

ENDPOINT = linear_api.ENDPOINT

# Matches the MCP's own ceiling, so a caller that outgrows one has outgrown
# both and should be folding rather than patching harder.
MAX_OPS = 50

# Linear rewrites a bare `ENG-###` into a mention node, so the stored text never
# matches an anchor containing one. Refused up front: the failure is otherwise a
# confusing "anchor not found" against text that is plainly on screen.
ISSUE_MENTION_RE = re.compile(r"\bENG-\d+\b")

_BODY_QUERY = """
query IssueBody($id: String!) {
  issue(id: $id) { id identifier url description team { id } state { name } }
}
"""

_UPDATE_MUTATION = """
mutation PatchIssue($id: String!, $description: String!) {
  issueUpdate(id: $id, input: { description: $description }) {
    success
    issue { identifier url }
  }
}
"""

_STATE_MUTATION = """
mutation SetState($id: String!, $stateId: String!) {
  issueUpdate(id: $id, input: { stateId: $stateId }) {
    success
    issue { identifier url state { name } }
  }
}
"""

_COMMENT_MUTATION = """
mutation AddComment($issueId: String!, $body: String!) {
  commentCreate(input: { issueId: $issueId, body: $body }) {
    success
    comment { id url }
  }
}
"""

_STATES_QUERY = """
query TeamStates($teamId: String!) {
  team(id: $teamId) { states(first: 100) { nodes { id name } } }
}
"""


class LinearPatchError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


# --------------------------------------------------------------------------
# Pure helpers — every one of these is testable without a network call.
# --------------------------------------------------------------------------


def _require_anchor(op_index: int, op: dict, key: str) -> str:
    value = op.get(key)
    if not isinstance(value, str) or not value:
        raise LinearPatchError(
            f"op {op_index}: {op.get('op')!r} needs a non-empty {key!r}"
        )
    if ISSUE_MENTION_RE.search(value):
        raise LinearPatchError(
            f"op {op_index}: {key!r} contains an ENG-### issue reference. Linear stores "
            "those as mention nodes, so the anchor can never match the stored text — "
            "anchor on neighboring prose instead."
        )
    return value


def _locate(body: str, anchor: str, op_index: int, key: str = "anchor") -> int:
    """The unique offset of ``anchor`` in ``body``, or a hard error.

    Ambiguity is refused rather than resolved: picking the first of several
    matches would silently edit a different part of the issue than the caller
    meant, and the caller cannot see the body from here.
    """
    count = body.count(anchor)
    if count == 0:
        raise LinearPatchError(
            f"op {op_index}: {key} not found in the stored body. Anchors must match the "
            "STORED text exactly — check for a span marker crossing an inline-code "
            "boundary, or an ENG-### rewritten as a mention."
        )
    if count > 1:
        raise LinearPatchError(
            f"op {op_index}: {key} matches {count} times and must match exactly once — "
            "extend it until it is unique."
        )
    return body.index(anchor)


def apply_ops(body: str, ops: list) -> str:
    """Apply ``ops`` in order and return the new body.

    Raises before any network write if a single op cannot be applied, which is
    what makes the whole sequence atomic: a half-applied patch on an issue body
    is far worse than a refused one.
    """
    if not isinstance(ops, list):
        raise LinearPatchError("the ops file must contain a JSON array")
    if not ops:
        raise LinearPatchError("the ops file is empty — nothing to apply")
    if len(ops) > MAX_OPS:
        raise LinearPatchError(
            f"{len(ops)} ops exceeds the cap of {MAX_OPS}. An issue needing more than "
            "that wants folding, not a larger patch."
        )

    result = body
    for index, op in enumerate(ops):
        if not isinstance(op, dict):
            raise LinearPatchError(f"op {index}: each op must be a JSON object")
        kind = op.get("op")
        text = op.get("text")
        if not isinstance(text, str):
            raise LinearPatchError(f"op {index}: {kind!r} needs a string 'text'")

        if kind == "append":
            result = result + text
        elif kind == "prepend":
            result = text + result
        elif kind == "insert_before":
            anchor = _require_anchor(index, op, "anchor")
            at = _locate(result, anchor, index)
            result = result[:at] + text + result[at:]
        elif kind == "insert_after":
            anchor = _require_anchor(index, op, "anchor")
            at = _locate(result, anchor, index) + len(anchor)
            result = result[:at] + text + result[at:]
        elif kind == "replace":
            anchor = _require_anchor(index, op, "anchor")
            at = _locate(result, anchor, index)
            result = result[:at] + text + result[at + len(anchor) :]
        elif kind == "replace_range":
            start = _require_anchor(index, op, "start")
            end = _require_anchor(index, op, "end")
            begin = _locate(result, start, index, key="start")
            tail = _locate(result, end, index, key="end")
            if tail < begin:
                raise LinearPatchError(
                    f"op {index}: 'end' occurs before 'start' in the stored body"
                )
            result = result[:begin] + text + result[tail + len(end) :]
        else:
            raise LinearPatchError(f"op {index}: unknown op {kind!r}")

    return result


def resolve_state(nodes: list, wanted: str) -> str:
    """A state id for ``wanted``, matched case-insensitively by name.

    Named states are resolved to UUIDs **here** rather than passed through: a
    state name handed to Linear as an id dies as an unnamed
    ``Argument Validation Error``, which is expensive to diagnose from the
    caller's side.
    """
    lowered = wanted.strip().lower()
    for node in nodes:
        if str(node.get("name", "")).strip().lower() == lowered:
            return str(node["id"])
    names = ", ".join(sorted(str(n.get("name")) for n in nodes))
    raise LinearPatchError(f"no state named {wanted!r} on this team — have: {names}")


# --------------------------------------------------------------------------
# Network paths
# --------------------------------------------------------------------------


def _post(api_key: str, query: str, variables: dict) -> dict:
    return linear_api.post(api_key, query, variables, error=LinearPatchError)


def _fetch(api_key: str, identifier: str) -> dict:
    data = _post(api_key, _BODY_QUERY, {"id": identifier})
    issue = data.get("issue") or {}
    if not issue.get("id"):
        raise LinearPatchError(
            f"{identifier} did not resolve — it may be archived, deleted, or misspelled"
        )
    return issue


def _read_text(path: str) -> str:
    try:
        with open(path, encoding="utf-8") as handle:
            return handle.read()
    except OSError as exc:
        raise LinearPatchError(f"cannot read {path}: {exc}") from exc


def cmd_read(api_key: str, identifier: str, out: str) -> str:
    issue = _fetch(api_key, identifier)
    body = issue.get("description") or ""
    try:
        with open(out, "w", encoding="utf-8") as handle:
            handle.write(body)
    except OSError as exc:
        raise LinearPatchError(f"cannot write {out}: {exc}") from exc
    # Size only. Printing the body here would reintroduce exactly the echo this
    # tool exists to avoid — slice the file with read_result.py instead.
    return f"READ {issue['identifier']} -> {out} ({len(body)} chars)"


def cmd_patch(api_key: str, identifier: str, ops_path: str, dry_run: bool) -> str:
    try:
        ops = json.loads(_read_text(ops_path))
    except json.JSONDecodeError as exc:
        raise LinearPatchError(f"decoding {ops_path}: {exc}") from exc

    issue = _fetch(api_key, identifier)
    stored = issue.get("description") or ""
    grown = apply_ops(stored, ops)

    if grown == stored:
        raise LinearPatchError(
            "the ops produced an identical body — refusing a write that changes nothing"
        )
    if dry_run:
        return (
            f"WOULD PATCH {issue['identifier']} | {len(ops)} op(s) | "
            f"{len(stored)} -> {len(grown)} chars"
        )

    data = _post(api_key, _UPDATE_MUTATION, {"id": issue["id"], "description": grown})
    result = data.get("issueUpdate") or {}
    if not result.get("success"):
        raise LinearPatchError(f"issueUpdate failed for {issue['identifier']}")
    updated = result.get("issue") or {}
    return (
        f"PATCHED {updated.get('identifier')} {updated.get('url')} | "
        f"{len(ops)} op(s) | {len(stored)} -> {len(grown)} chars"
    )


def cmd_comment(api_key: str, identifier: str, body_path: str, dry_run: bool) -> str:
    body = _read_text(body_path)
    if not body.strip():
        raise LinearPatchError(f"{body_path} is empty — nothing to comment")
    issue = _fetch(api_key, identifier)
    if dry_run:
        return f"WOULD COMMENT on {issue['identifier']} | {len(body)} chars"
    data = _post(api_key, _COMMENT_MUTATION, {"issueId": issue["id"], "body": body})
    result = data.get("commentCreate") or {}
    if not result.get("success"):
        raise LinearPatchError(f"commentCreate failed for {issue['identifier']}")
    comment = result.get("comment") or {}
    return f"COMMENTED {issue['identifier']} {comment.get('url')} | {len(body)} chars"


def cmd_state(api_key: str, identifier: str, wanted: str, dry_run: bool) -> str:
    issue = _fetch(api_key, identifier)
    current = (issue.get("state") or {}).get("name")
    if str(current).strip().lower() == wanted.strip().lower():
        return f"ALREADY {issue['identifier']} [{current}] — no write"
    team_id = (issue.get("team") or {}).get("id")
    if not team_id:
        raise LinearPatchError(f"{identifier} carried no team, so no state can resolve")
    data = _post(api_key, _STATES_QUERY, {"teamId": team_id})
    nodes = (((data.get("team") or {}).get("states")) or {}).get("nodes") or []
    state_id = resolve_state(nodes, wanted)
    if dry_run:
        return f"WOULD SET {issue['identifier']} [{current}] -> [{wanted}]"
    data = _post(api_key, _STATE_MUTATION, {"id": issue["id"], "stateId": state_id})
    result = data.get("issueUpdate") or {}
    if not result.get("success"):
        raise LinearPatchError(f"issueUpdate failed for {issue['identifier']}")
    updated = result.get("issue") or {}
    name = (updated.get("state") or {}).get("name")
    return (
        f"SET {updated.get('identifier')} {updated.get('url')} [{current}] -> [{name}]"
    )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Zero-echo writes against a Linear issue.",
        epilog=(
            "Prefer `comment` for narrative on a large issue: patching reduces what "
            "you send, never what you receive."
        ),
    )
    sub = parser.add_subparsers(dest="command", required=True)

    read = sub.add_parser("read", help="fetch the body to a file; print its size only")
    read.add_argument("identifier")
    read.add_argument("--out", required=True, help="file to write the body into")

    patch = sub.add_parser("patch", help="apply an ops file to the description")
    patch.add_argument("identifier")
    patch.add_argument("--ops", required=True, help="JSON array of patch ops")
    patch.add_argument("--dry-run", action="store_true")

    comment = sub.add_parser("comment", help="add a comment from a file")
    comment.add_argument("identifier")
    comment.add_argument("--body", required=True, help="file holding the comment text")
    comment.add_argument("--dry-run", action="store_true")

    state = sub.add_parser("state", help="move the issue to a named state")
    state.add_argument("identifier")
    state.add_argument("--state", required=True, dest="wanted")
    state.add_argument("--dry-run", action="store_true")

    return parser


def run(argv: list[str]) -> int:
    args = build_parser().parse_args(argv)
    try:
        api_key = linear_api.env_var("LINEAR_API_KEY", error=LinearPatchError)
        if args.command == "read":
            line = cmd_read(api_key, args.identifier, args.out)
        elif args.command == "patch":
            line = cmd_patch(api_key, args.identifier, args.ops, args.dry_run)
        elif args.command == "comment":
            line = cmd_comment(api_key, args.identifier, args.body, args.dry_run)
        else:
            line = cmd_state(api_key, args.identifier, args.wanted, args.dry_run)
    except LinearPatchError as exc:
        print(f"linear-patch: {exc}", file=sys.stderr)
        return 1
    print(line)
    return 0


if __name__ == "__main__":
    sys.exit(run(sys.argv[1:]))
