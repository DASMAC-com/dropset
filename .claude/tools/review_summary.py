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

``facts.json`` (``integrity`` optional; every other key required)::

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
      "tests": {"make test": "pass", "sdk/ts": "pass"},  // pass | fail | unverified
      "title": "pass",                  // pass | fail
      "freshness": "in sync", "skip_list": "skipped (surface gate)",
      "findings": {"found": 3, "fixed": 3, "blocking_open": 0},
      "fix_cycles": 1,
      "warnings": ["path:12 — rationale"],
      "ready": true,
      "integrity": [{"item": 1, "ticked": true, "note": "…"}],
      "findings_paragraph": "…"
    }

``converged`` is true only when no blocking finding is open, no checklist item
is missing, nothing reported ``fail``, CI's hint gate is ``green`` and the PR is
``mergeable``. Exit 0 when converged, 1 when not, 2 on bad input.

Standard library only; tests in ``tests/test_review_summary.py``.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

CHECKLIST = ("addressed", "partial", "missing")
GATE = ("pass", "fail", "unverified")
ARTIFACTS = ("clean", "refreshed", "unverified")
TITLE = ("pass", "fail")


class SummaryError(Exception):
    """Bad input: surfaced to stderr, exit 2."""


def _status(value, allowed, where: str) -> str:
    if value not in allowed:
        raise SummaryError(f"{where}: {value!r} is not one of {', '.join(allowed)}")
    return value


def _load(path: Path | None, what: str) -> dict | None:
    if path is None:
        return None
    try:
        data = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as exc:
        raise SummaryError(f"reading {what} {path}: {exc}") from exc
    if not isinstance(data, dict):
        raise SummaryError(f"{what} {path} is not a JSON object")
    return data


def _ci_line(ci: dict | None) -> tuple[str, bool]:
    if ci is None:
        return "not read — no CI verdict passed; not green", False
    gate = (ci.get("hint") or {}).get("gate")
    conclusion = ci.get("conclusion")
    if conclusion == "none" and gate == "green":
        return "no checks apply (path-filtered), treated as green", True
    counts = ci.get("counts") or {}
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


def render(facts: dict, ci: dict | None, merge: dict | None) -> tuple[str, bool]:
    """The summary markdown and whether the review converged."""
    try:
        checklist = facts["checklist"]
        findings = facts["findings"]
        tests = facts["tests"]
        lint = facts["lint"]
        artifacts = facts["artifacts"]
    except KeyError as exc:
        raise SummaryError(f"facts missing {exc}") from exc
    statuses = [
        _status(i.get("status"), CHECKLIST, f"checklist {i.get('item')!r}")
        for i in checklist
    ]
    lint_s = _status(lint.get("status"), GATE, "lint")
    art_s = _status(artifacts.get("status"), ARTIFACTS, "artifacts")
    test_s = {k: _status(v, GATE, f"tests {k!r}") for k, v in tests.items()}
    title_s = _status(facts.get("title"), TITLE, "title")
    ci_text, ci_green = _ci_line(ci)
    merge_text, mergeable = _merge_line(merge)

    blocking = int(findings.get("blocking_open", 0))
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
        and "missing" not in statuses
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
    parser.add_argument("--facts", type=Path, required=True)
    parser.add_argument("--ci", type=Path, help="wait_for_checks --pr verdict file")
    parser.add_argument(
        "--mergeable", type=Path, help="wait_for_checks --mergeable verdict file"
    )
    args = parser.parse_args(argv)
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
