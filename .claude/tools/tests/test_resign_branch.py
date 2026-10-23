"""Stdlib ``unittest`` tests for ``resign_branch.py``.

The load-bearing properties: the count reads signatures off the commit
objects; a locked signer moves no ref; a re-sign leaves every commit signed
and the tree unchanged; and the push never forces over a remote commit the
branch lacks. Everything runs on a real throwaway repo with a bare remote. The
signer is a stub script that writes a placeholder ``.sig`` (git embeds it
without verifying), or ``false`` for a locked one. Run via
``make tools-tests``.
"""

from __future__ import annotations

import os
import shutil
import stat
import subprocess
import tempfile
import unittest

import resign_branch as rb

STUB_SIGNER = """#!/bin/sh
for last; do :; done
printf '%s\\n' '-----BEGIN SSH SIGNATURE-----' 'c3R1Yg==' \\
  '-----END SSH SIGNATURE-----' > "$last.sig"
"""


def commit_object(*lines):
    return "\n".join(lines) + "\n"


class IsSigned(unittest.TestCase):
    def test_a_gpgsig_header_is_a_signature(self):
        obj = commit_object(
            "tree t", "author a", "gpgsig -----BEGIN SSH SIGNATURE-----", " x", "", "m"
        )
        self.assertTrue(rb.is_signed(obj))

    def test_a_sha256_signature_header_counts(self):
        obj = commit_object("tree t", "gpgsig-sha256 sig", "", "m")
        self.assertTrue(rb.is_signed(obj))

    def test_gpgsig_in_the_message_is_not_a_signature(self):
        obj = commit_object("tree t", "author a", "", "gpgsig in the body")
        self.assertFalse(rb.is_signed(obj))


class Summary(unittest.TestCase):
    def test_past_five_shas_the_rest_are_counted(self):
        shas = [f"{n:040d}" for n in range(7)]
        line = rb._summary(shas, 9)
        self.assertIn("7 unsigned of 9", line)
        self.assertTrue(line.endswith("(+2)"))


@unittest.skipUnless(shutil.which("false"), "needs a `false` binary")
class RealRepo(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        tmp = os.path.realpath(self._tmp.name)
        self.stub = os.path.join(tmp, "stub-sign")
        with open(self.stub, "w") as f:
            f.write(STUB_SIGNER)
        os.chmod(self.stub, os.stat(self.stub).st_mode | stat.S_IEXEC)
        self.remote = os.path.join(tmp, "remote.git")
        self.root = os.path.join(tmp, "work")
        subprocess.run(["git", "init", "-q", "--bare", self.remote], check=True)
        os.mkdir(self.root)
        cwd = os.getcwd()
        os.chdir(self.root)
        self.addCleanup(os.chdir, cwd)
        self.git("init", "-q", "-b", "main")
        for key, value in (
            ("user.email", "t@example.com"),
            ("user.name", "Test"),
            ("gpg.format", "ssh"),
            ("user.signingkey", "key::ssh-ed25519 placeholder"),
            ("commit.gpgsign", "false"),
        ):
            self.git("config", key, value)
        self.git("remote", "add", "origin", self.remote)
        self.signer(self.stub)
        self.commit("base", sign=True)
        self.git("push", "-q", "-u", "origin", "main")
        self.git("checkout", "-q", "-b", "eng-1")

    def git(self, *args):
        return subprocess.run(
            ["git", *args], capture_output=True, text=True, check=True
        ).stdout.strip()

    def signer(self, program):
        self.git("config", "gpg.ssh.program", program)

    def commit(self, name, sign=False):
        with open(name, "w") as f:
            f.write(name)
        self.git("add", name)
        self.git("commit", "-q", "-S" if sign else "--no-gpg-sign", "-m", name)

    def tree(self):
        return self.git("rev-parse", "HEAD^{tree}")

    def test_count_names_only_the_unsigned_commits(self):
        self.commit("a")
        self.commit("b", sign=True)
        code, line = rb.count("main")
        self.assertEqual(code, 0)
        self.assertIn("1 unsigned of 2", line)
        self.assertIn(self.git("rev-parse", "HEAD~1")[:9], line)

    def test_an_all_signed_branch_needs_nothing(self):
        self.commit("a", sign=True)
        head = self.git("rev-parse", "HEAD")
        code, line = rb.resign("main", push=False)
        self.assertEqual(code, 0)
        self.assertIn("nothing to sign", line)
        self.assertEqual(self.git("rev-parse", "HEAD"), head)

    def test_a_locked_signer_moves_no_ref(self):
        self.commit("a")
        head = self.git("rev-parse", "HEAD")
        self.signer(shutil.which("false"))
        code, line = rb.resign("main", push=False)
        self.assertEqual(code, 1)
        self.assertIn("LOCKED", line)
        self.assertEqual(self.git("rev-parse", "HEAD"), head)

    def test_resign_signs_every_commit_and_keeps_the_tree(self):
        self.commit("a")
        self.commit("b")
        tree = self.tree()
        code, line = rb.resign("main", push=False)
        self.assertEqual(code, 0, line)
        self.assertIn("signed 2 of 2", line)
        self.assertEqual(self.tree(), tree)
        self.assertIn("0 unsigned of 2", rb.count("main")[1])

    def test_resign_replaces_the_pushed_unsigned_branch(self):
        self.commit("a")
        self.git("push", "-q", "-u", "origin", "eng-1")
        code, line = rb.resign("main", push=True)
        self.assertEqual(code, 0, line)
        self.assertEqual(
            self.git("ls-remote", "origin", "refs/heads/eng-1").split()[0],
            self.git("rev-parse", "HEAD"),
        )

    def test_a_branch_rebased_onto_a_newer_base_still_pushes(self):
        self.commit("a")
        self.git("push", "-q", "-u", "origin", "eng-1")
        self.git("checkout", "-q", "main")
        self.commit("m", sign=True)
        self.git("push", "-q")
        self.git("checkout", "-q", "eng-1")
        # The flag after the upstream, as the bootstrap's rebase spells it.
        self.git("rebase", "-q", "main", "--no-gpg-sign")
        self.assertIn("1 unsigned of 1", rb.count("main")[1])
        code, line = rb.resign("main", push=True)
        self.assertEqual(code, 0, line)
        self.assertIn("signed 1 of 1", line)

    def test_a_branch_never_pushed_is_pushed_with_an_upstream(self):
        self.commit("a")
        code, line = rb.resign("main", push=True)
        self.assertEqual(code, 0, line)
        self.assertEqual(
            self.git("rev-parse", "--abbrev-ref", "eng-1@{upstream}"), "origin/eng-1"
        )

    def test_a_remote_commit_this_branch_never_held_refuses_before_moving(self):
        self.commit("a")
        self.git("push", "-q", "-u", "origin", "eng-1")
        other = os.path.join(os.path.dirname(self.root), "other")
        self.git("clone", "-q", "-b", "eng-1", self.remote, other)
        with open(os.path.join(other, "b"), "w") as f:
            f.write("b")
        for args in (
            ("add", "b"),
            ("-c", "user.name=O", "-c", "user.email=o@example.com",
             "commit", "-q", "--no-gpg-sign", "-m", "b"),
            ("push", "-q"),
        ):  # fmt: skip
            self.git("-C", other, *args)
        # A fetch (here, or in any worktree sharing the repo) moves the
        # tracking ref onto the unseen commit; that must not read as seen.
        self.git("fetch", "-q", "origin")
        head = self.git("rev-parse", "HEAD")
        tracked = self.git("rev-parse", "refs/remotes/origin/eng-1")
        with self.assertRaisesRegex(rb.Refused, "never held"):
            rb.resign("main", push=True)
        self.assertEqual(self.git("rev-parse", "HEAD"), head)
        self.assertEqual(self.git("rev-parse", "refs/remotes/origin/eng-1"), tracked)

    def test_a_rerun_after_a_failed_push_still_pushes(self):
        self.commit("a")
        self.git("push", "-q", "-u", "origin", "eng-1")
        # Signed locally, never pushed: the state a failed push leaves.
        self.assertEqual(rb.resign("main", push=False)[0], 0)
        code, line = rb.resign("main", push=True)
        self.assertEqual(code, 0, line)
        self.assertIn("nothing to sign | pushed", line)
        self.assertEqual(
            self.git("ls-remote", "origin", "refs/heads/eng-1").split()[0],
            self.git("rev-parse", "HEAD"),
        )
        self.assertIn("already matches", rb.resign("main", push=True)[1])

    def test_a_merge_commit_is_refused_before_moving(self):
        self.commit("a")
        self.git("checkout", "-q", "main")
        self.commit("m", sign=True)
        self.git("push", "-q")
        self.git("checkout", "-q", "eng-1")
        self.git("merge", "-q", "--no-gpg-sign", "--no-edit", "main")
        head = self.git("rev-parse", "HEAD")
        with self.assertRaisesRegex(rb.Refused, "merge commit"):
            rb.resign("main", push=False)
        self.assertEqual(self.git("rev-parse", "HEAD"), head)

    def test_a_dirty_tree_is_refused(self):
        self.commit("a")
        with open("a", "w") as f:
            f.write("changed")
        with self.assertRaisesRegex(rb.Refused, "uncommitted"):
            rb.resign("main", push=False)

    def test_the_cli_maps_outcomes_to_exit_codes(self):
        self.commit("a")
        head = self.git("rev-parse", "HEAD")
        self.signer(shutil.which("false"))
        self.assertEqual(rb.main(["resign", "--no-push"]), 1)
        self.signer(self.stub)
        with open("a", "w") as f:
            f.write("changed")
        self.assertEqual(rb.main(["resign", "--no-push"]), 2)
        self.git("checkout", "-q", "--", "a")
        self.assertEqual(self.git("rev-parse", "HEAD"), head)
        self.assertEqual(rb.main(["resign", "--no-push"]), 0)
        self.assertEqual(rb.main(["count"]), 0)
        self.assertEqual(self.git("ls-remote", "origin", "refs/heads/eng-1"), "")


if __name__ == "__main__":
    unittest.main()
