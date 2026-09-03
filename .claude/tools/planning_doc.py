#!/usr/bin/env python3
"""Scoped reader for the Planning Linear document.

**Why this exists.** The MCP `get_document` returns the *entire* content with
no slice accessor, and the Planning document only grows between close-out
rewrites — so the cost rises over time for every session that needs one fact
from it. Measured (session fa6fc519, the ENG-978 seam audit): a single
`get_document` was the **largest single main-loop result of the session at
≈7.9k**, more than double the next non-filing result. It was consumed for
four short passages — the audit-state row for the seam, a note raising the
issue to High, the ratified no-level-price-bound rule, and the
never-hardcode-slot-duration rule — out of a document also covering the board
schema, four live tracks, the current phase, feeds roster detail, migration
numbering, the calendar track, session-metrics pipeline state, standing
decisions and verification debt.

That read was *justified* — the operator had asked whether an audit's scope
was sufficient, which is a question only this document answers. It will recur
for every worktree session that checks itself against planning direction,
which is a habit worth **encouraging**. So the fix makes the check cheap
rather than rarer.

The document is fetched in this process and only the matching sections are
printed, the same read-it-outside-the-model design `session_metrics.py` and
`trim_levers.py` already use. It resolves ``LINEAR_PLANNING_DOC_ID`` itself,
so every invocation reduces to one allow-rule.

Usage::

    python3 .claude/tools/planning_doc.py --headings
    python3 .claude/tools/planning_doc.py --section 'Audit state'
    python3 .claude/tools/planning_doc.py --grep 'slot duration' --context 2
    python3 .claude/tools/planning_doc.py --out <scratchpad>/planning.md

``--out`` spills the whole content to a file and prints only a heading map, for
when several sections are wanted: slice that file with ``read_result.py``
afterwards rather than re-fetching.

**A live planning session is still the better source when there is one.** It
answers the question rather than the text — one session that read the document
then messaged its planning peer got the state of both blockers, the operator's
posture, a ruling on the actual question, and an unprompted correction to a
figure in the document that had gone stale. A stale line reads exactly like a
current one. Use `ListAgents` first; use this when none is live, or
unattended.

Standard library only. A Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling"). Tests live in ``tests/test_planning_doc.py``, run via
``make tools-tests``.
"""

from __future__ import annotations

import argparse
import os
import sys

import linear_api
import read_result

_DOC_QUERY = """
query Doc($id: String!) {
  document(id: $id) { title content }
}
"""


class PlanningDocError(Exception):
    """A failure worth one clean stderr line, never a traceback."""


def fetch(api_key: str, doc_id: str) -> tuple[str, str]:
    """``(title, content)`` for the planning document."""
    data = linear_api.post(api_key, _DOC_QUERY, {"id": doc_id}, error=PlanningDocError)
    doc = data.get("document") or {}
    content = doc.get("content")
    if not isinstance(content, str) or not content.strip():
        raise PlanningDocError(
            f"document {doc_id} returned no content — check "
            "LINEAR_PLANNING_DOC_ID names the Planning document"
        )
    return str(doc.get("title") or "(untitled)"), content


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="planning_doc.py",
        description="Read named sections of the Planning document, not all of it.",
    )
    parser.add_argument(
        "--id",
        default=None,
        help="document id (default: $LINEAR_PLANNING_DOC_ID)",
    )
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument(
        "--headings", action="store_true", help="the heading map — start here"
    )
    mode.add_argument("--section", default=None, metavar="RE", help="one section")
    mode.add_argument(
        "--sections",
        default=None,
        metavar="RE",
        help="every matching section, in document order",
    )
    mode.add_argument("--grep", default=None, metavar="RE", help="matching lines")
    mode.add_argument(
        "--out",
        default=None,
        metavar="FILE",
        help="spill the whole content to FILE and print only a heading map",
    )
    parser.add_argument(
        "--context", type=int, default=0, help="context lines for --grep"
    )
    args = parser.parse_args(argv[1:])

    doc_id = args.id or linear_api.env_var(
        "LINEAR_PLANNING_DOC_ID", error=PlanningDocError
    )
    api_key = linear_api.env_var("LINEAR_API_KEY", error=PlanningDocError)
    title, content = fetch(api_key, doc_id)
    lines = content.splitlines()

    # `is not None`, matching its siblings below. Dispatching on truthiness sent
    # `--out ''` down the chain to the terminal `grep` branch with
    # `args.grep is None`, producing a traceback instead of the one clean stderr
    # line this tool's exception class promises.
    if args.out is not None:
        try:
            handle = os.open(args.out, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(handle, "w", encoding="utf-8") as fh:
                # `O_CREAT`'s mode applies only when the file is CREATED, so a
                # re-spill over an existing world-readable file left its mode
                # untouched — and `--out <scratchpad>/planning.md` is exactly
                # the re-run-to-the-same-path shape this tool recommends, so the
                # owner-only guarantee held precisely in the case that does not
                # recur. `fchmod` makes it unconditional.
                os.fchmod(handle, 0o600)
                fh.write(content)
        except OSError as exc:
            raise PlanningDocError(f"cannot write {args.out}: {exc}") from exc
        out = read_result.headings(lines)
        summary = (
            f"{title} | spilled {len(content)} char(s) to {args.out}; "
            f"{len(out)} heading(s) — slice it with read_result.py"
        )
    elif args.headings:
        out = read_result.headings(lines)
        summary = f"{title} | {len(out)} heading(s) of {len(lines)} line(s)"
    elif args.section is not None:
        block, start = read_result.section(lines, args.section)
        out = block
        summary = f"{title} | section at line {start}, {len(block)} line(s)"
    elif args.sections is not None:
        block, count = read_result.sections(lines, args.sections)
        out = block
        summary = f"{title} | {count} section(s), {len(block)} line(s)"
    else:
        out = read_result.grep(lines, args.grep, args.context)
        summary = f"{title} | {len(out)} matching line(s) of {len(lines)}"

    print(f"planning-doc | {summary}", file=sys.stderr)
    for line in out:
        print(line)
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except (PlanningDocError, read_result.ReadResultError) as exc:
        print(f"planning-doc: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
