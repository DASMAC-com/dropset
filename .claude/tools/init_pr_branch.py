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

With ``--link-env`` it also performs **filesystem** mutations: symlinking two
operator-owned, git-ignored files from the base repo into this worktree, so
neither has to be copied by hand. It never clobbers an existing path, and each
file's outcome is reported separately, since a machine can legitimately have one
and not the other:

* ``frontend/.env.local`` (``env_link``) — so ``pnpm dev`` / ``make frontend``
  pick up the same env;
* ``infra/localnet/secrets.local.env`` (``secrets_env_link``) — the local
  secrets enclave's one operator file, holding the vault name and one ``op://``
  reference per credential. Unlike ``.claude/settings.local.json``, nothing
  resolves this path through a worktree to the main checkout, so without the
  link a fresh worktree has none and ``make fx-collectors-up`` there silently
  falls back to whatever keys happen to be exported. Both consumers follow a
  symlink: the ``Makefile``'s ``-include`` and its ``[ -f ]`` guard (``test -f``
  follows links, unlike ``-h``), and ``op run --env-file``.

That step used to be prose in the skill — a Glob pair
plus a bare ``ln -s`` against an **absolute base-repo path**, which re-prompted
the file-access gate on *every* bootstrap: the allow-rule would land in the new
worktree's ``settings.local.json``, and every ``/init-pr`` runs in a brand-new
worktree that has none. Folding it in here means the skill's single call carries
no absolute path for that heuristic to gate.

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

# The files `--link-env` mirrors, relative to a repo root. Both are gitignored,
# so neither symlink is ever tracked, and each is reported under its own key
# because a machine can legitimately have one and not the other: the frontend
# env is a dev convenience, the enclave file is credential resolution.
_ENV_REL = os.path.join("frontend", ".env.local")
_SECRETS_ENV_REL = os.path.join("infra", "localnet", "secrets.local.env")
_NODE_MODULES_REL = os.path.join("frontend", "node_modules")


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


def link_env(base_repo: str | None, worktree_root: str, rel: str = _ENV_REL) -> str:
    """Symlink ``rel`` in ``worktree_root`` to the base repo's copy. Returns the
    outcome as one of five strings:

    * ``"no-base"`` — no worktree has ``main`` checked out, so there is no base
      repo to link from;
    * ``"exists"`` — this worktree already has the path, so leave it alone (it
      may be a real file someone placed deliberately);
    * ``"no-source"`` — nothing to link: the base repo has no such file, or this
      worktree has no containing directory to link it into;
    * ``"created"`` — the symlink was created;
    * ``"failed"`` — the symlink couldn't be created (an unwritable parent
      directory, a read-only mount, a racing writer).

    Called once per mirrored file, so each gets its own outcome. That matters
    because the two are independent: a machine that has never run the frontend
    has no ``.env.local``, and one that has never touched the FX collectors has
    no enclave file, and neither absence says anything about the other.

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

    source = os.path.join(base_repo, rel)
    dest = os.path.join(worktree_root, rel)

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


def node_modules_state(worktree_root: str) -> str:
    """Whether this worktree has ``frontend/node_modules``.

    Reported so the skill acts on a measured fact rather than on a prediction
    about which hooks the diff will trip. A cold worktree has no
    ``node_modules``, and the ``biome`` and ``tsc`` hooks shell out to
    ``pnpm --dir frontend exec …`` — so the first full ``make lint`` fails on
    both, *whatever* the branch touches, with an error that says nothing about
    the diff. The conditional phrasing it replaces ("install when the task
    touches ``frontend/**``") loses reliably to "this diff doesn't touch the
    frontend", and the deferral is then paid at lint time as a failed sweep,
    an install, and a re-verify.

    Unlike the two symlink fields this mutates nothing, so it is reported on
    every run rather than riding ``--link-env``.
    """
    if not os.path.isdir(os.path.join(worktree_root, "frontend")):
        return "no-frontend"
    if os.path.isdir(os.path.join(worktree_root, _NODE_MODULES_REL)):
        return "present"
    return "absent"


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
        help="also symlink frontend/.env.local and "
        "infra/localnet/secrets.local.env from the base repo into this worktree "
        "(never clobbers an existing path, never raises); the outcomes are "
        "reported as `env_link` and `secrets_env_link` in the JSON",
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
        # `null` when --link-env wasn't asked for, so each key's shape is
        # stable. Also skipped on an invalid tag: that exits non-zero for the
        # skill to stop and ask, and a run that fails validation shouldn't
        # leave a filesystem mutation behind.
        "env_link": link_env(base_repo, args.worktree_root, _ENV_REL)
        if args.link_env and tag is not None
        else None,
        "secrets_env_link": link_env(base_repo, args.worktree_root, _SECRETS_ENV_REL)
        if args.link_env and tag is not None
        else None,
        # Read-only, so unconditional: `present` / `absent` / `no-frontend`.
        "frontend_node_modules": node_modules_state(args.worktree_root),
    }
    print(json.dumps(result, indent=2))
    # Exit non-zero on an invalid tag so the skill can stop and ask, without
    # parsing the JSON just to learn the tag was malformed.
    return 0 if tag is not None else 1


if __name__ == "__main__":
    raise SystemExit(main())
