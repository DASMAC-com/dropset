#!/usr/bin/env python3
"""Verify Grafana's provisioned alert rules — live, or statically from the YAML.

Verifying provisioned dashboards and alert rules had **no committed tool**, so a
session improvised entirely in forbidden shapes: a stopgap `curl` grant plus
four inline interpreter one-liners, none of which can reduce to a reusable
allow-rule. The improvisation also found something real — **nothing in CI parses
the alert YAML at all** — so a rule file can be malformed, or can contradict its
own header, and the first thing to notice is a human.

Two modes:

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


def fetch_live(url: str) -> dict:
    """Grafana's rule health, from a running instance."""
    endpoint = url.rstrip("/") + RULES_PATH
    request = urllib.request.Request(endpoint, headers={"Accept": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=REQUEST_TIMEOUT) as response:
            raw = response.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as exc:
        raise GrafanaCheckError(f"{endpoint} returned HTTP {exc.code}") from exc
    except urllib.error.URLError as exc:
        raise GrafanaCheckError(
            f"cannot reach {endpoint}: {exc.reason} — is the collector stack up?"
        ) from exc
    try:
        return json.loads(raw)
    except json.JSONDecodeError as exc:
        raise GrafanaCheckError(f"decoding {endpoint}: {exc}") from exc


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


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="grafana_check.py",
        description="Verify Grafana's provisioned alert rules.",
    )
    sub = parser.add_subparsers(dest="mode", required=True)

    static = sub.add_parser(
        "static", help="check the provisioned YAML; needs no Grafana"
    )
    static.add_argument("--file", required=True, help="path to the alerting YAML")

    live = sub.add_parser("live", help="ask a running Grafana for rule health")
    live.add_argument("--url", default=DEFAULT_URL, help=f"default {DEFAULT_URL}")

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
