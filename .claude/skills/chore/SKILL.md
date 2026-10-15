---
name: chore
description: Bootstrap a chore session from its Linear issue — the `chore <n>` verb's prompt. Reads the issue as the whole spec, restates the deliverable and where it lands, marks the issue In Progress, does the work READ-ONLY toward the repo (the session runs in the base checkout with no worktree, branch or PR), files anything repo-bound as a follow-up task, and ends by moving the issue to In Review. Never runs init-pr; only the operator marks the issue Done.
user-invocable: true
---

# `chore`

The bootstrap for a **chore** — a minor job whose deliverable lands
**outside the repo** (a key file, a local deploy, a one-off probe) or
on the issue itself. The `chore <n>` verb launches the helper tier
with `/chore <n>` as its only prompt; the Linear issue is the whole
brief, and this skill only kicks it off. The verb's shape — substrate,
marker, the model probe — is in `docs/conventions/local-integrations.md`
→ "Architect and explore sessions: temporary worktree, no PR".

## The posture

- **No worktree, no branch, no PR, no `init-pr`.** The session runs in
  the **base checkout**, which other sessions share: never edit, stage,
  commit, check out or stash anything there. Scratch files go under
  `/private/tmp`.
- **Anything repo-bound is a follow-up task**, filed with
  `linear-task`, never a local edit.
- **In Review at handoff, Done only on the operator's word.**

## Steps

1. **Validate the argument.** It is an issue number (`1843`) or tag
   (`eng-1843`); anything else, stop and ask.

1. **Mark the issue In Progress and keep the response** — its echo is
   the spec, so this is the read, not an extra one:

   ```txt
   save_issue(id: "ENG-<n>", state: "In Progress")   # the Linear MCP
   ```

   Then `list_comments` once: acceptance criteria sometimes live in a
   comment. Treat any `file:line` or "already landed" claim as a
   snapshot of its filing day and verify it before acting.

1. **Restate the job in three lines** before doing it: the
   deliverable, where it lands (outside the repo, or on the issue),
   and what would make it done. If the issue implies a repo edit, say
   so and ask whether to file it as a follow-up rather than starting.

1. **Do the work.** Decisions go through `AskUserQuestion`, the
   recommended option first.

1. **Hand off.** Record the outcome on the issue with a zero-echo
   append — what was produced, where it is, anything left over — then
   move it to In Review:

   ```sh
   python3 .claude/tools/linear_issue.py append --id ENG-<n> \
     --file <note>
   ```

   ```sh
   python3 .claude/tools/board_batch.py state --id ENG-<n> \
     --state 'In Review'
   ```

   There is no worktree to keep or remove, so `close_session.py` (which
   refuses the base checkout) is not the close. Say the session's cost
   at close, as every session does.
