"""Tests for ``commit_message.py`` — the skip-by-default message shape."""

from __future__ import annotations

import io
import subprocess
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

import commit_message as cm

#: GitHub skips the run only when the trailer follows TWO empty lines.
TWO_EMPTY = "\n\n\nskip-checks: true\n"


class ShapeTests(unittest.TestCase):
    def test_a_subject_alone_gets_two_empty_lines_then_the_trailer(self):
        self.assertEqual(cm.with_skip_trailer("fix: A\n"), "fix: A" + TWO_EMPTY)

    def test_the_trailer_follows_any_other_trailer_and_is_last(self):
        got = cm.with_skip_trailer("fix: A\n\nWhy it matters.\n\nRefs: x\n")
        self.assertEqual(got, "fix: A\n\nWhy it matters.\n\nRefs: x" + TWO_EMPTY)
        self.assertEqual(got.splitlines()[-1], cm.SKIP_TRAILER)

    def test_one_blank_line_before_the_trailer_is_repaired(self):
        # The PR 771 shape that started all five workflows.
        self.assertEqual(
            cm.with_skip_trailer("fix: A\n\nskip-checks: true\n"), "fix: A" + TWO_EMPTY
        )

    def test_the_rewrite_is_idempotent(self):
        once = cm.with_skip_trailer("fix: A\n\nBody line.  \n\n\n\nMore.\n")
        self.assertEqual(cm.with_skip_trailer(once), once)
        self.assertEqual(once, "fix: A\n\nBody line.\n\nMore." + TWO_EMPTY)

    def test_leading_blank_lines_and_trailing_whitespace_are_stripped(self):
        self.assertEqual(
            cm.with_skip_trailer("\n\n  \nfix: A   \n\n"), "fix: A" + TWO_EMPTY
        )

    def test_a_message_with_no_subject_is_refused(self):
        for text in ("", "\n\n", "skip-checks: true\n"):
            with self.subTest(text=text), self.assertRaises(ValueError):
                cm.with_skip_trailer(text)


class CliTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name) / "msg.txt"

    def run_main(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = cm.main(list(argv))
        return code, out.getvalue(), err.getvalue()

    def test_the_file_is_rewritten_in_place(self):
        self.path.write_text("feat: B\n", encoding="utf-8")
        code, out, _ = self.run_main(str(self.path))
        self.assertEqual(code, 0)
        self.assertTrue(out.startswith(cm.PREFIX))
        self.assertEqual(self.path.read_text(encoding="utf-8"), "feat: B" + TWO_EMPTY)

    def test_a_missing_file_or_empty_message_exits_2(self):
        self.assertEqual(self.run_main(str(self.path))[0], 2)
        self.path.write_text("\n", encoding="utf-8")
        self.assertEqual(self.run_main(str(self.path))[0], 2)


class GitRoundTripTests(unittest.TestCase):
    """The shape survives ``git commit --cleanup=verbatim -F`` byte for byte."""

    def git(self, *args):
        return subprocess.run(
            ["git", "-C", self.repo, *args],
            capture_output=True,
            text=True,
            check=True,
        ).stdout

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.repo = tmp.name
        self.git("init", "-q")
        self.git("config", "user.name", "T")
        self.git("config", "user.email", "t@example.com")
        self.git("config", "commit.gpgsign", "false")
        self.msg = Path(tmp.name) / ".msg"

    def commit(self, *flags):
        self.msg.write_text(cm.with_skip_trailer("fix: A\n\nWhy.\n"), encoding="utf-8")
        self.git("commit", "-q", "--allow-empty", *flags, "-F", str(self.msg))
        return self.git("log", "-1", "--format=%B")

    def test_verbatim_keeps_both_empty_lines(self):
        body = self.commit("--cleanup=verbatim")
        self.assertTrue(body.startswith("fix: A\n\nWhy." + TWO_EMPTY), body)
        trailer = self.git("log", "-1", "--format=%(trailers:key=skip-checks)")
        self.assertEqual(trailer.strip(), cm.SKIP_TRAILER)

    def test_default_cleanup_collapses_them_which_is_why_verbatim_is_required(self):
        body = self.commit()
        self.assertNotIn(TWO_EMPTY, body)


if __name__ == "__main__":
    unittest.main()
