#!/usr/bin/env python3
"""Unit tests for ``init_pr_branch.py`` (stdlib ``unittest``; no pytest)."""

from __future__ import annotations

import unittest

import init_pr_branch as ipb

PORCELAIN = """\
worktree /Users/alex/repos/dropset
HEAD 8fd8d470f85fe01073a417b25351c840df313c60
branch refs/heads/main

worktree /Users/alex/repos/dropset/.claude/worktrees/eng-603
HEAD 8da1695aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
branch refs/heads/worktree-eng-603
"""

PORCELAIN_NO_MAIN = """\
worktree /Users/alex/repos/dropset/.claude/worktrees/eng-603
HEAD 8da1695aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
branch refs/heads/eng-603
"""


class ParseBaseRepo(unittest.TestCase):
    def test_finds_main_worktree(self):
        self.assertEqual(ipb.parse_base_repo(PORCELAIN), "/Users/alex/repos/dropset")

    def test_none_when_no_main(self):
        self.assertIsNone(ipb.parse_base_repo(PORCELAIN_NO_MAIN))

    def test_detached_head_stanza_is_ignored(self):
        # A detached worktree has no `branch` line; it must not be misread as base.
        porcelain = "worktree /tmp/detached\nHEAD abc123\ndetached\n\n" + PORCELAIN
        self.assertEqual(ipb.parse_base_repo(porcelain), "/Users/alex/repos/dropset")


class NormalizeTag(unittest.TestCase):
    def test_valid_lowercase(self):
        self.assertEqual(ipb.normalize_tag("eng-603"), "eng-603")

    def test_valid_uppercase_normalized(self):
        self.assertEqual(ipb.normalize_tag("ENG-12"), "eng-12")

    def test_invalid(self):
        self.assertIsNone(ipb.normalize_tag("feature-x"))
        self.assertIsNone(ipb.normalize_tag("eng-"))
        self.assertIsNone(ipb.normalize_tag("eng-12a"))
        self.assertIsNone(ipb.normalize_tag(""))


class NormalizeBranch(unittest.TestCase):
    def test_strips_worktree_prefix(self):
        self.assertEqual(ipb.normalize_branch("worktree-eng-603"), ("eng-603", True))

    def test_bare_tag_is_noop(self):
        self.assertEqual(ipb.normalize_branch("eng-603"), ("eng-603", False))

    def test_other_name_is_noop(self):
        self.assertEqual(ipb.normalize_branch("main"), ("main", False))


if __name__ == "__main__":
    unittest.main()
