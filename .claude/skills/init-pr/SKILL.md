---
name: init-pr
description: Bootstrap a worktree — pull main, set up the branch, push a draft PR, and warm CI caches.
disable-model-invocation: true
user-invocable: true
---

# `init-pr`

Bootstrap the current worktree: pull main in the
base repo, set up the branch, push a draft PR so
CI caches start warming while work continues.

This is the first skill an agent should run after
`claude --worktree <tag>` starts.

## Input

Accepts an optional Linear tag like `eng-123`.
If not provided, infer it from the worktree
directory name (the last component of the current
working directory). If the inferred name doesn't
match `eng-###` (case-insensitive), stop and ask.

## Steps

1. Validate that the resolved tag matches the
   pattern `eng-###` (case-insensitive). If not,
   stop and ask the user for it.

1. Pull main in the **base repository** (not this
   worktree). Detect the main worktree path:

   ```sh
   main_wt=$(git worktree list --porcelain \
     | awk '/^worktree /{p=$2} /^branch refs\/heads\/main$/{print p}')
   git -C "$main_wt" pull --ff-only
   ```

   If `main_wt` is empty (main isn't checked out
   anywhere), skip the pull and warn the user.

1. Ensure the current branch is named after the
   Linear tag. Check the current branch:

   ```sh
   git branch --show-current
   ```

   If the name doesn't already match the tag,
   rename it:

   ```sh
   git branch -m <eng-###>
   ```

1. Rebase onto the freshly-pulled main so the
   worktree starts from the latest code:

   ```sh
   git rebase main
   ```

   If the rebase produces conflicts, abort it
   (`git rebase --abort`) and tell the user.
   Do not attempt to resolve conflicts
   automatically in this skill.

1. Create an empty, **signed** commit so there is
   something to push:

   ```sh
   git commit --allow-empty -S -m "<ENG-###>"
   ```

   The `-S` is mandatory: branch protection on
   this repo requires every commit to have a
   verified signature.

1. Push the branch:

   ```sh
   git push -u origin <eng-###>
   ```

1. Create a draft PR with the Linear tag as the
   title and an empty body:

   ```sh
   gh pr create --draft \
     --title "<ENG-###>" \
     --body ""
   ```

1. Print the new PR URL.
