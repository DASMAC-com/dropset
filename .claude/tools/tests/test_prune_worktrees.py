"""Stdlib ``unittest`` tests for the worktree-prune helper.

Run via the repo's ``make tools-tests`` (discovery adds ``.claude/tools`` as
the top-level dir so the bare ``import prune_worktrees`` below resolves).
"""

import os
import tempfile
import unittest
from types import SimpleNamespace

from prune_worktrees import (
    _read_merged,
    is_base,
    normalize_branch,
    parse_worktrees,
    prune,
)

PORCELAIN = """\
worktree /repo/dropset
HEAD abc
branch refs/heads/main

worktree /repo/dropset/.claude/worktrees/eng-701
HEAD def
branch refs/heads/eng-701

worktree /repo/dropset/.claude/worktrees/eng-702
HEAD 012
branch refs/heads/eng-702

worktree /repo/dropset/.claude/worktrees/eng-703
HEAD 345
branch refs/heads/eng-703
"""


class ParseTests(unittest.TestCase):
    def test_parse_worktrees(self):
        trees = parse_worktrees(PORCELAIN)
        self.assertEqual(len(trees), 4)
        self.assertEqual(trees[0]["branch"], "main")
        self.assertTrue(is_base(trees[0]))
        self.assertEqual(trees[1]["branch"], "eng-701")
        self.assertFalse(is_base(trees[1]))

    def test_normalize_branch(self):
        self.assertEqual(normalize_branch("refs/heads/eng-1"), "eng-1")
        self.assertEqual(normalize_branch("  eng-2 "), "eng-2")


class FakeGit:
    """Records calls; ``remove`` fails for paths in ``dirty``."""

    def __init__(self, porcelain, dirty=()):
        self.porcelain = porcelain
        self.dirty = set(dirty)
        self.calls = []

    def __call__(self, args):
        self.calls.append(args)
        if args[:2] == ["worktree", "list"]:
            return 0, self.porcelain, ""
        if args[:2] == ["worktree", "remove"]:
            path = args[2]
            if path in self.dirty:
                return 1, "", "contains modified or untracked files, use --force"
            return 0, "", ""
        return 0, "", ""


class PruneTests(unittest.TestCase):
    def test_removes_merged_leaves_unmerged_and_never_base(self):
        git = FakeGit(PORCELAIN)
        out = prune({"eng-701", "eng-702"}, dry_run=False, git=git)
        removed = {r["branch"] for r in out["removed"]}
        left = {r["branch"] for r in out["left"]}
        self.assertEqual(removed, {"eng-701", "eng-702"})
        self.assertEqual(left, {"eng-703"})  # unmerged
        # main is never a candidate (neither removed nor left)
        self.assertNotIn("main", removed | left)
        self.assertTrue(out["pruned"])
        # each removed branch got both a worktree remove and a branch -D
        self.assertIn(["branch", "-D", "eng-701"], git.calls)
        self.assertIn(["worktree", "prune"], git.calls)

    def test_dirty_worktree_is_skipped_not_removed(self):
        dirty_path = "/repo/dropset/.claude/worktrees/eng-701"
        git = FakeGit(PORCELAIN, dirty=[dirty_path])
        out = prune({"eng-701", "eng-702"}, dry_run=False, git=git)
        self.assertEqual([s["branch"] for s in out["skipped"]], ["eng-701"])
        self.assertEqual([r["branch"] for r in out["removed"]], ["eng-702"])
        # a skipped tree's branch is NOT force-deleted
        self.assertNotIn(["branch", "-D", "eng-701"], git.calls)

    def test_dry_run_removes_nothing(self):
        git = FakeGit(PORCELAIN)
        out = prune({"eng-701"}, dry_run=True, git=git)
        self.assertEqual([r["branch"] for r in out["removed"]], ["eng-701"])
        self.assertFalse(out["pruned"])
        # no mutating git call happened — no worktree remove, no branch delete
        self.assertFalse(any(c[:2] == ["worktree", "remove"] for c in git.calls))
        self.assertTrue(all(c[:2] != ["branch", "-D"] for c in git.calls))

    def test_prune_not_run_when_nothing_removed(self):
        git = FakeGit(PORCELAIN)
        out = prune(set(), dry_run=False, git=git)
        self.assertEqual(out["removed"], [])
        self.assertFalse(out["pruned"])
        self.assertNotIn(["worktree", "prune"], git.calls)


class ReadMergedTests(unittest.TestCase):
    def test_merged_args_and_file_union_normalized(self):
        with tempfile.TemporaryDirectory() as d:
            f = os.path.join(d, "merged.txt")
            with open(f, "w", encoding="utf-8") as fh:
                fh.write("refs/heads/eng-702\neng-703\n")
            args = SimpleNamespace(
                merged=["eng-701", "refs/heads/eng-701"], merged_file=f
            )
            self.assertEqual(_read_merged(args), {"eng-701", "eng-702", "eng-703"})

    def test_no_input_is_empty(self):
        args = SimpleNamespace(merged=[], merged_file=None)
        self.assertEqual(_read_merged(args), set())


if __name__ == "__main__":
    unittest.main()
