#!/usr/bin/env python3
"""Unit tests for ``regen_artifacts.py`` (stdlib ``unittest``; no pytest).

The families are fakes — one-line Python "generators" writing into a throwaway
repo — so the staging, diffing and committing run for real while no build does.
"""

from __future__ import annotations

import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

import regen_artifacts as ra

PY = sys.executable


def git(repo, *args):
    return subprocess.run(
        ["git", *args], cwd=repo, check=True, capture_output=True, text=True
    ).stdout


def writer(path, text, code=0, say=""):
    """A fake target: print ``say``, write ``text`` to ``path``, exit ``code``."""
    script = (
        f"import sys; print({say!r}); "
        f"open({path!r}, 'w').write({text!r}); sys.exit({code})"
    )
    return [PY, "-c", script]


class RegenTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self._tmp.name)
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "user.email", "t@example.com")
        git(self.repo, "config", "user.name", "T")
        git(self.repo, "config", "commit.gpgsign", "false")
        (self.repo / "idl.json").write_text("v1\n", encoding="utf-8")
        git(self.repo, "add", "idl.json")
        git(self.repo, "commit", "-q", "-m", "Seed")
        git(self.repo, "update-ref", "refs/remotes/origin/main", "HEAD")
        self._cwd = Path.cwd()
        os.chdir(self.repo)

    def tearDown(self):
        os.chdir(self._cwd)
        self._tmp.cleanup()

    def family(self, cmd, path="idl.json", message="Rebuild IDL"):
        return ra.Family("idl", cmd, [path], message)

    def test_an_unchanged_artifact_is_fresh(self):
        [entry] = ra.regenerate([self.family(writer("idl.json", "v1\n"))], sign=False)
        self.assertEqual(entry["status"], "fresh")
        self.assertIsNone(entry["commit"])
        self.assertEqual(len(git(self.repo, "log", "--oneline").splitlines()), 1)

    def test_a_moved_artifact_is_committed_under_the_canonical_message(self):
        [entry] = ra.regenerate([self.family(writer("idl.json", "v2\n"))], sign=False)
        self.assertEqual(entry["status"], "committed")
        self.assertEqual(
            git(self.repo, "log", "-1", "--format=%s").strip(), "Rebuild IDL"
        )
        self.assertEqual(
            entry["commit"], git(self.repo, "rev-parse", "--short", "HEAD").strip()
        )

    def test_a_new_file_in_the_family_is_committed_too(self):
        # `git add -A` over the family paths, so an added vector is not missed.
        (self.repo / "vectors").mkdir()
        family = ra.Family(
            "vectors", writer("vectors/new.json", "{}\n"), ["vectors"], "Regen"
        )
        [entry] = ra.regenerate([family], sign=False)
        self.assertEqual(entry["status"], "committed")

    def test_only_the_family_paths_are_committed(self):
        (self.repo / "unrelated.txt").write_text("wip\n", encoding="utf-8")
        git(self.repo, "add", "unrelated.txt")
        ra.regenerate([self.family(writer("idl.json", "v2\n"))], sign=False)
        self.assertEqual(
            git(self.repo, "show", "--name-only", "--format=").split(), ["idl.json"]
        )
        self.assertIn(
            "unrelated.txt", git(self.repo, "diff", "--cached", "--name-only")
        )

    def test_a_toolchain_abort_is_unverifiable_never_a_pass(self):
        cmd = [
            PY,
            "-c",
            "print('wasm-pack not found (cargo install wasm-pack)'); exit(2)",
        ]
        report = ra.regenerate([self.family(cmd)], sign=False)
        self.assertEqual(report[0]["status"], "unverifiable")
        self.assertEqual(ra.exit_status(report), ra.EXIT_UNVERIFIABLE)

    def test_any_other_failure_is_failed(self):
        report = ra.regenerate(
            [self.family(writer("idl.json", "v2\n", code=1, say="error[E0425]"))],
            sign=False,
        )
        self.assertEqual(report[0]["status"], "failed")
        self.assertEqual(ra.exit_status(report), ra.EXIT_FAILED)
        # A failed target commits nothing, even though it wrote the file.
        self.assertEqual(len(git(self.repo, "log", "--oneline").splitlines()), 1)

    def test_a_failure_does_not_stop_the_later_families(self):
        report = ra.regenerate(
            [
                self.family([PY, "-c", "print('anchor-cli 1.2.1 required'); exit(1)"]),
                ra.Family(
                    "sdk", writer("sdk.ts", "x\n"), ["sdk.ts"], "Regenerate SDK clients"
                ),
            ],
            sign=False,
        )
        self.assertEqual([e["status"] for e in report], ["unverifiable", "committed"])

    def test_a_failed_family_outranks_an_unverifiable_one(self):
        report = [{"status": "unverifiable"}, {"status": "failed"}, {"status": "fresh"}]
        self.assertEqual(ra.exit_status(report), ra.EXIT_FAILED)

    def test_the_path_gate_skips_a_docs_only_input_change(self):
        (self.repo / "programs").mkdir()
        (self.repo / "programs" / "README.md").write_text("# p\n", encoding="utf-8")
        git(self.repo, "add", "programs/README.md")
        git(self.repo, "commit", "-q", "-m", "Docs")
        self.assertFalse(ra.runs_artifact_gates("main"))
        (self.repo / "programs" / "lib.rs").write_text("fn a() {}\n", encoding="utf-8")
        git(self.repo, "add", "programs/lib.rs")
        git(self.repo, "commit", "-q", "-m", "Code")
        self.assertTrue(ra.runs_artifact_gates("main"))

    def test_main_skips_every_family_when_the_gate_is_false(self):
        out = io.StringIO()
        with (
            mock.patch.object(ra, "runs_artifact_gates", return_value=False),
            redirect_stdout(out),
        ):
            code = ra.main([])
        parsed = json.loads(out.getvalue())
        self.assertEqual(code, ra.EXIT_OK)
        self.assertEqual({e["status"] for e in parsed["families"]}, {"skipped"})
        self.assertFalse(parsed["lint_rerun"])
        self.assertTrue(parsed["verified"])

    def test_main_reports_lint_rerun_after_a_commit(self):
        families = [self.family(writer("idl.json", "v2\n"))]
        real = ra.regenerate
        out = io.StringIO()
        with (
            mock.patch.object(ra, "regenerate", lambda: real(families, sign=False)),
            redirect_stdout(out),
        ):
            code = ra.main(["--force"])
        parsed = json.loads(out.getvalue())
        self.assertEqual(code, ra.EXIT_OK)
        self.assertTrue(parsed["lint_rerun"])

    def test_a_base_that_looks_like_an_option_is_refused(self):
        self.assertEqual(ra.main(["--base=-x"]), ra.EXIT_USAGE)


if __name__ == "__main__":
    unittest.main()
