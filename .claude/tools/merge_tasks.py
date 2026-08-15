#!/usr/bin/env python3
"""``merge-tasks`` consolidation helper — the deterministic parts of folding
several Linear issues into one: number parsing/dedup, survivor resolution, body
assembly, and the ``**Touches**:`` union. The skill drives the Linear MCP reads
and writes; this tool never touches the network.

Two subcommands, each reading stdin/argv and printing JSON to stdout:

* ``plan [--survivor N] TOKEN...`` — parse the issue numbers the user passed
  (bare ``615`` or ``ENG-615``, any case, any order), **dedup** them, and
  resolve the survivor (the lowest-numbered by default, or ``--survivor N``).
  Prints ``{"survivor": "ENG-###", "ids": [...]}`` (ids sorted by number) so the
  skill knows what to ``get_issue`` before assembling.
* ``assemble ISSUES_JSON [--out PATH] [--ops-out PATH]`` — given a JSON file
  ``{"survivor": "ENG-###", "issues": [{id, number, title, description}, ...]}``,
  build the merged issue: the survivor body followed by each non-survivor body
  as a labeled ``# Part N — <title>`` section (every ``**Fingerprint**:`` line
  preserved verbatim), one consolidated ``**Touches**:`` line holding the union
  of all the globs, the title (``Claude:`` prefix applied when every folded
  issue is meta-work), and a cross-area flag when the set mixes meta and product
  surfaces.

  It emits the fold **two ways**, and the skill prefers the first:

  1. ``patch_ops`` — the fold as Linear ``patch`` operations: one ``append``
     per ``# Part`` section plus one ``replace`` swapping the survivor's
     ``**Touches**:`` line for the union. These carry only the *folded* bodies,
     so the survivor's existing text — 28KB is unremarkable — never has to be
     re-sent to ``save_issue`` at all. ``None`` when no safe anchor exists, with
     ``patch_fallback_reason`` saying which rule it tripped.
  2. ``description`` — the whole merged body, the wholesale fallback for exactly
     that case.

  Both are large, so both have a file handoff that keeps them out of stdout (see
  ``CLAUDE.md`` → "Context economy"): ``--out PATH`` writes the description and
  reports ``description_path``; ``--ops-out PATH`` writes the ops and reports
  ``patch_ops_path`` + ``patch_ops_count``. With neither flag, both payloads
  print inline.

Stdlib only. This is a Python skill-tool under ``.claude/tools/`` — deliberately
**not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys

# Path bases (besides the file CLAUDE.md) that count as agent-infra "meta-work"
# — the surface the ``Claude:`` issue-title prefix batches. The canonical
# definition is ``docs/conventions/linear-automation.md`` → "The Claude:
# meta-work prefix"; keep this copy in sync with it.
META_BASES = (".claude", "docs/conventions")

CLAUDE_PREFIX = "Claude: "

_NUM_RE = re.compile(r"(\d+)")


class MergeTasksError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


# --------------------------------------------------------------------------
# Pure helpers
# --------------------------------------------------------------------------


def parse_token(token: str) -> int:
    """Parse a bare number (``615``) or an ``ENG-615`` / ``eng-615`` identifier
    into its integer. Raises on anything without a trailing number."""
    m = _NUM_RE.search(token.strip())
    if not m:
        raise MergeTasksError(f"not an issue number or ENG-### tag: {token!r}")
    return int(m.group(1))


def plan(tokens: list[str], survivor_override: int | None) -> dict:
    """Resolve the deduped set of issue numbers and the survivor."""
    numbers = []
    for t in tokens:
        n = parse_token(t)
        if n not in numbers:
            numbers.append(n)
    if len(numbers) < 2:
        raise MergeTasksError("need at least two distinct issues to merge")
    numbers.sort()
    survivor = survivor_override if survivor_override is not None else numbers[0]
    if survivor not in numbers:
        raise MergeTasksError(
            f"survivor ENG-{survivor} is not among the issues to merge"
        )
    return {
        "survivor": f"ENG-{survivor}",
        "ids": [f"ENG-{n}" for n in numbers],
    }


def is_meta_glob(glob: str) -> bool:
    """True when a path glob sits on the agent-infra meta surface."""
    g = glob.strip()
    while g.startswith("./"):
        g = g[2:]
    g = g.rstrip("/")
    if g == "CLAUDE.md":
        return True
    return any(g == base or g.startswith(base + "/") for base in META_BASES)


def _is_touches_line(line: str) -> bool:
    s = line.strip()
    for marker in ("- ", "* "):
        if s.startswith(marker):
            s = s[len(marker) :]
            break
    return s.startswith("**Touches**:")


def raw_touches_lines(body: str) -> list[str]:
    """The ``**Touches**:`` line(s) exactly as **stored**, unparsed.

    :func:`extract_touches` returns the *globs*, trimmed and stripped of their
    surrounding backticks — useful for the union, useless as a ``patch`` anchor,
    which has to match the stored text byte for byte (a stored line may carry
    backticks, an odd list marker, or trailing whitespace). This returns the raw
    lines so the anchor is built from what Linear actually holds.
    """
    return [line for line in body.splitlines() if _is_touches_line(line)]


_PART_HEADING_RE = re.compile(r"^#\s+Part\s+(\d+)\s*(?:—|-|–|$)", re.MULTILINE)


def highest_part_number(body: str) -> int:
    """The largest ``# Part N`` heading already in ``body``, or 0 if none.

    A survivor accumulates folded sections over time, so the next fold has to
    continue its numbering. Matches only a top-level ``# Part N`` heading, so a
    mention of "Part 3" in prose can't inflate the count.
    """
    numbers = [int(m.group(1)) for m in _PART_HEADING_RE.finditer(body)]
    return max(numbers) if numbers else 0


def extract_touches(body: str) -> tuple[str, list[str]]:
    """Split a body into (body with its ``**Touches**:`` line(s) removed, the
    globs those lines carried). ``**Fingerprint**:`` and every other line stay.
    Trailing blank lines left by a removed line are trimmed to one."""
    kept: list[str] = []
    globs: list[str] = []
    for line in body.splitlines():
        if _is_touches_line(line):
            rest = line.split("**Touches**:", 1)[1]
            for g in rest.split(","):
                g = g.strip().strip("`").strip()
                if g and g not in globs:
                    globs.append(g)
            continue
        kept.append(line)
    return "\n".join(kept).rstrip() + "\n", globs


def strip_claude_prefix(title: str) -> str:
    return title[len(CLAUDE_PREFIX) :] if title.startswith(CLAUDE_PREFIX) else title


# Any Linear tag in an anchor is fatal: Linear rewrites `ENG-123` into an
# issue-mention node, so the stored text is an element, not the literal string,
# and the anchor can never match. See `docs/conventions/linear-automation.md`
# → "Partial edits — the `patch` argument".
_ENG_TAG_RE = re.compile(r"ENG-\d+")


def build_patch_ops(
    survivor_body: str,
    part_sections: list[str],
    union_globs: list[str],
) -> tuple[list[dict] | None, str]:
    """Express the fold as ``patch`` ops instead of a whole-body rewrite.

    Returns ``(ops, fallback_reason)``. ``ops`` is ``None`` when the fold cannot
    be expressed safely, and ``fallback_reason`` then says why — the caller keeps
    the wholesale ``description`` for exactly that case. On success
    ``fallback_reason`` is the empty string.

    The point is that the survivor's **existing** body never has to be re-sent:
    each folded ``# Part`` is an ``append`` (which needs no anchor at all), and
    the one thing that must change *in place* — the survivor's ``**Touches**:``
    line, which becomes the union — is a single ``replace``. Ops apply in order,
    so the line is deleted where it sat and the union re-appended at the end,
    reproducing the wholesale layout rather than leaving the union stranded
    above the folded parts.

    Two anchor rules, both from the convention, both enforced by falling back
    rather than emitting an op that would fail at save time:

    * The anchor must match the stored body **exactly once**. More than one
      ``**Touches**:`` line, or a line whose text recurs, is ambiguous.
    * The anchor must carry **no** ``ENG-###``. A ``# Part N — <title>`` heading
      is derived from an issue title and may well contain a tag, which is why no
      anchor is ever built from a title — headings only ever ride in appended
      text.
    """
    raw = raw_touches_lines(survivor_body)
    if len(raw) > 1:
        return None, (
            f"survivor carries {len(raw)} **Touches**: lines, so the replace "
            f"anchor is ambiguous"
        )

    ops: list[dict] = []
    remaining = survivor_body
    if raw:
        line = raw[0]
        if _ENG_TAG_RE.search(line):
            return None, (
                "survivor's **Touches**: line carries an ENG-### tag, which "
                "Linear stores as a mention node, so no anchor can match it"
            )
        # Swallow the preceding newline too, so deleting the line doesn't leave a
        # stray blank where it sat. When the line is the body's *first*, there is
        # no preceding newline to swallow, so anchor on the bare line — otherwise
        # a Touches-first body could never anchor and would be misreported as
        # "not unique".
        anchor = "\n" + line
        if survivor_body.startswith(line):
            anchor = line
        if survivor_body.count(anchor) != 1:
            return None, (
                "survivor's **Touches**: line does not occur exactly once in the "
                "stored body, so the replace anchor is not unique"
            )
        ops.append({"op": "replace", "old_string": anchor, "new_string": ""})
        remaining = survivor_body.replace(anchor, "", 1)

    # An `append` can't strip what's already there, so the first one has to
    # supply exactly the newlines missing from the body's own tail — otherwise a
    # stored body ending in "\n" grows a stray blank line the wholesale path
    # (which `rstrip()`s before joining) never produces. Beyond two the tail is
    # already over-separated and appending nothing is the closest we can get.
    trailing = len(remaining) - len(remaining.rstrip("\n"))
    lead = "\n" * max(0, 2 - trailing)

    for section in part_sections:
        ops.append({"op": "append", "text": f"{lead}{section}"})
        # Every section ends with body text, so subsequent appends always need
        # the full separator.
        lead = "\n\n"

    if union_globs:
        ops.append(
            {"op": "append", "text": f"{lead}**Touches**: {', '.join(union_globs)}"}
        )

    # `patch` is capped at 50 ops; a fold this wide is better done wholesale than
    # rejected by the API mid-merge.
    if len(ops) > 50:
        return None, f"the fold needs {len(ops)} ops, over Linear's 50-op cap"

    if not ops:
        # Reachable when the survivor has no Touches line, nothing folds in, and
        # the union is empty. `patch` requires at least one op, so an empty array
        # would be rejected — and a caller testing `if patch_ops:` would fall
        # through to wholesale with an *empty* reason, which reads as a bug.
        return None, "the fold produced no operations, so there is nothing to patch"

    return ops, ""


def assemble(data: dict) -> dict:
    """Build the merged issue from the survivor + folded issues."""
    survivor_id = data.get("survivor")
    issues = data.get("issues") or []
    by_id = {i["id"]: i for i in issues}
    if survivor_id not in by_id:
        raise MergeTasksError(f"survivor {survivor_id} not in the issues list")

    survivor = by_id[survivor_id]
    others = sorted(
        (i for i in issues if i["id"] != survivor_id),
        key=lambda i: i.get("number", 0),
    )
    if not others:
        raise MergeTasksError("nothing to fold in — only the survivor was given")

    union_globs: list[str] = []
    meta_count = 0
    non_meta_count = 0

    def absorb(globs: list[str]) -> None:
        for g in globs:
            if g not in union_globs:
                union_globs.append(g)

    # An issue counts as meta-work only if *every* one of its globs is meta.
    def issue_is_meta(globs: list[str]) -> bool:
        return bool(globs) and all(is_meta_glob(g) for g in globs)

    survivor_body, survivor_globs = extract_touches(survivor.get("description") or "")
    absorb(survivor_globs)
    if issue_is_meta(survivor_globs):
        meta_count += 1
    elif survivor_globs:
        non_meta_count += 1

    # Continue the survivor's existing numbering rather than restarting at 1.
    # A survivor that has been folded into before already carries `# Part 1 …`
    # through `# Part N`, and emitting a second `# Part 1` produces a body with
    # two sections of the same name — which has had to be hand-corrected more
    # than once, including on the issue that reported this.
    first_part = highest_part_number(survivor_body) + 1

    part_sections: list[str] = []
    for n, other in enumerate(others, start=first_part):
        body, globs = extract_touches(other.get("description") or "")
        absorb(globs)
        if issue_is_meta(globs):
            meta_count += 1
        elif globs:
            non_meta_count += 1
        heading = (
            f"# Part {n} — {strip_claude_prefix(other.get('title') or other['id'])}"
        )
        part_sections.append(f"---\n\n{heading}\n\n{body.rstrip()}")

    description = "\n\n".join([survivor_body.rstrip(), *part_sections])
    if union_globs:
        description += f"\n\n**Touches**: {', '.join(union_globs)}"

    # The ops are built from the survivor's *stored* body (Touches line intact),
    # not the stripped `survivor_body` the wholesale path assembles from.
    patch_ops, patch_fallback_reason = build_patch_ops(
        survivor.get("description") or "", part_sections, union_globs
    )

    # The prefix applies only when **every** folded issue is provably
    # meta-work (all its globs meta) — so a no-touch issue, which can't be
    # proven meta, withholds the prefix rather than silently mislabeling
    # possible product work as meta.
    all_meta = meta_count == len(issues)
    title = survivor.get("title") or survivor_id
    if all_meta and not title.startswith(CLAUDE_PREFIX):
        title = CLAUDE_PREFIX + title

    # Cross-area: the merge mixes meta-work issues with product-code ones.
    cross_area = meta_count > 0 and non_meta_count > 0

    return {
        "title": title,
        "description": description,
        "patch_ops": patch_ops,
        "patch_fallback_reason": patch_fallback_reason,
        "touches": union_globs,
        "all_meta": all_meta,
        "cross_area": cross_area,
    }


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def _write_private(path: str, text: str) -> None:
    """Write a handoff payload owner-only (``0o600``).

    Both handoffs here carry **full Linear issue bodies** into a shared temp
    directory, so they get the same treatment as ``review_diff.py``'s diff and
    ``run_quiet.py``'s captured logs rather than the umask default.
    """
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        fh.write(text)


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="merge_tasks.py")
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_plan = sub.add_parser("plan", help="resolve the deduped numbers + survivor")
    p_plan.add_argument("--survivor", type=int, default=None)
    p_plan.add_argument("tokens", nargs="+")

    p_asm = sub.add_parser("assemble", help="build the merged issue body")
    p_asm.add_argument("issues_json", help="path to the fetched-issues JSON file")
    p_asm.add_argument(
        "--out",
        default=None,
        help="write the merged description here and keep it out of stdout",
    )
    p_asm.add_argument(
        "--ops-out",
        default=None,
        help="write the patch ops JSON here and keep it out of stdout",
    )

    args = parser.parse_args(argv[1:])

    if args.cmd == "plan":
        result = plan(args.tokens, args.survivor)
    else:
        with open(args.issues_json, encoding="utf-8") as fh:
            data = json.load(fh)
        result = assemble(data)
        if args.out is not None:
            # File-handoff: the merged body is the large payload, so write it
            # out and replace it with its path — the skill passes the path to
            # save_issue, never echoing the body through context.
            _write_private(args.out, result["description"])
            del result["description"]
            result["description_path"] = args.out
        if args.ops_out is not None:
            # Same handoff for the ops. The ops carry only the *folded* bodies —
            # the survivor's existing text is never in them — so reading this
            # file costs strictly less than reading the merged description.
            ops = result["patch_ops"]
            del result["patch_ops"]
            if ops is None:
                result["patch_ops_path"] = None
                result["patch_ops_count"] = 0
            else:
                _write_private(args.ops_out, json.dumps(ops, indent=2))
                result["patch_ops_path"] = args.ops_out
                result["patch_ops_count"] = len(ops)

    json.dump(result, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except MergeTasksError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
