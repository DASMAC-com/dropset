---
name: architect
description: Run an architect session — the long-horizon design conversation, in the same seat quality as a planning session but doing a different job. Bootstraps minimally (the Planning document and the track umbrellas, nothing else), holds the conversation at decision altitude with deep code reads allowed and big surveys delegated, and writes NOTHING to the board: it hands its conclusions to the planning session through the Planning document's notes section and a direct message, naming the tracks its decisions likely affect without touching them. Runs in the base repo on the mandated model, launched with `caps <topic>`, never in a worktree.
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

`plan` stays the daily driver, launched with `paps`,
unchanged.

## Where it runs

**The base repo, on the mandated model, launched with
`caps <topic>`** — never a worktree. The verb is capitals for
CEO, takes a topic, and is **idempotent**: it creates the
session if the named one is absent and resumes it if present.
One verb, no resume twin — the same lesson that retired the
`naps` / `rnaps` pair for planning.

The session is named `ceo-<topic>`, so the fleet listing reads
by role: `eng-*` implementers, `plan-*` planning, `ceo-*`
architecture. The topic is load-bearing, not decoration —
each long-horizon design thread gets its **own resumable
session**, so parallel threads never share context and a
volatility conversation cannot drift into a custody one.

### Check the model before doing anything else

<!-- render:begin fable-model-guard verb=caps -->

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
1. *"Relaunch via `caps`"* — the deterministic path, at the cost of
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
relied on — `caps` passing `--model claude-fable-5` at launch is the
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
- **No source edits.** Specs land through the handoff, not
  through commits. A design that needs code to exist before it
  can be judged is a spike, and a spike is an implementation
  session.

## Required artifacts

A bookkeeper would not produce these, and they are what make
the session's output actionable rather than a transcript:

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

The note carries: the decisions, the rejected alternatives
with reasons, the failure paths, and the affected track or
issue numbers. The planning session executes every board
consequence.

## A note on the shared substrate

`plan` and this skill share most of their machinery — base-repo
launch, the model guard, the document conventions, the
write-mangle rules, no source edits, context economy — and
differ only in the **briefing**: the duty list, and what each
is allowed to write.

That overlap is real and worth de-duplicating **later**, when
the template extraction lands. It is deliberately not a reason
to delay this skill: implement the architect first, then let
the generator fold the shared blocks in. The launcher pair is
likewise one parameterized helper in the committed shell init,
differing only in session name and initial prompt.
