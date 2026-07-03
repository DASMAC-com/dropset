#!/usr/bin/env python3
"""Unit tests for ``firm_last.py`` (stdlib ``unittest``; no pytest)."""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import firm_last as fl


def _use(tid, name, tool_input):
    return json.dumps(
        {
            "message": {
                "content": [
                    {"type": "tool_use", "id": tid, "name": name, "input": tool_input}
                ]
            }
        }
    )


def _result(tid, content):
    return json.dumps(
        {
            "message": {
                "content": [
                    {"type": "tool_result", "tool_use_id": tid, "content": content}
                ]
            }
        }
    )


class MostRecentApprovedCall(unittest.TestCase):
    def test_picks_last_executed_non_self(self):
        lines = [
            _use("t1", "Bash", {"command": "git add -A"}),
            _result("t1", "ok"),
            _use("t2", "Bash", {"command": "cargo test -p dropset"}),
            _result("t2", "ok"),
            _use("t3", "Skill", {"skill": "f"}),  # the /f invocation, skipped
            _result("t3", "running"),
            _use(
                "t4", "Bash", {"command": "python3 .claude/tools/firm_last.py"}
            ),  # self
        ]
        call = fl.most_recent_approved_call(fl.iter_tool_calls(lines))
        self.assertEqual(call["input"]["command"], "cargo test -p dropset")

    def test_denied_call_is_skipped(self):
        lines = [
            _use("t1", "Bash", {"command": "git add -A"}),
            _result("t1", "ok"),
            _use("t2", "Bash", {"command": "rm -rf /"}),
            _result("t2", "The user doesn't want to proceed with this tool use."),
        ]
        call = fl.most_recent_approved_call(fl.iter_tool_calls(lines))
        self.assertEqual(call["input"]["command"], "git add -A")

    def test_call_without_result_is_skipped(self):
        lines = [
            _use("t1", "Bash", {"command": "git add -A"}),
            _result("t1", "ok"),
            _use("t2", "Bash", {"command": "git status"}),  # no result yet
        ]
        call = fl.most_recent_approved_call(fl.iter_tool_calls(lines))
        self.assertEqual(call["input"]["command"], "git add -A")

    def test_no_firmable_call_returns_none(self):
        lines = [_use("t1", "Skill", {"skill": "f"}), _result("t1", "ok")]
        self.assertIsNone(fl.most_recent_approved_call(fl.iter_tool_calls(lines)))


class SettingsIO(unittest.TestCase):
    def test_round_trip_preserves_other_keys(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "settings.local.json"
            path.write_text(
                json.dumps(
                    {
                        "permissions": {"allow": ["Bash(ls:*)"], "deny": ["x"]},
                        "other": 1,
                    }
                )
            )
            settings, allow = fl.load_settings(path)
            allow.append("Bash(git add:*)")
            fl.write_settings(path, settings, allow)
            reloaded = json.loads(path.read_text())
            self.assertEqual(reloaded["other"], 1)
            self.assertEqual(reloaded["permissions"]["deny"], ["x"])
            self.assertIn("Bash(git add:*)", reloaded["permissions"]["allow"])

    def test_firm_into_is_idempotent(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "settings.local.json"
            self.assertTrue(fl.firm_into(path, "Bash(git add:*)"))
            self.assertFalse(fl.firm_into(path, "Bash(git add:*)"))

    def test_firm_into_skips_when_covered_by_broader(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "settings.local.json"
            path.write_text(json.dumps({"permissions": {"allow": ["Bash(git:*)"]}}))
            self.assertFalse(fl.firm_into(path, "Bash(git status:*)"))

    def test_creates_missing_file(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "nested" / "settings.local.json"
            self.assertTrue(fl.firm_into(path, "Bash(cargo test:*)"))
            self.assertTrue(path.is_file())


class MainFlow(unittest.TestCase):
    def _run_with(self, lines, argv, base_dir=None):
        with tempfile.TemporaryDirectory() as d:
            transcript = Path(d) / "s.jsonl"
            transcript.write_text("\n".join(lines))
            worktree = Path(d) / "wt"
            worktree.mkdir()
            with (
                mock.patch.object(
                    fl, "resolve_active_transcript", return_value=transcript
                ),
                mock.patch.object(fl, "find_base_repo", return_value=base_dir),
                mock.patch.object(Path, "cwd", return_value=worktree),
            ):
                rc = fl.main(argv)
            allow_path = worktree / ".claude" / "settings.local.json"
            allow = []
            if allow_path.is_file():
                allow = json.loads(allow_path.read_text())["permissions"]["allow"]
            return rc, allow

    def test_generalized_firm_writes_worktree(self):
        lines = [
            _use("t1", "Bash", {"command": "cargo test -p dropset"}),
            _result("t1", "ok"),
        ]
        rc, allow = self._run_with(lines, [])
        self.assertEqual(rc, 0)
        self.assertIn("Bash(cargo test:*)", allow)

    def test_exact_mode_writes_verbatim(self):
        lines = [
            _use("t1", "Bash", {"command": "cargo test -p dropset"}),
            _result("t1", "ok"),
        ]
        rc, allow = self._run_with(lines, ["exact"])
        self.assertIn("Bash(cargo test -p dropset:*)", allow)

    def test_base_only_skips_worktree(self):
        lines = [_use("t1", "Bash", {"command": "git add -A"}), _result("t1", "ok")]
        rc, allow = self._run_with(lines, ["--base-only"], base_dir=None)
        self.assertEqual(allow, [])

    def test_bareverb_is_not_written(self):
        lines = [
            _use("t1", "Bash", {"command": "git --no-pager foo"}),
            _result("t1", "ok"),
        ]
        rc, allow = self._run_with(lines, [])
        self.assertEqual(allow, [])


if __name__ == "__main__":
    unittest.main()
