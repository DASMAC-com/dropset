"""Tests for ``_ds_worktree_ensure`` in ``.claude/shell/init.zsh``: every
issue-keyed verb's worktree is the bare number on branch ``eng-<n>``.

The helper is sourced from a copy of the file placed in a throwaway repo (the
file derives its repo root from its own location), so the real checkout's
worktrees are never touched. Skipped where ``zsh`` or ``git`` is absent.
"""

from __future__ import annotations

import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

INIT = Path(__file__).resolve().parents[2] / "shell" / "init.zsh"


def git(*args: str, cwd: Path) -> str:
    return subprocess.run(
        ["git", *args], cwd=cwd, capture_output=True, text=True, check=True
    ).stdout


@unittest.skipUnless(shutil.which("zsh") and shutil.which("git"), "needs zsh + git")
class WorktreeEnsureTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name).resolve()
        origin = root / "origin.git"
        self.repo = root / "repo"
        git("init", "--bare", "-b", "main", str(origin), cwd=root)
        git("init", "-b", "main", str(self.repo), cwd=root)
        for key, value in (("user.name", "t"), ("user.email", "t@t")):
            git("config", key, value, cwd=self.repo)
        git("config", "commit.gpgsign", "false", cwd=self.repo)
        git("commit", "--allow-empty", "-m", "root", cwd=self.repo)
        git("remote", "add", "origin", str(origin), cwd=self.repo)
        git("push", "-q", "origin", "main", cwd=self.repo)
        shell = self.repo / ".claude" / "shell"
        shell.mkdir(parents=True)
        shutil.copy(INIT, shell / "init.zsh")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def ensure(self, tag: str, fn: str = "_ds_worktree_ensure"):
        script = f'source "$1" && {fn} "$2"'
        init = str(self.repo / ".claude" / "shell" / "init.zsh")
        return subprocess.run(
            ["zsh", "-fc", script, "zsh", init, tag],
            cwd=self.repo,
            capture_output=True,
            text=True,
            env={"PATH": "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin"},
            check=False,
        )

    def branch_of(self, tag: str) -> str:
        wt = self.repo / ".claude" / "worktrees" / tag
        return git("rev-parse", "--abbrev-ref", "HEAD", cwd=wt).strip()

    def test_a_number_gets_a_bare_directory_on_an_untracked_eng_branch(self) -> None:
        self.assertEqual(self.ensure("1957").returncode, 0)
        self.assertEqual(self.branch_of("1957"), "eng-1957")
        upstream = subprocess.run(
            ["git", "rev-parse", "--abbrev-ref", "eng-1957@{upstream}"],
            cwd=self.repo,
            capture_output=True,
            text=True,
            check=False,
        )
        # `--no-track`: main must not become the branch's upstream.
        self.assertNotEqual(upstream.returncode, 0)

    def test_an_existing_eng_branch_is_checked_out_not_recreated(self) -> None:
        git("branch", "eng-1958", "main", cwd=self.repo)
        self.assertEqual(self.ensure("1958").returncode, 0)
        self.assertEqual(self.branch_of("1958"), "eng-1958")

    def test_an_existing_directory_and_a_worded_tag_are_left_alone(self) -> None:
        (self.repo / ".claude" / "worktrees" / "1959").mkdir(parents=True)
        self.assertEqual(self.ensure("1959").returncode, 0)
        self.assertEqual(self.ensure("exp-audit").returncode, 0)
        branches = git("branch", "--format=%(refname:short)", cwd=self.repo).split()
        self.assertEqual(branches, ["main"])

    def test_an_advisor_marks_only_a_tree_it_creates_or_already_owns(self) -> None:
        may = "_ds_advisor_may_mark"
        # No worktree yet: this launch creates it.
        self.assertEqual(self.ensure("1960", may).returncode, 0)
        # An unmarked tree another launch made: never.
        (self.repo / ".claude" / "worktrees" / "1960").mkdir(parents=True)
        self.assertNotEqual(self.ensure("1960", may).returncode, 0)
        # A reopen of a tree already marked advisor.
        markers = self.repo / ".claude" / "session-substrate"
        markers.mkdir(parents=True)
        (markers / "1960.tier").write_text("advisor\n")
        self.assertEqual(self.ensure("1960", may).returncode, 0)


if __name__ == "__main__":
    unittest.main()
