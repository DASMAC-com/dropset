#!/usr/bin/env python3
"""Render ``review-pr`` step 19's review summary from facts and tool verdicts.

The summary used to be assembled by hand, line by line, from a fourteen-item
list in the skill — so the mechanical lines drifted in wording and order, and a
review that ended with a blocking finding still open had no fixed place that
said so. This renders every mechanical line in a fixed order; the session
supplies only facts it alone holds (the tier that ran, the lint and test
outcomes it observed) and the one findings paragraph.

The CI and merge lines are never session-supplied: they are read from the
``wait_for_checks.py --verdict-out`` files, so the summary cannot claim a
green it did not observe. A status outside its enumeration is an error, never
rendered: an unverifiable state must read as ``unverified``, never as a pass.

Usage::

    python3 .claude/tools/review_summary.py --facts <facts.json> \\
      --ci <ci-verdict.json> --mergeable <mergeable-verdict.json>

``facts.json`` (``tier_proposed`` and ``integrity`` optional; every other key
required, with the type shown — a missing key or a wrong shape is exit 2)::

    {
      "tag": "ENG-1977",                // null when none resolved
      "checklist": [{"item": "…", "status": "addressed"}],  // partial | missing
      "tier": "meta", "tier_proposed": "meta",  // differing = an override
      "lenses_ran": ["correctness", "completeness"],
      "lenses_skipped": [{"lens": "style", "reason": "meta-work"}],
      "cross_check": "ran",             // or "skipped: <reason>"
      "overlap": "none",
      "lint": {"status": "pass", "detail": ""},       // pass | fail | unverified
      "artifacts": {"status": "clean", "detail": ""}, // clean | refreshed | unverified
      "tests": {"make test": "pass", "sdk/ts": "skipped: no TS in diff"},
                                        // pass | fail | unverified | skipped: <reason>
      "title": "pass",                  // pass | fail
      "freshness": "in sync", "skip_list": "skipped (surface gate)",
      "findings": {"found": 3, "fixed": 3, "blocking_open": 0},
      "fix_cycles": 1,
      "warnings": ["path:12 — rationale"],
      "ready": true,
      "integrity": [{"item": 1, "ticked": true, "note": "…"}],
      "findings_paragraph": "…"
    }

``converged`` is true only when no blocking finding is open, every checklist
item is ``addressed`` (a partial requirement is blocking, as step 3 rules),
nothing reported ``fail``, at least one suite was reported, CI's hint gate is
``green`` and the PR is ``mergeable``. An ``unverified`` local lint, artifact or
suite does **not** block it — step 17's gate reports a check that could not run
locally rather than gating on it, and CI is then the authority — but each is
named on the ``Converged`` line, never rendered as a pass. A suite step 11
skips by rule is ``skipped: <reason>`` — rendered with its reason and
non-blocking, and distinct from ``unverified``, which marks a gap; a bare
``skipped`` is an error. ``tests`` is never empty: a prose-only review reports
the skipped suites instead. Exit 0 when converged, 1 when not, 2 on bad input.

``--schema`` prints the ``facts.json`` example above and exits 0, so composing
the facts needs no read of this file.

The lint, artifact, test and title lines are session facts for now: no tool
emits a lint or suite verdict file yet. When one does, it replaces the fact.

``--dedup <findings.json>`` is a separate mode, run on the collected lens
findings **before** the cross-check and the fix round: several lenses raising
the same nit used to arrive as separate findings, each triaged on its own. It
takes a JSON list of ``{"lens", "file", "line", "end_line"?, "severity",
"claim"}`` (``end_line`` defaults to ``line``; severity is blocking | warning
| nit) and prints one row per group of findings on the same file, with
overlapping line ranges and near-identical claims (token-set overlap of at
least ``CLAIM_OVERLAP`` once case, punctuation and filler words are dropped).
Each row names every lens that raised it, spans the union of their ranges and
carries the highest severity; the first claim reported is kept. Exit 0, or 2
on bad input.

Standard library only; tests in ``tests/test_review_summary.py``.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

CHECKLIST = ("addressed", "partial", "missing")
GATE = ("pass", "fail", "unverified")
ARTIFACTS = ("clean", "refreshed", "unverified")
TITLE = ("pass", "fail")
SKIPPED = "skipped: "


class SummaryError(Exception):
    """Bad input: surfaced to stderr, exit 2."""


#: Required facts keys and their types. `tier_proposed` and `integrity` are
#: the optional ones, checked where they are read.
REQUIRED = {
    "tag": (str, type(None)),
    "checklist": list,
    "tier": str,
    "lenses_ran": list,
    "lenses_skipped": list,
    "cross_check": str,
    "overlap": str,
    "lint": dict,
    "artifacts": dict,
    "tests": dict,
    "title": str,
    "freshness": str,
    "skip_list": str,
    "findings": dict,
    "fix_cycles": int,
    "warnings": list,
    "ready": bool,
    "findings_paragraph": str,
}


def _status(value, allowed, where: str) -> str:
    if value not in allowed:
        raise SummaryError(f"{where}: {value!r} is not one of {', '.join(allowed)}")
    return value


def _suite_status(value, where: str) -> str:
    """A gate status, or ``skipped: <reason>`` with a non-empty reason."""
    if isinstance(value, str) and value.startswith(SKIPPED):
        if value[len(SKIPPED) :].strip():
            return value
    if value not in GATE:
        raise SummaryError(
            f"{where}: {value!r} is not one of {', '.join(GATE)}, or {SKIPPED}<reason>"
        )
    return value


def schema() -> str:
    """The ``facts.json`` example block from the module docstring."""
    doc = __doc__.splitlines()
    start = doc.index("    {")
    return "\n".join(line[4:] for line in doc[start : doc.index("    }", start) + 1])


def _count(value, where: str) -> int:
    # `bool` is an `int` subclass; `True` is not a count.
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise SummaryError(f"{where}: {value!r} is not a non-negative integer")
    return value


def _check_shape(facts: dict) -> None:
    """Every required key present with its type, every nested record a dict
    with the fields it is read for — so bad input is exit 2, never a traceback
    (exit 1, which reads as a plain ``Converged: NO``)."""
    for key, kind in REQUIRED.items():
        if key not in facts:
            raise SummaryError(f"facts missing {key!r}")
        if not isinstance(facts[key], kind) or (
            kind is int and isinstance(facts[key], bool)
        ):
            raise SummaryError(f"facts {key!r} has the wrong type")
    for item in facts["checklist"]:
        if not isinstance(item, dict):
            raise SummaryError(f"checklist entry {item!r} is not an object")
    for s in facts["lenses_skipped"]:
        if not (
            isinstance(s, dict)
            and isinstance(s.get("lens"), str)
            and isinstance(s.get("reason"), str)
        ):
            raise SummaryError(f"lenses_skipped entry {s!r} needs lens and reason")
    if not facts["tests"]:
        raise SummaryError(
            f"tests is empty: report at least one suite ({SKIPPED}<reason> for a skip)"
        )
    for key in ("found", "fixed", "blocking_open"):
        _count(facts["findings"].get(key), f"findings.{key}")
    _count(facts["fix_cycles"], "fix_cycles")
    integrity = facts.get("integrity", [])
    if not isinstance(integrity, list) or not all(
        isinstance(i, dict) for i in integrity
    ):
        raise SummaryError("integrity must be a list of objects")


def _load(path: Path | None, what: str) -> dict | None:
    if path is None:
        return None
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError) as exc:
        raise SummaryError(f"reading {what} {path}: {exc}") from exc
    if not isinstance(data, dict):
        raise SummaryError(f"{what} {path} is not a JSON object")
    return data


def _ci_line(ci: dict | None) -> tuple[str, bool]:
    if ci is None:
        return "not read — no CI verdict passed; not green", False
    hint = ci.get("hint")
    gate = hint.get("gate") if isinstance(hint, dict) else None
    conclusion = ci.get("conclusion")
    if conclusion == "none" and gate == "green":
        return "no checks apply (path-filtered), treated as green", True
    counts = ci.get("counts") if isinstance(ci.get("counts"), dict) else {}
    detail = ", ".join(f"{n} {k}" for k, n in sorted(counts.items()))
    return f"{conclusion} ({detail or 'no counts'}); gate {gate}", gate == "green"


def _merge_line(merge: dict | None) -> tuple[str, bool]:
    if merge is None:
        return "not read — no mergeable verdict passed", False
    return (
        f"{merge.get('conclusion')} (mergeable {merge.get('mergeable')}, "
        f"{merge.get('merge_state_status')})",
        merge.get("conclusion") == "mergeable",
    )


#: Severities in rising order; a group carries the highest of its members'.
SEVERITY = ("nit", "warning", "blocking")
#: Minimum token-set overlap (shared over combined words) for two claims to read as the same one.
CLAIM_OVERLAP = 0.5
#: Filler words dropped before claims are compared, so phrasing alone does not
#: keep two lenses' copies of one finding apart.
FILLER = frozenset(
    "a an the this that these those is are was be it its of to in on for and or "
    "but with as at by from should would could here there now".split()
)


def _claim_tokens(claim: str) -> frozenset[str]:
    words = re.findall(r"[a-z0-9_]+", claim.lower())
    return frozenset(w for w in words if w not in FILLER)


def _same_claim(a: frozenset[str], b: frozenset[str]) -> bool:
    if not a or not b:
        return a == b
    return len(a & b) / len(a | b) >= CLAIM_OVERLAP


def _check_finding(f, i: int) -> dict:
    where = f"finding {i}"
    if not isinstance(f, dict):
        raise SummaryError(f"{where} is not an object")
    for key in ("lens", "file", "claim"):
        if not isinstance(f.get(key), str) or not f[key].strip():
            raise SummaryError(f"{where}: {key!r} must be a non-empty string")
    _status(f.get("severity"), SEVERITY, f"{where} severity")
    start = _count(f.get("line"), f"{where} line")
    end = _count(f.get("end_line", start), f"{where} end_line")
    if end < start:
        raise SummaryError(f"{where}: end_line {end} is before line {start}")
    return {**f, "line": start, "end_line": end}


def dedup(findings: list) -> list[dict]:
    """Group duplicate findings: same file, overlapping lines, same claim.

    Grouping is transitive (a union-find over every matching pair), so a chain
    of pairwise matches lands in one group. Groups keep first-report order.
    """
    if not isinstance(findings, list):
        raise SummaryError("findings must be a JSON list")
    rows = [_check_finding(f, i) for i, f in enumerate(findings)]
    tokens = [_claim_tokens(r["claim"]) for r in rows]
    parent = list(range(len(rows)))

    def root(i: int) -> int:
        while parent[i] != i:
            parent[i] = parent[parent[i]]
            i = parent[i]
        return i

    for i, a in enumerate(rows):
        for j in range(i + 1, len(rows)):
            b = rows[j]
            if (
                a["file"] == b["file"]
                and a["line"] <= b["end_line"]
                and b["line"] <= a["end_line"]
                and _same_claim(tokens[i], tokens[j])
            ):
                parent[root(j)] = root(i)

    groups: dict[int, list[dict]] = {}
    for i, r in enumerate(rows):
        groups.setdefault(root(i), []).append(r)
    out = []
    for members in groups.values():
        lenses = list(dict.fromkeys(m["lens"] for m in members))
        out.append(
            {
                "file": members[0]["file"],
                "line": min(m["line"] for m in members),
                "end_line": max(m["end_line"] for m in members),
                "severity": max((m["severity"] for m in members), key=SEVERITY.index),
                "claim": members[0]["claim"],
                "lenses": lenses,
            }
        )
    return out


def render_dedup(groups: list[dict], total: int) -> str:
    """One markdown row per group, then a count line."""
    lines = []
    for g in groups:
        span = (
            f"{g['line']}"
            if g["line"] == g["end_line"]
            else f"{g['line']}-{g['end_line']}"
        )
        lines.append(
            f"- `{g['file']}:{span}` [{g['severity']}] {g['claim']} "
            f"— raised by {', '.join(g['lenses'])}"
        )
    lines.append(f"\n{total} finding(s) → {len(groups)} after dedup")
    return "\n".join(lines) + "\n"


def render(facts: dict, ci: dict | None, merge: dict | None) -> tuple[str, bool]:
    """The summary markdown and whether the review converged."""
    _check_shape(facts)
    checklist = facts["checklist"]
    findings = facts["findings"]
    tests = facts["tests"]
    lint = facts["lint"]
    artifacts = facts["artifacts"]
    statuses = [
        _status(i.get("status"), CHECKLIST, f"checklist {i.get('item')!r}")
        for i in checklist
    ]
    lint_s = _status(lint.get("status"), GATE, "lint")
    art_s = _status(artifacts.get("status"), ARTIFACTS, "artifacts")
    test_s = {k: _suite_status(v, f"tests {k!r}") for k, v in tests.items()}
    title_s = _status(facts.get("title"), TITLE, "title")
    ci_text, ci_green = _ci_line(ci)
    merge_text, mergeable = _merge_line(merge)

    blocking = findings["blocking_open"]
    failed = [
        n
        for n, s in [("lint", lint_s), ("title", title_s), *test_s.items()]
        if s == "fail"
    ]
    unverified = [
        n
        for n, s in [("lint", lint_s), ("artifacts", art_s), *test_s.items()]
        if s == "unverified"
    ]
    converged = (
        blocking == 0
        and all(s == "addressed" for s in statuses)
        and not failed
        and ci_green
        and mergeable
    )

    tag = facts.get("tag")
    tier = facts.get("tier")
    proposed = facts.get("tier_proposed")
    tier_text = tier if proposed in (None, tier) else f"{tier} (override of {proposed})"
    skipped = (
        "; ".join(
            f"{s['lens']} — {s['reason']}" for s in facts.get("lenses_skipped", [])
        )
        or "none"
    )

    lines = ["## Review summary", ""]
    if tag:
        lines.append(f"- **Linear coverage** ({tag}):")
        lines += [f"  - {i.get('item')} — {s}" for i, s in zip(checklist, statuses)]
    else:
        lines.append("- **Linear coverage**: no Linear task checked")
    lines += [
        f"- **Fan-out**: tier {tier_text}; lenses {', '.join(facts.get('lenses_ran', []))}; "
        f"skipped {skipped}; cross-check {facts.get('cross_check')}",
        f"- **In-flight overlap**: {facts.get('overlap')}",
        f"- **Lint**: {lint_s}"
        + (f" — {lint['detail']}" if lint.get("detail") else ""),
        f"- **Generated artifacts**: {art_s}"
        + (f" — {artifacts['detail']}" if artifacts.get("detail") else ""),
        "- **Tests**: " + ", ".join(f"{k} {s}" for k, s in test_s.items()),
        f"- **Title**: {title_s} (`Semantic PR`)",
        f"- **Merge**: {merge_text}",
        f"- **CI**: {ci_text}",
        "- **Linear status**: "
        + (
            "In Progress; In Review at the merge-queue handoff"
            if tag
            else "no tag resolved"
        ),
        f"- **Freshness**: {facts.get('freshness')}",
        f"- **CI skip-list**: {facts.get('skip_list')}",
        f"- **Issues**: {findings.get('found', 0)} found, "
        f"{findings.get('fixed', 0)} fixed, {blocking} blocking open; "
        f"{facts.get('fix_cycles', 0)} fix cycle(s)",
        f"- **Converged**: {'yes' if converged else 'NO'}"
        + (f" — failed: {', '.join(failed)}" if failed else "")
        + (f"; unverified: {', '.join(unverified)}" if unverified else ""),
        f"- **Ready**: {'marked ready' if facts.get('ready') else 'left in draft'}",
    ]
    warnings = facts.get("warnings") or []
    lines.append("- **Warnings and nits**:" + ("" if warnings else " none"))
    lines += [f"  - {w}" for w in warnings]
    integrity = facts.get("integrity")
    if integrity:
        lines.append("- **Integrity checklist**:")
        lines += [
            f"  - [{'x' if i.get('ticked') else ' '}] {i.get('item')}"
            + (f" — {i['note']}" if i.get("note") else "")
            for i in integrity
        ]
    lines += ["", facts.get("findings_paragraph") or "", ""]
    return "\n".join(lines), converged


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="review_summary.py")
    parser.add_argument("--facts", type=Path, help="facts JSON (see --schema)")
    parser.add_argument("--ci", type=Path, help="wait_for_checks --pr verdict file")
    parser.add_argument(
        "--mergeable", type=Path, help="wait_for_checks --mergeable verdict file"
    )
    parser.add_argument(
        "--schema", action="store_true", help="print the facts.json schema and exit"
    )
    parser.add_argument(
        "--dedup",
        type=Path,
        help="group duplicate lens findings from this JSON list and exit",
    )
    args = parser.parse_args(argv)
    if args.schema:
        print(schema())
        return 0
    if args.dedup is not None:
        try:
            raw = json.loads(args.dedup.read_text())
            groups = dedup(raw)
        except (OSError, ValueError) as exc:
            print(f"error: reading findings {args.dedup}: {exc}", file=sys.stderr)
            return 2
        except SummaryError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 2
        sys.stdout.write(render_dedup(groups, len(raw)))
        return 0
    if args.facts is None:
        parser.error("--facts is required (see --schema for its shape)")
    try:
        facts = _load(args.facts, "facts")
        text, converged = render(
            facts,
            _load(args.ci, "CI verdict"),
            _load(args.mergeable, "mergeable verdict"),
        )
    except SummaryError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    sys.stdout.write(text)
    return 0 if converged else 1


if __name__ == "__main__":
    sys.exit(main())
