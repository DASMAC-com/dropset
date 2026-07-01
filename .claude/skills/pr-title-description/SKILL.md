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

All GitHub reads and writes go through the **GitHub
MCP**. This repo is `DASMAC-com/dropset`, so every call
takes `owner: "DASMAC-com"`, `repo: "dropset"`.

## Steps

1. Identify the current branch
   (`git branch --show-current`) and its PR, if one
   exists, with `mcp__github__list_pull_requests` —
   the `head` filter is `owner:branch`:

   ```txt
   mcp__github__list_pull_requests(
     owner: "DASMAC-com",
     repo: "dropset",
     head: "DASMAC-com:<branch>",
     state: "open",
   )
   ```

1. Get the full diff against `main`:
   `git diff main..HEAD` and
   `git log main..HEAD --oneline`.

1. Fetch the 3 most recent merged PRs to match their
   style — with a **field-selected `gh pr list`**, the one
   place this skill uses `gh` over the MCP (a documented
   exception, per `CLAUDE.md` → "GitHub via MCP"):

   ```sh
   gh pr list --json number,title,body --state merged --limit 3
   ```

   This is strictly better than the MCP `list_pull_requests`
   here on both counts that bit before: `gh` has a `merged`
   state filter the MCP lacks (so no listing *every* closed
   PR and filtering on `merged_at` by hand), and `--json`
   returns the `body` in the **same** call (so no per-PR
   `pull_request_read` follow-up). The MCP path returned
   *every* closed PR with full bodies — ~104k tokens
   observed, replayed every later turn (per `CLAUDE.md` →
   "Context economy"); field-selecting three merged PRs'
   `number` / `title` / `body` is all the style lookup needs.
   `--json` is a command **flag**, not a shell pipe, so it
   stays shell-rule-clean and reduces to a
   `Bash(gh pr list:*)` allow-rule.

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

   **Never carry a `Claude:` prefix into the title.** The
   `Claude:` meta-work prefix (per `CLAUDE.md` → "Claude:
   meta-work prefix") is a **Linear issue-title** signal
   only; the PR title keeps the plain
   `type(ENG-###): Subject` form even when the linked issue
   title starts with `Claude:`. Drop the token — don't
   copy it from the issue title into the PR title.

1. Write a concise PR description that mirrors
   the format and tone of those recent PRs.
   Typically this means a `# Changes` section
   with a numbered list. Add a `# Background`
   section only if the changes need non-obvious
   context.

   **Keep Linear tags out of the body** (per
   `CLAUDE.md` → "Keep Linear tags out of PR bodies
   and comments"). Don't write `ENG-###` anywhere in
   the description — Linear auto-links any tag it finds
   in a PR body and can wrongly pull a merely-mentioned
   issue into this PR's lifecycle. Refer to other work
   by **title** or a **plain GitHub link**, never its
   Linear tag. The `ENG-###` scope in the **title**
   (step 4) is the one exception — that's required by
   `semantic-pr` and stays.

1. If a PR already exists for the branch, update it with
   `mcp__github__update_pull_request`. The title and body
   are **structured tool arguments**, so the whole
   description — backticks, code fences, braces and all —
   passes straight through; there is no shell quoting to
   trip and no `/tmp` body-file workaround:

   ```txt
   mcp__github__update_pull_request(
     owner: "DASMAC-com",
     repo: "dropset",
     pullNumber: <number>,
     title: "<conventional title>",
     body: "<full markdown description>",
   )
   ```

   One caveat, learned the hard way: the MCP write path
   **strips raw angle-bracket sequences** from the body —
   a literal `<!-- … -->` HTML comment or an unknown
   `<tag>` (e.g. a `<path>` placeholder), **even inside
   backticks**, vanishes from the stored body. So don't
   put literal `<…>` in the description: write placeholders
   without angle brackets (`PATH`, `N`) and describe HTML
   comments in prose rather than pasting a literal
   `<!-- … -->`.

   If no PR exists, report the title and description so
   the user can create one.

1. Show the user the PR URL when done.
