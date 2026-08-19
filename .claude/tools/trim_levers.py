#!/usr/bin/env python3
"""File and fold trim levers as parked Linear issues, without paying a body echo.

The producer/consumer pipeline for session trim levers used to run through a
Linear **document** — ``session-metrics`` appended an entry per session,
``trim-context`` mined the document later. That document outgrew the harness's
tool-result cap between drains (67.0k characters at the last one), so each mining
pass spilled it to disk and picked it apart with a hand-written scratchpad script.
With roughly ten parallel sessions a day it crossed the cap between any two
drains, which made the growth structural rather than a tidiness problem.

The ratified replacement, which this tool implements: **one parked issue per
lever**, keyed by its ``**Fingerprint**:``. A recurring lever accumulates evidence
on the issue that already exists, so cross-session recurrence becomes a fact on
the board instead of a pattern a miner has to re-detect in prose, and the
milestone lifecycle is the state machine — no drain bookkeeping survives.

**Why this is a tool and not MCP calls.** ``save_issue`` echoes the entire stored
body back on every write, even a write that sent no body at all. That is a fixed
cost per call which ``patch`` does not reduce, and it *compounds* on an
accumulator: five touches on one issue measured ~53k, with per-touch cost rising
monotonically because each append enlarged what the next would echo. So every
write here goes through raw GraphQL and prints **one line** — identifier and url.
``append-evidence`` does its read-modify-write entirely inside this process, so
the grown body never enters a transcript at all.

``docs/conventions/linear-automation.md`` deliberately keeps body edits on the MCP
``patch`` path; that rule governs interactive filing and planning flows, where a
human is reading along. This is a high-volume automated pipeline, where the echo
is pure waste — the doc states the carve-out explicitly.

Subcommands::

    # Does a lever already exist? Titles and urls only — never a body.
    python3 .claude/tools/trim_levers.py probe --fingerprint session-metrics:foo

    # File a new parked lever (milestone and state set in the CREATING call).
    python3 .claude/tools/trim_levers.py file \\
        --title 'Narrow a search by scope, not only output form' \\
        --fingerprint session-metrics:search-scope-axis \\
        --touches 'docs/conventions/context-economy.md' \\
        --body-file <scratchpad>/lever.md

    # Same lever seen again: append this session's evidence to the existing one.
    python3 .claude/tools/trim_levers.py append-evidence \\
        --fingerprint session-metrics:search-scope-axis \\
        --evidence-file <scratchpad>/evidence.md

    # The fold: what is parked right now, as one compact listing.
    python3 .claude/tools/trim_levers.py list

Every subcommand takes ``--dry-run``. Reads ``LINEAR_API_KEY``,
``LINEAR_PROJECT_ID``, ``LINEAR_TEAM_ID`` and (for ``file``)
``LINEAR_ASSIGNEE_ID`` from the environment — never a hard-coded UUID, per
``CLAUDE.md`` → "Linear automation".

**It writes no relations, ever.** A parked lever is not in the pull queue and is
exempt from the serial meta chain until it is folded; blocking edges are
human-curated in a planning session. Stdlib only; a Python skill-tool under
``.claude/tools/`` — deliberately **not** a Cargo workspace member. Tests live in
``tests/test_trim_levers.py``, run via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.request

ENDPOINT = "https://api.linear.app/graphql"

# Overall per-request timeout, so a hung endpoint can't wedge a run.
REQUEST_TIMEOUT = 30

# The parking milestone. Deliberately distinct from "Audit findings" so the
# planning bootstrap's audit-promotion offer stays audit-scoped and does not start
# sweeping up trim levers.
MILESTONE_NAME = "Trim levers"

# Parked findings sit in Todo, never Backlog: Backlog means pullable, and the
# operator's Next view is the unblocked Backlog, so a parked lever there would
# surface as available work. Promotion in a planning session is what moves a lever
# Todo -> Backlog and clears the milestone.
PARKED_STATE = "Todo"

# One page of a listing read. The parked pool is small by construction — it drains
# through folds — but the read follows the cursor anyway, because the sibling
# board tool shipped a one-page guard and every write on it failed the day the
# project crossed that size.
PAGE_SIZE = 100

# Runaway backstop on a cursor-following read.
MAX_PAGES = 40

# A fingerprint is ``<domain-token>:<slug>``. The domain half must be **dotless**:
# Linear linkifies a hostname-valid basename, which silently rewrites the stored
# key and breaks the dedup probe that is this pipeline's only guard against
# refiling. Roughly 40 stored keys were corrupted this way before the rule was
# written down, so it is enforced here rather than trusted to the caller.
FINGERPRINT_RE = re.compile(r"^[a-z0-9][a-z0-9-]*:[a-z0-9][a-z0-9._/-]*$")


class TrimLeversError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def env_var(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value:
        raise TrimLeversError(f"{name} is unset — export it before running")
    if not value.isprintable():
        # A pasted key with an embedded newline otherwise reaches http.client's
        # header validation, which raises a ValueError quoting the offending
        # value — i.e. leaking the credential into a traceback.
        raise TrimLeversError(f"{name} contains a non-printable character")
    return value


def _post(api_key: str, query: str, variables: dict) -> dict:
    """POST a GraphQL operation and return its ``data``.

    Transport, GraphQL and shape errors all surface as :class:`TrimLeversError`
    so the CLI never emits a traceback (which could quote the credential).
    """
    payload = json.dumps({"query": query, "variables": variables}).encode("utf-8")
    req = urllib.request.Request(
        ENDPOINT,
        data=payload,
        headers={"Content-Type": "application/json", "Authorization": api_key},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=REQUEST_TIMEOUT) as resp:
            raw = resp.read().decode("utf-8")
    except urllib.error.HTTPError as e:
        detail = e.read().decode("utf-8", errors="replace")
        raise TrimLeversError(f"Linear API returned HTTP {e.code}: {detail}") from e
    except urllib.error.URLError as e:
        raise TrimLeversError(f"Linear API request failed: {e.reason}") from e

    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError as e:
        raise TrimLeversError(f"decoding Linear GraphQL response: {e}") from e

    errors = parsed.get("errors")
    if errors:
        joined = "; ".join(e.get("message", "") for e in errors)
        raise TrimLeversError(f"Linear GraphQL error: {joined}")
    data = parsed.get("data")
    if data is None:
        raise TrimLeversError("Linear GraphQL response carried no data")
    return data


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


def compose_body(body: str, fingerprint: str, touches: list[str]) -> str:
    """The stored body: the lever's prose plus its two machine-parsed fields.

    The fields are appended here rather than expected in the prose so every filed
    lever carries them in the same place and spelling — the probe below is only as
    reliable as that consistency.
    """
    parts = [body.rstrip()]
    if f"**Fingerprint**: {fingerprint}" not in body:
        parts.append(f"**Fingerprint**: {fingerprint}")
    if touches:
        parts.append(f"**Touches**: {', '.join(touches)}")
    # Joined with a blank line, and never leaving a field directly under a
    # paragraph: a bare "---" or a field abutting prose is how Linear's round trip
    # has re-parsed a paragraph as a setext heading before.
    return "\n\n".join(parts) + "\n"


def split_touches(raw: str | None) -> list[str]:
    """``--touches`` as an ordered, de-duplicated glob list."""
    if not raw:
        return []
    out: list[str] = []
    for chunk in raw.split(","):
        glob = chunk.strip()
        if glob and glob not in out:
            out.append(glob)
    return out


# --------------------------------------------------------------------------
# Linear operations
# --------------------------------------------------------------------------

# Only identity fields are selected anywhere a body is not strictly needed. That
# selection *is* the zero-echo property — it is not an optimization detail.
_SEARCH_QUERY = """
query Levers($filter: IssueFilter, $first: Int!, $after: String) {
  issues(filter: $filter, first: $first, after: $after, includeArchived: true) {
    pageInfo { hasNextPage endCursor }
    nodes {
      identifier
      url
      title
      state { name type }
      projectMilestone { name }
    }
  }
}
"""

_MILESTONES_QUERY = """
query Milestones($projectId: String!) {
  project(id: $projectId) {
    projectMilestones(first: 100) { nodes { id name } }
  }
}
"""

_STATES_QUERY = """
query States($teamId: String!) {
  team(id: $teamId) {
    states(first: 100) { nodes { id name type } }
  }
}
"""

_CREATE_MUTATION = """
mutation FileLever($input: IssueCreateInput!) {
  issueCreate(input: $input) {
    success
    issue { identifier url }
  }
}
"""

# The read half of append-evidence. This is the ONE place a body is fetched, and
# it is fetched into this process, never into a transcript.
_BODY_QUERY = """
query LeverBody($id: String!) {
  issue(id: $id) { id identifier url description }
}
"""

_UPDATE_MUTATION = """
mutation AppendEvidence($id: String!, $description: String!) {
  issueUpdate(id: $id, input: { description: $description }) {
    success
    issue { identifier url }
  }
}
"""


def _paged(api_key: str, issue_filter: dict) -> list[dict]:
    """Every issue matching ``issue_filter``, following the cursor."""
    nodes: list[dict] = []
    after: str | None = None
    for _ in range(MAX_PAGES):
        data = _post(
            api_key,
            _SEARCH_QUERY,
            {"filter": issue_filter, "first": PAGE_SIZE, "after": after},
        )
        conn = data.get("issues") or {}
        nodes.extend(conn.get("nodes") or [])
        info = conn.get("pageInfo") or {}
        if not info.get("hasNextPage"):
            return nodes
        after = info.get("endCursor")
        if not after:
            raise TrimLeversError(
                "Linear reported another page but returned no cursor"
            )
    raise TrimLeversError(f"read did not terminate within {MAX_PAGES} pages")


def probe(api_key: str, project_id: str, fingerprint: str) -> list[dict]:
    """Issues whose body carries ``fingerprint``, in **any** state.

    Archived and completed issues are included deliberately: a lever that was
    rejected is closed *with its reason*, and dedup-against-resolved is what makes
    that rejection permanent rather than something the next mining pass
    re-proposes on intuition. Nine of thirteen inbox entries carried a
    "do not mine this as waste" note, several written because an earlier pass had
    re-proposed exactly that.
    """
    return _paged(
        api_key,
        {
            "project": {"id": {"eq": project_id}},
            "description": {"contains": fingerprint},
        },
    )


def parked(api_key: str, project_id: str) -> list[dict]:
    """Every lever currently parked under the milestone."""
    return _paged(
        api_key,
        {
            "project": {"id": {"eq": project_id}},
            "projectMilestone": {"name": {"eq": MILESTONE_NAME}},
        },
    )


def resolve_milestone_id(api_key: str, project_id: str) -> str:
    data = _post(api_key, _MILESTONES_QUERY, {"projectId": project_id})
    nodes = ((data.get("project") or {}).get("projectMilestones") or {}).get(
        "nodes"
    ) or []
    for node in nodes:
        if (node.get("name") or "").strip() == MILESTONE_NAME:
            return node["id"]
    available = ", ".join(sorted(str(n.get("name")) for n in nodes)) or "(none)"
    raise TrimLeversError(
        f"no {MILESTONE_NAME!r} milestone on this project — create it once, then "
        f"re-run. Available: {available}"
    )


def resolve_state_id(api_key: str, team_id: str) -> str:
    data = _post(api_key, _STATES_QUERY, {"teamId": team_id})
    nodes = ((data.get("team") or {}).get("states") or {}).get("nodes") or []
    for node in nodes:
        if (node.get("name") or "").strip() == PARKED_STATE:
            return node["id"]
    available = ", ".join(sorted(str(n.get("name")) for n in nodes)) or "(none)"
    raise TrimLeversError(
        f"no {PARKED_STATE!r} workflow state on this team. Available: {available}"
    )


def file_lever(
    api_key: str,
    *,
    project_id: str,
    team_id: str,
    assignee_id: str | None,
    title: str,
    body: str,
    fingerprint: str,
    touches: list[str],
    dry_run: bool,
) -> str:
    """Create one parked lever and return its one-line confirmation.

    Milestone, state and assignee all go in the **creating** call. Filing then
    amending costs a second full body echo and buys nothing — one measured session
    filed an issue in two writes purely to add a relation afterwards.
    """
    existing = probe(api_key, project_id, fingerprint)
    if existing:
        first = existing[0]
        raise TrimLeversError(
            f"fingerprint {fingerprint} already on {first.get('identifier')} "
            f"({first.get('url')}) — append-evidence instead of filing a duplicate"
        )

    description = compose_body(body, fingerprint, touches)
    if dry_run:
        return (
            f"WOULD FILE {fingerprint} | {title} | "
            f"{len(description)} char(s), state {PARKED_STATE}, "
            f"milestone {MILESTONE_NAME}"
        )

    milestone_id = resolve_milestone_id(api_key, project_id)
    state_id = resolve_state_id(api_key, team_id)
    payload = {
        "teamId": team_id,
        "projectId": project_id,
        "projectMilestoneId": milestone_id,
        "stateId": state_id,
        "title": title,
        "description": description,
    }
    if assignee_id:
        payload["assigneeId"] = assignee_id

    data = _post(api_key, _CREATE_MUTATION, {"input": payload})
    result = data.get("issueCreate") or {}
    if not result.get("success"):
        raise TrimLeversError(f"issueCreate failed for {fingerprint}")
    issue = result.get("issue") or {}
    return f"FILED {issue.get('identifier')} {issue.get('url')}"


def append_evidence(
    api_key: str,
    *,
    project_id: str,
    fingerprint: str,
    evidence: str,
    dry_run: bool,
) -> str:
    """Append this session's evidence to the lever that already exists.

    The read-modify-write happens **here**, inside this process: the stored body
    is fetched, grown, and sent back without ever being printed. That is the whole
    point — the accumulator shape is where the MCP echo compounds worst, since
    each append enlarges what the next one echoes.
    """
    matches = probe(api_key, project_id, fingerprint)
    if not matches:
        raise TrimLeversError(
            f"no issue carries fingerprint {fingerprint} — file it first"
        )
    if len(matches) > 1:
        names = ", ".join(str(m.get("identifier")) for m in matches)
        raise TrimLeversError(
            f"fingerprint {fingerprint} is on {len(matches)} issues ({names}) — "
            "refusing to guess which one accumulates the evidence"
        )
    identifier = matches[0].get("identifier")

    if dry_run:
        return (
            f"WOULD APPEND to {identifier} | {len(evidence.strip())} char(s) "
            "of evidence"
        )

    data = _post(api_key, _BODY_QUERY, {"id": identifier})
    issue = data.get("issue") or {}
    stored = issue.get("description") or ""
    # Two newlines, never one. A single newline before appended text can leave a
    # heading or rule abutting the previous paragraph, which Linear's round trip
    # re-parses — the setext-heading corruption the merge tool hit twice.
    grown = stored.rstrip("\n") + "\n\n" + evidence.strip() + "\n"

    data = _post(
        api_key, _UPDATE_MUTATION, {"id": issue["id"], "description": grown}
    )
    result = data.get("issueUpdate") or {}
    if not result.get("success"):
        raise TrimLeversError(f"issueUpdate failed for {identifier}")
    # Deliberately reports only the size, not the text: the grown body is exactly
    # what must not reach a transcript.
    return (
        f"APPENDED {identifier} {issue.get('url')} "
        f"({len(stored)} -> {len(grown)} chars)"
    )


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def _read_file(path: str, label: str) -> str:
    try:
        with open(path, "r", encoding="utf-8") as fh:
            text = fh.read()
    except OSError as e:
        raise TrimLeversError(f"cannot read {label} {path}: {e}") from e
    if not text.strip():
        raise TrimLeversError(f"{label} {path} is empty")
    return text


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="trim_levers.py",
        description="File and fold trim levers as parked Linear issues.",
    )
    parser.add_argument(
        "--dry-run", action="store_true", help="report what would happen only"
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("probe", help="find the issue carrying a fingerprint")
    p.add_argument("--fingerprint", required=True)
    p.add_argument("--dry-run", action="store_true", help=argparse.SUPPRESS)

    f = sub.add_parser("file", help="file a new parked lever")
    f.add_argument("--title", required=True)
    f.add_argument("--fingerprint", required=True)
    f.add_argument("--body-file", required=True)
    f.add_argument("--touches", default=None, help="comma-separated path globs")
    f.add_argument("--dry-run", action="store_true", help=argparse.SUPPRESS)

    a = sub.add_parser("append-evidence", help="grow an existing lever")
    a.add_argument("--fingerprint", required=True)
    a.add_argument("--evidence-file", required=True)
    a.add_argument("--dry-run", action="store_true", help=argparse.SUPPRESS)

    lst = sub.add_parser("list", help="the parked pool, for the fold")
    lst.add_argument("--dry-run", action="store_true", help=argparse.SUPPRESS)

    return parser.parse_args(argv[1:])


def run(argv: list[str]) -> int:
    args = _parse_args(argv)
    api_key = env_var("LINEAR_API_KEY")
    project_id = env_var("LINEAR_PROJECT_ID")

    if args.cmd == "probe":
        fingerprint = validate_fingerprint(args.fingerprint)
        matches = probe(api_key, project_id, fingerprint)
        if not matches:
            print(f"NONE {fingerprint}")
            return 1
        for m in matches:
            state = (m.get("state") or {}).get("name")
            milestone = (m.get("projectMilestone") or {}).get("name") or "-"
            print(
                f"MATCH {m.get('identifier')} [{state}] [{milestone}] "
                f"{m.get('url')} | {m.get('title')}"
            )
        return 0

    if args.cmd == "list":
        levers = parked(api_key, project_id)
        for m in sorted(levers, key=lambda m: str(m.get("identifier"))):
            state = (m.get("state") or {}).get("name")
            print(f"{m.get('identifier')} [{state}] {m.get('url')} | {m.get('title')}")
        print(f"-- {len(levers)} parked lever(s)", file=sys.stderr)
        return 0

    if args.cmd == "file":
        fingerprint = validate_fingerprint(args.fingerprint)
        title = args.title.strip()
        if not title:
            raise TrimLeversError("--title is empty")
        print(
            file_lever(
                api_key,
                project_id=project_id,
                team_id=env_var("LINEAR_TEAM_ID"),
                assignee_id=os.environ.get("LINEAR_ASSIGNEE_ID", "").strip() or None,
                title=title,
                body=_read_file(args.body_file, "--body-file"),
                fingerprint=fingerprint,
                touches=split_touches(args.touches),
                dry_run=args.dry_run,
            )
        )
        return 0

    fingerprint = validate_fingerprint(args.fingerprint)
    print(
        append_evidence(
            api_key,
            project_id=project_id,
            fingerprint=fingerprint,
            evidence=_read_file(args.evidence_file, "--evidence-file"),
            dry_run=args.dry_run,
        )
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
