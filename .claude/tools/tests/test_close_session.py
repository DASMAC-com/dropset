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


def ledger_file(entries) -> str:
    """A leftover ledger on disk, removed when the test run ends."""
    fd, path = tempfile.mkstemp(suffix=".json")
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        fh.write(entries if isinstance(entries, str) else json.dumps(entries))
    unittest.addModuleCleanup(os.unlink, path)
    return path


EMPTY_LEDGER = None


def ns(**kw):
    global EMPTY_LEDGER
    if EMPTY_LEDGER is None:
        EMPTY_LEDGER = ledger_file([])
    base = {
        "state": "Done",
        "issue": None,
        "keep_worktree": False,
        "dry_run": False,
        "session_id": None,
        "leftovers_file": EMPTY_LEDGER,
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
class LeftoverTests(unittest.TestCase):
    """The ledger gate: every leftover filed, handed or dropped before a
    Done close, refused before any write otherwise."""

    GOOD = [
        {"item": "stale USD fallback", "disposition": "filed", "ref": "ENG-2400"},
        {"item": "proxy query", "disposition": "handed", "ref": "schedule-30"},
        {"item": "nit: rename", "disposition": "Dropped", "ref": "landed in #730"},
    ]

    def test_every_disposition_passes_and_counts(self):
        rc, result, _, writer = run_close(
            args=ns(leftovers_file=ledger_file(self.GOOD))
        )
        self.assertEqual(rc, 0, result)
        self.assertEqual(result["leftovers"], 3)
        self.assertEqual(len(writer.calls), 1)

    def test_gaps_name_each_undisposed_item(self):
        gaps = close_session.leftover_gaps(
            [
                {"item": "a", "disposition": "noted in the PR body", "ref": "#594"},
                {"item": "b", "disposition": "filed", "ref": "later"},
                {"item": "c", "disposition": "dropped"},
                {"disposition": "filed", "ref": "ENG-1"},
                "d",
                {"item": "e", "disposition": "handed", "ref": "Operator"},
            ]
        )
        self.assertEqual(len(gaps), 6, gaps)
        self.assertIn("not a live session", gaps[5])
        self.assertIn("'noted in the PR body'", gaps[0])
        self.assertIn("no ENG-###", gaps[1])
        self.assertIn("no ref", gaps[2])
        self.assertIn("names no item", gaps[3])
        self.assertIn("not an object", gaps[4])
        self.assertEqual(close_session.leftover_gaps([]), [])
        self.assertEqual(len(close_session.leftover_gaps({})), 1)

    def test_bad_ledgers_refuse_before_the_write(self):
        cases = {
            "missing flag": (None, "needs --leftovers-file"),
            "unreadable": ("/nonexistent/ledger.json", "unreadable --leftovers-file"),
            "not json": (ledger_file("not json"), "unreadable --leftovers-file"),
            "undisposed": (
                ledger_file([{"item": "x", "disposition": "pr-body", "ref": "#1"}]),
                "1 leftover(s) have no disposition",
            ),
        }
        for name, (path, why) in cases.items():
            with self.subTest(name):
                for keep in (False, True):
                    rc, result, _, writer = run_close(
                        args=ns(leftovers_file=path, keep_worktree=keep)
                    )
                    self.assertEqual(rc, 1, result)
                    self.assertIn(why, result["refused"])
                    self.assertEqual(writer.calls, [])

    def test_dry_run_runs_the_gate(self):
        path = ledger_file([{"item": "x", "disposition": "", "ref": ""}])
        rc, result, _, writer = run_close(args=ns(leftovers_file=path, dry_run=True))
        self.assertEqual(rc, 1)
        self.assertIn("no disposition", result["refused"])
        self.assertEqual(writer.calls, [])

    def test_live_handoff_needs_no_ledger(self):
        rc, result, _, _ = run_close(
            args=ns(state="In Review", keep_worktree=True, leftovers_file=None)
        )
        self.assertEqual(rc, 0, result)
        self.assertIsNone(result["leftovers"])

    def test_parse_args_takes_the_ledger(self):
        args = close_session.parse_args(
            ["close_session.py", "--state", "Done", "--leftovers-file", "l.json"]
        )
        self.assertEqual(args.leftovers_file, "l.json")


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

        events: list[str] = []
        rc, result, order = self._close(
            ns(session_id="sid-1"),
            coster,
            writer=lambda *_: events.append(f"write after {len(priced)} pricing"),
            appender=lambda *_: events.append("append"),
        )
        self.assertEqual(rc, 0, result)
        self.assertEqual(priced, ["sid-1"])
        # Priced before the write, appended only after it, removed last.
        self.assertEqual(events, ["write after 1 pricing", "append"])
        self.assertEqual(order, ["append", "remove"])
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


class NotifyingRun(FakeRun):
    """A closable session whose PR has notification threads: the repo-scoped
    list answers ``threads`` as ``(id, pr)``, and a mark-done DELETE fails for
    an id in ``delete_fails``, an ignore PUT for one in ``ignore_fails``."""

    def __init__(self, threads=(), delete_fails=(), ignore_fails=(), **overrides):
        super().__init__(**overrides)
        self.threads = list(threads)
        self.delete_fails = set(delete_fails)
        self.ignore_fails = set(ignore_fails)

    def __call__(self, cmd, cwd=None):
        if cmd[:2] == ["gh", "api"] and "notifications?" in cmd[2]:
            self.calls.append(cmd)
            url = "https://api.github.com/repos/DASMAC-com/dropset/pulls/{}"
            page = [
                {"id": t, "subject": {"url": url.format(n)}} for t, n in self.threads
            ]
            return 0, json.dumps(page), ""
        if cmd[:3] == ["gh", "api", "-X"]:
            self.calls.append(cmd)
            path = cmd[4]
            if path.endswith("/subscription"):
                fails = path.split("/")[-2] in self.ignore_fails
            else:
                fails = path.rsplit("/", 1)[1] in self.delete_fails
            return (1, "", "HTTP 500") if fails else (0, "", "")
        return super().__call__(cmd, cwd)


class NotificationTests(unittest.TestCase):
    def _close(self, args=None, **fake):
        runner = NotifyingRun(**fake)
        rc, result = close(args or ns(), run=runner, writer=Writer(), pid=1000)
        return rc, result, runner

    def test_the_branch_threads_are_dismissed_before_the_removal(self):
        rc, result, runner = self._close(threads=[("t1", 503), ("t2", 777)])
        self.assertEqual(rc, 0, result)
        writes = [c[3:5] for c in runner.calls if c[:3] == ["gh", "api", "-X"]]
        # Only the closed branch's PR — never another PR's thread.
        self.assertEqual(
            writes,
            [
                ["PUT", "/notifications/threads/t1/subscription"],
                ["DELETE", "/notifications/threads/t1"],
            ],
        )
        self.assertEqual(
            result["notifications"], "dismissed 1 notification(s), unsubscribed 1"
        )
        first_delete = next(
            i for i, c in enumerate(runner.calls) if c[:3] == ["gh", "api", "-X"]
        )
        remove_at = next(
            i for i, c in enumerate(runner.calls) if c[3:5] == ["worktree", "remove"]
        )
        self.assertLess(first_delete, remove_at)

    def test_a_closed_pr_is_marked_done_and_keeps_its_subscription(self):
        """Unsubscribe only on merge: a closed PR can be reopened."""
        closed = [{"number": 503, "state": "CLOSED", "headRefOid": "abc123"}]
        rc, result, runner = self._close(prs=closed, threads=[("t1", 503)])
        self.assertEqual(rc, 0, result)
        writes = [c[3:5] for c in runner.calls if c[:3] == ["gh", "api", "-X"]]
        self.assertEqual(writes, [["DELETE", "/notifications/threads/t1"]])

    def test_a_mixed_pr_set_unsubscribes_only_the_merged_one(self):
        prs = [
            {"number": 503, "state": "MERGED", "headRefOid": "abc123"},
            {"number": 504, "state": "CLOSED", "headRefOid": "def456"},
        ]
        rc, result, runner = self._close(prs=prs, threads=[("t1", 503), ("t2", 504)])
        self.assertEqual(rc, 0, result)
        writes = [c[3:5] for c in runner.calls if c[:3] == ["gh", "api", "-X"]]
        self.assertEqual(
            writes,
            [
                ["PUT", "/notifications/threads/t1/subscription"],
                ["DELETE", "/notifications/threads/t1"],
                ["DELETE", "/notifications/threads/t2"],
            ],
        )

    def test_a_failed_dismissal_is_a_note_not_a_refusal(self):
        rc, result, _ = self._close(threads=[("t1", 503)], delete_fails=["t1"])
        self.assertEqual(rc, 0, result)
        self.assertTrue(result["removed"])
        self.assertIn(
            "notifications: dismissed 0 notification(s), unsubscribed 1, "
            "1 call(s) failed",
            result["notes"],
        )

    def test_a_failed_ignore_alone_is_still_a_note(self):
        rc, result, _ = self._close(threads=[("t1", 503)], ignore_fails=["t1"])
        self.assertEqual(rc, 0, result)
        self.assertIn(
            "notifications: dismissed 1 notification(s), unsubscribed 0, "
            "1 call(s) failed",
            result["notes"],
        )

    def test_no_pr_reads_no_notifications(self):
        rc, result, runner = self._close(prs=[], threads=[("t1", 503)])
        self.assertEqual(rc, 0, result)
        self.assertFalse(any(c[:2] == ["gh", "api"] for c in runner.calls))
        self.assertIsNone(result["notifications"])

    def test_an_unreadable_list_is_a_note_and_the_removal_still_happens(self):
        class ObjectPage(NotifyingRun):
            # gh exits 0 with an error object rather than a thread list.
            def __call__(self, cmd, cwd=None):
                if cmd[:2] == ["gh", "api"] and "notifications?" in cmd[2]:
                    self.calls.append(cmd)
                    return 0, json.dumps({"message": "Bad credentials"}), ""
                return super().__call__(cmd, cwd)

        runner = ObjectPage()
        rc, result = close(ns(), run=runner, writer=Writer(), pid=1000)
        self.assertEqual(rc, 0, result)
        self.assertTrue(result["removed"])
        self.assertTrue(any("not a thread list" in n for n in result["notes"]))

    def test_a_dismissal_that_raises_is_a_note_not_a_lost_removal(self):
        def boom(*_a, **_k):
            raise AttributeError("'str' object has no attribute 'get'")

        with mock.patch.object(close_session.pr_notifications, "dismiss", boom):
            rc, result, _ = self._close(threads=[("t1", 503)])
        self.assertEqual(rc, 0, result)
        self.assertTrue(result["removed"])
        self.assertIn("dismissal failed: AttributeError", result["notifications"])

    def test_a_dry_run_or_a_refusal_dismisses_nothing(self):
        for args, fake in ((ns(dry_run=True), {}), (ns(), {"status": " M x\n"})):
            _, result, runner = self._close(args, threads=[("t1", 503)], **fake)
            self.assertFalse(any(c[:3] == ["gh", "api", "-X"] for c in runner.calls))
            self.assertIsNone(result["notifications"])


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
