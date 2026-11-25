---
name: chore
description: Bootstrap a chore session from its Linear issue — the `chore <n>` verb's prompt. Reads the issue as the whole spec, restates the deliverable and where it lands, marks the issue In Progress, does the work READ-ONLY toward the repo (the session runs in the base checkout with no worktree, branch or PR), files anything repo-bound as a follow-up task, and ends by moving the issue to In Review, then asking once in the tab whether to mark it Done. Never runs init-pr; Done lands only on the operator's yes.
user-invocable: true
---

# `chore`

The bootstrap for a **chore** — a minor job whose deliverable lands
**outside the repo** (a key file, a local deploy, a one-off probe) or
on the issue itself. The `chore <n>` verb launches the helper tier
with `/chore <n>` as its only prompt; the Linear issue is the whole
brief, and this skill only kicks it off. The verb's shape — substrate,
marker, the model probe — is the `chore <n>` paragraph of
`docs/conventions/local-integrations.md` → "Architect and explore
sessions: temporary worktree, no PR".

## The posture

- **No worktree, no branch, no PR, no `init-pr`.** The session runs in
  the **base checkout**, which other sessions share: never edit, stage,
  commit, check out or stash anything there. Scratch files go under
  `/private/tmp` — never a scratch worktree (`EnterWorktree` included),
  which names no issue the pruner can resolve.
- **Anything repo-bound is a follow-up task**, filed with
  `linear-task`, never a local edit.
- **In Review at handoff, Done only on the operator's word** — asked
  for once, in this tab, at the close.

## Steps

1. **Validate the argument.** It is an issue number (`1843`) or tag
   (`eng-1843`); anything else, stop and ask.

1. **Mark the issue In Progress, then read the spec**, both zero-echo
   — never through `save_issue`, which echoes the whole body:

   ```sh
   python3 .claude/tools/board_batch.py state --id ENG-<n> \
     --state 'In Progress'
   ```

   ```sh
   python3 .claude/tools/linear_patch.py read \
     --out <scratchpad>/spec.md ENG-<n>
   ```

   Read that file once, whole. Then spill the comments once —
   acceptance criteria sometimes live in one — with
   `linear_issue.py comments --out <scratchpad>/comments.md ENG-<n>`,
   never `list_comments`; read the file only when it reports N > 0.
   Treat any `file:line` or "already landed" claim as a snapshot of its
   filing day and verify it before acting; a sibling's state is
   `linear_issue.py state ENG-<a> …`, one line each, never its body.

1. **Restate the job in three lines** before doing it: the
   deliverable, where it lands (outside the repo, or on the issue),
   and what would make it done. If the issue implies a repo edit, say
   so and ask whether to file it as a follow-up rather than starting.

1. **Do the work.** Decisions go through `AskUserQuestion`, the
   recommended option first.

1. **Hand off: summary, metrics, then one Done ask** — `review-pr`'s
   post-merge shape, so the operator answers Done in this tab rather
   than finding the issue on the board.

   1. **Say what was done and where it landed**, in a few lines.

   1. **Run `/session-metrics`**, the shared tail.

   1. **Record the outcome on the issue** with a zero-echo append —
      what was produced, where it is, anything left over, and last the
      cost line from
      `session_metrics.py --cost-line --session-id <uuid>` — then move
      it to In Review. Left over is a filed number, never a restated
      operator action (`docs/conventions/linear-automation.md` → "A
      close never recites a captured operator action"):

      ```sh
      python3 .claude/tools/linear_issue.py append --id ENG-<n> \
        --file <note>
      ```

      ```sh
      python3 .claude/tools/board_batch.py state --id ENG-<n> \
        --state 'In Review'
      ```

   1. **Ask once**, via `AskUserQuestion` — *"Everything on the
      checklist is done: mark Done and close?"* — **"Mark Done
      (Recommended)"** first, **"Leave In Review"** second. Ask only
      when every checklist item is done and nothing is left open; an
      open follow-up or an unticked item skips the ask and leaves In
      Review, naming why. On the yes, the Done write is the session's
      **last** tool call:

      ```sh
      python3 .claude/tools/board_batch.py state --id ENG-<n> \
        --state Done
      ```

   Done stays the operator's word — the ask is the mechanism, never a
   self-marked Done. A no leaves In Review, for `post-merge`'s chore
   branch later. There is no worktree to keep or remove, so `close_session.py`
   (which refuses the base checkout) is not the close.
