---
name: post-merge
description: The session close once its PR lands — entered from review-pr's queue watch on merged, or typed for a RAGGED ending (talk or planning after the merge, off the review-pr rails, a PR closed unmerged, a no-PR session). One fixed sequence: what landed, held PR notifications answered, the clear-to-close checklist (tree clean and pushed, PR terminal, the issue honest with its ticks, every leftover filed, handed or dropped, cross-session obligations met), session metrics unprompted and the cost stated; then one "mark it Done?" whose yes runs the close tool, Done before the worktree goes. Never invents work or self-marks Done.
user-invocable: true
---

# `post-merge`

The close reads, in order: **here was the cost, here is what landed,
here are the follow-ups — mark it Done?** Everything before the
question is a checklist this skill works through itself; the
question is the only operator touch.

**Two entries.** `review-pr` hands over on its queue watch's `merged`
verdict, the merged line in hand. Typed on its own, this verb covers
the **ragged endings** no happy path does: a session that kept talking
after its PR landed, one that coordinated with a scheduler post-merge,
one that ended off the `review-pr` rails, and a PR **closed and not
merged**. A PR **still in the merge queue is not here yet** — the
queue watch is `review-pr`'s, and its wait is never handed to the
operator.

**It never invents work.** Every check passes or names something
concrete that already exists; an empty checklist goes straight to the
question. It does not re-run the review: work found unfinished is
`/review-pr`'s, so say so and stop.

**A diversion does not discharge it** — another skill, a peer
session's message, a fresh request or a long wait closes nothing
(`.claude/skills/review-pr/history.md` → "Diverted tail"). Come back
and finish, or say which item is skipped and why. An unanswered
`AskUserQuestion` is a deferral the operator chose; an unasked one is
a dropped step.

## The sequence

1. **Relay the merged line.** Print its
   `Today's merge count is now N.` sentence exactly (or its
   "unavailable" one; never count by hand). Say so only when the
   notification dismissal failed — the scheduler's repo-wide sweep is
   the fallback, so never dismiss by hand or `mark_all_notifications_read`.
   Typed on its own, read the PR's terminal state once:

   ```sh
   gh pr view <number> --repo DASMAC-com/dropset \
     --json state,mergedAt,mergeStateStatus
   ```

   A **closed-not-merged** PR is terminal only with its reason written
   on the issue or the PR; "closed, no explanation" is an item.

1. **Answer the held notifications.** The merged line names any
   thread held `for a reply` (a mention, a review request, an
   assignment, or a conversation whose latest comment is another
   human's; each is in `wait-for-queue-<pr>.json` →
   `notifications.held`). Typed on its own, list them with one dry run:

   ```sh
   python3 .claude/tools/pr_notifications.py dismiss --pr <number> \
     --hold-attention
   ```

   Read that PR's conversation once through the GitHub MCP and give
   each a disposition: an answer posted on the PR, or a leftover
   (step 4) when it asks for work. The close summary lists each with
   its disposition, and the close dismisses them; never leave one
   unread to it.

1. **Git safety.** Nothing uncommitted, nothing unpushed:

   ```sh
   git status --short
   ```

   ```sh
   git log --oneline origin/HEAD..HEAD
   ```

   **Only pushed work survives a removal.** A locked signer is no
   reason to leave work uncommitted — commit and push with
   `--no-gpg-sign` (`commit-changes`). Surface anything modified and
   decide where it goes; never report clear with a dirty tree. Nor is
   anything here **load-bearing for another open PR** — an unmerged
   sibling branch, or a file only this checkout has.

1. **Every leftover has a disposition.** Each warning, nit and
   left-for-you item from the review summary and the PR body is
   **filed** (`linear-task`), **handed** to a live session by message,
   or **dropped** with a reason — by this session, never listed for
   the operator to decide; the PR body is a record, not a disposition
   (`docs/conventions/linear-automation.md` → "Every leftover has a
   disposition before the close"). Message each filing to the curator
   (`curate-<day>`, via `ListAgents`; else the scheduler). Write the
   ledger the close takes, `[]` when empty:
   `<scratchpad>/leftovers.json`, one `{"item", "disposition", "ref"}`
   each. A captured operator action is never restated; an uncaptured
   one is filed first (same doc → "A close never recites a captured
   operator action").

1. **The Linear issue is honest.** It sits **In Review** (the state
   tracks the session, `docs/conventions/linear-automation.md`), never
   Done before the question. Deferred checklist ticks ride one
   `linear_issue.py tick` write, never a `save_issue`; a disposition
   note — what landed, what did not, and why — goes through
   `linear_issue.py append`.

1. **Cross-session obligations.** Every message this session promised
   is sent; every question it asked another session is answered or
   **explicitly abandoned** (`ListAgents` shows what is live).

1. **Session metrics, unprompted.** Invoke `/session-metrics` — the
   brief, then the `Session account` — as the second-last act, so the
   capture covers the whole task. No approval and no question. If it
   already ran this session, say so and move on.

1. **The one question.** Print the close summary — the cost line, what
   landed (PR, merge time, the issue's checklist state), the held
   notifications and the leftovers with their dispositions — then
   dry-run the close, which re-checks
   the ledger, git safety, the PR and the worktree lock without writing:

   ```sh
   python3 .claude/tools/close_session.py --state Done \
     --session-id <uuid> --leftovers-file <scratchpad>/leftovers.json \
     --dry-run
   ```

   A `refused` line is an item: fix it, then re-run. Once it passes,
   ask via `AskUserQuestion` — *"N leftovers disposed, metrics captured —
   move it and mark it Done? Removing the worktree ends shell, file
   writes and memory saves here."* — with **"Mark Done and remove the
   worktree (Recommended)"**, **"Mark Done, keep the worktree"** and
   **"Not yet"**. Only this approval moves the issue to Done, and the
   question is the last word: nothing is recited after it.

## The close

Either yes runs one tool as the session's **last** tool call — the
same command without `--dry-run`, plus `--keep-worktree` for the
second option and `--issue ENG-###` on a branch that is not
`eng-###`. **The state write lands before any removal, never the
reverse**: a worktree removed while its issue reads In Review strands
the session. On either yes the tool then dismisses the PR's
notification threads (held ones included); on the removal path it runs
`make clean`, so the build trees free their disk now, and removes the
tree. Never
remove a worktree any other way. Exit 2 means Done landed and the
removal did not; relay its note.

**"Not yet"** leaves the issue In Review and says what remains in one
line. When a check cannot pass, the verdict is that list instead of
the question — each item handled or handed off before re-answering,
since a close that reports the same list twice has become a status
printer.

## A session with no PR

`architect`, `explore` and `chore` open no PR by design, so step 1's
PR read and step 2 are **skipped, not failed**. Git safety shrinks to:
nothing committed beyond `origin/main`, and any untracked spec already
captured in Linear. The resting state is **In Review** with the
durable state on the issue, and the close is unchanged for `architect`
/ `explore`. A **`chore`** runs in the base checkout, which `plan` and
other chores share: git safety is `git status --short` there (dirt
this session made is an item; other dirt is reported unattributed),
and a yes is a state write alone, never `close_session.py`, which
refuses the base checkout:

```sh
python3 .claude/tools/board_batch.py state --id ENG-### --state Done
```

## Landed content

If you want landed content anyway — a merge resolution, say — slice
it with `python3 .claude/tools/show_at_ref.py <ref> <path>`; **never
`git show <ref>:<path>`**, which prints the whole blob. This restates
a general rule **at the call site on purpose** — a rule stated in one
phase of a long session has repeatedly failed to fire in another — so
a later de-duplication pass must leave it.
