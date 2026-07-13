#!/usr/bin/env python3
"""Unit tests for ``firm_core.py`` (stdlib ``unittest``; no pytest)."""

# cspell:word subcommandless

from __future__ import annotations

import unittest

import firm_core as fc


class GeneralizeBash(unittest.TestCase):
    def test_keeps_program_and_subcommand(self):
        self.assertEqual(
            fc.generalize("Bash", {"command": "git add -A"}), "Bash(git add:*)"
        )
        self.assertEqual(
            fc.generalize("Bash", {"command": "cargo test -p dropset"}),
            "Bash(cargo test:*)",
        )
        self.assertEqual(
            fc.generalize("Bash", {"command": "git commit -S -m msg"}),
            "Bash(git commit:*)",
        )

    def test_git_dash_c_path_kept_and_worktree_collapsed(self):
        cmd = "git -C /repo/.claude/worktrees/eng-1 status --short"
        self.assertEqual(
            fc.generalize("Bash", {"command": cmd}),
            "Bash(git -C /repo/.claude/worktrees/* status:*)",
        )

    def test_value_flag_dir_kept(self):
        self.assertEqual(
            fc.generalize("Bash", {"command": "pnpm --dir frontend lint"}),
            "Bash(pnpm --dir frontend lint:*)",
        )

    def test_subcommandless_program_is_ok(self):
        self.assertEqual(
            fc.generalize("Bash", {"command": "ls -la /tmp"}), "Bash(ls:*)"
        )

    def test_compound_and_unfirmable_shapes_return_none(self):
        for cmd in [
            "grep foo bar | head",
            "cd /repo && git status",
            "echo hi > /tmp/x",
            "cat a; cat b",
            "echo $(date)",
            "echo `date`",
            "jq '.a' f.json",
            "cd /somewhere",
            "git status\nrm -rf /tmp",
        ]:
            with self.subTest(cmd=cmd):
                self.assertIsNone(fc.generalize("Bash", {"command": cmd}))

    def test_quoted_operator_is_not_a_compound(self):
        # A `;` inside quotes is text, so the command is still firmable.
        self.assertEqual(
            fc.generalize("Bash", {"command": "git log --grep 'a; b'"}),
            "Bash(git log:*)",
        )

    def test_interpreter_keeps_script_path(self):
        self.assertEqual(
            fc.generalize(
                "Bash", {"command": "python3 .claude/tools/firm_last.py exact"}
            ),
            "Bash(python3 .claude/tools/firm_last.py:*)",
        )
        self.assertEqual(
            fc.generalize("Bash", {"command": "bash scripts/deploy.sh a b"}),
            "Bash(bash scripts/deploy.sh:*)",
        )

    def test_interpreter_inline_or_module_refused(self):
        # A bare interpreter or any leading flag (-c/-m/-e) can't reduce to a
        # rule narrower than the whole interpreter, so it is refused.
        for cmd in [
            "python3 -c 'print(1)'",
            "python3 -m pytest",
            "node -e 'x'",
            "python3",
        ]:
            with self.subTest(cmd=cmd):
                self.assertIsNone(fc.generalize("Bash", {"command": cmd}))

    def test_command_runners_refused(self):
        for cmd in [
            "sudo rm -rf /tmp/x",
            "sudo -v",
            "env FOO=1 cargo test",
            "xargs rm",
            "nohup cargo run",
            "timeout 5 cargo test",
        ]:
            with self.subTest(cmd=cmd):
                self.assertIsNone(fc.generalize("Bash", {"command": cmd}))

    def test_global_flag_kept_before_subcommand(self):
        self.assertEqual(
            fc.generalize("Bash", {"command": "git --no-pager diff --stat"}),
            "Bash(git --no-pager diff:*)",
        )

    def test_exact_mode_keeps_command_verbatim(self):
        self.assertEqual(
            fc.generalize("Bash", {"command": "cargo test -p dropset"}, exact=True),
            "Bash(cargo test -p dropset:*)",
        )

    def test_exact_mode_collapses_worktree(self):
        cmd = "git -C /r/.claude/worktrees/eng-9 diff"
        self.assertEqual(
            fc.generalize("Bash", {"command": cmd}, exact=True),
            "Bash(git -C /r/.claude/worktrees/* diff:*)",
        )


class GeneralizeOtherTools(unittest.TestCase):
    def test_webfetch_domain(self):
        self.assertEqual(
            fc.generalize("WebFetch", {"url": "https://example.com/a/b?q=1"}),
            "WebFetch(domain:example.com)",
        )

    def test_mcp_verbatim(self):
        self.assertEqual(
            fc.generalize("mcp__github__create_pull_request", {"x": 1}),
            "mcp__github__create_pull_request",
        )

    def test_skill_rule(self):
        self.assertEqual(
            fc.generalize("Skill", {"skill": "firm-perms"}), "Skill(firm-perms)"
        )

    def test_read_path_worktree_collapsed(self):
        self.assertEqual(
            fc.generalize("Read", {"file_path": "/r/.claude/worktrees/eng-2/src/a.rs"}),
            "Read(/r/.claude/worktrees/*/src/a.rs)",
        )

    def test_unknown_tool_returns_none(self):
        self.assertIsNone(fc.generalize("TaskCreate", {"subject": "x"}))


class IsCovered(unittest.TestCase):
    def test_exact_match(self):
        self.assertTrue(fc.is_covered("Bash(git add:*)", ["Bash(git add:*)"]))

    def test_bash_prefix_subsumption(self):
        allow = ["Bash(git:*)"]
        self.assertTrue(fc.is_covered("Bash(git status:*)", allow))

    def test_bash_not_covered_by_sibling(self):
        allow = ["Bash(git status:*)"]
        self.assertFalse(fc.is_covered("Bash(git add:*)", allow))

    def test_space_star_equivalent_to_colon_star(self):
        self.assertTrue(fc.is_covered("Bash(git status:*)", ["Bash(git *)"]))

    def test_read_glob_subsumption(self):
        allow = ["Read(/r/.claude/worktrees/**)"]
        self.assertTrue(fc.is_covered("Read(/r/.claude/worktrees/eng-1/a.rs)", allow))

    def test_not_covered_when_absent(self):
        self.assertFalse(fc.is_covered("Bash(cargo test:*)", ["Bash(git add:*)"]))


class BareVerbWildcard(unittest.TestCase):
    def test_flags_dangerous_bare_verbs(self):
        for rule in [
            "Bash(git:*)",
            "Bash(pnpm:*)",
            "Bash(rm:*)",
            "Bash(cargo *)",
            "Bash(curl:*)",
            "Bash(dd:*)",
            "Bash(cp:*)",
            "Bash(chmod:*)",
        ]:
            with self.subTest(rule=rule):
                self.assertTrue(fc.is_bareverb_wildcard(rule))

    def test_allows_subcommandless_and_specific(self):
        for rule in ["Bash(ls:*)", "Bash(git status:*)", "Bash(pwd:*)"]:
            with self.subTest(rule=rule):
                self.assertFalse(fc.is_bareverb_wildcard(rule))

    def test_non_bash_never_flagged(self):
        self.assertFalse(fc.is_bareverb_wildcard("WebFetch(domain:x.com)"))


class CollapseWorktree(unittest.TestCase):
    def test_collapse(self):
        self.assertEqual(
            fc.collapse_worktree_tags("/a/.claude/worktrees/eng-42/b"),
            "/a/.claude/worktrees/*/b",
        )

    def test_no_worktree_unchanged(self):
        self.assertEqual(fc.collapse_worktree_tags("/a/b/c"), "/a/b/c")


if __name__ == "__main__":
    unittest.main()
