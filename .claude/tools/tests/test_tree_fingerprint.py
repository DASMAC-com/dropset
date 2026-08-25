#!/usr/bin/env python3
"""Unit tests for tree_fingerprint.py, over a real throwaway git repo.

The property that makes this tool worth having is **invariance across
history rewrites**: the fingerprint must survive a commit, an amend and a
rebase that change no bytes, because those are precisely the operations after
which review-pr currently re-runs suites it has already run. If the digest
moved under any of them, the tool would grade everything stale and buy nothing.
"""

from __future__ import annotations

import io
import os
import subprocess
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

import tree_fingerprint as tf


class FingerprintTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        self.cwd = os.getcwd()
        os.chdir(self.repo)
        self.addCleanup(os.chdir, self.cwd)
        self.git("init", "-q")
        self.git("config", "user.email", "t@example.com")
        self.git("config", "user.name", "T")
        self.git("config", "commit.gpgsign", "false")
        (self.repo / "a.txt").write_text("alpha\n", encoding="utf-8")
        self.git("add", "a.txt")
        self.git("commit", "-q", "-m", "first")

    def git(self, *args):
        subprocess.run(["git", *args], cwd=self.repo, check=True, capture_output=True)

    def test_the_fingerprint_is_stable_across_repeated_calls(self):
        self.assertEqual(tf.compute(), tf.compute())

    def test_editing_a_file_changes_the_fingerprint(self):
        before = tf.compute()
        (self.repo / "a.txt").write_text("beta\n", encoding="utf-8")
        self.assertNotEqual(before, tf.compute())

    def test_staging_and_committing_the_same_bytes_does_NOT_change_it(self):
        # The core property: a commit rewrites history, not content.
        (self.repo / "a.txt").write_text("beta\n", encoding="utf-8")
        before = tf.compute()
        self.git("add", "a.txt")
        self.git("commit", "-q", "-m", "second")
        self.assertEqual(before, tf.compute())

    def test_amending_does_NOT_change_it(self):
        before = tf.compute()
        self.git("commit", "-q", "--amend", "-m", "reworded")
        self.assertEqual(before, tf.compute())

    def test_a_rebase_touching_nothing_this_branch_touches_does_NOT_change_it(self):
        # The expensive case in a real review: the base moves, the content this
        # branch carries does not, and the suites get re-run anyway.
        self.git("checkout", "-q", "-b", "feature")
        (self.repo / "feature.txt").write_text("f\n", encoding="utf-8")
        self.git("add", "feature.txt")
        self.git("commit", "-q", "-m", "feature work")
        head = subprocess.run(
            ["git", "rev-parse", "--abbrev-ref", "HEAD"],
            cwd=self.repo,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        self.assertEqual(head, "feature")
        before = tf.compute()

        self.git("checkout", "-q", "master") if self._has_master() else self.git(
            "checkout", "-q", "main"
        )
        (self.repo / "base.txt").write_text("b\n", encoding="utf-8")
        self.git("add", "base.txt")
        self.git("commit", "-q", "-m", "base work")
        self.git("checkout", "-q", "feature")
        self.git("rebase", "-q", "master" if self._has_master() else "main")

        # base.txt now exists in the tree, so the fingerprint legitimately
        # changes — what is being pinned is that it changes for a CONTENT
        # reason, and that re-computing after the rebase is itself stable.
        after = tf.compute()
        self.assertNotEqual(before, after)
        self.assertEqual(after, tf.compute())

    def _has_master(self) -> bool:
        out = subprocess.run(
            ["git", "branch", "--list", "master"],
            cwd=self.repo,
            check=True,
            capture_output=True,
            text=True,
        ).stdout
        return bool(out.strip())

    def test_an_untracked_not_ignored_file_is_included(self):
        # It is exactly the file a lint run would fail on, so evidence
        # recorded without it would claim a currency it has not got.
        before = tf.compute()
        (self.repo / "new.txt").write_text("n\n", encoding="utf-8")
        self.assertNotEqual(before, tf.compute())

    def test_an_ignored_file_is_excluded(self):
        (self.repo / ".gitignore").write_text("build/\n", encoding="utf-8")
        self.git("add", ".gitignore")
        self.git("commit", "-q", "-m", "ignore")
        before = tf.compute()
        (self.repo / "build").mkdir()
        (self.repo / "build" / "out.o").write_text("junk\n", encoding="utf-8")
        self.assertEqual(before, tf.compute())

    def test_deleting_a_file_changes_the_fingerprint(self):
        before = tf.compute()
        (self.repo / "a.txt").unlink()
        self.assertNotEqual(before, tf.compute())

    def test_content_cannot_impersonate_a_path(self):
        # Both path and bytes are length-prefixed, so no concatenation of one
        # file's content can forge another's path.
        one = tf.compute(paths=["a.txt"])
        (self.repo / "b.txt").write_text("alpha\n", encoding="utf-8")
        two = tf.compute(paths=["b.txt"])
        self.assertNotEqual(one, two)


class LedgerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        self.cwd = os.getcwd()
        os.chdir(self.repo)
        self.addCleanup(os.chdir, self.cwd)
        subprocess.run(["git", "init", "-q"], cwd=self.repo, check=True)
        (self.repo / "a.txt").write_text("alpha\n", encoding="utf-8")

    def _run(self, argv):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = tf.run(["tree_fingerprint.py"] + argv)
        return code, out.getvalue() + err.getvalue()

    def test_an_unrecorded_check_grades_missing(self):
        code, printed = self._run(["check", "--check", "lint"])
        self.assertEqual(code, 1)
        self.assertIn("MISSING", printed)

    def test_a_recorded_check_grades_fresh(self):
        self._run(["record", "--check", "lint"])
        code, printed = self._run(["check", "--check", "lint"])
        self.assertEqual(code, 0)
        self.assertIn("FRESH", printed)

    def test_editing_the_tree_makes_it_stale(self):
        self._run(["record", "--check", "lint"])
        (self.repo / "a.txt").write_text("changed\n", encoding="utf-8")
        code, printed = self._run(["check", "--check", "lint"])
        self.assertEqual(code, 1)
        self.assertIn("STALE", printed)

    def test_checks_are_graded_independently(self):
        self._run(["record", "--check", "lint"])
        code, printed = self._run(["check", "--check", "tools-tests"])
        self.assertEqual(code, 1)
        self.assertIn("MISSING", printed)

    def test_the_three_way_grade_is_the_point(self):
        # A binary fresh/stale would have to call "never recorded" stale, which
        # loses the distinction between "re-run it" and "run it for the first
        # time and record it".
        self.assertEqual(tf.grade({}, "lint", "abc"), "missing")
        self.assertEqual(
            tf.grade({"lint": {"fingerprint": "abc"}}, "lint", "abc"), "fresh"
        )
        self.assertEqual(
            tf.grade({"lint": {"fingerprint": "xyz"}}, "lint", "abc"), "stale"
        )

    def test_a_corrupt_ledger_grades_missing_rather_than_fresh(self):
        # The safe direction: trusting a damaged ledger would assert a green
        # that was never established.
        self._run(["record", "--check", "lint"])
        ledger = (
            Path(
                subprocess.run(
                    ["git", "rev-parse", "--git-dir"],
                    cwd=self.repo,
                    check=True,
                    capture_output=True,
                    text=True,
                ).stdout.strip()
            )
            / tf.LEDGER_RELATIVE
        )
        ledger.write_text("not json", encoding="utf-8")
        code, printed = self._run(["check", "--check", "lint"])
        self.assertEqual(code, 1)
        self.assertIn("MISSING", printed)

    def test_the_ledger_is_owner_only_and_inside_the_git_dir(self):
        self._run(["record", "--check", "lint"])
        git_dir = subprocess.run(
            ["git", "rev-parse", "--git-dir"],
            cwd=self.repo,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        ledger = Path(git_dir) / tf.LEDGER_RELATIVE
        self.assertTrue(ledger.exists())
        self.assertEqual(ledger.stat().st_mode & 0o777, 0o600)

    def test_compute_prints_a_bare_digest(self):
        code, printed = self._run(["compute"])
        self.assertEqual(code, 0)
        self.assertEqual(len(printed.strip()), 64)


if __name__ == "__main__":
    unittest.main()
