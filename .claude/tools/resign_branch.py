#!/usr/bin/env python3
# cspell:word autosquash
"""Count a branch's unsigned commits, and sign them all in one pass at the
merge gate.

Branch protection requires a verified signature on every commit **on main**;
the ruleset targets the default branch only, so a feature branch accepts
unsigned pushes. That makes a locked signer (the 1Password app timing out
mid-session) a reason to defer signing, never to stop: a session commits and
pushes unsigned, and the one place the operator is genuinely needed is the
merge gate, where this tool signs the whole branch at once — one unlock instead
of one per commit (``docs/conventions/commits-and-prs.md`` → "Signing defers to
the merge gate").

Usage::

    python3 .claude/tools/resign_branch.py count [--base main]
    python3 .claude/tools/resign_branch.py resign [--base main] [--no-push]

``count`` prints one line — how many of the commits since the merge base with
``origin/<base>`` carry no signature — and exits 0. A signature is the
``gpgsig`` header on the commit object, read with ``git cat-file``: presence,
not verification, which would demand an ``allowedSignersFile`` and says
nothing about whether the merge gate's check will see one.

``resign`` signs any unsigned commits, then makes the remote match. With
unsigned commits it runs `signing_probe.py`'s probe first and exits 1 with its
``LOCKED`` line, moving no ref, so the ask to the operator is the single
"unlock and I sign and enqueue". Then it replays the branch onto **its own
merge base** (``git rebase --force-rebase --no-autosquash --gpg-sign``) — a
re-sign, never a rebase onto a newer main. A linear branch replayed onto its
own merge base cannot conflict and keeps its tree; a branch carrying a **merge
commit** would be linearized, so it is refused up front, and a tree that
differs after the replay anyway is reset to the old head.

Unless ``--no-push``, it then pushes whenever ``origin/<branch>`` differs from
``HEAD`` — **even with nothing to sign**, so a re-run after a failed push still
lands the signed branch — with a lease pinned to the remote sha read at the
start. It refuses before anything moves (and fetches nothing, so the
remote-tracking ref every worktree shares is left alone) when the remote tip
is a commit this branch never held — neither an ancestor of ``HEAD`` nor a tip
in the branch's own reflog — since forcing would drop another session's work.

Exit 2 is anything that is not the signer: a dirty tree, a merge commit, no
merge base, a failed rebase (aborted), a re-sign that changed the tree or left
a commit unsigned (reset), an unseen remote tip, a failed push. After a failed
push the branch is already re-signed locally with its tree unchanged, and a
re-run pushes it. Every line starts ``resign-branch |``.

Standard library only. A Python skill-tool under ``.claude/tools/`` —
deliberately **not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill
tooling"). It rides the existing ``python3 .claude/tools/`` grant, so the
merge-gate pass needs no ``git rebase`` allow-rule. Tests live in
``tests/test_resign_branch.py``.
"""

from __future__ import annotations

import argparse
import subprocess
import sys

from signing_probe import probe

PREFIX = "resign-branch |"


class Refused(Exception):
    """A non-signer failure; the message is the reported line's tail."""


def _git(*args: str, check: bool = True) -> subprocess.CompletedProcess:
    proc = subprocess.run(
        ["git", *args], capture_output=True, text=True, errors="replace", check=False
    )
    if check and proc.returncode != 0:
        err = (proc.stderr or proc.stdout).strip().splitlines()
        raise Refused(f"`git {args[0]}` failed: {err[-1] if err else proc.returncode}")
    return proc


def is_signed(commit_object: str) -> bool:
    """Whether a raw commit object carries a signature header.

    The headers end at the first blank line; a ``gpgsig`` past it is message
    text, not a signature.
    """
    headers = commit_object.split("\n\n", 1)[0]
    return any(
        line.startswith(("gpgsig ", "gpgsig-sha256 ")) for line in headers.splitlines()
    )


def merge_base(base: str) -> str:
    return _git("merge-base", "HEAD", f"origin/{base}").stdout.strip()


def unsigned_commits(base_sha: str) -> tuple[list[str], int]:
    """The unsigned commits in ``base_sha..HEAD`` (oldest first), and the total."""
    shas = _git("rev-list", "--reverse", f"{base_sha}..HEAD").stdout.split()
    unsigned = [s for s in shas if not is_signed(_git("cat-file", "commit", s).stdout)]
    return unsigned, len(shas)


def _summary(unsigned: list[str], total: int) -> str:
    if not unsigned:
        return f"{PREFIX} 0 unsigned of {total}"
    short = " ".join(s[:9] for s in unsigned[:5])
    more = f" (+{len(unsigned) - 5})" if len(unsigned) > 5 else ""
    return f"{PREFIX} {len(unsigned)} unsigned of {total} | {short}{more}"


def count(base: str) -> tuple[int, str]:
    unsigned, total = unsigned_commits(merge_base(base))
    return 0, _summary(unsigned, total)


def _remote_sha(branch: str) -> str:
    out = _git("ls-remote", "origin", f"refs/heads/{branch}").stdout
    return out.split()[0] if out.strip() else ""


def _was_branch_tip(branch: str, sha: str, head: str) -> bool:
    """Whether ``sha`` is work this branch has held: an ancestor of ``head`` or
    a tip recorded in the branch's own reflog (which keeps the pre-rebase tip
    of a branch since rebased onto a newer base).

    Never the remote-tracking ref: worktrees share it, so another checkout's
    fetch would mark a remote commit "seen" that this branch never took in.
    """
    if not _git("merge-base", "--is-ancestor", sha, head, check=False).returncode:
        return True
    reflog = _git("reflog", "show", "--format=%H", f"refs/heads/{branch}", check=False)
    return sha in reflog.stdout.split()


def _sign_in_place(mb: str, old_head: str, unsigned: list[str]) -> tuple[int, str]:
    """Re-sign ``mb..HEAD``; ``(1, line)`` on a locked signer, else ``(0, "")``."""
    if _git("status", "--porcelain", "--untracked-files=no").stdout.strip():
        raise Refused("uncommitted changes; commit them (unsigned is fine) first")
    if _git("rev-list", "--merges", f"{mb}..HEAD").stdout.strip():
        # A plain rebase would linearize the merge — a real replay that can
        # conflict or change the tree, not a re-sign.
        raise Refused("the branch carries a merge commit; rebase it linear first")
    code, line = probe()
    if code == 1:
        return 1, f"{PREFIX} {len(unsigned)} unsigned, nothing moved | {line}"
    if code != 0:
        raise Refused(line)
    old_tree = _git("rev-parse", "HEAD^{tree}").stdout.strip()
    # `--no-autosquash`: a configured `rebase.autoSquash` would otherwise fold
    # `fixup!` commits — history changed under an unchanged tree.
    done = _git(
        "rebase", "--force-rebase", "--no-autosquash", "--gpg-sign", mb, check=False
    )
    if done.returncode != 0:
        _git("rebase", "--abort", check=False)
        raise Refused(f"rebase failed and was aborted, HEAD is {old_head[:9]} again")
    left, _ = unsigned_commits(mb)
    changed = _git("rev-parse", "HEAD^{tree}").stdout.strip() != old_tree
    if left or changed:
        _git("reset", "-q", "--hard", old_head)
        why = (
            "the tree changed" if changed else f"{len(left)} commit(s) stayed unsigned"
        )
        raise Refused(f"{why} in the re-sign; HEAD reset to {old_head[:9]}")
    return 0, ""


def resign(base: str, push: bool) -> tuple[int, str]:
    mb = merge_base(base)
    unsigned, total = unsigned_commits(mb)
    if not unsigned and not push:
        return 0, f"{PREFIX} nothing to sign | {total} commit(s), all signed"
    branch = _git("symbolic-ref", "--short", "HEAD").stdout.strip()
    old_head = _git("rev-parse", "HEAD").stdout.strip()
    remote = _remote_sha(branch) if push else ""
    # No fetch: it would move the tracking ref every worktree shares onto the
    # very commit being refused. A tip this branch held is already local.
    if remote and remote != old_head and not _was_branch_tip(branch, remote, old_head):
        raise Refused(
            f"origin/{branch} ({remote[:9]}) holds commits this branch never "
            "held; reconcile by hand, never force here"
        )
    signed = f"signed {len(unsigned)} of {total}" if unsigned else "nothing to sign"
    if unsigned:
        code, line = _sign_in_place(mb, old_head, unsigned)
        if code:
            return code, line
    if not push:
        return 0, f"{PREFIX} {signed} | not pushed"
    head = _git("rev-parse", "HEAD").stdout.strip()
    if remote == head:
        return 0, f"{PREFIX} {signed} | origin/{branch} already matches"
    # Pushed even with nothing to sign: a re-run after a failed push holds
    # a signed branch the remote does not have yet.
    if remote:
        _git("push", f"--force-with-lease={branch}:{remote}", "origin", branch)
    else:
        _git("push", "-u", "origin", branch)
    return 0, f"{PREFIX} {signed} | pushed origin/{branch}"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="resign_branch.py",
        description="Count, or sign at the merge gate, a branch's unsigned commits.",
    )
    sub = parser.add_subparsers(dest="cmd", required=True)
    for name in ("count", "resign"):
        p = sub.add_parser(name)
        p.add_argument("--base", default="main", help="the PR's base branch")
        if name == "resign":
            p.add_argument("--no-push", action="store_true", help="sign locally only")
    args = parser.parse_args(argv)
    try:
        if args.cmd == "count":
            code, line = count(args.base)
        else:
            code, line = resign(args.base, push=not args.no_push)
    except Refused as exc:
        code, line = 2, f"{PREFIX} refused | {exc}"
    print(line, file=sys.stdout if code == 0 else sys.stderr)
    return code


if __name__ == "__main__":
    raise SystemExit(main())
