---
name: init-pr
description: Create a placeholder PR from a fresh branch to warm CI caches.
disable-model-invocation: true
user-invocable: true
---

# `init-pr`

Create a placeholder PR on a new branch so CI
caches start warming while work continues.

## Input

Requires a Linear tag like `eng-123` as the
argument. If not provided, stop and ask the
user for it.

## Steps

1. Validate the input matches the pattern
   `eng-###` (case-insensitive). If not, stop
   and ask the user for a valid Linear tag.

1. If the current branch is not `main`, check
   out `main` and pull the latest:

   ```sh
   git checkout main
   git pull
   ```

1. Create and check out a new branch using the
   Linear tag as the branch name:

   ```sh
   git checkout -b <eng-###>
   ```

1. Create an empty commit so there is something
   to push:

   ```sh
   git commit --allow-empty -m "<ENG-###>"
   ```

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
