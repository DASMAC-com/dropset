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

    def test_the_named_existing_paths_only_and_the_rest_named(self):
        with (
            redirect_stdout(io.StringIO()) as out,
            redirect_stderr(io.StringIO()) as err,
        ):
            code = lp.main(["--files", "b.md", "gone.md", "--print"])
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue().split(), ["b.md"])
        # A dropped path is said out loud, never a silent shrink to a pass.
        self.assertIn("gone.md", err.getvalue())

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
            mock.patch.object(lp, "ensure_frontend_hooks", return_value=([], "")),
            mock.patch.object(lp, "run_pre_commit", side_effect=runs) as rpc,
            redirect_stderr(io.StringIO()) as err,
        ):
            code = lp.lint("cfg.yml", [], ["a.md"], "/root")
        return code, rpc.call_count, err.getvalue()

    def test_a_rewrite_then_pass_is_a_pass_with_the_commit_reminder(self):
        code, calls, err = self._lint([(1, True, None), (0, False, None)])
        self.assertEqual((code, calls), (0, 2))
        self.assertIn("re-running once", err)
        # CI lints what is committed; the passing rewrite still needs a commit.
        self.assertIn("stage and commit", err)

    def test_a_second_failure_is_real(self):
        code, calls, _ = self._lint([(1, True, None), (1, True, None)])
        self.assertEqual((code, calls), (1, 2))

    def test_a_failure_without_a_rewrite_is_not_re_run(self):
        code, calls, _ = self._lint([(1, False, None)])
        self.assertEqual((code, calls), (1, 1))

    def test_a_pass_is_not_re_run(self):
        code, calls, _ = self._lint([(0, False, None)])
        self.assertEqual((code, calls), (0, 1))

    def test_the_marker_is_read_from_pre_commit_output(self):
        script = (
            "import sys; print('mdformat...Failed'); "
            f"print('- {lp.MODIFIED_MARKER}'); sys.exit(1)"
        )
        with redirect_stdout(io.StringIO()) as out:
            code, modified, broken = lp.run_pre_commit(
                [sys.executable, "-c", script], os.getcwd()
            )
        self.assertEqual((code, modified, broken), (1, True, None))
        # The output is mirrored, not swallowed.
        self.assertIn("mdformat...Failed", out.getvalue())


class FrontendHooks(unittest.TestCase):
    """A missing ``biome`` / ``tsc``: install, else skip and say unverifiable."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = self._tmp.name
        # pnpm is on PATH unless a test says otherwise.
        which = mock.patch.object(lp.shutil, "which", return_value="/bin/pnpm")
        self.which = which.start()
        self.addCleanup(which.stop)

    def _install_bins(self, *hooks):
        for hook in hooks:
            path = os.path.join(self.root, lp.PNPM_HOOKS[hook][0])
            os.makedirs(os.path.dirname(path), exist_ok=True)
            open(path, "w").close()

    def test_no_frontend_path_needs_nothing(self):
        self.assertEqual(lp.missing_frontend_hooks(self.root, [], ["a.rs"]), [])

    def test_a_ts_path_without_node_modules_misses_both(self):
        self.assertEqual(
            lp.missing_frontend_hooks(self.root, [], ["frontend/x.ts"]),
            ["biome", "tsc"],
        )

    def test_an_sdk_ts_path_also_selects_the_sdk_type_check(self):
        # The frontend tsc never type-checks the SDK; `tsc-sdk` does, and only
        # an `sdk/ts/` path selects it.
        self.assertEqual(
            lp.missing_frontend_hooks(self.root, [], ["sdk/ts/x.ts"]),
            ["biome", "tsc", "tsc-sdk"],
        )
        self._install_bins("biome", "tsc")
        self.assertEqual(
            lp.missing_frontend_hooks(self.root, [], ["sdk/ts/x.ts"]), ["tsc-sdk"]
        )

    def test_a_css_only_set_selects_biome_not_tsc(self):
        # tsc's types_or is ts/tsx only; installing for it would be wasted, and
        # reporting it unverifiable would name a hook that never runs.
        self.assertEqual(lp.missing_frontend_hooks(self.root, [], ["a.css"]), ["biome"])

    def test_a_pnpm_that_fails_to_launch_is_unverifiable_not_a_crash(self):
        with (
            mock.patch.object(
                lp.subprocess, "run", side_effect=FileNotFoundError("pnpm")
            ),
            redirect_stderr(io.StringIO()) as err,
        ):
            self.assertEqual(
                lp.ensure_frontend_hooks(self.root, [], ["x.ts"]),
                (["biome", "tsc"], "frontend install failed"),
            )
        self.assertIn("pnpm", err.getvalue())

    def test_pnpm_off_path_skips_every_selected_hook_without_installing(self):
        # node_modules is present, so the old check passed and the hooks then
        # failed as "Executable `pnpm` not found".
        self._install_bins("biome", "tsc")
        self.which.return_value = None
        with (
            mock.patch.object(lp.subprocess, "run") as run,
            mock.patch.object(
                lp, "run_pre_commit", return_value=(0, False, None)
            ) as rpc,
            redirect_stderr(io.StringIO()) as err,
        ):
            code = lp.lint("cfg.yml", [], ["x.ts"], self.root)
        run.assert_not_called()
        self.assertEqual(code, 0)
        self.assertEqual(
            rpc.call_args.args[2]["SKIP"].split(",")[-2:], ["biome", "tsc"]
        )
        self.assertIn(
            "UNVERIFIABLE locally: biome, tsc (pnpm not on PATH)", err.getvalue()
        )

    def test_pnpm_off_path_with_no_frontend_path_skips_nothing(self):
        self.which.return_value = None
        self.assertEqual(
            lp.ensure_frontend_hooks(self.root, [], ["a.rs"]), ([], "pnpm not on PATH")
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
            missing, _ = lp.ensure_frontend_hooks(self.root, [], ["x.ts"])
        self.assertEqual(missing, [])
        self.assertEqual(run.call_args.args[0], lp._FRONTEND_INSTALL)

    def test_a_failed_install_skips_and_reports_unverifiable(self):
        failed = subprocess.CompletedProcess([], 1, "", "ERR_PNPM offline")
        with (
            mock.patch.object(lp.subprocess, "run", return_value=failed),
            mock.patch.object(
                lp, "run_pre_commit", return_value=(0, False, None)
            ) as rpc,
            redirect_stderr(io.StringIO()) as err,
        ):
            code = lp.lint("cfg.yml", [], ["x.ts"], self.root)
        self.assertEqual(code, 0)
        env = rpc.call_args.args[2]
        self.assertEqual(env["SKIP"].split(",")[-2:], ["biome", "tsc"])
        self.assertIn(
            "UNVERIFIABLE locally: biome, tsc (frontend install failed)",
            err.getvalue(),
        )
        self.assertIn("ERR_PNPM offline", err.getvalue())


# A trimmed copy of the lint config's shape: a remote repo with two hooks, a
# local block mixing a system hook with an environment-building one, and a
# nested list that must not read as a hook key.
_CONFIG = """\
---
- hooks:
  - args:
    - '--format=parseable'
    id: 'cfn-lint'
    files: '^infra/'
  repo: 'https://github.com/aws-cloudformation/cfn-lint'
  rev: 'v1.53.0'
- hooks:
  - id: 'ruff-check'
  - id: 'ruff-format'
  repo: 'https://github.com/astral-sh/ruff-pre-commit'
  rev: 'v0.15.18'
- hooks:
  - entry: 'cargo fmt --'
    id: 'rustfmt'
    language: 'system'
    types:
    - 'rust'
  - entry: 'python3 x.py'
    id: 'dashboard-sql-lint'
    language: 'python'
  repo: 'local'
...
"""

# pre-commit's output for an environment that will not install, as captured
# from cfn-lint under an unsupported Python (paths trimmed).
_ENV_FAILURE = [
    "[INFO] Installing environment for https://github.com/astral-sh/ruff-pre-commit.\n",
    "[INFO] Once installed this environment will be reused.\n",
    "[INFO] Installing environment for https://github.com/aws-cloudformation/cfn-lint.\n",
    "[INFO] Once installed this environment will be reused.\n",
    "[INFO] This may take a few minutes...\n",
    "An unexpected error has occurred: CalledProcessError: command: "
    "('/cache/py_env-python3/bin/python', '-m', 'pip', 'install', '.')\n",
    "return code: 1\n",
    "Check the log at /cache/pre-commit.log\n",
]


class HookEnvironmentFailure(unittest.TestCase):
    """One hook environment that will not install: skip it, re-run, and report
    it unverifiable instead of aborting every hook."""

    def test_the_config_maps_each_repo_to_its_env_hooks(self):
        self.assertEqual(
            lp.env_hooks_by_repo(_CONFIG),
            {
                "https://github.com/aws-cloudformation/cfn-lint": ["cfn-lint"],
                "https://github.com/astral-sh/ruff-pre-commit": [
                    "ruff-check",
                    "ruff-format",
                ],
                # The system hook needs no environment, so it is never skipped.
                "local": ["dashboard-sql-lint"],
            },
        )

    def test_the_live_config_parses_to_known_repos(self):
        with open(
            os.path.join(lp.repo_root(), lp.DEFAULT_CONFIG), encoding="utf-8"
        ) as fh:
            repos = lp.env_hooks_by_repo(fh.read())
        self.assertEqual(
            repos["https://github.com/aws-cloudformation/cfn-lint"], ["cfn-lint"]
        )
        self.assertNotIn("biome", repos["local"])

    def test_the_last_install_line_names_the_broken_repo(self):
        self.assertEqual(
            lp.broken_env_repo(_ENV_FAILURE),
            "https://github.com/aws-cloudformation/cfn-lint",
        )

    def test_no_failure_marker_is_no_broken_repo(self):
        self.assertIsNone(lp.broken_env_repo(_ENV_FAILURE[:5]))

    def test_an_error_after_hooks_started_is_not_an_install_failure(self):
        # Every environment installed and a hook reported, so the last install
        # line is a healthy repo; blaming it would be one wrong skip.
        output = [
            *_ENV_FAILURE[:5],
            "cfn-lint.................Passed\n",
            *_ENV_FAILURE[5:],
        ]
        self.assertIsNone(lp.broken_env_repo(output))

    def test_coloured_output_is_still_read(self):
        output = [
            line.replace("[INFO]", "[\x1b[93mINFO\x1b[m]") for line in _ENV_FAILURE
        ]
        self.assertEqual(
            lp.broken_env_repo(output),
            "https://github.com/aws-cloudformation/cfn-lint",
        )

    def test_pre_commit_4_names_for_system_are_not_skipped(self):
        config = _CONFIG.replace("language: 'system'", "language: 'unsupported'")
        self.assertEqual(lp.env_hooks_by_repo(config)["local"], ["dashboard-sql-lint"])

    def test_the_failure_is_read_from_pre_commit_output(self):
        lines = "".join(_ENV_FAILURE)
        script = f"import sys; sys.stdout.write({lines!r}); sys.exit(3)"
        with redirect_stdout(io.StringIO()):
            _, _, broken = lp.run_pre_commit(
                [sys.executable, "-c", script], os.getcwd()
            )
        self.assertEqual(broken, "https://github.com/aws-cloudformation/cfn-lint")

    def _lint(self, runs, hook_args=(), frontend=([], "")):
        with (
            mock.patch.object(lp, "ensure_frontend_hooks", return_value=frontend),
            mock.patch.object(lp, "_config_text", return_value=_CONFIG),
            mock.patch.object(lp, "run_pre_commit", side_effect=runs) as rpc,
            redirect_stderr(io.StringIO()) as err,
        ):
            code = lp.lint("cfg.yml", list(hook_args), ["a.yaml"], "/root")
        return code, rpc, err.getvalue()

    def test_a_named_hook_skips_only_itself(self):
        ruff = "https://github.com/astral-sh/ruff-pre-commit"
        _, rpc, err = self._lint(
            [(3, False, ruff), (0, False, None)], hook_args=["ruff-check"]
        )
        self.assertEqual(rpc.call_args.args[2]["SKIP"].split(",")[-1], "ruff-check")
        self.assertNotIn("ruff-format", err)

    def test_a_frontend_skip_and_an_env_skip_both_report(self):
        cfn = "https://github.com/aws-cloudformation/cfn-lint"
        _, rpc, err = self._lint(
            [(3, False, cfn), (0, False, None)],
            frontend=(["biome", "tsc"], "pnpm not on PATH"),
        )
        self.assertEqual(
            rpc.call_args.args[2]["SKIP"].split(",")[-3:], ["biome", "tsc", "cfn-lint"]
        )
        self.assertIn("UNVERIFIABLE locally: biome, tsc (pnpm not on PATH)", err)
        self.assertIn("UNVERIFIABLE locally: cfn-lint (its pre-commit", err)

    def test_a_broken_env_is_skipped_and_the_rest_reports(self):
        cfn = "https://github.com/aws-cloudformation/cfn-lint"
        code, rpc, err = self._lint([(3, False, cfn), (0, False, None)])
        self.assertEqual((code, rpc.call_count), (0, 2))
        self.assertEqual(rpc.call_args.args[2]["SKIP"].split(",")[-1], "cfn-lint")
        self.assertIn(
            "UNVERIFIABLE locally: cfn-lint (its pre-commit environment failed "
            "to install)",
            err,
        )

    def test_a_second_broken_env_is_skipped_too(self):
        cfn = "https://github.com/aws-cloudformation/cfn-lint"
        ruff = "https://github.com/astral-sh/ruff-pre-commit"
        code, rpc, err = self._lint(
            [(3, False, cfn), (3, False, ruff), (1, False, None)]
        )
        self.assertEqual((code, rpc.call_count), (1, 3))
        self.assertIn("cfn-lint, ruff-check, ruff-format", err)

    def test_an_unattributable_failure_stands(self):
        # The same repo breaking again after its hooks were skipped, or a repo
        # the config does not name, cannot be skipped: the failure is the verdict.
        cfn = "https://github.com/aws-cloudformation/cfn-lint"
        code, rpc, _ = self._lint([(3, False, cfn), (3, False, cfn)])
        self.assertEqual((code, rpc.call_count), (3, 2))
        code, rpc, err = self._lint([(3, False, "https://example.invalid/x")])
        self.assertEqual((code, rpc.call_count), (3, 1))
        self.assertNotIn("UNVERIFIABLE", err)


if __name__ == "__main__":
    unittest.main()
