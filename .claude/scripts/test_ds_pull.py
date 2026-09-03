#!/usr/bin/env python3
"""Behavior tests for the session verbs' `main` fast-forward (stdlib unittest).

Two behavioral decisions landed with no committed test, verified only by
throwaway scratchpad scripts that are now gone. This is the committed
replacement, and it lands as Python driving real `zsh` rather than as a shell
harness, because `make tools-tests` already discovers `test_*.py` under
`.claude/scripts` and the repo has no shell test runner. Landing it here also
retires the `zsh <script>` permission churn, which is structurally unfirmable:
it generalizes only to `Bash(zsh:*)`, a bare-verb wildcard the safety floor in
`firm_core.is_bareverb_wildcard` refuses outright.

The shell-side branches ARE the substance here — the `git -C` calls and the
stamp write — so porting the arithmetic to Python and testing that instead
would test the easy half. These drive the real function against a real
two-repository git fixture.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

INIT = Path(__file__).resolve().parents[1] / "shell" / "init.zsh"

# The helpers are zsh, and deliberately so — they use zsh-only constructs
# (`<->` for an all-digits test, `>|` to clobber) that are load-bearing rather
# than incidental. So the suite needs a real zsh, and the Linux CI runner does
# not ship one: without this guard every case here fails with
# `FileNotFoundError: 'zsh'`, which says nothing about the code under test.
_NEEDS_ZSH = "the session helpers are zsh; no zsh on this machine"

# Fixture commits must not inherit the operator's signing config: this repo
# signs every commit, and a fixture has no key requirement.
GIT_BASE = [
    "git",
    "-c",
    "user.email=test@example.com",
    "-c",
    "user.name=Test",
    "-c",
    "commit.gpgsign=false",
    "-c",
    "init.defaultBranch=main",
]


def git(*args, cwd):
    return subprocess.run(
        GIT_BASE + list(args), cwd=cwd, capture_output=True, text=True, check=True
    )


@unittest.skipUnless(shutil.which("zsh"), _NEEDS_ZSH)
class PullHarness(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        root = Path(self._tmp.name)
        self.origin = root / "origin"
        self.base = root / "base"

    def _make_origin(self):
        self.origin.mkdir()
        git("init", "-q", cwd=self.origin)
        (self.origin / "README.md").write_text("a\n", encoding="utf-8")
        git("add", "-A", cwd=self.origin)
        git("commit", "-qm", "first", cwd=self.origin)

    def _clone(self):
        git("clone", "-q", str(self.origin), str(self.base), cwd=self._tmp.name)

    def _advance_origin(self, path="README.md", body="b\n"):
        target = self.origin / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(body, encoding="utf-8")
        git("add", "-A", cwd=self.origin)
        git("commit", "-qm", "second", cwd=self.origin)

    def _run(self, repo, *, extra="", env=None):
        """Source the real init, repoint `_DS_REPO` at the fixture, pull, and
        print the recorded outcome."""
        script = (
            f'source "{INIT}" 2>/dev/null; '
            f'_DS_REPO="{repo}"; '
            f"{extra}"
            "_ds_pull; "
            'print -r -- "OUTCOME=$_DS_PULL_LAST_OUTCOME"; '
            'print -r -- "ERROR=$_DS_PULL_LAST_ERROR"'
        )
        result = subprocess.run(
            ["zsh", "-c", script],
            capture_output=True,
            text=True,
            check=False,
            env={**os.environ, **(env or {})},
        )
        outcome = ""
        error = ""
        for line in result.stdout.splitlines():
            if line.startswith("OUTCOME="):
                outcome = line.partition("=")[2].strip()
            elif line.startswith("ERROR="):
                error = line.partition("=")[2].strip()
        return outcome, error, result

    def _stamp(self):
        return self.base / ".git" / ".ds-last-pull"

    def _head(self, repo):
        return git("rev-parse", "HEAD", cwd=repo).stdout.strip()


class Outcomes(PullHarness):
    """The three previously-indistinguishable cases. An operator reported that
    `cdds` did not pull, and a throttled skip, a successful quiet pull and a
    silent failure all looked identical from outside — which is what made the
    report unfalsifiable."""

    def test_a_non_repository_is_reported_and_writes_no_stamp(self):
        outcome, _, _ = self._run(self._tmp.name + "/nope")
        self.assertEqual(outcome, "not-a-repo")

    def test_a_fast_forward_reports_ok_and_moves_main(self):
        self._make_origin()
        self._clone()
        self._advance_origin()
        want = self._head(self.origin)

        outcome, _, result = self._run(self.base)
        self.assertEqual(outcome, "ok", result.stderr)
        self.assertEqual(self._head(self.base), want, "main must fast-forward")

    def test_a_fresh_stamp_throttles_and_does_not_pull(self):
        """Silent by design, but now recorded — the whole point of the
        outcome variable."""
        self._make_origin()
        self._clone()
        before = self._head(self.base)
        self._advance_origin()
        self._stamp().write_text(f"{int(time.time())}\n", encoding="utf-8")

        outcome, _, _ = self._run(self.base)
        self.assertEqual(outcome, "throttled")
        self.assertEqual(self._head(self.base), before, "a throttled run must not pull")

    def test_a_stale_stamp_lets_the_pull_through(self):
        self._make_origin()
        self._clone()
        self._advance_origin()
        stale = int(time.time()) - 3600
        self._stamp().write_text(f"{stale}\n", encoding="utf-8")

        outcome, _, _ = self._run(self.base)
        self.assertEqual(outcome, "ok")

    def test_the_stamp_is_claimed_before_pulling(self):
        """Two tabs launched in the same instant would otherwise both read a
        stale stamp and both pull."""
        self._make_origin()
        self._clone()
        self._advance_origin()
        self._run(self.base)
        written = self._stamp().read_text(encoding="utf-8").strip()
        self.assertTrue(written.isdigit(), written)
        self.assertLessEqual(abs(int(written) - int(time.time())), 120)

    def test_a_garbage_stamp_is_not_evaluated_as_arithmetic(self):
        """zsh math context RE-EVALUATES a non-numeric parameter as an
        arithmetic expression, which can assign and can index arrays. The
        all-digits guard removes that class, and the run proceeds rather than
        throttling on nonsense."""
        self._make_origin()
        self._clone()
        self._advance_origin()
        self._stamp().write_text("not-a-number\n", encoding="utf-8")

        outcome, _, result = self._run(self.base)
        self.assertEqual(outcome, "ok", result.stderr)


class NonMainBranch(PullHarness):
    def test_a_non_main_checkout_fetches_instead_of_pulling(self):
        """The base repo is the only thing this touches, and it must never
        fast-forward somebody's feature branch."""
        self._make_origin()
        self._clone()
        git("checkout", "-qb", "feature", cwd=self.base)
        before = self._head(self.base)
        self._advance_origin()

        outcome, _, result = self._run(self.base)
        self.assertEqual(outcome, "fetched", result.stderr)
        self.assertEqual(self._head(self.base), before, "the branch must not move")


class FailureReporting(PullHarness):
    def test_a_failure_reports_gits_own_last_line(self):
        """The old warning was one generic sentence, so a diverged base, a
        dirty tree, an expired credential and an offline network all read
        identically — telling the operator only that something went wrong,
        which is the part they could already see."""
        self._make_origin()
        self._clone()
        self._advance_origin()
        # Diverge: a local commit that is not an ancestor of origin/main makes
        # --ff-only fail for a real, reproducible reason.
        (self.base / "local.txt").write_text("x\n", encoding="utf-8")
        git("add", "-A", cwd=self.base)
        git("commit", "-qm", "local divergence", cwd=self.base)

        outcome, error, result = self._run(self.base)
        self.assertEqual(outcome, "failed", result.stdout + result.stderr)
        self.assertTrue(error, "the failure must carry git's own detail")
        self.assertNotEqual(error, "diverged, dirty or offline")
        # And the operator sees it, not just the variable.
        self.assertIn("could not fast-forward main", result.stderr)

    def test_a_failure_is_never_fatal(self):
        """Every session verb calls this, so a failure must not stop a launch."""
        self._make_origin()
        self._clone()
        self._advance_origin()
        (self.base / "local.txt").write_text("x\n", encoding="utf-8")
        git("add", "-A", cwd=self.base)
        git("commit", "-qm", "local divergence", cwd=self.base)

        _, _, result = self._run(self.base)
        self.assertEqual(result.returncode, 0)


class DebugFlag(PullHarness):
    def test_the_debug_flag_names_the_outcome_on_every_path(self):
        """`DS_PULL_DEBUG=1 cdds` is how an "it didn't pull" report gets
        answered without guessing which of the three causes it was."""
        self._make_origin()
        self._clone()
        self._stamp().write_text(f"{int(time.time())}\n", encoding="utf-8")

        _, _, result = self._run(self.base, env={"DS_PULL_DEBUG": "1"})
        self.assertIn("pull throttled", result.stderr)

    def test_without_the_flag_a_throttled_skip_is_silent(self):
        self._make_origin()
        self._clone()
        self._stamp().write_text(f"{int(time.time())}\n", encoding="utf-8")

        _, _, result = self._run(self.base)
        self.assertNotIn("throttled", result.stderr)


class SelfRefresh(PullHarness):
    def test_a_pull_that_changes_the_init_re_sources_it(self):
        """Nothing refreshed the helpers, which made "my shell is stale" both
        the most likely explanation for a reported verb misbehavior and the
        hardest to distinguish from a real bug."""
        self._make_origin()
        # The fixture's own init.zsh: sourcing it must be observable.
        self._advance_origin(
            path=".claude/shell/init.zsh", body="_DS_FIXTURE_RELOADED=1\n"
        )
        self._clone()
        self._advance_origin(
            path=".claude/shell/init.zsh",
            body="_DS_FIXTURE_RELOADED=2\n",
        )

        script = (
            f'source "{INIT}" 2>/dev/null; '
            f'_DS_REPO="{self.base}"; '
            "_ds_pull; "
            'print -r -- "RELOADED=$_DS_FIXTURE_RELOADED"'
        )
        result = subprocess.run(
            ["zsh", "-c", script], capture_output=True, text=True, check=False
        )
        self.assertIn("RELOADED=2", result.stdout, result.stderr)
        self.assertIn("session helpers reloaded", result.stderr)

    def test_a_pull_that_does_not_change_the_init_does_not_reload(self):
        self._make_origin()
        self._advance_origin(
            path=".claude/shell/init.zsh", body="_DS_FIXTURE_RELOADED=1\n"
        )
        self._clone()
        self._advance_origin(path="README.md", body="unrelated\n")

        _, _, result = self._run(self.base)
        self.assertNotIn("session helpers reloaded", result.stderr)


@unittest.skipUnless(shutil.which("zsh"), _NEEDS_ZSH)
class Throttle(unittest.TestCase):
    def test_the_throttle_window_is_sixty_seconds(self):
        """A minute is far shorter than a working session and long enough to
        collapse a whole fleet launch into one pull."""
        result = subprocess.run(
            [
                "zsh",
                "-c",
                f'source "{INIT}" 2>/dev/null; '
                'print -r -- "$_DS_PULL_THROTTLE_SECONDS"',
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.stdout.strip(), "60")


if __name__ == "__main__":
    unittest.main()
