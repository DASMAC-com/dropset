#!/usr/bin/env python3
"""Unit tests for the guard hooks' shared shell tokenizer.

The private-key guard's case table pins this parse end to end; these pin it
directly, one stage at a time, so a regression names the stage that broke
rather than a guard verdict that moved. Each quoting shape here is one an
adversarial review round found misread before the parse was shared.
"""

from __future__ import annotations

import os
import sys
import unittest

HOOKS = os.path.join(
    os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
    "hooks",
)
sys.path.insert(0, HOOKS)

import _shell_tokens as sh  # noqa: E402
import no_git_grep  # noqa: E402
import no_private_key_access  # noqa: E402


def _tokens(text):
    return sh.tokenize(sh.raw_pass(text)[0])


class RawPassTests(unittest.TestCase):
    def test_a_comment_is_dropped_only_at_a_word_start(self):
        self.assertEqual(sh.raw_pass("ls # op read x")[0].strip(), "ls")
        self.assertIn("a#b", sh.raw_pass("echo a#b")[0])
        self.assertIn("'# kept'", sh.raw_pass("echo '# kept'")[0])

    def test_ansi_c_quoting_is_decoded(self):
        self.assertEqual(_tokens("$'\\x6fp' read"), ["op", "read"])
        self.assertEqual(_tokens("echo $'it\\'s'"), ["echo", "it's"])

    def test_the_pid_parameter_is_not_ansi_c(self):
        _, _, uncertain = sh.raw_pass("echo $$'x'")
        self.assertFalse(uncertain)

    def test_live_substitutions_are_collected_literal_ones_are_not(self):
        self.assertEqual(sh.raw_pass("echo $(op read x)")[1], ["op read x"])
        self.assertEqual(sh.raw_pass('echo "`op read x`"')[1], ["op read x"])
        self.assertEqual(sh.raw_pass("echo '$(op read x)'")[1], [])
        self.assertEqual(sh.raw_pass("echo $(a $(b) c)")[1], ["a $(b) c"])

    def test_a_close_paren_inside_quotes_does_not_end_the_body(self):
        self.assertEqual(sh.raw_pass("x $(echo ')' ; op)")[1], ["echo ')' ; op"])

    def test_shapes_the_pass_does_not_model_are_uncertain(self):
        for text in ("cat <<EOF\nx\nEOF", "echo 'open", "echo $(open", "echo `open"):
            with self.subTest(text=text):
                self.assertTrue(sh.raw_pass(text)[2])
        self.assertFalse(sh.raw_pass("echo 'a << b'")[2])


class TokenizeTests(unittest.TestCase):
    def test_an_unquoted_newline_is_its_own_token(self):
        self.assertEqual(_tokens("ls\nop"), ["ls", "\n", "op"])

    def test_a_quoted_string_spanning_lines_stays_one_word(self):
        self.assertEqual(
            _tokens("git commit -m 'a\nb'"), ["git", "commit", "-m", "a\nb"]
        )

    def test_glued_operators_split_into_their_parts(self):
        self.assertEqual(_tokens("a;(b)"), ["a", ";", "(", "b", ")"])
        self.assertEqual(_tokens("a&&(b"), ["a", "&&", "(", "b"])
        self.assertEqual(_tokens("a;>f"), ["a", ";", ">", "f"])

    def test_locale_quoting_reads_as_a_plain_double_quote(self):
        self.assertEqual(_tokens('$"op" read'), ["op", "read"])

    def test_an_unbalanced_line_is_split_into_plain_words(self):
        tokens = _tokens("echo 'odd\nop;read")
        self.assertIn("op", tokens)
        self.assertIn(";", tokens)


class SegmentsTests(unittest.TestCase):
    def test_each_command_carries_the_operator_that_ended_the_last(self):
        self.assertEqual(
            sh.segments(_tokens("a | b && c\nd")),
            [(["a"], None), (["b"], "|"), (["c"], "&&"), (["d"], "\n")],
        )


class UnwrapTests(unittest.TestCase):
    def unwrap(self, text):
        return sh.unwrap(_tokens(text))

    def test_wrappers_assignments_and_redirects_are_stripped(self):
        argv, _, assigns, _ = self.unwrap(
            "X=1 env -u Y Z=2 sudo -u root nice -n 5 op read > out"
        )
        self.assertEqual(argv, ["op", "read"])
        self.assertEqual(assigns, ["X=1", "Z=2"])

    def test_a_shell_payload_is_nested(self):
        self.assertEqual(self.unwrap("bash -o pipefail -c 'op read'")[1], ["op read"])

    def test_a_shell_with_no_command_reads_stdin_and_its_here_string(self):
        argv, nested, _, reads_stdin = self.unwrap("bash <<< 'op read'")
        self.assertEqual(argv, ["bash"])
        self.assertTrue(reads_stdin)
        self.assertEqual(nested, ["op read"])

    def test_eval_watch_find_and_env_split_string_are_nested(self):
        self.assertEqual(self.unwrap("eval op read")[1], ["op read"])
        self.assertEqual(self.unwrap("watch -n 2 op read")[1], ["op read"])
        self.assertEqual(
            self.unwrap("find . -exec op read {} +")[1], ["op read '{}' +"]
        )
        self.assertEqual(self.unwrap("env -S 'op read'")[1], ["op read"])

    def test_command_v_runs_nothing(self):
        self.assertEqual(self.unwrap("command -v op")[0], [])
        self.assertEqual(self.unwrap("command op read")[0], ["op", "read"])

    def test_name_setters_report_the_names_they_set(self):
        self.assertIn("SSH_AUTH_SOCK=", self.unwrap("printf -v SSH_AUTH_SOCK %s x")[2])
        self.assertIn("V=", self.unwrap("read -r V")[2])
        self.assertNotIn("-r=", self.unwrap("read -r V")[2])
        self.assertIn("A=1", self.unwrap("export A=1 B")[2])


class SharedModuleTests(unittest.TestCase):
    def test_the_migrated_guards_import_this_one_module(self):
        """The point of the extraction: one parse, not a copy per guard."""
        self.assertIs(no_private_key_access.sh, sh)
        self.assertIs(no_git_grep.sh, sh)


if __name__ == "__main__":
    unittest.main()
