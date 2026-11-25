# Fleet wind-down

When the operator says the fleet is closing out, the scheduler winds it down:
sessions In Review try to merge, everything else pauses, every worktree session
parks (a chore and the curator stand down in their own way, below), and the next
morning `fleet go` resumes the fleet from the closed tabs. No client stays
running overnight. Sessions already resume from their transcripts, so a closed
tab costs nothing once each session has written its resume brief and pushed its
checkpoint; this document is the protocol, and `fleet down` is the verb.

## The listing: `fleet down`

`fleet down` (`.claude/tools/fleet_winddown.py`, read-only) lists
every in-flight issue (state type `started`, the set `fleet go`
reopens) and splits it with **no time argument**:

- **finish**: the issue is In Review, so `review-pr` has already
  handed off at the queue;
- **pause**: everything else, In Progress and chores included,
  however close the session thinks it is.

Each row says whether the session's **tab is open** (its iTerm
session name is the tag) and, from one GraphQL call over every
branch, its open PR's `mergeable`, `mergeStateStatus` and merge-queue
entry. A finisher with an open tab whose PR already needs a rebase
(`CONFLICTING` or `DIRTY`) is flagged **doomed**. The tool prints both
message texts after the listing.

An open tab is not a live client: a parked tab stays open until the
operator closes it, and the listing cannot tell whether its client
exited. **The roll-call is the stand-down messages**: the scheduler
counts each `Standing down; ready to resume tomorrow.` against the
sessions it messaged, and re-runs the listing when one arrives, for
the doomed flags. Nothing polls.

## The messages

The scheduler sends one `SendMessage` per live session, the finish
text to a finisher and the pause text to a pauser, verbatim from the
listing. The texts carry the whole rule and the stand-down act, so a
session needs no skill prose to follow them.

## The linear-path rule (finishers)

After the wind-down message, an In Review session proceeds only while
its remaining path to merge is **linear**: CI green or running,
enqueue, queue run, merged, with no local work between. The first
event needing local work or a retry is grounds to stand down at once:

- a rebase (the base moved; `mergeable` reads `CONFLICTING` or
  `DIRTY`);
- CI red on the PR's own checks;
- a dequeue of any kind, or any re-enqueue;
- a lens finding that reopens the review.

The rebase and the second review are tomorrow's, in one pass: a
rebase at wind-down buys a second review, unsupervised after the
operator has left. A finisher mid-queue when the message arrives
waits, since that path is linear, and a dequeue during the wait is a
stand-down. A finisher that merges runs `post-merge` up to the
Done question, leaves the issue In Review and stands down; tomorrow's
`fleet go` reopens it to ask. Normal-hours behavior (`review-pr`'s
queue watch) is unchanged.

## Pausers

A pauser reaches its **first safe point** (a build in flight finishes,
so its lane lock frees), pushes a checkpoint (unsigned is fine:
`review-pr` signs at the gate), and writes its next step as the resume
brief. It starts nothing new.

## The stand-down act

Every worktree worker (`task`, `explore`, `architect`) stands down
the same way:

1. Append a **Resume brief** on the issue, one paragraph, naming the next step,
   and the PR when one is open (`linear_issue.py append`).
1. Confirm the tree is clean and pushed.
1. Print `Standing down; ready to resume tomorrow.` as the last line,
   and send the scheduler that same line.
1. Run `close_session.py --park --session-id <id>` as the last tool
   call.

Park writes no state, keeps the worktree and branch, appends the cost line, then
marks the open PR's notification threads done with the subscription kept
(best-effort, a failure is a note), so a parked PR leaves none showing. It
refuses on a dirty or unpushed tree, a missing resume brief, a brief that does
not name the open PR when there is one, or a worktree lock this session does not
hold. A pauser with no PR parks on the brief alone, which must then be the issue
body's last paragraph (there is no PR number to tie it to). Its last acts mark
the tab green, move it to the leftmost position in its window (best-effort,
silent on failure) and end the client, so the operator sees one green tab at the
left edge per parked session and closes it. No tool closes a tab.

The base-checkout sessions the wind-down reaches cannot park
(`close_session.py` refuses the base checkout):

- **A chore** stands down with the resume brief, the printed line and
  its message to the scheduler, then
  `python3 .claude/tools/iterm_api.py --signal-parked`, the tab left
  for the operator to close. The same call is the handed-off chore's
  exit tail.
- **The curator** stands down through its own stop: the scheduler's
  close-out sends it "closing for the day", and its stop tail ends in
  `CLEAR TO CLOSE`. It carries no resume brief; tomorrow's curator is
  a fresh session.

## The morning resume

`fleet go` resumes every started-type issue without an open tab. That
includes an In Review issue whose PR has merged: that session may
still carry follow-ups and must ask for its state transition in its
own tab, so it is reopened, never skipped. The roll-call marks it
`PR merged, tail outstanding` so the operator knows what it will
ask. The operator's Done or Cancel before `fleet go` is the control
for a session they meant to drop.

## The day report

When the last worker has parked, the scheduler (the one writer)
prints this block as part of its existing close-out, beside the
metrics cost line and the dismissed-notifications summary, and
appends it to the Planning document under `Session-metrics state`:

- **PRs merged today**: the count, then each as PR number and issue,
  from the merge tail's existing read;
- **Spend since the previous close-out**: Bedrock metered plus
  subscription usage at Bedrock rates, from the local-cost report's
  today row plus yesterday's slice after the previous close.

Two numbers and a list, not a narrative. Then the close-out proper.

## Failure paths

- A park refused on an unpushed commit or a brief missing the PR
  number is fixed and retried. A push that cannot happen leaves the
  one residual manual tab.
- A worker whose tab is open but whose client has exited is messaged
  anyway. No stand-down reply by the next listing re-run counts as
  parked, and the day report says `N listed as live, unreachable`.
- A pause message to a session mid-build leaves its lane lock held
  until the build ends, which is why the message asks for the first
  safe point.

## Rejected

- A deadline argument (`--minutes 20`): Linear and the linear-path
  rule decide, not a clock.
- Letting a finisher rebase once at wind-down: the rebase buys a
  second review, unsupervised.
- Keeping clients alive with only an attention mark: a park that pins
  a fleet slot is worse than a client that exits. The mark is added
  after the exit, not offered instead of it.
- Skipping a merged In Review issue on the morning resume.
