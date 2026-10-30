"""`zsh_test_env.hermetic_env` — what the zsh suites' child env keeps and drops."""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from zsh_test_env import hermetic_env

_OPERATOR = {
    "PATH": "/usr/bin",
    "HOME": "/home/someone",
    "DS_MODEL_EXECUTOR": "real-model",
    "_DS_REPO": "/repo",
    "ANTHROPIC_MODEL": "real-model",
    "AWS_PROFILE": "ds-dev",
    "CLAUDE_CODE_USE_BEDROCK": "1",
    "OP_SESSION_acct": "secret",
    "CLAUDE_CONFIG_DIR": "/home/someone/.claude",
    "ENABLE_PROMPT_CACHING_1H": "1",
    "CARGO_TARGET_DIR": "/pool",
}


class HermeticEnv(unittest.TestCase):
    def build(self, *also, env=None):
        with mock.patch.dict(os.environ, _OPERATOR, clear=True):
            return hermetic_env("/scratch/home", *also, env=env)

    def test_every_operator_family_is_dropped(self):
        child = self.build()
        for name in (
            "DS_MODEL_EXECUTOR",
            "_DS_REPO",
            "ANTHROPIC_MODEL",
            "AWS_PROFILE",
            "CLAUDE_CODE_USE_BEDROCK",
            "OP_SESSION_acct",
            "ENABLE_PROMPT_CACHING_1H",
        ):
            with self.subTest(name=name):
                self.assertNotIn(name, child)

    def test_the_transcript_tree_is_the_scratch_one(self):
        self.assertEqual(self.build()["CLAUDE_CONFIG_DIR"], "/scratch/home")

    def test_the_rest_of_the_environment_survives(self):
        child = self.build()
        self.assertEqual(child["PATH"], "/usr/bin")
        self.assertEqual(child["CARGO_TARGET_DIR"], "/pool")

    def test_a_suite_can_drop_its_own_names(self):
        self.assertNotIn("CARGO_TARGET_DIR", self.build("CARGO_TARGET_DIR"))

    def test_iterm_is_always_disabled(self):
        self.assertEqual(self.build()["DS_ITERM_DISABLED"], "1")

    def test_the_case_env_wins(self):
        child = self.build(env={"DS_MODEL_EXECUTOR": "placeholder"})
        self.assertEqual(child["DS_MODEL_EXECUTOR"], "placeholder")

    def test_the_case_env_wins_over_the_forced_values_too(self):
        child = self.build(env={"CLAUDE_CONFIG_DIR": "/elsewhere"})
        self.assertEqual(child["CLAUDE_CONFIG_DIR"], "/elsewhere")


@unittest.skipUnless(shutil.which("zsh"), "no zsh on this machine")
class OperatorZshenvDoesNotRun(unittest.TestCase):
    """zsh sources `.zshenv` even for `zsh -c`, so the name scrub alone would
    lose to an operator `.zshenv` that re-exports a scrubbed variable."""

    def test_a_zshenv_cannot_re_export_a_scrubbed_name(self):
        with tempfile.TemporaryDirectory() as tmp:
            operator_home = Path(tmp) / "operator"
            operator_home.mkdir()
            (operator_home / ".zshenv").write_text(
                "export DS_MODEL_EXECUTOR=leaked\n", encoding="utf-8"
            )
            with mock.patch.dict(
                os.environ, {"HOME": str(operator_home), "ZDOTDIR": str(operator_home)}
            ):
                env = hermetic_env(Path(tmp) / "scratch")
            out = subprocess.run(
                ["zsh", "-c", 'print -r -- "${DS_MODEL_EXECUTOR-unset}"'],
                capture_output=True,
                text=True,
                check=True,
                env=env,
            ).stdout.strip()
        self.assertEqual(out, "unset")


if __name__ == "__main__":
    unittest.main()
