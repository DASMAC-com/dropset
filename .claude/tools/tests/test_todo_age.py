"""Tests for the Todo age bump: the rule in ``todo_age`` and ``board_batch.py
aged``, which reads the board and writes the set.

The CLI tests patch ``board_batch._post``, the one network seam, and pin the
clock by building histories relative to the real ``now``.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import board_batch as bb  # noqa: E402
import todo_age  # noqa: E402

NOW = datetime(2026, 11, 24, 12, 0, tzinfo=timezone.utc)
TODO_ID = "todo-state"
BACKLOG_ID = "backlog-state"


def _at(days_ago: float, now: datetime = NOW) -> str:
    return (now - timedelta(days=days_ago)).isoformat().replace("+00:00", "Z")


def entered_todo(days_ago: float, now: datetime = NOW) -> dict:
    return {
        "createdAt": _at(days_ago, now),
        "fromPriority": None,
        "toPriority": None,
        "fromStateId": BACKLOG_ID,
        "toStateId": TODO_ID,
        "toState": {"name": "Todo"},
    }


def left_todo(days_ago: float, now: datetime = NOW) -> dict:
    return {
        "createdAt": _at(days_ago, now),
        "fromPriority": None,
        "toPriority": None,
        "fromStateId": TODO_ID,
        "toStateId": "in-progress",
        "toState": {"name": "In Progress"},
    }


def priority_moved(days_ago: float, frm, to, now: datetime = NOW) -> dict:
    return {
        "createdAt": _at(days_ago, now),
        "fromPriority": frm,
        "toPriority": to,
        "fromStateId": None,
        "toStateId": None,
        "toState": None,
    }


def issue(
    number: int,
    priority: int = 4,
    history=None,
    *,
    created: float = 60,
    truncated: bool = False,
    title: str = "Some task",
    now: datetime = NOW,
    **extra,
) -> dict:
    node = {
        "id": f"uuid-{number}",
        "identifier": f"ENG-{number}",
        "number": number,
        "title": title,
        "priority": priority,
        "createdAt": _at(created, now),
        "team": {"id": "team"},
        "history": {
            "pageInfo": {"hasNextPage": truncated},
            "nodes": list(history or []),
        },
    }
    node.update(extra)
    return node


def judge(node: dict, days: int = 14, blocked: bool = False) -> todo_age.Aged:
    return todo_age.judge(
        node,
        days=days,
        now=NOW,
        blocked=blocked,
        tokens=bb.title_token(node["title"]),
    )


class ThresholdTests(unittest.TestCase):
    def _manifest(self, default) -> Path:
        path = Path(tempfile.mkdtemp()) / "manifest.json"
        rows = [{"name": todo_age.THRESHOLD_VAR, "default": default}]
        path.write_text(json.dumps({"variables": rows}), encoding="utf-8")
        return path

    def test_environment_wins_over_the_manifest(self):
        days, why = todo_age.resolve_threshold(
            {todo_age.THRESHOLD_VAR: "9"}, self._manifest("14")
        )
        self.assertEqual((days, why), (9, None))

    def test_unset_falls_back_to_the_manifest_default(self):
        self.assertEqual(todo_age.resolve_threshold({}, self._manifest("14"))[0], 14)

    def test_the_committed_manifest_declares_a_default(self):
        """The real row resolves, so an unset knob still bumps."""
        days, why = todo_age.resolve_threshold({})
        self.assertIsNone(why)
        self.assertGreater(days, 0)

    def test_unset_with_no_default_bumps_nothing_and_says_why(self):
        for manifest in (self._manifest(None), Path("/nonexistent/manifest.json")):
            with self.subTest(manifest=manifest):
                days, why = todo_age.resolve_threshold({}, manifest)
                self.assertIsNone(days)
                self.assertIn("unset", why)
                self.assertNotIn("\n", why)

    def test_a_value_that_is_not_a_positive_whole_number_is_refused(self):
        for raw in ("0", "-3", "1.5", "two", "14d"):
            with self.subTest(raw=raw):
                days, why = todo_age.resolve_threshold(
                    {todo_age.THRESHOLD_VAR: raw}, self._manifest("14")
                )
                self.assertIsNone(days)
                self.assertIn("not a positive whole number", why)

    def test_a_bad_manifest_default_is_refused_too(self):
        days, why = todo_age.resolve_threshold({}, self._manifest("soon"))
        self.assertIsNone(days)
        self.assertIn("manifest default", why)


class ClockTests(unittest.TestCase):
    def test_the_clock_runs_from_the_todo_entry(self):
        start, why = todo_age.clock_start(issue(1, history=[entered_todo(20)]))
        self.assertEqual((NOW - start).days, 20)
        self.assertEqual(why, "entered Todo")

    def test_a_later_priority_change_restarts_the_clock(self):
        node = issue(1, history=[priority_moved(3, 3, 4), entered_todo(20)])
        start, why = todo_age.clock_start(node)
        self.assertEqual((NOW - start).days, 3)
        self.assertEqual(why, "priority changed")

    def test_an_earlier_priority_change_does_not(self):
        node = issue(1, history=[entered_todo(20), priority_moved(30, 3, 4)])
        self.assertEqual((NOW - todo_age.clock_start(node)[0]).days, 20)

    def test_a_reentry_runs_the_clock_from_the_last_entry(self):
        history = [entered_todo(40), left_todo(30), entered_todo(5)]
        start, _ = todo_age.clock_start(issue(1, history=history))
        self.assertEqual((NOW - start).days, 5)

    def test_history_order_is_not_trusted(self):
        """Linear returns newest first today; the clock takes the max."""
        history = [entered_todo(5), left_todo(30), entered_todo(40)]
        start, _ = todo_age.clock_start(issue(1, history=history))
        self.assertEqual((NOW - start).days, 5)

    def test_an_issue_created_in_todo_dates_from_creation(self):
        """Creation records no state entry; a complete history with none
        means the issue was created in Todo."""
        start, why = todo_age.clock_start(issue(1, history=[], created=17))
        self.assertEqual((NOW - start).days, 17)
        self.assertEqual(why, "created in Todo")

    def test_a_non_priority_entry_is_not_a_priority_change(self):
        node = issue(1, history=[priority_moved(2, None, None), entered_todo(20)])
        self.assertEqual((NOW - todo_age.clock_start(node)[0]).days, 20)

    def test_a_truncated_history_is_age_unknown(self):
        node = issue(1, history=[entered_todo(40)], truncated=True)
        self.assertEqual(todo_age.clock_start(node), (None, "history truncated"))

    def test_an_unread_history_is_age_unknown(self):
        for history in (None, {"nodes": None}, "error"):
            with self.subTest(history=history):
                node = issue(1)
                node["history"] = history
                self.assertEqual(todo_age.clock_start(node), (None, "history unread"))

    def test_an_undated_entry_is_age_unknown(self):
        bad = entered_todo(40)
        bad["createdAt"] = None
        self.assertIsNone(todo_age.clock_start(issue(1, history=[bad]))[0])


class JudgeTests(unittest.TestCase):
    def test_a_stale_low_item_is_raised_one_level(self):
        verdict = judge(issue(1, 4, [entered_todo(15)]))
        self.assertEqual((verdict.proposed, verdict.age_days), (3, 15))

    def test_medium_goes_to_high_and_no_priority_to_low(self):
        self.assertEqual(judge(issue(1, 3, [entered_todo(15)])).proposed, 2)
        self.assertEqual(judge(issue(1, 0, [entered_todo(15)])).proposed, 4)

    def test_the_threshold_is_inclusive(self):
        self.assertEqual(judge(issue(1, 4, [entered_todo(14)])).proposed, 3)
        under = judge(issue(1, 4, [entered_todo(13.9)]))
        self.assertIsNone(under.proposed)
        self.assertTrue(under.note.startswith("under threshold"))

    def test_high_and_urgent_are_never_raised(self):
        for priority in (1, 2):
            with self.subTest(priority=priority):
                verdict = judge(issue(1, priority, [entered_todo(90)]))
                self.assertIsNone(verdict.proposed)
                self.assertIn("already high or urgent", verdict.note)

    def test_a_blocked_item_is_skipped(self):
        verdict = judge(issue(1, 4, [entered_todo(90)]), blocked=True)
        self.assertIsNone(verdict.proposed)
        self.assertEqual(verdict.note, "skipped: blocked")

    def test_an_operator_item_is_skipped(self):
        node = issue(1, 4, [entered_todo(90)], title="[Operator] Rotate the key")
        verdict = judge(node)
        self.assertIsNone(verdict.proposed)
        self.assertEqual(verdict.note, "skipped: operator item")

    def test_an_unknown_age_is_never_bumped(self):
        verdict = judge(issue(1, 4, [entered_todo(90)], truncated=True))
        self.assertIsNone(verdict.proposed)
        self.assertTrue(verdict.note.startswith("age unknown"))

    def test_a_lowered_item_is_not_exempt(self):
        """The decrease restarts the clock; once it passes again, it rises."""
        lowered = [priority_moved(5, 3, 4), entered_todo(40)]
        self.assertIsNone(judge(issue(1, 4, lowered)).proposed)
        lowered_long_ago = [priority_moved(15, 3, 4), entered_todo(40)]
        self.assertEqual(judge(issue(1, 4, lowered_long_ago)).proposed, 3)


class AgedCliTests(unittest.TestCase):
    """``board_batch.py aged`` end to end, against a faked transport."""

    def setUp(self):
        env = {"LINEAR_API_KEY": "k", "LINEAR_PROJECT_ID": "p"}
        patcher = mock.patch.dict(os.environ, env)
        patcher.start()
        self.addCleanup(patcher.stop)
        os.environ.pop(todo_age.THRESHOLD_VAR, None)
        self.now = datetime.now(timezone.utc)

    def _run(self, board, *argv, writes=None, reads=None):
        def fake_post(api_key, query, variables):
            if "history(" in query:
                if reads is not None:
                    reads.append(variables)
                return {"issues": {"nodes": board}}
            if writes is not None:
                writes.append(variables)
            return {"issueUpdate": {"success": True}}

        out, err = [], []

        def capture(*args, file=None, **_):
            (err if file is sys.stderr else out).append(" ".join(map(str, args)))

        with (
            mock.patch.object(bb, "_post", side_effect=fake_post),
            mock.patch("builtins.print", side_effect=capture),
        ):
            rc = bb.run(["board_batch.py", "aged", *argv])
        return rc, out, err

    def _board(self):
        n = self.now
        return [
            issue(10, 4, [entered_todo(20, n)], now=n),
            issue(11, 3, [entered_todo(2, n)], now=n),
            issue(12, 4, [entered_todo(30, n)], truncated=True, now=n),
            issue(13, 2, [entered_todo(60, n)], now=n),
            issue(14, 4, [entered_todo(60, n)], title="[Operator] Do it", now=n),
            issue(
                15,
                4,
                [entered_todo(60, n)],
                now=n,
                projectMilestone={"name": "Session pending"},
            ),
        ]

    def test_prints_the_bump_set_and_the_unknowns_writing_nothing(self):
        writes, reads = [], []
        rc, out, err = self._run(self._board(), writes=writes, reads=reads)
        self.assertEqual(rc, 0)
        self.assertEqual(writes, [])
        self.assertEqual(len(reads), 1, "one query reads Todo and its history")
        self.assertEqual(reads[0]["filter"]["state"], {"name": {"eq": "Todo"}})
        self.assertEqual(
            out,
            [
                "BUMP ENG-10 | 20d | low -> medium (entered Todo)",
                "UNKNOWN ENG-12 | age ? | low (age unknown: history truncated)",
            ],
        )
        self.assertIn("1 bump(s), 1 age unknown, 1 under threshold, 2 skipped", err[0])
        self.assertIn("of 5 un-parked Todo", err[0])

    def test_write_applies_the_set_in_one_batch(self):
        writes = []
        rc, out, _ = self._run(self._board(), "--write", writes=writes)
        self.assertEqual(rc, 0)
        self.assertEqual(writes, [{"id": "uuid-10", "input": {"priority": 3}}])
        self.assertIn("SET ENG-10 | priority=3", out)

    def test_write_with_dry_run_writes_nothing(self):
        writes = []
        _, out, _ = self._run(self._board(), "--write", "--dry-run", writes=writes)
        self.assertEqual(writes, [])
        self.assertIn("WOULD SET ENG-10 | priority=3", out)

    def test_a_second_run_after_the_bump_finds_nothing(self):
        """The bump is a priority change, so it restarts the clock."""
        n = self.now
        bumped = [
            issue(10, 3, [priority_moved(0, 4, 3, n), entered_todo(20, n)], now=n)
        ]
        writes = []
        _, out, err = self._run(bumped, "--write", writes=writes)
        self.assertEqual((out, writes), ([], []))
        self.assertIn("0 bump(s)", err[0])

    def test_a_bad_threshold_bumps_nothing_and_reads_nothing(self):
        reads = []
        with mock.patch.dict(os.environ, {todo_age.THRESHOLD_VAR: "fortnight"}):
            rc, out, _ = self._run(self._board(), "--write", reads=reads)
        self.assertEqual(rc, 0)
        self.assertEqual(reads, [])
        self.assertEqual(len(out), 1)
        self.assertIn("not a positive whole number", out[0])


if __name__ == "__main__":
    unittest.main()
