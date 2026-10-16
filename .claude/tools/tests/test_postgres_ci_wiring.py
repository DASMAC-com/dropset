"""Stdlib ``unittest`` tests for the Postgres CI wiring guard.

The load-bearing assertions are the bug class itself — a crate that gains a
Docker-gated test without joining the job's selection fails — and the
zero-match failure, without which a drifted attribute spelling would make the
guard approve everything. Run via the repo's ``make tools-tests``.
"""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import postgres_ci_wiring as pcw

GATE = '#[ignore = "requires a Docker daemon (Postgres container)"]\n'

MAKEFILE = (
    ".PHONY: test-postgres\n"
    "test-postgres: check-nextest\n"
    "\tcargo nextest run --run-ignored all \\\n"
    "\t\t-p dropset-feeds -p dropset-indexer \\\n"
    "\t\t--features dropset-feeds/store\n"
    "\n"
    "other:\n"
    "\tcargo test -p not-selected\n"
)


class WiringTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.write("Makefile", MAKEFILE)
        self.write(".github/workflows/test.yml", "run: 'make test-postgres'\n")
        self.crate("feeds", "dropset-feeds")
        self.write("feeds/tests/store.rs", GATE + "fn t() {}\n")

    def tearDown(self):
        self._tmp.cleanup()

    def write(self, rel, text):
        target = self.root / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8")

    def crate(self, path, name):
        # A `[[bin]]` name ahead of `[package]`, as several manifests here have.
        self.write(
            f"{path}/Cargo.toml",
            f'[[bin]]\nname = "{name}-bin"\n\n[package]\nname = "{name}"\n',
        )

    def check(self, *extra):
        return pcw.check(self.root, sources=["feeds/tests/store.rs", *extra])

    def test_a_selected_gated_crate_is_wired(self):
        self.assertEqual(self.check(), [])

    def test_a_new_gated_crate_missing_from_the_selection_fails(self):
        self.crate("ledger", "dropset-ledger")
        self.write("ledger/tests/db.rs", GATE + "fn t() {}\n")
        problems = self.check("ledger/tests/db.rs")
        self.assertEqual(len(problems), 1)
        self.assertIn("dropset-ledger", problems[0])
        self.assertIn("-p dropset-ledger", problems[0])

    def test_a_flag_in_another_target_does_not_count(self):
        self.crate("x", "not-selected")
        self.write("x/src/lib.rs", GATE)
        problems = self.check("x/src/lib.rs")
        self.assertEqual(len(problems), 1)
        self.assertIn("not-selected", problems[0])

    def test_a_gated_file_with_no_resolvable_package_fails(self):
        # Under the virtual root manifest, so no crate `[package]` exists. The
        # guard cannot vouch for it, so it says so rather than skipping it.
        self.write("Cargo.toml", '[workspace]\nmembers = ["feeds"]\n')
        self.write("stray/db.rs", GATE)
        problems = self.check("stray/db.rs")
        self.assertEqual(len(problems), 1)
        self.assertIn("stray/db.rs", problems[0])

    def test_a_workflow_mentioning_the_target_only_in_a_comment_fails(self):
        self.write(
            ".github/workflows/test.yml",
            "# mirrors `make test-postgres`\nrun: 'cargo nextest run'\n",
        )
        self.assertTrue(any("no longer runs" in p for p in self.check()))

    def test_a_list_item_or_double_quoted_run_counts(self):
        for line in ('- run: "make test-postgres"\n', "  run: make test-postgres\n"):
            self.write(".github/workflows/test.yml", line)
            self.assertEqual(self.check(), [], line)

    def test_a_longer_target_name_does_not_count(self):
        self.write(".github/workflows/test.yml", "run: 'make test-postgres-lite'\n")
        self.assertTrue(any("no longer runs" in p for p in self.check()))

    def test_an_ungated_crate_is_not_required(self):
        self.crate("tui", "dropset-tui")
        self.write("tui/src/e2e.rs", '#[ignore = "needs solana-test-validator"]\n')
        self.assertEqual(self.check("tui/src/e2e.rs"), [])

    def test_zero_gated_crates_fails_rather_than_passing(self):
        self.write("feeds/tests/store.rs", "#[ignore]\nfn t() {}\n")
        problems = self.check()
        self.assertTrue(any("drifted" in p for p in problems))

    def test_a_workflow_no_longer_running_the_target_fails(self):
        self.write(".github/workflows/test.yml", "run: 'cargo nextest run'\n")
        self.assertTrue(any("no longer runs" in p for p in self.check()))

    def test_a_missing_target_fails(self):
        self.write("Makefile", "test:\n\tcargo test\n")
        self.assertTrue(any("no `test-postgres` target" in p for p in self.check()))


class ParsingTests(unittest.TestCase):
    def test_package_name_ignores_other_tables(self):
        text = (
            '[[bin]]\nname = "bin-name"\n[package]\nname = "real"\n[lib]\nname = "l"\n'
        )
        self.assertEqual(pcw.package_name(text), "real")

    def test_package_name_accepts_a_literal_string_and_a_header_comment(self):
        text = "[package] # the crate\n  name = 'lit'\n"
        self.assertEqual(pcw.package_name(text), "lit")

    def test_package_name_absent(self):
        self.assertIsNone(pcw.package_name('[workspace]\nmembers = ["a"]\n'))

    def test_recipe_joins_continuations_and_stops_at_the_next_rule(self):
        body = pcw.recipe(MAKEFILE, "test-postgres")
        self.assertEqual(
            pcw.selected_packages(body), ["dropset-feeds", "dropset-indexer"]
        )

    def test_a_feature_flag_is_not_read_as_a_package(self):
        self.assertEqual(
            pcw.selected_packages("cargo t -p a --features a/x -E 'not p'"), ["a"]
        )


if __name__ == "__main__":
    unittest.main()
