"""Tests for rewrap.py, over an embedded markdown and YAML fixture."""

from __future__ import annotations

import io
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

import rewrap

#: A paragraph an edit lengthened, so its first line overflows; a list item
#: whose overflow would put a bare `-` at a line start; a fence and a heading.
MARKDOWN = """\
---
title: 'fixture, a front matter line long enough that it would overflow eighty'
---

# Heading

The rewrap tool re-flows one paragraph to the cap the linter applies, so an edit that lengthened a line
costs one call rather than one lint round per
overflowing line.

- A list item that names `a code span with spaces` and runs long enough to wrap - twice over, past the cap.
- The next item stays as it is.

```sh
a fenced line that is long enough to overflow the cap but must never be touched by the tool
```

> A quoted paragraph that is long enough to need wrapping onto a second line at the cap.
"""

YAML = """\
---
jobs:
  lint:
    # The strict yamllint config joins nothing, so a comment an edit made long overflows at once.
    # A second line of the same paragraph.
    #
    # A separate paragraph, left alone.
    runs-on: 'ubuntu-latest'
...
"""


class Rewrap(unittest.TestCase):
    def _file(self, name, text):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        path = Path(scratch.name) / name
        path.write_text(text)
        return path

    def _run(self, *argv):
        out = io.StringIO()
        with redirect_stdout(out):
            code = rewrap.run([str(a) for a in argv])
        return code, out.getvalue()

    def test_a_paragraph_is_refilled_to_eighty_and_only_its_range_printed(self):
        path = self._file("doc.md", MARKDOWN)
        code, out = self._run(path, "--line", "8")
        self.assertEqual(code, 0)
        lines = path.read_text().splitlines()
        self.assertEqual(
            lines[6:9],
            [
                # Exactly 80 fits: MD013 fails only past the cap.
                "The rewrap tool re-flows one paragraph to the cap the linter applies, "
                "so an edit",
                "that lengthened a line costs one call rather than one lint round per "
                "overflowing",
                "line.",
            ],
        )
        self.assertTrue(all(len(line) <= 80 for line in lines[6:9]))
        self.assertIn(f"rewrapped {path}:7-9 to 80: 3 -> 3 line(s)", out)
        self.assertEqual(len(out.splitlines()), 4)
        # Nothing outside the block moved, the fence's long line included.
        self.assertEqual(
            lines[:6] + lines[9:], MARKDOWN.splitlines()[:6] + MARKDOWN.splitlines()[9:]
        )

    def test_a_list_item_keeps_its_marker_code_span_and_avoids_a_bare_dash(self):
        path = self._file("doc.md", MARKDOWN)
        self._run(path, "--anchor", "A list item")
        lines = path.read_text().splitlines()
        item = lines[10 : lines.index("- The next item stays as it is.")]
        self.assertTrue(item[0].startswith("- A list item"))
        self.assertTrue(all(line.startswith("  ") for line in item[1:]))
        self.assertIn("`a code span with spaces`", " ".join(item))
        self.assertFalse(any(line.lstrip().startswith("- ") for line in item[1:]))
        self.assertTrue(all(len(line) <= 80 for line in item))

    def test_a_block_quote_keeps_its_prefix(self):
        path = self._file("doc.md", MARKDOWN)
        self._run(path, "--anchor", "quoted paragraph")
        tail = path.read_text().splitlines()[-2:]
        self.assertTrue(all(line.startswith("> ") for line in tail))

    def test_an_already_wrapped_block_is_reported_and_left(self):
        path = self._file("doc.md", MARKDOWN)
        self._run(path, "--line", "8")
        before = path.read_text()
        code, out = self._run(path, "--line", "8")
        self.assertEqual((code, path.read_text()), (0, before))
        self.assertIn("already wrapped to 80", out)

    def test_dry_run_writes_nothing(self):
        path = self._file("doc.md", MARKDOWN)
        _, out = self._run(path, "--line", "8", "--dry-run")
        self.assertEqual(path.read_text(), MARKDOWN)
        self.assertIn("would rewrap", out)

    def test_a_yaml_comment_paragraph_keeps_its_indent_and_stops_at_a_bare_hash(self):
        path = self._file("lint.yml", YAML)
        code, _ = self._run(path, "--anchor", "joins nothing")
        self.assertEqual(code, 0)
        lines = path.read_text().splitlines()
        block = lines[3 : lines.index("    #")]
        self.assertTrue(all(line.startswith("    # ") for line in block))
        self.assertTrue(all(len(line) <= 80 for line in block))
        self.assertIn("    # A separate paragraph, left alone.", lines)
        self.assertEqual(lines[-2:], ["    runs-on: 'ubuntu-latest'", "..."])

    def test_refusals(self):
        md = self._file("doc.md", MARKDOWN)
        for argv, message in (
            ((md, "--line", "2"), "fence or the front matter"),
            ((md, "--anchor", "a fenced line"), "fence or the front matter"),
            ((md, "--line", "5"), "not a paragraph"),
            ((md, "--anchor", "the"), "exactly one line"),
            ((md, "--line", "999"), "outside the file"),
            ((self._file("lint.yml", YAML), "--line", "2"), "not a `#` comment"),
            ((self._file("x.rs", "// a\n"), "--line", "1"), "pass --width"),
            ((self._file("hard.md", "one  \ntwo\n"), "--line", "1"), "hard line break"),
            ((self._file("crlf.md", "one\r\ntwo\r\n"), "--line", "1"), "CRLF"),
            ((self._file("sx.md", "Title words\n=====\n"), "--line", "1"), "setext"),
            ((self._file("t.md", "a | b\n--- | ---\n1 | 2\n"), "--line", "1"), "table"),
            (
                (self._file("bt.md", "an ` open tick\n"), "--line", "1"),
                "no span closes",
            ),
            ((self._file("d.yml", "# cspell:word zz\n"), "--line", "1"), "directive"),
        ):
            with self.subTest(argv=argv[1:]):
                with self.assertRaisesRegex(rewrap.RewrapError, message):
                    self._run(*argv)

    def _rewrapped(self, name, text, *argv):
        path = self._file(name, text)
        self._run(path, *argv)
        return path.read_text().splitlines()

    def test_a_code_span_may_cross_a_line_and_survives_intact(self):
        lines = self._rewrapped(
            "s.md", "Text (`a\nb`) and `` a ` b `` end.\n", "--line", "1"
        )
        self.assertEqual(lines, ["Text (`a b`) and `` a ` b `` end."])

    def test_a_quote_line_after_a_paragraph_is_its_own_block(self):
        lines = self._rewrapped("q.md", "Some para\n> quoted\n", "--line", "1")
        self.assertEqual(lines, ["Some para", "> quoted"])

    def test_a_backtick_fence_with_a_backtick_info_string_is_not_a_fence(self):
        # Not a fence, so the paragraph after it is not swallowed as code.
        text = "``` a ` b\n\n" + "word " * 20 + "\n"
        path = self._file("f.md", text)
        code, out = self._run(path, "--line", "3")
        self.assertEqual(code, 0)
        self.assertIn("rewrapped", out)

    def test_a_rule_like_token_never_starts_a_line(self):
        text = "x" * 70 + " words ---\n"
        lines = self._rewrapped("r.md", text, "--line", "1")
        self.assertFalse(any(line.startswith("---") for line in lines))

    def test_a_comment_keeps_directives_examples_and_bullets_apart(self):
        text = (
            "# cspell:word zz\n"
            "# Usage, a sentence long enough that the rewrap tool must break it in two.\n"
            "#   make foo ARG=1\n"
            "# - a bullet item that is also long enough to wrap onto a second line here\n"
            "# - a second bullet\n"
        )
        lines = self._rewrapped("c.yml", text, "--anchor", "Usage")
        self.assertEqual(lines[0], "# cspell:word zz")
        self.assertIn("#   make foo ARG=1", lines)
        self.assertEqual(lines[-1], "# - a second bullet")
        lines = self._rewrapped("c.yml", text, "--anchor", "a bullet item")
        item = lines[lines.index("#   make foo ARG=1") + 1 : -1]
        self.assertTrue(item[0].startswith("# - a bullet"))
        self.assertTrue(all(line.startswith("#   ") for line in item[1:]))

    def test_a_changed_line_count_names_the_new_range(self):
        path = self._file("n.md", "one two three four five six\n")
        _, out = self._run(path, "--line", "1", "--width", "10")
        self.assertIn("1 -> 3 line(s), now 1-3", out)
        self.assertEqual(path.read_text(), "one two\nthree four\nfive six\n")


if __name__ == "__main__":
    unittest.main()
