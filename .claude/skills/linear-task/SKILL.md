---
name: linear-task
description: File a follow-up to-do into Linear (Engineering team, Dropset project, assigned to Alex) via the Linear MCP. Use for deferring blockers and clean-ups uncovered during a PR to do after it merges.
disable-model-invocation: true
user-invocable: true
---

# `linear-task`

File a deferred to-do into Linear via the
`claude.ai Linear` MCP. The common case: while
working a PR you uncover a blocker, follow-up, or
clean-up that shouldn't hold up the current change —
capture it as its own issue to pick up later.

Every issue is filed into the same fixed
destination (do **not** ask the user for these):

| Field    | Value        | ID                                     |
| -------- | ------------ | -------------------------------------- |
| Team     | Engineering  | `84659a7c-5ea3-47b1-b2bd-c531e3721d6b` |
| Project  | Dropset      | `d505fe50-cc8b-41ca-be93-6215d9adcea0` |
| Assignee | Alex         | `b3ec6d9f-3c78-48da-8b4e-042176e8c579` |

Use the IDs, not the names — there are also
completed "Dropset beta" and "Dropset alpha"
projects that a name match could hit by mistake.

## Input

Free-text describing the to-do. If invoked with no
argument, infer the task from the current
conversation (the blocker just discussed) and
confirm the drafted title/description with the user
before filing. If there's no obvious context, ask
what to file.

## Steps

1. Draft the issue:

   - **Title** — concise, imperative, no trailing
     period (e.g. "Harden vault swap against
     partial fills").
   - **Description** — Markdown. Capture *why* this
     is deferred and enough context to act on it
     cold: what was uncovered, where in the code,
     and what the fix likely involves. Pass literal
     newlines, not `\n` escapes.
   - If the to-do came out of an open PR or branch,
     add a `links` entry to that PR so the issue
     traces back to where the blocker surfaced. Get
     the PR URL with `gh pr view --json url -q .url`
     if one exists for the current branch.
   - **Priority** — default to 3 (Medium). Bump to
     2 (High) only if the user calls it urgent.

1. Create the issue with `save_issue` (do **not**
   pass `id` — that's for updates only):

   ```
   mcp__claude_ai_Linear__save_issue(
     team: "84659a7c-5ea3-47b1-b2bd-c531e3721d6b",
     project: "d505fe50-cc8b-41ca-be93-6215d9adcea0",
     assignee: "b3ec6d9f-3c78-48da-8b4e-042176e8c579",
     title: "<title>",
     description: "<markdown body>",
     priority: 3,
     links: [{ url: "<pr-url>", title: "<pr-title>" }]  // omit if no PR
   )
   ```

1. Print the new issue's identifier (e.g. ENG-123)
   and URL so the user can jump to it.
