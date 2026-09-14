---
name: architect
description: Run an architect session — the long-horizon design conversation, in the same seat quality as a planning session but doing a different job. Bootstraps minimally (the Planning document and the track umbrellas, nothing else), holds the conversation at decision altitude with deep code reads allowed and big surveys delegated, and writes NOTHING to the board: it hands its conclusions to the planning session through the Planning document's notes section and a direct message, naming the tracks its decisions likely affect without touching them. Its deliverable is an on-disk spec FILE the operator edits in place — not a conversation — living on a PR branch in the session's own worktree, read back once per round with greppable NEEDS- markers on the open items. Runs in that worktree on the mandated model, launched with `architect <topic>`.
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
for you; the branch arrives named `worktree-ceo-<topic>`, since
there is no CLI flag to drop the prefix. Rename it to
`ceo-<topic>` before the first commit, exactly as `init-pr`
does for an implementation branch:

```sh
git branch -m worktree-ceo-<topic> ceo-<topic>
```

**This reverses the base-repo home this skill shipped with**
(operator ruling, 2026-09-11, after one day live), and the
reversal is worth understanding rather than just obeying,
because the old rule sounded right: a session that writes no
code has no obvious need for a branch.

What that missed is that the deliverable is a **file**, and an
untracked file in the base checkout is protected by nothing.
The first spec produced this way sat in the base repo awaiting
further operator feedback, its issue already marked Done while
the read-back loop was still open, and **no part of the cleanup
machinery recognized any of it as live work** — the operator
caught the invalid state on their way out, hours before a
`housekeeping` pass. A worktree plus an open PR makes the
protection **structural**: `housekeeping` refuses to prune a
worktree holding uncommitted or unpushed work and applies
open-PR protection, so the spec survives cleanup by
construction rather than by someone remembering it exists.

Two consequences that are easy to get wrong:

- **The worktree name is deliberately not an `eng-###`.** An
  architect topic has no issue of its own, and the `ceo-`
  prefix is what keeps the fleet listing readable by role.
- **The spec PR's title still needs an `ENG-###` scope**,
  because the semantic-PR check requires one and a
  guaranteed-failure run is durable noise in the checks
  rollup. Worktree name and PR scope are independent, so
  there is no conflict — see "The spec file is the
  deliverable" for where that number comes from.

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

## Zero board writes, ever

**This session files nothing, edges nothing, re-prioritizes
nothing, and closes nothing.**

The board monopoly stays with the planning session. Two
sessions writing the board recreates exactly the
conflicting-conclusions problem the monopoly exists to
prevent — and it would be worse here, because this session's
conclusions are the ones most likely to be sweeping.

That includes the tempting cases: an issue this conversation
obviously obsoletes, a priority that is obviously wrong, a
blocking edge that obviously belongs. Obvious is not the
test; **ownership** is. Name them in the handoff and let the
planning session execute.

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

- **No source edits — but the spec file is not a source
  edit.** This bullet used to read "specs land through the
  handoff, not through commits", and the second half is now
  exactly wrong: the spec **is** a commit, on this session's own
  PR branch, and that is the whole point of the worktree. What
  survives is the part that was always the real rule — **no
  product code**. A design that needs code to exist before it
  can be judged is a spike, and a spike is an implementation
  session.

  So the branch carries the spec file and nothing else. If you
  find yourself editing a crate to test an idea, stop and say
  the design needs a spike.

## The spec file is the deliverable

**The output of this session is a markdown file the operator
edits in place — not a conversation.** That is the process
ruling this skill is built around (operator, 2026-09-11), and
every rule below follows from it.

It lives on this session's branch at:

```txt
docs/specs/<ENG-number>-<topic>.md
```

The number prefix is dropped only if no issue governs the
thread yet. The path is **tracked and committed** — `docs/specs/`
is not ignored, and the first spec written this way was
untracked by omission rather than by rule.

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

### The governing issue, and its state

The spec PR's title needs an `ENG-###` scope, and this session
**writes nothing to the board** — so it cannot file its own
issue. If no number governs the thread yet, **ask for one**;
this is precisely the pre-draft blocker `AskUserQuestion` is
reserved for, since the file cannot be named without it.

**That issue stays In Progress for as long as the read-back
loop is open**, and reaches Done only at ratification plus
fold. Marking it Done at the handoff message is the measured
mistake that produced the invalid state described under "Where
it runs" — the handoff is the start of the operator's turn, not
the end of the work.

### Lifecycle: the fold is the cleanup

**The spec PR closes; it does not merge.** The spec is
scaffolding, not committed history — the ratified content lives
in the Linear issue body once the planning session folds it in
at dispatch, and that fold is also the cleanup: **close the PR
and remove the worktree in the same act.** Nothing accumulates,
and nothing depends on remembering that a loose file exists.

`plan` owns the fold, so this session's job is only to leave the
spec ratified and the PR open. Do not close it yourself on the
way out.

## Required artifacts

These are the **spec file's** required contents, not a summary
to narrate in the conversation. A bookkeeper would not produce
them, and they are what make the session's output actionable
rather than a transcript:

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

**Both channels now carry a POINTER, not the content.** The
spec file is the artifact, so the note names **its path, its
PR, and the governing issue**, plus the affected track or issue
numbers and a few lines on what was decided. It does not restate
the decisions, the rejected alternatives or the failure paths —
those are in the file, and duplicating them into the Planning
document is how the document grows without bound between
close-out rewrites.

State explicitly whether the spec is **ratified** or **still in
its read-back loop**, because that is what tells the planning
session whether it may fold yet. The planning session executes
every board consequence, and owns the fold-and-cleanup.

## A note on the shared substrate

`plan` and this skill share most of their machinery — the model
guard, the document conventions, the write-mangle rules,
context economy — and differ in the **briefing**: the duty
list, and what each is allowed to write.

**Two items dropped off that shared list** when the spec home
moved, and they are worth naming so the overlap is not
overstated: the **launch directory** (`plan` is a base-repo seat
verb; this one runs in its own worktree) and **no source
edits** (this session commits a spec file, `plan` commits
nothing at all). `explore` moved the same way this skill did, so
the worktree home is shared with *it* rather than with `plan`.

That overlap is real and worth de-duplicating **later**, when
the template extraction lands. It is deliberately not a reason
to delay this skill: implement the architect first, then let
the generator fold the shared blocks in. The launchers are
likewise one parameterized helper in the committed shell init
(`_ds_session`), differing in session name, initial prompt and
**worktree tag** — that third parameter is what the spec home
added, and it is empty for the base-repo verbs.
