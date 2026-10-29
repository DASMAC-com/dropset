"""Tests for deps_audit.py — run with `make tools-test T=test_deps_audit`."""

from __future__ import annotations

import json
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

import deps_audit
from deps_audit import (
    band,
    cvss3_score,
    parse_cargo,
    parse_ignores,
    parse_pnpm,
    render,
    run_audit,
    summarize,
)

# A trimmed `cargo audit --json`: one scored vulnerability, one with no CVSS,
# an unsound warning and a yanked crate (which has no advisory).
CARGO = {
    "vulnerabilities": {
        "found": True,
        "count": 2,
        "list": [
            {
                "advisory": {
                    "id": "RUSTSEC-2026-0185",
                    "package": "quinn-proto",
                    "title": "Remote memory exhaustion",
                    "cvss": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H",
                },
                "versions": {"patched": [">=0.11.15"], "unaffected": []},
                "package": {"name": "quinn-proto", "version": "0.11.14"},
            },
            {
                "advisory": {
                    "id": "RUSTSEC-2025-0111",
                    "package": "tokio-tar",
                    "title": "PAX headers",
                    "cvss": None,
                },
                "versions": {"patched": [], "unaffected": []},
                "package": {"name": "tokio-tar", "version": "0.3.1"},
            },
        ],
    },
    "warnings": {
        "unsound": [
            {
                "kind": "unsound",
                "package": {"name": "lru", "version": "0.12.5"},
                "advisory": {"id": "RUSTSEC-2026-0002", "title": "IterMut"},
                "versions": {"patched": [">=0.16.3"]},
            }
        ],
        "yanked": [
            {
                "kind": "yanked",
                "package": {"name": "spin", "version": "0.9.8"},
                "advisory": None,
                "versions": None,
            }
        ],
    },
}

# A trimmed `pnpm audit --json`: two advisories, keyed by npm's numeric id.
PNPM = {
    "actions": [],
    "advisories": {
        "1089867": {
            "findings": [
                {"version": "0.0.1", "paths": ["decks>spectacle>remark-parse>trim"]}
            ],
            "id": 1089867,
            "title": "ReDoS in trim",
            "severity": "high",
            "module_name": "trim",
            "github_advisory_id": "GHSA-w5p7-h5w8-2hfq",
            "patched_versions": ">=0.0.3",
        },
        "1200000": {
            "findings": [
                {"version": "1.9.0", "paths": ["a>shell-quote"]},
                {"version": "1.9.1", "paths": ["b>shell-quote"]},
            ],
            "id": 1200000,
            "title": "quote() command injection",
            "severity": "critical",
            "module_name": "shell-quote",
            "github_advisory_id": "GHSA-pqg4-j6r4-53mv",
            "patched_versions": ">=1.11.0",
        },
    },
    "metadata": {"vulnerabilities": {"high": 1, "critical": 1}},
}

IGNORES = """\
[advisories]
ignore = [
  "RUSTSEC-2025-0111", # no fix upstream; dev-only via testcontainers
  # a comment line is not an entry
  "RUSTSEC-2099-0001", # matches nothing, so it is stale
]
"""


def fake_run(cargo_rc=1, pnpm_rc=1, installed=True):
    calls = []

    def run(cmd, timeout, cwd=None):
        calls.append(cmd)
        if cmd[:3] == ["cargo", "audit", "--version"]:
            return (0, "cargo-audit 0.22.1", "") if installed else (101, "", "no such")
        if cmd[:2] == ["cargo", "install"]:
            return 0, "", ""
        if cmd[:2] == ["cargo", "audit"]:
            return cargo_rc, json.dumps(CARGO), ""
        if cmd[:2] == ["pnpm", "audit"]:
            return pnpm_rc, json.dumps(PNPM), "ERR" if pnpm_rc > 1 else ""
        return 127, "", "unexpected"

    run.calls = calls
    return run


class CvssTests(unittest.TestCase):
    def test_known_vectors_score_to_their_published_base_scores(self):
        cases = {
            "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H": 7.5,
            "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H": 9.8,
            "CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:C/C:L/I:L/A:N": 6.1,
            "CVSS:3.0/AV:L/AC:H/PR:L/UI:N/S:U/C:L/I:N/A:N": 2.5,
            "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:N": 0.0,
        }
        for vector, score in cases.items():
            with self.subTest(vector=vector):
                self.assertEqual(cvss3_score(vector), score)

    def test_a_vector_it_cannot_score_is_unrated(self):
        self.assertIsNone(cvss3_score("CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N"))
        self.assertIsNone(cvss3_score("CVSS:3.1/AV:Q"))
        self.assertIsNone(cvss3_score(""))
        self.assertEqual(band(None), "unrated")

    def test_bands_follow_the_cvss_rating_scale(self):
        self.assertEqual(
            [band(s) for s in (9.0, 7.0, 6.9, 4.0, 0.1, 0.0)],
            ["critical", "high", "moderate", "moderate", "low", "info"],
        )


class ParseTests(unittest.TestCase):
    def test_cargo_vulnerabilities_and_warnings(self):
        found = {f["id"]: f for f in parse_cargo(CARGO)}
        self.assertEqual(found["RUSTSEC-2026-0185"]["severity"], "high")
        self.assertEqual(found["RUSTSEC-2026-0185"]["fixed"], ">=0.11.15")
        self.assertEqual(found["RUSTSEC-2025-0111"]["severity"], "unrated")
        self.assertEqual(found["RUSTSEC-2025-0111"]["fixed"], "no fix")
        self.assertEqual(found["RUSTSEC-2026-0002"]["severity"], "warning")
        self.assertIn("unsound", found["RUSTSEC-2026-0002"]["title"])
        self.assertIn("yanked:spin@0.9.8", found)

    def test_pnpm_keys_by_ghsa_and_folds_findings(self):
        found = {f["id"]: f for f in parse_pnpm(PNPM)}
        self.assertEqual(set(found), {"GHSA-w5p7-h5w8-2hfq", "GHSA-pqg4-j6r4-53mv"})
        quote = found["GHSA-pqg4-j6r4-53mv"]
        self.assertEqual(quote["version"], "1.9.0, 1.9.1")
        self.assertEqual(quote["paths"], ["a>shell-quote", "b>shell-quote"])
        self.assertEqual(quote["severity"], "critical")


class IgnoreTests(unittest.TestCase):
    def test_reasons_parse_and_comment_lines_are_skipped(self):
        reasons, unreasoned = parse_ignores(IGNORES)
        self.assertEqual(set(reasons), {"RUSTSEC-2025-0111", "RUSTSEC-2099-0001"})
        self.assertEqual(unreasoned, [])

    def test_an_ignore_without_a_reason_is_named(self):
        _reasons, unreasoned = parse_ignores(
            'ignore = [\n  "RUSTSEC-1",\n  "RUSTSEC-2", #   \n]\n'
        )
        self.assertEqual(unreasoned, ["RUSTSEC-1", "RUSTSEC-2"])

    def test_an_inline_list_hides_whose_reason_is_whose(self):
        _reasons, unreasoned = parse_ignores('ignore = ["A", "B"] # both\n')
        self.assertEqual(len(unreasoned), 1)

    def test_every_committed_ignore_carries_a_reason(self):
        # The standing-exception rule itself: an ignore without a reason fails.
        root = Path(deps_audit.__file__).resolve().parents[2]
        for rel in (deps_audit.CARGO_IGNORES, deps_audit.PNPM_IGNORES):
            with self.subTest(file=str(rel)):
                self.assertTrue((root / rel).is_file())
                _reasons, unreasoned = deps_audit.load_ignores(root, rel)
                self.assertEqual(unreasoned, [])
        self.assertEqual(deps_audit.check_ignores(root), 0)


class SummaryTests(unittest.TestCase):
    def findings(self):
        return parse_cargo(CARGO) + parse_pnpm(PNPM)

    def test_ranking_counts_and_ignores(self):
        reasons, _ = parse_ignores(IGNORES)
        s = summarize(self.findings(), reasons, None, [], top=2)
        self.assertEqual(s["total"], 5)
        self.assertEqual(s["counts"], {"critical": 1, "high": 2, "warning": 2})
        self.assertEqual(
            [f["id"] for f in s["top"]], ["GHSA-pqg4-j6r4-53mv", "GHSA-w5p7-h5w8-2hfq"]
        )
        self.assertEqual(s["ignored"], ["RUSTSEC-2025-0111"])
        self.assertEqual(s["stale_ignores"], ["RUSTSEC-2099-0001"])
        self.assertIn("no previous run", s["line"])
        self.assertIn("1 ignored", s["line"])
        self.assertIn("stale ignores: RUSTSEC-2099-0001", s["line"])

    def test_delta_against_the_previous_ids(self):
        previous = ["GHSA-w5p7-h5w8-2hfq", "RUSTSEC-2000-0001"]
        s = summarize(self.findings(), {}, previous, [])
        self.assertEqual(s["delta"]["resolved"], ["RUSTSEC-2000-0001"])
        self.assertNotIn("GHSA-w5p7-h5w8-2hfq", s["delta"]["new"])
        self.assertEqual(len(s["delta"]["new"]), 5)
        self.assertIn("5 new / 1 resolved since the last run", s["line"])

    def test_zero_when_only_ignored_remain(self):
        ids = {f["id"]: "known" for f in self.findings()}
        s = summarize(self.findings(), ids, [], [])
        self.assertEqual(s["total"], 0)
        self.assertTrue(s["line"].startswith("0 advisories (none), 0 new / 0 resolved"))

    def test_default_render_is_bounded_and_full_lists_paths(self):
        s = summarize(self.findings(), {}, None, [], top=2)
        short = render(s, full=False)
        self.assertEqual(len(short.splitlines()), 4)
        self.assertIn("4 more (--full lists them)", short)
        full = render(s, full=True)
        self.assertIn("via decks>spectacle>remark-parse>trim", full)
        self.assertNotIn("--full lists them", full)


class RunTests(unittest.TestCase):
    def run_in(self, tmp, **kw):
        state = Path(tmp) / "state.json"
        run = fake_run(**kw)
        s = run_audit(
            Path(tmp),
            run=run,
            state=state,
            now=datetime(2026, 10, 29, tzinfo=timezone.utc),
        )
        return s, state, run

    def test_state_round_trips_into_the_next_delta(self):
        with tempfile.TemporaryDirectory() as tmp:
            first, state, _ = self.run_in(tmp)
            self.assertIsNone(first["delta"])
            self.assertEqual(json.loads(state.read_text())["ids"], first["ids"])
            second, _, _ = self.run_in(tmp)
            self.assertEqual(second["delta"], {"new": [], "resolved": []})

    def test_a_failed_scanner_keeps_the_baseline_and_the_other_findings(self):
        with tempfile.TemporaryDirectory() as tmp:
            s, state, _ = self.run_in(tmp, pnpm_rc=2)
            self.assertEqual(s["errors"][0][:5], "pnpm:")
            self.assertTrue(all(f["source"] == "cargo" for f in s["findings"]))
            self.assertFalse(state.exists())
            self.assertIn("FAILED", s["line"])

    def test_cargo_audit_is_installed_on_demand(self):
        with tempfile.TemporaryDirectory() as tmp:
            _s, _state, run = self.run_in(tmp, installed=False)
            self.assertIn(["cargo", "install", "cargo-audit", "--locked"], run.calls)
            _s, _state, run = self.run_in(tmp)
            self.assertFalse(any(c[:2] == ["cargo", "install"] for c in run.calls))

    def test_ignores_are_read_from_the_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            (Path(tmp) / "cfg").mkdir()
            (Path(tmp) / deps_audit.CARGO_IGNORES).write_text(IGNORES)
            s, _state, _run = self.run_in(tmp)
            self.assertEqual(s["ignored"], ["RUSTSEC-2025-0111"])


if __name__ == "__main__":
    unittest.main()
