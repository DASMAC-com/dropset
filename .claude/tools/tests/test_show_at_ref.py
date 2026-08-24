#!/usr/bin/env python3
"""Unit tests for show_at_ref.py, over a real throwaway git repo.

The tool exists because every *conforming* way to slice a file at another ref
fails: the Grep tool reads the working tree, `git grep` is guard-blocked, a pipe
is a forbidden compound, and a temp checkout costs more than the read. The
property under test is that a slice really is a slice — the whole blob must
never be what gets printed.
"""

from __future__ import annotations

import io
import os
import subprocess
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

import show_at_ref as sar

OLD = """# Title

## Alpha

old alpha body
const TIMEOUT: u64 = 5;

## Beta

old beta body
"""

NEW = """# Title

## Alpha

new alpha body
const TIMEOUT: u64 = 30;

## Beta

new beta body
"""


class ShowAtRefTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.repo = Path(cls.tmp.name)
        cls.cwd = os.getcwd()

        def git(*args):
            subprocess.run(
                ["git", *args],
                cwd=cls.repo,
                check=True,
                capture_output=True,
            )

        git("init", "-q")
        git("config", "user.email", "t@example.com")
        git("config", "user.name", "T")
        git("config", "commit.gpgsign", "false")
        (cls.repo / "doc.md").write_text(OLD, encoding="utf-8")
        git("add", "doc.md")
        git("commit", "-q", "-m", "first")
        cls.first = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=cls.repo,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        (cls.repo / "doc.md").write_text(NEW, encoding="utf-8")
        git("add", "doc.md")
        git("commit", "-q", "-m", "second")
        os.chdir(cls.repo)

    @classmethod
    def tearDownClass(cls):
        os.chdir(cls.cwd)
        cls.tmp.cleanup()

    def _run(self, argv):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = sar.run(["show_at_ref.py"] + argv)
        return code, out.getvalue(), err.getvalue()

    def test_it_reads_the_old_revision_not_the_working_tree(self):
        # The whole point: the working tree says 30.
        code, out, _ = self._run([self.first, "doc.md", "--grep", "TIMEOUT"])
        self.assertEqual(code, 0)
        self.assertIn("= 5;", out)
        self.assertNotIn("= 30;", out)

    def test_a_grep_prints_only_matching_lines(self):
        _, out, _ = self._run([self.first, "doc.md", "--grep", "TIMEOUT"])
        self.assertEqual(len(out.strip().splitlines()), 1)
        self.assertNotIn("old beta body", out)

    def test_a_section_prints_one_heading_block(self):
        _, out, _ = self._run([self.first, "doc.md", "--section", "Alpha"])
        self.assertIn("old alpha body", out)
        self.assertNotIn("old beta body", out)

    def test_headings_map_the_file_without_its_content(self):
        _, out, _ = self._run([self.first, "doc.md", "--headings"])
        self.assertIn("Alpha", out)
        self.assertIn("Beta", out)
        self.assertNotIn("old alpha body", out)

    def test_a_slice_prints_the_requested_range_only(self):
        _, out, _ = self._run([self.first, "doc.md", "--slice", "3:5"])
        lines = out.strip().splitlines()
        self.assertEqual(len(lines), 3)
        self.assertTrue(lines[0].startswith("3:"))

    def test_count_reports_size_without_the_content(self):
        _, out, _ = self._run([self.first, "doc.md", "--count"])
        self.assertIn("line(s)", out)
        self.assertNotIn("old alpha body", out)

    def test_no_mode_is_refused_rather_than_printing_everything(self):
        # A print-it-all default would rebuild the whole-file `git show` this
        # tool replaces, which is the thing that cost ~4.7k for one constant.
        with self.assertRaises(sar.ShowAtRefError) as caught:
            self._run([self.first, "doc.md"])
        self.assertIn("pick a mode", str(caught.exception))

    def test_out_spills_the_blob_and_prints_only_a_size(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "old.md")
            code, out, err = self._run([self.first, "doc.md", "--out", path])
            self.assertEqual(code, 0)
            self.assertEqual(Path(path).read_text(encoding="utf-8"), OLD)
        self.assertNotIn("old alpha body", out)
        self.assertIn("chars to", err)

    def test_a_spilled_file_is_owner_only(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "old.md")
            self._run([self.first, "doc.md", "--out", path])
            self.assertEqual(os.stat(path).st_mode & 0o777, 0o600)

    def test_a_missing_path_at_that_ref_is_a_clear_error(self):
        with self.assertRaises(sar.ShowAtRefError) as caught:
            self._run([self.first, "absent.md", "--count"])
        self.assertIn("absent.md", str(caught.exception))

    def test_a_ref_that_looks_like_an_option_is_refused(self):
        # No shell is involved, so this is option injection rather than command
        # injection — refused regardless. Tested at `read_blob`, which is the
        # seam that reaches git: on the CLI path argparse rejects a leading dash
        # first (covered below), so the guard is what protects a library caller.
        with self.assertRaises(sar.ShowAtRefError) as caught:
            sar.read_blob("--upload-pack=evil", "doc.md")
        self.assertIn("invalid ref", str(caught.exception))

    def test_a_path_that_looks_like_an_option_is_refused(self):
        with self.assertRaises(sar.ShowAtRefError) as caught:
            sar.read_blob("HEAD", "--output=evil")
        self.assertIn("invalid path", str(caught.exception))

    def test_the_cli_itself_rejects_a_leading_dash_before_git_is_reached(self):
        with self.assertRaises(SystemExit):
            self._run(["--upload-pack=evil", "doc.md", "--count"])

    def test_grep_context_is_honored(self):
        _, out, _ = self._run(
            [self.first, "doc.md", "--grep", "TIMEOUT", "--context", "1"]
        )
        self.assertIn("old alpha body", out)

    def test_a_grep_with_no_match_exits_non_zero(self):
        code, _, _ = self._run([self.first, "doc.md", "--grep", "nothing-here"])
        self.assertEqual(code, 1)


if __name__ == "__main__":
    unittest.main()
