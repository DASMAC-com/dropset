---
name: linear-task
description: File a follow-up to-do into Linear (Engineering team, Dropset project, assigned to Alex) via the Linear MCP. Use for deferring blockers and clean-ups uncovered during a PR to do after it merges.
user-invocable: true
---

# `linear-task`

File a deferred to-do into Linear via the
`claude.ai Linear` MCP. The common case: while
working a PR you uncover a blocker, follow-up, or
clean-up that shouldn't hold up the current change —
capture it as its own issue to pick up later.

Every issue is filed into one fixed destination —
a single team, project, and assignee. Do **not**
hard-code the IDs and do **not** ask the user for
them: resolve them at run time from the environment
with a single bare command, so the call reduces to
a stable `Bash(printenv:*)` allow-rule:

```sh
printenv LINEAR_TEAM_ID LINEAR_PROJECT_ID LINEAR_ASSIGNEE_ID
```

| Field    | Env var              |
| -------- | -------------------- |
| Team     | `LINEAR_TEAM_ID`     |
| Project  | `LINEAR_PROJECT_ID`  |
| Assignee | `LINEAR_ASSIGNEE_ID` |

Pass the **IDs** these resolve to. If any variable
is empty, stop and tell the user to export it in
their shell profile (`~/.zshrc`); don't guess the
value.

Every issue is filed **into the Backlog with no
parent** (`state: "Backlog"`, no `parentId`). There is
no umbrella issue: the staging plan — which issues
group into which PR, and in what order — is owned by
the `stage-backlog` skill, which reads this Backlog
and writes the implementation-sequence document. So
just file the to-do; don't attach it to a parent.

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

   ```txt
   mcp__claude_ai_Linear__save_issue(
     team: "<$LINEAR_TEAM_ID>",
     project: "<$LINEAR_PROJECT_ID>",
     assignee: "<$LINEAR_ASSIGNEE_ID>",
     state: "Backlog",
     title: "<title>",
     description: "<markdown body>",
     priority: 3,  // 2 if the user calls it urgent
     links: [{ url: "<pr-url>", title: "<pr-title>" }]  // omit if no PR
   )
   ```

1. Print the new issue's identifier (e.g. ENG-123)
   and URL so the user can jump to it.
