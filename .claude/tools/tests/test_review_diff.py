#!/usr/bin/env python3
"""Stdlib ``unittest`` tests for ``review_diff.py``.

The pure logic — pattern matching, ``--numstat`` parsing, and the gate/verdict
assembly over a real throwaway git repo — is what these cover. Run via the
repo's ``make tools-tests``.
"""

from __future__ import annotations

import io
import json
import subprocess
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

import review_diff as rd


class MatchesTests(unittest.TestCase):
    """The three pattern shapes the path lists use."""

    def test_subtree_pattern(self):
        self.assertTrue(rd.matches("frontend/app/page.tsx", "frontend/**"))
        self.assertTrue(rd.matches("frontend", "frontend/**"))
        # A sibling sharing a string prefix must not match.
        self.assertFalse(rd.matches("frontend-old/app.tsx", "frontend/**"))
        self.assertFalse(rd.matches("programs/dropset/src/lib.rs", "frontend/**"))

    def test_suffix_pattern_matches_at_any_depth(self):
        self.assertTrue(rd.matches("README.md", "**/*.md"))
        self.assertTrue(rd.matches("docs/conventions/shell-commands.md", "**/*.md"))
        self.assertFalse(rd.matches("docs/notes.txt", "**/*.md"))

    def test_literal_pattern(self):
        self.assertTrue(rd.matches("pnpm-lock.yaml", "pnpm-lock.yaml"))
        self.assertFalse(rd.matches("sub/pnpm-lock.yaml", "pnpm-lock.yaml"))

    def test_matches_any(self):
        self.assertTrue(rd.matches_any("decks/app/x.tsx", rd.CODE_FILTER_EXCLUDES))
        self.assertFalse(
            rd.matches_any("programs/dropset/src/swap.rs", rd.CODE_FILTER_EXCLUDES)
        )

    def test_generation_inputs(self):
        self.assertTrue(rd.matches_any("programs/dropset/src/lib.rs", rd.GENERATION_INPUTS))
        self.assertTrue(rd.matches_any("sdk/math-core/src/price.rs", rd.GENERATION_INPUTS))
        self.assertFalse(rd.matches_any("tui/src/ui.rs", rd.GENERATION_INPUTS))
        # sdk/rs is a *consumer* of the generators, not an input to them.
        self.assertFalse(rd.matches_any("sdk/rs/src/events.rs", rd.GENERATION_INPUTS))


class ParseNumstatTests(unittest.TestCase):
    def test_parses_and_sums_counts(self):
        text = "3\t4\ttui/src/ui.rs\n10\t0\tdocs/a.md\n"
        self.assertEqual(
            rd.parse_numstat(text),
            [{"path": "docs/a.md", "changes": 10}, {"path": "tui/src/ui.rs", "changes": 7}],
        )

    def test_binary_file_lands_with_zero_changes(self):
        # git reports "-" for both counts on a binary file; it still changed.
        self.assertEqual(
            rd.parse_numstat("-\t-\tassets/logo.png\n"),
            [{"path": "assets/logo.png", "changes": 0}],
        )

    def test_rename_takes_the_destination_path(self):
        text = "1\t1\told/name.rs\tnew/name.rs\n"
        self.assertEqual(
            rd.parse_numstat(text), [{"path": "new/name.rs", "changes": 2}]
        )

    def test_ignores_blank_and_short_lines(self):
        self.assertEqual(rd.parse_numstat("\n\nnot-a-row\n"), [])


def git(repo: Path, *args):
    subprocess.run(
        ["git", *args],
        cwd=str(repo),
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


class GateTests(unittest.TestCase):
    """The verdict, over a real throwaway repo with an ``origin/main`` ref."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.out = self.root / "review-diff.txt"

        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "user.email", "t@example.com")
        git(self.repo, "config", "user.name", "T")
        git(self.repo, "config", "commit.gpgsign", "false")
        (self.repo / "seed.txt").write_text("seed\n", encoding="utf-8")
        git(self.repo, "add", "seed.txt")
        git(self.repo, "commit", "-q", "-m", "Seed")
        # A local ref standing in for the remote-tracking branch, so the tests
        # need no network and no second repository.
        git(self.repo, "update-ref", "refs/remotes/origin/main", "HEAD")

        self._cwd = Path.cwd()
        import os

        os.chdir(self.repo)

    def tearDown(self):
        import os

        os.chdir(self._cwd)
        self._tmp.cleanup()

    def commit(self, rel, text, message="Change"):
        target = self.repo / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8")
        git(self.repo, "add", rel)
        git(self.repo, "commit", "-q", "-m", message)

    def test_clean_branch_is_ready(self):
        self.commit("programs/dropset/src/swap.rs", "fn swap() {}\n", "Add swap")
        v = rd.gate("main", self.out, fetch=False)
        self.assertTrue(v["ready"])
        self.assertTrue(v["base_fresh"])
        self.assertEqual(v["base_ahead"], [])
        self.assertEqual(v["blockers"], [])
        self.assertEqual(len(v["commits"]), 1)
        self.assertIn("Add swap", v["commits"][0])
        self.assertGreater(v["diff_lines"], 0)
        self.assertFalse(v["diff_empty"])
        self.assertEqual(
            [f["path"] for f in v["files"]], ["programs/dropset/src/swap.rs"]
        )
        self.assertTrue(self.out.exists())

    def test_empty_diff_is_not_ready(self):
        v = rd.gate("main", self.out, fetch=False)
        self.assertFalse(v["ready"])
        self.assertTrue(v["diff_empty"])
        self.assertEqual(v["diff_lines"], 0)
        self.assertTrue(any("is empty" in b for b in v["blockers"]))

    def test_stale_base_is_not_ready(self):
        # Advance the branch, then advance origin/main *past* it — the phantom
        # deletion condition a line count cannot detect.
        self.commit("a.rs", "fn a() {}\n", "Branch work")
        git(self.repo, "checkout", "-q", "-b", "side", "HEAD~1")
        self.commit("landed.rs", "fn landed() {}\n", "Landed on main")
        git(self.repo, "update-ref", "refs/remotes/origin/main", "HEAD")
        git(self.repo, "checkout", "-q", "main")

        v = rd.gate("main", self.out, fetch=False)
        self.assertFalse(v["ready"])
        self.assertFalse(v["base_fresh"])
        self.assertEqual(len(v["base_ahead"]), 1)
        self.assertIn("Landed on main", v["base_ahead"][0])
        self.assertTrue(any("commit(s) HEAD lacks" in b for b in v["blockers"]))
        # The diff is non-empty, so line count alone would have said "fine".
        self.assertGreater(v["diff_lines"], 0)

    def test_generated_families_are_excluded_from_the_diff(self):
        self.commit("sdk/rs/src/generated/big.rs", "// generated\n" * 50, "Regenerate")
        v = rd.gate("main", self.out, fetch=False)
        self.assertEqual(v["diff_lines"], 0)
        # --numstat is unfiltered, so the file is still reported as changed.
        self.assertEqual(
            [f["path"] for f in v["files"]], ["sdk/rs/src/generated/big.rs"]
        )

    def test_docs_only_diff_skips_both_gate_families(self):
        self.commit("docs/conventions/x.md", "# x\n", "Doc tweak")
        v = rd.gate("main", self.out, fetch=False)
        self.assertTrue(v["ready"])
        self.assertFalse(v["runs_rust_suites"])
        self.assertFalse(v["runs_artifact_gates"])

    def test_program_diff_runs_both_gate_families(self):
        self.commit("programs/dropset/src/swap.rs", "fn swap() {}\n", "Program")
        v = rd.gate("main", self.out, fetch=False)
        self.assertTrue(v["runs_rust_suites"])
        self.assertTrue(v["runs_artifact_gates"])

    def test_tui_diff_runs_suites_but_not_artifact_gates(self):
        # tui/ is outside the CI code filter, but generates no artifact.
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        v = rd.gate("main", self.out, fetch=False)
        self.assertTrue(v["runs_rust_suites"])
        self.assertFalse(v["runs_artifact_gates"])

    def test_one_foreign_path_is_enough_to_run_the_suites(self):
        self.commit("docs/a.md", "# a\n", "Doc")
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        v = rd.gate("main", self.out, fetch=False)
        self.assertTrue(v["runs_rust_suites"])

    def test_cli_prints_json_and_signals_readiness_by_exit_code(self):
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = rd.run(
                ["review_diff.py", "--base", "main", "--out", str(self.out), "--no-fetch"]
            )
        parsed = json.loads(buf.getvalue())
        self.assertEqual(code, 0)
        self.assertTrue(parsed["ready"])
        self.assertEqual(parsed["diff_path"], str(self.out))

    def test_cli_exits_non_zero_when_not_ready(self):
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = rd.run(
                ["review_diff.py", "--base", "main", "--out", str(self.out), "--no-fetch"]
            )
        self.assertEqual(code, 1)
        self.assertFalse(json.loads(buf.getvalue())["ready"])

    def test_missing_base_ref_errors(self):
        with self.assertRaises(rd.ReviewDiffError):
            rd.gate("no-such-base", self.out, fetch=False)


if __name__ == "__main__":
    unittest.main()
