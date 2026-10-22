"""Stdlib ``unittest`` tests for the signing probe.

The load-bearing properties: a failed signer reads ``LOCKED`` with the
instruction for this configuration, the probe moves no ref, and a probe that
could not run is not mistaken for a locked signer. The locked path runs on a
real throwaway repo whose signer is wired to ``false``; the signed path stubs
the subprocess, since a real signature needs a real key. Run via
``make tools-tests``.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock

import signing_probe as sp


class UnlockInstruction(unittest.TestCase):
    def test_an_external_signer_names_its_backend(self):
        with tempfile.TemporaryDirectory() as tmp:
            signer = os.path.join(tmp, "op-ssh-sign")
            open(signer, "w").close()
            text = sp.unlock_instruction("ssh", signer)
        self.assertIn("op-ssh-sign", text)
        self.assertIn("1Password", text)

    def test_a_missing_external_signer_says_so(self):
        text = sp.unlock_instruction("ssh", "/nonexistent/op-ssh-sign")
        self.assertIn("does not resolve", text)

    def test_an_agent_signer_names_the_agent(self):
        self.assertIn("ssh agent", sp.unlock_instruction("ssh", None))

    def test_gpg_names_gpg_agent(self):
        self.assertIn("gpg-agent", sp.unlock_instruction(None, None))


@unittest.skipUnless(shutil.which("false"), "needs a `false` binary")
class RealRepo(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = os.path.realpath(self._tmp.name)
        cwd = os.getcwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, cwd)
        self._git("init", "-q", "-b", "main")
        for key, value in (
            ("user.email", "t@example.com"),
            ("user.name", "Test"),
            ("gpg.format", "ssh"),
            ("gpg.ssh.program", shutil.which("false")),
            ("user.signingkey", "key::ssh-ed25519 placeholder"),
        ):
            self._git("config", key, value)

    def _git(self, *args: str) -> str:
        return subprocess.run(
            ["git", *args], check=True, capture_output=True, text=True
        ).stdout.strip()

    def test_no_head_is_not_a_locked_signer(self):
        code, line = sp.probe()
        self.assertEqual(code, 2)
        self.assertNotIn("LOCKED", line)

    def test_a_failing_signer_reads_locked_and_moves_no_ref(self):
        self._git("commit", "-q", "--allow-empty", "-m", "base", "--no-gpg-sign")
        before = self._git("rev-parse", "HEAD")
        code, line = sp.probe()
        self.assertEqual(code, 1)
        self.assertIn("LOCKED", line)
        self.assertIn("false backend", line)
        self.assertEqual(self._git("rev-parse", "HEAD"), before)


class Signed(unittest.TestCase):
    def test_a_successful_probe_reports_the_dangling_sha(self):
        done = subprocess.CompletedProcess([], 0, "0123456789abcdef\n", "")
        with (
            mock.patch.object(sp, "has_head", return_value=True),
            mock.patch.object(sp.subprocess, "run", return_value=done) as run,
        ):
            code, line = sp.probe()
        self.assertEqual(code, 0)
        self.assertIn("signed | 0123456789ab", line)
        # The probe is commit-tree, never a ref-moving commit.
        self.assertEqual(run.call_args.args[0][:3], ["git", "commit-tree", "-S"])


if __name__ == "__main__":
    unittest.main()
