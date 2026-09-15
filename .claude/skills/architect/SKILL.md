---
name: architect
description: Run an architect session — the long-horizon design conversation, in the same seat quality as a planning session but doing a different job. Bootstraps minimally (the Planning document and the track umbrellas, nothing else), holds the conversation at decision altitude with deep code reads allowed and big surveys delegated, and makes no SCHEDULING writes to the board — it files its own Linear tasks and moves its own governing issue to In Review, but places no edges, re-prioritizes nothing, closes nothing and touches no other issue; it hands its conclusions to the planning session through the Planning document's notes section and a direct message, naming the tracks its decisions likely affect without touching them. Its durable output is Linear tasks; when a longer-term repo artifact is warranted it also iterates an on-disk spec FILE the operator edits in place — not a conversation — read back once per round with greppable NEEDS- markers on the open items. Runs in its own worktree on the mandated model, launched with `architect <topic>`, but is READ-ONLY toward the repo: the worktree is temporary working state, there are no commits and no PR, and any repo-bound artifact lands via a follow-up worker task.
user-invocable: true
model: fable
---

# `architect`

The **CEO hat**. Where a planning session keeps the board
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
`architect <topic>`.** The verb takes a topic and is
**idempotent**: it creates the session if the named one is
absent and resumes it if present. One verb, no resume twin —
the same lesson that folded `explore`'s start/resume pair, and
`explore` now shares both the worktree home and the
idempotency.

The worktree is named **`ceo-<topic>`** and the verb creates it
for you. **It is temporary working state and nothing more** —
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
   that `housekeeping` will not prune a worktree holding
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

**The worktree name is deliberately not an `eng-###`.** An
architect topic has no issue of its own, and the `ceo-` prefix
is what keeps the fleet listing readable by role.

It is a **seat** verb, like `plan`: this session argues
strategy, so it runs the top tier, and a seat launch is what
a Fable pin means. If the shell arrived carrying Bedrock
exports from an earlier `task` in the same tab, `architect`
clears them and says so.

The session is named `ceo-<topic>`, so the fleet listing reads
by role: `eng-*` implementers, `plan-*` planning, `ceo-*`
architecture. The topic is load-bearing, not decoration —
each long-horizon design thread gets its **own resumable
session**, so parallel threads never share context and a
volatility conversation cannot drift into a custody one.

### Check the model before doing anything else

<!-- render:begin fable-model-guard verb=architect -->

Sessions of this kind deliberately run the most capable model —
**fidelity is the point**, and a session that has quietly landed on
the default implementation model will still *work*, which is exactly
why the slip goes unnoticed.

So on invocation, **before the bootstrap read**, check the model this
session is running as. The system prompt states it. If it is **not** a
Fable/Mythos-tier model, say so and offer the fix via
`AskUserQuestion`, recommended option first:

1. *"Run `/model fable` now and continue"* — recommended; it switches
   the running session in place.
1. *"Relaunch via `architect`"* — the deterministic path, at the cost of
   restarting the session.
1. *"Continue on this model anyway"* — proceed, and don't ask again
   this session.

This is the mirror of `init-pr`'s guard, pointing the other way: that
one catches a planning-tier model about to burn a long implementation
run, this one catches an implementation-tier model about to do work
that needs the top tier.

The `model:` frontmatter on this skill is **belt-and-braces, not the
mechanism**. Whether it switches the session going forward or applies
only to this invocation's execution is not specified, so it is not
relied on — `architect` passing `--model claude-fable-5` at launch is the
deterministic path, and the check above is what catches every other
route in.

<!-- render:end fable-model-guard -->

For an architect session specifically: doing long-horizon
design on the implementation tier is the cheap-tier slip this
catches.

## Bootstrap: minimal, deliberately

Read **two things**, in one cheap call each:

1. **The Planning document** (`LINEAR_PLANNING_DOC_ID`). This
   is the non-negotiable read — standing decisions, strategy
   direction, the vocabulary the operator uses. Arguing
   architecture without it re-derives settled ground.
1. **The track umbrellas** — the Todo tier, milestone-carrying
   issues excluded.

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
  have no way to make its output durable.
- **Its own governing issue's state**, In Progress → In Review
  at handoff. Never Done — that is the operator's.

**What it must not write, and this is the real monopoly:**

- **no blocking edges**, ever;
- **no re-prioritizing**, no closing, no milestone changes;
- **nothing on another issue** — not its state, not its body.

The scheduling monopoly stays with the planning session. Two
sessions scheduling the board recreates exactly the
conflicting-conclusions problem the monopoly exists to
prevent — and it would be worse here, because this session's
conclusions are the ones most likely to be sweeping.

That still forbids the tempting cases: an issue this
conversation obviously obsoletes, a priority that is obviously
wrong, a blocking edge that obviously belongs. Obvious is not
the test; **ownership** is. Name them in the handoff and let the
planning session execute.

(An earlier version read "this session **files nothing**, edges
nothing, re-prioritizes nothing, and closes nothing." The first
clause is retired: it forbade the act that makes this session's
output durable, which the 2026-09-14 ruling requires. The other
three stand unchanged.)

## Flag, don't touch

When a decision affects existing work, **name the tracks or
issues by number** in the handoff — and do not read or edit
their bodies. The planning session knows where to reconcile,
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

**Linear tasks are the durable output. A spec file is
optional** — reach for one only when the thinking is too large
to hold in issue bodies, or when a longer-term repo artifact is
genuinely warranted. A design can be ratified purely as Linear
tasks that workers then pick up, and that is a complete,
normal outcome, not a shortcut.

This is a correction worth stating plainly, because the 09-11
shape made the file the *point*: it isn't. **The file is an
iteration medium; Linear is the record.**

When you do write one, it goes in the worktree at:

```txt
docs/specs/<issue-number>-<topic>.md
```

(A bare number, matching the existing
`docs/specs/1313-mainnet-laptop.md` — the literal token `ENG-`
is not part of the filename.)

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

### The loop: write, edit, read back ONCE

1. **Write the file.** Say its path in the conversation, in
   plain text, the moment it exists.
1. **The operator edits it directly**, in a real editor,
   between turns.
1. **Read it back exactly once** and incorporate their edits.
1. **They edit again**, and you ratify.

**One read-back per round is the budget.** Re-reading the file
to check whether it changed is the failure mode: a spec is
consulted many times across a session, and each full read buys
it again. Grep it, or read the section you are amending.

**`AskUserQuestion` is reserved for calls that must be answered
before the spec can be drafted at all** — a fork where both
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

Put an **index of the open items at the top of the file**,
pointing at the marked sections, so the operator sees the whole
ask before reading anything. Searching for `NEEDS-` then walks
every one of them in order.

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
anyway: a design whose output is Linear tasks needs no filename
at all.

### Lifecycle: nothing to close, nothing to clean

There is **no PR to close and no branch to delete**, so the
cleanup is trivial by construction. When `plan` dispatches the
work, it folds the ratified spec into the implementing issue's
body and deletes the file — **if there was a file at all**;
where the design landed as Linear tasks, which is the common
case, there is nothing to fold and that step is a no-op. Either
way the worktree can go whenever, since its contents are
expendable.

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
   non-planning sessions are a named cause of that. Confining
   them to one heading is what lets the `plan` bootstrap
   consolidate in a single rewrite.

   This channel is **durable across session death**, which is
   why it is not optional.

1. **Message any live planning session** (`ListAgents`, then
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

State explicitly whether the design is **ratified** or **still
in its read-back loop**, because that is what tells the planning
session whether it may dispatch yet.

**Move the governing issue to In Review as part of this
handoff** — that is what the handoff *is*. Then stop: Done is
the operator's call, and the planning session executes every
board consequence.

**And if any part of the outcome belongs in the repo, the note
must say so as a task**, not as an instruction to a future
reader. This session cannot commit, so a repo-bound conclusion
nobody filed is one that will not happen.

## A note on the shared substrate

`plan` and this skill share most of their machinery — the model
guard, the document conventions, the write-mangle rules,
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

That overlap is real and worth de-duplicating **later**, when
the template extraction lands. It is deliberately not a reason
to delay this skill: implement the architect first, then let
the generator fold the shared blocks in. The launchers are
likewise one parameterized helper in the committed shell init
(`_ds_session`), differing in session name, initial prompt and
**worktree tag** — that third parameter is what the spec home
added, and it is empty for the base-repo verbs.
