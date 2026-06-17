---
name: pr-title-description
description: Write or update a PR title and description for the current branch, matching the style of recent PRs.
user-invocable: true
---

<!-- cspell:word oneline -->

# `pr-title-description`

Write (or update) the title and description
for the pull request on the current branch.

This skill only writes metadata. It does **not**
mark the PR as ready — that is handled by
`/review-pr` after lint and adversarial review
pass.

## Steps

1. Identify the current branch and its PR
   (if one exists) using
   `gh pr list --head <branch>`.

1. Get the full diff against `main`:
   `git diff main..HEAD` and
   `git log main..HEAD --oneline`.

1. Fetch the body of the 3 most recent merged
   PRs to match their style:

   ```sh
   gh pr list --state merged --limit 3 \
     --json number,title,body
   ```

1. Write the PR title using the **Semantic PR /
   Conventional Commits** format:

   ```txt
   <type>(<scope>): <short summary>
   ```

   - **type**: one of `feat`, `fix`, `refactor`,
     `test`, `docs`, `ci`, `chore`, `perf`,
     `build`, or `style`.
   - **scope**: the ticket ID extracted from the
     branch name (e.g. `ENG-254`). If the branch
     has no ticket ID, use the most relevant
     module or area instead.
   - **short summary**: imperative voice (as if
     telling the repo what to do), capitalize the
     first word, no trailing period.

   Examples:

   - `feat(ENG-123): Add frame offset scaffolding and asm config sourcing`
   - `fix(ENG-456): Correct off-by-one in order matching`
   - `docs(ENG-789): Add algorithm index page`

1. Write a concise PR description that mirrors
   the format and tone of those recent PRs.
   Typically this means a `# Changes` section
   with a numbered list. Add a `# Background`
   section only if the changes need non-obvious
   context.

1. If a PR already exists for the branch, update it.
   **Don't pass the body inline** — a description full
   of markdown (backticks, code fences, braces) trips
   the shell "expansion obfuscation" guard and forces a
   manual approval every run, even though `gh pr edit`
   is allow-listed (see `CLAUDE.md` → "Shell commands":
   pass large/special-character arguments through a
   file). Instead, **Write** the description to a
   throwaway file under `/tmp` (e.g.
   `/tmp/pr-body-<branch>.md`) and hand `gh` its path
   with `--body-file`, so only a stable, glob-able path
   rides the command line:

   ```sh
   gh pr edit <number> --title "..." --body-file /tmp/pr-body-<branch>.md
   ```

   The title stays inline — it's short, conventional,
   and carries no markdown. If no PR exists, report the
   title and description so the user can create one.

1. Show the user the PR URL when done.
