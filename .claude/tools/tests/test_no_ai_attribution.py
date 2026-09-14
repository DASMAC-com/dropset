#!/usr/bin/env python3
"""Unit tests for the AI-attribution guard (stdlib ``unittest``).

The hook's own ``--self-test`` table covers the block/allow decision across the
command shapes, and `test_hook_self_tests.py` runs it. This file covers the two
things that table cannot reach: the quote-aware line splitter that makes a
multi-line message work at all, and the ``--scan`` entry point the skills use
for a PR body created through the GitHub MCP.
"""

from __future__ import annotations

import subprocess
import sys
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

HOOKS = Path(__file__).resolve().parents[2] / "hooks"
GUARD = HOOKS / "no_ai_attribution.py"

sys.path.insert(0, str(HOOKS))

import no_ai_attribution as guard  # noqa: E402

TRAILER = "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
FOOTER = "🤖 Generated with [Claude Code](https://claude.com/claude-code)"


def _blocked(command):
    code, _ = guard.evaluate({"tool_name": "Bash", "tool_input": {"command": command}})
    return code == 2


class LogicalLines(unittest.TestCase):
    """A newline inside quotes is message content, not a command separator.

    This is the distinction that makes the guard work: the sibling guards split
    on every newline, and doing that here tore a quoted multi-line message into
    unbalanced fragments, so `shlex` raised and the guard failed OPEN on exactly
    the shape it exists to catch.
    """

    def test_a_newline_inside_quotes_does_not_split(self):
        self.assertEqual(
            guard._logical_lines('git commit -m "Subject\n\nBody"'),
            ['git commit -m "Subject\n\nBody"'],
        )

    def test_a_newline_outside_quotes_splits(self):
        self.assertEqual(
            guard._logical_lines("git add -A\ngit commit -m 'x'"),
            ["git add -A", "git commit -m 'x'"],
        )

    def test_a_backslash_is_literal_inside_single_quotes(self):
        # The one place shell quoting has no escape, so a trailing backslash
        # must not swallow the closing quote and merge the two commands.
        self.assertEqual(
            guard._logical_lines("echo 'a\\'\ngit status"),
            ["echo 'a\\'", "git status"],
        )

    def test_a_multiline_message_is_still_inspected(self):
        # The regression test for the fail-open bug, at the level of the verdict
        # rather than the splitter.
        self.assertTrue(_blocked(f'git commit -S -m "feat(ENG-1): X\n\n{TRAILER}"'))


class NarrowScope(unittest.TestCase):
    """Only message/body VALUES are scanned, never the command string.

    The repo's own agent material quotes the forbidden strings in order to
    forbid them, so a whole-command scan would block searching for them — the
    false-positive class that gets a guard turned off.
    """

    def test_searching_for_the_trailer_is_not_blocked(self):
        for command in (
            "python3 .claude/tools/search_source.py 'Co-Authored-By'",
            f"grep -rn '{TRAILER}' docs",
            f"rg '{FOOTER}' .claude",
        ):
            with self.subTest(command=command):
                self.assertFalse(_blocked(command))

    def test_a_commit_message_about_the_guard_is_not_blocked(self):
        self.assertFalse(
            _blocked('git commit -m "feat(ENG-1299): Block AI attribution"')
        )

    def test_a_human_co_author_passes(self):
        self.assertFalse(
            _blocked('git commit -m "S\n\nCo-Authored-By: A Person <a@example.com>"')
        )

    def test_a_non_authoring_git_subcommand_is_not_inspected(self):
        self.assertFalse(_blocked(f"git log --grep='{TRAILER}'"))

    def test_authored_text_extracts_only_the_message(self):
        self.assertEqual(
            guard.authored_text('git commit -m "Subject line" --no-verify'),
            ["Subject line"],
        )


class NoEscapeMarker(unittest.TestCase):
    """Deliberately absent, unlike the compound guard's `#compound-ok`."""

    def test_the_destructive_guards_marker_does_not_lift_this(self):
        self.assertTrue(_blocked(f'git commit -m "S\n\n{TRAILER}" #destructive-ok'))

    def test_no_marker_spelling_lifts_it(self):
        for marker in ("#compound-ok", "#attribution-ok", "#ai-ok"):
            with self.subTest(marker=marker):
                self.assertTrue(_blocked(f'git commit -m "S\n\n{TRAILER}" {marker}'))


class ScanMode(unittest.TestCase):
    """The entry point for a PR body, which never passes through Bash."""

    def _scan(self, text):
        with TemporaryDirectory() as tmp:
            path = Path(tmp) / "body.md"
            path.write_text(text, encoding="utf-8")
            return subprocess.run(
                [sys.executable, str(GUARD), "--scan", str(path)],
                capture_output=True,
                text=True,
                timeout=60,
            )

    def test_a_clean_body_exits_zero(self):
        result = self._scan("## Summary\n\nThis PR narrows a guard.\n")
        self.assertEqual(result.returncode, 0)

    def test_a_body_with_the_footer_exits_one_and_names_it(self):
        result = self._scan(f"## Summary\n\nA change.\n\n{FOOTER}\n")
        self.assertEqual(result.returncode, 1)
        self.assertIn("Generated with", result.stderr)

    def test_a_body_with_the_trailer_exits_one(self):
        self.assertEqual(self._scan(f"Body\n\n{TRAILER}\n").returncode, 1)

    def test_a_missing_file_is_a_usage_error_not_a_pass(self):
        # Exit 2, distinct from the exit 1 that means "attribution found", so a
        # caller cannot read a typo'd path as a clean bill of health.
        result = subprocess.run(
            [sys.executable, str(GUARD), "--scan", "/nonexistent/body.md"],
            capture_output=True,
            text=True,
            timeout=60,
        )
        self.assertEqual(result.returncode, 2)


class FailsOpen(unittest.TestCase):
    """A guard must never wedge a session on a shape it cannot parse."""

    def test_an_unbalanced_quote_is_allowed(self):
        self.assertFalse(_blocked('git commit -m "unterminated'))

    def test_a_non_bash_tool_is_untouched(self):
        self.assertEqual(guard.evaluate({"tool_name": "Read", "tool_input": {}})[0], 0)

    def test_a_malformed_payload_is_allowed(self):
        for payload in (None, [], "nope", {}):
            with self.subTest(payload=payload):
                self.assertEqual(guard.evaluate(payload)[0], 0)


if __name__ == "__main__":
    unittest.main()
