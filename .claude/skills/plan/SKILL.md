---
name: plan
description: Run a planning session — board work, the complement to a worktree implementation session, in the base repo and never a worktree. Bootstraps from the "Planning" Linear document (`LINEAR_PLANNING_DOC_ID`), polls the Backlog review inbox, runs the audit heartbeat (file an audit issue or decline with a recorded reason), and folds the parked `Claude meta` pool into small themed edge-free batches, promoting a file-disjoint subset. Then keeps the board coherent: Todo dispatched as standing authorization, blocking edges curated by a human, file collisions reconciled by reading, parked findings promoted to keep the inbox stocked, issues filed and amended to house convention, and architect or explore specs folded into their implementing issues. Writes decisions back to the Planning document as it goes and rewrites it at close-out.
user-invocable: true
model: fable
---

<!-- cspell:word startable -->

<!-- cspell:word backticked -->

# `plan`

Implementation sessions run one deterministic spec to completion
in a worktree (`/init-pr` → build → `/review-pr`). Planning
sessions are their complement: they stage that work, keep the
board coherent, and carry direction across days. This skill is
their method. Measured incidents and retired designs behind the
rules are in [`history.md`](history.md).

## The Planning document is the living state

The session's memory between days is the Linear document
"Planning", whose id is `LINEAR_PLANNING_DOC_ID`. Resolve it with
its own bare call (`docs/conventions/linear-automation.md`):

```sh
printenv LINEAR_PLANNING_DOC_ID
```

If it is unset, say so and stop — never search Linear for it by
title, or decisions land in the wrong place.

**The document is ephemeral state only.** Every standing
instruction is committed: the method here, the operator's
rulings, fleet doctrine and standing decisions in
[`rulings.md`](rulings.md). Work items live in issues. The
document is **not** an append log; step 6's rewrite produces
exactly these headings, in this order, and nothing else:

1. **Discuss with operator** — a single blank checkbox the
   operator fills before a session (Enter in Linear adds the
   next).
1. **Current direction and checkpoints** — the phase, the live
   tracks, the operator-named exceptions to any freeze, and the
   next bootstrap's first moves.
1. **Operator actions outstanding** — numbered, kept current.
1. **Fleet state and chains** — in flight, in review, queued,
   held with reasons, and the parked counts.
1. **Collision clusters** (step 1).
1. **Audit state** — the bounded table (step 1).
1. **Session-metrics state** (step 7).
1. **In-session notes** — this session's incremental writes;
   after a close-out, only the rulings not yet landed in
   `rulings.md`.
1. **Notes for the next planning session** — the only heading
   other sessions write under. The upkeep tool's machine-written
   report is its level-2 subsection, carried verbatim.

Above them, a header of one line plus the `Bootstrap stamp`.

**A new ruling is a reviewed change**, never a document edit:
file a `Claude:` issue carrying its text, which a worker lands as
a PR to `rulings.md` — under a meta freeze, only once the
operator names it — and carry it under `In-session notes` until
it lands.

## Where it runs

The **base repo**, never a worktree. Start or resume with a bare
`plan` — idempotent; it creates or resumes today's
`plan-<day-of-month>` on the advisor tier's model, with `/plan`
as a new session's prompt. `plan bedrock` is the credit-pinch
override. There is no in-session model check: the launcher pins
the model and refuses an unresolved tier. Verbs:
`docs/conventions/local-integrations.md`.

## Cycling

The hub's prefix only grows, so the session **cycles**: close
out, exit, and the `plan` launcher relaunches it in the same tab
under a fresh id and the same name. Compaction is a manual
operator lever, never this mechanism.

- **The operator types `cycle`** → run the procedure now, no ask.

- **The trigger.** At each idle point — turn done, nothing
  queued, no consult half-answered — run:

  ```sh
  python3 .claude/tools/plan_cycle.py check
  ```

  On `"due": true` (a 250k prefix or 40 inbound messages), and no
  fold sub-agent still running (its notification would be lost),
  ask once: cycle now? — yes recommended. On no, run
  `plan_cycle.py snooze` (+50k tokens / +15 messages).

- **The procedure.** A one-line FYI to every live worker with an
  unanswered consult — planning is cycling, re-send anything
  unanswered; then step 6's close-out rewrite, echo verified;
  then `plan_cycle.py cycle`, which leaves the marker and ends
  the client.

- **The warm bootstrap.** A full bootstrap writes a
  `Bootstrap stamp: <YYYY-MM-DD>` line to the document, and every
  close-out rewrite keeps it. A bootstrap finding today's date
  there is **warm**: read `rulings.md` and the document, run
  `ListAgents`, surface
  the outstanding list — and skip the once-a-day duties (the lever
  and meta folds, the audit heartbeat, the parked counts, the upkeep
  report, step 8). No stamp
  means a full bootstrap.

## Steps

**1. Bootstrap.** Read
[`rulings.md`](rulings.md) first — the rules this session obeys.
Then fetch the Planning document with
`mcp__claude_ai_Linear__get_document` — the last session's
handoff — and, warm or full:

- **Act on each `Discuss with operator` item first**, before the
  board — one needing the operator's word joins step 8's ask.
- **Surface `Operator actions outstanding` unprompted**, itemized
  and numbered, so the operator answers all of it in one message.
  Never leave it waiting for the operator to open the document.
- **Verify overnight landings with `git log`** and the In Review
  listing, never the Done column; release chains as blockers land.

On a full bootstrap, the last two run after the lever-fold spawn
below.

**Say the upkeep report's highlights, with the parked counts.**
The `plan` launcher ran `.claude/tools/upkeep.py` before this
session started and spliced its report under the document's
machine-written `Upkeep report` heading, so the fetch above holds
it; never re-run the pass here. Say its `parked` line — each
pool's count and direction (ruling 23) — what was pruned and dismissed
(or would have been, unarmed), each refusal (unpushed work, an
open PR — reported, never retried), and any inert guard,
dangling reference or file near its size cap. Say the
`fleet-cost` day lines and the `allowance` per-day figure too,
any flagged day first. A run stamp
older than today means the pass did not reach the document —
say so. Its **destructive items**
are operator-only and ride step 8's `AskUserQuestion`:

- **allowlist cruft** — remove the approved entries from
  `<base>/.claude/settings.local.json` with the Edit tool, then
  an exit-code-only parse check;
- **permission-refresh candidates**, when listed —
  `allowlist.py add` per approved rule;
- **the transcript-purge apply** — `/purge-conversations`, this
  yes being its approval.

**Spawn the lever fold — every full bootstrap, unasked**, right
after saying the parked counts, unless the report's line reads
`Trim levers 0`. Invoking `plan` is the authorization (ruling 11).
One fresh background `general-purpose` `Agent` — never a fork, no
model override — whose brief is pointers, not content: the
canonical brief from

```sh
python3 .claude/tools/lens_preamble.py --out <scratchpad>/fold.md \
  --no-facts
```

then: this is a fold, not a review — read-only toward the repo,
but its Linear writes are the task; run `/trim-context`; fold and
kill with reasons, unasked (ruling 23);
at most 40 turns; the final message is the report, at most thirty
lines. Add one paragraph only for what the operator said that the
document does not hold. The fold's reads stay in its context.

**The bootstrap runs unattended until its one ask.** After the
spawn, do every duty that asks nothing — the outstanding list,
`fleet resume`, the overnight landing check, the board read, the
Queue's honesty — then end the turn (with no spawn, go straight
on). The completion notification resumes it: say the report,
record it under `In-session notes`, finish the board (the meta
fold included, since the fold files into `Claude meta`), and stop
at step 8's consolidated ask. A
Discuss item or duty needing the operator's word joins that ask.
While the fold runs, make **no** `Trim levers` or `Claude meta`
write, and never fold levers by hand while a sub-agent can.

Then read the board. **Which tier an issue sits in is the whole
scoping signal:**

- **Backlog is the review inbox** — specs awaiting the
  operator's review. Priority there is review order.
- **Todo is the approved queue** — the operator's move to Todo is
  standing authorization to dispatch (step 5); priority there is
  advisory. Planning never moves an issue to Todo.
- **A project milestone means parked** — Backlog plus a parking
  milestone, not this phase. Out of scope for a bootstrap read
  unless the operator names it.

Two calls; `list` defaults to Backlog, milestoned issues dropped:

```sh
python3 .claude/tools/board_batch.py list --include-milestoned \
  --show-milestone
```

```sh
python3 .claude/tools/board_batch.py list --state Todo
```

The first keeps parked issues, tagged, for the meta review.
**Say three counts**: the inbox (untagged Backlog), its Urgent
subset, and Todo. Pull a body only when a decision turns on it.

**Poll the inbox** — inbox specs changed since the day before
the prior stamp (`<WHEN>` as `<YYYY-MM-DD>T00:00:00Z`), skipping
any a live architect or explore session owns:

```sh
python3 .claude/tools/board_batch.py list --updated-since <WHEN>
```

Handle their open threads by **the fold protocol**, which every
reviser reuses: fold the comment into the body, reply saying how
it was addressed, then resolve the thread
(`linear_gate.py resolve <comment-id>`). Resolved means
addressed-and-how, never approval. Never fold an In Progress
spec; a fold into an approved (Todo) body moves it back to
Backlog, priority and edges unchanged.

**The one relay is "please review the Urgent specs (N)".** Keep
the Urgent set small, a few at a time, and the inbox stocked so
the operator is never idle: when it runs thin, stage more specs
before anything else.

**Then read Next as the operator sees it** — their named Linear
view, filtered by Linear rather than reconstructed:

```sh
python3 .claude/tools/board_batch.py view
```

**Verify Next against the holds.** Name each row this session
would hold, with its blocker, in step 8's ask; on a yes it gets a
blocking edge through step 3's `edges` (dry run first): a hold is
an edge or it does not exist.

**Run the audit heartbeat — every full bootstrap, unprompted.**
The document
carries a bounded **audit-state table**, one row per unit in
`docs/conventions/audit-registry.md` (unit, last audited, finding
count, outcome pointer). Act on it:

- **A unit is stale**, or it *just settled*, is *about to be
  built on*, or is *suspect* → **file an audit issue**.
- **Nothing qualifies** → **decline, and record the reason** in
  the document. An unrecorded non-decision is the failure this
  step exists to catch.

Staleness is judged against the **code**, not the calendar: a
unit rewritten since its last audit is stale in the way that
matters; one untouched because its subsystem is untouched is not
urgent for being old.

**An audit issue** is a first-class **Backlog** task naming one
registry **target** (subsystem, interface, or a random-target
pass for breadth), its **scope**, its **rationale** (which
criterion fired, with evidence), and its **sequencing** as prose.
It competes in the queue like any capacity spend — an audit that
cannot win that argument should not run this week. The pulling
session runs it as one scoped `audit-scope` run, which that pull
authorizes, fan-out included; findings file parked (Backlog plus
`Audit findings`).

It dispatches as **`explore <tag>`, never `task <tag>`**
(operator rule, 2026-09-11) — a temporary worktree, no PR; see
`docs/conventions/local-integrations.md` → "Architect and explore
sessions: temporary worktree, no PR". **Name the verb in the
sequencing prose**, since a later puller reads only the body. An
audit sits **In Progress** until its findings are ratified and
folded, so an old In Progress audit is live, not stalled.

**Fold the `Claude meta` pool into SMALL THEMED batches — by
default, unprompted.** Every `Claude:` filing lands parked
(Backlog plus `Claude meta`). The pool is that milestone **plus
every open, unblocked `Claude:` inbox issue** (never Todo) —
without that clause an unpulled batch is
duplicated rather than absorbed. Group by the surface each issue
changes and fold each group into its lowest-numbered survivor via
`/merge-tasks`. Routine bookkeeping; no per-fold proposal.
Promotion stays subject to any freeze in `rulings.md`.

- **Roughly 4–5 parts per batch** (operator rule, 2026-09-11):
  five or fewer parts is one batch, fifteen is three. Session
  cost is roughly quadratic in length —
  `docs/conventions/context-economy.md` → "Session length is
  itself a cost lever". Cross-check against that doc's line band
  (300–800 changed lines, floor ~150, split at 1,000); the parts
  count is the primary bound. A batch far under the floor may
  take another part **of the same theme** — never an unrelated
  one, never padding. Which rule yields when they disagree: same
  doc → "Which one yields in the interim".

- **Fold strays, promote batches.** An already-assembled batch,
  parked or promoted, is never a fold input —
  `/merge-tasks` cannot split, so a re-fold only grows it.

- **Survivors file parked**; only the ones you promote enter the
  inbox, Urgent. Assembling and promoting are two acts.

- **Nothing serializes batches** — no edge, no in-flight gate.
  Several may be open at once; their file contention is paid as
  a rebase. What bounds Next is promoting only a
  **file-disjoint** set (step 8's selection procedure, on the
  collision clusters), never a state check or a count.

- **Assembly consumes the pool as of this bootstrap.** A later
  stray waits for the next one.

- **Never fold an In Progress issue** — a session is
  implementing it. **Never fold across the meta /
  product boundary.**

- **Append to a batch with the zero-echo writer**, never
  `save_issue`: the MCP echoes the whole accumulated body on
  every write (one ~30-line append cost ≈6.6k).

  ```sh
  python3 .claude/tools/linear_issue.py append \
    --id <ENG-###> --file <scratchpad>/addition.md
  ```

  Anchored edits stay on `patch`.

**Review every meta issue at every full bootstrap — parked, Next
and Blocked — whether or not the document names a lane.** It is
a **necessity gate**: each ends doable, blocked, or killed. From
the two listings:

1. Say the counts out loud.
1. **Before assembly, kill on doubt**, per issue: one that cannot
   name a concrete recurring cost larger than its
   landing-plus-ratification cost goes on a kill list (titles
   first; read only a kill candidate's body).
1. After assembly, group the batches by the surface each rewrites
   (`review-pr`, `init-pr`, the shell init file, guard hooks,
   tools, conventions, AWS) from **titles alone**. A
   `[blocked by …]` row stays blocked behind its human-placed
   edge and is never promoted.
1. Mark each group **free**, or **in flight** when an In Progress
   / In Review meta issue (`list --state`) already rewrites that
   surface.
1. Promote a file-disjoint set from the free groups in one
   `board_batch.py fields` write (milestone `null`, Urgent); the
   operator reviews each in the inbox.
1. Offer the kill list in step 8's `AskUserQuestion`; on a yes,
   close each kill `Canceled` with its reason, as `trim-context`
   step 6 does.
1. Record promoted, killed, blocked, each held group with why,
   and the parked count under the `In-session notes` heading.

**Bound the Planning document.** It grows between close-out
rewrites: other sessions append, and an abrupt close skips the
rewrite. So:

- **If it carries foreign notes or a prior session's
  unconsolidated notes, the bootstrap's first write is the
  wholesale rewrite**, not another append.
- **This session's incremental notes** live under one
  `In-session notes` heading, so a rewrite sweeps one section.
- **Non-planning sessions append only under**
  `Notes for the next planning session` — stated wherever those
  sessions are told to write here.

**Reconcile file collisions once, here — by reading.** This is
the only session that reconciles overlap; the automated machinery
and the `**Touches**:` field are retired, so judge from the prose
you already hold. Record **collision clusters** per
`docs/conventions/linear-automation.md` → "Overlap is a cluster,
never an ordering" — per shared area, never by connected
component, never as an ordering — as prose in the document, never
a `related` link. Step 2 uses them for merge proposals, step 8
for parallel batches.

**Then look for the two scheduling smells, and act on what you
find** — an edge whose blocker is already closed, and an
unblocked Urgent issue sitting behind one. Both are visible in
this read, and invisible until something looks.

**2. Keep the Queue honest.** The Queue — the unblocked Todo,
which the operator calls **"Next"** — is the safe-to-start set:
an implementation session pulls from it and runs to completion
without re-planning, so every member must be startable *now*,
with no refactor-later burden a later issue will undo.

- An issue is in Next **iff** it is in Todo with no live
  blocking edge. "Put it in Next" means resolving or re-scoping
  what blocks it — **never** silently dropping a real dependency.
- **Edges and Backlog priorities are curated here**;
  implementation sessions never place edges.
- **A hold is an edge or it does not exist** (ruling 2). An issue
  held behind an in-flight surface or a sequencing reason gets a
  blocking edge on that blocker, asked in step 8's ask at
  bootstrap or as one question when found mid-session, so it
  shows as Blocked, not Next; a row awaiting that answer is not
  dispatched. A completed blocker stops blocking on its own;
  removing a hold edge on a re-sequence is the operator's call
  too, asked the same way and run through step 3.
- **A promoted `Claude:` batch is Urgent**, so agent-infra goes
  first (operator rule, 2026-08-17). Meta lands
  as short batches (step 1), several possibly in flight, with no
  edge and no exception to the proposal-per-edge rule.

An issue that isn't startable gets reordered behind what it
depends on, re-scoped, or split so its first actionable unit is
its own task. "Startable if you also do X first" is a scheduling
bug.

**Propose merge groups aggressively**, to minimize open PRs:
Backlog clusters that would land as one PR (same subsystem,
crate, language-domain) fold via `/merge-tasks`. Step 1's
collision clusters are the raw material. The **coherence floor**
binds: never across separate apps, languages, or deploy units.

**3. Curate blocking edges — the one place they are placed.**
Edges mean **semantic rollout ordering** or **this session's own
dispatch hold** (step 2), and are human-curated end to end
(`docs/conventions/linear-automation.md` → "Blocking
relations"). Place the operator's decided edges in one batch:

```sh
python3 .claude/tools/board_batch.py edges --pairs <file>
```

`--remove` retires one. **Rehearse with `--dry-run` first** — a
wrong edge drops an issue out of the available set.

Not edges: **file overlap** on its own, as a filer would record
it (a collision cluster — it costs a rebase, not an ordering);
**coupling that belongs in one PR**
(fold via `/merge-tasks`); **a suspicion** (prose in the body —
an unsure edge costs more than a missing one).

**4. File and amend to house convention.** Every rule binding a
filing skill binds this session:

- **Dedup first** — search the open Backlog.

- **Amend with `patch` ops**, never by re-sending a body; anchor
  on plain prose, never on a line carrying an `ENG-###` (stored
  as a mention node) or heavy bold / code spans. The payload
  field is **`text`**, not `content`, and the anchor must match the **stored**
  text.

- **Fewest coherent PRs, each sized to a SHORT session**
  (`CLAUDE.md`), the `Claude:` class included (`history.md`).

- **State a scope posture when staging an issue** — **hold**
  (build what is specified), **expand** (under-scoped; here is
  what is missing), or **cut** (over-scoped; here is the smaller
  thing). Default it from context and **say which you took**: an
  unstated posture is hold by inertia, which silently endorses
  whatever the filer wrote. It is one issue at staging time — the
  whether-to-build-it question is `architect`'s.

- **Count the declared parts against the scope band** and stamp
  a **`**Cost band**:`** line beside the fingerprint — a dollar
  range, a line band and the substrate, from
  `docs/conventions/context-economy.md` → "Cost bands". A band,
  never a point. A seat-bound issue (`architect`, `explore`)
  states its seat and a token profile instead: dollar bands bind
  Bedrock workers only.

- **The Todo / Backlog split** (step 1), **the `Claude:` prefix
  with its parked landing**, and **the mandatory
  `**Fingerprint**:` line** — as `CLAUDE.md` states them. **File
  no relations**: overlap is step 1's prose, edges step 3's.

- **Field-only writes go through the tool**, never `save_issue`,
  which echoes the whole body for each:

  ```sh
  python3 .claude/tools/board_batch.py fields --updates <file>
  ```

  Body edits stay on the MCP `patch` path
  (`docs/conventions/linear-automation.md`).

- **Buffer folds onto one issue; write them once.** Each echo
  returns the whole body, so a survivor costs most exactly when
  it is folded into most. Accumulate several folds and an amendment into a
  **single** `patch` call (up to 50 ops). See
  `docs/conventions/linear-automation.md` → "Partial edits".

- **A re-scope reaches the whole body**, not one paragraph —
  re-read the sections it contradicts and amend them in the
  **same write**, the `**Fingerprint**:` too when the issue's
  subject changes (a stale key silently re-files).

- **Tell an in-flight session when you amend its issue** — it
  read the body once (step 5).

- **Retiring a framing means sweeping the board for it now.**
  Issue bodies that quote it do not retract themselves, and a
  decision recorded only in the Planning document never reaches a
  session that reads only its issue. Search the open board for
  the retired language and strike it.

**5. Coordinate with in-flight sessions.** When a decision
invalidates an implementation session's ground truth — a
re-scope, a changed sequencing, a retired approach — **message
it** (find it with `ListAgents`) rather than letting review
discover it. Message freely: it is cheap, and it prevents
duplicated work. **When a session owns the issue, send it the
amendment rather than writing the body yourself** — that skips
the full-body echo, and the owner can judge it against what it
has built.

**A mid-flight split message is a filing act** (ruling 7). A
worker that finds its issue too big for one PR finishes the
first, self-contained PR and messages the remainder here as
pullable units. File each, insert it into the lane, and
**re-point the issue's dependents' blocking edges to the chain's
tail**, so the operator's Done on the first issue does not
release work that needs the whole. The re-point is this
session's write, never the worker's.

**Dispatching is the other half; Todo is its standing
authorization.** The gate is two checks: state Todo
(`linear_gate.py approvals ENG-<n>…`, exit 1 if any is not; In
Progress or In Review is a resume) and unblocked (Next, no
pending hold: step 2). An open
thread on a Todo issue is no refusal — only the operator makes
that move (2026-10-21); the probe's count feeds a later fold.
Dispatch unblocked Todo as slots open under the fleet ceiling, in
file-disjoint sets, with no per-dispatch ask. Moving an issue
back to Backlog is the operator's "not now".

```sh
python3 .claude/tools/session_dispatch.py task 1234
```

It opens a **tab in this session's own window** (operator ruling,
2026-09-10) and types the verb. Several verbs go in one call,
separated by a bare `+`:

```sh
python3 .claude/tools/session_dispatch.py task 1234 + plan
```

It prints one `verb -> tty` line each, then a roll-call of open
sessions — the roll-call is the confirmation to relay.

A **minor non-repo job** (a key file, a local deploy, a one-off
probe) dispatches as **`chore <n>`**, not `task <n>`: the helper
tier, no PR, In Review at handoff.

- **The substrate stays at the call site** — `task anthropic 1234`
  for work needing web research; see
  `docs/conventions/local-integrations.md`.
- **Best effort.** If iTerm's Python API or Automation access is
  off, the tool fails loudly and prints the verbs — relay them
  rather than retrying. `--dry-run` shows them.
- **A batch is all-or-nothing on validation** — fix the typo and
  re-run the whole call.
- **An `architect` kick-off is this fixed text, never a
  paraphrase:** "The read-back is mandatory: file tasks into
  Backlog as the design settles, move to Todo only those the
  operator ratifies, and open your handoff with where and when
  they did." Never call the read-back optional. A handoff lacking
  that line is unratified: its tasks stay in Backlog for review.
- **A ratified spec FILE is folded in first** (usually the
  design landed as tasks, and there is none): an implementer
  cannot read an architect or explore worktree, so **copy it into the
  implementing issue's body, then delete the file.** One still in
  its read-back is live work.

**6. Write back — incrementally, then rewritten at the gate.**
Write after each major decision: planning sessions are often
closed abruptly, and an unwritten decision is lost. A session
that decides without writing back has dropped its main
deliverable — the board shows *what*, only the document carries
*why*.

**Gate the close-out; don't wait to be told.** When the operator
signals wrap-up, or the conversation lulls after significant
decisions, offer via `AskUserQuestion` — "close out now?
(rewrites the Planning doc to the current state of play)" —
recommended first.

**The close-out deliverable is the whole document, rewritten**
to the ephemeral shape above and nothing else: superseded
phases, dated addenda and the session's incremental notes
disappear, and no ruling, doctrine or method text survives
outside `In-session notes`. **Restore `Discuss with operator`
to a single blank checkbox**, once its items are acted on, and
**bring `Operator actions outstanding` current**, so the next
bootstrap surfaces something true. **Read the document back**
after the write: a write against a stale snapshot silently saves
the stale base. A wholesale replacement re-parses on the round
trip, so compose it:

- **without inline code spans** — plain names, not backticked;
- with **no emphasis span crossing a newline**;
- with **no machine-parsed field starting with a bare
  hostname-valid `name.ext`**.

These rules live here only; the document's one-line header
points at this skill rather than restating them.

**The close-out carries the audit-state table forward — and
writes no directive** (why: `history.md`). The table is
state-shaped: a row is **replaced** when its unit is audited
again, so its size is bounded by the registry, and the board
holds the history. A table growing rows per audit has become the
log it exists to avoid. **Update the rows for whatever landed
this cycle** — an audit issue pulled and completed sets its
unit's date, count and pointer; one filed but unpulled changes
nothing, since the row records what was audited, not scheduled.
**Carry the upkeep report forward verbatim** — its heading
through its `End of upkeep report.` line: the tool splices only
between the two and refuses a section whose end line is gone.

**7. Capture the session's token profile at close-out.** A
planning session is a lever producer too. **Compute its id; never
list the projects directory** (`history.md`):

```sh
python3 .claude/tools/resolve_session.py --daily-id plan
```

```sh
make session-metrics SESSION=<uuid>
```

It reads the cycle counter itself; pass `--date YYYYMMDD` (and
`--cycle N`) for an earlier session. Levers file through
`.claude/tools/trim_levers.py` (probe, then `file` or
`append-evidence`); `/session-metrics` does this for you.

**A planning-derived lever must be quality-preserving.** Fidelity
is the point and is never traded for tokens: a cheaper fetch, a
narrower echo, a hoisted survey or a removed re-read qualifies; a
model downgrade, a skipped bootstrap read, or anything that thins
the decision context does not. Such a lever is **filed and
immediately closed with its reason** (or left closed if one is
already parked), so the fingerprint probe stops the next fold
re-deriving it.

**8. Promote the parked findings, and ask once.** `audit-scope`
files confirmed findings under the **Audit findings** milestone:
parked, searchable, costing a bootstrap nothing. This step is how
planning absorbs them, and the **`Follow-ups` milestone** too, so
`review-pr`'s deferred items never sit indefinitely.

**At bootstrap, say each count, never a listing.** Promotion is
planning's own write, made when the inbox needs stocking; the
operator reviews each promoted issue there. **The consolidated
ask** carries only what needs the operator's word — the meta kill
list, the holds' edges, Discuss items and the upkeep's destructive
items. To promote:

- List the milestone **field-selected** (title, priority).
- **Select a parallelizable batch, using the collision
  clusters.** Promoted issues are worked simultaneously, so
  promote a set whose members share **no common cluster**. Where
  two collide, promote one and park the other, or fold them under
  the fewest-coherent-PRs rule. This is the selection procedure,
  not advice.
- **Sequence against the live tracks**: a finding touching a
  mid-flight track's files rides with it or waits.
- Fetch a body **only** for a finding being promoted.
- **Promote by clearing the milestone** in one
  `board_batch.py fields` write. Never close it and file a fresh
  copy.

**Do not re-adjudicate a finding.** `audit-scope` cross-checks
adversarially before filing; the only question left is whether it
belongs in this phase.

## Notes

- **No source edits.** This skill writes to Linear and the
  Planning document; it authors no diff and commits nothing.
- **It is not a one-shot** — a planning session spans hours or
  days.
- **The board is this session's alone.** No other skill analyses
  it, and none folds or merges the meta pool — the filing skills
  only feed it — so two can't reach conflicting conclusions: one
  folder, one rhythm (`history.md`).
