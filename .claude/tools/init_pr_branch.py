#!/usr/bin/env python3
"""``init-pr`` branch/worktree helper — the deterministic string/path checks the
``init-pr`` skill used to do inline as shell + prose.

Given the ``git worktree list --porcelain`` output, the current branch name, and
a tag, it resolves three things the skill no longer has to hand-parse:

* **base-repo path** — the worktree whose branch is ``refs/heads/main``;
* **branch normalization** — strip a leading ``worktree-`` prefix so
  ``worktree-eng-603`` becomes the bare ``eng-603`` that matches the Linear
  issue, reporting whether a rename is needed;
* **tag validation** — the resolved tag must match ``eng-###``
  (case-insensitive), normalized to lowercase.

By default it runs the two **read-only** git reads itself
(``git worktree list --porcelain`` and ``git branch --show-current``) and prints
the answers as JSON, so the skill needs a single call and no inline parsing.
It performs **no** git mutation — the one mutation, the branch rename, stays the
skill's ``git branch -m`` call. ``--porcelain-file`` and ``--branch`` override
the git reads (used by the tests, and handy for a dry run).

With ``--link-env`` it also performs one **filesystem** mutation: symlinking
this worktree's ``frontend/.env.local`` to the base repo's copy, so ``pnpm dev``
picks up the same env without a manual copy (``.env*`` is gitignored, so the
link is never tracked). That step used to be prose in the skill — a Glob pair
plus a bare ``ln -s`` against an **absolute base-repo path**, which re-prompted
the file-access gate on *every* bootstrap: the allow-rule would land in the new
worktree's ``settings.local.json``, and every ``/init-pr`` runs in a brand-new
worktree that has none. Folding it in here means the skill's single call carries
no absolute path for that heuristic to gate. It never clobbers an existing file.

Stdlib only. This is a Python skill-tool under ``.claude/tools/`` — deliberately
**not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess

# A worktree tag: `eng-` followed by digits, case-insensitive.
_TAG_RE = re.compile(r"^eng-\d+$", re.IGNORECASE)

# The `aps` helper names worktree branches `worktree-eng-###`; the bare
# `eng-###` is what matches the Linear issue identifier.
_WORKTREE_PREFIX = "worktree-"

# The env file `--link-env` mirrors, relative to a repo root. Gitignored, so the
# symlink is never tracked.
_ENV_REL = os.path.join("frontend", ".env.local")


def parse_base_repo(porcelain: str) -> str | None:
    """Return the path of the worktree whose branch is ``refs/heads/main`` (the
    base repo), or ``None`` if no worktree has ``main`` checked out.

    ``git worktree list --porcelain`` emits stanzas separated by blank lines,
    each with a ``worktree <path>`` line and (for a branch checkout) a
    ``branch <ref>`` line.
    """
    current_path: str | None = None
    for raw in porcelain.splitlines():
        line = raw.strip()
        if line.startswith("worktree "):
            current_path = line[len("worktree ") :].strip()
        elif line.startswith("branch "):
            ref = line[len("branch ") :].strip()
            if ref == "refs/heads/main" and current_path:
                return current_path
        elif not line:
            current_path = None
    return None


def normalize_tag(tag: str) -> str | None:
    """Validate ``tag`` against ``eng-###`` (case-insensitive) and return it
    lowercased, or ``None`` if it doesn't match.
    """
    tag = tag.strip()
    if _TAG_RE.match(tag):
        return tag.lower()
    return None


def normalize_branch(branch: str) -> tuple[str, bool]:
    """Return ``(normalized_branch, rename_needed)``.

    A ``worktree-eng-###`` branch (the ``aps`` default) is stripped to the bare
    ``eng-###``; any other name is left as-is. ``rename_needed`` is ``True`` only
    when the leading ``worktree-`` prefix was actually present.
    """
    branch = branch.strip()
    if branch.startswith(_WORKTREE_PREFIX):
        stripped = branch[len(_WORKTREE_PREFIX) :]
        return stripped, True
    return branch, False


def link_env(base_repo: str | None, worktree_root: str) -> str:
    """Symlink ``frontend/.env.local`` in ``worktree_root`` to the base repo's
    copy. Returns the outcome as one of five strings:

    * ``"no-base"`` — no worktree has ``main`` checked out, so there is no base
      repo to link from;
    * ``"exists"`` — this worktree already has the path, so leave it alone (it
      may be a real file someone placed deliberately);
    * ``"no-source"`` — nothing to link: the base repo has no env file, or this
      worktree has no ``frontend/`` directory to link it into;
    * ``"created"`` — the symlink was created;
    * ``"failed"`` — the symlink couldn't be created (an unwritable
      ``frontend/``, a read-only mount, a racing writer).

    Never clobbers: the ``"exists"`` check uses ``lexists``, so even a dangling
    symlink is left as found rather than replaced.

    Never raises. The caller evaluates this while building the result dict, so
    an escaping ``OSError`` would abort before any JSON is printed — costing
    the skill the ``tag_valid`` / ``base_repo`` / ``rename_needed`` answers it
    reads from the same call, over an optional convenience link. ``"failed"``
    keeps the bootstrap contract intact.
    """
    if base_repo is None:
        return "no-base"

    source = os.path.join(base_repo, _ENV_REL)
    dest = os.path.join(worktree_root, _ENV_REL)

    # `lexists`, not `exists`: a symlink whose target is gone still counts as
    # occupied — replacing it silently is exactly the clobber we're avoiding.
    if os.path.lexists(dest):
        return "exists"
    if not os.path.exists(source):
        return "no-source"
    if not os.path.isdir(os.path.dirname(dest)):
        return "no-source"

    try:
        os.symlink(source, dest)
    except OSError:
        return "failed"
    return "created"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="init_pr_branch.py",
        description=(
            "Resolve the base-repo path, the normalized branch name, and the "
            "validated tag for the init-pr bootstrap. Prints JSON; performs no "
            "git mutation."
        ),
    )
    parser.add_argument(
        "--tag",
        required=True,
        help="the Linear tag to validate (e.g. eng-603, case-insensitive)",
    )
    parser.add_argument(
        "--branch",
        help="the current branch name; if omitted, runs `git branch --show-current`",
    )
    parser.add_argument(
        "--porcelain-file",
        help="path to a file holding `git worktree list --porcelain` output; "
        "if omitted, the tool runs that command itself",
    )
    parser.add_argument(
        "--link-env",
        action="store_true",
        help="also symlink frontend/.env.local from the base repo into this "
        "worktree (never clobbers an existing path, never raises); the outcome "
        "is reported as `env_link` in the JSON",
    )
    parser.add_argument(
        "--worktree-root",
        default=".",
        help="the worktree root --link-env writes into (default: the current "
        "directory, which is where the skill runs this from)",
    )
    args = parser.parse_args(argv)

    if args.porcelain_file:
        with open(args.porcelain_file, encoding="utf-8") as handle:
            porcelain = handle.read()
    else:
        porcelain = subprocess.run(
            ["git", "worktree", "list", "--porcelain"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
    if args.branch is not None:
        branch = args.branch
    else:
        branch = subprocess.run(
            ["git", "branch", "--show-current"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()

    tag = normalize_tag(args.tag)
    base_repo = parse_base_repo(porcelain)
    normalized_branch, rename_needed = normalize_branch(branch)

    result = {
        "tag": tag,
        "tag_valid": tag is not None,
        "base_repo": base_repo,
        "current_branch": branch.strip(),
        "normalized_branch": normalized_branch,
        "rename_needed": rename_needed,
        # `null` when --link-env wasn't asked for, so the key's shape is
        # stable. Also skipped on an invalid tag: that exits non-zero for the
        # skill to stop and ask, and a run that fails validation shouldn't
        # leave a filesystem mutation behind.
        "env_link": link_env(base_repo, args.worktree_root)
        if args.link_env and tag is not None
        else None,
    }
    print(json.dumps(result, indent=2))
    # Exit non-zero on an invalid tag so the skill can stop and ask, without
    # parsing the JSON just to learn the tag was malformed.
    return 0 if tag is not None else 1


if __name__ == "__main__":
    raise SystemExit(main())
