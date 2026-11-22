#!/usr/bin/env python3
"""Unit tests for ``planning_doc.py`` (stdlib ``unittest``; no pytest).

The property that matters is the one the tool exists for: a scoped read must
print **only** the requested sections, never the whole document. A
`get_document` was one measured session's largest single main-loop result at
≈7.9k, consumed for four short passages.
"""

from __future__ import annotations

import io
import os
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

import planning_doc as pd

DOC = "\n".join(
    [
        "# Board schema",
        "",
        "read this first",
        "",
        "# Audit state",
        "",
        "| unit | last | findings |",
        "| ---- | ---- | -------- |",
        "| seam | 09-01 | 8 |",
        "",
        "# Standing decisions",
        "",
        "never hardcode slot duration",
        "",
        "## Program and timing",
        "",
        "the no-level-price-bound rule is ratified",
    ]
)

ENV = {"LINEAR_API_KEY": "k", "LINEAR_PLANNING_DOC_ID": "doc-1"}


class _Stubbed(unittest.TestCase):
    def setUp(self):
        self._post = mock.patch.object(
            pd.linear_api,
            "post",
            return_value={"document": {"title": "Planning", "content": DOC}},
        )
        self._post.start()
        self.addCleanup(self._post.stop)

    def _run(self, *args):
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.dict(os.environ, ENV, clear=True):
            with redirect_stdout(out), redirect_stderr(err):
                code = pd.run(["planning_doc.py", *args])
        return code, out.getvalue(), err.getvalue()


class ScopedReads(_Stubbed):
    def test_headings_prints_the_map_and_no_content(self):
        code, out, err = self._run("--headings")
        self.assertEqual(code, 0)
        self.assertIn("Audit state", out)
        # The map is not the document: body prose must not come along.
        self.assertNotIn("never hardcode slot duration", out)
        # The accounting line goes to stderr, so stdout stays consumable.
        self.assertIn("Planning", err)
        self.assertIn("heading(s)", err)

    def test_a_section_returns_only_that_section(self):
        """The measured case: four short passages wanted out of a document
        covering the board schema, four tracks, feeds detail and more."""
        code, out, _ = self._run("--section", "Audit state")
        self.assertEqual(code, 0)
        self.assertIn("| seam | 09-01 | 8 |", out)
        self.assertNotIn("read this first", out)
        self.assertNotIn("never hardcode", out)

    def test_a_section_carries_its_subsections(self):
        _, out, _ = self._run("--section", "Standing decisions")
        self.assertIn("Program and timing", out)
        self.assertIn("no-level-price-bound", out)

    def test_grep_returns_matching_lines_only(self):
        _, out, _ = self._run("--grep", "slot duration")
        self.assertIn("never hardcode slot duration", out)
        self.assertNotIn("Board schema", out)

    def test_sections_takes_every_match(self):
        _, out, _ = self._run("--sections", "^(Audit state|Board schema)$")
        self.assertIn("read this first", out)
        self.assertIn("| seam | 09-01 | 8 |", out)
        self.assertNotIn("never hardcode", out)

    def test_an_unknown_section_is_a_clean_error_naming_the_map(self):
        with mock.patch.dict(os.environ, ENV, clear=True):
            with redirect_stderr(io.StringIO()):
                with self.assertRaises(pd.read_result.ReadResultError) as ctx:
                    pd.run(["planning_doc.py", "--section", "Nope"])
        self.assertIn("--headings", str(ctx.exception))


class Spill(_Stubbed):
    def test_out_writes_the_document_and_prints_only_a_map(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "planning.md")
            code, out, err = self._run("--out", path)
            self.assertEqual(code, 0)
            written = Path(path).read_text(encoding="utf-8")
            self.assertIn("never hardcode slot duration", written)
            # Zero echo: the content is in the file, not in the transcript.
            self.assertNotIn("never hardcode slot duration", out + err)
            self.assertIn("Audit state", out)

    def test_the_spill_is_owner_only(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "planning.md")
            self._run("--out", path)
            self.assertEqual(os.stat(path).st_mode & 0o777, 0o600)

    def test_a_re_spill_narrows_an_existing_permissive_file(self):
        """`O_CREAT`'s mode argument applies only when the file is CREATED, so
        this test's sibling above — which always writes into a fresh temp dir —
        could not fail for the reason it names, and the owner-only guarantee
        held only on first write.

        Re-running `--out <scratchpad>/planning.md` to the same path is exactly
        the shape this tool recommends, so the guarantee was absent in the case
        that actually recurs. `fchmod` after open makes it unconditional.
        """
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "planning.md")
            Path(path).write_text("stale\n", encoding="utf-8")
            os.chmod(path, 0o644)
            self._run("--out", path)
            self.assertEqual(os.stat(path).st_mode & 0o777, 0o600)

    def test_an_empty_out_path_is_a_clean_error_not_a_traceback(self):
        """`--out ''` is falsy, so dispatching on truthiness fell through the
        chain to the terminal `grep` branch with `args.grep is None` — a
        traceback, where this tool's exception class promises one clean stderr
        line. Nuisance input, but the promise is the point.
        """
        with self.assertRaises((pd.PlanningDocError, OSError)):
            self._run("--out", "")


AUDIT_NEW = "# Audit state\n\n| unit | last | findings |\n| seam | 11-21 | 3 |\n\n"


class ReplaceSections(unittest.TestCase):
    """The close-out's section patch must equal the wholesale rewrite."""

    def test_the_result_is_byte_identical_to_the_wholesale_rewrite(self):
        stored = DOC + "\n"
        start = stored.index("# Audit state")
        end = stored.index("# Standing decisions")
        wholesale = stored[:start] + AUDIT_NEW + stored[end:]
        got, count = pd.replace_sections(stored, pd._file_blocks(AUDIT_NEW), [])
        self.assertEqual(got, wholesale)
        self.assertEqual(count, 1)

    def test_a_stored_subsection_left_out_is_refused_unless_dropped(self):
        # The live case: the notes section's machine-written `## Upkeep report`
        # child would vanish with a replacement that did not re-supply it.
        new = "# Standing decisions\n\nonly this now\n"
        with self.assertRaises(pd.PlanningDocError) as ctx:
            pd.replace_sections(DOC, pd._file_blocks(new), [])
        self.assertIn("'## Program and timing'", str(ctx.exception))
        got, count = pd.replace_sections(
            DOC, pd._file_blocks(new), ["## Program and timing"]
        )
        self.assertNotIn("Program and timing", got)
        self.assertTrue(got.endswith("only this now\n"))
        self.assertIn("| seam | 09-01 | 8 |", got)
        self.assertEqual(count, 1)  # the acknowledged child is inside the splice

    def test_a_re_supplied_subsection_is_carried_in(self):
        new = "# Standing decisions\n\nnew\n\n## Program and timing\n\nkept\n"
        got, _ = pd.replace_sections(DOC, pd._file_blocks(new), [])
        self.assertTrue(got.endswith("## Program and timing\n\nkept\n"))

    def test_a_depth_two_section_can_be_dropped_alone(self):
        got, _ = pd.replace_sections(
            DOC, pd._file_blocks(AUDIT_NEW), ["## Program and timing"]
        )
        self.assertNotIn("no-level-price-bound", got)
        self.assertIn("never hardcode slot duration", got)

    def test_the_blank_separator_before_the_next_heading_survives(self):
        # A block ending without its blank line would butt against the next
        # stored heading — renders the same, but not the wholesale rewrite.
        tight = AUDIT_NEW.rstrip("\n") + "\n"
        stored = DOC + "\n"
        start = stored.index("# Audit state")
        end = stored.index("# Standing decisions")
        got, _ = pd.replace_sections(stored, pd._file_blocks(tight), [])
        self.assertEqual(got, stored[:start] + AUDIT_NEW + stored[end:])

    def test_drop_deletes_a_section_and_keeps_the_rest(self):
        got, count = pd.replace_sections(
            DOC, pd._file_blocks(AUDIT_NEW), ["# Board schema"]
        )
        self.assertNotIn("read this first", got)
        self.assertTrue(got.startswith("# Audit state"))
        self.assertEqual(count, 2)

    def test_a_heading_must_match_exactly_not_by_prefix(self):
        with self.assertRaises(pd.PlanningDocError) as ctx:
            pd.replace_sections(DOC, pd._file_blocks("# Audit\n\nx\n"), [])
        self.assertIn("0 stored heading(s)", str(ctx.exception))

    def test_a_heading_matching_twice_is_refused(self):
        twice = DOC + "\n# Audit state\n\nagain\n"
        with self.assertRaises(pd.PlanningDocError):
            pd.replace_sections(twice, pd._file_blocks(AUDIT_NEW), [])

    def test_depth_must_match_too(self):
        with self.assertRaises(pd.PlanningDocError):
            pd.replace_sections(DOC, pd._file_blocks("## Audit state\n\nx\n"), [])

    def test_a_missing_final_newline_does_not_glue_the_next_heading(self):
        got, _ = pd.replace_sections(DOC, pd._file_blocks("# Audit state\n\nx"), [])
        self.assertIn("x\n\n# Standing decisions", got)

    def test_the_file_is_validated(self):
        for bad in (
            "no heading",
            "stray\n# Audit state\n",
            "## A\n# B\n",
            "# A\n# A\n",
        ):
            with self.subTest(bad=bad), self.assertRaises(pd.PlanningDocError):
                pd._file_blocks(bad)

    def test_a_malformed_drop_is_refused(self):
        with self.assertRaises(pd.PlanningDocError):
            pd.replace_sections(DOC, pd._file_blocks(AUDIT_NEW), ["Board schema"])


class ReplaceRun(unittest.TestCase):
    def _run(self, responses, *extra):
        calls = []

        def post(api_key, query, variables, error):
            calls.append((query, variables))
            return responses.pop(0)

        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "changed.md")
            Path(path).write_text(AUDIT_NEW, encoding="utf-8")
            err = io.StringIO()
            with mock.patch.object(pd.linear_api, "post", side_effect=post):
                with mock.patch.dict(os.environ, ENV, clear=True):
                    with redirect_stdout(io.StringIO()) as out, redirect_stderr(err):
                        code = pd.run(
                            [
                                "planning_doc.py",
                                "replace-sections",
                                "--file",
                                path,
                                *extra,
                            ]
                        )
        return code, out.getvalue(), err.getvalue(), calls

    @staticmethod
    def doc(content):
        return {"document": {"title": "Planning", "content": content}}

    OK = {"documentUpdate": {"success": True}}

    def test_writes_once_reads_back_and_echoes_nothing(self):
        composed, _ = pd.replace_sections(DOC, pd._file_blocks(AUDIT_NEW), [])
        code, out, err, calls = self._run(
            [self.doc(DOC), self.doc(DOC), self.OK, self.doc(composed)]
        )
        self.assertEqual(code, 0)
        # fetch, re-fetch just before the write, the write, the read-back.
        self.assertEqual(len(calls), 4)
        self.assertEqual(calls[2][1], {"id": "doc-1", "content": composed})
        self.assertIn("read-back identical", err)
        # Zero echo: neither the carried sections nor the new one reach output.
        self.assertNotIn("never hardcode", out + err)
        self.assertNotIn("11-21", out + err)

    def test_a_differing_read_back_exits_one_naming_the_line(self):
        code, _, err, _ = self._run(
            [self.doc(DOC), self.doc(DOC), self.OK, self.doc(DOC)]
        )
        self.assertEqual(code, 1)
        self.assertIn("read-back differs from line", err)

    def test_an_edit_landing_before_the_write_is_kept_not_overwritten(self):
        # Linear has no server-side anchored document patch, so the in-process
        # splice must not clobber an append that landed after the first fetch.
        appended = DOC + "\n\n# Late note\n\nfrom another session\n"
        composed, _ = pd.replace_sections(appended, pd._file_blocks(AUDIT_NEW), [])
        code, _, err, calls = self._run(
            [
                self.doc(DOC),
                self.doc(appended),
                self.doc(appended),
                self.OK,
                self.doc(composed),
            ]
        )
        self.assertEqual(code, 0)
        self.assertIn("from another session", calls[3][1]["content"])
        self.assertIn("re-spliced over a concurrent edit", err)

    def test_two_edits_in_a_row_refuse_rather_than_guess(self):
        with self.assertRaises(pd.PlanningDocError):
            self._run([self.doc(DOC), self.doc(DOC + "\nx\n"), self.doc(DOC + "\ny\n")])

    def test_dry_run_reads_and_writes_nothing(self):
        code, _, err, calls = self._run([self.doc(DOC)], "--dry-run")
        self.assertEqual(code, 0)
        self.assertEqual(len(calls), 1)
        self.assertIn("WOULD REPLACE", err)

    def test_an_unchanged_set_is_refused(self):
        composed, _ = pd.replace_sections(DOC, pd._file_blocks(AUDIT_NEW), [])
        with self.assertRaises(pd.PlanningDocError):
            self._run([self.doc(composed)])

    def test_a_failed_update_raises(self):
        with self.assertRaises(pd.PlanningDocError):
            self._run(
                [self.doc(DOC), self.doc(DOC), {"documentUpdate": {"success": False}}]
            )


class Failures(unittest.TestCase):
    def test_empty_content_is_an_error_not_a_silent_empty_read(self):
        with mock.patch.object(
            pd.linear_api, "post", return_value={"document": {"content": ""}}
        ):
            with mock.patch.dict(os.environ, ENV, clear=True):
                with self.assertRaises(pd.PlanningDocError) as ctx:
                    pd.run(["planning_doc.py", "--headings"])
        self.assertIn("LINEAR_PLANNING_DOC_ID", str(ctx.exception))

    def test_a_mode_is_required(self):
        with mock.patch.dict(os.environ, ENV, clear=True):
            with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                pd.run(["planning_doc.py"])

    def test_main_maps_a_failure_to_exit_two(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            err = io.StringIO()
            with redirect_stderr(err):
                with mock.patch.object(
                    pd.sys, "argv", ["planning_doc.py", "--headings"]
                ):
                    code = pd.main()
        self.assertEqual(code, 2)
        self.assertIn("planning-doc:", err.getvalue())


if __name__ == "__main__":
    unittest.main()
