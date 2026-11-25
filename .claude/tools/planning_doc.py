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

Three write modes. The scheduler's close-out::

    python3 .claude/tools/planning_doc.py replace-sections \
        --file <scratchpad>/changed.md [--drop '## Old addendum'] \
        [--keep '## Upkeep report …'] [--dry-run]

Each top-depth section in the file replaces the stored section with the same
heading text and depth (exactly one match, else nothing is written); every
other section is carried byte for byte, and the result is read back.

And an in-session anchored edit, in place of the MCP ``save_document``
``patch``::

    python3 .claude/tools/planning_doc.py patch --ops <scratchpad>/ops.json \
        [--dry-run]
    python3 .claude/tools/planning_doc.py patch --verify <hash>
    python3 .claude/tools/planning_doc.py patch --verify-file <written.md>

The ops are ``linear_patch.py``'s (same vocabulary, same atomic abort). The
write is read back, and only the body's hash and the changed headings are
printed; ``--verify`` later confirms the stored body still has that hash.
``--verify-file`` is the curator's read-back after its wholesale write: the
stored body is compared to the file it wrote, and only the first differing
line number is printed.

And another session's handoff note, under the one heading it may write::

    python3 .claude/tools/planning_doc.py append-under --file <note.md> \
        [--heading 'Notes for the next planning session'] [--dry-run]

The heading resolves by its text at any depth, the note lands after the
section's last line, and a note breaking the document's composition rules (an
inline code span, emphasis across a line break, a bare file name with its
extension) is refused before anything is written.

**A live scheduler is still the better source when there is one.** It
answers the question rather than the text — one session that read the document
then messaged its scheduler peer got the state of both blockers, the operator's
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
import hashlib
import json
import os
import re
import sys

import linear_api
import linear_patch
import read_result

#: Hex digits of the sha256 printed as a body's hash — enough to tell two
#: versions of one document apart, short enough to carry in a later command.
HASH_LEN = 12

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


_UPDATE_MUTATION = """
mutation SetDoc($id: String!, $content: String!) {
  documentUpdate(id: $id, input: { content: $content }) { success }
}
"""


def _file_blocks(text: str) -> list[tuple[int, str, str]]:
    """``(depth, heading, block)`` for each top-depth section of ``text``.

    The top depth is the first heading's; a deeper heading stays inside its
    block, a shallower one is refused (it would own sections the caller did
    not name). Text before the first heading is refused too: it belongs to
    no section, so where it should land is a guess.
    """
    plain = text.splitlines()
    kept = text.splitlines(keepends=True)
    heads = list(read_result.iter_headings(plain))
    if not heads:
        raise PlanningDocError("the sections file holds no heading")
    if any(line.strip() for line in plain[: heads[0][0]]):
        raise PlanningDocError(
            "the sections file has text before its first heading — every line "
            "must belong to a section"
        )
    top = heads[0][1]
    starts = []
    for index, depth, heading in heads:
        if depth < top:
            raise PlanningDocError(
                f"heading {heading!r} is shallower than the file's first "
                f"({'#' * top}) — one depth of section per file"
            )
        if depth == top:
            starts.append((index, heading))
    blocks = []
    for n, (index, heading) in enumerate(starts):
        end = starts[n + 1][0] if n + 1 < len(starts) else len(kept)
        blocks.append((top, heading, "".join(kept[index:end])))
    names = [b[1] for b in blocks]
    dupes = sorted({h for h in names if names.count(h) > 1})
    if dupes:
        raise PlanningDocError(f"the sections file repeats heading(s): {dupes}")
    return blocks


def _stored_range(plain: list[str], depth: int, heading: str) -> tuple[int, int]:
    """Line range ``[start, end)`` of the stored section titled exactly ``heading``.

    Exact text at the same depth, matched exactly once: a regex or a prefix
    match would let ``Notes`` replace ``Notes for the next planning session``.
    """
    heads = list(read_result.iter_headings(plain))
    found = [i for i, d, text in heads if d == depth and text == heading]
    if len(found) != 1:
        raise PlanningDocError(
            f"{'#' * depth} {heading!r} matches {len(found)} stored heading(s), "
            "not exactly one — run --headings and use its exact text"
        )
    start = found[0]
    end = next((i for i, d, _ in heads if i > start and d <= depth), len(plain))
    return start, end


def _heading_arg(flag: str, heading: str) -> tuple[int, str]:
    """``(depth, title)`` from a ``--drop`` / ``--keep`` full heading line."""
    hashes, _, title = heading.partition(" ")
    if not hashes or set(hashes) != {"#"} or not title.strip():
        raise PlanningDocError(
            f"{flag} {heading!r} must be a full heading line, e.g. '## Notes'"
        )
    return len(hashes), title.strip()


def replace_sections(
    content: str, blocks: list, drops: list[str], keeps: list[str] = ()
) -> tuple[str, int]:
    """``content`` with each named section replaced (or dropped); and the count.

    Every unnamed section is carried byte for byte, so the result equals the
    wholesale rewrite that changed only those sections. Every range is
    resolved against the stored text before any splice, so one bad heading
    refuses the whole set.

    A section owns its subsections, so a stored subsection the replacement
    does not re-supply would vanish — the machine-written ``## Upkeep report``
    under the notes heading is the live case. That is refused unless the
    subsection is named in ``drops`` (the loss made deliberate) or ``keeps``.
    A kept subsection is carried forward from ``content`` itself — the live
    fetch, not a copy the caller read earlier, which a later upkeep run may
    have superseded — and lands after the block's own text, in stored order.
    """
    plain = content.splitlines()
    kept = content.splitlines(keepends=True)
    heads = list(read_result.iter_headings(plain))
    dropped: list[tuple[int, int]] = [
        _stored_range(plain, *_heading_arg("--drop", h)) for h in drops
    ]
    carried = [_stored_range(plain, *_heading_arg("--keep", h)) for h in keeps]
    edits = []
    for depth, heading, text in blocks:
        start, end = _stored_range(plain, depth, heading)
        supplied = {(d, t) for _, d, t in read_result.iter_headings(text.splitlines())}
        mine = sorted(r for r in carried if start < r[0] < end)
        for i, d, t in heads:
            if not start < i < end or (d, t) in supplied:
                continue
            if any(ks <= i < ke for ks, ke in mine):
                continue  # carried forward whole, its own children included
            line = f"{'#' * d} {t}"
            named_drop = [x for x in dropped if x[0] == i]
            if not named_drop:
                raise PlanningDocError(
                    f"replacing {'#' * depth} {heading!r} would delete its "
                    f"stored subsection {line!r} — re-supply it in the file, "
                    f"--keep {line!r} to carry the live copy forward, or "
                    f"--drop {line!r} to delete it on purpose"
                )
            dropped.remove(named_drop[0])  # inside this splice; not its own edit
        if mine:
            text = text.rstrip("\n") + "\n\n"
            text += "".join("".join(kept[ks:ke]) for ks, ke in mine)
        for r in mine:
            carried.remove(r)
        edits.append((start, end, text))
    if carried:
        raise PlanningDocError(
            "--keep names a section that is not a subsection of a replaced one"
        )
    edits += [(start, end, "") for start, end in dropped]
    edits.sort()
    for (_, prev_end, _), (start, _, _) in zip(edits, edits[1:]):
        if start < prev_end:
            raise PlanningDocError("two named sections overlap — name each once")
    out, cursor = [], 0
    for start, end, text in edits:
        if text and end < len(kept):
            # Keep the stored spacing before the next heading: a range ending in
            # a blank separator gets one back, else the next heading would butt
            # against this block (or glue onto its last line, with no newline).
            if not kept[end - 1].strip() and not text.endswith("\n\n"):
                text = text.rstrip("\n") + "\n\n"
            elif not text.endswith("\n"):
                text += "\n"
        out.append("".join(kept[cursor:start]))
        out.append(text)
        cursor = end
    out.append("".join(kept[cursor:]))
    return "".join(out), len(edits)


def run_replace(argv: list[str]) -> int:
    """``replace-sections``: the close-out's section-scoped write.

    Measured on two scheduler close-outs: the wholesale rewrite re-sent 44k
    characters to carry eleven unchanged sections forward, and in another the
    spill was re-read whole twice (about 37k and 74k characters) because the
    rewrite needed the full body — while a save race with an architect's
    appends lost two patches. Here the stored body is fetched, spliced and
    written inside this process: the transcript carries only the changed
    sections, and the read-modify-write window shrinks from the minutes a
    model takes to compose a body to one round trip (a re-fetch just before
    the write re-splices over an edit that landed meanwhile).
    """
    parser = argparse.ArgumentParser(
        prog="planning_doc.py replace-sections",
        description="Replace named sections of the Planning document in one write.",
    )
    parser.add_argument("--file", required=True, help="the changed sections")
    parser.add_argument(
        "--drop",
        action="append",
        default=[],
        metavar="HEADING",
        help="a stored section to delete, as its heading line (repeatable)",
    )
    parser.add_argument(
        "--keep",
        action="append",
        default=[],
        metavar="HEADING",
        help="a stored subsection of a replaced section to carry forward from "
        "the live document, as its heading line (repeatable)",
    )
    parser.add_argument("--id", default=None, help="default $LINEAR_PLANNING_DOC_ID")
    parser.add_argument("--dry-run", action="store_true", help="write nothing")
    args = parser.parse_args(argv)

    try:
        with open(args.file, encoding="utf-8") as fh:
            blocks = _file_blocks(fh.read())
    except OSError as exc:
        raise PlanningDocError(f"cannot read {args.file}: {exc}") from exc
    doc_id = args.id or linear_api.env_var(
        "LINEAR_PLANNING_DOC_ID", error=PlanningDocError
    )
    api_key = linear_api.env_var("LINEAR_API_KEY", error=PlanningDocError)
    title, stored = fetch(api_key, doc_id)
    composed, count = replace_sections(stored, blocks, args.drop, args.keep)
    if composed == stored:
        raise PlanningDocError("the sections match what is stored — nothing to write")
    summary = f"{title} | {count} section(s), {len(stored)} -> {len(composed)} chars"
    if args.dry_run:
        print(f"planning-doc | WOULD REPLACE {summary}", file=sys.stderr)
        return 0
    # Linear's GraphQL `documentUpdate` takes whole content only — there is no
    # server-side anchored patch to call — so a write that lands between our
    # fetch and our update is overwritten. Re-fetching immediately before the
    # write and re-splicing on any change narrows that window to one round
    # trip; a second change in that window refuses rather than guesses.
    _, fresh = fetch(api_key, doc_id)
    if fresh != stored:
        composed, count = replace_sections(fresh, blocks, args.drop, args.keep)
        _, again = fetch(api_key, doc_id)
        if again != fresh:
            raise PlanningDocError(
                "the document is changing under this write (two concurrent edits "
                "in a row) — wait for the other writer and re-run"
            )
        stored = fresh
        summary = (
            f"{title} | {count} section(s), {len(stored)} -> {len(composed)} chars "
            "(re-spliced over a concurrent edit)"
        )
    return _write_and_read_back(api_key, doc_id, composed, "REPLACED", summary)


def _write_and_read_back(
    api_key: str, doc_id: str, composed: str, verb: str, summary: str
) -> int:
    """Write ``composed``, read it back, and print one line; 0 when identical."""
    data = linear_api.post(
        api_key,
        _UPDATE_MUTATION,
        {"id": doc_id, "content": composed},
        error=PlanningDocError,
    )
    if not (data.get("documentUpdate") or {}).get("success"):
        raise PlanningDocError(f"documentUpdate failed for {doc_id}")
    # The read-back the close-out requires, done here without an echo: a
    # writer landing after ours, or Linear's markdown re-parse, shows as a
    # mismatch. (One landing before ours is what the re-fetch above narrows.)
    _, after = fetch(api_key, doc_id)
    if after != composed:
        print(
            f"planning-doc | {verb} {summary} — but the read-back differs from "
            f"line {_first_difference(composed, after)} (stored hash "
            f"{content_hash(after)}): slice it "
            "(--grep / --section) before trusting it",
            file=sys.stderr,
        )
        return 1
    print(
        f"planning-doc | {verb} {summary}; read-back identical, "
        f"hash {content_hash(after)}",
        file=sys.stderr,
    )
    return 0


def _first_difference(expected: str, actual: str) -> int:
    """The 1-based line where ``actual`` first departs from ``expected``."""
    old, new = expected.splitlines(), actual.splitlines()
    return next(
        (n + 1 for n, (a, b) in enumerate(zip(old, new)) if a != b),
        min(len(old), len(new)) + 1,
    )


def content_hash(content: str) -> str:
    """A short, stable digest of the stored body — the ``--verify`` key."""
    return hashlib.sha256(content.encode("utf-8")).hexdigest()[:HASH_LEN]


def _own_texts(content: str) -> dict[tuple[str, int], str]:
    """Each heading's *own* text (up to the next heading of any depth).

    Keyed by the heading line and its occurrence count, so a repeated heading
    is still told apart; the preamble before the first heading keys as ``""``.
    """
    lines = content.splitlines()
    heads = list(read_result.iter_headings(lines))
    bounds = [(0, "")] + [(i, f"{'#' * d} {t}") for i, d, t in heads]
    seen: dict[str, int] = {}
    out = {}
    for n, (start, line) in enumerate(bounds):
        end = bounds[n + 1][0] if n + 1 < len(bounds) else len(lines)
        seen[line] = seen.get(line, 0) + 1
        out[(line, seen[line])] = "\n".join(lines[start:end]).strip()
    return out


def changed_headings(before: str, after: str) -> list[str]:
    """The heading lines whose own text a patch changed, added or removed.

    Order follows ``after``, then any heading the patch removed. Text before
    the first heading reports as ``(preamble)``.
    """
    old, new = _own_texts(before), _own_texts(after)
    hits = [k for k in new if old.get(k) != new[k]]
    hits += [k for k in old if k not in new]
    return [line or "(preamble)" for line, _ in hits]


def run_patch(argv: list[str]) -> int:
    """``patch``: anchored ops on the Planning document, zero echo.

    Measured (the 2026-11-22 scheduler session): the whole document was
    re-read five times only to confirm a patch had landed, because the MCP
    ``save_document`` response kept returning a stale timestamp. Here the ops
    (``linear_patch.py``'s vocabulary and atomic abort) are applied in this
    process, the write is read back, and only a hash plus the changed
    headings are printed; ``--verify <hash>`` later confirms the stored body
    is still that one, without reading it into the transcript.
    """
    parser = argparse.ArgumentParser(
        prog="planning_doc.py patch",
        description="Apply anchored ops to the Planning document, or verify one.",
    )
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--ops", metavar="FILE", help="a JSON array of patch ops")
    mode.add_argument(
        "--verify",
        metavar="HASH",
        help="confirm the stored body still has this hash; write nothing",
    )
    mode.add_argument(
        "--verify-file",
        metavar="FILE",
        help="confirm the stored body equals FILE (a wholesale write's source); "
        "print only the first differing line",
    )
    parser.add_argument("--id", default=None, help="default $LINEAR_PLANNING_DOC_ID")
    parser.add_argument("--dry-run", action="store_true", help="write nothing")
    args = parser.parse_args(argv)

    doc_id = args.id or linear_api.env_var(
        "LINEAR_PLANNING_DOC_ID", error=PlanningDocError
    )
    api_key = linear_api.env_var("LINEAR_API_KEY", error=PlanningDocError)
    if args.verify_file is not None:
        try:
            with open(args.verify_file, encoding="utf-8") as fh:
                wanted = fh.read()
        except OSError as exc:
            raise PlanningDocError(f"cannot read {args.verify_file}: {exc}") from exc
        title, stored = fetch(api_key, doc_id)
        if stored.rstrip("\n") == wanted.rstrip("\n"):
            print(
                f"planning-doc | VERIFIED {title} | equals {args.verify_file}, "
                f"hash {content_hash(stored)}",
                file=sys.stderr,
            )
            return 0
        print(
            f"planning-doc | MISMATCH {title} | differs from {args.verify_file} "
            f"at line {_first_difference(wanted, stored)}: slice it "
            "(--grep / --section) before trusting it",
            file=sys.stderr,
        )
        return 1
    if args.verify is not None:
        title, stored = fetch(api_key, doc_id)
        actual = content_hash(stored)
        if actual == args.verify.strip().lower():
            print(f"planning-doc | VERIFIED {title} | hash {actual}", file=sys.stderr)
            return 0
        print(
            f"planning-doc | MISMATCH {title} | stored hash {actual}, expected "
            f"{args.verify} — another write landed since; slice before trusting it",
            file=sys.stderr,
        )
        return 1

    try:
        with open(args.ops, encoding="utf-8") as fh:
            ops = json.load(fh)
    except OSError as exc:
        raise PlanningDocError(f"cannot read {args.ops}: {exc}") from exc
    except json.JSONDecodeError as exc:
        raise PlanningDocError(f"decoding {args.ops}: {exc}") from exc
    title, stored = fetch(api_key, doc_id)
    composed = linear_patch.apply_ops(stored, ops)
    if composed == stored:
        raise PlanningDocError("the ops produced an identical body — nothing to write")

    def summarize(before: str, after: str, note: str = "") -> str:
        heads = changed_headings(before, after)
        named = "; ".join(heads) or "(blank lines only)"
        return (
            f"{title} | {len(ops)} op(s), {len(before)} -> {len(after)} chars{note} "
            f"| changed: {named}"
        )

    if args.dry_run:
        print(
            f"planning-doc | WOULD PATCH {summarize(stored, composed)}", file=sys.stderr
        )
        return 0
    # The same one-round-trip race guard as `replace-sections`: ops re-applied
    # over an edit that landed since the first fetch, or a refusal on two.
    _, fresh = fetch(api_key, doc_id)
    note = ""
    if fresh != stored:
        try:
            composed = linear_patch.apply_ops(fresh, ops)
        except linear_patch.LinearPatchError as exc:
            raise PlanningDocError(
                f"a concurrent edit landed since the first read and the ops no "
                f"longer apply ({exc}) — re-read the section and re-anchor"
            ) from exc
        if composed == fresh:
            raise PlanningDocError(
                "a concurrent edit already made this change — nothing to write"
            )
        _, again = fetch(api_key, doc_id)
        if again != fresh:
            raise PlanningDocError(
                "the document is changing under this write (two concurrent edits "
                "in a row) — wait for the other writer and re-run"
            )
        stored, note = fresh, " (re-applied over a concurrent edit)"
    return _write_and_read_back(
        api_key, doc_id, composed, "PATCHED", summarize(stored, composed, note)
    )


#: File extensions whose ``name.ext`` is also a hostname Linear linkifies —
#: most are country-code TLDs (``.py``, ``.rs``, ``.md``, ``.sh``).
_LINKIFIED_EXTS = (
    "py|rs|md|ts|tsx|js|json|toml|yaml|yml|sh|zsh|sql|txt|lock|css|html|so"
)
_BARE_NAME_EXT = re.compile(
    rf"(?<![\w/.~-])[A-Za-z0-9_-]+\.(?:{_LINKIFIED_EXTS})\b(?![/\w])"
)
_LIST_ITEM = re.compile(r"^\s*(?:[-*+]|\d+[.)])\s")


def composition_problems(note: str) -> list[str]:
    """Where ``note`` breaks the Planning document's composition rules.

    The document re-parses on every write, so the shapes the convention's
    write-mangle rules name are refused before the write rather than repaired
    after it: an inline code span, an emphasis span left open across a line
    break, and a bare ``name.ext`` (linkified as a hostname). Measured: one
    handoff note took a third ``save_document`` only to strip code spans the
    document's header forbids, which the writer had no way to check.
    """
    problems = []
    for n, line in enumerate(note.splitlines(), 1):
        if "`" in line:
            problems.append(f"line {n}: an inline code span (write the name plainly)")
        body = _LIST_ITEM.sub("", line, count=1)
        if body.count("**") % 2:
            problems.append(f"line {n}: a ** emphasis span not closed on its line")
        elif body.replace("**", "").count("*") % 2:
            problems.append(f"line {n}: a * emphasis span not closed on its line")
        for match in _BARE_NAME_EXT.finditer(line):
            problems.append(
                f"line {n}: bare {match.group(0)!r} is linkified as a hostname "
                "(name it without the extension, or give its directory)"
            )
    return problems


def append_under(content: str, heading: str, note: str) -> str:
    """``content`` with ``note`` appended at the end of the section ``heading``.

    The heading is matched by its text at any depth, so the caller never
    guesses its markdown level (a guessed level was a measured aborted
    anchor); exactly one match, else nothing is composed. The section runs to
    the next heading at the same or a shallower depth, so a subsection stays
    inside it. The note follows the section's last non-blank line — directly
    when both are list items, so a bullet list stays one list, else after a
    blank line — and the blank lines before the next heading are kept.
    """
    text = note.strip("\n")
    if not text.strip():
        raise PlanningDocError("the note is empty — nothing to append")
    lines = content.splitlines(keepends=True)
    plain = [line.rstrip("\n") for line in lines]
    heads = list(read_result.iter_headings(plain))
    wanted = heading.lstrip("#").strip()
    found = [(i, d) for i, d, t in heads if t.strip() == wanted]
    if len(found) != 1:
        raise PlanningDocError(
            f"heading {wanted!r} matches {len(found)} stored heading(s), not "
            "exactly one — run --headings and use its exact text"
        )
    start, depth = found[0]
    end = next((i for i, d, _ in heads if i > start and d <= depth), len(lines))
    last = end
    while last > start + 1 and not plain[last - 1].strip():
        last -= 1
    tight = (
        last > start + 1
        and _LIST_ITEM.match(plain[last - 1])
        and _LIST_ITEM.match(text.splitlines()[0])
    )
    before = lines[:last]
    if before and not before[-1].endswith("\n"):
        before[-1] += "\n"
    inserted = ("" if tight else "\n") + text + "\n"
    after = lines[last:]
    if after and after[0].strip():
        # The section ran straight into the next heading: keep one blank line.
        inserted += "\n"
    return "".join(before) + inserted + "".join(after)


def run_append_under(argv: list[str]) -> int:
    """``append-under``: add a note under one Planning document heading.

    Measured (session c09bcba8, a worker's post-merge handoff note): three
    ``save_document`` calls for one line — an anchor aborted on a guessed
    heading level, the insert, then a rewrite to strip inline code spans. Here
    the heading resolves by text, the note is linted before any write, and the
    write is guarded and read back like ``patch``, printing only a hash.
    """
    parser = argparse.ArgumentParser(
        prog="planning_doc.py append-under",
        description="Append a note at the end of one Planning document section.",
    )
    parser.add_argument(
        "--heading",
        default="Notes for the next planning session",
        help="the section's heading text, any depth (default: %(default)s)",
    )
    parser.add_argument("--file", required=True, help="the note to append")
    parser.add_argument("--id", default=None, help="default $LINEAR_PLANNING_DOC_ID")
    parser.add_argument("--dry-run", action="store_true", help="write nothing")
    args = parser.parse_args(argv)

    try:
        with open(args.file, encoding="utf-8") as fh:
            note = fh.read()
    except OSError as exc:
        raise PlanningDocError(f"cannot read {args.file}: {exc}") from exc
    problems = composition_problems(note)
    if problems:
        raise PlanningDocError(
            "the note breaks the document's composition rules, so nothing was "
            "written:\n  " + "\n  ".join(problems)
        )
    doc_id = args.id or linear_api.env_var(
        "LINEAR_PLANNING_DOC_ID", error=PlanningDocError
    )
    api_key = linear_api.env_var("LINEAR_API_KEY", error=PlanningDocError)
    title, stored = fetch(api_key, doc_id)
    composed = append_under(stored, args.heading, note)
    summary = (
        f"{title} | under {args.heading!r}, {len(stored)} -> {len(composed)} chars"
    )
    if args.dry_run:
        print(f"planning-doc | WOULD APPEND {summary}", file=sys.stderr)
        return 0
    # The same one-round-trip race guard as `patch`.
    _, fresh = fetch(api_key, doc_id)
    if fresh != stored:
        composed = append_under(fresh, args.heading, note)
        _, again = fetch(api_key, doc_id)
        if again != fresh:
            raise PlanningDocError(
                "the document is changing under this write (two concurrent edits "
                "in a row) — wait for the other writer and re-run"
            )
        summary += " (re-applied over a concurrent edit)"
    return _write_and_read_back(api_key, doc_id, composed, "APPENDED", summary)


def run(argv: list[str]) -> int:
    if argv[1:2] == ["replace-sections"]:
        return run_replace(argv[2:])
    if argv[1:2] == ["patch"]:
        return run_patch(argv[2:])
    if argv[1:2] == ["append-under"]:
        return run_append_under(argv[2:])
    parser = argparse.ArgumentParser(
        prog="planning_doc.py",
        description="Read named sections of the Planning document, not all of it.",
        epilog="The write modes: planning_doc.py replace-sections --help, "
        "planning_doc.py patch --help, planning_doc.py append-under --help",
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
    except (
        PlanningDocError,
        read_result.ReadResultError,
        linear_patch.LinearPatchError,
    ) as exc:
        print(f"planning-doc: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
