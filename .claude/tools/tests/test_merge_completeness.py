#!/usr/bin/env python3
"""Unit tests for merge_completeness.py.

The headline case is the measured one: a doc comment both parents rewrote
independently, which merges cleanly and is still wrong. Two tests pin it from
both sides — the detectable form, where the resolution keeps neither parent's
line and the check catches it, and the form where both parents wrote the
identical line, which passes. The second is the tool's honest bound, and it is
pinned deliberately: the value here is a failure forcing adjudication, never a
pass read as authorization.
"""

from __future__ import annotations

import io
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import merge_completeness as mc  # noqa: E402
from merge_completeness import audit, normalize, normalized_set, report, run  # noqa: E402


class NormalizeTests(unittest.TestCase):
    def test_surrounding_whitespace_is_irrelevant(self):
        self.assertEqual(normalize("  word  "), "word")

    def test_internal_runs_collapse(self):
        # Re-indentation is not a loss; treating it as one produced two measured
        # false losses, which is how a checker earns being ignored.
        self.assertEqual(normalize("a\t\tb   c"), "a b c")

    def test_a_blank_line_normalizes_to_empty(self):
        self.assertEqual(normalize("   \t "), "")

    def test_blank_lines_are_dropped_from_the_set(self):
        self.assertEqual(list(normalized_set("a\n\n \nb")), ["a", "b"])

    def test_the_first_raw_spelling_is_kept_for_reporting(self):
        self.assertEqual(normalized_set("  a  \na")["a"], "  a  ")


BASE = "alpha\nbravo\ncharlie\n"


class AuditTests(unittest.TestCase):
    def test_a_union_resolution_is_complete(self):
        ours = BASE + "ours-one\n"
        theirs = BASE + "theirs-one\n"
        resolution = BASE + "ours-one\ntheirs-one\n"
        v = audit(BASE, ours, theirs, resolution)
        self.assertTrue(v["complete"])
        self.assertEqual(v["ours"]["survived"], 1)
        self.assertEqual(v["theirs"]["survived"], 1)
        self.assertEqual(v["union_added"], 2)

    def test_dropping_one_side_is_caught_per_side(self):
        ours = BASE + "ours-one\n"
        theirs = BASE + "theirs-one\n"
        resolution = BASE + "ours-one\n"
        v = audit(BASE, ours, theirs, resolution)
        self.assertFalse(v["complete"])
        self.assertEqual(v["ours"]["missing"], [])
        self.assertEqual(v["theirs"]["missing"], ["theirs-one"])

    def test_identical_rewrites_by_both_sides_pass_the_honest_bound(self):
        # The four-kinds shape in its WORST form: both parents independently
        # rewrote one prose line to the same text, each counting a different new
        # variant, and the true post-merge answer was five. The line does
        # survive, so completeness passes and the bug goes through. Pinned here
        # because it is the tool's honest bound — a pass is not a green light,
        # and this test is the evidence for saying so.
        base = "// supports all three kinds\n"
        ours = "// supports all four kinds\n"
        theirs = "// supports all four kinds\n"
        resolution = "// supports all four kinds\n"
        v = audit(base, ours, theirs, resolution)
        self.assertTrue(v["complete"])

    def test_a_supersession_that_loses_both_lines_is_caught(self):
        # The detectable form of the same class: the resolution rewrote the line
        # into something neither parent wrote, so both contributions are gone.
        base = "// supports all three kinds\n"
        ours = "// supports all four kinds\n"
        theirs = "// supports all four sorts\n"
        resolution = "// supports all five kinds\n"
        v = audit(base, ours, theirs, resolution)
        self.assertFalse(v["complete"])
        self.assertEqual(v["ours"]["missing"], ["// supports all four kinds"])
        self.assertEqual(v["theirs"]["missing"], ["// supports all four sorts"])

    def test_an_indentation_change_is_not_a_loss(self):
        ours = BASE + "    indented\n"
        resolution = BASE + "\tindented\n"
        v = audit(BASE, ours, BASE, resolution)
        self.assertTrue(v["complete"])
        self.assertEqual(v["ours"]["survived"], 1)

    def test_an_acknowledged_loss_passes_but_is_itemized(self):
        ours = BASE + "ours-one\n"
        v = audit(BASE, ours, BASE, BASE, acknowledged=("ours-one",))
        self.assertTrue(v["complete"])
        self.assertEqual(v["ours"]["acknowledged"], ["ours-one"])
        self.assertEqual(v["ours"]["missing"], [])

    def test_an_acknowledgement_is_whitespace_insensitive(self):
        ours = BASE + "ours   one\n"
        v = audit(BASE, ours, BASE, BASE, acknowledged=("ours one",))
        self.assertTrue(v["complete"])

    def test_an_acknowledgement_matching_nothing_is_surfaced(self):
        # A misquoted acknowledgement silently fails to cover the loss it was
        # written for, so it has to be reported rather than ignored.
        ours = BASE + "ours-one\n"
        v = audit(BASE, ours, BASE, BASE, acknowledged=("typo-line",))
        self.assertFalse(v["complete"])
        self.assertEqual(v["unused_acknowledgements"], ["typo-line"])

    def test_a_line_present_in_base_is_not_a_contribution(self):
        # Only ADDED lines count. A base line both sides kept is not something
        # either parent contributed, and counting it inflates every total.
        v = audit(BASE, BASE, BASE, BASE)
        self.assertEqual(v["union_added"], 0)
        self.assertTrue(v["complete"])

    def test_deleting_a_base_line_is_not_reported_as_incomplete(self):
        # This tool checks ADDITIONS survive. A deliberate deletion of a base
        # line is a different question and must not be conflated with a loss.
        v = audit(BASE, BASE, BASE, "alpha\nbravo\n")
        self.assertTrue(v["complete"])

    def test_a_file_new_on_both_sides_has_no_base(self):
        v = audit("", "new-ours\n", "new-theirs\n", "new-ours\nnew-theirs\n")
        self.assertTrue(v["complete"])
        self.assertEqual(v["union_added"], 2)

    def test_a_line_both_sides_added_counts_for_both(self):
        ours = BASE + "shared\n"
        theirs = BASE + "shared\n"
        v = audit(BASE, ours, theirs, BASE + "shared\n")
        self.assertEqual(v["ours"]["added"], 1)
        self.assertEqual(v["theirs"]["added"], 1)
        # ...but the union counts it once, which is what "union" has to mean.
        self.assertEqual(v["union_added"], 1)


class ReportTests(unittest.TestCase):
    def test_the_headline_names_both_sides_and_the_union(self):
        v = audit(BASE, BASE + "a\n", BASE + "b\n", BASE + "a\nb\n")
        lines = report(v, "main", "branch")
        self.assertIn("main contributed 1", lines[0])
        self.assertIn("branch contributed 1", lines[0])
        self.assertIn("union 2", lines[0])
        self.assertEqual(lines[-1], "COMPLETE")

    def test_every_miss_is_itemized_individually(self):
        ours = BASE + "lost-one\nlost-two\n"
        v = audit(BASE, ours, BASE, BASE)
        lines = report(v, "main", "branch")
        self.assertIn("    - lost-one", lines)
        self.assertIn("    - lost-two", lines)
        self.assertEqual(lines[-1], "INCOMPLETE")


class GitTests(unittest.TestCase):
    """Against a real throwaway repo, since the git plumbing is half the tool."""

    def setUp(self):
        self.repo = Path(tempfile.mkdtemp())
        self._git("init", "--quiet")
        self._git("config", "user.email", "t@t.t")
        self._git("config", "user.name", "t")
        (self.repo / "list.txt").write_text(BASE, encoding="utf-8")
        self._git("add", "-A")
        self._git("commit", "--quiet", "-m", "base")
        self._git("branch", "-M", "main")
        self.base_sha = self._out("rev-parse", "HEAD")

        self._git("checkout", "--quiet", "-b", "side")
        (self.repo / "list.txt").write_text(BASE + "from-side\n", encoding="utf-8")
        self._git("commit", "--quiet", "-am", "side")

        self._git("checkout", "--quiet", "main")
        (self.repo / "list.txt").write_text(BASE + "from-main\n", encoding="utf-8")
        self._git("commit", "--quiet", "-am", "main")

    def _git(self, *args):
        subprocess.run(
            ["git", "-C", str(self.repo), *args], check=True, capture_output=True
        )

    def _out(self, *args):
        return subprocess.run(
            ["git", "-C", str(self.repo), *args],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()

    def _invoke(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            rc = run(["merge_completeness.py", *argv])
        return rc, out.getvalue(), err.getvalue()

    def test_a_complete_resolution_exits_zero(self):
        (self.repo / "list.txt").write_text(
            BASE + "from-main\nfrom-side\n", encoding="utf-8"
        )
        rc, out, _ = self._invoke(
            "--repo",
            str(self.repo),
            "--path",
            "list.txt",
            "--base",
            self.base_sha,
            "--ours",
            "main",
            "--theirs",
            "side",
        )
        self.assertEqual(rc, 0)
        self.assertIn("COMPLETE", out)

    def test_a_dropped_side_exits_one_and_names_the_line(self):
        (self.repo / "list.txt").write_text(BASE + "from-main\n", encoding="utf-8")
        rc, out, _ = self._invoke(
            "--repo",
            str(self.repo),
            "--path",
            "list.txt",
            "--base",
            self.base_sha,
            "--ours",
            "main",
            "--theirs",
            "side",
        )
        self.assertEqual(rc, 1)
        self.assertIn("from-side", out)
        self.assertIn("INCOMPLETE", out)

    def test_the_base_is_computed_when_not_given(self):
        (self.repo / "list.txt").write_text(
            BASE + "from-main\nfrom-side\n", encoding="utf-8"
        )
        rc, _, _ = self._invoke(
            "--repo",
            str(self.repo),
            "--path",
            "list.txt",
            "--ours",
            "main",
            "--theirs",
            "side",
        )
        self.assertEqual(rc, 0)

    def test_acknowledging_the_dropped_line_exits_zero(self):
        (self.repo / "list.txt").write_text(BASE + "from-main\n", encoding="utf-8")
        rc, out, _ = self._invoke(
            "--repo",
            str(self.repo),
            "--path",
            "list.txt",
            "--base",
            self.base_sha,
            "--ours",
            "main",
            "--theirs",
            "side",
            "--acknowledge",
            "from-side",
        )
        self.assertEqual(rc, 0)
        self.assertIn("acknowledged as superseded", out)

    def test_a_missing_blob_is_treated_as_empty_not_an_error(self):
        # A file added on only one side has no base content; that is a valid
        # shape and must not abort the check.
        self.assertEqual(mc.git_show(self.base_sha, "nope.txt", self.repo), "")

    def test_resolve_theirs_finds_MERGE_HEAD_during_a_conflicted_merge(self):
        # The SUCCESS branch of resolve_theirs had no coverage, so the tool's
        # own documented default invocation (`--path X` with no revisions) was
        # never exercised. A real conflicted merge is what creates MERGE_HEAD.
        merge = subprocess.run(
            ["git", "-C", str(self.repo), "merge", "side"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(merge.returncode, 0, "expected a conflicted merge")
        self.assertEqual(mc.resolve_theirs(self.repo), "MERGE_HEAD")

    def test_a_resolution_still_holding_conflict_markers_is_refused(self):
        # An unresolved file contains BOTH sides by construction, so it would
        # score COMPLETE and exit 0 — the strongest possible green at the
        # exact moment nothing has been resolved.
        subprocess.run(
            ["git", "-C", str(self.repo), "merge", "side"],
            capture_output=True,
            check=False,
        )
        with self.assertRaises(mc.MergeCompletenessError) as caught:
            self._invoke(
                "--repo",
                str(self.repo),
                "--path",
                "list.txt",
                "--base",
                self.base_sha,
                "--ours",
                "main",
                "--theirs",
                "side",
            )
        self.assertIn("conflict markers", str(caught.exception))

    def test_a_path_in_no_revision_is_refused_rather_than_vacuously_complete(self):
        (self.repo / "other.txt").write_text("x\n", encoding="utf-8")
        with self.assertRaises(mc.MergeCompletenessError) as caught:
            self._invoke(
                "--repo",
                str(self.repo),
                "--path",
                "nope.txt",
                "--base",
                self.base_sha,
                "--ours",
                "main",
                "--theirs",
                "side",
                "--resolution",
                str(self.repo / "other.txt"),
            )
        self.assertIn("check the revisions and the path", str(caught.exception))

    def test_no_merge_in_progress_is_a_clean_error(self):
        with self.assertRaises(mc.MergeCompletenessError) as caught:
            mc.resolve_theirs(self.repo)
        self.assertIn("no merge in progress", str(caught.exception))

    def test_an_unreadable_resolution_is_a_clean_error(self):
        with self.assertRaises(mc.MergeCompletenessError) as caught:
            self._invoke(
                "--repo",
                str(self.repo),
                "--path",
                "list.txt",
                "--base",
                self.base_sha,
                "--ours",
                "main",
                "--theirs",
                "side",
                "--resolution",
                "/nonexistent/x.txt",
            )
        self.assertIn("cannot read", str(caught.exception))

    def test_main_maps_an_error_to_exit_two(self):
        argv = ["merge_completeness.py", "--path", "x", "--repo", str(self.repo)]
        real = sys.argv
        try:
            sys.argv = argv
            with redirect_stderr(io.StringIO()) as err:
                rc = mc.main()
        finally:
            sys.argv = real
        self.assertEqual(rc, 2)
        self.assertIn("error:", err.getvalue())


if __name__ == "__main__":
    unittest.main()
