---
name: commit-changes
description: Commit only the files edited in the current session, ignoring changes from any parallel session, with a plain hand-authored-looking commit message.
disable-model-invocation: true
user-invocable: true
---

# `commit-changes`

Commit only the changes that originated in the
current session. A parallel session may have
edits in flight on other files in the same
working tree — those must be left alone.

## Steps

1. Inspect the working tree:

   ```sh
   git status
   git diff --stat
   ```

1. From the current session's transcript, build
   an explicit list of files that were edited in
   this session (via Read / Edit / Write tool
   calls). This is the allowlist.

1. Cross-reference the allowlist against
   `git status`. Drop any file that has no
   uncommitted change.

1. Show the user the resulting list and have
   them confirm before staging. Any file that
   wasn't touched in this session must be
   excluded.

1. Stage only the confirmed files by explicit
   path:

   ```sh
   git add <path1> <path2> ...
   ```

   Never use `git add -A`, `git add .`, or
   `git add -u`; those sweep in changes from
   the parallel session.

1. Draft a concise commit message:

   - Summary line in imperative voice, capital
     first letter, no trailing period.
   - Optional body explaining the *why* (not the
     *what*), wrapped at 72 chars.
   - **Do not** include a `Co-Authored-By:`
     trailer, a "Generated with …" footer, or
     any other attribution. The commit must
     look like a regular hand-authored commit.

1. Commit:

   ```sh
   git commit -m "<message>"
   ```

1. Print the resulting commit hash and short
   summary so the user can verify.