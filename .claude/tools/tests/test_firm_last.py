#!/usr/bin/env python3
"""Unit tests for ``firm_last.py`` (stdlib ``unittest``; no pytest)."""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import firm_last as fl


class ResolveActiveTranscript(unittest.TestCase):
    """The resolution contract. `$CLAUDE_SESSION_ID` used to be consulted here
    and reads as the primary mechanism, but it is never set in a Bash tool
    call — so the branch was unreachable and every firm silently resolved by
    newest-mtime. These assert that the fallback is what actually runs, which
    is the general class no lint rule catches: a tool reading an environment
    variable the harness does not supply."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.home = Path(self._tmp.name)
        self.projects = self.home / "projects"
        self.slug = self.projects / fl.slugify(Path.cwd())
        self.slug.mkdir(parents=True)

    def _transcript(self, name, mtime):
        path = self.slug / f"{name}.jsonl"
        path.write_text("", encoding="utf-8")
        import os as _os

        _os.utime(path, (mtime, mtime))
        return path

    def _resolve(self, session_id=None, env=None):
        with mock.patch.object(fl, "claude_home", return_value=self.home):
            with mock.patch.dict("os.environ", env or {}, clear=False):
                return fl.resolve_active_transcript(session_id)

    def test_an_explicit_session_id_wins(self):
        self._transcript("newest", mtime=9000)
        wanted = self._transcript("wanted", mtime=1000)
        self.assertEqual(self._resolve(session_id="wanted"), wanted)

    def test_without_an_id_the_newest_transcript_is_used(self):
        self._transcript("older", mtime=1000)
        newest = self._transcript("newest", mtime=9000)
        self.assertEqual(self._resolve(), newest)

    def test_the_session_id_env_var_is_not_consulted(self):
        """It is never set in a Bash tool call, so honoring it would be an
        apparent mechanism that never runs. Newest-mtime must win instead."""
        self._transcript("from-env", mtime=1000)
        newest = self._transcript("newest", mtime=9000)
        resolved = self._resolve(env={"CLAUDE_SESSION_ID": "from-env"})
        self.assertEqual(resolved, newest)

    def test_a_blank_explicit_id_falls_back(self):
        newest = self._transcript("newest", mtime=9000)
        self.assertEqual(self._resolve(session_id="   "), newest)

    def test_an_unknown_explicit_id_is_an_error_not_a_silent_fallback(self):
        self._transcript("newest", mtime=9000)
        with self.assertRaises(FileNotFoundError):
            self._resolve(session_id="no-such-session")


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


def _result(tid, content, is_error=False):
    return json.dumps(
        {
            "message": {
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": tid,
                        "content": content,
                        "is_error": is_error,
                    }
                ]
            }
        }
    )


def _allow(root):
    path = root / ".claude" / "settings.local.json"
    if path.is_file():
        return json.loads(path.read_text())["permissions"]["allow"]
    return []


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
            _result(
                "t2",
                "The user doesn't want to proceed with this tool use.",
                is_error=True,
            ),
        ]
        call = fl.most_recent_approved_call(fl.iter_tool_calls(lines))
        self.assertEqual(call["input"]["command"], "git add -A")

    def test_approved_call_with_marker_text_not_skipped(self):
        # An approved command whose *output* contains a denial phrase (is_error
        # false) must not be misclassified as denied.
        lines = [
            _use("t1", "Bash", {"command": "cargo build"}),
            _result("t1", "ok"),
            _use("t2", "Bash", {"command": "grep rejected log"}),
            _result("t2", "hit: the user rejected the change", is_error=False),
        ]
        call = fl.most_recent_approved_call(fl.iter_tool_calls(lines))
        self.assertEqual(call["input"]["command"], "grep rejected log")

    def test_non_firm_skill_is_not_self(self):
        lines = [
            _use("t1", "Skill", {"skill": "commit-changes"}),
            _result("t1", "ok"),
        ]
        call = fl.most_recent_approved_call(fl.iter_tool_calls(lines))
        self.assertEqual(call["name"], "Skill")

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

    def test_firm_into_prunes_subsumed_existing(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "settings.local.json"
            path.write_text(
                json.dumps(
                    {
                        "permissions": {
                            "allow": ["Bash(cargo test -p dropset:*)", "Bash(ls:*)"]
                        }
                    }
                )
            )
            self.assertTrue(fl.firm_into(path, "Bash(cargo test:*)"))
            allow = json.loads(path.read_text())["permissions"]["allow"]
            self.assertIn("Bash(cargo test:*)", allow)
            self.assertNotIn("Bash(cargo test -p dropset:*)", allow)  # pruned
            self.assertIn("Bash(ls:*)", allow)  # untouched


class MainFlow(unittest.TestCase):
    def _run_with(self, lines, argv, with_base=False):
        with tempfile.TemporaryDirectory() as d:
            transcript = Path(d) / "s.jsonl"
            transcript.write_text("\n".join(lines))
            worktree = Path(d) / "wt"
            worktree.mkdir()
            base = Path(d) / "base"
            base.mkdir()
            base_dir = str(base) if with_base else None
            with (
                mock.patch.object(
                    fl, "resolve_active_transcript", return_value=transcript
                ),
                mock.patch.object(fl, "find_base_repo", return_value=base_dir),
                mock.patch.object(Path, "cwd", return_value=worktree),
            ):
                rc = fl.main(argv)
            return rc, _allow(worktree), _allow(base)

    def test_generalized_firm_writes_base(self):
        lines = [
            _use("t1", "Bash", {"command": "cargo test -p dropset"}),
            _result("t1", "ok"),
        ]
        rc, wt, base = self._run_with(lines, [], with_base=True)
        self.assertEqual(rc, 0)
        self.assertIn("Bash(cargo test:*)", base)

    def test_exact_mode_writes_verbatim(self):
        lines = [
            _use("t1", "Bash", {"command": "cargo test -p dropset"}),
            _result("t1", "ok"),
        ]
        rc, wt, base = self._run_with(lines, ["exact"], with_base=True)
        self.assertIn("Bash(cargo test -p dropset:*)", base)

    def test_writes_only_the_base_never_the_worktree(self):
        # settings.local.json resolves through a worktree to the main
        # checkout, so a worktree-local copy would be a file nothing reads.
        lines = [_use("t1", "Bash", {"command": "git add -A"}), _result("t1", "ok")]
        rc, wt, base = self._run_with(lines, [], with_base=True)
        self.assertIn("Bash(git add:*)", base)
        self.assertEqual(wt, [])

    def test_base_only_flag_is_accepted_and_ignored(self):
        lines = [_use("t1", "Bash", {"command": "git add -A"}), _result("t1", "ok")]
        rc, wt, base = self._run_with(lines, ["--base-only"], with_base=True)
        self.assertEqual(rc, 0)
        self.assertEqual(wt, [])
        self.assertIn("Bash(git add:*)", base)

    def test_no_base_repo_firms_nothing_and_exits_non_zero(self):
        """Reporting success on a no-op is how a missed rule goes unnoticed."""
        lines = [_use("t1", "Bash", {"command": "git add -A"}), _result("t1", "ok")]
        rc, wt, base = self._run_with(lines, [], with_base=False)
        self.assertEqual(rc, 1)
        self.assertEqual(wt, [])
        self.assertEqual(base, [])

    def test_bareverb_is_not_written(self):
        lines = [
            _use("t1", "Bash", {"command": "curl https://example.com/x"}),
            _result("t1", "ok"),
        ]
        rc, wt, base = self._run_with(lines, [], with_base=True)
        self.assertEqual(base, [])


if __name__ == "__main__":
    unittest.main()
