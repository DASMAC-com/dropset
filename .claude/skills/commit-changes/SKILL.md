---
name: commit-changes
description: Stage, commit, and push changes from this worktree with a clean hand-authored commit message.
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

   **Stage new files before treating a lint run as
   meaningful for them.** `pre-commit --all-files`
   is `git ls-files`, so an untracked file is
   simply *not checked* — one run's two freshly
   written test files passed a green `make lint`
   and then failed cspell on the next run, once
   committing had made them visible. A green lint
   proves nothing about a file git has never seen.

   **Don't re-derive the diff you already have.**
   If `review_diff.py` has already written slices
   for this range, self-review reads those files —
   re-running a bare `git diff` buys the same bytes
   twice, and did so in two measured sessions
   (≈3.4k and ≈3.1k, the second-largest single
   result of each). This is the "never re-fetch
   what's already in context" rule applied to a
   payload the session itself produced.

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

   **A `failed to fill whole buffer` error means the
   1Password SSH agent is locked** — it is not a git
   or a key problem, and the
   message says nothing to suggest otherwise. It
   appears mid-session, after earlier commits in
   the same run signed fine, because the agent
   locks on its own timer. Nothing an agent can do
   fixes it: ask the user to unlock 1Password, then
   retry the same commit. One run stalled on two
   consecutive attempts before this was diagnosed.

1. Push to the branch's upstream:

   ```sh
   git push
   ```

   If that fails because the branch has no
   upstream yet, get the branch name on its own
   and pass it to `git push -u` literally (no
   command substitution, no redirect, no `||`
   compound — each call reduces to a stable
   allow-rule):

   ```sh
   git branch --show-current
   ```

   ```sh
   git push -u origin <branch>
   ```

1. Print the commit hash, short summary, and push
   result.
