---
name: plan
description: Run a planning session — the complement to a worktree implementation session. Bootstraps from the "Planning" Linear document (id in `LINEAR_PLANNING_DOC_ID`), surfaces the Todo umbrellas and the standing audit directive plus its outcome unprompted, then keeps the board coherent: the Queue honest, blocking edges curated, file collisions reconciled by reading, not by a tool (this session is the only place that happens at all — the automated collision machinery is retired and nothing files a collision link), parked audit findings offered for sequencing (promotion = clear the milestone AND move Todo → Backlog), and issues filed and amended to house convention. Writes decisions back into the Planning doc incrementally and as a wholesale rewrite at close-out — which also writes the next audit directive, one named subsystem or interface for housekeeping to audit, or explicitly none — and captures the session's own token profile as parked lever issues. Planning sessions run in the base repo (started and resumed with `paps`), never in a worktree.
user-invocable: true
model: fable
---

<!-- cspell:word startable -->

<!-- cspell:word backticked -->

# `plan`

The working split is two session kinds. **Implementation**
sessions run one deterministic spec to completion in a
worktree: `/init-pr` → build → `/review-pr`. **Planning**
sessions are their complement — they stage that work, keep
the board coherent, and carry direction across days.

This skill is the planning session's method. It exists so
each new session doesn't reconstruct it from scratch.

## The Planning document is the living state

The session's memory between days is a **Linear document**
titled "Planning", whose id is exported as
`LINEAR_PLANNING_DOC_ID` in `~/.zshrc`. Resolve it with a
bare `printenv` — its own call, per
`docs/conventions/linear-automation.md`:

```sh
printenv LINEAR_PLANNING_DOC_ID
```

If it is unset, say so and stop rather than searching Linear
for a document by title: a planning session that guesses its
own state file will write decisions into the wrong place.

**The skill maintains that document; it never becomes a
second copy of it.** Nothing here restates the current phase,
the standing decisions, or the sequencing — those live in the
doc, they change weekly, and a copy in this file would be
wrong within days. This file carries only the *method*.

Two standing rules for the doc itself:

- **Work items live in issues, never here.** The document
  carries direction, sequencing, and vocabulary.
- **It is not an append-accumulating log.** It carries the
  **current state of play** and nothing else. Step 6 says
  how that is maintained: incremental appends during the
  session, consolidated away by a **wholesale rewrite** at
  close-out.

## Where it runs

The **base repo**, never a worktree — a planning session
touches the board, not a branch. Start *or* resume one with
a bare **`paps`** (Planning Agentic Programming Session):

```sh
paps
```

`paps` is **idempotent by design** — one verb, no
new-vs-resume split to remember. It names the session
`plan-<day-of-month>` (run on the 14th → `plan-14`), and:

- if today's `plan-<day>` session does not exist, it creates
  it — in the base repo, with `--model claude-fable-5`, and
  `/plan` as the initial prompt so this skill bootstraps
  immediately;
- if it already exists, it **resumes** it.

That supersedes the older `naps planning-<day>` /
`rnaps planning-<day>` pair, and the older `planning-<day>`
session naming. `paps`, its worktree counterparts `aps` /
`raps`, and the general-purpose `naps` / `rnaps` are
documented in `docs/conventions/local-integrations.md`.

### Check the model before doing anything else

Planning sessions deliberately run the most capable model —
**fidelity is the point** (step 7 makes that a hard
constraint on what may be proposed). A planning session that
has quietly landed on the default implementation model will
still *work*, which is exactly why the slip goes unnoticed.

So on invocation, **before the bootstrap read**, check the
model this session is running as. The system prompt states
it. If it is **not** a Fable/Mythos-tier model, say so and
offer the fix via `AskUserQuestion`, recommended option
first:

1. *"Run `/model fable` now and continue"* — recommended;
   it switches the running session in place.
1. *"Relaunch via `paps`"* — the deterministic path, at the
   cost of restarting the session.
1. *"Continue on this model anyway"* — proceed, and don't
   ask again this session.

This is the mirror of `init-pr`'s Fable guard, pointing the
other way: that one catches a planning-tier model about to
burn an implementation run, this one catches an
implementation-tier model about to make board decisions.

The `model: fable` frontmatter on this skill is
**belt-and-braces, not the mechanism**. Whether it switches
the session going forward or applies only to this
invocation's execution is not specified, so it is not relied
on — `paps` passing `--model claude-fable-5` at launch is
the deterministic path, and the check above is what catches
every other route in.

## Steps

**1. Bootstrap, and surface the umbrellas unprompted.**
Fetch the Planning doc with
`mcp__claude_ai_Linear__get_document` (id = the resolved
value) and read it first — it is the handoff from the last
session, and everything below assumes it.

Then read the board. **The board has three tiers, and which
tier an issue sits in is the whole scoping signal:**

- **Backlog is the pullable set** — everything about to be
  pulled into an agentic programming session. This is the
  Queue, whose honesty is step 2's job. A genuinely
  startable work item belongs here and nowhere else.
- **Todo holds the live tracks** — the umbrellas and
  initiatives, each a parent that accrues children. It is
  **not** a staging area for work items. An item stranded in
  Todo is invisible to the operator, who pulls from Backlog.
- **A project milestone means parked** — as in *not this
  phase*. The milestones are thematic parking lots that
  Linear collapses out of the default view. Anything
  carrying one is out of scope for a bootstrap read unless
  the operator names it.

So the bootstrap reads the **un-milestoned Backlog** (the
Queue) and the **un-milestoned Todo** (the live tracks).
Linear's issue-list API has **no milestone filter**, so the
drop is **client-side**: request the `projectMilestone`
field and ignore every issue carrying one.

Prefer the compact listing for this — it is the same
information for a fraction of the echo. **Both tiers, two
calls**: `list` defaults to the Backlog, so the Todo half —
the live tracks you are about to say out loud — needs its
own invocation.

```sh
python3 .claude/tools/board_batch.py list
```

```sh
python3 .claude/tools/board_batch.py list --state Todo
```

Milestoned (parked) issues are dropped from both by default.
Pull a full body only when a decision turns on it.

**What keeps the Todo set small:** a planning session either
**acts on** an un-milestoned Todo item or **parks** it.
State that discipline out loud when the set has grown; it is
the only thing that stops the bootstrap re-growing.

**Then say the umbrellas out loud, without being asked** —
"these meta tracks are open: …". The operator should not have
to remember to ask which tracks exist; that is exactly the
context the session was started to hold.

**And fire step 8's parked-findings offer here**, as part of
this same bootstrap. It is written up as step 8 because
`audit` and `housekeeping` both cite it by that name, but it
**runs now** — a count and a prompt, alongside the umbrellas.
Do not defer it to the end of the session.

**Read the standing audit directive and its outcome too.**
The document carries one — a named subsystem or interface for
`housekeeping` to audit, or explicitly none — plus what the
previous one found. Surface both alongside the umbrellas: the
outcome is the audit log the operator reads, and the standing
directive is what the close-out (step 6) will either keep or
replace. A directive whose target has since been rewritten is
stale and should be replaced rather than left to fire.

**Reconcile file collisions once, here — by reading, not by
running a tool.** This is the only session that reconciles
overlap at all: the automated file-collision machinery is
retired, and nothing records a collision link any more.
Judge it from the board read you already have. You are
holding enough of each issue's prose to know what the work
actually is, which is the whole argument for doing it here —
content beats the exact glob matching the retired tool did,
and it is why that sweep was dropped rather than moved. (The
declared-scope `**Touches**:` field went with it, so there is
no shortcut: read what the issue is about.)

Record the conclusions as **collision clusters** (see
`docs/conventions/linear-automation.md` → "Overlap is a
cluster, never an ordering"): group per shared area, never by
connected component, and never as an ordering. Step 2 uses
those clusters for merge proposals and step 8 uses them to
pick a parallelizable batch. Write them into the document as
prose — do **not** file a `related` link for one.

**Then look for the two scheduling smells** and act on what
you find — a dead edge holding an issue out of the available
set is invisible until something looks for it. Read the
blocking edges off the board directly: an edge whose blocker
is already closed, and an unblocked Urgent issue sitting
behind one, are both visible in the step-1 read.

Two notes on cost, because they correct the obvious
intuition. The whole board read is roughly **2–3k tokens**,
while **one issue-save echo for a single field change is
roughly 3k**. Narrowing the read buys **judgment** — a
smaller, truer picture — not tokens. The token lever is
batching *writes*, which is step 4's tool.

*Rejected alternative, recorded so it is not re-derived:*
filtering Todo to parents that have **no children** was
considered and rejected — it inverts both real cases. The
market-data umbrella is a parent *with* children and is the
most important item on the board; a childless issue may be
the most parked thing there is. Structure is not a proxy for
currency. The milestone is a deliberate signal; child count
is not.

**2. Keep the Queue honest.** The Queue — the unblocked
Backlog — is the **safe-to-start set**, and its meaning is
load-bearing: an implementation session pulls from it and
runs to completion without re-planning. So every unblocked
issue must be genuinely startable *now*, without carrying
refactor-later burden that a later issue is going to undo.

**The operator calls this view "Next"** (earlier: "Q"). It is
a Linear view over the same set this skill calls the Queue,
so the two words name one thing and "put it in Next" needs no
follow-up question. Four semantics follow, and they are the
whole reason the vocabulary is worth writing down:

- An issue appears in Next **iff** it is in Backlog with no
  live blocking edge. So "add something to Next" means: make
  it a Backlog task with nothing blocking it — by resolving
  or re-scoping what blocks it, **never** by silently
  dropping a real dependency.

- **Priority within Next is the pull order.** The operator
  launches an implementation session by pulling the
  highest-priority item there, so setting a high priority in
  Next *is* how a planning session decides what gets built
  next. It is the lever, not a label.

- **Blocking edges and priorities are curated here**, in the
  planning session orchestrating those implementation
  sessions. Implementation sessions never place edges (see
  `CLAUDE.md` → "Blocking relations").

- **`Claude:`-prefixed meta-work is always Urgent once
  unblocked** (operator rule, 2026-08-17). When a meta issue
  enters Next, set it Urgent, so agent-infra improvements are
  the next pull rather than queuing behind product work.

- **The serial meta chain** (operator rule, 2026-08-18). Open
  `Claude:`-prefixed issues are kept blocking one another in
  **one serial chain**: file a new meta issue, and this
  session chains it behind the current tail. So exactly
  **one** meta issue is ever unblocked in Next — Urgent, per
  the rule above — and meta improvements land one batch at a
  time as each head merges, instead of several sessions
  rewriting the same skills at once.

  This is the **one standing exception** to the
  proposal-per-edge rule: an operator-ratified edge *class*,
  so a planning session places a chain edge as routine
  bookkeeping without a fresh per-edge `AskUserQuestion`. It
  changes nothing for anyone else — **automated filers still
  place no edges, ever** (`CLAUDE.md` → "Blocking
  relations"). The exception is this session's, and it is
  scoped to this one chain.

When an issue isn't: reorder it behind what it depends on,
re-scope it, or split the first genuinely actionable unit
into its own pullable task. An issue that is startable only
"if you also do X first" is a scheduling bug, not a task.

**Propose merge groups aggressively, to minimize open PRs.**
Scan the Backlog for clusters that would land as a single PR
— same subsystem, crate, or language-domain — and propose
folding them via `/merge-tasks`. The collision clusters you
recorded in step 1 are the raw material: a cluster whose
members share files is usually a cluster that wants to be one
PR.
The **coherence floor** still binds — never fold across
separate apps, languages, or deploy units.

**3. Curate blocking edges — this is the one place they are
placed.** Blocking edges mean **semantic rollout ordering**,
and they are human-curated end to end: no automated writer
files one, ever (see `docs/conventions/linear-automation.md`
→ "Blocking relations"). Planning sessions are where they are
placed, changed, and removed, because that is where somebody
is actually deciding the order.

Place the operator's decided edges in one batch:

```sh
python3 .claude/tools/board_batch.py edges --pairs <file>
```

Removing an operator-retired edge is the same command with
`--remove`. **Rehearse either with `--dry-run` first** — a
blocking edge drops an issue out of the available set, so a
wrong one is expensive, and the flag is accepted in either
position.

That subcommand is the **planning session's hands**, not a
new writer: it takes an explicit pair list, has no discovery
mode, and refuses an empty list. It executes a human's
decision — it never derives one.

Three things that are **not** blocking edges, each with its
own handling:

- **File overlap** — recorded as a collision cluster in your
  own notes (step 1), never as a relation. It costs a rebase,
  not an ordering.
- **Coupling that belongs in one PR** — fold the issues into
  one via `/merge-tasks`, don't relate them.
- **A suspicion** — record it as prose in the body. An edge
  you are not sure about costs more than a missing one: a
  spurious edge silently drops an issue out of the available
  set.

**4. File and amend to house convention.** A planning session
does a lot of writing, and every rule that binds a filing
skill binds it too:

- **Dedup first** — search the open Backlog before filing.

- **Amend with `patch` ops**, never by re-sending a body;
  anchor on **plain prose**, never on a line carrying an
  `ENG-###` (Linear stores it as a mention node) or heavy
  bold / code spans. The op's payload field is **`text`**,
  not `content`, and the anchor must match the **stored**
  text, not the rendered body.

- **Fewest coherent PRs** — fold coupled findings, but never
  across separate apps, languages, or deploy units.

- **Todo / Backlog split** — initiatives and meta work in
  **Todo**; pullable work items in **Backlog**, which is what
  the operator pulls from. See step 1 for the full three-tier
  schema, milestones included.

- **`Claude:` prefix** on meta-work titles (anything every
  one of whose edited paths sits under `.claude/**`,
  `CLAUDE.md`, or `docs/conventions/**`). A planning session
  is where most meta-work gets filed, so this is the skill
  that emits the prefix most often.

- **The `**Fingerprint**:` line is mandatory**, exactly as
  for every other filing skill — it is the dedup key, and its
  first token is a **dotless domain token**, never a bare
  `name.ext`, since Linear linkifies a hostname-valid
  basename and corrupts the key. It is now the **only**
  structured field: `**Touches**:` is retired and nothing
  emits it. See `CLAUDE.md` → "Structured filing fields".

- **Field-only writes go through the tool, not the MCP.**
  Priority, state, parent, milestone, labels and assignee
  change no body, yet `save_issue` echoes
  the whole stored body back for each one. One session made
  **17 field-only writes and paid ≈40k** for confirmations
  that fit on 17 lines:

  ```sh
  python3 .claude/tools/board_batch.py fields --updates <file>
  ```

  **Body edits stay on the MCP `patch` path** — that is not
  an omission, see step 4's note in
  `docs/conventions/linear-automation.md`.

- **Buffer folds onto the same issue; write them once.**
  Every `save_issue` echoes the issue's whole body back,
  whatever the write said — so an aggregated survivor is
  most expensive to touch exactly when it is being folded
  into most. One planning session made **97 `save_issue`
  calls for ≈164k**, its five costliest results all late
  folds onto one survivor. When several folds and a body
  amendment land on the same issue in one sitting,
  accumulate them into a **single** call (a `patch` array
  takes up to 50 ops) rather than writing each as it is
  decided. See `docs/conventions/linear-automation.md` →
  "Partial edits".

- **File no relations.** There is no per-issue collision step
  after `save_issue` — the automated file-overlap machinery is
  retired. Overlap is your step-1 judgement, recorded as prose
  clusters; blocking edges are step 3's job and yours alone.

- **A re-scope has to reach the whole body, not one
  paragraph.** Appending "this also needs X" while the rest
  of the issue still describes the narrower job leaves a spec
  that contradicts itself, and the implementing session reads
  it cold. This has happened in the sharper form the retired
  `**Touches**:` field used to make visible: one issue's prose
  said its scope was widening while the declared-scope line
  still named a single file, and the shipped diff spanned four
  areas. Losing the field lost the tell, not the failure — so
  re-read the sections a re-scope contradicts and amend them
  in the **same write**.

  Same for `**Fingerprint**:` when the re-scope changes what
  the issue is fundamentally *about* — that is the dedup key,
  and a stale one silently re-files.

- **Tell an in-flight session when you amend its issue.**
  An implementation session reads the body once at the
  start; anything appended afterwards is invisible to it
  until its next echo, which costs it a write it could not
  have batched (see `docs/conventions/linear-automation.md`
  → "The write floor assumes the body is read once"). Step 5
  is the mechanism — a message turns an invisible amendment
  into a known one.

- **Retiring a framing means sweeping the board for it, in
  the same session.** A retired decision does not retract
  itself from the issue bodies that already quote it, so
  every session that later reads one of those bodies
  re-emits the stale framing — and a planning session
  correcting it downstream is pure rework. This is
  load-bearing because of *what implementation sessions
  actually read*: a decision recorded only in the Planning
  document never reaches a session that reads only its
  issue. So when a standing decision retires language,
  search the open board for that language and strike it now,
  rather than leaving it to be re-derived.

**5. Coordinate with in-flight sessions.** A planning
decision can invalidate the ground truth an implementation
session is working from — a re-scope, a changed sequencing, a
retired approach. When that happens, **message the affected
worktree session** rather than letting it discover the change
at review time. Use `ListAgents` to find them.

**6. Write back — incrementally, then rewritten at the
gate.**

**Prefer writing after each major decision** over composing
one entry at the end. Planning sessions are frequently closed
abruptly — the terminal is killed, the worktree is deleted —
and an un-written decision is simply lost. Incremental
write-back means an abrupt close costs nothing.

**Then gate the close-out; don't wait to be told.** When the
operator signals wrap-up, **or the conversation lulls after
significant decisions**, fire an `AskUserQuestion` offering
to close out — "close out now? (rewrites the Planning doc to
the current state of play)" — with that as the recommended
first option. The gate exists precisely because the
write-back is otherwise the thing that gets forgotten.

**The close-out deliverable is the whole document, rewritten
— not an entry appended to it.** Superseded phases, dated
addenda, and previous close-out entries simply **disappear**:
the board and the issues carry history, the doc carries the
**present**. The incremental appends made during the session
are consolidated away by the same rewrite.

Composing that replacement has one hazard worth stating,
because a wholesale write is exactly where it bites. A full
content replacement **re-parses** on the round trip, so the
rewritten document is composed:

- **without inline code spans** — use plain names, not
  backticked ones; a code span can come back corrupted;
- with **no emphasis span crossing a newline** — a bolded
  run wrapping a line break stores garbled;
- with **no machine-parsed field starting with a bare
  hostname-valid `name.ext`** — the linkifier rewrites it
  (the same rule as the fingerprint format in step 4).

The document's own header paragraph states this convention.
**Keep the skill and that header consistent** when editing
either.

**A session that decides without writing back has dropped its
main deliverable.** The board shows *what* was decided; the
doc is the only place that carries *why*, and why is what the
next session needs.

**The close-out also writes the audit directive.** The
rewritten document carries an **audit directive** naming
**one** subsystem or interface from
`docs/conventions/audit-registry.md` — or explicitly **none**
— for `housekeeping` to audit until the next planning session
replaces it. Choose the target because it *just settled*, is
*about to be built on*, or is *suspect*; "none" is a
legitimate and often correct answer in a heavy-feature phase.

This is what replaced the daily random rotation. A random
seven-unit pass in a heavy-feature phase audits code that is
about to be rewritten: one pass filed **fifteen** parked
findings, several against maker-model and fair-value files
that open Backlog issues were already slated to rewrite. The
engine was working; the targeting was not — and because the
parked pool drains only through the promotion step (step 8,
which runs at bootstrap), over-filing costs this session
directly.

Also **record the previous directive's outcome** — its target
and finding count, from the housekeeping report — so the
document accumulates the ongoing audit log the operator asked
for: what has been audited, when, and what it found.

**7. Capture the session's token profile at close-out.** Run
the committed metrics tool over the planning transcript, then
file each lever it yields as a **parked lever issue**, the
same as an implementation session — so the fold mines
planning-session shapes too, rather than only review shapes:

```sh
make session-metrics SESSION=<uuid>
```

Levers file through `.claude/tools/trim_levers.py` (probe,
then `file` or `append-evidence`), **not** into a document —
that inbox is retired. Invoking `/session-metrics` does this
for you; the point of naming it here is that a planning
session is a producer too.

**One hard constraint on what may be proposed from it.**
Planning sessions deliberately run the most capable models
with generous context. **Fidelity is the point, and is never
traded for tokens.** So a planning-derived lever must be
**quality-preserving**: a cheaper bootstrap fetch, a narrower
Linear echo, a repo survey hoisted into a sub-agent, a
redundant re-read removed. Never a model downgrade, never a
skipped bootstrap read, never anything that thins the
decision context.

A lever that would trade planning fidelity for tokens is not
proposed — it is **filed and immediately closed with its
reason**, or, if an equivalent lever is already parked, left
closed. Recording the rejection on the board is what stops the
next fold from re-deriving it, since the fingerprint probe
searches resolved issues too.

**8. Offer the parked audit findings — don't wait to be
asked.** `audit` files its confirmed findings as real issues
stamped with the **Audit findings** project milestone, which
means they are **parked**: first-class open issues for dedup
and search, but costing a bootstrap read nothing. The repo is
continuously audited, so this step is
how the planning flow continuously **absorbs** that output
instead of letting it pile up unlooked-at.

**Offer at bootstrap: a count and a prompt, never a
listing.** "N audit findings are parked — slate any in?" The
bodies stay unread unless the answer is yes.

On a yes:

- List the Audit findings milestone **field-selected**
  (title and priority only).
- **Select a parallelizable batch, using the collision
  clusters.** The operator runs several worktree sessions at
  once, so a promoted batch is only useful if its members can
  be worked **simultaneously** — which means they must not
  collide on files. Your step-1 clusters already answer this:
  promote a set whose members appear in **no common
  cluster**. Where two findings
  do collide, either promote one and leave the other parked
  for the next round, or fold them into a single issue under
  the fewest-coherent-PRs rule. This is the selection
  procedure, not advice.
- **Sequence against the live tracks**, not as a free-floating
  batch. A finding that touches a track's files while that
  track is mid-flight should ride with it or wait for it. The
  current phase decides *what* gets promoted; the umbrellas
  decide *where it belongs*.
- Fetch a body **only** for a finding actually being
  promoted.
- **Un-park it by clearing the milestone AND moving it Todo →
  Backlog.** Promotion has **two** halves now, because parked
  findings file in state `Todo` rather than `Backlog`:
  clearing the milestone alone leaves the issue out of the
  pullable set, and moving the state alone leaves it looking
  parked. Do both, in one field-only write per issue through
  `board_batch.py fields` (an MCP `save_issue` would echo the
  whole body back for a two-field change). There is still no
  re-filing and no new issue — the obvious wrong
  implementation is to close the parked finding and file a
  fresh copy; don't.

**A planning session does not re-adjudicate a finding.**
`audit` already cross-checks adversarially before filing, so
a parked finding is **validated**. The only decision left is
whether it belongs in the current phase. Re-validating would
duplicate the expensive half of the audit inside the most
expensive session in the rotation.

## Notes

- **No source edits.** Like the other board skills, this one
  writes to Linear and to the Planning document. It authors
  no diff, and commits nothing.
- **It is not a one-shot.** A planning session is a
  conversation that spans hours or days; this skill describes
  how to run one, not a pass to execute and exit.
- **The board is this session's alone.** `housekeeping` no
  longer analyses the board — no merge groups, no scheduling
  smells (and no collision recording anywhere — that
  machinery is retired) — precisely so the two can't
  reach conflicting conclusions between planning sessions.
  Its one remaining carve-out is proposing merges among
  `Claude:`-prefixed meta-work, which is upkeep on its own
  filing output.
