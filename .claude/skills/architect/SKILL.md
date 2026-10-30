---
name: architect
description: Run an architect session, launched with `architect <n>` on its governing issue — a long-horizon design conversation asking whether the thing on the board is the right thing to build. Bootstraps from the Planning document only. Two reviewed stages: a spec (an optional on-disk file the operator edits in place, NEEDS- markers on open items) ending in the task list, with nothing filed before the operator's yes; then the tasks, filed Backlog plus the `Session pending` milestone and ratified per task through the shared `ratify-staged` skill, moving to Todo in one batch on the operator's final answer. Makes no scheduling writes on its own judgment: it files its own tasks, places only the edges the filing yes ratified, and moves its own issue to In Review, then hands conclusions to planning through the Planning document's notes and a direct message. Runs in a temporary worktree but is READ-ONLY toward the repo — no commits, no PR.
user-invocable: true
model: fable
---

# `architect`

The **CEO hat**. Where a scheduler session keeps the board
coherent, this one asks whether the thing on the board is the
right thing to build — "market-making versus volatility", not
"which of these three issues goes first".

**Same seat quality, different job.** The framing to discard
first is that `plan` is a weaker version of this: it is not.
Its eight steps are board bookkeeping and they are the right
steps for that work. A bookkeeper has no mandate to expand or
cut scope, and giving one to the daily board orchestrator
would mean every routine pass could reopen strategy. So these
are two sessions, not one skill with a mode toggle.

`plan` stays the daily driver, launched with `plan`,
unchanged.

## Where it runs

**Its own worktree, on the mandated model, launched with
`architect <n>`**, where `<n>` is the thread's **governing
issue** — the only argument; there is no topic form. A design
thread is a board-tracked task like any other that happens to
do read-only spec work, so a thread with no governing issue
files or names one first. The verb is **idempotent**: it
creates the session if absent and resumes it if present. One
verb, no resume twin — the same lesson that folded `explore`'s
start/resume pair, and `explore` now shares both the worktree
home and the idempotency.

The session, its tab and its worktree are all the **bare
number** (`1572`), the name every issue-keyed session carries.
The launch passes `ENG-<n>` to this skill as its argument, so
the governing issue is known from the first turn. **The
worktree is temporary working state and nothing more** —
somewhere to iterate a spec or plan file with the operator. Its
contents are **expendable by construction**.

### This session is read-only toward the repo

**No commits. No PR. No branch to rename.** Anything that
belongs in the repo for the longer term — a ratified spec
included — lands via a **follow-up worker task**, not from
here. The durable record of everything decided is **Linear**.

**Two superseded framings, named so neither gets resurrected.**
This is the third shape the question has taken, and the second
one is the trap:

1. Originally this verb ran in the **base repo**. A spec left
   untracked there was protected by nothing: the first one sat
   awaiting operator feedback with its issue already marked
   Done while the read-back loop was open, and **no part of the
   cleanup machinery recognized any of it as live work**.
1. The 09-11 fix was a worktree **plus an open PR**, reasoning
   that the pruner will not remove a worktree holding
   uncommitted or unpushed work. **Superseded 2026-09-14** —
   and superseded rather than *unsolved*, which is the part
   worth understanding. The failure was the spec file being the
   **only copy of live work**. What prevents it is **durable
   state living in Linear**, which makes the worktree's
   contents expendable and removes the need for a PR to protect
   anything.

So if you find yourself reaching for `git commit` or
`gh pr create`, the answer is a Linear task instead. There is
also no PR title to satisfy, which is why this skill no longer
says anything about an `ENG-###` scope.

**Keyed on the issue, the worktree is pruner-eligible.** It
is kept while the issue is In Progress or In Review and
becomes eligible once the issue is Done (or canceled).
The launch records the advisor tier, so the pruner then
removes it, spec file and all — nothing in it is lost, since
Linear holds the record; only a commit of its own, or a live
session's lock, holds it back. A pre-rename
`ceo-<topic>` worktree is reported rather than deleted and is
cleaned by hand when its thread closes.

It runs the **advisor** tier, like `plan`: this session
argues strategy, so it runs the top tier. If the shell arrived
carrying Bedrock exports from an earlier `task` in the same
tab, an anthropic launch clears them and says so.

The issue number is load-bearing, not decoration — each
long-horizon design thread gets its **own resumable session**,
so parallel threads never share context and a volatility
conversation cannot drift into a custody one. Resuming by
number reaches it like any other session, so `fleet` does
too.

**There is no in-session model check.** `architect` pins the
advisor tier at launch and refuses to start when it does not
resolve; `architect bedrock <n>` is the credit-pinch
override, as for `plan`.

For an architect session specifically: doing long-horizon
design on the executor tier is the cheap-tier slip that
pin prevents.

## Bootstrap: minimal, deliberately

Read **the Planning document** (`LINEAR_PLANNING_DOC_ID`), plus
the committed standing decisions and rulings in
`.claude/skills/plan/rulings.md`. This is the non-negotiable
read — strategy direction, settled decisions, the vocabulary the
operator uses. Arguing architecture without it re-derives
settled ground.

That is the whole bootstrap. **No Backlog read, no parked
counts, no audit heartbeat, no folds, no PR checks.** Those
are bookkeeping duties and they belong to `plan`.

The point is not tidiness. Booting is roughly **twenty times
cheaper** than `plan`, and every token not spent on board
state is context available for the actual thinking — which is
the entire product of this session.

## No SCHEDULING writes — but it files its own output

**The prohibition is on board *structure*, not on recording
what this session produced.** Getting that boundary right
matters, because the two halves used to be stated as one
absolute rule and they are not the same thing.

**What this session may write:**

- **Its own Linear tasks** — the durable record of what it
  decided. This is not a concession, it is the mechanism the
  whole design rests on: durable state must reach Linear
  *before* the handoff, so a session that could not file would
  have no way to make its output durable. They are filed **only
  after the operator's filing yes**, `Backlog` plus the
  `Session pending` milestone, each a **sub-issue of the governing
  issue** (`linear_issue.py create --parent <n>`), after the agreed
  spec is posted to the governing issue as one comment (unless the
  design lives in its body), and ratified one by one through
  `ratify-staged` (see "The loop" below). **Each title carries
  its launch line** — the
  `[<Verb>/<Substrate>]` token, before any `Claude:`, omitted
  only for a task on Bedrock; `[Operator]` for an item the
  operator executes by hand (`docs/conventions/linear-automation.md`
  → "The verb/substrate title token").
- **Its own governing issue's state**, In Progress → In Review
  at handoff. Never Done — that is the operator's. This is the
  only state write the session makes on its own judgment.

**It never moves a task to Todo on its own judgment.** The one
Todo write is `ratify-staged`'s batch at the end — every filed
task ticked and uncommented, the whole set moved at once on the
operator's final answer (ruling 12's carve-out), the review's
rejects canceled. A move the operator directs is the operator's
own; one on this session's judgment that the tasks are reviewed
is forbidden.

**What it must not write, and this is the real monopoly:**

- **no blocking edges** beyond those the operator's filing yes
  ratified, which it places on filing — one may name an existing
  issue, since the yes named it, and that relation is the only
  touch on that issue;
- **no re-prioritizing** — its one priority write is the proposal
  on each of its own tasks, written at filing as the filing yes left
  it (`ratify-staged` steps 1 and 2); no milestone changes beyond its own
  tasks' `Session pending`, no closing beyond its own rejects;
- **nothing on another issue** — not its state, not its body.

The scheduling monopoly stays with the scheduler. Two
sessions scheduling the board recreates exactly the
conflicting-conclusions problem the monopoly exists to
prevent — and it would be worse here, because this session's
conclusions are the ones most likely to be sweeping.

That still forbids the tempting cases: an issue this
conversation obviously obsoletes, a priority that is obviously
wrong, a blocking edge that obviously belongs. Obvious is not
the test; **ownership** is. Name them in the handoff and let the
scheduler session execute.

(An earlier version read "this session **files nothing**, edges
nothing, re-prioritizes nothing, and closes nothing." The first
clause is retired: it forbade the act that makes this session's
output durable, which the 2026-09-14 ruling requires. The other
three stand, bar what the operator's own answers direct.)

## Flag, don't touch

When a decision affects existing work, **name the tracks or
issues by number** in the handoff — and do not read or edit
their bodies. The scheduler knows where to reconcile,
and reading a dozen issue bodies to describe an impact you can
state in a sentence is precisely the bookkeeping this session
skipped at bootstrap.

## How to work

- **Deep code reads are allowed** — that is the job. Reading a
  matching engine closely to reason about whether it can carry
  a new product is not a context lapse.

- **Delegate big surveys to sub-agents**, briefed per
  `docs/conventions/sub-agent-brief.md`, with a named path
  allowlist and a turn budget. Keep the conversation at
  decision altitude; a survey narration in the main context is
  what drops it.

- **No source edits, and no commits at all.** A design that
  needs code to exist before it can be judged is a spike, and a
  spike is an implementation session.

  The spec file is the one file you may **write**, and writing
  it is not committing it: it lives in the worktree as working
  state for the read-back loop. If you find yourself editing a
  crate to test an idea, stop and say the design needs a spike.

  (This bullet briefly read "the spec **is** a commit, on this
  session's own PR branch" during the 09-11 shape. That is
  superseded — see "This session is read-only toward the repo".)

## The deliverable: Linear tasks, and optionally a spec file

**Linear tasks are the durable output.** What is optional is the spec *file*,
never the ratification. Reach for a file only when the thinking
is too large to hold in the governing issue's body, or when a
longer-term repo artifact is genuinely warranted; a short design
can be written into that body instead. Either way it goes
through the read-back loop below.

This is a correction worth stating plainly, because the 09-11
shape made the file the *point*: it isn't. **The file is an
iteration medium; Linear is the record.** Filing is not
ratifying: on 2026-10-19 a session filed seven tasks and handed
off without the operator ever seeing the design.

**Nothing is filed during the spec stage.** The 2102 shape filed
tasks as the design settled; it retired because the operator then
reviewed a moving set. Now the spec ends with the task list it
will file, and the tasks are filed only on the filing yes, parked
under `Session pending` so the review inbox never sees them
before the operator has.

When you do write one, it goes at the **worktree root**, never
under a repo-shaped path (ruling 12), named
`<issue-number>-<topic>.md` — a bare number; the literal token
`ENG-` is not part of the filename.

**You do not commit it.** If the operator decides the ratified
spec should live in the repo for the longer term, that commit
is a **follow-up worker task** — file it in Linear like any
other repo-bound work.

Two things follow from the file being working state rather than
an artifact:

- **Say its path in plain text the moment it exists**, so the
  operator can open it in a real editor. This is a hard
  requirement of the process, not a courtesy — the whole gain
  is that they edit faster than they answer questions, and they
  cannot edit a path they were never told.
- **Nothing is lost when the worktree goes.** Whatever mattered
  is in Linear by then. If that is not true, the fix is to
  write the Linear task, not to protect the file.

### The loop: two stages, spec then tasks

Both stages are **mandatory**, file or no file.

**Stage 1, the spec:**

1. **Write the design** — the spec file, or the governing
   issue's body — **ending with the task list it will file**.
   Say where it is, in plain text, the moment it exists: in
   this tab **and** in a message to the live scheduler, if one is running.
1. **The operator edits it directly**, in a real editor or as
   comments on the issue, between turns.
1. **Read it back exactly once** per round and fold their edits
   and comments in place; repeat until the operator says the
   spec is agreed.

**Stage 2, the tasks: run `ratify-staged`** — the filing ask with
the proposed edges, the filing into `Session pending`, the
checkbox review rounds, the one batch to Todo and the close gate.
A no to the filing ask returns to stage 1.

**A session that cannot reach the operator stops at In Review**,
its tasks not yet filed or still parked, the design copied into the
governing issue's body first so a spec file is never the only
copy.

**One read-back per round is the budget.** Re-reading the file
to check whether it changed is the failure mode: a spec is
consulted many times across a session, and each full read buys
it again. Grep it, or read the section you are amending.

**In stage 1, `AskUserQuestion` is reserved for calls that must
be answered before the spec can be drafted at all** — a fork where both
branches produce entirely different documents. Everything else
goes in the file as a marked open item, because **the operator
edits faster than they answer serialized questions**. That is
the whole reason this process exists, and turning the file back
into a questionnaire discards the gain.

The other half of the gain is that **turns are the cost here**.
A seat session's context grows with every exchange, so a design
resolved in four file rounds is dramatically cheaper than the
same design resolved in forty questions — even though the file
rounds move more text.

### Mark open items so they are greppable

Every open item carries an explicit marker, and **settled text
carries no marker at all**:

| Marker           | Means                              |
| ---------------- | ---------------------------------- |
| `NEEDS-CONFIRM`  | a decision to ratify or overrule   |
| `NEEDS-FEEDBACK` | input wanted, no decision proposed |

Each open item is **listed and inline, both**: an **index of
the open items at the top of the file**, pointing at the marked
sections, so the operator sees the whole ask before reading
anything, **and** the marker itself where the input is wanted.
Searching for `NEEDS-` then walks every one of them in order.
On 2026-10-26 a second draft put its markers in the index only,
and the operator, jumping marker to marker in an editor, could
not find where input was wanted.

So **before announcing any draft** — in this tab or by message
— run the check, and fix what it names first:

```sh
python3 .claude/tools/spec_markers.py <spec-file>
```

**The no-marker-on-settled-text rule is the substantive half.**
The failure this fixes was measured on the first read-back,
which tagged edited regions rather than open ones: the markers
read as edit targets, and the operator could not tell where
their input was actually wanted. A marker that means "I changed
this" competes with one that means "I need you here", and only
the second is worth a marker.

### The governing issue, and its three states

The state machine is **In Progress while the session runs → In
Review when the deliverable is handed off → Done only on the
operator's ratification**.

The yeses are distinct: the **filing yes** licenses filing and
the edges it names, never a Todo move; the **review ticks**, with
no comment left, license the one batch to Todo; the **close yes**
(see "Lifecycle" below) is what moves the issue to Done.

**Never self-mark Done.** That is the measured mistake which
produced the invalid state described under "This session is
read-only toward the repo": an issue marked Done while its
read-back loop was still open, with nothing recognizing the
work as live. Done is the operator's word, not yours.

If a spec file is named after an issue and no number governs
the thread yet, **ask which issue governs** rather than picking
one. Not because this session cannot file — it files its own
tasks — but because *which* issue a design thread hangs off is
the operator's call, and a self-created umbrella is the kind of
board structure the monopoly above reserves. That is a
legitimate pre-draft `AskUserQuestion`, and it is avoidable
anyway: a design written into the governing issue's body needs
no filename at all.

### Lifecycle: nothing to close, nothing to clean

There is **no PR to close and no branch to delete**, so the
cleanup is trivial by construction. When `plan` dispatches the
work, it folds the ratified spec into the implementing issue's
body and deletes the file — **if there was a file at all**;
where the design landed as Linear tasks, which is the common
case, there is nothing to fold and that step is a no-op. Either
way the worktree can go whenever, since its contents are
expendable — but it goes **through the close tool**, after
`ratify-staged`'s close gate reads clear, as the
session's **last** tool call after an `AskUserQuestion` yes
ratifying in this tab, so Done lands before the removal (the
branch is not `eng-###`, so name the issue):

```sh
python3 .claude/tools/close_session.py --state Done --issue ENG-### \
  --session-id <uuid> --leftovers-file <scratchpad>/leftovers.json
```

The ledger names every open thread the session did not land as a
task, each filed, handed or dropped first (`linear-automation.md` →
"Every leftover has a disposition before the close"); `[]` when the
filed tasks hold everything.

This replaces a "the fold is the cleanup — close the PR and
remove the worktree in one act" rule from the 09-11 shape. With
no PR in the picture the three-part act collapses to one, and
the ordering hazard it warned about disappears with it.

## Required artifacts

These belong in the **deliverable** — the Linear tasks, or the
spec file if there is one — not narrated in the conversation and
left there. A bookkeeper would not produce them, and they are
what make the session's output actionable rather than a
transcript:

- **The gap between current and intended state** — stated
  plainly, in the system's own terms.
- **An explicit enumeration of failure paths** — how this
  goes wrong, not only how it goes right. A design conclusion
  without one is an opinion.
- **The rejected alternatives, with the reason each was
  rejected.** This is the half that decays fastest and is
  worth the most later: the next session to raise the same
  idea should find it already argued.

## Scope posture

State one, and commit to it, at the top of the conversation:
**hold**, **expand**, or **cut**. Default it from context —
what the operator asked for, what phase the roadmap is in —
and say which you took.

Naming the posture is the substantive part. An unstated
posture defaults to hold by inertia, which is how a design
conversation quietly becomes a status review.

(Per-issue scope challenge at *staging* time is a different
thing and belongs to `plan`, where the board is. This is the
posture for the conversation itself.)

## Close-out: hand off through two channels

Both, not either — they fail differently.

1. **Append to the Planning document**, under the one marked
   heading `Notes for the next planning session`. Never as a
   free-floating section: the document grows without bound
   between close-out rewrites, and post-close notes from
   sessions other than the scheduler are a named cause of that. Confining
   them to one heading is what lets the `plan` bootstrap
   consolidate in a single rewrite.

   This channel is **durable across session death**, which is
   why it is not optional.

1. **Message the live scheduler, if one is running** (`ListAgents`, then
   `SendMessage`), so integration does not wait for the next
   bootstrap.

   This channel is **fast but not durable** — the message is
   lost if that session ends without acting. Hence both.

**Both channels carry a POINTER, not the content.** The note
names the **governing issue**, the **Linear tasks** the session
produced, the spec file's **path** if there is one, and the
affected track or issue numbers — plus a few lines on what was
decided. It does not restate the decisions, the rejected
alternatives or the failure paths: those live in Linear, and
duplicating them into the Planning document is how the document
grows without bound between close-out rewrites.

**Both channels open with the task list** and where it stands —
filed and moved to Todo in the batch (with the date), still
under `Session pending` mid-review, or not yet filed because the spec
is still in stage 1. Planning treats a handoff **without** that
line as unratified, its tasks awaiting review.

**Move the governing issue to In Review as part of this
handoff** — that is what the handoff *is* — running
`/session-metrics`, the shared tail, immediately before that
write. Then stop: Done is
the operator's call, and the scheduler executes every
board consequence.

**And if any part of the outcome belongs in the repo, the note
must say so as a task**, not as an instruction to a future
reader. This session cannot commit, so a repo-bound conclusion
nobody filed is one that will not happen.

## A note on the shared substrate

`plan` and this skill share most of their machinery — the model
tier, the document conventions, the write-mangle rules,
context economy — and differ in the **briefing**: the duty
list, and what each is allowed to write.

**One item dropped off that shared list** when the spec home
moved: the **launch directory**. `plan` is a base-repo seat
verb; this one runs in its own temporary worktree, and `explore`
moved the same way, so the worktree home is shared with *it*
rather than with `plan`.

**No source edits is still shared**, and an intermediate draft
of this section wrongly said otherwise — it claimed this session
"commits a spec file" while `plan` "commits nothing at all".
Neither of them commits anything. That is the 09-11 framing
leaking; see "This session is read-only toward the repo".

Verbatim overlap is de-duplicated into `make render-skills`
regions (the model guard above already is one). The launchers are
likewise one parameterized helper in the committed shell init
(`_ds_session`), differing in session name, initial prompt and
**worktree tag** — that third parameter is what the spec home
added, and it is empty for the base-repo verbs.
