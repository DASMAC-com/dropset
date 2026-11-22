"""Stdlib ``unittest`` tests for the review-summary renderer."""

from __future__ import annotations

import copy
import io
import json
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

import review_summary as rs

FACTS = {
    "tag": "ENG-1977",
    "checklist": [{"item": "Part 1", "status": "addressed"}],
    "tier": "meta",
    "tier_proposed": "meta",
    "lenses_ran": ["correctness", "completeness"],
    "lenses_skipped": [{"lens": "style", "reason": "meta-work"}],
    "cross_check": "ran",
    "overlap": "none",
    "lint": {"status": "pass", "detail": ""},
    "artifacts": {"status": "clean", "detail": ""},
    "tests": {"make tools-tests": "pass", "make test": "unverified"},
    "title": "pass",
    "freshness": "in sync",
    "skip_list": "skipped (surface gate)",
    "findings": {"found": 2, "fixed": 2, "blocking_open": 0},
    "fix_cycles": 1,
    "warnings": [],
    "ready": True,
    "findings_paragraph": "Two findings, both fixed.",
}
CI = {"conclusion": "pass", "counts": {"pass": 9}, "hint": {"gate": "green"}}
MERGE = {
    "conclusion": "mergeable",
    "mergeable": "MERGEABLE",
    "merge_state_status": "CLEAN",
}


class RenderTests(unittest.TestCase):
    def _render(self, ci=CI, merge=MERGE, **overrides):
        facts = copy.deepcopy(FACTS)
        facts.update(overrides)
        return rs.render(facts, ci, merge)

    def test_a_clean_review_converges_and_says_what_ran(self):
        text, converged = self._render()
        self.assertTrue(converged)
        self.assertIn("**Converged**: yes; unverified: make test", text)
        self.assertIn("tier meta; lenses correctness, completeness", text)
        self.assertIn("1 fix cycle(s)", text)
        self.assertTrue(text.rstrip().endswith("Two findings, both fixed."))

    def test_an_open_blocking_finding_says_so_in_the_fixed_place(self):
        text, converged = self._render(
            findings={"found": 2, "fixed": 1, "blocking_open": 1}
        )
        self.assertFalse(converged)
        self.assertIn("1 blocking open", text)
        self.assertIn("**Converged**: NO", text)

    def test_a_partial_checklist_item_does_not_converge(self):
        """Step 3 rules a partial requirement blocking."""
        _, converged = self._render(checklist=[{"item": "Part 5", "status": "partial"}])
        self.assertFalse(converged)

    def test_a_failed_gate_is_named(self):
        text, converged = self._render(lint={"status": "fail", "detail": "cspell"})
        self.assertFalse(converged)
        self.assertIn("failed: lint", text)

    def test_bad_shapes_are_errors_not_tracebacks(self):
        """A traceback exits 1, which reads as a plain `Converged: NO`."""
        findings_no_blocking = {"found": 1, "fixed": 1}
        for bad in (
            {"findings": findings_no_blocking},
            {"findings": {"found": 1, "fixed": 1, "blocking_open": "0"}},
            {"findings": {"found": 1, "fixed": 1, "blocking_open": True}},
            {"checklist": ["Part 1"]},
            {"lenses_skipped": [{"lens": "style"}]},
            {"lint": "pass"},
            {"tests": {}},
            {"integrity": ["x"]},
        ):
            with self.assertRaises(rs.SummaryError, msg=bad):
                self._render(**bad)
        facts = copy.deepcopy(FACTS)
        del facts["tier"]
        with self.assertRaises(rs.SummaryError):
            rs.render(facts, CI, MERGE)

    def test_a_malformed_ci_verdict_is_not_green(self):
        _, converged = self._render(ci={"conclusion": "pass", "hint": "green"})
        self.assertFalse(converged)

    def test_ci_and_merge_come_from_the_verdicts_not_the_facts(self):
        """Without the verdict files, nothing may claim green."""
        text, converged = self._render(ci=None)
        self.assertFalse(converged)
        self.assertIn("no CI verdict passed; not green", text)
        _, converged = self._render(merge={"conclusion": "conflicting"})
        self.assertFalse(converged)

    def test_an_unverified_ci_gate_does_not_converge(self):
        ci = {"conclusion": "timeout", "hint": {"gate": "unverified"}}
        self.assertFalse(self._render(ci=ci)[1])

    def test_none_with_a_green_gate_reads_as_no_checks(self):
        ci = {"conclusion": "none", "hint": {"gate": "green"}}
        text, converged = self._render(ci=ci)
        self.assertTrue(converged)
        self.assertIn("no checks apply", text)

    def test_a_tier_override_is_named(self):
        text, _ = self._render(tier="full", tier_proposed="meta")
        self.assertIn("tier full (override of meta)", text)

    def test_an_unknown_status_is_an_error_not_a_line(self):
        with self.assertRaises(rs.SummaryError):
            self._render(lint={"status": "ok"})
        with self.assertRaises(rs.SummaryError):
            self._render(tests={"make test": "skipped"})
        with self.assertRaises(rs.SummaryError):
            self._render(tests={"make test": "skipped:  "})

    def test_a_suite_skipped_by_rule_renders_its_reason_and_does_not_block(self):
        """A by-rule skip is not a gap, so it is not named as unverified."""
        text, converged = self._render(
            tests={"make test": "skipped: no Rust reachable", "sdk/ts": "pass"}
        )
        self.assertTrue(converged)
        self.assertIn("make test skipped: no Rust reachable, sdk/ts pass", text)
        self.assertIn("**Converged**: yes\n", text)

    def test_every_suite_may_be_skipped_by_rule(self):
        _, converged = self._render(tests={"make test": "skipped: prose-only diff"})
        self.assertTrue(converged)

    def test_a_missing_checklist_item_blocks_convergence(self):
        _, converged = self._render(checklist=[{"item": "Part 2", "status": "missing"}])
        self.assertFalse(converged)

    def test_the_integrity_checklist_renders_ticks(self):
        text, _ = self._render(
            integrity=[{"item": "7 no unverified pass", "ticked": True}]
        )
        self.assertIn("  - [x] 7 no unverified pass", text)


class CliTests(unittest.TestCase):
    def test_exit_codes(self):
        with tempfile.TemporaryDirectory() as tmp:
            facts, ci, merge = (Path(tmp) / n for n in ("f.json", "c.json", "m.json"))
            facts.write_text(json.dumps(FACTS))
            ci.write_text(json.dumps(CI))
            merge.write_text(json.dumps(MERGE))
            argv = ["--facts", str(facts), "--ci", str(ci), "--mergeable", str(merge)]
            with redirect_stdout(io.StringIO()):
                self.assertEqual(rs.main(argv), 0)
                self.assertEqual(rs.main(["--facts", str(facts)]), 1)
            facts.write_text("[]")
            with redirect_stderr(io.StringIO()):
                self.assertEqual(rs.main(["--facts", str(facts)]), 2)

    def test_schema_names_every_facts_key(self):
        out = io.StringIO()
        with redirect_stdout(out):
            self.assertEqual(rs.main(["--schema"]), 0)
        printed = out.getvalue()
        self.assertTrue(printed.startswith("{"))
        for key in [*rs.REQUIRED, "tier_proposed", "integrity"]:
            self.assertIn(f'"{key}"', printed, key)

    def test_facts_is_required_without_schema(self):
        with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as exc:
            rs.main([])
        self.assertEqual(exc.exception.code, 2)


#: Three lenses raising one nit in different words and overlapping ranges,
#: plus a distinct claim on the same lines and the same claim in another file.
FINDINGS = [
    {
        "lens": "correctness",
        "file": "tools/x.py",
        "line": 40,
        "severity": "nit",
        "claim": "The exception reason should name the missing key.",
    },
    {
        "lens": "naming",
        "file": "tools/x.py",
        "line": 39,
        "end_line": 41,
        "severity": "warning",
        "claim": "exception reason does not name the missing key",
    },
    {
        "lens": "comments",
        "file": "tools/x.py",
        "line": 41,
        "severity": "nit",
        "claim": "Name the missing key in the exception reason",
    },
    {
        "lens": "correctness",
        "file": "tools/x.py",
        "line": 40,
        "severity": "blocking",
        "claim": "Off-by-one: the loop skips the final record",
    },
    {
        "lens": "comments",
        "file": "tools/y.py",
        "line": 40,
        "severity": "nit",
        "claim": "The exception reason should name the missing key.",
    },
]


class DedupTests(unittest.TestCase):
    def test_three_lens_duplicate_collapses_to_one_row(self):
        groups = rs.dedup(FINDINGS)
        self.assertEqual(len(groups), 3)
        dup = groups[0]
        self.assertEqual(dup["lenses"], ["correctness", "naming", "comments"])
        self.assertEqual((dup["line"], dup["end_line"]), (39, 41))
        self.assertEqual(dup["severity"], "warning")
        self.assertEqual(dup["claim"], FINDINGS[0]["claim"])

    def test_distinct_claim_and_other_file_stay_apart(self):
        groups = rs.dedup(FINDINGS)
        self.assertEqual(groups[1]["lenses"], ["correctness"])
        self.assertEqual(groups[1]["severity"], "blocking")
        self.assertEqual(groups[2]["file"], "tools/y.py")

    def test_disjoint_ranges_stay_apart(self):
        far = {**FINDINGS[0], "line": 90, "lens": "naming"}
        self.assertEqual(len(rs.dedup([FINDINGS[0], far])), 2)

    def test_bad_finding_is_an_error(self):
        for bad in (
            {**FINDINGS[0], "severity": "major"},
            {**FINDINGS[0], "end_line": 1},
            {k: v for k, v in FINDINGS[0].items() if k != "lens"},
        ):
            with self.assertRaises(rs.SummaryError):
                rs.dedup([bad])

    def test_cli_prints_one_row_per_group(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "findings.json"
            path.write_text(json.dumps(FINDINGS))
            out = io.StringIO()
            with redirect_stdout(out):
                self.assertEqual(rs.main(["--dedup", str(path)]), 0)
            text = out.getvalue()
            self.assertIn(
                "- `tools/x.py:39-41` [warning] The exception reason should name "
                "the missing key. — raised by correctness, naming, comments",
                text,
            )
            self.assertIn("5 finding(s) → 3 after dedup", text)
            path.write_text("{}")
            with redirect_stderr(io.StringIO()):
                self.assertEqual(rs.main(["--dedup", str(path)]), 2)


if __name__ == "__main__":
    unittest.main()
