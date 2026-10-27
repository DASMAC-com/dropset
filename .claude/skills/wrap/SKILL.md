---
name: wrap
description: The clear-to-close gate for ending a session. Runs a deterministic checklist — no uncommitted or unpushed work, the PR in a terminal state, the Linear issue honest with its deferred ticks written, follow-ups filed or handed off, cross-session obligations discharged, session metrics captured — and ends with an explicit verdict: CLEAR TO CLOSE and the worktree removable, or an enumerated list of what remains. Exists for the RAGGED endings review-pr does not cover: post-merge straggler conversations, sessions that coordinated with planning after their PR landed, sessions that ended off the review-pr rails, and closed-not-merged PRs. Never invents work; an empty checklist answers CLEAR immediately.
user-invocable: true
---

<!-- cspell:word unfiled -->

<!-- cspell:word actioned -->

# `wrap`

**Give this to a session as its final prompt.** It answers one
question — *are we clear to close this session and remove the
worktree?* — and it answers it explicitly rather than leaving
the operator to reconstruct the state.

That question genuinely varies, which is why it needs a verb.
A merged session may still owe a cross-session reply, an
unfiled follow-up, its session metrics, or a Linear
transition. The operator was hand-typing the question at the
end of every session and reading the answer out of the
transcript; this makes it a checklist with a verdict.

**`review-pr` covers the happy path** — it watches its
queued PR to merged, then runs the session-metrics tail and
asks the one close question. So this verb
exists for the **ragged endings**:

- a session that kept talking after its PR landed;
- a session that coordinated with a planning session
  post-merge;
- a session that ended off the `review-pr` rails entirely;
- a PR **closed and not merged**, which no happy path covers.

## What it is not

**It never invents work.** Every check below either passes or
names something concrete that already exists. If the checklist
is empty, the answer is CLEAR TO CLOSE, immediately — a wrap
that manufactures a to-do list has failed at its job, because
the operator will stop trusting the verdict and go back to
reading transcripts.

**It does not re-run the review.** If the checks reveal that
the work is not actually finished, say so and stop; the fix is
`/review-pr`, not this verb.

## Output discipline

**The verdict line comes first**, then one line of evidence per
check. Not the reverse: the operator is asking a yes/no
question, and burying the answer under a report is what this
replaces.

```txt
CLEAR TO CLOSE — worktree eng-942 can be removed.
  git: clean, 9 commits pushed, HEAD == origin/eng-942
  PR:  #367 merged 2026-08-25
  ...
```

or

```txt
NOT CLEAR — 2 items remain.
  1. ENG-942 is In Progress; the session-state convention wants In Review.
  2. A follow-up was surfaced at the CI-skew finding and never filed.
  git: clean, 9 commits pushed
  ...
```

## The checks

Run them in order. Order matters: git safety first, because
everything below it is recoverable and that one is not.

1. **Git safety.** No uncommitted changes and nothing unpushed
   in this worktree.

   ```sh
   git status --short
   ```

   ```sh
   git log --oneline origin/HEAD..HEAD
   ```

   **Only pushed work survives a prune**, and this repo has
   lost work to that twice — once benignly, once for real. So
   this check is not a formality. **A locked signer is no reason
   to leave work uncommitted**: commit and push with
   `--no-gpg-sign` (`commit-changes`) — an unsigned pushed
   commit survives a prune, and the merge gate signs it. An
   **open** PR still carrying unsigned commits
   (`python3 .claude/tools/resign_branch.py count`) is an
   outstanding item: name the count.

   Anything outstanding → **NOT CLEAR**, and say precisely
   what: N modified files, or N commits ahead of the remote.

1. **PR terminal state.** Merged, or closed deliberately with
   the closure recorded somewhere durable.

   ```sh
   gh pr view <number> --repo DASMAC-com/dropset \
     --json state,mergedAt,mergeStateStatus
   ```

   **In the merge queue is NOT clear.** That is `review-pr`'s
   own rule and it holds here: the session stays until the
   queue answers, because a dequeue needs someone to notice.

   A **closed-not-merged** PR is a legitimate terminal state,
   but only when the reason is written down — on the Linear
   issue or in the PR itself. "Closed, no explanation" is an
   item, not a pass.

1. **Linear state honest.** The issue sits where the
   session-state convention puts it (see
   `docs/conventions/linear-automation.md` → "The Linear state
   tracks the SESSION, not the PR"), which for a merged PR
   with no follow-up outstanding means **In Review** — never
   Done, which is the operator's word.

   Three things travel together here, and all three are part
   of this check:

   - the **state** itself;
   - any **deferred checklist ticks** — and they ride the
     state write, never their own `save_issue` (two full-body
     echoes where one write serves);
   - the **disposition** recorded on the issue: what landed,
     what did not, and why.

1. **Follow-ups discharged.** Anything this session surfaced
   but did not do is either **filed** to convention
   (`linear-task`, which messages the curator) or **handed to
   the planning session** with a message. Named, not remembered.

   The failure this catches is specific: a finding raised
   mid-session, acknowledged, and then carried only in the
   transcript — which the prune deletes.

1. **Cross-session obligations.** Every message this session
   promised to send has been sent, and every question it asked
   another session is either answered or **explicitly
   abandoned**. Use `ListAgents` to see what is live.

   Abandoning is a legitimate outcome; leaving it hanging is
   not, because the other session may be waiting on it.

1. **Session metrics captured.** The tail ran — its brief printed
   and its `Session account` written — or it runs **now**,
   immediately before the close question.

   Invoke `/session-metrics`; do not duplicate its logic here.
   If `review-pr` already ran it, say so and move on. The close
   passes `--session-id` so the tool appends the cost line.

1. **Worktree verdict.** Confirm nothing in this worktree is
   **load-bearing for an open PR** — an unmerged sibling
   branch, or a file only this checkout has. The close itself
   removes it (below), merged or closed-not-merged alike.

### A session with no PR

`architect`, `explore` and `chore` sessions open no PR by design, so
check 2 is **skipped, not failed**. Check 1 shrinks to: nothing
committed beyond `origin/main`, and any untracked spec file already
captured in Linear, since the worktree's contents are expendable.
Check 3's resting state is **In Review** with the durable state on
the issue. The close below is unchanged for `architect` / `explore`.
A **`chore`** has no worktree at all — it runs in the base checkout,
which `plan` and other chores share — so check 7 is
skipped, and check 1 becomes `git status --short` in the base
checkout: it should be clean, and any dirt is named (an item if this
session made it, otherwise reported as unattributed). The close is
the same one question below, but a yes is a state write alone, never
`close_session.py` (which refuses the base checkout):

```sh
python3 .claude/tools/board_batch.py state --id ENG-### --state Done
```

## The verdict

**CLEAR TO CLOSE** only when every check passes. Otherwise
enumerate what remains, and — this is the part that makes the
verb worth having — **each item is actioned or handed off
before re-answering.** A wrap that reports the same list twice
has become a status printer.

Once the verdict is CLEAR, close through one `AskUserQuestion`
— the operator's answer in this tab is the ratification.
Offer **"Mark Done and remove the worktree (Recommended)"**,
then **"Mark Done, keep the worktree"**, then **"Not yet"**.
Either yes runs one tool as the session's **last** tool call:

```sh
python3 .claude/tools/close_session.py --state Done --session-id <uuid>
```

with `--keep-worktree` for the second option, and
`--issue ENG-###` on a branch that is not `eng-###`. **The state
write lands before any removal, never the reverse** — a
worktree removed while its issue still reads In Review
strands the session, which a later resume cannot recover.
Never remove the worktree any other way. The tool re-checks
git safety, the PR, and the worktree lock before writing
anything, and removes only after Done or Canceled; on a
refusal, report its `refused` line as a
NOT CLEAR item. Exit 2 means Done landed but the removal did
not; relay the tool's note, which says when the tree needs
removing by hand.
