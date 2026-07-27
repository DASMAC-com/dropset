#!/usr/bin/env python3
"""Unit tests for ``init_pr_branch.py`` (stdlib ``unittest``; no pytest)."""

from __future__ import annotations

import io
import json
import os
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

import init_pr_branch as ipb

PORCELAIN = """\
worktree /Users/alex/repos/dropset
HEAD 8fd8d470f85fe01073a417b25351c840df313c60
branch refs/heads/main

worktree /Users/alex/repos/dropset/.claude/worktrees/eng-603
HEAD 8da1695aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
branch refs/heads/worktree-eng-603
"""

PORCELAIN_NO_MAIN = """\
worktree /Users/alex/repos/dropset/.claude/worktrees/eng-603
HEAD 8da1695aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
branch refs/heads/eng-603
"""


class ParseBaseRepo(unittest.TestCase):
    def test_finds_main_worktree(self):
        self.assertEqual(ipb.parse_base_repo(PORCELAIN), "/Users/alex/repos/dropset")

    def test_none_when_no_main(self):
        self.assertIsNone(ipb.parse_base_repo(PORCELAIN_NO_MAIN))

    def test_detached_head_stanza_is_ignored(self):
        # A detached worktree has no `branch` line; it must not be misread as base.
        porcelain = "worktree /tmp/detached\nHEAD abc123\ndetached\n\n" + PORCELAIN
        self.assertEqual(ipb.parse_base_repo(porcelain), "/Users/alex/repos/dropset")


class NormalizeTag(unittest.TestCase):
    def test_valid_lowercase(self):
        self.assertEqual(ipb.normalize_tag("eng-603"), "eng-603")

    def test_valid_uppercase_normalized(self):
        self.assertEqual(ipb.normalize_tag("ENG-12"), "eng-12")

    def test_invalid(self):
        self.assertIsNone(ipb.normalize_tag("feature-x"))
        self.assertIsNone(ipb.normalize_tag("eng-"))
        self.assertIsNone(ipb.normalize_tag("eng-12a"))
        self.assertIsNone(ipb.normalize_tag(""))


class NormalizeBranch(unittest.TestCase):
    def test_strips_worktree_prefix(self):
        self.assertEqual(ipb.normalize_branch("worktree-eng-603"), ("eng-603", True))

    def test_bare_tag_is_noop(self):
        self.assertEqual(ipb.normalize_branch("eng-603"), ("eng-603", False))

    def test_other_name_is_noop(self):
        self.assertEqual(ipb.normalize_branch("main"), ("main", False))


class LinkEnv(unittest.TestCase):
    """``--link-env``'s four outcomes, plus the never-clobber invariant.

    Each case builds a throwaway base repo / worktree pair on a real
    filesystem — ``os.symlink`` is the behavior under test, so it isn't mocked.
    """

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        root = Path(self._tmp.name)
        self.base = root / "base"
        self.worktree = root / "worktree"
        (self.base / "frontend").mkdir(parents=True)
        (self.worktree / "frontend").mkdir(parents=True)

    @property
    def source(self) -> Path:
        return self.base / "frontend" / ".env.local"

    @property
    def dest(self) -> Path:
        return self.worktree / "frontend" / ".env.local"

    def test_created_when_base_has_env_and_worktree_does_not(self):
        self.source.write_text("KEY=value\n", encoding="utf-8")
        self.assertEqual(ipb.link_env(str(self.base), str(self.worktree)), "created")
        self.assertTrue(self.dest.is_symlink())
        self.assertEqual(os.readlink(self.dest), str(self.source))
        self.assertEqual(self.dest.read_text(encoding="utf-8"), "KEY=value\n")

    def test_no_source_when_base_has_no_env(self):
        self.assertEqual(ipb.link_env(str(self.base), str(self.worktree)), "no-source")
        self.assertFalse(self.dest.exists())

    def test_no_source_when_worktree_has_no_frontend_dir(self):
        self.source.write_text("KEY=value\n", encoding="utf-8")
        (self.worktree / "frontend").rmdir()
        self.assertEqual(ipb.link_env(str(self.base), str(self.worktree)), "no-source")

    def test_no_base_when_main_is_not_checked_out_anywhere(self):
        self.source.write_text("KEY=value\n", encoding="utf-8")
        self.assertEqual(ipb.link_env(None, str(self.worktree)), "no-base")
        self.assertFalse(self.dest.exists())

    def test_exists_never_clobbers_a_real_file(self):
        # The invariant: a file someone placed deliberately survives untouched.
        self.source.write_text("FROM_BASE=1\n", encoding="utf-8")
        self.dest.write_text("DELIBERATE=1\n", encoding="utf-8")
        self.assertEqual(ipb.link_env(str(self.base), str(self.worktree)), "exists")
        self.assertFalse(self.dest.is_symlink())
        self.assertEqual(self.dest.read_text(encoding="utf-8"), "DELIBERATE=1\n")

    def test_failed_when_the_link_cannot_be_created(self):
        # An unwritable frontend/ must not raise: the caller evaluates this
        # while building the JSON the skill's other answers ride in.
        self.source.write_text("KEY=value\n", encoding="utf-8")
        frontend = self.worktree / "frontend"
        frontend.chmod(0o500)
        self.addCleanup(frontend.chmod, 0o700)
        self.assertEqual(ipb.link_env(str(self.base), str(self.worktree)), "failed")

    def test_exists_leaves_a_dangling_symlink_as_found(self):
        # `lexists`, not `exists` — an occupied path is occupied either way.
        self.source.write_text("FROM_BASE=1\n", encoding="utf-8")
        self.dest.symlink_to(self.base / "frontend" / "gone.env")
        self.assertEqual(ipb.link_env(str(self.base), str(self.worktree)), "exists")
        self.assertEqual(
            os.readlink(self.dest), str(self.base / "frontend" / "gone.env")
        )


class MainCli(unittest.TestCase):
    """Drive ``main()`` through its ``--porcelain-file`` / ``--branch``
    overrides so no real git is invoked.
    """

    def _run(
        self,
        tag: str,
        branch: str,
        porcelain: str,
        extra: list[str] | None = None,
    ):
        with tempfile.TemporaryDirectory() as tmp:
            pfile = Path(tmp) / "wt.txt"
            pfile.write_text(porcelain, encoding="utf-8")
            buf = io.StringIO()
            with redirect_stdout(buf):
                code = ipb.main(
                    ["--tag", tag, "--branch", branch, "--porcelain-file", str(pfile)]
                    + (extra or [])
                )
            return code, json.loads(buf.getvalue())

    def test_worktree_branch_resolves_and_normalizes(self):
        code, out = self._run("ENG-603", "worktree-eng-603", PORCELAIN)
        self.assertEqual(code, 0)
        self.assertEqual(out["tag"], "eng-603")
        self.assertTrue(out["tag_valid"])
        self.assertEqual(out["base_repo"], "/Users/alex/repos/dropset")
        self.assertEqual(out["normalized_branch"], "eng-603")
        self.assertTrue(out["rename_needed"])

    def test_invalid_tag_exits_nonzero(self):
        code, out = self._run("not-a-tag", "eng-603", PORCELAIN)
        self.assertEqual(code, 1)
        self.assertFalse(out["tag_valid"])
        self.assertIsNone(out["tag"])

    def test_env_link_is_null_without_the_flag(self):
        # The key is always present so the skill can read one stable shape.
        _, out = self._run("eng-603", "worktree-eng-603", PORCELAIN)
        self.assertIn("env_link", out)
        self.assertIsNone(out["env_link"])

    def test_env_link_reports_its_outcome_with_the_flag(self):
        # End-to-end through the CLI: a temp base repo holding an env file, a
        # temp worktree without one. The porcelain names that temp base, so
        # the case never depends on a real checkout being present.
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp) / "base"
            worktree = Path(tmp) / "worktree"
            (base / "frontend").mkdir(parents=True)
            (worktree / "frontend").mkdir(parents=True)
            (base / "frontend" / ".env.local").write_text("K=v\n", encoding="utf-8")
            porcelain = f"worktree {base}\nHEAD abc123\nbranch refs/heads/main\n"
            _, out = self._run(
                "eng-603",
                "worktree-eng-603",
                porcelain,
                ["--link-env", "--worktree-root", str(worktree)],
            )
            self.assertEqual(out["env_link"], "created")
            self.assertTrue((worktree / "frontend" / ".env.local").is_symlink())

    def test_env_link_reports_no_base_when_main_is_absent(self):
        # Isolate the root like every sibling case, so the run can never reach
        # the real checkout even if link_env's guard order changes.
        with tempfile.TemporaryDirectory() as wt:
            _, out = self._run(
                "eng-603",
                "eng-603",
                PORCELAIN_NO_MAIN,
                ["--link-env", "--worktree-root", wt],
            )
        self.assertEqual(out["env_link"], "no-base")

    def test_env_link_is_skipped_on_an_invalid_tag(self):
        # A run that fails validation stops the skill, so it must not leave a
        # filesystem mutation behind.
        with tempfile.TemporaryDirectory() as wt:
            frontend = Path(wt) / "frontend"
            frontend.mkdir()
            code, out = self._run(
                "not-a-tag",
                "eng-603",
                PORCELAIN,
                ["--link-env", "--worktree-root", wt],
            )
            self.assertEqual(code, 1)
            self.assertIsNone(out["env_link"])
            self.assertFalse((frontend / ".env.local").exists())


if __name__ == "__main__":
    unittest.main()
