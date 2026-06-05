---
name: commit-changes
description: Stage, commit, and push changes from this worktree with a clean hand-authored commit message.
disable-model-invocation: true
user-invocable: true
---

# `commit-changes`

Commit and push the changes in this worktree.
Each worktree is an isolated copy of the repo
owned by a single agent, so all uncommitted
changes here belong to this session.

## Steps

1. Inspect the working tree:

   ```sh
   git status
   git diff --stat
   ```

1. Review all changed and untracked files. Stage
   them by explicit path:

   ```sh
   git add <path1> <path2> ...
   ```

   Never use `git add -A`, `git add .`, or
   `git add -u`. Always list paths explicitly
   so nothing unintended slips in (build
   artifacts, generated files, secrets).

1. Draft a concise commit message:

   - Summary line in imperative voice, capital
     first letter, no trailing period.
   - Optional body explaining the *why* (not the
     *what*), wrapped at 72 chars.
   - **Do not** include a `Co-Authored-By:`
     trailer, a "Generated with …" footer, or
     any other attribution. The commit must
     look like a regular hand-authored commit.

1. Commit, **signed**:

   ```sh
   git commit -S -m "<message>"
   ```

   The `-S` is mandatory — branch protection on
   this repo requires every commit to have a
   verified signature.

1. Push to the branch's upstream:

   ```sh
   git push 2>/dev/null || \
     git push -u origin "$(git branch --show-current)"
   ```

1. Print the commit hash, short summary, and push
   result.
