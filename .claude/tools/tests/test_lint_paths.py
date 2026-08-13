#!/usr/bin/env python3
"""Unit tests for ``lint_paths.py`` (stdlib ``unittest``; no pytest).

The end-to-end property under test — that an **untracked** file reaches the
hooks, which is the whole reason the tool exists — is covered by
``UntrackedFilesAreIncluded``, which builds a throwaway git repo in a temp
directory and reads the resolved list back.
"""

from __future__ import annotations

import os
import subprocess
import tempfile
import unittest

import lint_paths as lp


def _git(*args: str, cwd: str) -> None:
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


class ParseLsFiles(unittest.TestCase):
    def test_splits_on_nul(self):
        self.assertEqual(
            lp.parse_ls_files("a.rs\0b/c.md\0"),
            ["a.rs", "b/c.md"],
        )

    def test_drops_empty_trailing_field(self):
        # `-z` terminates every record, so the split always yields a final "".
        self.assertEqual(lp.parse_ls_files("only.txt\0"), ["only.txt"])

    def test_empty_input(self):
        self.assertEqual(lp.parse_ls_files(""), [])

    def test_dedupes_and_sorts(self):
        self.assertEqual(
            lp.parse_ls_files("b.txt\0a.txt\0b.txt\0"),
            ["a.txt", "b.txt"],
        )

    def test_preserves_paths_with_spaces(self):
        # The reason for `-z`: a space must not split a path.
        self.assertEqual(
            lp.parse_ls_files("dir name/file name.md\0"),
            ["dir name/file name.md"],
        )


class Existing(unittest.TestCase):
    def test_keeps_present_drops_absent(self):
        with tempfile.TemporaryDirectory() as root:
            with open(os.path.join(root, "here.txt"), "w", encoding="utf-8") as fh:
                fh.write("x")
            self.assertEqual(
                lp.existing(["here.txt", "deleted.txt"], root),
                ["here.txt"],
            )

    def test_keeps_dangling_symlink(self):
        # `lexists`, not `exists`: a tracked symlink is in CI's list too.
        with tempfile.TemporaryDirectory() as root:
            os.symlink("nowhere", os.path.join(root, "link"))
            self.assertEqual(lp.existing(["link"], root), ["link"])

    def test_empty_list(self):
        with tempfile.TemporaryDirectory() as root:
            self.assertEqual(lp.existing([], root), [])


class UntrackedFilesAreIncluded(unittest.TestCase):
    """The regression this tool exists for: `--all-files` would miss `new.md`."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = os.path.realpath(self._tmp.name)
        _git("init", "-q", cwd=self.root)
        _git("config", "user.email", "t@example.com", cwd=self.root)
        _git("config", "user.name", "Test", cwd=self.root)
        self._write("tracked.md", "tracked\n")
        self._write(".gitignore", "ignored.md\n")
        _git("add", "tracked.md", ".gitignore", cwd=self.root)
        _git("commit", "-q", "-m", "init", "--no-gpg-sign", cwd=self.root)

    def tearDown(self):
        self._tmp.cleanup()

    def _write(self, rel: str, body: str) -> None:
        with open(os.path.join(self.root, rel), "w", encoding="utf-8") as fh:
            fh.write(body)

    def test_untracked_included_ignored_excluded(self):
        self._write("new.md", "never git added\n")
        self._write("ignored.md", "gitignored\n")
        self.assertEqual(
            lp.lint_files(self.root),
            [".gitignore", "new.md", "tracked.md"],
        )

    def test_deleted_tracked_file_filtered_out(self):
        os.remove(os.path.join(self.root, "tracked.md"))
        self.assertEqual(lp.lint_files(self.root), [".gitignore"])

    def test_clean_tree_matches_tracked_set(self):
        # With nothing untracked, the resolved set is exactly what `--all-files`
        # would have produced — the tool is a superset, never a different set.
        self.assertEqual(lp.lint_files(self.root), [".gitignore", "tracked.md"])


if __name__ == "__main__":
    unittest.main()
