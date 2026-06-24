#!/usr/bin/env python3
"""Unit tests for ``init_pr_branch.py`` (stdlib ``unittest``; no pytest)."""

from __future__ import annotations

import io
import json
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

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


class MainCli(unittest.TestCase):
    """Drive ``main()`` through its ``--porcelain-file`` / ``--branch``
    overrides so no real git is invoked.
    """

    def _run(self, tag: str, branch: str, porcelain: str):
        with tempfile.TemporaryDirectory() as tmp:
            pfile = Path(tmp) / "wt.txt"
            pfile.write_text(porcelain, encoding="utf-8")
            buf = io.StringIO()
            with redirect_stdout(buf):
                code = ipb.main(
                    ["--tag", tag, "--branch", branch, "--porcelain-file", str(pfile)]
                )
            return code, json.loads(buf.getvalue())

    def test_worktree_branch_resolves_and_normalizes(self):
        code, out = self._run("ENG-603", "worktree-eng-603", PORCELAIN)
        self.assertEqual(code, 0)
        self.assertEqual(out["tag"], "eng-603")
        self.assertTrue(out["tag_valid"])
        self.assertEqual(out["base_repo"], "/Users/alex/repos/dropset")
        self.assertEqual(out["normalized_branch"], "eng-603")
        self.assertTrue(out["rename_needed"])

    def test_invalid_tag_exits_nonzero(self):
        code, out = self._run("not-a-tag", "eng-603", PORCELAIN)
        self.assertEqual(code, 1)
        self.assertFalse(out["tag_valid"])
        self.assertIsNone(out["tag"])


if __name__ == "__main__":
    unittest.main()
