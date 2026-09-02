#!/usr/bin/env python3
"""Unit tests for resolve_session.py, over a real throwaway project tree.

The fixture reproduces the measured failure: a `-w`-launched session whose
transcript sits under the BASE repo's slug while its `cwd` stamps point into the
worktree, and no worktree project directory exists at all.
"""

from __future__ import annotations

import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import resolve_session as rs  # noqa: E402
from resolve_session import (  # noqa: E402
    ResolveSessionError,
    normalize_tag,
    resolve,
    run,
    slugify,
    stamps_into,
)


def _record(cwd):
    return json.dumps({"type": "user", "cwd": cwd})


class Fixture:
    """A throwaway `~/.claude/projects` tree plus a repo with one worktree."""

    def __init__(self):
        root = Path(tempfile.mkdtemp())
        self.home = root / "home"
        self.repo = root / "repos" / "dropset"
        self.projects = self.home / ".claude" / "projects"
        self.projects.mkdir(parents=True)
        self.repo.mkdir(parents=True)
        os.environ["CLAUDE_CONFIG_DIR"] = str(self.home / ".claude")

    def worktree(self, tag):
        wt = self.repo / ".claude" / "worktrees" / tag
        wt.mkdir(parents=True, exist_ok=True)
        return wt

    def transcript(self, slug_path, session_id, cwd_list):
        slug = self.projects / slugify(slug_path)
        slug.mkdir(parents=True, exist_ok=True)
        path = slug / f"{session_id}.jsonl"
        path.write_text(
            "\n".join(_record(c) for c in cwd_list) + "\n", encoding="utf-8"
        )
        return path


class SlugAndTagTests(unittest.TestCase):
    def test_slugify_replaces_slashes_and_dots(self):
        self.assertEqual(slugify(Path("/a/b.c/d")), "-a-b-c-d")

    def test_a_bare_number_gets_the_prefix(self):
        self.assertEqual(normalize_tag("1051"), "eng-1051")

    def test_an_already_prefixed_tag_is_unchanged(self):
        self.assertEqual(normalize_tag("eng-1051"), "eng-1051")

    def test_an_uppercase_tag_is_normalized(self):
        self.assertEqual(normalize_tag("ENG-1051"), "eng-1051")

    def test_an_empty_tag_is_refused(self):
        with self.assertRaises(ResolveSessionError):
            normalize_tag("   ")


class StampsIntoTests(unittest.TestCase):
    def setUp(self):
        self.fx = Fixture()
        self.wt = self.fx.worktree("eng-1")

    def test_a_matching_cwd_is_detected(self):
        t = self.fx.transcript(self.fx.repo, "s1", [str(self.wt)])
        self.assertTrue(stamps_into(t, self.wt))

    def test_a_nested_cwd_counts(self):
        t = self.fx.transcript(self.fx.repo, "s2", [str(self.wt / "program")])
        self.assertTrue(stamps_into(t, self.wt))

    def test_a_sibling_worktree_does_not_match(self):
        other = self.fx.worktree("eng-2")
        t = self.fx.transcript(self.fx.repo, "s3", [str(other)])
        self.assertFalse(stamps_into(t, self.wt))

    def test_a_prefix_collision_does_not_match(self):
        # `eng-1` must not match `eng-10` — a plain startswith without the
        # separator would, and would resume the wrong session.
        ten = self.fx.worktree("eng-10")
        t = self.fx.transcript(self.fx.repo, "s4", [str(ten)])
        self.assertFalse(stamps_into(t, self.wt))

    def test_a_malformed_line_is_skipped_not_fatal(self):
        slug = self.fx.projects / slugify(self.fx.repo)
        slug.mkdir(parents=True, exist_ok=True)
        path = slug / "s5.jsonl"
        path.write_text(
            "not json at all\n" + _record(str(self.wt)) + "\n", encoding="utf-8"
        )
        self.assertTrue(stamps_into(path, self.wt))

    def test_a_truncated_final_record_is_tolerated(self):
        slug = self.fx.projects / slugify(self.fx.repo)
        slug.mkdir(parents=True, exist_ok=True)
        path = slug / "s6.jsonl"
        path.write_text(
            _record(str(self.wt)) + '\n{"type": "user", "cw', encoding="utf-8"
        )
        self.assertTrue(stamps_into(path, self.wt))

    def test_the_scan_is_bounded(self):
        # A stamp past the cap is deliberately NOT found: the head is where a
        # `-w` launch writes it, and unbounded scanning is what this avoids.
        filler = [str(self.fx.repo)] * (rs.CWD_SCAN_LINES + 5)
        t = self.fx.transcript(self.fx.repo, "s7", filler + [str(self.wt)])
        self.assertFalse(stamps_into(t, self.wt))

    def test_an_unreadable_file_is_false_not_an_exception(self):
        self.assertFalse(stamps_into(Path("/nonexistent/x.jsonl"), self.wt))


class ResolveTests(unittest.TestCase):
    def setUp(self):
        self.fx = Fixture()

    def test_a_worktree_with_its_own_transcript_uses_continue(self):
        wt = self.fx.worktree("eng-1024")
        self.fx.transcript(wt, "own", [str(wt)])
        v = resolve("eng-1024", self.fx.repo)
        self.assertEqual(v["mode"], "continue")
        self.assertEqual(v["run_from"], str(wt))

    def test_the_measured_bug_resolves_to_resume_by_id(self):
        # The ENG-1051 shape: worktree present, no worktree project dir, the
        # transcript filed under the base slug with worktree cwd stamps.
        wt = self.fx.worktree("eng-1051")
        self.fx.transcript(self.fx.repo, "afce0c54", [str(wt)])
        v = resolve("eng-1051", self.fx.repo)
        self.assertEqual(v["mode"], "resume")
        self.assertEqual(v["session_id"], "afce0c54")
        self.assertEqual(v["run_from"], str(self.fx.repo))

    def test_a_worktree_transcript_wins_over_a_base_one(self):
        wt = self.fx.worktree("eng-7")
        self.fx.transcript(self.fx.repo, "base", [str(wt)])
        self.fx.transcript(wt, "own", [str(wt)])
        self.assertEqual(resolve("eng-7", self.fx.repo)["mode"], "continue")

    def test_a_base_transcript_for_another_worktree_is_not_offered(self):
        self.fx.worktree("eng-8")
        other = self.fx.worktree("eng-9")
        self.fx.transcript(self.fx.repo, "wrong", [str(other)])
        v = resolve("eng-8", self.fx.repo)
        self.assertEqual(v["mode"], "picker")
        self.assertIsNone(v["session_id"])

    def test_a_transcript_under_an_unrelated_slug_is_still_found(self):
        wt = self.fx.worktree("eng-11")
        self.fx.transcript(Path("/somewhere/else"), "stray", [str(wt)])
        v = resolve("eng-11", self.fx.repo)
        self.assertEqual(v["mode"], "resume")
        self.assertEqual(v["session_id"], "stray")

    def test_a_missing_worktree_falls_back_to_the_picker(self):
        v = resolve("eng-999", self.fx.repo)
        self.assertFalse(v["worktree_exists"])
        self.assertEqual(v["mode"], "picker")
        self.assertIn("pruned", v["reason"])

    def test_a_worktree_with_no_session_anywhere_says_so(self):
        self.fx.worktree("eng-12")
        v = resolve("eng-12", self.fx.repo)
        self.assertEqual(v["mode"], "picker")
        self.assertIn("never have started", v["reason"])

    def test_the_newest_matching_transcript_wins(self):
        wt = self.fx.worktree("eng-13")
        old = self.fx.transcript(self.fx.repo, "older", [str(wt)])
        new = self.fx.transcript(self.fx.repo, "newer", [str(wt)])
        os.utime(old, (1, 1))
        os.utime(new, (10**9, 10**9))
        self.assertEqual(resolve("eng-13", self.fx.repo)["session_id"], "newer")


class CliTests(unittest.TestCase):
    def setUp(self):
        self.fx = Fixture()

    def _invoke(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            rc = run(["resolve_session.py", *argv])
        return rc, out.getvalue(), err.getvalue()

    def test_a_resolvable_session_exits_zero_with_json(self):
        wt = self.fx.worktree("eng-20")
        self.fx.transcript(self.fx.repo, "sid20", [str(wt)])
        rc, out, _ = self._invoke("--tag", "20", "--repo", str(self.fx.repo))
        self.assertEqual(rc, 0)
        self.assertEqual(json.loads(out)["session_id"], "sid20")

    def test_a_symlinked_repo_path_is_not_resolved_away(self):
        # macOS `/var` and `/tmp` are symlinks, and the slug is derived from the
        # cwd STRING — so resolving the path yields a slug for a directory that
        # does not exist, and every lookup misses while looking correct.
        wt = self.fx.worktree("eng-21")
        self.fx.transcript(self.fx.repo, "sid21", [str(wt)])
        symlinked = str(self.fx.repo).replace("/private/var", "/var", 1)
        rc, out, _ = self._invoke("--tag", "21", "--repo", symlinked)
        self.assertEqual(rc, 0)
        self.assertEqual(json.loads(out)["session_id"], "sid21")

    def test_an_unresolvable_tag_exits_one(self):
        rc, out, _ = self._invoke("--tag", "404", "--repo", str(self.fx.repo))
        self.assertEqual(rc, 1)
        self.assertEqual(json.loads(out)["mode"], "picker")

    def test_an_empty_tag_exits_two_through_main(self):
        argv = ["resolve_session.py", "--tag", " ", "--repo", str(self.fx.repo)]
        real = sys.argv
        try:
            sys.argv = argv
            with redirect_stderr(io.StringIO()) as err:
                rc = rs.main()
        finally:
            sys.argv = real
        self.assertEqual(rc, 2)
        self.assertIn("error:", err.getvalue())


if __name__ == "__main__":
    unittest.main()
