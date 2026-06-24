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

Stdlib only. This is a Python skill-tool under ``.claude/tools/`` — deliberately
**not** a Cargo workspace member (see ``CLAUDE.md`` → "Skill tooling").
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess

# A worktree tag: `eng-` followed by digits, case-insensitive.
_TAG_RE = re.compile(r"^eng-\d+$", re.IGNORECASE)

# The `aps` helper names worktree branches `worktree-eng-###`; the bare
# `eng-###` is what matches the Linear issue identifier.
_WORKTREE_PREFIX = "worktree-"


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
        help="the current branch name; if omitted, runs "
        "`git branch --show-current`",
    )
    parser.add_argument(
        "--porcelain-file",
        help="path to a file holding `git worktree list --porcelain` output; "
        "if omitted, the tool runs that command itself",
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
    }
    print(json.dumps(result, indent=2))
    # Exit non-zero on an invalid tag so the skill can stop and ask, without
    # parsing the JSON just to learn the tag was malformed.
    return 0 if tag is not None else 1


if __name__ == "__main__":
    raise SystemExit(main())
