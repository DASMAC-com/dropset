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
worktree /repos/dropset
HEAD 8fd8d470f85fe01073a417b25351c840df313c60
branch refs/heads/main

worktree /repos/dropset/.claude/worktrees/eng-603
HEAD 8da1695aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
branch refs/heads/worktree-eng-603
"""

PORCELAIN_NO_MAIN = """\
worktree /repos/dropset/.claude/worktrees/eng-603
HEAD 8da1695aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
branch refs/heads/eng-603
"""


class ParseBaseRepo(unittest.TestCase):
    def test_finds_main_worktree(self):
        self.assertEqual(ipb.parse_base_repo(PORCELAIN), "/repos/dropset")

    def test_none_when_no_main(self):
        self.assertIsNone(ipb.parse_base_repo(PORCELAIN_NO_MAIN))

    def test_detached_head_stanza_is_ignored(self):
        # A detached worktree has no `branch` line; it must not be misread as base.
        porcelain = "worktree /tmp/detached\nHEAD abc123\ndetached\n\n" + PORCELAIN
        self.assertEqual(ipb.parse_base_repo(porcelain), "/repos/dropset")


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
    """``--link-env``'s five outcomes, plus the never-clobber invariant.

    Each case builds a throwaway base repo / worktree pair on a real
    filesystem — ``os.symlink`` is the behavior under test, so it isn't mocked.
    """

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        root = Path(self._tmp.name)
        self.base = root / "base"
        self.worktree = root / "worktree"
        for repo in (self.base, self.worktree):
            (repo / "frontend").mkdir(parents=True)
            (repo / "infra" / "localnet").mkdir(parents=True)

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

    def test_links_the_secrets_enclave_file_too(self):
        # The enclave file is a plain per-checkout path — nothing resolves it
        # through a worktree to the main checkout the way settings.local.json
        # is resolved, so the symlink is what gives it that resolution.
        source = self.base / ipb._SECRETS_ENV_REL
        dest = self.worktree / ipb._SECRETS_ENV_REL
        source.write_text("DROPSET_OP_ACCOUNT=acct\n", encoding="utf-8")
        self.assertEqual(
            ipb.link_env(str(self.base), str(self.worktree), ipb._SECRETS_ENV_REL),
            "created",
        )
        self.assertTrue(dest.is_symlink())
        self.assertEqual(dest.read_text(encoding="utf-8"), "DROPSET_OP_ACCOUNT=acct\n")

    def test_the_two_outcomes_are_independent(self):
        # The whole reason for two keys: a machine that has never run the
        # frontend has no .env.local, and that says nothing about the enclave.
        (self.base / ipb._SECRETS_ENV_REL).write_text("K=v\n", encoding="utf-8")
        self.assertEqual(ipb.link_env(str(self.base), str(self.worktree)), "no-source")
        self.assertEqual(
            ipb.link_env(str(self.base), str(self.worktree), ipb._SECRETS_ENV_REL),
            "created",
        )


class NodeModulesState(unittest.TestCase):
    """A cold worktree is measured, not predicted.

    The field exists because the conditional it replaces — install "when the
    surfaced task touches ``frontend/**``" — loses reliably to "this diff
    doesn't touch the frontend". The ``biome`` and ``tsc`` hooks then fail on
    the first full lint whatever the branch changed, with an error that says
    nothing about the diff.
    """

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = Path(self._tmp.name)

    def test_a_cold_worktree_reports_absent(self):
        (self.root / "frontend").mkdir()
        self.assertEqual(ipb.node_modules_state(str(self.root)), "absent")

    def test_an_installed_worktree_reports_present(self):
        (self.root / "frontend" / "node_modules").mkdir(parents=True)
        self.assertEqual(ipb.node_modules_state(str(self.root)), "present")

    def test_no_frontend_is_distinguished_from_a_cold_worktree(self):
        # Distinct from `absent` on purpose: there is nothing to install, so a
        # skill must not report this as a missing prerequisite to go fix.
        self.assertEqual(ipb.node_modules_state(str(self.root)), "no-frontend")

    def test_a_file_named_node_modules_is_not_an_install(self):
        # `isdir`, not `exists` — a stray file of that name would otherwise
        # report `present` and send the session into the lint failure the
        # field exists to prevent.
        (self.root / "frontend").mkdir()
        (self.root / "frontend" / "node_modules").write_text("", encoding="utf-8")
        self.assertEqual(ipb.node_modules_state(str(self.root)), "absent")


class SigningState(unittest.TestCase):
    """All three signing configurations, because the bug was covering one.

    The pre-check this replaces ran ``ssh-add -l`` unconditionally. That is the
    right probe for **agent-based** ssh signing — reading ``user.signingkey``
    passes for a locked agent, so only the listing catches it — and it is an
    unclearable false positive for **external-signer** ssh signing, where git
    never consults an agent at all. Four bootstraps on one machine hard-stopped
    on it, each burning an operator interaction that provably could not help;
    the fourth was the session that wrote these tests, whose own bootstrap
    commit signed fine in the state the probe called broken.

    So the load-bearing assertions here are the two that pin the *dispatch*:
    ``gpg.ssh.program`` is consulted first, and the agent probe is never called
    when it is set.
    """

    def _never(self):  # pragma: no cover - called only if the dispatch breaks
        raise AssertionError(
            "the ssh-agent probe must not run when the agent is not in the signing path"
        )

    def test_external_signer_never_probes_the_agent(self):
        # The regression. On this configuration the agent listing fails
        # unconditionally, so probing it at all is what produced the false stop.
        self.assertEqual(
            ipb.signing_state(
                "ssh",
                "/Applications/1Password.app/Contents/MacOS/op-ssh-sign",
                agent_probe=self._never,
                program_exists=lambda _p: True,
            ),
            "external-signer",
        )

    def test_external_signer_with_a_missing_binary_is_distinguished(self):
        # A configured signer that isn't on disk genuinely cannot sign, so this
        # is a real catch the old probe never had — a moved app bundle would
        # otherwise surface as a failed bootstrap commit.
        self.assertEqual(
            ipb.signing_state(
                "ssh",
                "/nonexistent/op-ssh-sign",
                agent_probe=self._never,
                program_exists=lambda _p: False,
            ),
            "external-signer-missing",
        )

    def test_agent_based_ssh_with_identities_is_ok(self):
        self.assertEqual(
            ipb.signing_state("ssh", None, agent_probe=lambda: 0),
            "agent-ok",
        )

    def test_agent_based_ssh_with_no_identities_is_locked(self):
        # The original measured case, preserved exactly: `ssh-add -l` exits 1
        # when the agent holds nothing, which is what a locked 1Password agent
        # looks like when it *is* in the signing path.
        self.assertEqual(
            ipb.signing_state("ssh", None, agent_probe=lambda: 1),
            "agent-locked",
        )

    def test_an_unreachable_agent_collapses_into_locked(self):
        # Exit 2, a different cause with an identical operator action.
        self.assertEqual(
            ipb.signing_state("ssh", None, agent_probe=lambda: 2),
            "agent-locked",
        )

    def test_an_empty_signer_program_falls_through_to_the_agent(self):
        # `git config --get` yields None for unset, but a configured-empty or
        # whitespace value must not read as "an external signer is configured".
        self.assertEqual(
            ipb.signing_state("ssh", "   ", agent_probe=lambda: 0),
            "agent-ok",
        )

    def test_unset_gpg_format_is_gpg_and_probes_nothing(self):
        # git defaults `gpg.format` to openpgp, so unset means gpg signing —
        # where an ssh-agent listing says nothing whatsoever.
        self.assertEqual(
            ipb.signing_state(None, None, agent_probe=self._never),
            "gpg",
        )

    def test_explicit_openpgp_is_gpg(self):
        self.assertEqual(
            ipb.signing_state("openpgp", None, agent_probe=self._never),
            "gpg",
        )

    def test_gpg_format_is_matched_case_and_space_insensitively(self):
        self.assertEqual(
            ipb.signing_state(" SSH ", None, agent_probe=lambda: 0),
            "agent-ok",
        )

    def test_gpg_wins_over_a_stray_signer_program(self):
        # `gpg.ssh.program` is inert under openpgp signing, so a leftover value
        # must not flip the verdict to an ssh configuration.
        self.assertEqual(
            ipb.signing_state(
                "openpgp",
                "/Applications/1Password.app/Contents/MacOS/op-ssh-sign",
                agent_probe=self._never,
                program_exists=lambda _p: True,
            ),
            "gpg",
        )

    def test_a_missing_probe_is_treated_as_locked(self):
        # Defensive: no caller omits it, but defaulting to "can sign" would
        # reintroduce a silent pass in the one case the probe exists to catch.
        self.assertEqual(ipb.signing_state("ssh", None), "agent-locked")


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
        self.assertEqual(out["base_repo"], "/repos/dropset")
        self.assertEqual(out["normalized_branch"], "eng-603")
        self.assertTrue(out["rename_needed"])

    def test_invalid_tag_exits_nonzero(self):
        code, out = self._run("not-a-tag", "eng-603", PORCELAIN)
        self.assertEqual(code, 1)
        self.assertFalse(out["tag_valid"])
        self.assertIsNone(out["tag"])

    def test_both_env_keys_are_null_without_the_flag(self):
        # Both keys are always present so the skill can read one stable shape.
        _, out = self._run("eng-603", "worktree-eng-603", PORCELAIN)
        self.assertIn("env_link", out)
        self.assertIn("secrets_env_link", out)
        self.assertIsNone(out["env_link"])
        self.assertIsNone(out["secrets_env_link"])

    def test_env_link_reports_its_outcome_with_the_flag(self):
        # End-to-end through the CLI: a temp base repo holding both operator
        # files, a temp worktree with neither. The porcelain names that temp
        # base, so the case never depends on a real checkout being present.
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp) / "base"
            worktree = Path(tmp) / "worktree"
            for repo in (base, worktree):
                (repo / "frontend").mkdir(parents=True)
                (repo / "infra" / "localnet").mkdir(parents=True)
            (base / "frontend" / ".env.local").write_text("K=v\n", encoding="utf-8")
            (base / ipb._SECRETS_ENV_REL).write_text("K=v\n", encoding="utf-8")
            porcelain = f"worktree {base}\nHEAD abc123\nbranch refs/heads/main\n"
            _, out = self._run(
                "eng-603",
                "worktree-eng-603",
                porcelain,
                ["--link-env", "--worktree-root", str(worktree)],
            )
            self.assertEqual(out["env_link"], "created")
            self.assertEqual(out["secrets_env_link"], "created")
            self.assertTrue((worktree / "frontend" / ".env.local").is_symlink())
            self.assertTrue((worktree / ipb._SECRETS_ENV_REL).is_symlink())

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
        self.assertEqual(out["secrets_env_link"], "no-base")

    def test_env_link_is_skipped_on_an_invalid_tag(self):
        # A run that fails validation stops the skill, so it must not leave a
        # filesystem mutation behind — for either file.
        with tempfile.TemporaryDirectory() as wt:
            frontend = Path(wt) / "frontend"
            frontend.mkdir()
            localnet = Path(wt) / "infra" / "localnet"
            localnet.mkdir(parents=True)
            code, out = self._run(
                "not-a-tag",
                "eng-603",
                PORCELAIN,
                ["--link-env", "--worktree-root", wt],
            )
            self.assertEqual(code, 1)
            self.assertIsNone(out["env_link"])
            self.assertIsNone(out["secrets_env_link"])
            self.assertFalse((frontend / ".env.local").exists())
            self.assertFalse((localnet / "secrets.local.env").exists())


if __name__ == "__main__":
    unittest.main()
