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
from unittest import mock

import board_batch
import close_session
from close_session import (
    Refusal,
    ancestor_pids,
    close,
    issue_for,
    lock_reasons,
    lock_verdict,
    pid_alive,
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
        self.head = "abc123"
        self.prs = [{"number": 503, "state": "MERGED", "headRefOid": "abc123"}]
        self.gh_rc = 0
        self.gh_out = None
        self.parents = {1000: 999, 999: SESSION_PID, SESSION_PID: 1}
        self.remove_rc = 0
        self.unlock_rc = 0
        self.lock_rc = 0
        self.branch_rc = 0
        self.__dict__.update(overrides)
        self.calls: list[list[str]] = []

    def __call__(self, cmd, cwd=None):
        self.calls.append(cmd)
        if cmd[:2] == ["ps", "-o"]:
            parent = self.parents.get(int(cmd[-1]))
            return (0, f"{parent}\n", "") if parent else (1, "", "")
        if cmd[0] == "gh":
            out = json.dumps(self.prs) if self.gh_out is None else self.gh_out
            return self.gh_rc, out, "gh: boom"
        args = cmd[1:]
        if args[:2] == ["-C", BASE]:
            sub = args[2:]
            if sub[:2] == ["worktree", "unlock"]:
                return self.unlock_rc, "", "unlock: no"
            if sub[:2] == ["worktree", "remove"]:
                return self.remove_rc, "", "fatal: contains modified files"
            if sub[:2] == ["worktree", "lock"]:
                return self.lock_rc, "", "lock: no"
            if sub[:2] == ["branch", "-D"]:
                return self.branch_rc, "", "error: branch busy"
            return 0, "", ""
        if args == ["rev-parse", "--show-toplevel"]:
            return 0, self.cwd_top + "\n", ""
        if args == ["worktree", "list", "--porcelain"]:
            return 0, self.porcelain, ""
        if args[2:] == ["status", "--porcelain"]:
            return 0, self.status, ""
        if args[2:] == ["rev-parse", "HEAD"]:
            return 0, self.head + "\n", ""
        if args[2:3] == ["rev-list"] and args[-1] == "origin/main..HEAD":
            return 0, self.beyond_main + "\n", ""
        if args[2:3] == ["rev-list"]:
            if not self.upstream:
                return 128, "", "fatal: no upstream configured"
            return 0, self.unpushed + "\n", ""
        raise AssertionError(f"unexpected command {cmd}")


def ns(**kw):
    base = {
        "state": "Done",
        "issue": None,
        "keep_worktree": False,
        "dry_run": False,
        "session_id": None,
    }
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


def run_close(args=None, writer=None, alive=lambda _pid: True, **fake):
    runner = FakeRun(**fake)
    writer = writer or Writer()
    rc, result = close(args or ns(), run=runner, writer=writer, pid=1000, alive=alive)
    return rc, result, runner, writer


class PureTests(unittest.TestCase):
    def test_issue_from_branch_and_override(self):
        self.assertEqual(issue_for("eng-1669", None), "ENG-1669")
        self.assertEqual(issue_for("worktree-eng-42", None), "ENG-42")
        # A bare-number architect / explore session never renames its branch.
        self.assertEqual(issue_for("worktree-1572", None), "ENG-1572")
        self.assertEqual(issue_for("worktree-ceo-1571", "eng-1571"), "ENG-1571")
        with self.assertRaises(Refusal):
            issue_for("worktree-ceo-1571", None)
        with self.assertRaises(Refusal):
            issue_for("eng-1", "FIN-1")
        self.assertEqual(issue_for("eng-1669", "ENG-1669"), "ENG-1669")
        with self.assertRaisesRegex(Refusal, "disagrees"):
            issue_for("eng-1669", "ENG-1")

    def test_lock_reasons(self):
        locks = lock_reasons(PORCELAIN)
        self.assertIsNone(locks[BASE])
        self.assertEqual(locks[TREE], LOCK)
        self.assertEqual(lock_reasons("worktree /x\nlocked\n"), {"/x": ""})

    def test_lock_reasons_keys_resolve_through_symlinks(self):
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp, True)
        real = os.path.join(tmp, "real")
        os.mkdir(real)
        link = os.path.join(tmp, "link")
        os.symlink(real, link)
        locks = lock_reasons(f"worktree {link}\nlocked {LOCK}\n")
        self.assertEqual(locks, {os.path.realpath(real): LOCK})

    def test_lock_verdict(self):
        self.assertFalse(lock_verdict(None, set()))
        self.assertTrue(lock_verdict(LOCK, {SESSION_PID}))
        with self.assertRaisesRegex(Refusal, "another live process"):
            lock_verdict(LOCK, {1, 2}, alive=lambda _pid: True)
        with self.assertRaisesRegex(Refusal, "stale lock from pid 4242"):
            lock_verdict(LOCK, {1, 2}, alive=lambda _pid: False)
        with self.assertRaisesRegex(Refusal, "no session pid"):
            lock_verdict("", {SESSION_PID})

    def test_pid_alive(self):
        self.assertTrue(pid_alive(os.getpid()))
        proc = subprocess.Popen(["true"])
        proc.wait()
        self.assertFalse(pid_alive(proc.pid))

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
            "uncommitted": ({"status": " M a.py\n"}, "uncommitted"),
            "unpushed": ({"unpushed": "2"}, "2 unpushed commit(s)"),
            "open PR": ({"prs": [{"number": 503, "state": "OPEN"}]}, "still open"),
            "gh failure": ({"gh_rc": 1}, "could not read the branch's PRs"),
            "gh garbage": ({"gh_out": "not json"}, "unreadable gh pr list"),
            "gh non-list": ({"gh_out": "{}"}, "unreadable gh pr list"),
            "foreign lock": ({"parents": {1000: 1}}, "another live process"),
            "base checkout": ({"cwd_top": BASE}, "not the base checkout"),
        }
        for name, (fake, why) in cases.items():
            with self.subTest(name):
                rc, result, _, writer = run_close(**fake)
                self.assertEqual(rc, 1, result)
                self.assertIn(why, result["refused"])
                self.assertEqual(writer.calls, [])

    def test_live_state_refuses_removal_unless_kept(self):
        rc, result, _, writer = run_close(args=ns(state="In Review"))
        self.assertEqual(rc, 1)
        self.assertIn("--keep-worktree", result["refused"])
        self.assertEqual(writer.calls, [])
        rc, result, _, _ = run_close(args=ns(state="In Review", keep_worktree=True))
        self.assertEqual(rc, 0, result)

    def test_squash_merged_branch_with_upstream_is_not_held_to_main(self):
        rc, result, _, _ = run_close(beyond_main="3")
        self.assertEqual(rc, 0, result)

    def test_no_upstream_passes_with_nothing_beyond_main(self):
        rc, result, _, _ = run_close(
            args=ns(issue="ENG-1669"), upstream=False, head="def456"
        )
        self.assertEqual(rc, 0, result)

    def test_no_upstream_passes_when_head_is_the_merged_pr_head(self):
        rc, result, _, _ = run_close(upstream=False, beyond_main="3")
        self.assertEqual(rc, 0, result)

    def test_no_upstream_refuses_unproven_commits(self):
        cases = {
            "head differs": {"head": "def456"},
            "head of an unmerged PR": {
                "prs": [{"number": 503, "state": "CLOSED", "headRefOid": "abc123"}]
            },
        }
        for name, fake in cases.items():
            with self.subTest(name):
                rc, result, _, writer = run_close(
                    upstream=False, beyond_main="3", **fake
                )
                self.assertEqual(rc, 1, result)
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

    def test_failed_lock_restore_is_reported(self):
        rc, result, _, _ = run_close(remove_rc=1, lock_rc=1)
        self.assertEqual(rc, 2)
        self.assertIn("re-lock FAILED", " ".join(result["notes"]))

    def test_failed_unlock_stops_before_remove(self):
        rc, result, runner, _ = run_close(unlock_rc=1)
        self.assertEqual(rc, 2)
        self.assertIn("unlock failed", " ".join(result["notes"]))
        self.assertFalse(any(c[3:5] == ["worktree", "remove"] for c in runner.calls))

    def test_failed_branch_delete_is_noted(self):
        rc, result, _, _ = run_close(branch_rc=1)
        self.assertEqual(rc, 0, result)
        self.assertTrue(result["removed"])
        self.assertIn("git branch -D failed", " ".join(result["notes"]))

    def test_unlocked_tree_removes_without_unlocking(self):
        porcelain = PORCELAIN.replace(f"locked {LOCK}\n", "")
        rc, result, runner, _ = run_close(porcelain=porcelain, parents={1000: 1})
        self.assertEqual(rc, 0, result)
        self.assertFalse(any(c[3:5] == ["worktree", "unlock"] for c in runner.calls))


@unittest.skipUnless(shutil.which("git"), "git not installed")
class CostLineTests(unittest.TestCase):
    LINE = "**Cost actual**: $72 estimate, 140 turns, peak prefix 310k, 2026-10-27\n"

    def _close(self, args, coster, appender=None, writer=None):
        order: list[str] = []
        writer = writer or (lambda *_: order.append("write"))

        def append(issue, line):
            order.append("append")
            if appender is not None:
                appender(issue, line)

        runner = FakeRun()

        def tracking_run(cmd, cwd=None):
            if cmd[3:5] == ["worktree", "remove"]:
                order.append("remove")
            return runner(cmd, cwd)

        rc, result = close(
            args,
            run=tracking_run,
            writer=writer,
            pid=1000,
            alive=lambda _pid: True,
            coster=coster,
            appender=append,
        )
        return rc, result, order

    def test_line_is_appended_after_the_state_and_before_removal(self):
        priced = []

        def coster(sid, _notes):
            priced.append(sid)
            return self.LINE

        rc, result, order = self._close(ns(session_id="sid-1"), coster)
        self.assertEqual(rc, 0, result)
        self.assertEqual(priced, ["sid-1"])
        self.assertEqual(order, ["write", "append", "remove"])
        self.assertEqual(result["cost_line"], self.LINE)

    def test_no_session_id_appends_nothing(self):
        rc, result, order = self._close(ns(), close_session.price_session)
        self.assertEqual(rc, 0, result)
        self.assertNotIn("append", order)
        self.assertIsNone(result["cost_line"])

    def test_unresolvable_session_skips_the_line_and_says_so(self):
        def missing(_sid, *_):
            raise FileNotFoundError("no transcript sid-x.jsonl found")

        with mock.patch.object(
            close_session.session_metrics, "session_cost_line", missing
        ):
            rc, result, order = self._close(
                ns(session_id="sid-x"), close_session.price_session
            )
        self.assertEqual(rc, 0, result)
        self.assertNotIn("append", order)
        self.assertTrue(any("cost line skipped" in n for n in result["notes"]))

    def test_a_failed_append_is_a_note_not_a_refusal(self):
        def boom(*_):
            raise RuntimeError("linear down")

        rc, result, order = self._close(
            ns(session_id="sid-1"), lambda *_: self.LINE, appender=boom
        )
        self.assertEqual(rc, 0, result)
        self.assertTrue(result["removed"])
        self.assertIsNone(result["cost_line"])
        self.assertTrue(any("not appended" in n for n in result["notes"]))

    def test_dry_run_prices_but_appends_nothing(self):
        rc, result, order = self._close(
            ns(session_id="sid-1", dry_run=True), lambda *_: self.LINE
        )
        self.assertEqual(rc, 0, result)
        self.assertNotIn("append", order)
        self.assertEqual(result["cost_line"], self.LINE)

    def test_failed_state_write_appends_nothing(self):
        rc, result, order = self._close(
            ns(session_id="sid-1"), lambda *_: self.LINE, writer=Writer(fail=True)
        )
        self.assertEqual(rc, 1)
        self.assertEqual(order, [])


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
        notes = []
        self.assertIsNone(
            remove(self.run_git, self.base, self.tree, "eng-1", LOCK, notes)
        )
        self.assertEqual(notes, [])
        self.assertFalse(os.path.exists(self.tree))
        rc, out, _ = self.run_git(["git", "-C", self.base, "branch", "--list", "eng-1"])
        self.assertEqual((rc, out.strip()), (0, ""))

    def test_removes_the_tree_its_caller_stands_in(self):
        # The tool runs with its cwd inside the tree, and its later git calls
        # inherit a cwd that no longer exists; `-C base` carries them.
        here = os.getcwd()
        os.chdir(self.tree)
        self.addCleanup(os.chdir, here)
        notes = []
        self.assertIsNone(
            remove(self.run_git, self.base, self.tree, "eng-1", LOCK, notes)
        )
        self.assertEqual(notes, [])
        self.assertFalse(os.path.exists(self.tree))

    def test_still_locked_tree_is_refused_and_kept(self):
        why = remove(self.run_git, self.base, self.tree, "eng-1", None, [])
        self.assertIn("refused", why)
        self.assertTrue(os.path.isdir(self.tree))


if __name__ == "__main__":
    unittest.main()
