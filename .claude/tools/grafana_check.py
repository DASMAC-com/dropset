#!/usr/bin/env python3
"""Verify Grafana's provisioned alert rules and dashboard panels.

Verifying provisioned dashboards and alert rules had **no committed tool**, so a
session improvised entirely in forbidden shapes: a stopgap `curl` grant plus
four inline interpreter one-liners, none of which can reduce to a reusable
allow-rule. The improvisation also found something real — **nothing in CI parses
the alert YAML at all** — so a rule file can be malformed, or can contradict its
own header, and the first thing to notice is a human.

Three modes:

``live``
    Ask a running Grafana for each rule's ``health``, ``state`` and
    ``lastError``. Exits non-zero if any rule is unhealthy, which is what makes
    it usable as a check rather than a report.

``static``
    Read the provisioned YAML directly. Needs no Grafana, no network and no
    credentials, so it is the natural basis for the missing CI gate. It checks
    that no rule uid repeats, that every rule carries both a uid and a title,
    and that any **prose claim about the rule count** in the file's own comments
    still matches reality — the exact drift that shipped once, when a fix commit
    updated four artifacts and left the alert file's header saying "three rules".

``panel``
    Run a dashboard's panel queries through Grafana's own query endpoint and
    report the rows each returns, under the variable values a browser would
    resolve. That is the DATA half of "does this panel work", and it needs no
    browser: the dashboard and query APIs answer anonymously on the localnet
    stack. Whether the panel RENDERS is the half it cannot see. Exits non-zero
    on a failing query or one under ``--min-rows`` (default 1; pass 0 for a
    panel that is legitimately empty, such as a recent-errors table).

Deliberately stdlib-only, so it runs in CI with no install step. The static mode
is a **structural scan, not a YAML parse**: it keys on the `- title:` / `uid:`
lines that Grafana's own provisioning schema fixes, which is enough for the
checks above and avoids taking a dependency to read four fields. It is honest
about that limit — it is not a schema validator, and it says so rather than
implying the file is fully verified.

Usage::

    python3 .claude/tools/grafana_check.py static \\
        --file market-data/grafana/provisioning/alerting/maker.yml
    python3 .claude/tools/grafana_check.py live --url http://localhost:3200
    python3 .claude/tools/grafana_check.py panel --dashboard market-data \\
        --panel 3 --var product_id=EUR-USD

Tests live in ``tests/test_grafana_check.py``, run via ``make tools-tests``.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import urllib.error
import urllib.request

DEFAULT_URL = "http://localhost:3200"

# The localnet stack runs Grafana with anonymous access, so no credentials are
# needed or accepted here. A deployment that turns auth on wants a token, and
# adding one is a deliberate change rather than something to guess at.
RULES_PATH = "/api/prometheus/grafana/api/v1/rules"

REQUEST_TIMEOUT = 15

# A provisioned rule's identity lines. The optional `- ` accepts the key when it
# is the FIRST key of a list item, which is how a rule is commonly written even
# though the committed file happens to order its keys differently — a scanner
# that only handled one of the two spellings would report zero rules on a
# perfectly valid file, which is the worst possible failure for a gate.
_UID_RE = re.compile(r"^\s*(?:-\s+)?uid:\s*['\"]?([A-Za-z0-9_-]+)['\"]?\s*$")
_TITLE_RE = re.compile(r"^\s*(?:-\s+)?title:\s*(?:'([^']*)'|\"([^\"]*)\"|(\S.*?))\s*$")

# The start of a rule list item, whatever its first key. Two uses: counting how
# many rules the file DECLARES, so a rule missing its uid can be reported rather
# than silently omitted from the parse (a rule is identified by its uid here, so
# without this it would simply not exist as far as the gate is concerned); and
# bounding title attribution, so a title that OPENS an item is never
# back-attached to the previous one.
#
# The first-key list is a bounded heuristic, not the schema. An item whose first
# key is something else (`data:`, `execErrState:`, `isPaused:`, `orgId:`) is
# invisible to the declared-count check — it under-reports, which is the safe
# direction: it can miss a missing-uid rule, never invent one.
_RULE_ITEM_RE = re.compile(
    r"^\s*-\s+(?:uid|title|condition|for|annotations|labels|noDataState):"
)

# A prose claim about how many rules the file defines, in a comment. Both
# spellings occur: "three rules" in a sentence, "5 rules" in a note.
_NUMBER_WORDS = {
    "one": 1,
    "two": 2,
    "three": 3,
    "four": 4,
    "five": 5,
    "six": 6,
    "seven": 7,
    "eight": 8,
    "nine": 9,
    "ten": 10,
    "eleven": 11,
    "twelve": 12,
}
_COUNT_CLAIM_RE = re.compile(
    r"\b(\d+|" + "|".join(_NUMBER_WORDS) + r")\s+(?:alert\s+)?rules\b",
    re.IGNORECASE,
)


class GrafanaCheckError(Exception):
    """A user-facing failure: surfaced to stderr, exits non-zero."""


def parse_rules(text: str) -> list[dict]:
    """``[{"uid": …, "title": …, "line": …}]`` for each provisioned rule.

    A rule is identified by its ``uid``; the nearest ``title`` above it in the
    same block is its name. Both lines are part of Grafana's provisioning schema
    for an alert rule, so this is stable against formatting without needing a
    YAML parser.
    """
    rules: list[dict] = []
    pending_title = None
    # Has a new list item opened since the last uid was recorded? Without this,
    # "attach to the previous rule if it has no title" reaches ACROSS the item
    # boundary: a uid-first rule that genuinely has no title swallows the next
    # rule's title, so rule 1 looks named, rule 2 is reported title-less, and
    # the problem names the wrong uid.
    item_since_uid = False
    for number, line in enumerate(text.splitlines(), start=1):
        # Strip a trailing comment from EVERY line. The inverted form of this —
        # stripping only on lines that are entirely comments — was a no-op where
        # it ran and absent where it mattered: both identity regexes are
        # end-anchored, so `uid: 'x'  # provisioned 8/24` matched neither and the
        # rule was dropped from the parse entirely. A gate going blind is the
        # worst direction for it to fail in.
        stripped = _strip_comment(line)
        if _RULE_ITEM_RE.match(stripped):
            item_since_uid = True
        title_match = _TITLE_RE.match(stripped)
        if title_match:
            title = next((g for g in title_match.groups() if g is not None), "")
            if rules and not item_since_uid and not rules[-1]["title"]:
                # The title FOLLOWS its uid, IN THE SAME ITEM — the `- uid:`-
                # first ordering the uid regex deliberately accepts. Carrying it
                # forward instead reported "no title" here and mis-attached it
                # to the NEXT rule.
                rules[-1]["title"] = title
            else:
                # The title PRECEDES its uid, which is how the committed file
                # is written.
                pending_title = title
            continue
        uid_match = _UID_RE.match(stripped)
        if uid_match:
            rules.append(
                {
                    "uid": uid_match.group(1),
                    "title": pending_title or "",
                    "line": number,
                }
            )
            pending_title = None
            item_since_uid = False
    return rules


def _strip_comment(line: str) -> str:
    """``line`` with any trailing YAML comment removed, quotes respected.

    A ``#`` inside a quoted scalar is part of the value, not a comment — rule
    titles are quoted, so a naive split would truncate one containing a ``#``.
    """
    quote = None
    for index, char in enumerate(line):
        if quote:
            if char == quote:
                quote = None
            continue
        if char in "\"'":
            quote = char
        elif char == "#":
            return line[:index]
    return line


def count_claims(text: str) -> list[dict]:
    """Prose claims about the rule count found in the file's comments.

    Only comment lines are scanned: a count inside a rule expression is data,
    not a claim about the file.
    """
    claims = []
    for number, line in enumerate(text.splitlines(), start=1):
        if not line.lstrip().startswith("#"):
            continue
        for match in _COUNT_CLAIM_RE.finditer(line):
            token = match.group(1).lower()
            value = _NUMBER_WORDS.get(token)
            if value is None:
                try:
                    value = int(token)
                except ValueError:
                    continue
            claims.append({"line": number, "text": match.group(0), "value": value})
    return claims


def check_static(text: str) -> dict:
    """Structural findings for one provisioned alerting file."""
    rules = parse_rules(text)
    problems: list[str] = []

    if not rules:
        problems.append("no alert rules found — is this a provisioning file?")

    # A rule is IDENTIFIED by its uid here, so a rule block with a title and no
    # uid never becomes an entry and would be invisible — which is exactly what
    # Grafana rejects at load. Count the list-item starts independently and
    # compare, so the docstring's "every rule carries both a uid and a title"
    # is actually checked rather than half-checked.
    declared = sum(1 for line in text.splitlines() if _RULE_ITEM_RE.match(line))
    if declared > len(rules):
        problems.append(
            f"{declared} rule item(s) declared but only {len(rules)} carry a uid "
            f"— a rule without one is rejected by Grafana at load"
        )

    seen: dict[str, int] = {}
    for rule in rules:
        if rule["uid"] in seen:
            problems.append(
                f"duplicate uid {rule['uid']!r} at line {rule['line']} "
                f"(first seen at line {seen[rule['uid']]}) — Grafana keys on the "
                f"uid, so one rule silently replaces the other"
            )
        else:
            seen[rule["uid"]] = rule["line"]
        if not rule["title"]:
            problems.append(f"rule {rule['uid']!r} at line {rule['line']} has no title")

    # The drift that actually shipped: a fix commit updated the README, the
    # migration, the panel and the doc, and left this file's own header saying
    # "three rules" while it defined more.
    #
    # A claim is accepted if it matches EITHER the file total or the number of
    # rules defined above it. Both readings are legitimate and both occur in the
    # real file — a header counts the file, while a mid-file note says "the
    # three rules above" and means exactly that. Checking only the total flags
    # every positional reference, which is a false positive that would train the
    # reader to ignore this check.
    for claim in count_claims(text):
        above = sum(1 for rule in rules if rule["line"] < claim["line"])
        if claim["value"] not in (len(rules), above):
            problems.append(
                f"line {claim['line']} claims {claim['text']!r} but the file "
                f"defines {len(rules)} ({above} above that line) — update the "
                f"comment or the rules"
            )

    return {"rules": rules, "problems": problems}


def _fetch_json(endpoint: str, body: dict | None = None) -> dict:
    """GET (or POST ``body`` to) ``endpoint`` and decode the JSON reply.

    A query batch in which ANY query fails comes back as HTTP 400 with every
    query's result still in the body — the failing one carrying its ``error``.
    Treating that status as fatal would hide which query failed and drop the
    row counts of the ones that succeeded, so a 400 that decodes to a
    ``results`` payload is returned rather than raised.
    """
    headers = {"Accept": "application/json"}
    data = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        data = json.dumps(body).encode("utf-8")
    request = urllib.request.Request(endpoint, data=data, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=REQUEST_TIMEOUT) as response:
            raw = response.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as exc:
        if body is not None and exc.code == 400:
            try:
                payload = json.loads(exc.read().decode("utf-8", errors="replace"))
            except (json.JSONDecodeError, OSError):
                payload = None
            if isinstance(payload, dict) and "results" in payload:
                return payload
        raise GrafanaCheckError(f"{endpoint} returned HTTP {exc.code}") from exc
    except urllib.error.URLError as exc:
        raise GrafanaCheckError(
            f"cannot reach {endpoint}: {exc.reason} — is the collector stack up?"
        ) from exc
    try:
        return json.loads(raw)
    except json.JSONDecodeError as exc:
        raise GrafanaCheckError(f"decoding {endpoint}: {exc}") from exc


def fetch_live(url: str) -> dict:
    """Grafana's rule health, from a running instance."""
    return _fetch_json(url.rstrip("/") + RULES_PATH)


def check_live(payload: dict) -> dict:
    """Flatten Grafana's rule payload and name every unhealthy rule."""
    rules = []
    for group in (payload.get("data") or {}).get("groups") or []:
        for rule in group.get("rules") or []:
            rules.append(
                {
                    "uid": rule.get("uid") or rule.get("name") or "?",
                    "title": rule.get("name") or "",
                    "health": (rule.get("health") or "unknown").lower(),
                    "state": (rule.get("state") or "unknown").lower(),
                    "last_error": rule.get("lastError") or "",
                }
            )
    problems = [
        f"{r['uid']} is {r['health']}"
        + (f": {r['last_error']}" if r["last_error"] else "")
        for r in rules
        if r["health"] not in ("ok", "nodata")
    ]
    if not rules:
        problems.append(
            "Grafana reported no alert rules at all — provisioning did not load"
        )
    return {"rules": rules, "problems": problems}


# The two template forms the frontend interpolates before a panel's SQL reaches
# Grafana. `$__timeFilter` and the other `$__` macros are expanded server-side by
# the SQL datasource, so they pass through untouched — which is why the pattern
# demands a brace and a letter after the `$`.
_TOKEN_RE = re.compile(r"\$\{([A-Za-z_]\w*)(?::(\w+))?\}")

# Grafana's sentinel for "All" in a variable's stored selection.
_ALL = "$__all"


def _quote(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def interpolate(text: str, resolved: dict, display: bool = False) -> str:
    """``text`` with every ``${name}`` / ``${name:sqlstring}`` substituted.

    Mirrors the SQL datasources' own rule: ``sqlstring`` always quotes; the bare
    form quotes only for a multi-value or include-all variable and otherwise
    inserts the value with its single quotes doubled. A custom ``allValue`` is a
    literal and is inserted as-is. An unknown name, or a format other than these
    two (``:raw``, ``:csv``, …), is left in place so the caller can report it —
    guessing a format's quoting would make a query pass or fail for the
    tool's reasons rather than the panel's. ``display`` is for a panel title,
    which is not SQL: values are joined plainly, with no quoting at all.
    """

    def replace(match: re.Match) -> str:
        variable = resolved.get(match.group(1))
        if variable is None or match.group(2) not in (None, "sqlstring"):
            return match.group(0)
        if variable["literal"] is not None:
            return variable["literal"]
        values = variable["values"]
        if display:
            return ",".join(values)
        if match.group(2) == "sqlstring" or variable["multi"]:
            return ",".join(_quote(v) for v in values)
        return ",".join(v.replace("'", "''") for v in values)

    return _TOKEN_RE.sub(replace, text)


def _variable_sql(variable: dict) -> str:
    query = variable.get("query")
    if isinstance(query, dict):
        query = query.get("rawSql") or query.get("query")
    return query or variable.get("definition") or ""


def _current_values(variable: dict) -> list[str]:
    value = (variable.get("current") or {}).get("value")
    if value is None:
        return []
    return [str(v) for v in value] if isinstance(value, list) else [str(value)]


def resolve_variables(
    dashboard: dict, overrides: dict[str, list[str]], run_query
) -> tuple[dict, list[dict], list[str]]:
    """Resolve the dashboard's variables in order, the way the frontend does.

    ``run_query(datasource, sql)`` returns one variable query's options. Order
    matters because variables chain — a later query interpolates an earlier
    variable — so each is resolved against everything above it. The stored
    selection is kept where it is still among the options, ``$__all`` expands to
    every option, and a selection that no longer exists falls back to the first
    real option. (Grafana versions differ on whether an include-all variable
    falls back to All instead; ``--var`` pins either reading.) An override
    naming no variable is a problem, not a silent no-op — the run would
    otherwise pass under the stored default rather than the asked-for value.
    Returns the resolution map, a printable row per variable, and any problems.
    """
    resolved: dict = {}
    rows: list[dict] = []
    problems: list[str] = []
    for variable in (dashboard.get("templating") or {}).get("list") or []:
        name = variable.get("name")
        if not name:
            continue
        kind = variable.get("type")
        selection = overrides.get(name) or _current_values(variable)
        options = None
        if kind == "query":
            sql = interpolate(_variable_sql(variable), resolved)
            options, error = run_query(variable.get("datasource"), sql)
            if error:
                problems.append(f"variable {name!r} query failed: {error}")
        elif kind == "custom":
            options = [
                item.split(" : ")[-1].strip()
                for item in (variable.get("query") or "").split(",")
                if item.strip()
            ]
        multi = bool(variable.get("multi") or variable.get("includeAll"))
        literal = None
        if _ALL in selection:
            if variable.get("allValue"):
                literal = variable["allValue"]
            values = list(options or [])
        elif options is not None and name not in overrides:
            values = [v for v in selection if v in options] or options[:1]
        else:
            values = selection
        if not values and literal is None:
            problems.append(f"variable {name!r} resolved to no values")
        resolved[name] = {"values": values, "multi": multi, "literal": literal}
        rows.append(
            {
                "name": name,
                "values": [literal] if literal is not None else values,
                "all": _ALL in selection,
            }
        )
    for name in sorted(set(overrides) - set(resolved)):
        problems.append(f"--var {name!r} names no variable on this dashboard")
    return resolved, rows, problems


def flatten_panels(dashboard: dict) -> list[dict]:
    """Every panel, including those nested inside a collapsed row."""
    panels: list[dict] = []
    for panel in dashboard.get("panels") or []:
        panels.append(panel)
        panels.extend(panel.get("panels") or [])
    return panels


def frame_rows(result: dict) -> int:
    """Rows across one query's frames — the length of a frame's first field."""
    total = 0
    for frame in result.get("frames") or []:
        values = (frame.get("data") or {}).get("values") or []
        if values:
            total += len(values[0])
    return total


def frame_column(result: dict) -> list[str]:
    """A variable query's options: its ``__value`` field, else its first."""
    for frame in result.get("frames") or []:
        fields = (frame.get("schema") or {}).get("fields") or []
        values = (frame.get("data") or {}).get("values") or []
        if not values:
            continue
        names = [f.get("name") for f in fields]
        index = names.index("__value") if "__value" in names else 0
        return ["" if v is None else str(v) for v in values[index]]
    return []


def check_panels(
    dashboard: dict,
    panel_id: int | None,
    overrides: dict[str, list[str]],
    run_batch,
    min_rows: int,
) -> dict:
    """Run each panel's queries with resolved variables and count the rows.

    ``run_batch(queries)`` returns Grafana's ``results`` map keyed by refId.
    Answers the DATA half of "does this panel work" — the rows it returns and
    the variable values it ran under — with no browser. Whether it RENDERS is a
    separate question this cannot answer.
    """

    def run_query(datasource, sql):
        query = {"refId": "A", "datasource": datasource, "rawSql": sql}
        results = run_batch([{**query, "format": "table"}])
        result = results.get("A") or {}
        return frame_column(result), result.get("error")

    resolved, variables, problems = resolve_variables(dashboard, overrides, run_query)

    candidates = [p for p in flatten_panels(dashboard) if p.get("targets")]
    if panel_id is not None:
        chosen = [p for p in candidates if p.get("id") == panel_id]
        if not chosen:
            known = ", ".join(str(p.get("id")) for p in candidates)
            problems.append(f"no queryable panel with id {panel_id} (have: {known})")
        candidates = chosen

    panels = []
    for panel in candidates:
        for label, scope in _instances(panel, resolved):
            panels.extend(
                _run_panel(panel, label, scope, run_batch, min_rows, problems)
            )
    return {"variables": variables, "panels": panels, "problems": problems}


def _instances(panel: dict, resolved: dict) -> list[tuple[str, dict]]:
    """``(label, variables)`` for each copy of ``panel`` a browser would draw.

    A panel that repeats over a variable is drawn once per value with that
    variable pinned to the one value. Running it once with every value spliced
    in instead is not a weaker check but a wrong one — the SQL compares with
    ``=``, so it fails outright. The pinned value keeps the variable's own
    ``multi`` flag, because the frontend formats it with the variable's model:
    a bare ``${name}`` in a repeat over a multi-value variable stays quoted.
    """
    label = str(panel.get("id"))
    name = panel.get("repeat")
    if not name or name not in resolved:
        return [(label, resolved)]
    variable = resolved[name]
    return [
        (
            f"{label}[{value}]",
            {
                **resolved,
                name: {"values": [value], "multi": variable["multi"], "literal": None},
            },
        )
        for value in variable["values"]
    ]


def _run_panel(panel, label, resolved, run_batch, min_rows, problems) -> list[dict]:
    queries = []
    for index, target in enumerate(panel["targets"]):
        if target.get("hide"):
            continue
        sql = interpolate(target.get("rawSql") or "", resolved)
        for name, fmt in sorted(set(_TOKEN_RE.findall(sql))):
            if name in resolved:
                problems.append(
                    f"panel {label} uses unsupported format ${{{name}:{fmt}}}"
                )
            else:
                problems.append(f"panel {label} refers to unknown variable {name!r}")
        queries.append(
            {
                **target,
                # Grafana defaults a missing refId to "A", so two targets
                # without one would collide on the same result key.
                "refId": target.get("refId") or f"Q{index}",
                "datasource": target.get("datasource") or panel.get("datasource"),
                "rawSql": sql,
            }
        )
    if not queries:
        problems.append(f"panel {label} has no visible queries")
        return []
    try:
        results = run_batch(queries)
    except GrafanaCheckError as exc:
        # One panel's unanswerable batch (a 400 with no per-query results)
        # should name the panel rather than abort the whole dashboard run.
        problems.append(f"panel {label} could not be queried: {exc}")
        return []
    rows_out = []
    for query in queries:
        ref = query["refId"]
        if ref not in results:
            problems.append(f"panel {label} query {ref} returned no result at all")
            continue
        result = results[ref] or {}
        rows = frame_rows(result)
        error = result.get("error") or ""
        rows_out.append(
            {
                "id": label,
                "title": interpolate(panel.get("title") or "", resolved, display=True),
                "ref": ref,
                "rows": rows,
                "error": error,
            }
        )
        where = f"panel {label} query {ref}"
        if error:
            problems.append(f"{where} failed: {error}")
        elif rows < min_rows:
            problems.append(f"{where} returned {rows} row(s), wanted ≥ {min_rows}")
    return rows_out


def fetch_panels(
    url: str,
    uid: str,
    panel_id: int | None,
    overrides: dict[str, list[str]],
    time_from: str | None,
    time_to: str | None,
    min_rows: int,
) -> dict:
    """``check_panels`` against a running Grafana's dashboard ``uid``."""
    base = url.rstrip("/")
    dashboard = _fetch_json(f"{base}/api/dashboards/uid/{uid}").get("dashboard") or {}
    window = dashboard.get("time") or {}

    def run_batch(queries):
        body = {
            "from": time_from or window.get("from") or "now-6h",
            "to": time_to or window.get("to") or "now",
            "queries": queries,
        }
        return _fetch_json(f"{base}/api/ds/query", body).get("results") or {}

    return check_panels(dashboard, panel_id, overrides, run_batch, min_rows)


def parse_overrides(pairs: list[str]) -> dict[str, list[str]]:
    """``["a=1", "a=2", "b=x"]`` → ``{"a": ["1", "2"], "b": ["x"]}``."""
    overrides: dict[str, list[str]] = {}
    for pair in pairs:
        name, sep, value = pair.partition("=")
        if not sep or not name:
            raise GrafanaCheckError(f"--var wants name=value, got {pair!r}")
        overrides.setdefault(name, []).append(value)
    return overrides


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="grafana_check.py",
        description="Verify Grafana's provisioned alert rules and dashboard panels.",
    )
    sub = parser.add_subparsers(dest="mode", required=True)

    static = sub.add_parser(
        "static", help="check the provisioned YAML; needs no Grafana"
    )
    static.add_argument("--file", required=True, help="path to the alerting YAML")

    live = sub.add_parser("live", help="ask a running Grafana for rule health")
    live.add_argument("--url", default=DEFAULT_URL, help=f"default {DEFAULT_URL}")

    panel = sub.add_parser(
        "panel", help="run a dashboard's panel queries and count their rows"
    )
    panel.add_argument("--dashboard", required=True, help="dashboard uid")
    panel.add_argument(
        "--panel", type=int, help="panel id; omit to check every queryable panel"
    )
    panel.add_argument(
        "--var",
        action="append",
        default=[],
        metavar="NAME=VALUE",
        help="override a variable; repeat a name for several values",
    )
    panel.add_argument("--from", dest="time_from", help="default: the dashboard's")
    panel.add_argument("--to", dest="time_to", help="default: the dashboard's")
    panel.add_argument(
        "--min-rows", type=int, default=1, help="rows each query must return"
    )
    panel.add_argument("--url", default=DEFAULT_URL, help=f"default {DEFAULT_URL}")

    return parser


def run(argv: list[str]) -> int:
    args = build_parser().parse_args(argv[1:])

    if args.mode == "static":
        try:
            with open(args.file, encoding="utf-8") as handle:
                text = handle.read()
        except OSError as exc:
            raise GrafanaCheckError(f"cannot read {args.file}: {exc}") from exc
        result = check_static(text)
        for rule in result["rules"]:
            print(f"{rule['uid']} | {rule['title']}")
        summary = f"{len(result['rules'])} rule(s)"
    elif args.mode == "panel":
        result = fetch_panels(
            args.url,
            args.dashboard,
            args.panel,
            parse_overrides(args.var),
            args.time_from,
            args.time_to,
            args.min_rows,
        )
        for variable in result["variables"]:
            shown = ",".join(variable["values"]) or "(none)"
            print(f"var | {variable['name']} | {shown}" + (" (all)" * variable["all"]))
        for row in result["panels"]:
            line = f"panel | {row['id']} | {row['ref']} | {row['rows']} row(s)"
            print(f"{line} | {row['title']}")
        summary = f"{len(result['panels'])} query(ies)"
    else:
        result = check_live(fetch_live(args.url))
        for rule in result["rules"]:
            line = (
                f"{rule['uid']} | {rule['health']} | {rule['state']} | {rule['title']}"
            )
            if rule["last_error"]:
                line += f" | {rule['last_error']}"
            print(line)
        summary = f"{len(result['rules'])} rule(s)"

    for problem in result["problems"]:
        print(f"PROBLEM: {problem}", file=sys.stderr)
    print(
        f"grafana-check | {summary}, {len(result['problems'])} problem(s)",
        file=sys.stderr,
    )
    return 1 if result["problems"] else 0


def main() -> int:
    try:
        return run(sys.argv)
    except GrafanaCheckError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
