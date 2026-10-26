# cspell:word geteuid

"""Tests for hook_wiring.py — the committed-but-unwired guard detector."""

from __future__ import annotations

import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stdout, redirect_stderr
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import hook_wiring as hw  # noqa: E402


def _settings(commands, event="PreToolUse", matcher="Bash", deny=None):
    settings = {
        "hooks": {
            event: [
                {
                    "matcher": matcher,
                    "hooks": [{"type": "command", "command": c} for c in commands],
                }
            ]
        }
    }
    if deny is not None:
        settings["permissions"] = {"deny": list(deny)}
    return settings


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
        # Likewise the iTerm deploy dirs: absent unless a test builds them.
        self.iterm_scripts = self.repo / "home-scripts"
        self.autolaunch = self.repo / "AutoLaunch"
        for name, path in (
            ("ITERM_SCRIPTS_DIR", self.iterm_scripts),
            ("ITERM_AUTOLAUNCH_DIR", self.autolaunch),
        ):
            patcher = mock.patch.object(hw, name, path)
            patcher.start()
            self.addCleanup(patcher.stop)

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

    def test_an_env_assignment_before_the_interpreter_still_counts(self):
        """Token 0 is the assignment, not the program. This reported UNWIRED —
        a false alarm on a working hook, which is the one outcome this module's
        docstring says to avoid.
        """
        self._write(
            "settings.local.json",
            _settings(["DS_DEBUG=1 python3 .claude/hooks/no_git_grep.py"]),
        )
        result = hw.scan(self.repo, self.no_user)
        self.assertIn("no_git_grep.py", result["wired"])
        self.assertNotIn("no_git_grep.py", result["unwired"])

    def test_a_cd_before_the_interpreter_still_counts(self):
        """Separators split a command into segments that are each considered on
        their own; token 0 of the whole string is `cd`.
        """
        self._write(
            "settings.local.json",
            _settings(
                ['cd "$CLAUDE_PROJECT_DIR" && python3 .claude/hooks/no_git_grep.py']
            ),
        )
        result = hw.scan(self.repo, self.no_user)
        self.assertIn("no_git_grep.py", result["wired"])
        self.assertNotIn("no_git_grep.py", result["unwired"])

    def test_a_shell_c_payload_is_wired_not_misdirected(self):
        """The payload is ONE token whose basename is the script, so it matched
        and was returned whole — and `path_status` then could not resolve
        `python3 .claude/hooks/no_git_grep.py` as a path, reporting MISDIRECTED.
        Accusing a working hook of a typo is worse than missing it, because the
        operator goes looking for a defect that is not there.
        """
        self._write(
            "settings.local.json",
            _settings(['sh -c "python3 .claude/hooks/no_git_grep.py"']),
        )
        result = hw.scan(self.repo, self.no_user)
        self.assertIn("no_git_grep.py", result["wired"])
        self.assertEqual(result["misdirected"], {})

    def test_a_mention_inside_a_shell_payload_is_still_a_mention(self):
        """Recursing into a shell payload must not lose the mention rule — the
        recursion re-applies it rather than trusting any basename it finds.
        """
        self._write(
            "settings.local.json",
            _settings(['sh -c "echo skipping no_git_grep.py"']),
        )
        result = hw.scan(self.repo, self.no_user)
        self.assertIn("no_git_grep.py", result["unwired"])
        self.assertEqual(result["wired"], {})


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

    def _edit_guard_settings(self, *specs):
        """One settings payload wiring `worktree_edit_guard.py` several times,
        each entry with its own command and matcher. The precedence branches
        only distinguish themselves across SEPARATE entries, so a fixture that
        can express several is what the adjacency tests need.
        """
        (self.repo / ".claude" / "hooks" / "worktree_edit_guard.py").write_text(
            "", encoding="utf-8"
        )
        entries = []
        for command, matcher in specs:
            payload = _settings([command], matcher=matcher)
            entries.extend(payload["hooks"]["PreToolUse"])
        self._write("settings.local.json", {"hooks": {"PreToolUse": entries}})
        return hw.scan(self.repo, self.no_user)

    def test_a_bad_path_outranks_a_bad_matcher(self):
        """The `misdirected > mismatched` adjacency, which nothing pinned: no
        test produced a script with a bad path from one entry AND a bad matcher
        from another, so those two branches could be swapped with the suite
        staying green.

        A bad path is the more actionable report — that entry points at nothing
        at all, whereas a mismatched one runs and merely never selects.
        """
        result = self._edit_guard_settings(
            (".claude/hooks/typo/worktree_edit_guard.py", "Edit|Write"),
            (".claude/hooks/worktree_edit_guard.py", "Bash"),
        )
        self.assertIn("worktree_edit_guard.py", result["misdirected"])
        self.assertNotIn("worktree_edit_guard.py", result["mismatched"])
        self.assertNotIn("worktree_edit_guard.py", result["unwired"])

    def test_a_good_reference_outranks_a_bad_matcher(self):
        """The `wired > mismatched` adjacency, likewise unpinned. One correct
        entry means the guard fires; a second entry under a matcher that cannot
        select it is redundant, not a defect.
        """
        result = self._edit_guard_settings(
            (".claude/hooks/worktree_edit_guard.py", "Bash"),
            (".claude/hooks/worktree_edit_guard.py", "Edit|Write"),
        )
        self.assertIn("worktree_edit_guard.py", result["wired"])
        self.assertNotIn("worktree_edit_guard.py", result["mismatched"])

    def test_one_entry_with_both_faults_is_reported_by_its_matcher(self):
        """The per-ENTRY ordering, which is the opposite of the aggregate one
        and is deliberate: a matcher that cannot select the tool means the hook
        never runs at all, so the path it names is moot. Recorded because the
        two orderings look contradictory side by side and a future round should
        not "fix" one into agreeing with the other.
        """
        result = self._edit_guard_settings(
            (".claude/hooks/typo/worktree_edit_guard.py", "Bash"),
        )
        self.assertIn("worktree_edit_guard.py", result["mismatched"])
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
            _settings(
                [WIRING, "python3 .claude/hooks/no_git_grep.py"],
                deny=hw.REQUIRED_DENY_RULES,
            ),
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


CONVENTION = (
    Path(__file__).resolve().parents[3] / "docs/conventions/local-integrations.md"
)


class DenyRuleTests(RepoFixture):
    """The private-key deny rules are reported missing like an unwired guard."""

    def test_no_settings_reports_every_rule_missing(self):
        result = hw.scan(self.repo, self.no_user)
        self.assertEqual(result["deny_missing"], list(hw.REQUIRED_DENY_RULES))

    def test_rules_in_any_scope_count(self):
        rules = list(hw.REQUIRED_DENY_RULES)
        self._write("settings.json", {"permissions": {"deny": rules[:3]}})
        self._write("settings.local.json", {"permissions": {"deny": rules[3:]}})
        self.assertEqual(hw.scan(self.repo, self.no_user)["deny_missing"], [])

    def test_user_settings_count(self):
        user = self.repo / "user-settings.json"
        user.write_text(
            json.dumps({"permissions": {"deny": list(hw.REQUIRED_DENY_RULES)}}),
            encoding="utf-8",
        )
        self.assertEqual(hw.scan(self.repo, user)["deny_missing"], [])

    def test_one_missing_rule_is_named(self):
        rules = [r for r in hw.REQUIRED_DENY_RULES if r != "Bash(op:*)"]
        self._write("settings.local.json", {"permissions": {"deny": rules}})
        self.assertEqual(
            hw.scan(self.repo, self.no_user)["deny_missing"], ["Bash(op:*)"]
        )

    def test_the_space_prefix_spelling_counts(self):
        rules = [
            "Bash(op *)" if r == "Bash(op:*)" else r for r in hw.REQUIRED_DENY_RULES
        ]
        self._write("settings.local.json", {"permissions": {"deny": rules}})
        self.assertEqual(hw.scan(self.repo, self.no_user)["deny_missing"], [])

    def test_an_allow_rule_is_not_a_deny_rule(self):
        self._write(
            "settings.local.json",
            {"permissions": {"allow": list(hw.REQUIRED_DENY_RULES)}},
        )
        result = hw.scan(self.repo, self.no_user)
        self.assertEqual(len(result["deny_missing"]), len(hw.REQUIRED_DENY_RULES))

    def test_a_malformed_permissions_key_is_tolerated(self):
        self._write("settings.local.json", {"permissions": {"deny": "Bash(op:*)"}})
        result = hw.scan(self.repo, self.no_user)
        self.assertIn("Bash(op:*)", result["deny_missing"])

    def test_a_missing_rule_exits_one_even_with_every_guard_wired(self):
        self._write(
            "settings.local.json",
            _settings([WIRING, "python3 .claude/hooks/no_git_grep.py"]),
        )
        out = io.StringIO()
        # `run` reads the real user settings by default; once the operator
        # pastes the deny block there, this absence test would go red.
        with (
            mock.patch.object(hw, "USER_SETTINGS", self.no_user),
            redirect_stdout(out),
        ):
            code = hw.run(["hook_wiring.py", "--repo", str(self.repo)])
        self.assertEqual(code, 1)
        self.assertIn("DENY-MISSING  Bash(op:*)", out.getvalue())
        self.assertIn("every committed guard hook is wired", out.getvalue())

    def test_the_private_key_guard_is_expected_on_bash_and_the_edit_tools(self):
        self.assertEqual(
            hw.EXPECTED_TOOLS["no_private_key_access.py"],
            frozenset({"Bash", "Edit", "Write", "MultiEdit"}),
        )

    def test_the_convention_paste_block_lists_exactly_these_rules(self):
        """The doc's block and this tool's table must not drift apart: an
        operator pastes the one, and the upkeep checks the other."""
        text = CONVENTION.read_text(encoding="utf-8")
        section = text.split("## The private-key guard hook", 1)[1]
        section = section.split("\n## ", 1)[0]
        blocks = [
            json.loads(chunk.split("```", 1)[0])
            for chunk in section.split("```json\n")[1:]
        ]
        denies = [b["permissions"]["deny"] for b in blocks if "permissions" in b]
        self.assertEqual(len(denies), 1)
        self.assertEqual(denies[0], list(hw.REQUIRED_DENY_RULES))
        matchers = [
            entry["matcher"]
            for b in blocks
            for entry in b.get("hooks", {}).get("PreToolUse", [])
            if any(
                "no_private_key_access.py" in h.get("command", "")
                for h in entry["hooks"]
            )
        ]
        self.assertEqual(len(matchers), 1)
        self.assertTrue(
            hw.matcher_selects(
                matchers[0], hw.EXPECTED_TOOLS["no_private_key_access.py"]
            )
        )
        for tool in hw.EXPECTED_TOOLS["no_private_key_access.py"]:
            self.assertTrue(hw.matcher_selects(matchers[0], [tool]), tool)


PK_WIRING = 'python3 "$CLAUDE_PROJECT_DIR/.claude/hooks/no_private_key_access.py"'


class PartialWiringTests(RepoFixture):
    """The private-key guard needs Bash AND the edit tools selected."""

    def setUp(self):
        super().setUp()
        (self.repo / ".claude" / "hooks" / "no_private_key_access.py").write_text(
            "", encoding="utf-8"
        )

    def _entries(self, *matchers):
        return {
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": m,
                        "hooks": [{"type": "command", "command": PK_WIRING}],
                    }
                    for m in matchers
                ]
            }
        }

    def test_bash_only_is_mismatched_naming_the_uncovered_tools(self):
        self._write("settings.local.json", self._entries("Bash"))
        result = hw.scan(self.repo, self.no_user)
        self.assertIn("no_private_key_access.py", result["mismatched"])
        self.assertEqual(
            result["uncovered"]["no_private_key_access.py"],
            ["Edit", "MultiEdit", "Write"],
        )
        self.assertIn("that half of it cannot fire", hw.render(result))

    def test_edit_only_is_mismatched(self):
        self._write("settings.local.json", self._entries("Edit|Write|MultiEdit"))
        result = hw.scan(self.repo, self.no_user)
        self.assertEqual(result["uncovered"]["no_private_key_access.py"], ["Bash"])

    def test_the_documented_matcher_is_wired(self):
        self._write("settings.local.json", self._entries("Bash|Edit|Write|MultiEdit"))
        result = hw.scan(self.repo, self.no_user)
        self.assertIn("no_private_key_access.py", result["wired"])
        self.assertEqual(result["uncovered"], {})

    def test_two_entries_that_cover_every_tool_are_wired(self):
        self._write(
            "settings.local.json", self._entries("Bash", "Edit|Write|MultiEdit")
        )
        self.assertIn(
            "no_private_key_access.py", hw.scan(self.repo, self.no_user)["wired"]
        )

    def test_other_guards_still_need_only_one_tool(self):
        self._write("settings.local.json", _settings([WIRING]))
        self.assertIn("no_compound_bash.py", hw.scan(self.repo, self.no_user)["wired"])


class ItermDriftTests(RepoFixture):
    """The deployed iTerm files must stay pure pointers into the checkout."""

    def setUp(self):
        super().setUp()
        self.source = self.repo / ".claude" / "scripts"
        self.source.mkdir()
        for name in ("iterm-paint.sh", "iterm-reorder.py"):
            (self.source / name).write_text("", encoding="utf-8")
        # Not part of the integration, so never expected to be deployed.
        (self.source / "test_iterm_heal.py").write_text("", encoding="utf-8")

    def _deploy(self):
        self.iterm_scripts.mkdir()
        self.autolaunch.mkdir()
        for name in ("iterm-paint.sh", "iterm-reorder.py"):
            (self.iterm_scripts / name).symlink_to(self.source / name)
        (self.autolaunch / "iterm-reorder.py").symlink_to(
            self.source / "iterm-reorder.py"
        )

    def _drift(self):
        return hw.scan(self.repo, self.no_user)["iterm"]

    def test_a_fully_linked_deployment_has_no_drift(self):
        self._deploy()
        self.assertEqual(self._drift(), {"deployed": True, "drift": {}})

    def test_no_deployment_is_not_drift(self):
        self.assertEqual(self._drift(), {"deployed": False, "drift": {}})

    def test_a_copy_is_drift(self):
        """The measured failure: a hand copy that silently ran months stale."""
        self._deploy()
        (self.iterm_scripts / "iterm-paint.sh").unlink()
        (self.iterm_scripts / "iterm-paint.sh").write_text("old", encoding="utf-8")
        self.assertEqual(
            self._drift()["drift"],
            {str(self.iterm_scripts / "iterm-paint.sh"): "stale-copy"},
        )

    def test_a_copy_in_autolaunch_is_drift(self):
        self._deploy()
        (self.autolaunch / "iterm-reorder.py").unlink()
        (self.autolaunch / "iterm-reorder.py").write_text("old", encoding="utf-8")
        self.assertEqual(
            self._drift()["drift"],
            {str(self.autolaunch / "iterm-reorder.py"): "stale-copy"},
        )

    def test_a_newly_committed_script_not_yet_linked_is_drift(self):
        self._deploy()
        (self.source / "iterm-attend.sh").write_text("", encoding="utf-8")
        self.assertEqual(
            self._drift()["drift"],
            {str(self.iterm_scripts / "iterm-attend.sh"): "missing"},
        )

    def test_a_link_to_a_deleted_script_is_dangling(self):
        self._deploy()
        gone = self.repo / "gone" / "iterm-old.sh"
        (self.iterm_scripts / "iterm-old.sh").symlink_to(gone)
        self.assertEqual(
            self._drift()["drift"],
            {str(self.iterm_scripts / "iterm-old.sh"): "dangling-link"},
        )

    def test_an_uncommitted_file_is_an_orphan(self):
        self._deploy()
        (self.iterm_scripts / "iterm-bg-old.sh").write_text("", encoding="utf-8")
        self.assertEqual(
            self._drift()["drift"],
            {str(self.iterm_scripts / "iterm-bg-old.sh"): "orphan"},
        )

    def test_a_link_into_another_checkout_is_drift(self):
        """A link into a worktree vanishes with the worktree."""
        self._deploy()
        other = self.repo / "worktree" / "iterm-paint.sh"
        other.parent.mkdir()
        other.write_text("", encoding="utf-8")
        (self.iterm_scripts / "iterm-paint.sh").unlink()
        (self.iterm_scripts / "iterm-paint.sh").symlink_to(other)
        self.assertEqual(
            self._drift()["drift"],
            {str(self.iterm_scripts / "iterm-paint.sh"): "wrong-link"},
        )

    def test_deleting_the_lone_autolaunch_reorderer_is_drift(self):
        """Emptying AutoLaunch must not read as "never deployed there" — the
        deploy tool links into it whenever it exists, and reports this the
        same way."""
        self._deploy()
        (self.autolaunch / "iterm-reorder.py").unlink()
        self.assertEqual(
            self._drift()["drift"],
            {str(self.autolaunch / "iterm-reorder.py"): "missing"},
        )

    def test_autolaunch_with_only_other_scripts_is_not_a_deployment(self):
        self.autolaunch.mkdir()
        (self.autolaunch / "someone-elses.py").write_text("", encoding="utf-8")
        self.assertEqual(self._drift(), {"deployed": False, "drift": {}})

    def test_a_dangling_link_at_a_committed_name_is_a_wrong_link(self):
        self._deploy()
        (self.iterm_scripts / "iterm-paint.sh").unlink()
        (self.iterm_scripts / "iterm-paint.sh").symlink_to(self.repo / "gone.sh")
        self.assertEqual(
            self._drift()["drift"],
            {str(self.iterm_scripts / "iterm-paint.sh"): "wrong-link"},
        )

    def test_a_deployment_with_no_source_dir_is_an_error_not_clean(self):
        self._deploy()
        for path in self.source.iterdir():
            path.unlink()
        self.source.rmdir()
        with self.assertRaises(hw.HookWiringError):
            hw.scan(self.repo, self.no_user)

    @unittest.skipIf(os.geteuid() == 0, "root reads a mode-000 directory anyway")
    def test_an_unreadable_deploy_dir_is_a_scan_error_not_a_clean_answer(self):
        """Path.glob swallows PermissionError and returns nothing, which would
        read an unreadable deploy dir as "not deployed" and exit 0. AutoLaunch
        is left absent, so nothing else marks the integration deployed."""
        self.iterm_scripts.mkdir()
        (self.iterm_scripts / "iterm-paint.sh").symlink_to(
            self.source / "iterm-paint.sh"
        )
        self.iterm_scripts.chmod(0)
        self.addCleanup(self.iterm_scripts.chmod, 0o755)
        with self.assertRaises(hw.HookWiringError):
            hw.scan(self.repo, self.no_user)

    def test_no_deployment_says_so(self):
        out = io.StringIO()
        with redirect_stdout(out):
            hw.run(["hook_wiring.py", "--repo", str(self.repo)])
        self.assertIn("not deployed on this machine", out.getvalue())

    def test_a_deploy_dir_that_is_itself_a_link_into_the_source_is_clean(self):
        self.iterm_scripts.symlink_to(self.source)
        self.assertEqual(self._drift(), {"deployed": True, "drift": {}})

    def test_drift_exits_one_and_names_the_file(self):
        self._deploy()
        (self.iterm_scripts / "iterm-paint.sh").unlink()
        (self.iterm_scripts / "iterm-paint.sh").write_text("old", encoding="utf-8")
        self._write(
            "settings.local.json",
            _settings([WIRING, "python3 .claude/hooks/no_git_grep.py"]),
        )
        out = io.StringIO()
        with redirect_stdout(out):
            code = hw.run(["hook_wiring.py", "--repo", str(self.repo)])
        self.assertEqual(code, 1)
        self.assertIn("DRIFT", out.getvalue())
        self.assertIn("iterm-paint.sh", out.getvalue())
        self.assertIn("every committed guard hook is wired", out.getvalue())

    def test_a_clean_deployment_exits_zero(self):
        self._deploy()
        self._write(
            "settings.local.json",
            _settings(
                [WIRING, "python3 .claude/hooks/no_git_grep.py"],
                deny=hw.REQUIRED_DENY_RULES,
            ),
        )
        out = io.StringIO()
        with redirect_stdout(out):
            code = hw.run(["hook_wiring.py", "--repo", str(self.repo)])
        self.assertEqual(code, 0)
        self.assertIn(
            "every deployed iTerm file links into the checkout", out.getvalue()
        )

    def test_the_check_writes_nothing(self):
        self._deploy()
        (self.iterm_scripts / "iterm-paint.sh").unlink()
        (self.iterm_scripts / "iterm-paint.sh").write_text("old", encoding="utf-8")
        hw.scan(self.repo, self.no_user)
        self.assertFalse((self.iterm_scripts / "iterm-paint.sh").is_symlink())
        self.assertEqual(
            sorted(p.name for p in self.iterm_scripts.iterdir()),
            ["iterm-paint.sh", "iterm-reorder.py"],
        )


if __name__ == "__main__":
    unittest.main()
