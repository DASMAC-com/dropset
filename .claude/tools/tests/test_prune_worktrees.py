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
    _real_alive,
    is_base,
    normalize_branch,
    parse_worktrees,
    prune,
    stale_lock,
    tier_of,
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

    def __init__(
        self,
        porcelain,
        dirty=(),
        modified=(),
        unpushed=(),
        no_upstream=(),
        local_branches=(),
        refs_fail=False,
        delete_fails=(),
        on_main=(),
        locked=(),
    ):
        # Paths `git worktree remove` refuses even with one `--force`: a live
        # session holds the lock.
        self.locked = set(locked)
        # Branches with no upstream whose every commit is on `origin/main` —
        # the never-pushed architect worktree.
        self.on_main = set(on_main)
        self.refs_fail = refs_fail
        self.delete_fails = set(delete_fails)
        self.porcelain = porcelain
        self.dirty = set(dirty)
        self.modified = set(modified)
        self.unpushed = dict(unpushed)
        self.no_upstream = set(no_upstream)
        self.local_branches = list(local_branches)
        self.calls = []

    def __call__(self, args):
        self.calls.append(args)
        if args[:2] == ["worktree", "list"]:
            return 0, self.porcelain, ""
        if args[0] == "for-each-ref":
            if self.refs_fail:
                return 128, "", "fatal: not a git repository"
            return 0, "".join(f"{b}\n" for b in self.local_branches), ""
        if args[:2] == ["branch", "-D"] and args[2] in self.delete_fails:
            return 1, "", "error: cannot delete\nhint: second line\n"

        if args[0] == "rev-list":
            if args[2].startswith("origin/main.."):
                branch = args[2].split("..", 1)[1]
                return 0, "0\n" if branch in self.on_main else "1\n", ""
            branch = args[2].split("@", 1)[0]
            if branch in self.no_upstream:
                return 128, "", "no upstream configured"
            return 0, "%s\n" % self.unpushed.get(branch, 0), ""
        if args[:2] == ["worktree", "unlock"]:
            self.locked.discard(args[2])
            return 0, "", ""
        if args[:2] == ["worktree", "remove"]:
            path = args[-1]
            if path in self.locked:
                return 1, "", "cannot remove a locked working tree"
            if path in self.dirty and "--force" not in args:
                return 1, "", "contains modified or untracked files, use --force"
            return 0, "", ""
        if args[0] == "-C":
            path = args[1]
            if args[2] == "status":
                return (
                    (0, " M src/lib.rs\n", "") if path in self.modified else (0, "", "")
                )
            if args[2] == "rev-list":
                spec = args[4]
                if "@" in spec:
                    branch = spec.split("@", 1)[0]
                else:
                    branch = spec.split("..", 1)[1]
                if spec.startswith("origin/main.."):
                    return 0, "0\n" if branch in self.on_main else "1\n", ""
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

    def test_a_never_pushed_branch_with_nothing_beyond_main_is_removed(self):
        # The read-only architect worktree: no upstream by design, and no
        # commit of its own, so there is nothing unpushed to lose.
        git = FakeGit(PORCELAIN, no_upstream=["eng-701"], on_main=["eng-701"])
        out = prune({"eng-701"}, dry_run=False, git=git)
        self.assertEqual([r["branch"] for r in out["removed"]], ["eng-701"])
        self.assertEqual(out["skipped"], [])

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


ADVISOR_PORCELAIN = """\
worktree /repo/dropset
HEAD abc
branch refs/heads/main

worktree /repo/dropset/.claude/worktrees/1957
HEAD def
branch refs/heads/eng-1957

worktree /repo/dropset/.claude/worktrees/1958
HEAD 012
branch refs/heads/eng-1958
"""
ADVISOR_PATH = "/repo/dropset/.claude/worktrees/1957"
EXECUTOR_PATH = "/repo/dropset/.claude/worktrees/1958"


def tiers(**by_path):
    return lambda path: by_path.get(path)


class TierGateTests(unittest.TestCase):
    """The gate follows the launch's recorded tier, never the branch name.

    The caller (``upkeep``) has already confirmed the issue completed or
    canceled; an In Review issue never reaches ``merged``, so its worktree is
    in ``left`` whatever its tier.
    """

    TIERS = staticmethod(tiers(**{ADVISOR_PATH: "advisor", EXECUTOR_PATH: "executor"}))

    def test_a_done_advisor_worktree_with_no_upstream_and_a_spec_file_is_removed(self):
        git = FakeGit(
            ADVISOR_PORCELAIN,
            no_upstream=["eng-1957"],
            on_main=["eng-1957"],
            modified=[ADVISOR_PATH],
            dirty=[ADVISOR_PATH],
        )
        out = prune({"eng-1957"}, dry_run=False, git=git, tier=self.TIERS)
        self.assertEqual([r["branch"] for r in out["removed"]], ["eng-1957"])
        self.assertIn(["worktree", "remove", "--force", ADVISOR_PATH], git.calls)
        self.assertIn(["branch", "-D", "eng-1957"], git.calls)

    def test_an_in_review_advisor_worktree_is_kept(self):
        git = FakeGit(ADVISOR_PORCELAIN, no_upstream=["eng-1957"])
        out = prune(set(), dry_run=False, git=git, tier=self.TIERS)
        self.assertIn("eng-1957", [t["branch"] for t in out["left"]])
        self.assertFalse(any(c[:2] == ["worktree", "remove"] for c in git.calls))

    def test_an_advisor_commit_beyond_main_is_refused(self):
        git = FakeGit(ADVISOR_PORCELAIN, no_upstream=["eng-1957"])
        out = prune({"eng-1957"}, dry_run=False, git=git, tier=self.TIERS)
        self.assertEqual([s["branch"] for s in out["skipped"]], ["eng-1957"])
        self.assertIn("beyond origin/main", out["skipped"][0]["reason"])
        self.assertNotIn(["branch", "-D", "eng-1957"], git.calls)

    def test_a_locked_advisor_worktree_still_refuses(self):
        git = FakeGit(ADVISOR_PORCELAIN, on_main=["eng-1957"], locked=[ADVISOR_PATH])
        out = prune({"eng-1957"}, dry_run=False, git=git, tier=self.TIERS)
        self.assertEqual([s["branch"] for s in out["skipped"]], ["eng-1957"])
        self.assertNotIn(["branch", "-D", "eng-1957"], git.calls)

    def test_an_executor_worktree_with_no_upstream_and_its_own_commit_is_refused(self):
        git = FakeGit(ADVISOR_PORCELAIN, no_upstream=["eng-1958"])
        out = prune({"eng-1958"}, dry_run=False, git=git, tier=self.TIERS)
        self.assertEqual([s["branch"] for s in out["skipped"]], ["eng-1958"])
        self.assertIn("no upstream", out["skipped"][0]["reason"])

    def test_an_executor_worktree_with_uncommitted_work_is_refused(self):
        git = FakeGit(ADVISOR_PORCELAIN, modified=[EXECUTOR_PATH])
        out = prune({"eng-1958"}, dry_run=False, git=git, tier=self.TIERS)
        self.assertEqual([s["branch"] for s in out["skipped"]], ["eng-1958"])
        self.assertFalse(any(c[:2] == ["worktree", "remove"] for c in git.calls))

    def test_an_unknown_tier_gets_the_executor_gate(self):
        git = FakeGit(ADVISOR_PORCELAIN, modified=[ADVISOR_PATH])
        out = prune({"eng-1957"}, dry_run=False, git=git, tier=tiers())
        self.assertEqual([s["branch"] for s in out["skipped"]], ["eng-1957"])
        self.assertIn("uncommitted", out["skipped"][0]["reason"])


LOCK = "claude session 1957 (pid 4242 start Wed Oct 21 23:12:13 2026)"
LOCKED_PORCELAIN = ADVISOR_PORCELAIN.replace(
    "branch refs/heads/eng-1957\n", f"branch refs/heads/eng-1957\nlocked {LOCK}\n"
).replace(
    "branch refs/heads/eng-1958\n", f"branch refs/heads/eng-1958\nlocked {LOCK}\n"
)


class LockTests(unittest.TestCase):
    """A live session's lock holds every tier; only an advisor's stale Claude
    Code lock — measured to outlive its session — is lifted."""

    TIERS = staticmethod(TierGateTests.TIERS)

    def git(self):
        return FakeGit(
            LOCKED_PORCELAIN,
            on_main=["eng-1957"],
            locked=[ADVISOR_PATH, EXECUTOR_PATH],
        )

    def test_a_live_lock_holds_an_advisor_tree(self):
        git = self.git()
        out = prune(
            {"eng-1957"}, False, git=git, tier=self.TIERS, alive=lambda p, s: True
        )
        self.assertIn("live session", out["skipped"][0]["reason"])
        self.assertFalse(any(c[:2] == ["worktree", "remove"] for c in git.calls))

    def test_a_stale_lock_is_lifted_and_the_advisor_tree_removed(self):
        git = self.git()
        out = prune(
            {"eng-1957"}, False, git=git, tier=self.TIERS, alive=lambda p, s: False
        )
        self.assertEqual([r["branch"] for r in out["removed"]], ["eng-1957"])
        self.assertLess(
            git.calls.index(["worktree", "unlock", ADVISOR_PATH]),
            git.calls.index(["worktree", "remove", "--force", ADVISOR_PATH]),
        )

    def test_a_dry_run_unlocks_nothing(self):
        git = self.git()
        out = prune(
            {"eng-1957"}, True, git=git, tier=self.TIERS, alive=lambda p, s: False
        )
        self.assertEqual([r["branch"] for r in out["removed"]], ["eng-1957"])
        self.assertNotIn(["worktree", "unlock", ADVISOR_PATH], git.calls)

    def test_an_executor_stale_lock_is_never_lifted(self):
        git = self.git()
        out = prune(
            {"eng-1958"}, False, git=git, tier=self.TIERS, alive=lambda p, s: False
        )
        self.assertEqual([s["branch"] for s in out["skipped"]], ["eng-1958"])
        self.assertNotIn(["worktree", "unlock", EXECUTOR_PATH], git.calls)

    def test_stale_lock_reads_only_a_claude_session_lock(self):
        dead = lambda p, s: False  # noqa: E731
        self.assertTrue(stale_lock(LOCK, dead))
        self.assertFalse(stale_lock(LOCK, lambda p, s: True))
        self.assertFalse(stale_lock("held by the operator", dead))
        self.assertFalse(stale_lock("", dead))
        self.assertFalse(stale_lock(None, dead))

    def test_the_real_probe_sees_this_process_alive_whatever_the_start_text(self):
        # The guard against the measured UTC-vs-local mismatch: a live pid
        # must read alive even when the recorded start text differs.
        self.assertTrue(_real_alive(os.getpid(), "Thu Jan  1 00:00:00 1970"))

    def test_the_lock_line_is_parsed(self):
        trees = parse_worktrees(LOCKED_PORCELAIN)
        self.assertEqual(trees[1]["locked"], LOCK)
        self.assertNotIn("locked", trees[0])


class TierOfTests(unittest.TestCase):
    def _markers(self, files):
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        markers = os.path.join(d.name, ".claude", "session-substrate")
        os.makedirs(markers)
        for name, text in files.items():
            with open(os.path.join(markers, name), "w", encoding="utf-8") as fh:
                fh.write(text)
        return d.name

    def test_a_tier_file_names_the_tier(self):
        base = self._markers({"1957": "anthropic\n", "1957.tier": "advisor\n"})
        self.assertEqual(tier_of(base, f"{base}/.claude/worktrees/1957"), "advisor")

    def test_a_bare_marker_is_an_executor(self):
        base = self._markers({"1958": "bedrock\n"})
        self.assertEqual(tier_of(base, f"{base}/.claude/worktrees/1958"), "executor")

    def test_a_pre_rename_directory_and_marker_key_resolve(self):
        base = self._markers({"eng-814": "bedrock\n"})
        self.assertEqual(tier_of(base, f"{base}/.claude/worktrees/ceo-814"), "executor")

    def test_an_advisor_marker_never_lifts_the_gate_on_a_pre_rename_tree(self):
        # The marker keys on the number: an `explore 814` that reused a legacy
        # `eng-814` task tree must not hand that tree the advisor gate.
        base = self._markers({"814": "anthropic\n", "814.tier": "advisor\n"})
        self.assertIsNone(tier_of(base, f"{base}/.claude/worktrees/eng-814"))
        self.assertEqual(tier_of(base, f"{base}/.claude/worktrees/814"), "advisor")

    def test_no_marker_is_unknown(self):
        base = self._markers({})
        self.assertIsNone(tier_of(base, f"{base}/.claude/worktrees/1959"))
        self.assertIsNone(tier_of(base, f"{base}/.claude/worktrees/scratch"))


class StrayBranchTests(unittest.TestCase):
    """A merged branch whose worktree is already gone is deleted too."""

    LOCALS = ["main", "eng-701", "eng-702", "eng-703", "eng-800", "eng-801"]

    def test_a_stray_merged_branch_is_deleted(self):
        git = FakeGit(PORCELAIN, local_branches=self.LOCALS)
        out = prune({"eng-800"}, dry_run=False, git=git)
        self.assertEqual(out["branches_removed"], ["eng-800"])
        self.assertIn(["branch", "-D", "eng-800"], git.calls)
        # an unmerged stray is untouched
        self.assertNotIn(["branch", "-D", "eng-801"], git.calls)

    def test_a_stray_with_unpushed_commits_is_skipped(self):
        git = FakeGit(PORCELAIN, local_branches=self.LOCALS, unpushed={"eng-800": 2})
        out = prune({"eng-800"}, dry_run=False, git=git)
        self.assertEqual(out["branches_removed"], [])
        self.assertEqual(out["skipped"][0]["path"], None)
        self.assertIn("unpushed", out["skipped"][0]["reason"])
        self.assertNotIn(["branch", "-D", "eng-800"], git.calls)

    def test_a_stray_with_no_upstream_is_skipped(self):
        git = FakeGit(PORCELAIN, local_branches=self.LOCALS, no_upstream=["eng-800"])
        out = prune({"eng-800"}, dry_run=False, git=git)
        self.assertEqual(out["branches_removed"], [])
        self.assertIn("no upstream", out["skipped"][0]["reason"])
        self.assertNotIn(["branch", "-D", "eng-800"], git.calls)

    def test_main_is_never_deleted_as_a_stray(self):
        git = FakeGit(PORCELAIN, local_branches=self.LOCALS)
        out = prune({"main"}, dry_run=False, git=git)
        self.assertEqual(out["branches_removed"], [])
        self.assertNotIn(["branch", "-D", "main"], git.calls)

    def test_a_refused_stray_delete_keeps_only_the_first_error_line(self):
        git = FakeGit(PORCELAIN, local_branches=self.LOCALS, delete_fails=["eng-800"])
        out = prune({"eng-800"}, dry_run=False, git=git)
        self.assertEqual(out["branches_removed"], [])
        self.assertEqual(out["skipped"][0]["reason"], "error: cannot delete")

    def test_a_failed_branch_list_deletes_nothing_and_reports_no_unmatched(self):
        git = FakeGit(PORCELAIN, refs_fail=True)
        out = prune({"eng-800", "eng-701"}, dry_run=False, git=git)
        self.assertEqual(out["branches_removed"], [])
        self.assertEqual(out["unmatched"], [])
        self.assertNotIn(["branch", "-D", "eng-800"], git.calls)

    def test_a_dry_run_reports_strays_without_deleting(self):
        git = FakeGit(PORCELAIN, local_branches=self.LOCALS)
        out = prune({"eng-800"}, dry_run=True, git=git)
        self.assertEqual(out["branches_removed"], ["eng-800"])
        self.assertTrue(all(c[:2] != ["branch", "-D"] for c in git.calls))

    def test_a_worktree_branch_is_not_double_counted_as_a_stray(self):
        git = FakeGit(PORCELAIN, local_branches=self.LOCALS)
        out = prune({"eng-701"}, dry_run=False, git=git)
        self.assertEqual([r["branch"] for r in out["removed"]], ["eng-701"])
        self.assertEqual(out["branches_removed"], [])

    def test_a_name_matching_nothing_is_reported_unmatched(self):
        git = FakeGit(PORCELAIN, local_branches=self.LOCALS)
        out = prune({"eng-701", "eng-999"}, dry_run=True, git=git)
        self.assertEqual(out["unmatched"], ["eng-999"])


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
