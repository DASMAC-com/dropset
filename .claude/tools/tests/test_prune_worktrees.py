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
    """Records calls; ``remove`` fails for paths in ``dirty``.

    ``status`` and ``rev-list`` answer the safety pre-flight. By default every
    tree is clean and fully pushed, so tests opt in to a hazard via ``modified``
    (uncommitted changes), ``unpushed`` (commits never pushed), or
    ``no_upstream``. ``dirty`` is different from ``modified``: it makes ``git
    worktree remove`` itself refuse, which is the *second* check rather than
    the pre-flight.
    """

    def __init__(self, porcelain, dirty=(), modified=(), unpushed=(), no_upstream=()):
        self.porcelain = porcelain
        self.dirty = set(dirty)
        self.modified = set(modified)
        self.unpushed = dict(unpushed)
        self.no_upstream = set(no_upstream)
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
        if args[0] == "-C":
            path = args[1]
            if args[2] == "status":
                return (
                    (0, " M src/lib.rs\n", "") if path in self.modified else (0, "", "")
                )
            if args[2] == "rev-list":
                branch = path.rsplit("/", 1)[-1]
                if branch in self.no_upstream:
                    return 128, "", "no upstream configured"
                return 0, "%s\n" % self.unpushed.get(branch, 0), ""
        return 0, "", ""


class SafetyGateTests(unittest.TestCase):
    """A merged PR is not sufficient — the tree must also hold no unsaved work.

    Two worktrees were destroyed while their PRs were open, the second losing a
    verified review fix that had to be reauthored from a transcript. `git worktree
    remove` catches a dirty tree; nothing caught unpushed commits, and
    `git branch -D` force-deletes them.
    """

    def test_a_tree_with_uncommitted_changes_is_skipped(self):
        path = "/repo/dropset/.claude/worktrees/eng-701"
        git = FakeGit(PORCELAIN, modified=[path])
        out = prune({"eng-701", "eng-702"}, dry_run=False, git=git)
        self.assertEqual([s["branch"] for s in out["skipped"]], ["eng-701"])
        self.assertIn("uncommitted", out["skipped"][0]["reason"])
        # Never reached the destructive calls at all.
        self.assertNotIn(["worktree", "remove", path], git.calls)
        self.assertNotIn(["branch", "-D", "eng-701"], git.calls)

    def test_a_branch_with_unpushed_commits_is_skipped(self):
        git = FakeGit(PORCELAIN, unpushed=[("eng-701", 2)])
        out = prune({"eng-701", "eng-702"}, dry_run=False, git=git)
        self.assertEqual([s["branch"] for s in out["skipped"]], ["eng-701"])
        self.assertIn("2 unpushed commit(s)", out["skipped"][0]["reason"])
        self.assertNotIn(["branch", "-D", "eng-701"], git.calls)
        # The safe sibling still goes.
        self.assertEqual([r["branch"] for r in out["removed"]], ["eng-702"])

    def test_a_branch_with_no_upstream_is_skipped_rather_than_assumed_pushed(self):
        # Guessing in the destructive direction is the entire failure mode.
        git = FakeGit(PORCELAIN, no_upstream=["eng-701"])
        out = prune({"eng-701"}, dry_run=False, git=git)
        self.assertEqual([s["branch"] for s in out["skipped"]], ["eng-701"])
        self.assertIn("upstream", out["skipped"][0]["reason"])

    def test_an_unreadable_status_is_skipped(self):
        class Broken(FakeGit):
            def __call__(self, args):
                if args[0] == "-C" and args[2] == "status":
                    self.calls.append(args)
                    return 128, "", "not a git repository"
                return super().__call__(args)

        git = Broken(PORCELAIN)
        out = prune({"eng-701"}, dry_run=False, git=git)
        self.assertEqual([s["branch"] for s in out["skipped"]], ["eng-701"])
        self.assertIn("unprovable", out["skipped"][0]["reason"])

    def test_an_unreadable_commit_count_is_skipped_not_removed(self):
        """The one branch that used to guess toward REMOVING.

        An rc-0 `rev-list` with empty stdout fell through to "safe to delete".
        Every other branch refuses when it cannot prove safety; this one now does
        too, because the guess must never run in the destructive direction.
        """

        class EmptyCount(FakeGit):
            def __call__(self, args):
                if args[0] == "-C" and args[2] == "rev-list":
                    self.calls.append(args)
                    return 0, "   \n", ""
                return super().__call__(args)

        git = EmptyCount(PORCELAIN)
        out = prune({"eng-701"}, dry_run=False, git=git)
        self.assertEqual([s["branch"] for s in out["skipped"]], ["eng-701"])
        self.assertIn("unprovable", out["skipped"][0]["reason"])
        self.assertNotIn(["branch", "-D", "eng-701"], git.calls)

    def test_a_non_numeric_commit_count_is_skipped(self):
        git = FakeGit(PORCELAIN, unpushed=[("eng-701", "not-a-number")])
        out = prune({"eng-701"}, dry_run=False, git=git)
        self.assertEqual([s["branch"] for s in out["skipped"]], ["eng-701"])

    def test_a_clean_pushed_tree_is_still_removed(self):
        # The gate must not be so cautious that it never removes anything.
        git = FakeGit(PORCELAIN)
        out = prune({"eng-701"}, dry_run=False, git=git)
        self.assertEqual([r["branch"] for r in out["removed"]], ["eng-701"])
        self.assertEqual(out["skipped"], [])

    def test_a_dry_run_reports_the_skip_instead_of_promising_a_removal(self):
        git = FakeGit(PORCELAIN, unpushed=[("eng-701", 1)])
        out = prune({"eng-701"}, dry_run=True, git=git)
        self.assertEqual(out["removed"], [])
        self.assertEqual([s["branch"] for s in out["skipped"]], ["eng-701"])


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
