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
``init_pr_branch.signing_state``, the one classifier) and the signer's first
error line. Exit 2 is a probe that could not run at all — no ``HEAD``, not a
repository, no committer identity — which says nothing about signing: an
unsigned ``commit-tree`` runs first, so only a failure that ``-S`` alone adds
reads as locked. A signer still silent after ``TIMEOUT_SECONDS`` (an unlock
prompt nobody is answering) reads as locked too.

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

# Long enough for a human to answer an unlock prompt, short enough that an
# unattended probe reports instead of hanging.
TIMEOUT_SECONDS = 90

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


def _first_error(proc: subprocess.CompletedProcess) -> str:
    errors = [line for line in (proc.stderr or "").strip().splitlines() if line.strip()]
    return errors[0] if errors else f"exit {proc.returncode}"


def _run(cmd: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        errors="replace",
        check=False,
        timeout=TIMEOUT_SECONDS,
    )


def probe() -> tuple[int, str]:
    """Run the probe. Returns ``(exit code, one line)``."""
    if not has_head():
        return 2, "signing-probe | cannot run: no HEAD commit here"
    # Unsigned first: a failure here (no identity, a broken object store) is
    # git's, not the signer's, and must not read as an unlock instruction.
    unsigned = _run(["--no-gpg-sign" if arg == "-S" else arg for arg in _PROBE])
    if unsigned.returncode != 0:
        return 2, f"signing-probe | cannot run: {_first_error(unsigned)}"
    instruction = unlock_instruction(
        _git_config("gpg.format"), _git_config("gpg.ssh.program")
    )
    try:
        proc = _run(_PROBE)
    except subprocess.TimeoutExpired:
        return 1, (
            f"signing-probe | LOCKED | {instruction} | no answer from the signer "
            f"within {TIMEOUT_SECONDS}s"
        )
    if proc.returncode == 0:
        sha = proc.stdout.strip()[:12]
        return 0, f"signing-probe | signed | {sha} (dangling, no ref moved)"
    return 1, f"signing-probe | LOCKED | {instruction} | {_first_error(proc)}"


def main() -> int:
    code, line = probe()
    print(line, file=sys.stdout if code == 0 else sys.stderr)
    return code


if __name__ == "__main__":
    raise SystemExit(main())
