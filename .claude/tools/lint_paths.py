#!/usr/bin/env python3
"""``make lint`` file-set resolver — run pre-commit over every file in the
working tree, **including the ones git doesn't track yet**.

``pre-commit run --all-files`` sounds exhaustive but isn't: it enumerates its
file list with ``git ls-files``, which sees only what is **in the index**. A
brand-new file that has never been ``git add``ed is not in that list, so no hook
ever looks at it — cspell, rustfmt, biome, yamllint, mdformat, all of them. The
run reports a cheerful ``Passed`` for a file it never opened.

That is a real, load-bearing divergence from CI, because CI checks out a branch
where the file **is** committed and therefore **is** tracked. The observed shape
(PR #308): a new venue module was written, ``make lint`` passed locally, and the
CI Lint job failed on cspell violations in that same file. Nothing about the
command or the config differed — the only variable was whether the path had
reached the index. Reproduced deterministically on a clean tree with a fixture
holding one nonsense word: untracked ⇒ ``cspell...Passed``; ``git add`` the
byte-identical file ⇒ ``cspell...Failed``.

So this tool builds the file list itself:

    git ls-files --cached --others --exclude-standard -z

— tracked **plus** untracked-but-not-ignored — filters it to paths that still
exist on disk, and hands the result to ``pre-commit run --files …``. The local
set is then a **superset** of CI's, which is the safe direction: a violation
that CI would catch is caught locally first, on the same tree, and reported as
the actual violation rather than as a "you forgot to stage something" nag.

Deliberately **not** applied to the CI workflow, which stays on ``--all-files``.
CI's checkout is clean, so for tracked files the two are identical there — but
CI runs ``make decks-build`` before linting, and that step leaves build output
in the tree. Under ``--all-files`` those artifacts are invisible (untracked);
under this tool they would suddenly be linted. Superset locally, unchanged in
CI.

**Do not "optimize" this back to ``--all-files``.** The chunked, parallel cspell
batches visible in the output (``Files checked: 36`` repeated) are pre-commit's
ordinary xargs partitioning for concurrency — they were once suspected of hiding
the violation and they do not: every chunk runs, and the chunk holding a bad
file reports it. Chunking is not the bug; the file list is.

Stdlib only. This is a Python skill-tool under ``.claude/tools/`` — deliberately
**not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys

# The lint hook set. Matches the `--config` CI's Lint job passes, so the two
# runs differ only in their file list (and this tool's list is the superset).
DEFAULT_CONFIG = "cfg/pre-commit-lint.yml"

# Tracked (`--cached`) plus untracked-but-not-gitignored (`--others` with
# `--exclude-standard`), NUL-separated so a path containing a space — or, in
# principle, a newline — survives the round trip.
_LS_FILES = [
    "git",
    "ls-files",
    "--cached",
    "--others",
    "--exclude-standard",
    "-z",
]


def repo_root() -> str:
    """Return the working tree's top-level directory.

    ``git ls-files`` reports paths relative to the **current** directory, and
    pre-commit insists on running from the root anyway, so both subprocesses are
    pinned here rather than trusting the caller's cwd.
    """
    return subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()


def parse_ls_files(raw: str) -> list[str]:
    """Split NUL-separated ``git ls-files -z`` output into a deduped, sorted
    path list.

    Dedup matters because ``--cached --others`` can report the same path twice
    (an unmerged entry, say); handing pre-commit a duplicate would lint it
    twice for no benefit.
    """
    return sorted({path for path in raw.split("\0") if path})


def existing(paths: list[str], root: str) -> list[str]:
    """Drop paths that aren't present in the working tree.

    A file deleted from the worktree but still in the index is listed by
    ``--cached``, and passing pre-commit a path that isn't there makes hooks
    fail on a missing file rather than on anything real.

    Uses ``lexists``, so a symlink counts as present even when its target is
    gone — a tracked symlink is in CI's list too, and silently dropping it here
    would reopen the very gap this tool closes.
    """
    return [p for p in paths if os.path.lexists(os.path.join(root, p))]


def pre_commit_cmd(config: str, hook_args: list[str], files: list[str]) -> list[str]:
    """Build the ``pre-commit run`` argv.

    Order matters and is easy to get subtly wrong: a hook id is a **positional**
    argument, so anything forwarded by the caller has to land before the
    ``--files`` list — once ``--files`` starts consuming, every remaining token
    is a path.
    """
    return ["pre-commit", "run", "--config", config, *hook_args, "--files", *files]


def lint_files(root: str) -> list[str]:
    """Resolve the full set of files ``make lint`` should check, under ``root``."""
    raw = subprocess.run(
        _LS_FILES,
        capture_output=True,
        text=True,
        check=True,
        cwd=root,
    ).stdout
    return existing(parse_ls_files(raw), root)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="lint_paths.py",
        description=(
            "Run pre-commit over every file in the working tree, including "
            "untracked ones that `--all-files` silently skips. Extra arguments "
            "after `--` are forwarded to pre-commit (e.g. a single hook id)."
        ),
    )
    parser.add_argument(
        "--config",
        default=DEFAULT_CONFIG,
        help=f"the pre-commit config to run (default: {DEFAULT_CONFIG})",
    )
    parser.add_argument(
        "--print",
        action="store_true",
        dest="print_only",
        help="print the resolved file list, one path per line, and exit "
        "without running pre-commit",
    )
    parser.add_argument(
        "hook_args",
        nargs="*",
        help="arguments forwarded verbatim to `pre-commit run` (pass them after `--`)",
    )
    args = parser.parse_args(argv)

    root = repo_root()
    files = lint_files(root)

    if args.print_only:
        for path in files:
            print(path)
        return 0

    # Deliberately non-zero. An empty set means the resolver found nothing to
    # check, and reporting success for a run that opened no files is the exact
    # failure this tool exists to remove — just arrived at from the other end.
    # A working tree always has files, so this fires only when something is
    # wrong (a bad cwd, a future refactor of the resolver), and it should fire
    # loudly rather than hand back a green lint.
    if not files:
        print(
            "lint-paths: resolved zero files to lint — refusing to report success",
            file=sys.stderr,
        )
        return 1

    return subprocess.run(
        pre_commit_cmd(args.config, args.hook_args, files),
        cwd=root,
    ).returncode


if __name__ == "__main__":
    raise SystemExit(main())
