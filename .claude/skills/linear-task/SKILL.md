---
name: linear-task
description: File a follow-up to-do into Linear (Engineering team, Dropset project, assigned to the configured assignee) via the Linear MCP. Use for deferring blockers and clean-ups uncovered during a PR to do after it merges.
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
with a bare `printenv` per variable — each call
reduces to the same stable `Bash(printenv:*)`
allow-rule:

```sh
printenv LINEAR_TEAM_ID
printenv LINEAR_PROJECT_ID
printenv LINEAR_ASSIGNEE_ID
```

Query each variable on **its own** `printenv` line.
Do **not** combine them into one
`printenv LINEAR_TEAM_ID LINEAR_PROJECT_ID LINEAR_ASSIGNEE_ID`:
macOS / BSD `printenv` honors only its **first**
operand, so the combined form prints just
`LINEAR_TEAM_ID` and you'd wrongly conclude the
other two are unset.

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
no umbrella issue. What gates what is recorded as
native Linear blocking edges, which a **human curates** —
this skill may propose one but never files one unasked
(per `CLAUDE.md` → "Blocking relations"). The
`sync-blockers` skill records file overlap separately, as
a `related` link (this skill calls it after filing — see
the final step). So just file the to-do; don't attach it
to a parent.

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
     partial fills"). If the to-do is **meta-work** —
     its `**Touches**:` sit entirely under `.claude/**`,
     `CLAUDE.md`, or `docs/conventions/**` —
     prepend the **`Claude:`** prefix (e.g. "Claude:
     Harden the audit dedup key"), per `CLAUDE.md` →
     "Claude: meta-work prefix". Anything also touching
     product / on-chain / SDK / frontend code gets no
     prefix.

   - **Description** — Markdown. Capture *why* this
     is deferred and enough context to act on it
     cold: what was uncovered, where in the code,
     and what the fix likely involves. Pass literal
     newlines, not `\n` escapes. Include a
     `**Touches**: <glob>[, <glob>…]` line — the
     machine-readable path globs the fix will edit
     (a directory like `tui/` when it spans a dir, a
     file when it's one file), comma-separated, so
     `sync-blockers` can detect a file collision with
     another issue. See `CLAUDE.md` → "Structured filing
     fields".

     **Two shapes Linear's writer mangles**, so keep them
     out of the body:

     - **No emphasis span may wrap a newline** — a bolded
       run crossing a line break stores garbled.
     - **No machine-parsed field may start with a bare
       hostname-valid `name.ext`** — it gets linkified
       (`http.rs` stores as a link), which corrupts a key a
       later search has to match. Use a dotless domain
       token instead.

   - If the to-do came out of an open PR or branch,
     add a `links` entry to that PR so the issue
     traces back to where the blocker surfaced. Resolve
     the PR's `html_url` through the GitHub MCP — this
     repo is `DASMAC-com/dropset`, and the `head` filter
     is `owner:branch`:

     ```txt
     mcp__github__list_pull_requests(
       owner: "DASMAC-com",
       repo: "dropset",
       head: "DASMAC-com:<branch>",
       state: "open",
     )
     ```

     Take the matching PR's `html_url`; skip the link if
     no PR exists for the branch.

   - **Dependencies** — a blocking edge is
     **human-curated** (per `CLAUDE.md` → "Blocking
     relations"), so there are exactly two ways one
     gets set here. Either **the user stated it** — they
     named the `ENG-###` that must land first, or that
     this one gates — in which case pass it. Or you
     believe one exists from what you saw: then
     **propose** it via `AskUserQuestion`, naming the
     candidate blocker and the concrete evidence, and
     pass it only on an explicit yes. Never infer one
     silently, and when nobody can answer, file **no
     edge** and write the suspicion into the description
     as prose instead — the reasoning is kept either way,
     and a spurious edge drops an issue out of the board's
     available set.

   - **Priority** — default to 3 (Medium). Bump to
     2 (High) only if the user calls it urgent.

1. **Check the Backlog for a duplicate first — cheaply.**
   This step exists because its absence filed a real
   duplicate; it stays because the check is worth one small
   call. Search the open Backlog for the issue's subject
   before creating anything.

   Ask for **titles**, not bodies. A dedup question is a
   yes/no, and the default response shape answers it at
   thousands of tokens: one measured call returned 8 full
   issue objects — each with a long truncated description —
   for ≈3.0k, the 4th-costliest tool of that session, and
   another spent ≈5.6k on a 15-result query. So cap it:

   ```txt
   mcp__claude_ai_Linear__list_issues(
     project: "<$LINEAR_PROJECT_ID>",
     state: "Backlog",
     query: "<the distinctive noun phrase>",
     limit: 5,
   )
   ```

   Treat a **title** match as sufficient to investigate and
   a title mismatch as sufficient to proceed — only
   `get_issue` a candidate that actually looks like the same
   work. If one is a genuine duplicate, stop and amend that
   issue (or hand off to `/merge-tasks`) instead of filing a
   second.

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
     links: [{ url: "<pr-url>", title: "<pr-title>" }],  // omit if no PR
     blockedBy: ["<ENG-###>"],  // ONLY if the user stated it or approved it
     blocks: ["<ENG-###>"]      // ONLY if the user stated it or approved it
   )
   ```

1. **Record the new issue's file collisions.** Right
   after `save_issue` returns the identifier, run the
   incremental sweep to `related`-link its `**Touches**:`
   collisions against the open Backlog — one bare command
   that reduces to the
   `Bash(python3 .claude/tools/sync_blockers.py:*)`
   allow-rule (the overlap scan happens in the tool's own
   process, so nothing enters context):

   ```sh
   python3 .claude/tools/sync_blockers.py --for <ENG-###>
   ```

   Best-effort: it needs `LINEAR_API_KEY` /
   `LINEAR_PROJECT_ID`; if either is unset the tool says
   so — note it and continue, the full sweep will catch
   the collision later.

   This files **no blocking edge** — a collision means the
   two issues touch the same files, which costs at most a
   rebase. Each one prints the paths it collides on:

   ```txt
   related-linked: ENG-806 ~ ENG-798 (overlaps on .claude/tools)
   ```

   **Relay those lines** when reporting the new issue. If
   one of them looks like a real dependency rather than
   incidental co-location, that is the moment to
   `AskUserQuestion` about a blocking edge — never to file
   one.

1. Print the new issue's identifier (e.g. ENG-123)
   and URL so the user can jump to it.
