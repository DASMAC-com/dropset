"""Tests for hook_wiring.py — the committed-but-unwired guard detector."""

from __future__ import annotations

import io
import json
import sys
import tempfile
import unittest
from contextlib import redirect_stdout, redirect_stderr
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import hook_wiring as hw  # noqa: E402


def _settings(commands, event="PreToolUse", matcher="Bash"):
    return {
        "hooks": {
            event: [
                {
                    "matcher": matcher,
                    "hooks": [{"type": "command", "command": c} for c in commands],
                }
            ]
        }
    }


WIRING = 'python3 "$CLAUDE_PROJECT_DIR/.claude/hooks/no_compound_bash.py"'


class RepoFixture(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)
        (self.repo / ".claude" / "hooks").mkdir(parents=True)
        (self.repo / ".claude" / "hooks" / "no_compound_bash.py").write_text(
            "", encoding="utf-8"
        )
        (self.repo / ".claude" / "hooks" / "no_git_grep.py").write_text(
            "", encoding="utf-8"
        )
        # A user-settings path that does not exist, so tests never read the
        # developer's real ~/.claude/settings.json and pick up its wiring.
        self.no_user = self.repo / "absent-user-settings.json"

    def _write(self, name, data):
        (self.repo / ".claude" / name).write_text(json.dumps(data), encoding="utf-8")


class ScanTests(RepoFixture):
    def test_a_committed_script_with_no_entry_is_reported_unwired(self):
        result = hw.scan(self.repo, self.no_user)
        self.assertEqual(result["unwired"], ["no_compound_bash.py", "no_git_grep.py"])
        self.assertEqual(result["wired"], {})

    def test_a_wired_script_is_not_reported(self):
        self._write("settings.local.json", _settings([WIRING]))
        result = hw.scan(self.repo, self.no_user)
        self.assertEqual(result["unwired"], ["no_git_grep.py"])
        self.assertEqual(
            result["wired"], {"no_compound_bash.py": [".claude/settings.local.json"]}
        )

    def test_the_live_repo_state_that_prompted_this_tool(self):
        """One of three guards wired, two committed and inert — the 2026-08-14
        finding, and the shape the report has to make obvious."""
        (self.repo / ".claude" / "hooks" / "worktree_edit_guard.py").write_text(
            "", encoding="utf-8"
        )
        self._write("settings.local.json", _settings([WIRING]))
        result = hw.scan(self.repo, self.no_user)
        self.assertEqual(
            result["unwired"], ["no_git_grep.py", "worktree_edit_guard.py"]
        )

    def test_both_repo_settings_files_are_read(self):
        self._write("settings.json", _settings([WIRING]))
        self._write(
            "settings.local.json",
            _settings(["python3 .claude/hooks/no_git_grep.py"]),
        )
        result = hw.scan(self.repo, self.no_user)
        self.assertEqual(result["unwired"], [])
        self.assertEqual(
            result["wired"]["no_compound_bash.py"], [".claude/settings.json"]
        )
        self.assertEqual(
            result["wired"]["no_git_grep.py"], [".claude/settings.local.json"]
        )

    def test_user_settings_count_as_wiring(self):
        user = self.repo / "user-settings.json"
        user.write_text(json.dumps(_settings([WIRING])), encoding="utf-8")
        result = hw.scan(self.repo, user)
        self.assertIn("no_compound_bash.py", result["wired"])

    def test_a_hook_on_another_event_still_counts_as_wired(self):
        """Reporting a PostToolUse hook as unwired would be a false positive,
        and false positives are what make a checker get ignored."""
        self._write("settings.local.json", _settings([WIRING], event="PostToolUse"))
        result = hw.scan(self.repo, self.no_user)
        self.assertNotIn("no_compound_bash.py", result["unwired"])

    def test_a_relative_spelling_of_the_path_counts(self):
        self._write(
            "settings.local.json", _settings(["python3 .claude/hooks/no_git_grep.py"])
        )
        result = hw.scan(self.repo, self.no_user)
        self.assertIn("no_git_grep.py", result["wired"])

    def test_a_missing_settings_file_is_not_an_error(self):
        result = hw.scan(self.repo, self.no_user)
        self.assertEqual(result["scanned_settings"], [])

    def test_a_malformed_settings_file_is_an_error_not_a_clean_scan(self):
        """Reporting "all wired" from a file the tool could not read is the
        silent-clean-result this tool exists to prevent."""
        (self.repo / ".claude" / "settings.local.json").write_text(
            "{not json", encoding="utf-8"
        )
        with self.assertRaises(hw.HookWiringError):
            hw.scan(self.repo, self.no_user)

    def test_a_malformed_hooks_entry_is_skipped_not_raised_on(self):
        """The file is user-authored and schema-less; one odd entry should cost
        a missed match, not a crashed upkeep run."""
        self._write(
            "settings.local.json",
            {"hooks": {"PreToolUse": ["not-a-dict", {"hooks": [{"command": 7}]}]}},
        )
        result = hw.scan(self.repo, self.no_user)
        self.assertEqual(result["unwired"], ["no_compound_bash.py", "no_git_grep.py"])

    def test_hooks_key_of_the_wrong_shape_is_tolerated(self):
        self._write("settings.local.json", {"hooks": []})
        self.assertEqual(hw.scan(self.repo, self.no_user)["wired"], {})

    def test_non_python_files_in_the_hooks_dir_are_ignored(self):
        (self.repo / ".claude" / "hooks" / "README.md").write_text("", encoding="utf-8")
        result = hw.scan(self.repo, self.no_user)
        self.assertNotIn("README.md", result["unwired"])

    def test_a_repo_with_no_hooks_dir_reports_nothing(self):
        with tempfile.TemporaryDirectory() as d:
            result = hw.scan(Path(d), self.no_user)
            self.assertEqual(result["unwired"], [])
            self.assertEqual(result["wired"], {})


class MatcherTests(RepoFixture):
    """The matcher gap: a guard filed under a tool it never sees cannot fire,
    and used to report `wired` — the likeliest operator slip, because each
    guard's documented paste block invites copying the Bash block."""

    def _add_worktree_guard(self):
        (self.repo / ".claude" / "hooks" / "worktree_edit_guard.py").write_text(
            "", encoding="utf-8"
        )

    def test_the_edit_guard_under_a_bash_matcher_is_mismatched(self):
        self._add_worktree_guard()
        self._write(
            "settings.local.json",
            _settings(["python3 .claude/hooks/worktree_edit_guard.py"], matcher="Bash"),
        )
        result = hw.scan(self.repo, self.no_user)
        self.assertIn("worktree_edit_guard.py", result["mismatched"])
        self.assertNotIn("worktree_edit_guard.py", result["wired"])
        self.assertNotIn("worktree_edit_guard.py", result["unwired"])

    def test_the_edit_guard_under_its_real_matcher_is_wired(self):
        self._add_worktree_guard()
        self._write(
            "settings.local.json",
            _settings(
                ["python3 .claude/hooks/worktree_edit_guard.py"],
                matcher="Edit|Write|MultiEdit|NotebookEdit",
            ),
        )
        result = hw.scan(self.repo, self.no_user)
        self.assertIn("worktree_edit_guard.py", result["wired"])
        self.assertEqual(result["mismatched"], {})

    def test_a_star_matcher_selects_everything(self):
        self._add_worktree_guard()
        self._write(
            "settings.local.json",
            _settings(["python3 .claude/hooks/worktree_edit_guard.py"], matcher="*"),
        )
        self.assertIn(
            "worktree_edit_guard.py", hw.scan(self.repo, self.no_user)["wired"]
        )

    def test_an_absent_matcher_selects_everything(self):
        self._add_worktree_guard()
        self._write(
            "settings.local.json",
            {
                "hooks": {
                    "PreToolUse": [
                        {
                            "hooks": [
                                {
                                    "command": (
                                        "python3 .claude/hooks/worktree_edit_guard.py"
                                    )
                                }
                            ]
                        }
                    ]
                }
            },
        )
        self.assertIn(
            "worktree_edit_guard.py", hw.scan(self.repo, self.no_user)["wired"]
        )

    def test_an_unparseable_matcher_does_not_cry_wolf(self):
        """Linting regex syntax is not this tool's job, and a false MISMATCHED
        is the noise that gets a checker ignored."""
        self._add_worktree_guard()
        self._write(
            "settings.local.json",
            _settings(
                ["python3 .claude/hooks/worktree_edit_guard.py"], matcher="Edit["
            ),
        )
        self.assertIn(
            "worktree_edit_guard.py", hw.scan(self.repo, self.no_user)["wired"]
        )

    def test_a_guard_absent_from_the_expected_table_is_unconstrained(self):
        (self.repo / ".claude" / "hooks" / "brand_new_guard.py").write_text(
            "", encoding="utf-8"
        )
        self._write(
            "settings.local.json",
            _settings(["python3 .claude/hooks/brand_new_guard.py"], matcher="Bash"),
        )
        result = hw.scan(self.repo, self.no_user)
        self.assertIn("brand_new_guard.py", result["wired"])
        self.assertEqual(result["mismatched"], {})


class MentionTests(RepoFixture):
    """A script NAMED in an argument is not wiring. The shape that mattered:
    one `echo` mentioning three base names marked all three wired, so a guard
    disabled by commenting it into an echo read as fully protected."""

    def test_an_echo_mentioning_the_base_names_is_not_wiring(self):
        self._write(
            "settings.local.json",
            _settings(["echo skipping no_compound_bash.py no_git_grep.py"]),
        )
        result = hw.scan(self.repo, self.no_user)
        self.assertEqual(result["unwired"], ["no_compound_bash.py", "no_git_grep.py"])
        self.assertEqual(result["wired"], {})

    def test_the_script_as_the_command_itself_counts(self):
        self._write(
            "settings.local.json",
            _settings([".claude/hooks/no_git_grep.py"]),
        )
        self.assertIn("no_git_grep.py", hw.scan(self.repo, self.no_user)["wired"])

    def test_an_interpreter_flag_before_the_script_still_counts(self):
        self._write(
            "settings.local.json",
            _settings(["python3 -u .claude/hooks/no_git_grep.py"]),
        )
        self.assertIn("no_git_grep.py", hw.scan(self.repo, self.no_user)["wired"])


class MisdirectedTests(RepoFixture):
    """A command pointing at a path that does not exist is inert, and used to
    report `wired`."""

    def test_a_mistyped_path_for_a_committed_guard_is_misdirected(self):
        self._write(
            "settings.local.json",
            _settings(["python3 .claude/hooks/typo/no_compound_bash.py"]),
        )
        result = hw.scan(self.repo, self.no_user)
        self.assertIn("no_compound_bash.py", result["misdirected"])
        self.assertNotIn("no_compound_bash.py", result["wired"])

    def test_the_documented_project_dir_spelling_resolves(self):
        self._write("settings.local.json", _settings([WIRING]))
        self.assertIn("no_compound_bash.py", hw.scan(self.repo, self.no_user)["wired"])

    def test_an_unresolvable_other_variable_is_given_the_benefit_of_the_doubt(self):
        self._write(
            "settings.local.json",
            _settings(["python3 $MY_HOOKS/no_compound_bash.py"]),
        )
        result = hw.scan(self.repo, self.no_user)
        self.assertIn("no_compound_bash.py", result["wired"])
        self.assertEqual(result["misdirected"], {})

    def test_one_good_reference_outweighs_a_broken_one(self):
        self._write(
            "settings.local.json",
            _settings(["python3 .claude/hooks/typo/no_compound_bash.py", WIRING]),
        )
        result = hw.scan(self.repo, self.no_user)
        self.assertIn("no_compound_bash.py", result["wired"])
        self.assertEqual(result["misdirected"], {})

    def test_a_mismatched_guard_exits_one(self):
        (self.repo / ".claude" / "hooks" / "worktree_edit_guard.py").write_text(
            "", encoding="utf-8"
        )
        self._write(
            "settings.local.json",
            _settings(
                [
                    WIRING,
                    "python3 .claude/hooks/no_git_grep.py",
                    "python3 .claude/hooks/worktree_edit_guard.py",
                ],
                matcher="Bash",
            ),
        )
        out = io.StringIO()
        with redirect_stdout(out):
            code = hw.run(["hook_wiring.py", "--repo", str(self.repo)])
        self.assertEqual(code, 1)
        self.assertIn("MISMATCHED", out.getvalue())


class CliTests(RepoFixture):
    def _capture(self, argv):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = hw.run(argv)
        return code, out.getvalue(), err.getvalue()

    def test_exits_one_when_a_guard_is_unwired(self):
        code, out, _ = self._capture(["hook_wiring.py", "--repo", str(self.repo)])
        self.assertEqual(code, 1)
        self.assertIn("UNWIRED", out)
        self.assertIn("no_git_grep.py", out)

    def test_exits_zero_when_everything_is_wired(self):
        self._write(
            "settings.local.json",
            _settings([WIRING, "python3 .claude/hooks/no_git_grep.py"]),
        )
        code, out, _ = self._capture(["hook_wiring.py", "--repo", str(self.repo)])
        self.assertEqual(code, 0)
        self.assertIn("every committed guard hook is wired", out)
        self.assertNotIn("UNWIRED", out)

    def test_the_report_names_the_script_not_a_settings_diff(self):
        code, out, _ = self._capture(["hook_wiring.py", "--repo", str(self.repo)])
        self.assertIn("no_compound_bash.py", out)
        # The wiring block itself must not be echoed: the operator decides, and
        # a diff would read as a change proposal rather than a report.
        self.assertNotIn("CLAUDE_PROJECT_DIR", out)

    def test_the_report_says_nothing_was_written(self):
        _, out, _ = self._capture(["hook_wiring.py", "--repo", str(self.repo)])
        self.assertIn("nothing was written", out)

    def test_json_mode_is_machine_readable(self):
        code, out, _ = self._capture(
            ["hook_wiring.py", "--repo", str(self.repo), "--json"]
        )
        self.assertEqual(code, 1)
        parsed = json.loads(out)
        self.assertEqual(parsed["unwired"], ["no_compound_bash.py", "no_git_grep.py"])

    def test_finding_no_hooks_at_all_is_not_reported_as_clean(self):
        """A repo with no .claude/hooks/ has nothing to say — and returning 0
        for it would be byte-identical, in the one signal a caller branches on,
        to "every guard is wired". That is the conflation this tool exists to
        prevent, so it must not reproduce it."""
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(hw.HookWiringError):
                self._capture(["hook_wiring.py", "--repo", d])

    def test_a_bad_repo_errors_rather_than_reporting_clean(self):
        with self.assertRaises(hw.HookWiringError):
            self._capture(["hook_wiring.py", "--repo", str(self.repo / "nope")])

    def test_main_maps_a_scan_failure_to_exit_two(self):
        """0 clean / 1 unwired / 2 broken — a clean scan and a broken one must
        never look alike."""
        argv = ["hook_wiring.py", "--repo", str(self.repo / "nope")]
        err = io.StringIO()
        with redirect_stderr(err):
            original, sys.argv = sys.argv, argv
            try:
                code = hw.main()
            finally:
                sys.argv = original
        self.assertEqual(code, 2)
        self.assertIn("hook-wiring:", err.getvalue())


if __name__ == "__main__":
    unittest.main()
