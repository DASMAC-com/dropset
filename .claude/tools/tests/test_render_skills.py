#!/usr/bin/env python3
"""Unit tests for render_skills.py.

The gate's whole value is that it **fails**: a rendered region that has been
hand-edited, an unclosed marker, or a marker naming a source that does not
exist must all be caught. A renderer that only ever passes is the same silent
failure as a committed-but-unwired guard hook.
"""

from __future__ import annotations

import io
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import render_skills as rs  # noqa: E402

BLOCK = "Shared prose about {{verb}}.\n\nA second paragraph.\n"


class RenderTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        (self.root / rs.SHARED_DIR).mkdir(parents=True)
        (self.root / rs.SHARED_DIR / "guard.md").write_text(BLOCK, encoding="utf-8")

    def _skill(self, name: str, body: str) -> Path:
        directory = self.root / rs.SKILLS_DIR / name
        directory.mkdir(parents=True, exist_ok=True)
        path = directory / "SKILL.md"
        path.write_text(body, encoding="utf-8")
        return path

    def _run(self, argv):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = rs.run(["render_skills.py", "--root", str(self.root)] + argv)
        return code, out.getvalue() + err.getvalue()

    def test_an_empty_region_is_filled_from_the_source(self):
        path = self._skill(
            "a",
            "# A\n\n<!-- render:begin guard verb=paps -->\n<!-- render:end guard -->\n",
        )
        self._run(["--write"])
        text = path.read_text(encoding="utf-8")
        self.assertIn("Shared prose about paps.", text)
        self.assertIn("A second paragraph.", text)

    def test_the_substitution_differs_per_call_site(self):
        a = self._skill(
            "a",
            "<!-- render:begin guard verb=paps -->\n<!-- render:end guard -->\n",
        )
        b = self._skill(
            "b",
            "<!-- render:begin guard verb=caps -->\n<!-- render:end guard -->\n",
        )
        self._run(["--write"])
        self.assertIn("about paps.", a.read_text(encoding="utf-8"))
        self.assertIn("about caps.", b.read_text(encoding="utf-8"))

    def test_rendering_is_idempotent(self):
        path = self._skill(
            "a",
            "<!-- render:begin guard verb=paps -->\n<!-- render:end guard -->\n",
        )
        self._run(["--write"])
        once = path.read_text(encoding="utf-8")
        self._run(["--write"])
        self.assertEqual(once, path.read_text(encoding="utf-8"))

    def test_check_passes_when_in_sync(self):
        self._skill(
            "a",
            "<!-- render:begin guard verb=paps -->\n<!-- render:end guard -->\n",
        )
        self._run(["--write"])
        code, _ = self._run(["--check"])
        self.assertEqual(code, 0)

    def test_check_FAILS_on_a_hand_edited_region(self):
        # The regression the gate exists for.
        path = self._skill(
            "a",
            "<!-- render:begin guard verb=paps -->\n<!-- render:end guard -->\n",
        )
        self._run(["--write"])
        path.write_text(
            path.read_text(encoding="utf-8").replace("Shared prose", "Edited prose"),
            encoding="utf-8",
        )
        code, printed = self._run(["--check"])
        self.assertEqual(code, 1)
        self.assertIn("STALE", printed)

    def test_check_FAILS_when_the_source_changed_and_the_skill_did_not(self):
        self._skill(
            "a",
            "<!-- render:begin guard verb=paps -->\n<!-- render:end guard -->\n",
        )
        self._run(["--write"])
        (self.root / rs.SHARED_DIR / "guard.md").write_text(
            "Rewritten prose about {{verb}}.\n", encoding="utf-8"
        )
        code, printed = self._run(["--check"])
        self.assertEqual(code, 1)
        self.assertIn("STALE", printed)

    def test_check_does_not_write(self):
        path = self._skill(
            "a",
            "<!-- render:begin guard verb=paps -->\n<!-- render:end guard -->\n",
        )
        before = path.read_text(encoding="utf-8")
        self._run(["--check"])
        self.assertEqual(before, path.read_text(encoding="utf-8"))

    def test_a_file_with_no_markers_is_left_alone(self):
        path = self._skill("a", "# Plain\n\nNothing to render.\n")
        before = path.read_text(encoding="utf-8")
        code, _ = self._run(["--write"])
        self.assertEqual(code, 0)
        self.assertEqual(before, path.read_text(encoding="utf-8"))

    def test_an_unclosed_marker_is_an_error(self):
        self._skill("a", "<!-- render:begin guard verb=paps -->\nbody\n")
        with self.assertRaises(rs.RenderError) as caught:
            self._run(["--check"])
        self.assertIn("unclosed", str(caught.exception))

    def test_an_end_without_a_begin_is_an_error(self):
        self._skill("a", "text\n<!-- render:end guard -->\n")
        with self.assertRaises(rs.RenderError) as caught:
            self._run(["--check"])
        self.assertIn("no matching begin", str(caught.exception))

    def test_a_mismatched_end_is_an_error(self):
        self._skill(
            "a",
            "<!-- render:begin guard verb=paps -->\n<!-- render:end other -->\n",
        )
        with self.assertRaises(rs.RenderError) as caught:
            self._run(["--check"])
        self.assertIn("closed by", str(caught.exception))

    def test_a_marker_naming_a_missing_source_is_an_error(self):
        # A region that can never render is the same silent failure as an
        # unwired guard hook, so it must be loud.
        self._skill(
            "a",
            "<!-- render:begin absent verb=paps -->\n<!-- render:end absent -->\n",
        )
        with self.assertRaises(rs.RenderError) as caught:
            self._run(["--check"])
        self.assertIn("no shared block", str(caught.exception))

    def test_an_unresolved_placeholder_is_refused_not_emitted(self):
        # A `{{verb}}` reaching a rendered skill would be read by an agent as
        # literal text — a silent instruction defect rather than a visible one.
        self._skill("a", "<!-- render:begin guard -->\n<!-- render:end guard -->\n")
        with self.assertRaises(rs.RenderError) as caught:
            self._run(["--check"])
        self.assertIn("no value given for verb", str(caught.exception))

    def test_a_malformed_marker_argument_is_refused(self):
        self._skill(
            "a",
            "<!-- render:begin guard paps -->\n<!-- render:end guard -->\n",
        )
        with self.assertRaises(rs.RenderError) as caught:
            self._run(["--check"])
        self.assertIn("malformed", str(caught.exception))

    def test_indentation_is_carried_onto_every_rendered_line(self):
        self._skill(
            "a",
            "   <!-- render:begin guard verb=paps -->\n   <!-- render:end guard -->\n",
        )
        self._run(["--write"])
        text = (self.root / rs.SKILLS_DIR / "a" / "SKILL.md").read_text(
            encoding="utf-8"
        )
        self.assertIn("   Shared prose about paps.", text)

    def test_the_region_is_padded_with_a_blank_line_on_each_side(self):
        # mdformat inserts a blank line between an HTML comment and an adjacent
        # paragraph. Emitting it here makes rendering a FIXED POINT of the
        # formatter — without it every render is immediately reformatted and
        # --check reports the file stale forever, which makes the gate useless.
        path = self._skill(
            "a",
            "<!-- render:begin guard verb=paps -->\n<!-- render:end guard -->\n",
        )
        self._run(["--write"])
        lines = path.read_text(encoding="utf-8").splitlines()
        begin = lines.index("<!-- render:begin guard verb=paps -->")
        end = lines.index("<!-- render:end guard -->")
        self.assertEqual(lines[begin + 1], "")
        self.assertEqual(lines[end - 1], "")

    def test_a_blank_line_is_emitted_without_trailing_indent(self):
        # Trailing whitespace on a blank line is what the whitespace hooks
        # strip, so emitting it would make rendered output fail lint by
        # construction.
        self._skill(
            "a",
            "   <!-- render:begin guard verb=paps -->\n   <!-- render:end guard -->\n",
        )
        self._run(["--write"])
        text = (self.root / rs.SKILLS_DIR / "a" / "SKILL.md").read_text(
            encoding="utf-8"
        )
        self.assertNotIn("   \n", text)


class RealRepoTests(unittest.TestCase):
    """The committed skills must pass their own gate."""

    def test_the_repo_is_in_sync(self):
        repo = Path(
            os.path.dirname(
                os.path.dirname(
                    os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
                )
            )
        )
        if not (repo / rs.SHARED_DIR).is_dir():
            self.skipTest("shared blocks not present in this checkout")
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = rs.run(["render_skills.py", "--check", "--root", str(repo)])
        self.assertEqual(code, 0, err.getvalue())


if __name__ == "__main__":
    unittest.main()
