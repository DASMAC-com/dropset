"""Stdlib ``unittest`` tests for the rebase-overlap reporter.

The load-bearing assertions are the two that drive a gate skip: that the
predicates are computed over the **base delta** rather than the branch's own
files, and that ``branch_files`` is measured from the merge base (so a rebase
doesn't fold the base's movement into the branch's set and report overlap
everywhere). Run via the repo's ``make tools-tests``.
"""

from __future__ import annotations

import io
import os
import subprocess
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest import mock

import rebase_overlap as ro
from review_diff import ReviewDiffError


class AnalyzeTests(unittest.TestCase):
    """``analyze`` is pure path arithmetic over three git reads, so the reads are
    stubbed and the arithmetic asserted."""

    def _analyze(self, base_files, branch_files, commits=("abc123 some commit",)):
        with (
            mock.patch.object(ro, "merge_base", return_value="mb"),
            mock.patch.object(ro, "commit_subjects", return_value=list(commits)),
            mock.patch.object(
                ro,
                "changed_files",
                side_effect=lambda rng: (
                    sorted(base_files)
                    if rng.startswith("old..")
                    else sorted(branch_files)
                ),
            ),
        ):
            return ro.analyze("old", "new", "HEAD")

    def test_a_ts_only_base_delta_skips_both_gates(self):
        """The measured case: main moved, the delta was TS-only, and both re-runs
        of the full suite were redundant."""
        result = self._analyze(
            base_files=["frontend/app/page.tsx", "frontend/lib/x.ts"],
            branch_files=["programs/dropset/src/swap.rs"],
        )
        self.assertFalse(result["runs_artifact_gates"])
        self.assertFalse(result["runs_rust_suites"])
        self.assertEqual(result["overlap"], [])

    def test_a_program_change_in_the_base_arms_the_artifact_gate(self):
        result = self._analyze(
            base_files=["programs/dropset/src/lib.rs"],
            branch_files=["frontend/app/page.tsx"],
        )
        self.assertTrue(result["runs_artifact_gates"])
        self.assertTrue(result["runs_rust_suites"])

    def test_the_predicates_read_the_base_delta_not_the_branch(self):
        """A branch full of program changes must not arm a gate when the *base*
        moved only under the CI code filter — that inversion is the whole bug."""
        result = self._analyze(
            base_files=["README.md"],
            branch_files=["programs/dropset/src/swap.rs", "sdk/idl/dropset.json"],
        )
        self.assertFalse(result["runs_artifact_gates"])

    def test_overlap_is_the_intersection(self):
        result = self._analyze(
            base_files=["a.rs", "shared.rs"],
            branch_files=["shared.rs", "b.rs"],
        )
        self.assertEqual(result["overlap"], ["shared.rs"])

    def test_branch_files_are_measured_from_the_merge_base(self):
        """Not from ``previous_base`` — after a rebase that range would include
        the base's own commits and report overlap on every file it moved."""
        seen = []

        with (
            mock.patch.object(ro, "merge_base", return_value="MB") as mb,
            mock.patch.object(ro, "commit_subjects", return_value=[]),
            mock.patch.object(
                ro, "changed_files", side_effect=lambda rng: seen.append(rng) or []
            ),
        ):
            ro.analyze("old", "new", "eng-814")

        mb.assert_called_once_with("eng-814", "new")
        self.assertIn("old..new", seen)
        self.assertIn("MB..eng-814", seen)

    def test_rename_detection_is_disabled_on_the_diff(self):
        """With renames detected, git prints only the destination path — so a
        base that renamed a file the branch still edits reports NO overlap, on
        the one delta shape most likely to have dropped the branch's edits."""
        seen = []
        with mock.patch.object(ro, "_git", side_effect=lambda a: seen.append(a) or ""):
            ro.changed_files("a..b")
        self.assertIn("--no-renames", seen[0])

    def test_commit_count_comes_from_the_log(self):
        result = self._analyze(
            base_files=["a.md"],
            branch_files=[],
            commits=("a1 one", "b2 two", "c3 three"),
        )
        self.assertEqual(result["base_commits"], 3)


class SummaryTests(unittest.TestCase):
    """The summary is the line a reader acts on, so the skip must be said out
    loud rather than inferred from two empty lists."""

    def _summary(self, **over):
        result = {
            "base_commits": 15,
            "base_files": ["frontend/a.ts"],
            "overlap": [],
            "runs_artifact_gates": False,
            "runs_rust_suites": False,
        }
        result.update(over)
        return ro.summarize(result)

    def test_a_skippable_delta_says_so_explicitly(self):
        text = self._summary()
        self.assertIn("assert the gates once", text)
        self.assertIn("no overlap", text)

    def test_an_armed_gate_is_named(self):
        text = self._summary(runs_artifact_gates=True)
        self.assertIn("artifact gates", text)
        self.assertNotIn("assert the gates once", text)

    def test_overlap_is_counted(self):
        text = self._summary(overlap=["shared.rs", "other.rs"])
        self.assertIn("2 overlap", text)


class CliTests(unittest.TestCase):
    def setUp(self):
        # The CLI validates `--from` against real git before analyzing. These
        # cases use placeholder revisions, so the check is stubbed out; it has
        # its own real-git coverage in AncestorGuardTests below.
        patch = mock.patch.object(ro, "check_from_is_ancestor")
        self.addCleanup(patch.stop)
        patch.start()

    def test_emits_json_on_stdout_and_a_summary_on_stderr(self):
        payload = {
            "previous_base": "old",
            "new_base": "new",
            "base_commits": 1,
            "base_commit_subjects": [],
            "base_files": [],
            "branch_files": [],
            "overlap": [],
            "runs_artifact_gates": False,
            "runs_rust_suites": False,
        }
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(ro, "analyze", return_value=payload):
            with redirect_stdout(out), redirect_stderr(err):
                code = ro.run(["rebase_overlap.py", "--from", "old"])
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue().strip().splitlines()[0], "{")
        self.assertIn("rebase-overlap |", err.getvalue())

    def test_the_cli_defaults_reach_analyze(self):
        """`--from` semantics are this tool's whole hazard, so pin that the two
        values the caller does NOT pass are the ones it expects."""
        with mock.patch.object(ro, "analyze") as analyze:
            analyze.return_value = {"conclusion": "x"}
            with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                with mock.patch.object(ro, "summarize", return_value="s"):
                    ro.run(["rebase_overlap.py", "--from", "old"])
        analyze.assert_called_once_with("old", "origin/main", "HEAD")

    def test_from_is_required(self):
        """The pre-fetch base tip cannot be recovered afterwards, so guessing it
        would silently compare the wrong range."""
        with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            ro.run(["rebase_overlap.py"])


class AncestorGuardTests(unittest.TestCase):
    """The guard against a non-ancestor ``--from``.

    Built on a throwaway repo with a real branch, because the property under
    test is a genuine ancestry relation — stubbing `merge_base` would only
    assert that the comparison compares.
    """

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = os.path.realpath(self._tmp.name)
        self._cwd = os.getcwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, self._cwd)
        self._git("init", "-q", "-b", "main")
        self._git("config", "user.email", "t@example.com")
        self._git("config", "user.name", "Test")
        self.fork_point = self._commit("base.md", "base\n")
        # main advances past the fork point.
        self._git("checkout", "-q", "-b", "feature")
        self.branch_tip = self._commit("only-on-branch.md", "branch\n")
        self._git("checkout", "-q", "main")
        self.main_tip = self._commit("landed.md", "theirs\n")

    def _git(self, *args: str) -> None:
        subprocess.run(["git", *args], cwd=self.root, check=True, capture_output=True)

    def _commit(self, rel: str, body: str) -> str:
        with open(os.path.join(self.root, rel), "w", encoding="utf-8") as fh:
            fh.write(body)
        self._git("add", rel)
        self._git("commit", "-q", "-m", f"add {rel}", "--no-gpg-sign")
        return subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=self.root,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()

    def test_the_merge_base_is_accepted(self):
        # The documented correct usage: `git merge-base HEAD <base>`.
        self.assertIsNone(ro.check_from_is_ancestor(self.fork_point, "main"))

    def test_a_ref_equal_to_the_target_is_accepted(self):
        # A base that has not moved at all: merge-base(a, a) == a, so the
        # identity comparison must treat a commit as its own ancestor.
        self.assertIsNone(ro.check_from_is_ancestor("main", "main"))

    def test_the_branch_tip_is_refused(self):
        """The slip this guard exists for: passing the branch tip yields a
        symmetric tree diff, so the branch's own files land in `base_files`."""
        with self.assertRaises(ReviewDiffError) as caught:
            ro.check_from_is_ancestor(self.branch_tip, "main")
        message = str(caught.exception)
        self.assertIn("not an ancestor", message)
        # The error must name the merge-base the caller should have passed —
        # a refusal that doesn't say what to do instead just gets retried.
        self.assertIn(self.fork_point, message)

    def test_a_bad_revision_surfaces_as_an_error_not_as_a_false_no(self):
        # `merge-base --is-ancestor` answers through its exit status, which
        # `_git` discards; the identity comparison keeps a bad revision an
        # error rather than folding it into "not an ancestor".
        with self.assertRaises(ReviewDiffError) as caught:
            ro.check_from_is_ancestor("no-such-rev", "main")
        self.assertNotIn("not an ancestor", str(caught.exception))

    def test_is_ancestor_agrees_with_git_in_both_directions(self):
        self.assertTrue(ro.is_ancestor(self.fork_point, self.main_tip))
        self.assertFalse(ro.is_ancestor(self.main_tip, self.fork_point))


if __name__ == "__main__":
    unittest.main()
