#!/usr/bin/env python3
"""Tests for `lens_preamble.py`.

The interesting cases are the failure ones: this tool exists so a skill never
reads the convention doc, which means nothing downstream would notice a
silently truncated or empty brief. So a doc edit that moves the marker or
breaks the blockquote must fail loudly here rather than shipping half the
shell rules into every lens.
"""

from __future__ import annotations

import io
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

import lens_preamble as lp

DOC = (
    "# Briefing sub-agents\n"
    "\n"
    "Some framing prose that must not be emitted.\n"
    "\n"
    f"{lp.BRIEF_MARKER}\n"
    "\n"
    "> - You are a **read-only** agent.\n"
    "> - One bare command per Bash call.\n"
    ">   Continued on an indented line.\n"
    "\n"
    "**Trailing prose that must not be emitted.**\n"
)


class ExtractTests(unittest.TestCase):
    def test_the_blockquote_is_emitted_unquoted(self):
        brief = lp.extract_brief(DOC)
        self.assertNotIn(">", brief)
        self.assertIn("You are a **read-only** agent.", brief)

    def test_nested_indentation_is_preserved(self):
        """The brief uses continuation lines; flattening them would reflow it
        into a different list structure."""
        brief = lp.extract_brief(DOC)
        self.assertIn("\n  Continued on an indented line.", brief)

    def test_framing_prose_is_not_emitted(self):
        brief = lp.extract_brief(DOC)
        self.assertNotIn("framing prose", brief)
        self.assertNotIn("Trailing prose", brief)

    def test_a_missing_marker_is_a_loud_failure(self):
        """Nothing downstream reads the doc, so a silent empty brief would ship
        every lens without the shell rules."""
        with self.assertRaises(lp.LensPreambleError) as ctx:
            lp.extract_brief("# Briefing sub-agents\n\n> - orphaned quote\n")
        self.assertIn("marker", str(ctx.exception))

    def test_a_marker_with_no_blockquote_is_a_loud_failure(self):
        text = f"{lp.BRIEF_MARKER}\n\nplain prose, no quote\n"
        with self.assertRaises(lp.LensPreambleError) as ctx:
            lp.extract_brief(text)
        self.assertIn("no blockquote", str(ctx.exception))

    def test_the_quote_ends_at_the_first_blank_line_inside_it(self):
        """Documents the boundary: the brief is authored as one unbroken block,
        so a doc edit that splits it is visible as a short brief."""
        text = f"{lp.BRIEF_MARKER}\n\n> - first\n\n> - second\n"
        self.assertEqual(lp.extract_brief(text), "- first")


class ComposeTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)
        doc = self.root / lp.BRIEF_DOC
        doc.parent.mkdir(parents=True, exist_ok=True)
        doc.write_text(DOC, encoding="utf-8")

    def test_the_brief_lands_in_the_composed_output(self):
        out = lp.compose(self.root, [])
        self.assertIn("You are a **read-only** agent.", out)
        self.assertIn("Standing brief for this review", out)

    def test_appended_sections_follow_the_brief_in_order(self):
        (self.root / "a.md").write_text("## Alpha\n", encoding="utf-8")
        (self.root / "b.md").write_text("## Beta\n", encoding="utf-8")
        out = lp.compose(self.root, [Path("a.md"), Path("b.md")])
        self.assertLess(out.index("read-only"), out.index("## Alpha"))
        self.assertLess(out.index("## Alpha"), out.index("## Beta"))

    def test_a_missing_append_target_errors(self):
        with self.assertRaises(lp.LensPreambleError):
            lp.compose(self.root, [Path("nope.md")])

    def test_a_missing_brief_doc_errors(self):
        empty = Path(self._tmp.name) / "empty"
        empty.mkdir()
        with self.assertRaises(lp.LensPreambleError):
            lp.compose(empty, [])


class CliTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)
        doc = self.root / lp.BRIEF_DOC
        doc.parent.mkdir(parents=True, exist_ok=True)
        doc.write_text(DOC, encoding="utf-8")

    def _capture(self, argv):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = lp.run(argv)
        return code, out.getvalue(), err.getvalue()

    def test_it_writes_the_file_and_prints_its_path(self):
        target = self.root / "scratch" / "lens-preamble.md"
        code, out, err = self._capture(
            [
                "lens_preamble.py",
                "--root",
                str(self.root),
                "--out",
                str(target),
            ]
        )
        self.assertEqual(code, 0)
        self.assertEqual(out.strip(), str(target))
        self.assertTrue(target.is_file())
        self.assertIn("read-only", target.read_text(encoding="utf-8"))
        self.assertIn("0 appended section(s)", err)

    def test_it_creates_the_output_directory(self):
        target = self.root / "a" / "b" / "c.md"
        code, _, _ = self._capture(
            ["lens_preamble.py", "--root", str(self.root), "--out", str(target)]
        )
        self.assertEqual(code, 0)
        self.assertTrue(target.is_file())

    def test_appends_are_counted_on_the_summary_line(self):
        (self.root / "extra.md").write_text("## Extra\n", encoding="utf-8")
        target = self.root / "out.md"
        _, _, err = self._capture(
            [
                "lens_preamble.py",
                "--root",
                str(self.root),
                "--out",
                str(target),
                "--append",
                "extra.md",
            ]
        )
        self.assertIn("1 appended section(s)", err)


class RealDocTests(unittest.TestCase):
    """Guards the wiring against the actual committed doc — the failure this
    tool's own tests would otherwise miss, since every other test builds its
    own fixture."""

    def test_the_committed_brief_still_extracts(self):
        root = Path(__file__).resolve().parents[3]
        doc = root / lp.BRIEF_DOC
        if not doc.is_file():  # pragma: no cover - depends on checkout layout
            self.skipTest(f"{doc} not present")
        brief = lp.extract_brief(doc.read_text(encoding="utf-8"))
        self.assertIn("read-only", brief)
        self.assertNotIn("\n>", brief)
        # Long enough that a truncation to one bullet is caught.
        self.assertGreater(len(brief.splitlines()), 20)

    def test_the_committed_standing_template_still_composes(self):
        root = Path(__file__).resolve().parents[3]
        template = Path(".claude/skills/review-pr/lens-standing.md")
        if not (root / template).is_file():  # pragma: no cover
            self.skipTest(f"{template} not present")
        out = lp.compose(root, [template])
        self.assertIn("Negative scope", out)
        self.assertIn("read-only", out)


if __name__ == "__main__":
    unittest.main()
