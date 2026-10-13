# cspell:word NOSYSTEM
"""Stdlib ``unittest`` tests for the atomic session-close tool.

Run via the repo's ``make tools-tests`` (discovery adds ``.claude/tools`` as
the top-level dir so the bare ``import close_session`` below resolves).
"""

import json
import os
import shutil
import subprocess
import tempfile
import unittest
from types import SimpleNamespace

import board_batch
from close_session import (
    Refusal,
    ancestor_pids,
    close,
    issue_for,
    lock_reasons,
    lock_verdict,
    pr_verdict,
    remove,
)

BASE = "/repo/dropset"
TREE = "/repo/dropset/.claude/worktrees/eng-1669"
SESSION_PID = 4242
LOCK = f"claude session eng-1669 (pid {SESSION_PID} start Tue Oct 13 01:17:34 2026)"

PORCELAIN = f"""\
worktree {BASE}
HEAD abc
branch refs/heads/main

worktree {TREE}
HEAD def
branch refs/heads/eng-1669
locked {LOCK}

worktree /repo/dropset/.claude/worktrees/eng-7
HEAD 012
branch refs/heads/eng-7
"""


class FakeRun:
    """Answers each command from a small table; records every call.

    Defaults describe a closable session: clean, pushed, PR merged, locked by
    an ancestor pid. Tests flip one field to introduce one hazard.
    """

    def __init__(self, **overrides):
        self.cwd_top = TREE
        self.porcelain = PORCELAIN
        self.status = ""
        self.unpushed = "0"
        self.upstream = True
        self.beyond_main = "0"
        self.prs = [{"number": 503, "state": "MERGED"}]
        self.gh_rc = 0
        self.parents = {1000: 999, 999: SESSION_PID, SESSION_PID: 1}
        self.remove_rc = 0
        self.unlock_rc = 0
        self.__dict__.update(overrides)
        self.calls: list[list[str]] = []

    def __call__(self, cmd, cwd=None):
        self.calls.append(cmd)
        if cmd[:2] == ["ps", "-o"]:
            parent = self.parents.get(int(cmd[-1]))
            return (0, f"{parent}\n", "") if parent else (1, "", "")
        if cmd[0] == "gh":
            return self.gh_rc, json.dumps(self.prs), "gh: boom"
        args = cmd[1:]
        if args[:2] == ["-C", BASE]:
            sub = args[2:]
            if sub[:2] == ["worktree", "unlock"]:
                return self.unlock_rc, "", "unlock: no"
            if sub[:2] == ["worktree", "remove"]:
                return self.remove_rc, "", "fatal: contains modified files"
            return 0, "", ""
        if args == ["rev-parse", "--show-toplevel"]:
            return 0, self.cwd_top + "\n", ""
        if args == ["worktree", "list", "--porcelain"]:
            return 0, self.porcelain, ""
        if args[2:] == ["status", "--porcelain"]:
            return 0, self.status, ""
        if args[2:3] == ["rev-list"] and args[-1] == "origin/main..HEAD":
            return 0, self.beyond_main + "\n", ""
        if args[:1] == ["rev-list"] or args[2:3] == ["rev-list"]:
            if not self.upstream:
                return 128, "", "fatal: no upstream configured"
            return 0, self.unpushed + "\n", ""
        raise AssertionError(f"unexpected command {cmd}")


def ns(**kw):
    base = {"state": "Done", "issue": None, "keep_worktree": False, "dry_run": False}
    base.update(kw)
    return SimpleNamespace(**base)


class Writer:
    def __init__(self, fail: bool = False):
        self.fail = fail
        self.calls: list[tuple] = []

    def __call__(self, issue, state, dry_run):
        self.calls.append((issue, state, dry_run))
        if self.fail:
            raise board_batch.BoardBatchError("no state named 'Bogus'")


def run_close(args=None, writer=None, **fake):
    runner = FakeRun(**fake)
    writer = writer or Writer()
    rc, result = close(args or ns(), run=runner, writer=writer, pid=1000)
    return rc, result, runner, writer


class PureTests(unittest.TestCase):
    def test_issue_from_branch_and_override(self):
        self.assertEqual(issue_for("eng-1669", None), "ENG-1669")
        self.assertEqual(issue_for("worktree-eng-42", None), "ENG-42")
        self.assertEqual(issue_for("worktree-ceo-1571", "eng-1571"), "ENG-1571")
        with self.assertRaises(Refusal):
            issue_for("worktree-ceo-1571", None)
        with self.assertRaises(Refusal):
            issue_for("eng-1", "FIN-1")

    def test_lock_reasons(self):
        locks = lock_reasons(PORCELAIN)
        self.assertIsNone(locks[BASE])
        self.assertEqual(locks[TREE], LOCK)
        self.assertEqual(lock_reasons("worktree /x\nlocked\n"), {"/x": ""})

    def test_lock_verdict(self):
        self.assertFalse(lock_verdict(None, set()))
        self.assertTrue(lock_verdict(LOCK, {SESSION_PID}))
        with self.assertRaisesRegex(Refusal, "another live process"):
            lock_verdict(LOCK, {1, 2})
        with self.assertRaisesRegex(Refusal, "no session pid"):
            lock_verdict("", {SESSION_PID})

    def test_pr_verdict(self):
        self.assertIsNone(pr_verdict([]))
        merged = {"number": 1, "state": "MERGED"}
        self.assertEqual(pr_verdict([merged]), merged)
        with self.assertRaisesRegex(Refusal, "#2 is still open"):
            pr_verdict([merged, {"number": 2, "state": "OPEN"}])

    def test_ancestor_pids_walks_to_init(self):
        runner = FakeRun()
        self.assertEqual(ancestor_pids(runner, 1000), {1000, 999, SESSION_PID})


class CloseTests(unittest.TestCase):
    def test_happy_path_writes_then_unlocks_removes_and_deletes(self):
        rc, result, runner, writer = run_close()
        self.assertEqual(rc, 0, result)
        self.assertTrue(result["state_written"])
        self.assertTrue(result["removed"])
        self.assertEqual(writer.calls, [("ENG-1669", "Done", False)])
        cmds = [c[3:5] for c in runner.calls if c[1:3] == ["-C", BASE]]
        self.assertEqual(
            cmds,
            [
                ["worktree", "unlock"],
                ["worktree", "remove"],
                ["branch", "-D"],
                ["worktree", "prune"],
            ],
        )
        remove = next(c for c in runner.calls if c[3:5] == ["worktree", "remove"])
        self.assertNotIn("--force", remove)
        self.assertNotIn("-f", remove)

    def test_state_is_written_before_any_removal(self):
        order = []
        runner = FakeRun()

        def tracking_run(cmd, cwd=None):
            if cmd[1:3] == ["-C", BASE]:
                order.append("git")
            return runner(cmd, cwd)

        def writer(*_):
            order.append("write")

        close(ns(), run=tracking_run, writer=writer, pid=1000)
        self.assertEqual(order[0], "write")

    def test_failed_state_write_removes_nothing(self):
        rc, result, runner, _ = run_close(writer=Writer(fail=True))
        self.assertEqual(rc, 1)
        self.assertIn("state write failed", result["refused"])
        self.assertFalse(result["removed"])
        self.assertFalse(any(c[1:3] == ["-C", BASE] for c in runner.calls))

    def test_each_gate_refuses_before_the_write(self):
        cases = {
            "uncommitted": {"status": " M a.py\n"},
            "unpushed": {"unpushed": "2"},
            "open PR": {"prs": [{"number": 503, "state": "OPEN"}]},
            "gh failure": {"gh_rc": 1},
            "foreign lock": {"parents": {1000: 1}},
            "base checkout": {"cwd_top": BASE},
        }
        for name, fake in cases.items():
            with self.subTest(name):
                rc, result, _, writer = run_close(**fake)
                self.assertEqual(rc, 1, result)
                self.assertIsNotNone(result["refused"])
                self.assertEqual(writer.calls, [])

    def test_no_upstream_passes_only_with_nothing_beyond_main(self):
        rc, result, _, _ = run_close(
            args=ns(state="In Review", issue="ENG-1669"), upstream=False
        )
        self.assertEqual(rc, 0, result)
        rc, result, _, writer = run_close(upstream=False, beyond_main="3")
        self.assertEqual(rc, 1)
        self.assertIn("3 commit(s) beyond origin/main", result["refused"])
        self.assertEqual(writer.calls, [])

    def test_no_pr_is_a_terminal_state(self):
        rc, result, _, _ = run_close(prs=[])
        self.assertEqual(rc, 0, result)
        self.assertIsNone(result["pr"])

    def test_keep_worktree_writes_and_skips_lock_and_removal(self):
        rc, result, runner, writer = run_close(
            args=ns(keep_worktree=True), parents={1000: 1}
        )
        self.assertEqual(rc, 0, result)
        self.assertTrue(result["state_written"])
        self.assertFalse(result["removed"])
        self.assertEqual(len(writer.calls), 1)
        self.assertFalse(any(c[1:3] == ["-C", BASE] for c in runner.calls))

    def test_dry_run_resolves_state_and_mutates_nothing(self):
        rc, result, runner, writer = run_close(args=ns(dry_run=True))
        self.assertEqual(rc, 0, result)
        self.assertEqual(writer.calls, [("ENG-1669", "Done", True)])
        self.assertFalse(result["state_written"])
        self.assertFalse(any(c[1:3] == ["-C", BASE] for c in runner.calls))

    def test_refused_removal_after_write_locks_again_and_exits_2(self):
        rc, result, runner, _ = run_close(remove_rc=1)
        self.assertEqual(rc, 2)
        self.assertTrue(result["state_written"])
        self.assertFalse(result["removed"])
        locks = [c for c in runner.calls if c[3:5] == ["worktree", "lock"]]
        self.assertEqual(
            locks, [["git", "-C", BASE, "worktree", "lock", "--reason", LOCK, TREE]]
        )
        self.assertFalse(any(c[3:5] == ["branch", "-D"] for c in runner.calls))

    def test_unlocked_tree_removes_without_unlocking(self):
        porcelain = PORCELAIN.replace(f"locked {LOCK}\n", "")
        rc, result, runner, _ = run_close(porcelain=porcelain, parents={1000: 1})
        self.assertEqual(rc, 0, result)
        self.assertFalse(any(c[3:5] == ["worktree", "unlock"] for c in runner.calls))


@unittest.skipUnless(shutil.which("git"), "git not installed")
class RealGitRemoveTests(unittest.TestCase):
    """``remove`` against a real repository: a harness-style lock and an
    ignored build tree must not stop a no-``--force`` removal."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.base = os.path.join(self.tmp, "base")
        self.tree = os.path.join(self.tmp, "eng-1")
        env = {"GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}
        self.env = {**os.environ, **env}
        self.git("init", "-q", "-b", "main", self.base)
        with open(os.path.join(self.base, ".gitignore"), "w") as f:
            f.write("node_modules/\n")
        self.git("-C", self.base, "add", ".gitignore")
        self.git(
            "-C", self.base, "-c", "user.name=t", "-c", "user.email=t@t",
            "-c", "commit.gpgsign=false", "commit", "-qm", "init",
        )  # fmt: skip
        self.git("-C", self.base, "worktree", "add", "-q", "-b", "eng-1", self.tree)
        os.makedirs(os.path.join(self.tree, "node_modules", "x"))
        self.git("-C", self.base, "worktree", "lock", "--reason", LOCK, self.tree)

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def git(self, *args):
        subprocess.run(["git", *args], check=True, capture_output=True, env=self.env)

    def run_git(self, cmd, cwd=None):
        proc = subprocess.run(cmd, capture_output=True, text=True, env=self.env)
        return proc.returncode, proc.stdout, proc.stderr

    def test_unlocks_removes_and_deletes_branch(self):
        self.assertIsNone(remove(self.run_git, self.base, self.tree, "eng-1", LOCK))
        self.assertFalse(os.path.exists(self.tree))
        rc, out, _ = self.run_git(["git", "-C", self.base, "branch", "--list", "eng-1"])
        self.assertEqual((rc, out.strip()), (0, ""))

    def test_still_locked_tree_is_refused_and_kept(self):
        why = remove(self.run_git, self.base, self.tree, "eng-1", None)
        self.assertIn("refused", why)
        self.assertTrue(os.path.isdir(self.tree))


if __name__ == "__main__":
    unittest.main()
