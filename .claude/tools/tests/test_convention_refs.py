#!/usr/bin/env python3
"""Unit tests for ``convention_refs.py`` (stdlib ``unittest``; no pytest).

The properties that matter are the two the prose version did not settle and
that a heading-only implementation gets wrong: a citation may target a **bold
paragraph** rather than a section, and a citation inside a fenced example is
documenting the form rather than making a claim.
"""

from __future__ import annotations

import io
import shutil
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

import convention_refs as cr


class Normalize(unittest.TestCase):
    def test_it_folds_emphasis_backticks_case_and_trailing_punctuation(self):
        """Citations quote loosely, so comparing raw strings would report drift
        that is only transcription."""
        for raw in (
            "The Levers",
            "**the levers**",
            "`the levers`",
            "the levers.",
            "  the   levers  ",
            "the levers —",
        ):
            with self.subTest(raw=raw):
                self.assertEqual(cr.normalize(raw), "the levers")


class Anchors(unittest.TestCase):
    def test_headings_and_bold_spans_both_count(self):
        """Heading-only matching reports false drift — the live instance was
        `"Relations and state belong in the CREATING call"`, a bold lead-in
        rather than a section."""
        found = cr.anchors(
            "# Top\n\n## Sub section\n\n**Relations and state belong in the "
            "CREATING call** — text follows.\n"
        )
        self.assertIn("top", found)
        self.assertIn("sub section", found)
        self.assertIn("relations and state belong in the creating call", found)

    def test_a_fenced_heading_is_not_an_anchor(self):
        """A doc that quotes a heading in an example is not defining one, and
        counting it would let a real dangling citation resolve against a
        sample."""
        found = cr.anchors("# Real\n\n```md\n# Quoted\n```\n")
        self.assertIn("real", found)
        self.assertNotIn("quoted", found)


class Citations(unittest.TestCase):
    def test_both_arrow_spellings_and_both_path_forms(self):
        text = (
            'See `CLAUDE.md` → "Context economy" and\n'
            '`docs/conventions/context-economy.md` -> "The levers" and\n'
            '`context-economy.md` → "The levers" again.\n'
        )
        got = cr.citations(text)
        self.assertEqual(len(got), 3)
        self.assertIn(("CLAUDE.md", "Context economy"), got)
        self.assertIn(("context-economy.md", "The levers"), got)

    def test_a_fenced_citation_is_ignored(self):
        """A skill showing the citation FORM in a worked example is not making
        a claim that has to resolve."""
        text = '```md\nSee `CLAUDE.md` → "Nonexistent section"\n```\n'
        self.assertEqual(cr.citations(text), [])

    def test_prose_without_a_citation_yields_nothing(self):
        self.assertEqual(cr.citations("just prose about CLAUDE.md\n"), [])


class _Fixture:
    """The embedded tree, with no test methods of its own.

    Deliberately NOT a ``TestCase``. It used to be, and ``Cli`` subclassed it —
    which silently re-ran all eight scan tests a second time under the CLI
    class's name, inflating the count while adding no coverage. A fixture that
    holds no tests cannot do that.
    """

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.repo = Path(self._tmp.name)
        (self.repo / ".claude" / "skills" / "demo").mkdir(parents=True)
        (self.repo / "docs" / "conventions").mkdir(parents=True)
        (self.repo / "docs" / "conventions" / "thing.md").write_text(
            "# Thing\n\n## A real section\n\n**A bold lead-in that is cited**"
            " — and prose.\n",
            encoding="utf-8",
        )
        (self.repo / "CLAUDE.md").write_text(
            "# Project\n\n## Context economy\n\ntext\n", encoding="utf-8"
        )

    def _skill(self, body):
        (self.repo / ".claude" / "skills" / "demo" / "SKILL.md").write_text(
            body, encoding="utf-8"
        )


class ScanFixture(_Fixture, unittest.TestCase):
    """Behavior of `scan` against the embedded tree."""

    def test_a_resolving_citation_is_clean(self):
        self._skill('See `docs/conventions/thing.md` → "A real section".\n')
        result = cr.scan(self.repo)
        self.assertEqual(result["dangling"], [])
        self.assertEqual(result["checked"], 1)

    def test_a_bold_anchor_resolves(self):
        self._skill('See `thing.md` → "A bold lead-in that is cited".\n')
        self.assertEqual(cr.scan(self.repo)["dangling"], [])

    def test_a_moved_anchor_is_reported(self):
        """The defect the tool exists for: the doc was renamed and the citer
        was not."""
        self._skill('See `thing.md` → "A section that moved".\n')
        result = cr.scan(self.repo)
        self.assertEqual(result["count"], 1)
        self.assertEqual(result["dangling"][0]["kind"], "missing-anchor")
        self.assertEqual(result["dangling"][0]["anchor"], "A section that moved")

    def test_a_missing_target_file_is_reported_distinctly(self):
        """A renamed doc and a renamed section need different fixes, so they
        get different kinds."""
        self._skill('See `docs/conventions/gone.md` → "Anything".\n')
        result = cr.scan(self.repo)
        self.assertEqual(result["dangling"][0]["kind"], "missing-file")

    def test_a_bare_doc_name_resolves_against_the_conventions_dir(self):
        self._skill('See `thing.md` → "A real section".\n')
        self.assertEqual(cr.scan(self.repo)["dangling"], [])

    def test_an_abbreviated_citation_still_resolves(self):
        """Citations routinely shorten a long heading; demanding the whole
        thing would report drift that is only abbreviation."""
        self._skill('See `thing.md` → "real section".\n')
        self.assertEqual(cr.scan(self.repo)["dangling"], [])

    def test_claude_md_is_both_a_citer_and_a_target(self):
        self._skill('See `CLAUDE.md` → "Context economy".\n')
        self.assertEqual(cr.scan(self.repo)["dangling"], [])

    def test_the_citer_is_named_so_the_fix_is_locatable(self):
        self._skill('See `thing.md` → "nope".\n')
        result = cr.scan(self.repo)
        self.assertEqual(result["dangling"][0]["citer"], ".claude/skills/demo/SKILL.md")

    def test_a_citation_wrapped_across_two_lines_is_checked(self):
        """MD013's 80 columns put the path and arrow on one line and the anchor
        on the next, and per-line matching could not see it. The live tree
        carried 14 such citations in its highest-traffic skills — 61% of the
        real population invisible — so the tool's own count was the evidence
        that looked healthiest.
        """
        self._skill('See `docs/conventions/thing.md` →\n"A real section".\n')
        result = cr.scan(self.repo)
        self.assertEqual(result["checked"], 1)
        self.assertEqual(result["dangling"], [])

    def test_a_wrapped_citation_that_dangles_is_still_reported(self):
        """The point is coverage, not silence: a wrapped citation must be able
        to FAIL, or the fix only moved the blind spot.
        """
        self._skill('See `docs/conventions/thing.md` →\n"A section that moved".\n')
        result = cr.scan(self.repo)
        self.assertEqual(result["checked"], 1)
        self.assertEqual(result["dangling"][0]["kind"], "missing-anchor")

    def test_a_bold_anchor_wrapped_across_lines_resolves(self):
        """The mirror of the same defect, on the other side of the comparison.
        Fixing only the citation half reported two anchors as missing that
        existed perfectly well as multi-line bold spans.
        """
        (self.repo / "docs" / "conventions" / "wrapped.md").write_text(
            "# Wrapped\n\n- **An anchor whose bold span runs past the\n"
            "  eightieth column.** Then prose.\n",
            encoding="utf-8",
        )
        self._skill('See `wrapped.md` → "An anchor whose bold span runs past".\n')
        result = cr.scan(self.repo)
        self.assertEqual(result["dangling"], [])

    def test_a_stray_emphasis_marker_cannot_desync_the_next_paragraph(self):
        """Why matching is per PARAGRAPH and not per document. Bold pairing is
        positional, so one unpaired `**` shifts every pairing after it —
        whole-document DOTALL turned 2 unresolved citations into 5, all false,
        by mispairing spans hundreds of lines away. Per line that damage is
        contained to a line; per paragraph, to a paragraph.
        """
        (self.repo / "docs" / "conventions" / "stray.md").write_text(
            "# Stray\n\nA paragraph with one ** unpaired marker.\n\n"
            "- **A clean anchor after the stray.** Prose.\n",
            encoding="utf-8",
        )
        self._skill('See `stray.md` → "A clean anchor after the stray".\n')
        result = cr.scan(self.repo)
        self.assertEqual(result["dangling"], [])

    def test_an_anchor_that_normalizes_away_is_refused_not_resolved(self):
        """`normalize` strips emphasis, backticks and trailing punctuation, so
        `→ "."` reaches the comparison empty — and `"" in candidate` is always
        true, resolving against every target that has any anchor at all. The
        literal vacuous pass.
        """
        self._skill('See `thing.md` → ".".\n')
        result = cr.scan(self.repo)
        self.assertEqual(result["checked"], 1)
        self.assertEqual(result["dangling"][0]["kind"], "empty-anchor")

    def test_a_citation_inside_a_fence_is_still_ignored(self):
        """Paragraph-scoping must not lose the fence rule: a worked example
        documents the form and is not a claim that has to resolve.
        """
        self._skill(
            'Example:\n\n```\nSee `thing.md` → "Not a real anchor".\n```\n',
        )
        result = cr.scan(self.repo)
        self.assertEqual(result["checked"], 0)


class Cli(_Fixture, unittest.TestCase):
    def _run(self, argv):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = cr.run(["convention_refs.py", "--repo", str(self.repo)] + argv)
        return code, out.getvalue(), err.getvalue()

    def test_exits_zero_when_everything_resolves(self):
        self._skill('See `thing.md` → "A real section".\n')
        code, out, _ = self._run([])
        self.assertEqual(code, 0)
        self.assertIn("resolve", out)

    def test_a_moved_citer_family_is_refused_not_reported_clean(self):
        """The zero-citation refusal cannot catch this: four globs feed the
        scan, so losing the whole skills tree leaves `docs/conventions/*.md`
        citing, the total non-zero and the exit 0 — the loss invisible behind a
        healthy-looking count. That is the one failure the refusal exists to
        prevent, arriving one level down.
        """
        self._skill('See `thing.md` → "A real section".\n')
        shutil.rmtree(self.repo / ".claude" / "skills")
        with self.assertRaises(cr.ConventionRefsError) as ctx:
            self._run([])
        message = str(ctx.exception)
        self.assertIn("matched no files", message)
        self.assertIn(".claude/skills/*/SKILL.md", message)
        # And specifically NOT the zero-citation message, which is the less
        # actionable diagnosis of the same tree state.
        self.assertNotIn("nothing was checked", message)

    def test_exits_one_when_something_dangles(self):
        self._skill('See `thing.md` → "nope".\n')
        code, out, _ = self._run([])
        self.assertEqual(code, 1)
        self.assertIn("missing-anchor", out)

    def test_json_mode_is_machine_readable(self):
        import json

        self._skill('See `thing.md` → "nope".\n')
        code, out, _ = self._run(["--json"])
        self.assertEqual(code, 1)
        self.assertEqual(json.loads(out)["count"], 1)

    def test_finding_no_citations_is_not_reported_as_clean(self):
        """A wrong --repo, a renamed skills tree or a changed citation form all
        produce zero citations, and returning 0 for that is byte-identical — in
        the one signal a caller branches on — to "everything resolves"."""
        self._skill("no citations here at all\n")
        (self.repo / "CLAUDE.md").write_text("# Project\n", encoding="utf-8")
        (self.repo / "docs" / "conventions" / "thing.md").write_text(
            "# Thing\n", encoding="utf-8"
        )
        with self.assertRaises(cr.ConventionRefsError):
            self._run([])

    def test_main_maps_a_failure_to_exit_two(self):
        import sys

        argv = ["convention_refs.py", "--repo", str(self.repo / "nope")]
        err = io.StringIO()
        with redirect_stderr(err):
            original, sys.argv = sys.argv, argv
            try:
                code = cr.main()
            finally:
                sys.argv = original
        self.assertEqual(code, 2)
        self.assertIn("convention-refs:", err.getvalue())


if __name__ == "__main__":
    unittest.main()
