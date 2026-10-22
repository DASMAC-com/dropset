#!/usr/bin/env python3
"""Unit tests for ``lint_paths.py`` (stdlib ``unittest``; no pytest).

The end-to-end property under test — that an **untracked** file reaches the
hooks, which is the whole reason the tool exists — is covered by
``UntrackedFilesAreIncluded``, which builds a throwaway git repo in a temp
directory and reads the resolved list back.
"""

from __future__ import annotations

import io
import os
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest import mock

import lint_paths as lp


def _git(*args: str, cwd: str) -> None:
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


class ParseLsFiles(unittest.TestCase):
    def test_splits_on_nul(self):
        self.assertEqual(
            lp.parse_ls_files("a.rs\0b/c.md\0"),
            ["a.rs", "b/c.md"],
        )

    def test_drops_empty_trailing_field(self):
        # `-z` terminates every record, so the split always yields a final "".
        self.assertEqual(lp.parse_ls_files("only.txt\0"), ["only.txt"])

    def test_empty_input(self):
        self.assertEqual(lp.parse_ls_files(""), [])

    def test_dedupes_and_sorts(self):
        self.assertEqual(
            lp.parse_ls_files("b.txt\0a.txt\0b.txt\0"),
            ["a.txt", "b.txt"],
        )

    def test_preserves_paths_with_spaces(self):
        # The reason for `-z`: a space must not split a path.
        self.assertEqual(
            lp.parse_ls_files("dir name/file name.md\0"),
            ["dir name/file name.md"],
        )


class Existing(unittest.TestCase):
    def test_keeps_present_drops_absent(self):
        with tempfile.TemporaryDirectory() as root:
            with open(os.path.join(root, "here.txt"), "w", encoding="utf-8") as fh:
                fh.write("x")
            self.assertEqual(
                lp.existing(["here.txt", "deleted.txt"], root),
                ["here.txt"],
            )

    def test_keeps_dangling_symlink(self):
        # `lexists`, not `exists`: a tracked symlink is in CI's list too.
        with tempfile.TemporaryDirectory() as root:
            os.symlink("nowhere", os.path.join(root, "link"))
            self.assertEqual(lp.existing(["link"], root), ["link"])

    def test_empty_list(self):
        with tempfile.TemporaryDirectory() as root:
            self.assertEqual(lp.existing([], root), [])


class PreCommitCmd(unittest.TestCase):
    def test_files_come_last(self):
        self.assertEqual(
            lp.pre_commit_cmd("cfg/x.yml", [], ["a.md", "b.md"]),
            ["pre-commit", "run", "--config", "cfg/x.yml", "--files", "a.md", "b.md"],
        )

    def test_hook_id_precedes_files(self):
        # A hook id is positional, so it must land before `--files` starts
        # consuming paths.
        self.assertEqual(
            lp.pre_commit_cmd("cfg/x.yml", ["cspell"], ["a.md"]),
            ["pre-commit", "run", "--config", "cfg/x.yml", "cspell", "--files", "a.md"],
        )


class UntrackedFilesAreIncluded(unittest.TestCase):
    """The regression this tool exists for: `--all-files` would miss `new.md`."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = os.path.realpath(self._tmp.name)
        _git("init", "-q", cwd=self.root)
        _git("config", "user.email", "t@example.com", cwd=self.root)
        _git("config", "user.name", "Test", cwd=self.root)
        self._write("tracked.md", "tracked\n")
        self._write(".gitignore", "ignored.md\n")
        _git("add", "tracked.md", ".gitignore", cwd=self.root)
        _git("commit", "-q", "-m", "init", "--no-gpg-sign", cwd=self.root)

    def tearDown(self):
        self._tmp.cleanup()

    def _write(self, rel: str, body: str) -> None:
        with open(os.path.join(self.root, rel), "w", encoding="utf-8") as fh:
            fh.write(body)

    def test_untracked_included_ignored_excluded(self):
        self._write("new.md", "never git added\n")
        self._write("ignored.md", "gitignored\n")
        self.assertEqual(
            lp.lint_files(self.root),
            [".gitignore", "new.md", "tracked.md"],
        )

    def test_deleted_tracked_file_filtered_out(self):
        os.remove(os.path.join(self.root, "tracked.md"))
        self.assertEqual(lp.lint_files(self.root), [".gitignore"])

    def test_empty_file_set_exits_non_zero(self):
        # A green verdict for a run that opened no files is the failure this
        # tool exists to remove, so an empty resolve must not report success.
        # `main` returns before ever invoking pre-commit here.
        for rel in ("tracked.md", ".gitignore"):
            os.remove(os.path.join(self.root, rel))
        _git("rm", "-q", "--cached", "tracked.md", ".gitignore", cwd=self.root)
        cwd = os.getcwd()
        os.chdir(self.root)
        try:
            with redirect_stderr(io.StringIO()) as err:
                self.assertEqual(lp.main([]), 1)
        finally:
            os.chdir(cwd)
        self.assertIn("refusing to report success", err.getvalue())

    def test_clean_tree_matches_tracked_set(self):
        # With nothing untracked, the resolved set is exactly what `--all-files`
        # would have produced — the tool is a superset, never a different set.
        self.assertEqual(lp.lint_files(self.root), [".gitignore", "tracked.md"])


class ChangedNarrowsToTheBranch(unittest.TestCase):
    """``--changed``: this branch's own files, measured from the merge-base.

    Built on a throwaway repo with a real second branch, so the merge-base is
    a genuine one rather than a mocked ref.
    """

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = os.path.realpath(self._tmp.name)
        _git("init", "-q", "-b", "main", cwd=self.root)
        _git("config", "user.email", "t@example.com", cwd=self.root)
        _git("config", "user.name", "Test", cwd=self.root)
        self._write("base.md", "base\n")
        self._write("shared.md", "original\n")
        self._write(".gitignore", "ignored.md\n")
        _git("add", "-A", cwd=self.root)
        _git("commit", "-q", "-m", "init", "--no-gpg-sign", cwd=self.root)
        _git("checkout", "-q", "-b", "feature", cwd=self.root)

    def tearDown(self):
        self._tmp.cleanup()

    def _write(self, rel: str, body: str) -> None:
        with open(os.path.join(self.root, rel), "w", encoding="utf-8") as fh:
            fh.write(body)

    def test_only_the_branch_s_own_files(self):
        self._write("shared.md", "edited on the branch\n")
        _git("add", "shared.md", cwd=self.root)
        _git("commit", "-q", "-m", "edit", "--no-gpg-sign", cwd=self.root)
        self.assertEqual(lp.changed_files(self.root, "main"), ["shared.md"])

    def test_the_base_s_own_commits_are_not_reported(self):
        # The merge-base, not the ref tip: a commit that landed on `main`
        # after the branch point is not this branch's work, and diffing
        # against the tip would report it as one.
        self._write("shared.md", "edited on the branch\n")
        _git("add", "shared.md", cwd=self.root)
        _git("commit", "-q", "-m", "edit", "--no-gpg-sign", cwd=self.root)
        _git("checkout", "-q", "main", cwd=self.root)
        self._write("landed-later.md", "someone else's PR\n")
        _git("add", "landed-later.md", cwd=self.root)
        _git("commit", "-q", "-m", "theirs", "--no-gpg-sign", cwd=self.root)
        _git("checkout", "-q", "feature", cwd=self.root)
        self.assertEqual(lp.changed_files(self.root, "main"), ["shared.md"])

    def test_unstaged_edits_are_included(self):
        # The post-edit check runs before staging, so an unstaged edit is the
        # normal case rather than an edge one.
        self._write("shared.md", "not staged\n")
        self.assertEqual(lp.changed_files(self.root, "main"), ["shared.md"])

    def test_untracked_included_ignored_excluded(self):
        # An untracked file is by definition the branch's work, and is the
        # class the whole-tree resolver exists to stop losing.
        self._write("new.md", "never git added\n")
        self._write("ignored.md", "gitignored\n")
        self.assertEqual(lp.changed_files(self.root, "main"), ["new.md"])

    def test_deleted_file_is_filtered_out(self):
        # `git diff` reports a deletion, but pre-commit fails on a path that
        # isn't there — the same filter the whole-tree resolver applies.
        os.remove(os.path.join(self.root, "base.md"))
        self.assertEqual(lp.changed_files(self.root, "main"), [])

    def test_nothing_changed_reports_success(self):
        # Unlike the whole-tree case, an empty changed set is a real answer.
        cwd = os.getcwd()
        os.chdir(self.root)
        try:
            with redirect_stdout(io.StringIO()) as out:
                self.assertEqual(lp.main(["--changed", "--base", "main"]), 0)
        finally:
            os.chdir(cwd)
        self.assertIn("nothing changed", out.getvalue())

    def test_an_unknown_base_ref_raises_rather_than_widening(self):
        # A missing `origin/main` means the caller hasn't fetched. Falling back
        # to the whole tree would turn that into a surprise full sweep.
        with self.assertRaises(subprocess.CalledProcessError):
            lp.changed_files(self.root, "origin/nonexistent")


class FilesFlag(unittest.TestCase):
    """``--files`` takes exactly the named paths, so the prose stops teaching
    the hand-spelled pre-commit form."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = os.path.realpath(self._tmp.name)
        _git("init", "-q", cwd=self.root)
        for name in ("a.md", "b.md"):
            with open(os.path.join(self.root, name), "w", encoding="utf-8") as fh:
                fh.write("x\n")
        cwd = os.getcwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, cwd)

    def test_the_named_existing_paths_only(self):
        with redirect_stdout(io.StringIO()) as out:
            code = lp.main(["--files", "b.md", "gone.md", "--print"])
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue().split(), ["b.md"])

    def test_a_hook_id_follows_the_separator(self):
        with mock.patch.object(lp, "lint", return_value=0) as lint:
            lp.main(["--files", "a.md", "--", "cspell"])
        self.assertEqual(lint.call_args.args[:2], (lp.DEFAULT_CONFIG, ["cspell"]))
        self.assertEqual(lint.call_args.args[2], ["a.md"])

    def test_no_existing_path_is_a_failure(self):
        with redirect_stderr(io.StringIO()):
            self.assertEqual(lp.main(["--files", "gone.md"]), 1)

    def test_files_and_changed_are_exclusive(self):
        with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            lp.main(["--files", "a.md", "--changed"])


class FixThenFail(unittest.TestCase):
    """A rewriting hook fails by design: one automatic re-run, and only its
    result counts."""

    def _lint(self, runs):
        with (
            mock.patch.object(lp, "ensure_frontend_hooks", return_value=[]),
            mock.patch.object(lp, "run_pre_commit", side_effect=runs) as rpc,
            redirect_stderr(io.StringIO()) as err,
        ):
            code = lp.lint("cfg.yml", [], ["a.md"], "/root")
        return code, rpc.call_count, err.getvalue()

    def test_a_rewrite_then_pass_is_a_pass(self):
        code, calls, err = self._lint([(1, True), (0, False)])
        self.assertEqual((code, calls), (0, 2))
        self.assertIn("re-running once", err)

    def test_a_second_failure_is_real(self):
        code, calls, _ = self._lint([(1, True), (1, True)])
        self.assertEqual((code, calls), (1, 2))

    def test_a_failure_without_a_rewrite_is_not_re_run(self):
        code, calls, _ = self._lint([(1, False)])
        self.assertEqual((code, calls), (1, 1))

    def test_a_pass_is_not_re_run(self):
        code, calls, _ = self._lint([(0, False)])
        self.assertEqual((code, calls), (0, 1))

    def test_the_marker_is_read_from_pre_commit_output(self):
        script = (
            "import sys; print('mdformat...Failed'); "
            f"print('- {lp.MODIFIED_MARKER}'); sys.exit(1)"
        )
        with redirect_stdout(io.StringIO()) as out:
            code, modified = lp.run_pre_commit(
                [sys.executable, "-c", script], os.getcwd()
            )
        self.assertEqual((code, modified), (1, True))
        # The output is mirrored, not swallowed.
        self.assertIn("mdformat...Failed", out.getvalue())


class FrontendHooks(unittest.TestCase):
    """A missing ``biome`` / ``tsc``: install, else skip and say unverifiable."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = self._tmp.name
        self.bin = os.path.join(self.root, "frontend", "node_modules", ".bin")

    def _install_bins(self, *names):
        os.makedirs(self.bin, exist_ok=True)
        for name in names:
            open(os.path.join(self.bin, name), "w").close()

    def test_no_frontend_path_needs_nothing(self):
        self.assertEqual(lp.missing_frontend_hooks(self.root, [], ["a.rs"]), [])

    def test_a_ts_path_without_node_modules_misses_both(self):
        self.assertEqual(
            lp.missing_frontend_hooks(self.root, [], ["sdk/ts/x.ts"]),
            ["biome", "tsc"],
        )

    def test_a_non_frontend_hook_id_needs_nothing(self):
        self.assertEqual(lp.missing_frontend_hooks(self.root, ["cspell"], ["x.ts"]), [])

    def test_an_install_that_provides_the_bins_clears_them(self):
        def install(*_a, **_k):
            self._install_bins("biome", "tsc")
            return subprocess.CompletedProcess([], 0, "", "")

        with (
            mock.patch.object(lp.subprocess, "run", side_effect=install) as run,
            redirect_stderr(io.StringIO()),
        ):
            self.assertEqual(lp.ensure_frontend_hooks(self.root, [], ["x.ts"]), [])
        self.assertEqual(run.call_args.args[0], lp._FRONTEND_INSTALL)

    def test_a_failed_install_skips_and_reports_unverifiable(self):
        failed = subprocess.CompletedProcess([], 1, "", "ERR_PNPM offline")
        with (
            mock.patch.object(lp.subprocess, "run", return_value=failed),
            mock.patch.object(lp, "run_pre_commit", return_value=(0, False)) as rpc,
            redirect_stderr(io.StringIO()) as err,
        ):
            code = lp.lint("cfg.yml", [], ["x.ts"], self.root)
        self.assertEqual(code, 0)
        env = rpc.call_args.args[2]
        self.assertEqual(env["SKIP"].split(",")[-2:], ["biome", "tsc"])
        self.assertIn("UNVERIFIABLE locally: biome, tsc", err.getvalue())
        self.assertIn("ERR_PNPM offline", err.getvalue())


if __name__ == "__main__":
    unittest.main()
