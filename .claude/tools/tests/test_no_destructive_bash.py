#!/usr/bin/env python3
"""Unit tests for the destructive-command guard hook.

**Why this exists as a test module and not only as `--self-test`.** The hook
ships with a built-in self-test, but a hand-invoked flag is not a gate: nothing
ran it, so `make tools-tests` would have stayed green through a regression that
reordered the deny check and the marker read. That is the same
committed-but-unwired shape `make hook-wiring` exists to catch, one level down.

The properties pinned hardest are the two whose failure is silent:

* a comment ends at the **newline**, so a commented first line cannot swallow
  the rest of a multi-line command;
* the **deny** tier is decided before the escape marker is read, so no marker
  can lift it.

Both were live bypasses found by adversarial review of this hook.
"""

from __future__ import annotations

import os
import sys
import unittest

sys.path.insert(
    0,
    os.path.join(
        os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
        "hooks",
    ),
)

import no_destructive_bash as guard  # noqa: E402


def _evaluate(command: str) -> int:
    return guard.evaluate({"tool_name": "Bash", "tool_input": {"command": command}})[0]


class SelfTestTests(unittest.TestCase):
    def test_the_hooks_own_self_test_passes(self):
        # Runs the hook's built-in case table under the suite, so the flag is
        # no longer the only thing that exercises it.
        self.assertEqual(guard._self_test(), 0)


class MultiLineCommentTests(unittest.TestCase):
    """A comment ends at the newline — the bypass this hook shipped with."""

    def test_a_commented_first_line_does_not_hide_a_deny_below_it(self):
        self.assertEqual(_evaluate("ls # check\nrm -rf /"), 2)

    def test_a_commented_first_line_does_not_hide_an_ask_below_it(self):
        self.assertEqual(_evaluate("ls # check\nrm -rf build"), 2)

    def test_a_benign_multi_line_command_is_still_allowed(self):
        self.assertEqual(_evaluate("echo one # note\necho two"), 0)

    def test_a_trailing_line_does_not_defeat_the_deny_end_anchor(self):
        # The deny patterns anchor the target at end-of-line; classifying the
        # whole blob as one string would let an appended line defeat them.
        self.assertEqual(_evaluate("rm -rf /\necho done"), 2)

    def test_a_quoted_hash_is_not_a_comment(self):
        self.assertEqual(_evaluate("echo '# not a comment'\nrm -rf /"), 2)

    def test_split_comments_bounds_each_comment_to_its_line(self):
        effective, comments = guard.split_comments("ls # a\nrm -rf / # b")
        self.assertIn("rm -rf /", effective)
        self.assertIn("# a", comments)
        self.assertIn("# b", comments)


class DenyTierTests(unittest.TestCase):
    """No marker lifts these, and the tier is decided before the marker."""

    def test_the_marker_cannot_lift_a_deny(self):
        self.assertEqual(_evaluate("rm -rf / #destructive-ok"), 2)

    def test_the_marker_cannot_lift_a_deny_on_a_multi_line_command(self):
        self.assertEqual(_evaluate("rm -rf / #destructive-ok\necho done"), 2)

    def test_uppercase_R_is_recursive_on_macos(self):
        # BSD/macOS `rm` takes -R. A case-sensitive `r` left these entirely
        # unclassified — no deny, no ask, no message.
        for command in ("rm -Rf /", "rm -Rf ~", "rm -fR $HOME"):
            with self.subTest(command=command):
                self.assertEqual(guard.classify(command)[0], "deny")

    def test_a_trailing_slash_on_home_is_still_catastrophic(self):
        for command in ("rm -rf ~/", "rm -rf $HOME/", "rm -rf ${HOME}"):
            with self.subTest(command=command):
                self.assertEqual(guard.classify(command)[0], "deny")

    def test_a_force_push_to_the_default_branch_denies_in_either_flag_order(self):
        for command in (
            "git push --force origin main",
            "git push origin main --force",
            "git push origin master -f",
        ):
            with self.subTest(command=command):
                self.assertEqual(guard.classify(command)[0], "deny")

    def test_a_refspec_force_push_carries_no_flag_and_still_denies(self):
        self.assertEqual(guard.classify("git push origin +main:main")[0], "deny")


class AskTierTests(unittest.TestCase):
    def test_the_marker_lifts_an_ask(self):
        self.assertEqual(_evaluate("rm -rf build #destructive-ok"), 0)

    def test_a_quoted_marker_does_not_lift_an_ask(self):
        self.assertEqual(_evaluate("grep '#destructive-ok' log.txt && rm -rf build"), 2)

    def test_a_recursive_delete_outside_the_catastrophic_set_is_ask(self):
        self.assertEqual(guard.classify("rm -rf /tmp/scratch")[0], "ask")


class FalsePositiveTests(unittest.TestCase):
    """A guard that blocks safe commands is a guard that gets turned off.

    That is a security outcome, not a usability one, which is why these are
    pinned as hard as the bypasses.
    """

    def test_git_clean_dry_run_is_not_blocked(self):
        # `-n` is the DRY RUN, and `git clean -ndx` is the recommended preview.
        for command in ("git clean -ndx", "git clean -nx", "git clean --dry-run -x"):
            with self.subTest(command=command):
                self.assertIsNone(guard.classify(command)[0])

    def test_git_clean_that_really_deletes_is_still_blocked(self):
        self.assertEqual(guard.classify("git clean -fdx")[0], "ask")

    def test_destructive_sql_words_in_a_commit_message_are_not_blocked(self):
        # This repo commits with -m constantly, and these words are ordinary
        # English. Un-gated patterns blocked all three.
        for message in (
            'git commit -m "Drop table borders in the report"',
            'git commit -m "Delete from the dictionary the single-file words"',
            'git commit -m "Truncate table headers to two lines"',
        ):
            with self.subTest(message=message):
                self.assertIsNone(guard.classify(message)[0])

    def test_destructive_sql_through_a_real_client_is_still_blocked(self):
        for command in (
            "psql -c 'DROP TABLE ticks'",
            "psql -c 'TRUNCATE TABLE ticks'",
            "psql -c 'DELETE FROM ticks'",
        ):
            with self.subTest(command=command):
                self.assertEqual(guard.classify(command)[0], "ask")

    def test_a_scoped_sql_delete_is_not_blocked(self):
        self.assertIsNone(
            guard.classify("psql -c 'DELETE FROM ticks WHERE ts < now()'")[0]
        )


class ScopeTests(unittest.TestCase):
    def test_a_non_bash_tool_is_none_of_this_hooks_business(self):
        self.assertEqual(
            guard.evaluate(
                {"tool_name": "Write", "tool_input": {"command": "rm -rf /"}}
            )[0],
            0,
        )

    def test_a_malformed_payload_fails_open(self):
        # A guard that wedges the session on bad input is worse than one that
        # misses a command.
        for payload in (
            None,
            [],
            {"tool_name": "Bash"},
            {"tool_name": "Bash", "tool_input": None},
        ):
            with self.subTest(payload=payload):
                self.assertEqual(guard.evaluate(payload)[0], 0)

    def test_an_empty_command_is_allowed(self):
        self.assertEqual(_evaluate("   "), 0)


if __name__ == "__main__":
    unittest.main()
