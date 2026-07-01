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
* ``assemble ISSUES_JSON`` — given a JSON file
  ``{"survivor": "ENG-###", "issues": [{id, number, title, description}, ...]}``,
  build the merged issue: the survivor body followed by each non-survivor body
  as a labeled ``# Part N — <title>`` section (every ``**Fingerprint**:`` line
  preserved verbatim), one consolidated ``**Touches**:`` line holding the union
  of all the globs, the title (``Claude:`` prefix applied when every folded
  issue is meta-work), and a cross-area flag when the set mixes meta and product
  surfaces. Prints ``{title, description, touches, all_meta, cross_area}``.

Stdlib only. This is a Python skill-tool under ``.claude/tools/`` — deliberately
**not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import json
import re
import sys

# Path bases (besides the file CLAUDE.md) that count as agent-infra "meta-work"
# — the surface the ``Claude:`` issue-title prefix batches. The canonical
# definition is ``docs/conventions/linear-automation.md`` → "The Claude:
# meta-work prefix"; keep this copy in sync with it.
META_BASES = (".claude", "docs/conventions", "tools")

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

    sections = [survivor_body.rstrip()]
    for n, other in enumerate(others, start=1):
        body, globs = extract_touches(other.get("description") or "")
        absorb(globs)
        if issue_is_meta(globs):
            meta_count += 1
        elif globs:
            non_meta_count += 1
        heading = (
            f"# Part {n} — {strip_claude_prefix(other.get('title') or other['id'])}"
        )
        sections.append(f"---\n\n{heading}\n\n{body.rstrip()}")

    description = "\n\n".join(sections)
    if union_globs:
        description += f"\n\n**Touches**: {', '.join(union_globs)}"

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
        "touches": union_globs,
        "all_meta": all_meta,
        "cross_area": cross_area,
    }


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def run(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="merge_tasks.py")
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_plan = sub.add_parser("plan", help="resolve the deduped numbers + survivor")
    p_plan.add_argument("--survivor", type=int, default=None)
    p_plan.add_argument("tokens", nargs="+")

    p_asm = sub.add_parser("assemble", help="build the merged issue body")
    p_asm.add_argument("issues_json", help="path to the fetched-issues JSON file")

    args = parser.parse_args(argv[1:])

    if args.cmd == "plan":
        result = plan(args.tokens, args.survivor)
    else:
        with open(args.issues_json, encoding="utf-8") as fh:
            data = json.load(fh)
        result = assemble(data)

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
