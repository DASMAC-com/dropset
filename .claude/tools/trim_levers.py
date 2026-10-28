#!/usr/bin/env python3
"""Probe trim-lever fingerprints and compose the fold analyst's batches.

The fold analyst — the ``plan`` bootstrap's sub-agent running ``trim-context``
— reads the day's session digest, judges which levers deserve a task, and files
small themed ``Claude:`` batches straight into Backlog. This tool serves the two
deterministic acts in that loop:

- ``probe`` — does an issue already carry this ``**Fingerprint**:``, in any
  state? A **canceled** match is a recorded rejection and suppresses the lever
  permanently; an open match is a batch to fold into rather than a sibling to
  file. Titles and urls only — never a body.
- ``compose`` — the batch bodies, from a parts file the analyst writes: one
  ``## <fingerprint> | <title>`` section per part. It numbers the parts, demotes
  their headings under ``# Part N``, carries exactly one ``**Fingerprint**:`` per
  part, and refuses a part assigned to no batch or to two.

Filing goes through ``linear_issue.py create`` (and a fold into an existing
issue through ``linear_issue.py append``), both zero-echo. The parked
``Trim levers`` lane this tool used to write — ``file``, ``append-evidence``,
``list`` — is retired: a session files nothing that is only a trim, and the
digest is the analyst's whole input.

Subcommands::

    python3 .claude/tools/trim_levers.py probe --fingerprint session-metrics:foo

    # N batches in one call, each numbering its parts from 1.
    python3 .claude/tools/trim_levers.py compose --parts-file <scratchpad>/parts.md \\
        --groups-file <scratchpad>/groups.json --out-dir <scratchpad>/tasks

    # One body — e.g. a part folded into an existing batch, continuing at Part 4.
    python3 .claude/tools/trim_levers.py compose --parts-file <scratchpad>/parts.md \\
        --out <scratchpad>/fold.md --start 4

``probe`` reads ``LINEAR_API_KEY`` and ``LINEAR_PROJECT_ID`` from the
environment — never a hard-coded UUID, per ``CLAUDE.md`` → "Linear automation";
``compose`` touches no API. It writes no relations, ever. Stdlib only; a Python
skill-tool under ``.claude/tools/`` — deliberately **not** a Cargo workspace
member. Tests live in ``tests/test_trim_levers.py``, run via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys

import linear_api

ENDPOINT = linear_api.ENDPOINT

# Overall per-request timeout, so a hung endpoint can't wedge a run.
REQUEST_TIMEOUT = 30

# One page of a probe read. The read follows the cursor anyway, because the
# sibling board tool shipped a one-page guard and every write on it failed the
# day the project crossed that size.
PAGE_SIZE = 100

# Runaway backstop on a cursor-following read.
MAX_PAGES = 40

# A fingerprint is ``<domain-token>:<slug>``. The domain half must be **dotless**:
# Linear linkifies a hostname-valid basename, which silently rewrites the stored
# key and breaks the dedup probe that is this pipeline's only guard against
# refiling. Roughly 40 stored keys were corrupted this way before the rule was
# written down, so it is enforced here rather than trusted to the caller.
FINGERPRINT_RE = re.compile(r"^[a-z0-9][a-z0-9-]*:[a-z0-9][a-z0-9._/-]*$")

# A fenced code block, opening or closing. Part bodies quote filing examples, so
# a `**Field**:` line inside a fence is an illustration, not a field — see
# `field_values`. Kept identical in shape to `read_result.py`'s guard.
FENCE_RE = re.compile(r"^\s*(```+|~~~+)")


class TrimLeversError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def env_var(name: str) -> str:
    return linear_api.env_var(name, error=TrimLeversError)


def _post(api_key: str, query: str, variables: dict) -> dict:
    """POST a GraphQL operation and return its ``data``.

    Delegates to the shared transport, which refuses redirects so a 3xx can
    never re-send the ``Authorization`` header to another host. Errors surface
    as :class:`TrimLeversError` so the CLI never emits a traceback (which could
    quote the credential).
    """
    return linear_api.post(
        api_key,
        query,
        variables,
        endpoint=ENDPOINT,
        timeout=REQUEST_TIMEOUT,
        error=TrimLeversError,
    )


# --------------------------------------------------------------------------
# Pure helpers
# --------------------------------------------------------------------------


def validate_fingerprint(key: str) -> str:
    """The fingerprint, normalized, or a hard error naming the rule it broke."""
    key = key.strip()
    if not key:
        raise TrimLeversError("--fingerprint is empty")
    if ":" not in key:
        raise TrimLeversError(
            f"fingerprint {key!r} needs a <domain-token>:<slug> shape"
        )
    domain = key.split(":", 1)[0]
    if "." in domain:
        raise TrimLeversError(
            f"fingerprint domain token {domain!r} contains a dot — Linear "
            "linkifies a hostname-valid basename and corrupts the stored key; "
            "use a dotless domain (e.g. 'feeds-http', not 'http.rs')"
        )
    if not FINGERPRINT_RE.match(key):
        raise TrimLeversError(
            f"fingerprint {key!r} must be lowercase <domain-token>:<slug>"
        )
    return key


def field_line_re(field: str, value: str | None = None) -> re.Pattern:
    """A line-anchored matcher for a ``**Field**: value`` line.

    Anchored, and not a substring test. A substring test gets this wrong in both
    directions: a fingerprint merely *mentioned in prose* reads as the field,
    and a nested slug (``a:foo`` inside ``a:foo-bar``) reads as a match. Both
    break the dedup this pipeline rests on.
    """
    tail = re.escape(value) + r"\s*$" if value is not None else r".*$"
    return re.compile(rf"^\*\*{re.escape(field)}\*\*:\s*{tail}", re.MULTILINE)


def field_values(body: str, field: str) -> list[str]:
    """Every value carried by a ``**Field**: value`` line, **outside a fence**.

    A part *about filing conventions* legitimately quotes a fenced example
    showing ``**Fingerprint**: <domain>:<slug>``, which would otherwise read as
    a second, foreign key. (Its sibling parser in ``read_result.py`` carries the
    same guard; the two must not disagree about what a fence is.)
    """
    pattern = re.compile(rf"^\*\*{re.escape(field)}\*\*:\s*(.*)$")
    out: list[str] = []
    in_fence = False
    for line in body.splitlines():
        if FENCE_RE.match(line):
            in_fence = not in_fence
            continue
        if not in_fence:
            m = pattern.match(line)
            if m:
                out.append(m.group(1))
    return out


# --------------------------------------------------------------------------
# Linear operations
# --------------------------------------------------------------------------

# Selects `description` because the exact line-anchored match in `probe` needs
# it — the server-side `contains` filter is only a pre-filter. The body is read
# in this process and never printed, which is the zero-echo property.
_PROBE_QUERY = """
query LeverProbe($filter: IssueFilter, $first: Int!, $after: String) {
  issues(filter: $filter, first: $first, after: $after, includeArchived: true) {
    pageInfo { hasNextPage endCursor }
    nodes {
      identifier
      url
      title
      description
      state { name type }
      projectMilestone { name }
    }
  }
}
"""


def _paged(api_key: str, issue_filter: dict, query: str = _PROBE_QUERY) -> list[dict]:
    """Every issue matching ``issue_filter``, following the cursor."""
    nodes: list[dict] = []
    after: str | None = None
    for _ in range(MAX_PAGES):
        data = _post(
            api_key,
            query,
            {"filter": issue_filter, "first": PAGE_SIZE, "after": after},
        )
        conn = data.get("issues") or {}
        page = conn.get("nodes") or []
        nodes.extend(page)
        info = conn.get("pageInfo") or {}
        if not info.get("hasNextPage"):
            # A full page with no pageInfo at all is not evidence of a complete
            # read — refuse rather than report a possibly-truncated set as whole.
            if not info and len(page) >= PAGE_SIZE:
                raise TrimLeversError(
                    f"read returned a full page of {PAGE_SIZE} with no pageInfo — "
                    "refusing to treat a possibly-truncated result as complete"
                )
            return nodes
        after = info.get("endCursor")
        if not after:
            raise TrimLeversError("Linear reported another page but returned no cursor")
    raise TrimLeversError(f"read did not terminate within {MAX_PAGES} pages")


def probe(api_key: str, project_id: str, fingerprint: str) -> list[dict]:
    """Issues whose body carries ``fingerprint`` as a field, in **any** state.

    Archived and completed issues are included deliberately: a lever that was
    rejected is closed *with its reason*, and dedup-against-resolved is what makes
    that rejection permanent rather than something the next fold re-proposes on
    intuition.

    **The server filter is a substring pre-filter; the exact match happens here.**
    ``description: {contains: …}`` is a substring test, and slugs nest: a probe
    for ``a:search-scope`` matches the issue carrying ``a:search-scope-axis``.
    So the body is fetched for the (few) candidates and matched line-anchored in
    this process.
    """
    matcher = field_line_re("Fingerprint", fingerprint)
    candidates = _paged(
        api_key,
        {
            "project": {"id": {"eq": project_id}},
            "description": {"contains": fingerprint},
        },
    )
    return [c for c in candidates if matcher.search(c.get("description") or "")]


# --------------------------------------------------------------------------
# Compose
# --------------------------------------------------------------------------

_PART_SECTION_RE = re.compile(r"^## (\S+) \| (.*)$")


def demote_headings(body: str) -> str:
    """Push every ATX heading down one level, leaving fenced blocks alone.

    A part body pasted under a ``# Part N`` heading with its own ``#``-level
    headings collides at the same level and the batch loses its structure —
    silent damage: the body is all there and merely reads as one flat document.
    A fenced ``#`` is quoted material, not a heading.
    """
    out: list[str] = []
    in_fence = False
    for line in body.splitlines():
        if FENCE_RE.match(line):
            in_fence = not in_fence
            out.append(line)
            continue
        if not in_fence and line.startswith("#"):
            out.append("#" + line)
        else:
            out.append(line)
    return "\n".join(out)


def split_parts(text: str) -> list[tuple[str, str, str]]:
    """``[(fingerprint, title, body), …]`` from a parts file, validated.

    The section key **is** the part's fingerprint, so a part cannot reach a
    batch without one, and the body's own ``**Fingerprint**:`` line (if the
    analyst wrote one) must agree with it.
    """
    # Scanned line by line with the same fence toggle as `field_values`, so a
    # fenced `## …` example inside a part is quoted material, not a new part.
    # Any other out-of-fence `## ` line is a malformed section heading: merged
    # silently into the part above, it would carry that part's content away.
    sections: list[tuple[re.Match, list[str]]] = []
    preamble: list[str] = []
    malformed: list[str] = []
    in_fence = False
    for line in text.splitlines():
        if FENCE_RE.match(line):
            in_fence = not in_fence
        elif not in_fence and line.startswith("## "):
            match = _PART_SECTION_RE.match(line)
            if match:
                sections.append((match, []))
                continue
            malformed.append(line.strip())
        (sections[-1][1] if sections else preamble).append(line)
    if in_fence:
        raise TrimLeversError(
            "the parts file ends inside an unclosed fence — every section "
            "after its opening line would be swallowed into one part"
        )
    if malformed:
        raise TrimLeversError(
            "these '## ' lines are not '## <fingerprint> | <title>' section "
            f"headings: {'; '.join(malformed)}"
        )
    if not sections:
        raise TrimLeversError(
            "no '## <fingerprint> | <title>' sections found in the parts file"
        )
    if any(line.strip() for line in preamble):
        raise TrimLeversError(
            "the parts file has text before its first section — it would be "
            "dropped; move it into a part or delete it"
        )
    parts: list[tuple[str, str, str]] = []
    seen: set[str] = set()
    repeated: list[str] = []
    foreign: list[str] = []
    empty: list[str] = []
    for match, lines in sections:
        key = validate_fingerprint(match.group(1))
        body = "\n".join(lines).strip("\n")
        if not body.strip():
            empty.append(key)
        # A part owns one key. A body carrying a different field would store
        # two, and a probe for either would then match this batch — the dedup
        # guard failing in the direction hardest to notice.
        values = [v.strip() for v in field_values(body, "Fingerprint")]
        if any(v != key for v in values) or len(values) > 1:
            foreign.append(key)
        if key in seen:
            repeated.append(key)
        seen.add(key)
        parts.append((key, match.group(2).strip(), body))
    if repeated:
        raise TrimLeversError(
            f"these fingerprints head more than one part: {', '.join(repeated)} — "
            "one part per fingerprint"
        )
    if foreign:
        raise TrimLeversError(
            "these parts carry a **Fingerprint**: line that differs from their "
            f"section key, or more than one: {', '.join(foreign)}"
        )
    if empty:
        raise TrimLeversError(f"these parts have an empty body: {', '.join(empty)}")
    return parts


def render_part(number: int, key: str, title: str, body: str) -> str:
    """One ``# Part N`` section, carrying its ``**Fingerprint**:`` exactly once."""
    text = demote_headings(body).rstrip()
    # Fence-aware, like `split_parts`: a fenced example carrying the same key
    # is quoted material, so the real field is still appended.
    if not field_values(text, "Fingerprint"):
        # Joined with a blank line: a field abutting prose is how Linear's round
        # trip has re-parsed a paragraph as a setext heading before.
        text = (
            f"{text}\n\n**Fingerprint**: {key}" if text else f"**Fingerprint**: {key}"
        )
    return f"# Part {number} — {title}\n\n{text}\n"


def compose_one(text: str, *, start: int = 1) -> tuple[str, dict]:
    """Every part in the file as one body, numbered from ``start``.

    ``start`` exists for the near-match fold: a part folded into an existing
    batch continues that batch's numbering rather than restarting at 1.
    """
    if start < 1:
        raise TrimLeversError("--start must be at least 1")
    parts = split_parts(text)
    body = "\n".join(
        render_part(start + i, key, title, raw)
        for i, (key, title, raw) in enumerate(parts)
    )
    return body, {
        "folded": [key for key, _, _ in parts],
        "next_part": start + len(parts),
    }


# The batch size bound: a task is sized to a short session, roughly 4–5 parts.
# Advisory rather than enforced, because the rule is explicitly approximate — a
# hard limit would push callers into splitting a coherent theme to satisfy a tool.
_MAX_GROUP_PARTS = 5

# A body this tool emits, for telling one apart from whatever else a caller keeps
# in the output directory. Matches the `f"{index:02d}-{slug}.md"` shape written
# below, allowing three digits so the hundredth task is still recognized as ours.
_TASK_FILE_RE = re.compile(r"^\d{2,3}-[a-z0-9-]*\.md$")


def _slug(text: str) -> str:
    """A filename-safe slug for a task title."""
    cleaned = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
    return (cleaned[:48].strip("-")) or "task"


def parse_groups_spec(raw: str) -> list[tuple[str, tuple[str, ...]]]:
    """The ``--groups-file`` spec: one ``{title, parts}`` entry per batch.

    A file rather than repeated flags, matching how ``board_batch.py --updates``
    and ``linear_patch.py --ops`` already take assembled work — it keeps the
    call a single bare command whatever the batch count.
    """
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise TrimLeversError(f"groups file is not valid JSON: {exc}") from exc
    if not isinstance(data, list) or not data:
        raise TrimLeversError(
            "groups file must be a non-empty JSON array of "
            '{"title": ..., "parts": [...]} objects, one per batch'
        )

    groups: list[tuple[str, tuple[str, ...]]] = []
    for index, entry in enumerate(data, start=1):
        if not isinstance(entry, dict):
            raise TrimLeversError(f"group {index} is not a JSON object")
        title = entry.get("title")
        parts = entry.get("parts")
        if not isinstance(title, str) or not title.strip():
            raise TrimLeversError(f'group {index} has no non-empty "title"')
        if not isinstance(parts, list) or not parts:
            raise TrimLeversError(
                f'group {index} ("{title}") has no non-empty "parts" array'
            )
        # A title is echoed verbatim into the pipe-delimited summary a skill
        # reads back, so an interior newline or a literal ` | ` can forge a line
        # of machine-shaped output. Reject it at the boundary.
        if any(ch in title for ch in "\n\r|") or any(
            ord(ch) < 32 or ord(ch) == 127 for ch in title
        ):
            raise TrimLeversError(
                f"group {index} has a title containing a newline, a '|', or a "
                "control character — those forge a line in the summary this "
                "tool prints for a caller to parse"
            )
        names: list[str] = []
        for item in parts:
            if not isinstance(item, str) or not item.strip():
                raise TrimLeversError(
                    f'group {index} ("{title}") has a part entry that is not '
                    "a non-empty string"
                )
            names.append(item.strip())
        groups.append((title.strip(), tuple(names)))
    return groups


def compose_groups(
    text: str, groups: list[tuple[str, tuple[str, ...]]]
) -> tuple[list[dict], dict]:
    """One composed body per group, from a single parts file.

    **Parts number from 1 within each batch.** Assignment is checked in full
    before anything is composed: a part in **no** group is the dangerous case —
    it would be filed nowhere while the run looked complete — and a part in
    **two** would land twice.
    """
    by_key = {key: (key, title, raw) for key, title, raw in split_parts(text)}

    assigned: dict[str, int] = {}
    for index, (_, names) in enumerate(groups, start=1):
        for name in names:
            if name in assigned:
                where = (
                    f"twice in group {index}"
                    if assigned[name] == index
                    else f"in more than one group (groups {assigned[name]} and {index})"
                )
                raise TrimLeversError(
                    f"{name} is listed {where} — a part lands in exactly one batch"
                )
            assigned[name] = index

    unknown = sorted(name for name in assigned if name not in by_key)
    if unknown:
        raise TrimLeversError(
            f"these grouped parts are not in the parts file: {', '.join(unknown)}"
        )
    unassigned = [key for key in by_key if key not in assigned]
    if unassigned:
        raise TrimLeversError(
            "these parts are in the parts file but in no group, so composing "
            f"would silently drop them: {', '.join(unassigned)}"
        )

    tasks: list[dict] = []
    oversized: list[tuple[str, int]] = []
    for title, names in groups:
        body = "\n".join(
            render_part(number, *by_key[name])
            for number, name in enumerate(names, start=1)
        )
        if len(names) > _MAX_GROUP_PARTS:
            oversized.append((title, len(names)))
        tasks.append({"title": title, "body": body, "folded": list(names)})

    return tasks, {
        "tasks": len(tasks),
        "folded": sorted(assigned),
        "oversized": oversized,
    }


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def _read_file(path: str, label: str) -> str:
    try:
        with open(path, "r", encoding="utf-8") as fh:
            text = fh.read()
    except OSError as e:
        raise TrimLeversError(f"cannot read {label} {path}: {e}") from e
    except UnicodeDecodeError as e:
        # A UnicodeDecodeError is a ValueError rather than an OSError, so without
        # this it escapes the CLI as a traceback.
        raise TrimLeversError(f"{label} {path} is not valid UTF-8: {e}") from e
    if not text.strip():
        raise TrimLeversError(f"{label} {path} is empty")
    return text


def _write_private(path: str, text: str) -> None:
    """Write ``text`` at mode 0o600, refusing to follow a planted symlink.

    O_NOFOLLOW because the mode protects only content this call creates;
    scratchpads live under a shared /tmp root. The chmod covers re-running over
    an existing looser file, whose mode O_CREAT would leave intact. A missing
    parent directory (a fresh scratchpad) is created rather than failing.
    """
    try:
        os.makedirs(os.path.dirname(path) or ".", mode=0o700, exist_ok=True)
        handle = os.open(
            path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600
        )
        with os.fdopen(handle, "w", encoding="utf-8") as fh:
            fh.write(text)
        os.chmod(path, 0o600)
    except OSError as exc:
        raise TrimLeversError(f"cannot write {path}: {exc}") from exc


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="trim_levers.py",
        description="Probe lever fingerprints and compose the fold's batches.",
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("probe", help="find the issues carrying a fingerprint")
    p.add_argument("--fingerprint", required=True)

    comp = sub.add_parser("compose", help="batch bodies from a parts file")
    comp.add_argument(
        "--parts-file",
        required=True,
        metavar="FILE",
        help="one '## <fingerprint> | <title>' section per part",
    )
    comp.add_argument(
        "--out",
        metavar="FILE",
        help="single-body mode: every part in the file, as one body",
    )
    comp.add_argument(
        "--start",
        type=int,
        default=None,
        help="single-body mode only: the first `# Part N` number (default 1) — "
        "the existing batch's next part when folding into it",
    )
    comp.add_argument(
        "--groups-file",
        metavar="FILE",
        help='batch mode: a JSON array of {"title", "parts"} objects, one per '
        "batch, each numbering its parts from 1. Requires --out-dir",
    )
    comp.add_argument(
        "--out-dir",
        metavar="DIR",
        help="batch mode: where to write the per-batch bodies (created if absent)",
    )

    args = parser.parse_args(argv[1:])

    if args.cmd == "compose":
        if bool(args.out) == bool(args.groups_file):
            parser.error(
                "compose needs exactly one of --out (one body) or "
                "--groups-file (N batches)"
            )
        if args.groups_file and not args.out_dir:
            parser.error("--groups-file requires --out-dir")
        if args.out and args.out_dir:
            parser.error("--out-dir belongs to --groups-file mode, not --out")
        if args.groups_file and args.start is not None:
            parser.error(
                "--start belongs to --out mode; in --groups-file mode each "
                "batch numbers its parts from 1"
            )
    return args


def _compose_batches(args: argparse.Namespace) -> int:
    text = _read_file(args.parts_file, "parts file")
    groups = parse_groups_spec(_read_file(args.groups_file, "groups file"))
    tasks, summary = compose_groups(text, groups)
    try:
        os.makedirs(args.out_dir, exist_ok=True)
    except OSError as exc:
        raise TrimLeversError(f"cannot create {args.out_dir}: {exc}") from exc
    for index, task in enumerate(tasks, start=1):
        path = os.path.join(args.out_dir, f"{index:02d}-{_slug(task['title'])}.md")
        _write_private(path, task["body"])
        task["path"] = path
    print(
        f"trim-levers: composed {summary['tasks']} batch(es) from "
        f"{len(summary['folded'])} part(s) into {args.out_dir}"
    )
    for task in tasks:
        print(
            f"trim-levers: {task['path']} | {len(task['folded'])} part(s) "
            f"| {', '.join(task['folded'])} | {task['title']}"
        )
    # A reused --out-dir keeps bodies this run did not write; filing every file
    # in it would file a phantom batch. Reported, never deleted.
    written = {os.path.basename(task["path"]) for task in tasks}
    stale = sorted(
        name
        for name in os.listdir(args.out_dir)
        if _TASK_FILE_RE.match(name) and name not in written
    )
    if stale:
        print(
            "trim-levers: ADVISORY these files in "
            f"{args.out_dir} are NOT from this run: {', '.join(stale)} — "
            "left by an earlier compose. File only the paths listed above"
        )
    for title, count in summary["oversized"]:
        print(
            f"trim-levers: ADVISORY {count} parts exceeds the {_MAX_GROUP_PARTS}"
            f"-part short-session bound: {title} — split it, or carry it "
            "deliberately"
        )
    return 0


def run(argv: list[str]) -> int:
    args = _parse_args(argv)

    # `compose` is pure local text work, handled before the credential lookup so
    # it runs in a shell with no Linear secrets resolved.
    if args.cmd == "compose" and args.groups_file:
        return _compose_batches(args)
    if args.cmd == "compose":
        start = 1 if args.start is None else args.start
        body, summary = compose_one(
            _read_file(args.parts_file, "parts file"), start=start
        )
        _write_private(args.out, body)
        print(
            f"trim-levers: composed {len(summary['folded'])} part(s) "
            f"({len(body)} chars) into {args.out}"
        )
        print(f"trim-levers: parts {start}..{summary['next_part'] - 1}")
        return 0

    fingerprint = validate_fingerprint(args.fingerprint)
    matches = probe(
        env_var("LINEAR_API_KEY"), env_var("LINEAR_PROJECT_ID"), fingerprint
    )
    if not matches:
        print(f"NONE {fingerprint}")
        return 1
    for m in matches:
        # The type beside the name: a rejection is any `canceled`-type state
        # (Canceled, Duplicate), which the name alone does not say.
        state = m.get("state") or {}
        milestone = (m.get("projectMilestone") or {}).get("name") or "-"
        print(
            f"MATCH {m.get('identifier')} [{state.get('name')}:{state.get('type')}] "
            f"[{milestone}] "
            f"{m.get('url')} | {m.get('title')}"
        )
    return 0


def main() -> int:
    try:
        return run(sys.argv)
    except TrimLeversError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
