#!/usr/bin/env python3
"""``merge-tasks`` consolidation helper — the deterministic parts of folding
several Linear issues into one: number parsing/dedup, survivor resolution, body
assembly, and the ``**Touches**:`` union. The skill drives the Linear MCP reads
and writes; this tool never touches the network.

**On ``**Touches**:``, which is retired.** No filing emits that field any more
(``CLAUDE.md`` → "Structured filing fields"). The union logic stays because
issues filed before the retirement still carry a line, and a fold must not
silently drop what one declared. It is **tolerate-and-carry, never invent**: the
union is computed from globs found in the folded bodies, and a fold of issues
that have none emits no line at all — which every branch below already handles,
since the field was optional from the start. Do not add a path that manufactures
globs.

Three subcommands, each printing JSON to stdout:

* ``plan [--survivor N] TOKEN...`` — parse the issue numbers the user passed
  (bare ``615`` or ``ENG-615``, any case, any order), **dedup** them, and
  resolve the survivor (the lowest-numbered by default, or ``--survivor N``).
  Prints ``{"survivor": "ENG-###", "ids": [...]}`` (ids sorted by number) so the
  skill knows what to fetch before assembling.
* ``fetch --survivor N --out PATH TOKEN...`` — read those issues' bodies over
  GraphQL **in this process** and write the ``assemble`` input file, printing
  only identifiers and a byte count. This is the zero-echo half, and it is where
  the cost was: folded bodies used to transit context roughly **three** times
  per fold — each ``get_issue`` echo, the hand-composed ``Write`` of this very
  file, and the ``Read`` of the generated ops — and about **50k of one planning
  session's ~135k output** was body re-emission. This removes the first two.

  The third is **structural and stays**: the ops are applied through the MCP
  ``patch`` path, whose anchor matching with atomic abort is load-bearing safety
  (see ``board_batch.py`` → "Body edits stay on the MCP ``patch`` path"), and
  handing those ops to an MCP call means reading them. Moving the write to raw
  GraphQL would drop that safety to save the smallest of the three — a bad
  trade. Note it shrank anyway: with ``**Touches**:`` retired a fold of
  post-retirement issues is **appends only**, so the ops carry no ``replace``
  and no anchor.

  Interim technique worth keeping for a fold of very large *legacy* survivors:
  a survivor-body **skeleton** — its headings plus the exact stored
  ``**Touches**:`` line — satisfies ``assemble`` without re-emitting a 30KB
  survivor, and is safe because the patch path never re-sends the survivor body.
  It saved ~10k on the largest measured fold. ``fetch`` makes it unnecessary.
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
     per ``# Part`` section plus — only when a folded body carries legacy globs
     — one ``replace`` swapping the survivor's
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

import linear_api

ENDPOINT = linear_api.ENDPOINT

_FETCH_QUERY = """
query MergeFetch($filter: IssueFilter, $first: Int!) {
  issues(filter: $filter, first: $first) {
    nodes {
      id
      identifier
      number
      title
      description
    }
  }
}
"""

# Path bases (besides the file CLAUDE.md) that count as agent-infra "meta-work"
# — the surface the ``Claude:`` issue-title prefix batches. The canonical
# definition is ``docs/conventions/linear-automation.md`` → "The Claude:
# meta-work prefix"; keep this copy in sync with it.
#
# ``cfg`` is in the set because the lint config and the cspell dictionary are
# what the agent material drives: a meta batch that wires a hook or adds a
# spelling escape edits them, and without this a batch that plainly IS
# meta-work would fail the test and lose its prefix.
META_BASES = (".claude", "docs/conventions", "cfg")

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


_FENCE_RE = re.compile(r"^\s*(```+|~~~+)")
#: Matches every heading depth markdown has, `######` included, so the depth
#: cap below is what actually enforces the bound. An earlier `{1,5}` here made
#: the guard unreachable — `len(group(1)) < 6` could not be false — leaving two
#: sources of truth for one rule, of which only the silent one was live.
_HEADING_RE = re.compile(r"^(#{1,6})(\s)")

#: Markdown's deepest heading. A body already at `######` cannot be demoted, so
#: it is left alone rather than silently growing a seventh `#` that renders as
#: literal text.
_MAX_HEADING_DEPTH = 6


def demote_headings(body: str) -> str:
    """Push every heading in ``body`` one level deeper.

    A folded body is appended **under** an outer ``# Part N`` heading, so any
    top-level heading it carries of its own collides with the outer numbering.
    A trim-lever fold always has that shape — each folded issue is itself a
    ``# Part 1``…``# Part 5`` document — and the result is two interleaved sets
    of the same section names in one issue. The 2026-08-25 housekeeping pass
    renumbered them by hand.

    Fenced blocks are skipped, because a `#` at the start of a line inside one
    is a shell comment, not a heading — demoting it would edit the code sample.
    """
    out: list[str] = []
    fence: str | None = None
    for line in body.split("\n"):
        match = _FENCE_RE.match(line)
        if match:
            marker = match.group(1)
            if fence is None:
                fence = marker[0]
            elif marker[0] == fence:
                fence = None
            out.append(line)
            continue
        if fence is None:
            heading = _HEADING_RE.match(line)
            if heading and len(heading.group(1)) < _MAX_HEADING_DEPTH:
                line = "#" + line
        out.append(line)
    return "\n".join(out)


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

    # Every append leads with a full blank line — the first one included.
    #
    # This used to subtract the body's own trailing newlines from the separator,
    # to avoid a stray blank line the wholesale path never produces. The trouble
    # is that the count was taken from *our* copy of the body, while the one that
    # matters is in Linear's **stored** text — and the two need not agree. When the
    # stored tail carried one newline fewer, the first append emitted "\n---" onto
    # a body ending mid-paragraph, storing that paragraph directly above a dash
    # rule. That is setext heading syntax, so Linear's round trip re-parsed the
    # paragraph as an H2: observed twice in one session, each costing a follow-up
    # patch write to repair.
    #
    # The asymmetry decides it. One newline too many is an invisible blank line;
    # one too few silently rewrites the survivor's prose into a heading. So always
    # send two, and never depend on trailing whitespace we cannot see.
    lead = "\n\n"

    for section in part_sections:
        ops.append({"op": "append", "text": f"{lead}{section}"})

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
        # Demote the folded body's own headings, so its `# Part 1` becomes a
        # `## Part 1` nested under this section rather than a second top-level
        # series colliding with the outer numbering.
        nested = demote_headings(body.rstrip())
        part_sections.append(f"---\n\n{heading}\n\n{nested}")

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


def _post(api_key: str, query: str, variables: dict) -> dict:
    """POST a GraphQL operation and return its ``data``.

    Delegates to the shared transport rather than keeping a fourth HTTP idiom
    in this directory — which is also what gives this tool the redirect refusal
    (a followed 3xx would re-send the ``Authorization`` header to a new host).
    """
    return linear_api.post(
        api_key,
        query,
        variables,
        endpoint=ENDPOINT,
        error=MergeTasksError,
    )


def fetch(api_key: str, survivor: int, numbers: list[int]) -> dict:
    """Fetch the bodies for ``numbers`` and return the ``assemble`` input shape.

    **This is the zero-echo half of the fold, and it is the bulk of the cost.**
    Measured: folded bodies used to transit context roughly *three* times per
    fold — once as each ``get_issue`` echo, once as the ``Write`` composing this
    same JSON by hand, and once as the ``Read`` of the generated ops. Roughly
    **50k of one planning session's ~135k output** was body re-emission. The
    first two of those are pure waste: the tool can read the bodies in its own
    process and write the file itself, which is what this does.

    One GraphQL call for the whole set, not one per issue — the filter takes a
    number list, so a five-issue fold is one round trip.

    Every issue asked for must come back. A silently-short result would produce
    a fold that *looks* complete while dropping an issue's content, and the
    non-survivor would then be canceled as a duplicate of a survivor that never
    absorbed it — losing the content outright.
    """
    data = _post(
        api_key,
        _FETCH_QUERY,
        {
            "filter": {"number": {"in": numbers}},
            "first": max(len(numbers), 1),
        },
    )
    nodes = (data.get("issues") or {}).get("nodes") or []
    got = {node.get("number") for node in nodes}
    missing = sorted(set(numbers) - got)
    if missing:
        raise MergeTasksError(
            "Linear returned no issue for: "
            + ", ".join(f"ENG-{n}" for n in missing)
            + " — refusing to assemble a partial fold"
        )
    survivor_id = next(
        (n.get("identifier") for n in nodes if n.get("number") == survivor), None
    )
    if survivor_id is None:
        raise MergeTasksError(f"the survivor ENG-{survivor} is not in the fetched set")

    # **Normalize `id` to the IDENTIFIER, not the UUID.** `assemble` keys on
    # `id` and was written against the MCP's shape, where `id` *is* `ENG-###`.
    # Raw GraphQL calls that field `identifier` and uses `id` for the UUID, so
    # passing nodes through untouched hands `assemble` a survivor key it can
    # never match. Caught by a test that composes the two — which is the point
    # of having one: the two halves are only useful together.
    # The UUID is deliberately **not** carried. Nothing downstream takes one:
    # `assemble` keys on the identifier, and the skill's close-out writes go
    # through the MCP, which addresses issues as `ENG-###` too. Emitting a
    # second id "in case" would just be an unused field that a later reader has
    # to work out is dead — and picking the wrong one of two id-shaped keys is
    # exactly the bug the normalization above exists to prevent.
    return {
        "survivor": survivor_id,
        "issues": [
            {
                "id": node.get("identifier"),
                "number": node.get("number"),
                "title": node.get("title"),
                "description": node.get("description") or "",
            }
            for node in sorted(nodes, key=lambda n: n.get("number") or 0)
        ],
    }


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="merge_tasks.py")
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_plan = sub.add_parser("plan", help="resolve the deduped numbers + survivor")
    p_plan.add_argument("--survivor", type=int, default=None)
    p_plan.add_argument("tokens", nargs="+")

    p_fetch = sub.add_parser(
        "fetch",
        help="read the issue bodies over GraphQL and write the assemble input "
        "— so no body transits context",
    )
    p_fetch.add_argument("--survivor", type=int, required=True)
    p_fetch.add_argument(
        "--out", required=True, help="where to write the fetched-issues JSON"
    )
    p_fetch.add_argument("tokens", nargs="+")

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
    elif args.cmd == "fetch":
        api_key = os.environ.get("LINEAR_API_KEY", "").strip()
        if not api_key:
            raise MergeTasksError(
                "LINEAR_API_KEY is unset — the fetch mode reads the bodies "
                "itself, so it needs the key (see "
                "docs/conventions/linear-automation.md)"
            )
        numbers = [parse_token(t) for t in args.tokens]
        fetched = fetch(api_key, args.survivor, sorted(set(numbers)))
        _write_private(args.out, json.dumps(fetched, indent=2))
        # Report counts and identifiers only. Echoing a body here would undo
        # the entire point of the mode.
        result = {
            "issues_json_path": args.out,
            "survivor": fetched["survivor"],
            # `id` carries the identifier post-normalization — see `fetch`.
            "fetched": [n.get("id") for n in fetched["issues"]],
            "bytes": sum(len(n.get("description") or "") for n in fetched["issues"]),
        }
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
