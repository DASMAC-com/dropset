"""Stdlib ``unittest`` tests for ``ci_mirror.py`` and the committed manifest.

The parser's refusals, the stage order ``verify_head.py --full`` reads, and
three pins on ``cfg/ci-mirror.json`` itself: one row per required context,
every named target a real Makefile target, and every workflow a real file.
Run via ``make tools-tests``.
"""

from __future__ import annotations

import json
import re
import unittest
from pathlib import Path

import ci_mirror

ROOT = Path(__file__).resolve().parents[3]

#: The ruleset's required contexts (``docs/conventions/github-mcp.md``
#: mirrors the list; the ruleset is the gate).
REQUIRED = {
    "Lint",
    "Lint (clippy)",
    "Lint (rustdoc)",
    "Tests",
    "Tests (no teardown)",
    "Tests (asm parity)",
    "Tests (Postgres)",
    "Detect changes",
    "SDK",
    "Frontend",
    "Semantic PR",
}


def row(context, local=(), ci=(), workflow="w.yml"):
    return {
        "context": context,
        "workflow": workflow,
        "ci": list(ci),
        "local": list(local),
    }


def doc(*rows):
    return json.dumps({"contexts": list(rows)})


class ParseTest(unittest.TestCase):
    def test_rows_in_file_order(self):
        rows = ci_mirror.parse(doc(row("A", ["lint"]), row("B", ["test"], ["program"])))
        self.assertEqual([r.context for r in rows], ["A", "B"])
        self.assertEqual(rows[1].ci, ("program",))

    def test_refusals(self):
        for text in (
            "not json",
            "[]",
            json.dumps({"contexts": []}),
            json.dumps({"contexts": ["x"]}),
            doc({"context": "A", "workflow": "w", "ci": []}),
            doc({**row("A"), "local": "lint"}),
            doc(row("A", local=[""])),
            doc(row("A"), row("A")),
        ):
            with self.subTest(text=text), self.assertRaises(ci_mirror.ManifestError):
                ci_mirror.parse(text)

    def test_a_missing_file_is_a_manifest_error(self):
        with self.assertRaises(ci_mirror.ManifestError):
            ci_mirror.load(ROOT / "cfg" / "no-such-manifest.json")

    def test_local_targets_are_distinct_in_first_mention_order(self):
        rows = ci_mirror.parse(
            doc(
                row("Lint", ["lint", "tools-tests"]),
                row("Lint (clippy)", ["lint"]),
                row("Tests", ["test"]),
                row("Semantic PR"),
            )
        )
        self.assertEqual(ci_mirror.local_targets(rows), ["lint", "tools-tests", "test"])
        self.assertEqual(
            ci_mirror.contexts_for(rows, "lint"), {"Lint", "Lint (clippy)"}
        )
        self.assertEqual(ci_mirror.contexts_for(rows, "absent"), frozenset())


class CommittedManifestTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.rows = ci_mirror.load()

    def test_one_row_per_required_context(self):
        self.assertEqual({r.context for r in self.rows}, REQUIRED)

    def test_lint_runs_first(self):
        self.assertEqual(ci_mirror.local_targets(self.rows)[0], "lint")

    def test_every_target_is_a_makefile_target(self):
        makefile = (ROOT / "Makefile").read_text()
        defined = set(re.findall(r"^([A-Za-z0-9_.-]+):", makefile, re.MULTILINE))
        named = {t for r in self.rows for t in (*r.ci, *r.local)}
        self.assertEqual(named - defined, set())

    def test_every_workflow_exists(self):
        for r in self.rows:
            self.assertTrue((ROOT / ".github" / "workflows" / r.workflow).is_file(), r)


if __name__ == "__main__":
    unittest.main()
