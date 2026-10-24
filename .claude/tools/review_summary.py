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
    args = parser.parse_args(argv)
    if args.schema:
        print(schema())
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
