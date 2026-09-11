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
from unittest import mock

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
    """A throwaway `~/.claude/projects` tree plus a repo with one worktree.

    Takes the TestCase so the `CLAUDE_CONFIG_DIR` override is registered for
    cleanup. Mutating `os.environ` unguarded leaked the temp path into every
    module loaded after this one in the single `make tools-tests` process —
    and `resolve_session` itself names another reader of that variable
    (`prune_conversations.py`), so a green suite was only green by
    alphabetical load order rather than by isolation.
    """

    def __init__(self, case: unittest.TestCase):
        root = Path(tempfile.mkdtemp())
        self.home = root / "home"
        self.repo = root / "repos" / "dropset"
        self.projects = self.home / ".claude" / "projects"
        self.projects.mkdir(parents=True)
        self.repo.mkdir(parents=True)
        patch = mock.patch.dict(
            os.environ, {"CLAUDE_CONFIG_DIR": str(self.home / ".claude")}
        )
        patch.start()
        case.addCleanup(patch.stop)

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
        self.fx = Fixture(self)
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
        self.fx = Fixture(self)

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

    def test_a_nested_sub_agent_transcript_is_not_offered(self):
        # `transcripts_in` globs TOP-LEVEL *.jsonl only, and its docstring
        # states that as a safety property — a sub-agent transcript is not a
        # session a human can resume, so offering one is a wrong answer rather
        # than a missing one. Nothing pinned it: switching `glob` to `rglob`
        # kept every test green while `task resume` began offering sub-agent ids.
        wt = self.fx.worktree("eng-14")
        slug = self.fx.projects / slugify(wt)
        nested = slug / "subagents"
        nested.mkdir(parents=True)
        (nested / "sub.jsonl").write_text(_record(str(wt)) + "\n", encoding="utf-8")
        v = resolve("eng-14", self.fx.repo)
        self.assertNotEqual(v["session_id"], "sub")
        self.assertEqual(v["mode"], "picker")

    def test_the_newest_matching_transcript_wins(self):
        wt = self.fx.worktree("eng-13")
        old = self.fx.transcript(self.fx.repo, "older", [str(wt)])
        new = self.fx.transcript(self.fx.repo, "newer", [str(wt)])
        os.utime(old, (1, 1))
        os.utime(new, (10**9, 10**9))
        self.assertEqual(resolve("eng-13", self.fx.repo)["session_id"], "newer")


class CliTests(unittest.TestCase):
    def setUp(self):
        self.fx = Fixture(self)

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

    def test_the_lines_format_prints_three_fields_in_order(self):
        # `--format lines` is the format the shipping caller (`task resume`) reads
        # positionally, and it had no coverage at all: swapping the print
        # order, or dropping the `or ""`, left every test green while `task resume`
        # resumed from the wrong directory or with a literal "None".
        wt = self.fx.worktree("eng-22")
        self.fx.transcript(self.fx.repo, "sid22", [str(wt)])
        rc, out, _ = self._invoke(
            "--tag", "22", "--repo", str(self.fx.repo), "--format", "lines"
        )
        self.assertEqual(rc, 0)
        self.assertEqual(out.splitlines(), ["resume", "sid22", str(self.fx.repo)])

    def test_the_lines_format_keeps_three_lines_when_there_is_no_id(self):
        # The line count must be CONSTANT, or a caller doing three reads has
        # its third read consume a missing second.
        self.fx.worktree("eng-23")
        rc, out, _ = self._invoke(
            "--tag", "23", "--repo", str(self.fx.repo), "--format", "lines"
        )
        self.assertEqual(rc, 1)
        self.assertEqual(out.splitlines(), ["picker", "", str(self.fx.repo)])

    def test_a_tag_with_a_path_separator_is_refused(self):
        with self.assertRaises(ResolveSessionError):
            normalize_tag("../../etc")

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


class DailySessionIdTests(unittest.TestCase):
    """The daily id is computed, never searched.

    A planning session that lists the Claude projects directory to find its own
    transcript pays ≈6.0k for one call. The id is an md5 of the launcher's own
    seed, so it is a one-line computation.
    """

    #: Pinned against `md5 -qs 'dropset-plan-20260910'`, run directly. This is the
    #: parity that matters: `_ds_daily_sid` in `.claude/shell/init.zsh` is what
    #: actually names the session at launch, so a drift here means the tool
    #: confidently reports an id no session ever had.
    def test_the_seed_matches_the_shell_launcher_byte_for_byte(self):
        self.assertEqual(
            rs.daily_session_id("plan", "20260910"),
            "90195a08-2348-adba-e976-f4b4a7aa6bf9",
        )

    def test_the_shape_is_a_uuid(self):
        got = rs.daily_session_id("housekeeping", "20260101")
        self.assertRegex(got, r"^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$")

    def test_the_kind_and_the_date_both_change_the_id(self):
        # The kind keeps a day's planning and housekeeping sessions apart; the
        # full date keeps `plan-18` in August from colliding with September's.
        base = rs.daily_session_id("plan", "20260910")
        self.assertNotEqual(base, rs.daily_session_id("housekeeping", "20260910"))
        self.assertNotEqual(base, rs.daily_session_id("plan", "20260911"))

    def test_the_cli_prints_just_the_id(self):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            rc = rs.run(
                ["resolve_session.py", "--daily-id", "plan", "--date", "20260910"]
            )
        self.assertEqual(rc, 0)
        self.assertEqual(out.getvalue().strip(), "90195a08-2348-adba-e976-f4b4a7aa6bf9")

    def test_daily_id_needs_no_tag(self):
        # The two modes are independent; requiring --tag here would defeat it.
        out = io.StringIO()
        with redirect_stdout(out):
            rc = rs.run(["resolve_session.py", "--daily-id", "plan"])
        self.assertEqual(rc, 0)
        self.assertTrue(out.getvalue().strip())

    def test_the_resolution_path_still_requires_a_tag(self):
        with self.assertRaises(rs.ResolveSessionError):
            rs.run(["resolve_session.py"])

    def test_an_unknown_KIND_is_refused_rather_than_hashed(self):
        """A wrong kind is undetectable downstream, so it has to be caught here.

        Every string hashes to a well-formed UUID, so the wrong one prints
        something indistinguishable from an answer and names a session that never
        existed — which the caller then reads as a lost transcript. `architect` is
        included because it is the plausible wrong guess: it *is* a seat verb, but
        it is seeded by topic rather than by day.
        """
        for kind in ("Plan", "PLAN", "plan-18", "architect", "explore", ""):
            with self.assertRaises(rs.ResolveSessionError, msg=kind):
                rs.daily_session_id(kind, "20260910")

    def test_a_malformed_DATE_is_refused(self):
        for date in ("2026-09-10", "20260910 ", "260910", "2026/09/10", "today", ""):
            with self.assertRaises(rs.ResolveSessionError, msg=date):
                rs.daily_session_id("plan", date)

    def test_both_launcher_kinds_are_accepted(self):
        # The other half: the guard must not reject what the launcher really seeds.
        for kind in rs.DAILY_KINDS:
            self.assertRegex(
                rs.daily_session_id(kind, "20260910"),
                r"^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$",
            )

    def test_DAILY_KINDS_matches_the_launcher(self):
        # Pinned against the `_ds_daily_session` callers in
        # `.claude/shell/init.zsh`. If a third daily verb is added there, this
        # fails and points at the list that needs it.
        self.assertEqual(rs.DAILY_KINDS, ("plan", "housekeeping"))


if __name__ == "__main__":
    unittest.main()
