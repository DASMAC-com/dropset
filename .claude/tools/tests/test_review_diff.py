#!/usr/bin/env python3
# cspell:word gpgsign
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
        self.assertTrue(
            rd.matches_any("programs/dropset/src/lib.rs", rd.GENERATION_INPUTS)
        )
        self.assertTrue(
            rd.matches_any("sdk/math-core/src/price.rs", rd.GENERATION_INPUTS)
        )
        self.assertFalse(rd.matches_any("tui/src/ui.rs", rd.GENERATION_INPUTS))
        # sdk/rs is a *consumer* of the generators, not an input to them.
        self.assertFalse(rd.matches_any("sdk/rs/src/events.rs", rd.GENERATION_INPUTS))


def z(*records: str) -> str:
    """Join NUL-terminated ``--numstat -z`` fields into one payload.

    Every fixture goes through this rather than embedding ``\\0`` inline. A
    literal ``"…\\0"`` adjacent to a string starting with a digit is a trap: the
    formatter may merge the two, and ``\\0`` + ``1`` reads back as the *octal*
    escape ``\\01``, silently changing the fixture. ``\\x00`` here is
    unambiguous under any joining.
    """
    return "".join(f + "\x00" for f in records)


class ParseNumstatZTests(unittest.TestCase):
    """Fixtures here are **verbatim `git diff --numstat -z` output**, captured
    from a real repo rather than assumed. The earlier version of this suite
    asserted a rename shape git does not emit without `-z`, so it passed while
    the parser mishandled real renames."""

    def test_parses_and_sums_counts(self):
        text = z("3\t4\ttui/src/ui.rs", "10\t0\tdocs/a.md")
        self.assertEqual(
            rd.parse_numstat_z(text),
            [
                {"path": "docs/a.md", "changes": 10},
                {"path": "tui/src/ui.rs", "changes": 7},
            ],
        )

    def test_binary_file_lands_with_zero_changes(self):
        # git reports "-" for both counts on a binary file; it still changed.
        self.assertEqual(
            rd.parse_numstat_z(z("-\t-\tassets/logo.png")),
            [{"path": "assets/logo.png", "changes": 0}],
        )

    def test_rename_takes_the_destination_path(self):
        # Real -z rename layout: an EMPTY path field, then source, then dest.
        text = z("0\t0\t", "infra/aws/x.yaml", "cfg/x.yaml")
        self.assertEqual(
            rd.parse_numstat_z(text), [{"path": "cfg/x.yaml", "changes": 0}]
        )

    def test_rename_destination_is_gate_matchable(self):
        """The regression the old fixture hid: a cross-tree rename must yield a
        real path, not git's pretty `{old => new}` form, or it matches no prefix
        and the artifact gate silently fails OPEN."""
        text = z("0\t0\t", "sdk/math-core/gen.rs", "sdk/interface/gen.rs")
        files = rd.parse_numstat_z(text)
        self.assertEqual([f["path"] for f in files], ["sdk/interface/gen.rs"])
        self.assertTrue(rd.matches_any(files[0]["path"], rd.GENERATION_INPUTS))

    def test_rename_mixed_with_plain_entries(self):
        # The exact interleaving a real repo produced: rename, plain, binary,
        # non-ASCII (unquoted under -z), lockfile.
        text = z(
            "0\t0\t",
            "infra/aws/x.yaml",
            "cfg/x.yaml",
            "1\t0\tkeep.rs",
            "-\t-\tlogo.png",
            "1\t0\tnon-ascii dir/f.md",
            "4\t0\tpnpm-lock.yaml",
        )
        self.assertEqual(
            [f["path"] for f in rd.parse_numstat_z(text)],
            [
                "cfg/x.yaml",
                "keep.rs",
                "logo.png",
                "non-ascii dir/f.md",
                "pnpm-lock.yaml",
            ],
        )

    def test_non_ascii_path_is_not_quoted_under_z(self):
        # Plain --numstat emits this path quoted with octal escapes; -z passes
        # the bytes through verbatim, which is one reason -z is mandatory.
        got = rd.parse_numstat_z(z("1\t0\tcafé/f.md"))
        self.assertEqual(got, [{"path": "café/f.md", "changes": 1}])

    def test_ignores_short_records(self):
        self.assertEqual(rd.parse_numstat_z(""), [])
        self.assertEqual(rd.parse_numstat_z(z("not-a-row")), [])

    def test_truncated_rename_trailer_is_dropped_not_crashed(self):
        # A rename record whose path pair is cut off must not raise.
        self.assertEqual(rd.parse_numstat_z(z("0\t0\t", "only-source.rs")), [])


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

    def test_nothing_changed_is_not_ready(self):
        v = rd.gate("main", self.out, fetch=False)
        self.assertFalse(v["ready"])
        self.assertTrue(v["diff_empty"])
        self.assertEqual(v["diff_lines"], 0)
        self.assertEqual(v["files"], [])
        self.assertTrue(any("no files changed" in b for b in v["blockers"]))

    def test_only_excluded_families_blocks_with_its_own_reason(self):
        """A lockfile-only PR is not "nothing to review, check the base" — it is
        "no source to review, but the regeneration gates still apply". The two
        used to collapse into one misleading blocker."""
        self.commit("pnpm-lock.yaml", "lockfileVersion: 9\n" * 20, "Bump deps")
        v = rd.gate("main", self.out, fetch=False)
        self.assertFalse(v["ready"])
        self.assertTrue(v["diff_empty"])
        self.assertEqual([f["path"] for f in v["files"]], ["pnpm-lock.yaml"])
        joined = " ".join(v["blockers"])
        self.assertIn("excluded generated family", joined)
        self.assertNotIn("no files changed", joined)
        # pnpm-lock.yaml is inside the CI code filter's exclusions.
        self.assertFalse(v["runs_rust_suites"])

    def test_ready_is_exactly_not_blockers(self):
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        v = rd.gate("main", self.out, fetch=False)
        self.assertEqual(v["ready"], not v["blockers"])

    def test_failed_fetch_blocks_rather_than_claiming_freshness(self):
        """The tool must not report a base it could not verify as fresh. This
        repo has no `origin` remote, so a real fetch attempt fails."""
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        v = rd.gate("main", self.out, fetch=True)
        self.assertFalse(v["fetched"])
        self.assertIsNotNone(v["fetch_error"])
        self.assertFalse(v["ready"])
        self.assertTrue(any("could not fetch" in b for b in v["blockers"]))
        # The local ref still shows no divergence — which is precisely why
        # base_fresh alone must not be the gate.
        self.assertTrue(v["base_fresh"])

    def test_no_fetch_is_the_deliberate_override(self):
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        v = rd.gate("main", self.out, fetch=False)
        self.assertFalse(v["fetched"])
        self.assertIsNone(v["fetch_error"])
        self.assertTrue(v["ready"])

    def test_diff_file_is_owner_only(self):
        self.commit("tui/src/ui.rs", "fn ui() {}\n", "TUI")
        rd.gate("main", self.out, fetch=False)
        self.assertEqual(self.out.stat().st_mode & 0o777, 0o600)

    def test_rejects_a_base_that_looks_like_an_option(self):
        with self.assertRaises(rd.ReviewDiffError):
            rd.gate("--upload-pack=evil", self.out, fetch=False)

    def test_cross_tree_rename_of_a_generation_input_still_trips_the_gate(self):
        """End-to-end guard for the parser fix: git reports this rename as
        `sdk/{math-core => interface}/gen.rs` without -z, which matches no
        GENERATION_INPUTS prefix and would skip the conformance gate."""
        self.commit("sdk/math-core/gen.rs", "fn gen() {}\n" * 30, "Add generator")
        (self.repo / "sdk" / "interface").mkdir(parents=True, exist_ok=True)
        git(self.repo, "mv", "sdk/math-core/gen.rs", "sdk/interface/gen.rs")
        git(self.repo, "commit", "-q", "-m", "Move generator across trees")
        v = rd.gate("main", self.out, fetch=False)
        paths = [f["path"] for f in v["files"]]
        self.assertNotIn("sdk/{math-core => interface}/gen.rs", paths)
        self.assertTrue(v["runs_artifact_gates"])

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
                [
                    "review_diff.py",
                    "--base",
                    "main",
                    "--out",
                    str(self.out),
                    "--no-fetch",
                ]
            )
        parsed = json.loads(buf.getvalue())
        self.assertEqual(code, 0)
        self.assertTrue(parsed["ready"])
        self.assertEqual(parsed["diff_path"], str(self.out))

    def test_cli_exits_non_zero_when_not_ready(self):
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = rd.run(
                [
                    "review_diff.py",
                    "--base",
                    "main",
                    "--out",
                    str(self.out),
                    "--no-fetch",
                ]
            )
        self.assertEqual(code, 1)
        self.assertFalse(json.loads(buf.getvalue())["ready"])

    def test_missing_base_ref_errors(self):
        with self.assertRaises(rd.ReviewDiffError):
            rd.gate("no-such-base", self.out, fetch=False)


if __name__ == "__main__":
    unittest.main()
