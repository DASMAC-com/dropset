#!/usr/bin/env python3
"""Prove commit signing works *now*, without moving a ref.

Branch protection requires a verified signature on every commit, and with
``commit.gpgsign`` set a rebase re-signs every commit it replays — so a signer
that locked while the session idled (a 1Password app timing out, an ssh agent
emptied) kills a rebase part-way and leaves the worktree mid-rebase, or fails a
checkpoint commit right before a long unattended wait. A configuration read
cannot catch that: an external signer is exactly where it was when its backend
locks. The only check that proves signing is a signed commit.

So this writes one: a **dangling** signed commit of ``HEAD``'s tree,
parented on ``HEAD``::

    git commit-tree -S -p HEAD -m signing-probe HEAD^{tree}

No ref moves and the object is garbage-collected later, so the probe is free to
run before any rebase, checkpoint or bootstrap. Usage::

    python3 .claude/tools/signing_probe.py

Prints one line and exits 0 on ``signed``; on ``locked`` exits 1 with the
unlock instruction for this machine's signing configuration (read with
``init_pr_branch.signing_state``, the one classifier) and the signer's last
error line. Exit 2 is a probe that could not run at all — no ``HEAD``, not a
repository — which says nothing about signing.

Standard library only. A Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling"). Tests live in ``tests/test_signing_probe.py``.
"""

from __future__ import annotations

import os
import subprocess
import sys

from init_pr_branch import _git_config, signing_state

_PROBE = [
    "git",
    "commit-tree",
    "-S",
    "-p",
    "HEAD",
    "-m",
    "signing-probe",
    "HEAD^{tree}",
]

# What to do about a locked signer, keyed by `signing_state`'s verdict.
_UNLOCK = {
    "external-signer": "unlock the {program} backend (the 1Password app for "
    "op-ssh-sign), then re-run this probe",
    "external-signer-missing": "gpg.ssh.program ({program}) does not resolve — "
    "reinstall it or fix the setting",
    # The only agent verdict a configuration-only call can return.
    "agent-locked": "unlock the ssh agent or load the signing key, then re-run "
    "this probe",
    "gpg": "unlock gpg-agent (sign anything once to prompt), then re-run this probe",
}


def unlock_instruction(gpg_format: str | None, ssh_program: str | None) -> str:
    """The operator action for a failed probe on this configuration.

    The classifier is called configuration-only — no agent probe, since the
    signed commit already answered whether signing works.
    """
    state = signing_state(gpg_format, ssh_program, agent_probe=lambda: 1)
    program = os.path.basename(ssh_program or "") or "signer"
    return _UNLOCK[state].format(program=program)


def has_head() -> bool:
    return (
        subprocess.run(
            ["git", "rev-parse", "--verify", "-q", "HEAD"],
            capture_output=True,
            check=False,
        ).returncode
        == 0
    )


def probe() -> tuple[int, str]:
    """Run the probe. Returns ``(exit code, one line)``."""
    if not has_head():
        return 2, "signing-probe | cannot run: no HEAD commit here"
    proc = subprocess.run(
        _PROBE, capture_output=True, text=True, errors="replace", check=False
    )
    if proc.returncode == 0:
        sha = proc.stdout.strip()[:12]
        return 0, f"signing-probe | signed | {sha} (dangling, no ref moved)"
    errors = [line for line in proc.stderr.strip().splitlines() if line.strip()]
    detail = errors[0] if errors else f"exit {proc.returncode}"
    instruction = unlock_instruction(
        _git_config("gpg.format"), _git_config("gpg.ssh.program")
    )
    return 1, f"signing-probe | LOCKED | {instruction} | {detail}"


def main() -> int:
    code, line = probe()
    print(line, file=sys.stdout if code == 0 else sys.stderr)
    return code


if __name__ == "__main__":
    raise SystemExit(main())
