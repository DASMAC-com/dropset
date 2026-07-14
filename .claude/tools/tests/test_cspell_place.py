"""Stdlib ``unittest`` tests for the cspell word-placement helper.

Run via the repo's ``make tools-tests`` (discovery adds ``.claude/tools`` as
the top-level dir so the bare ``import cspell_place`` below resolves).
"""

import os
import tempfile
import unittest
from pathlib import Path

from cspell_place import (
    comment_style,
    count_word_files,
    tokenize,
    verdict,
)


class TokenizeTests(unittest.TestCase):
    def test_letter_runs_and_camel_splits(self):
        tokens = tokenize("the fooBar snake_case FooBAZ")
        self.assertIn("foo", tokens)
        self.assertIn("bar", tokens)
        self.assertIn("snake", tokens)
        self.assertIn("case", tokens)
        self.assertIn("baz", tokens)
        # the whole letter run is kept too, alongside its camel parts
        self.assertIn("foobar", tokens)

    def test_case_insensitive(self):
        self.assertIn("audc", tokenize("AUDC is a token"))


class CommentStyleTests(unittest.TestCase):
    def test_line_and_block_styles(self):
        self.assertEqual(comment_style("a.rs"), "// cspell:word {word}")
        self.assertEqual(comment_style("a.md"), "<!-- cspell:word {word} -->")
        self.assertEqual(comment_style("cfg/x.yml"), "# cspell:word {word}")
        self.assertEqual(comment_style("a.py"), "# cspell:word {word}")

    def test_json_has_no_comment(self):
        self.assertIsNone(comment_style("keys/x.json"))

    def test_unknown_ext_is_none(self):
        self.assertIsNone(comment_style("a.bin"))


class CountTests(unittest.TestCase):
    def test_counts_files_and_excludes_dictionary(self):
        with tempfile.TemporaryDirectory() as d:
            f1 = os.path.join(d, "a.rs")
            f2 = os.path.join(d, "b.md")
            dic = os.path.join(d, "dictionary.txt")
            Path(f1).write_text("let audc = 1;", encoding="utf-8")
            Path(f2).write_text("audc and eurc", encoding="utf-8")
            # the dictionary lists the word but must not count as a usage
            Path(dic).write_text("audc\n", encoding="utf-8")
            hits = count_word_files(
                ["audc", "eurc"], [f1, f2, dic], dictionary_path=dic
            )
            self.assertEqual(set(hits["audc"]), {f1, f2})
            self.assertEqual(set(hits["eurc"]), {f2})


class VerdictTests(unittest.TestCase):
    def test_two_files_go_to_dictionary(self):
        out = verdict("audc", ["a.rs", "b.md"], [], set())
        self.assertEqual(out["placement"], "dictionary")
        self.assertEqual(out["target"], "cfg/dictionary.txt")

    def test_one_file_goes_inline_with_style(self):
        out = verdict("borsh", ["src/router.rs"], [], set())
        self.assertEqual(out["placement"], "inline")
        self.assertEqual(out["target"], "src/router.rs")
        self.assertEqual(out["directive"], "// cspell:word borsh")

    def test_one_json_file_falls_back_to_dictionary(self):
        out = verdict("keypair", ["keys/mint.json"], [], set())
        self.assertEqual(out["placement"], "dictionary")
        self.assertIn("can't carry a comment", out["reason"])

    def test_already_in_dictionary(self):
        out = verdict("audc", ["a.rs"], [], {"audc"})
        self.assertEqual(out["placement"], "already-in-dictionary")

    def test_zero_hits_falls_back_to_changed_file(self):
        out = verdict("newword", [], ["docs/x.md"], set())
        self.assertEqual(out["placement"], "inline")
        self.assertEqual(out["directive"], "<!-- cspell:word newword -->")


if __name__ == "__main__":
    unittest.main()
