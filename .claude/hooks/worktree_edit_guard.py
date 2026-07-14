#!/usr/bin/env python3
"""PreToolUse guard: block a file edit aimed at the base repo from a worktree.

In a worktree session the build, tests, and `cargo`/`pnpm` all run against
the *worktree* checkout. Editing a file through its **base-repo absolute
path** (`/…/dropset/foo.rs`) instead of the worktree path
(`/…/dropset/.claude/worktrees/<tag>/foo.rs`) writes to a copy the worktree
build never sees — so a new test "doesn't appear," a fix "doesn't take," and
the mistake is only caught after a wasted rebuild (the recurring
worktree edit-path pitfall). This guard catches it at the tool call.

It fires only when the active checkout *is* a worktree (its root contains
`/.claude/worktrees/`), and only for the file-mutating tools (`Edit`,
`Write`, `MultiEdit`, `NotebookEdit`). A `Read` of a base path is merely
wasteful, not corrupting, so it is left alone. On a match it exits 2 with a
reason on stderr — which Claude Code feeds back to the model — naming the
worktree-local path to use instead.

Two carve-outs let the legitimate base writes through:

* the base `.claude/settings.json` / `settings.local.json` — `firm-perms`
  and `firm_last.py` write the base allowlist on purpose; and
* the env escape `ALLOW_BASE_REPO_EDITS=1`, for a rare deliberate base edit.

The guard fails *open*: any missing field or parse problem returns 0 rather
than wedging the session. Relative paths are always allowed (they resolve
against the worktree cwd, so they can't be a stray base edit).

Run the built-in cases with `--self-test` (no stdin needed).
"""

import json
import os
import sys

# The tools that write a file — the only ones whose base-path target corrupts
# the worktree build. Read is deliberately excluded (wasteful, not corrupting).
EDIT_TOOLS = ("Edit", "Write", "MultiEdit", "NotebookEdit")

# Marker in the checkout root that identifies a worktree checkout.
WORKTREE_MARKER = os.sep + os.path.join(".claude", "worktrees") + os.sep

# Env var that disables the guard for a deliberate base edit.
ESCAPE_ENV = "ALLOW_BASE_REPO_EDITS"

DENY_MESSAGE = (
    "Blocked: this edit targets the base-repo path\n"
    "  {target}\n"
    "but this is a worktree session rooted at\n"
    "  {worktree}\n"
    "Edits to the base checkout are invisible to this worktree's "
    "cargo/pnpm/test runs, so the change won't take and you'll rebuild for "
    "nothing (the worktree edit-path pitfall). Edit the worktree copy "
    "instead:\n"
    "  {suggested}\n\n"
    "If you truly mean to write the base checkout, set "
    "{escape}=1 in the environment to bypass this guard."
)

# A sibling worktree has no sensible worktree-local equivalent path, so its
# deny message names the target without a (would-be-nonsense) "edit this
# instead" suggestion.
SIBLING_DENY_MESSAGE = (
    "Blocked: this edit targets another worktree's path\n"
    "  {target}\n"
    "from the worktree session rooted at\n"
    "  {worktree}\n"
    "Edits to a sibling worktree are invisible to this one's build. Make the "
    "edit from a session rooted in that worktree instead.\n\n"
    "If you truly mean to write it from here, set "
    "{escape}=1 in the environment to bypass this guard."
)


def _base_repo_of(worktree_root):
    """Return the base-repo root for a worktree checkout, or None.

    The worktree root is `<base>/.claude/worktrees/<tag>`; the base is the
    segment before the `/.claude/worktrees/` marker. Returns None when the
    root is not a worktree checkout (so the guard is a no-op).
    """
    marker_at = worktree_root.find(WORKTREE_MARKER)
    if marker_at == -1:
        return None
    return worktree_root[:marker_at]


def _target_path(payload):
    """Pull the file path a mutating tool would write, or None."""
    tool_input = payload.get("tool_input") or {}
    if not isinstance(tool_input, dict):
        return None
    # Edit/Write/MultiEdit use file_path; NotebookEdit uses notebook_path.
    path = tool_input.get("file_path") or tool_input.get("notebook_path")
    return path if isinstance(path, str) and path else None


def _is_allowed_base_settings(target, base):
    """True for the base `.claude/settings*.json` files firm-perms may write."""
    claude_dir = os.path.join(base, ".claude")
    allowed = (
        os.path.join(claude_dir, "settings.json"),
        os.path.join(claude_dir, "settings.local.json"),
    )
    return target in allowed


def evaluate(payload, project_dir, env):
    """Return (exit_code, message). 2 blocks; 0 allows.

    `project_dir` is the active checkout root (from `$CLAUDE_PROJECT_DIR`,
    falling back to cwd); `env` is the process environment mapping.
    """
    if not isinstance(payload, dict):
        return 0, ""
    if payload.get("tool_name") not in EDIT_TOOLS:
        return 0, ""
    if env.get(ESCAPE_ENV):
        return 0, ""
    if not project_dir:
        return 0, ""

    worktree_root = os.path.normpath(project_dir)
    base = _base_repo_of(worktree_root + os.sep)
    if base is None:
        # Not a worktree checkout — nothing to guard.
        return 0, ""

    target = _target_path(payload)
    if target is None:
        return 0, ""
    # A relative path resolves against the worktree cwd, so it can't be a
    # stray base edit — allow it untouched.
    if not os.path.isabs(target):
        return 0, ""

    target = os.path.normpath(target)

    # Editing inside this worktree is exactly right.
    if target == worktree_root or target.startswith(worktree_root + os.sep):
        return 0, ""

    # Outside this worktree but inside the base tree → a base-repo (or
    # sibling-worktree) edit, which the worktree build won't see.
    if target == base or target.startswith(base + os.sep):
        if _is_allowed_base_settings(target, base):
            return 0, ""
        # A target under base's worktrees dir (but not this worktree, excluded
        # above) is a *sibling* worktree — there is no worktree-local copy of
        # it to point at, so use the suggestion-free message.
        if target.startswith(base + WORKTREE_MARKER):
            return 2, SIBLING_DENY_MESSAGE.format(
                target=target,
                worktree=worktree_root,
                escape=ESCAPE_ENV,
            )
        suggested = os.path.join(worktree_root, os.path.relpath(target, base))
        return 2, DENY_MESSAGE.format(
            target=target,
            worktree=worktree_root,
            suggested=suggested,
            escape=ESCAPE_ENV,
        )

    # Somewhere else entirely (another repo, /tmp, …) — not our concern.
    return 0, ""


def _self_test():
    """Built-in cases, run with `--self-test` so it needs no piped stdin."""
    base = "/Users/x/repos/dropset"
    wt = base + "/.claude/worktrees/eng-690"

    def payload(tool, path, key="file_path"):
        return {"tool_name": tool, "tool_input": {key: path}}

    # (payload, project_dir, env, should_block)
    cases = [
        # Base-path edit from a worktree → blocked.
        (payload("Edit", base + "/program/src/lib.rs"), wt, {}, True),
        (payload("Write", base + "/README.md"), wt, {}, True),
        (payload("MultiEdit", base + "/a.rs"), wt, {}, True),
        (payload("NotebookEdit", base + "/n.ipynb", "notebook_path"), wt, {}, True),
        # A sibling worktree is also invisible to this build → blocked.
        (payload("Edit", base + "/.claude/worktrees/eng-1/x.rs"), wt, {}, True),
        # Editing inside this worktree → allowed.
        (payload("Edit", wt + "/program/src/lib.rs"), wt, {}, False),
        # Base settings files firm-perms writes → allowed.
        (payload("Write", base + "/.claude/settings.local.json"), wt, {}, False),
        (payload("Write", base + "/.claude/settings.json"), wt, {}, False),
        # Relative path (resolves against worktree cwd) → allowed.
        (payload("Edit", "program/src/lib.rs"), wt, {}, False),
        # Escape env set → allowed even for a base path.
        (payload("Edit", base + "/a.rs"), wt, {ESCAPE_ENV: "1"}, False),
        # Not a worktree checkout (base session) → guard is a no-op.
        (payload("Edit", base + "/a.rs"), base, {}, False),
        # A Read of a base path is never blocked.
        (payload("Read", base + "/a.rs"), wt, {}, False),
        # An unrelated absolute path → allowed.
        (payload("Edit", "/tmp/scratch.txt"), wt, {}, False),
    ]
    failures = []
    for pl, pd, env, should_block in cases:
        code, _ = evaluate(pl, pd, env)
        blocked = code == 2
        if blocked != should_block:
            failures.append(
                "  %-50s expected block=%s got block=%s"
                % (_target_path(pl), should_block, blocked)
            )

    # Message-body checks. A base-repo target's message names the
    # worktree-local path to use; a sibling-worktree target's message does not
    # carry a (nonsensical) nested suggestion.
    _, base_msg = evaluate(payload("Edit", base + "/program/src/lib.rs"), wt, {})
    if wt + "/program/src/lib.rs" not in base_msg:
        failures.append("  base-repo deny message lacks the worktree suggestion")
    _, sib_msg = evaluate(
        payload("Edit", base + "/.claude/worktrees/eng-1/x.rs"), wt, {}
    )
    if (
        "another worktree" not in sib_msg
        or "eng-690/.claude/worktrees/eng-1" in sib_msg
    ):
        failures.append("  sibling deny message is wrong or has a bogus suggestion")

    if failures:
        sys.stderr.write("self-test FAILED:\n" + "\n".join(failures) + "\n")
        return 1
    sys.stdout.write("self-test OK (%d cases)\n" % len(cases))
    return 0


def main(argv):
    if "--self-test" in argv:
        return _self_test()
    try:
        payload = json.loads(sys.stdin.read())
    except Exception:
        # Fail open: a read or parse problem must never wedge the session.
        return 0
    project_dir = os.environ.get("CLAUDE_PROJECT_DIR") or os.getcwd()
    code, message = evaluate(payload, project_dir, os.environ)
    if message:
        sys.stderr.write(message + "\n")
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
