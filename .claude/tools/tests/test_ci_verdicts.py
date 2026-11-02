"""Tests for ``ci_verdicts.py`` — the local-versus-remote verdict records
and their pairing by tree."""

from __future__ import annotations

import io
import os
import subprocess
import tempfile
import unittest
from contextlib import redirect_stderr
from datetime import datetime, timezone
from pathlib import Path

import ci_verdicts as cv


def at(minute: int) -> datetime:
    return datetime(2026, 11, 2, 10, minute, tzinfo=timezone.utc)


def race(minute: int, tree: str, local: str) -> dict:
    return {
        "at": f"2026-11-02T10:{minute:02d}:00",
        "tree": tree,
        "tool": cv.RACE,
        "local": local,
    }


def checks(minute: int, tree: str, conclusion: str, failing=()) -> dict:
    return {
        "at": f"2026-11-02T10:{minute:02d}:00",
        "tree": tree,
        "tool": cv.CHECKS,
        "conclusion": conclusion,
        "failing": list(failing),
    }


SINCE = "2026-11-02T10:00:00"


class Records(unittest.TestCase):
    def test_race_record_reads_the_local_side(self):
        self.assertEqual(
            cv.race_record("GREEN", "a" * 40, {"lint": True})["local"], "green"
        )
        self.assertEqual(
            cv.race_record("RED", "a" * 40, {"test": False})["local"], "red"
        )
        # Settled on CI only, or a local green beside a CI red: nothing to pair.
        self.assertIsNone(cv.race_record("GREEN", "a" * 40, {}))
        self.assertIsNone(cv.race_record("RED", "a" * 40, {"lint": True}))
        self.assertEqual(
            cv.race_record("GREEN", "a" * 40, {"lint": True})["sha"], "a" * 12
        )

    def test_checks_record_only_on_a_settled_pass_or_fail(self):
        rec = cv.checks_record(
            {"pr": 9, "conclusion": "fail", "failing": [{"name": "Tests"}, {"x": 1}]}
        )
        self.assertEqual((rec["conclusion"], rec["failing"]), ("fail", ["Tests"]))
        for conclusion in ("timeout", "pending", "none", "conflicting"):
            self.assertIsNone(cv.checks_record({"conclusion": conclusion}))

    def test_append_then_read_round_trips_within_the_window(self):
        with tempfile.TemporaryDirectory() as tmp:
            state = Path(tmp)
            cv.append(
                cv.race_record("GREEN", "a" * 40, {"lint": True}), "t1", state, at(1)
            )
            cv.append(cv.checks_record({"conclusion": "pass"}), "t1", state, at(5))
            cv.append(cv.checks_record({"conclusion": "pass"}), None, state, at(6))
            cv.append(None, "t1", state, at(7))
            (state / cv.LOG).open("a").write("not json\n")
            records = cv.read(state / cv.LOG, "2026-11-02T10:05:00")
        self.assertEqual([r["tool"] for r in records], [cv.RACE])
        self.assertEqual(records[0]["tree"], "t1")

    def test_a_failed_write_is_one_stderr_line(self):
        with tempfile.TemporaryDirectory() as tmp:
            blocker = Path(tmp) / "file"
            blocker.write_text("x")
            err = io.StringIO()
            with redirect_stderr(err):
                cv.append({"tool": cv.RACE}, "t", blocker, at(0))
        self.assertIn("could not append", err.getvalue())

    def test_a_missing_log_reads_as_no_records(self):
        self.assertEqual(cv.read(Path("/nonexistent/ci-verdicts.jsonl"), SINCE), [])


class Pairing(unittest.TestCase):
    def test_each_kind_by_tree(self):
        records = [
            race(1, "t1", "green"),
            race(2, "t2", "red"),
            race(3, "t3", "green"),
            checks(4, "t1", "fail", ["Tests (sdk)"]),
            checks(5, "t2", "pass"),
            checks(6, "t3", "pass"),
        ]
        self.assertEqual(
            cv.pair(records, SINCE),
            [
                (cv.LOCAL_GREEN_REMOTE_RED, ["Tests (sdk)"]),
                (cv.LOCAL_RED_REMOTE_GREEN, []),
                (cv.AGREED, []),
            ],
        )

    def test_a_race_pairs_once_and_another_tree_never(self):
        records = [
            race(1, "t1", "green"),
            checks(2, "t9", "fail"),
            checks(3, "t1", "pass"),
            checks(4, "t1", "fail"),
        ]
        self.assertEqual(cv.pair(records, SINCE), [(cv.AGREED, [])])

    def test_the_latest_race_on_a_tree_wins(self):
        records = [
            race(1, "t1", "red"),
            race(2, "t1", "green"),
            checks(3, "t1", "fail"),
        ]
        self.assertEqual(cv.pair(records, SINCE), [(cv.LOCAL_GREEN_REMOTE_RED, [])])

    def test_a_race_before_the_window_pairs_and_a_checks_before_it_does_not(self):
        early = [
            {**race(0, "t1", "green"), "at": "2026-11-01T23:00:00"},
            {**race(0, "t2", "green"), "at": "2026-11-01T23:00:00"},
            {**checks(0, "t2", "fail"), "at": "2026-11-01T23:30:00"},
        ]
        records = early + [checks(5, "t1", "fail"), checks(6, "t2", "fail")]
        self.assertEqual(cv.pair(records, SINCE), [(cv.LOCAL_GREEN_REMOTE_RED, [])])


class Tree(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        cwd = os.getcwd()
        self.addCleanup(os.chdir, cwd)
        os.chdir(self.tmp.name)
        for args in (
            ["init", "-q", "-b", "main"],
            ["config", "user.email", "t@t"],
            ["config", "user.name", "t"],
            ["config", "commit.gpgsign", "false"],
        ):
            subprocess.run(["git", *args], check=True)

    def commit(self, *extra: str) -> None:
        subprocess.run(["git", "commit", "-q", "-m", "c", *extra], check=True)

    def test_the_trigger_commit_keeps_the_raced_tree(self):
        Path("a.txt").write_text("a")
        subprocess.run(["git", "add", "a.txt"], check=True)
        self.commit()
        raced = cv.head_tree()
        self.assertRegex(raced or "", r"^[0-9a-f]{40,64}$")
        self.commit("--allow-empty")
        self.assertEqual(cv.head_tree(), raced)
        self.assertEqual(cv.head_tree("HEAD~1"), raced)

    def test_no_commit_is_no_tree(self):
        self.assertIsNone(cv.head_tree())


if __name__ == "__main__":
    unittest.main()
