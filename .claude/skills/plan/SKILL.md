---
name: plan
description: Run a scheduler session — board work, the complement to a worktree implementation session, in the base repo and never a worktree. Bootstraps from the "Planning" Linear document (`LINEAR_PLANNING_DOC_ID`), runs the audit heartbeat (file an audit issue or decline with a recorded reason), and messages the launcher once the state of play is written, so it starts the curator on the Backlog review inbox. Then keeps the board coherent: Todo dispatched as standing authorization, blocking edges curated by a human, file collisions reconciled by reading, the inbox kept stocked with staged specs, issues filed and amended to house convention, and architect or explore specs folded into their implementing issues. Writes decisions back to the Planning document as it goes and rewrites it at close-out.
user-invocable: true
model: fable
---

<!-- cspell:word startable -->

<!-- cspell:word backticked -->

# `plan`

Implementation sessions run one deterministic spec to completion
in a worktree (`/init-pr` → build → `/review-pr`). Scheduler
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
   tracks, and the next bootstrap's first moves.
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
a PR to `rulings.md`, and carry it under `In-session notes` until
it lands.

## Where it runs

The **base repo**, never a worktree. Start or resume with a bare
`plan` — idempotent; it creates or resumes today's
`plan-<day-of-month>` on the advisor tier's model, with `/plan`
as a new session's prompt, on Bedrock; `plan anthropic` is the
seat override, and a reopen keeps the substrate its session
recorded. There is no in-session model check: the `plan` helper pins
the model and refuses an unresolved tier. Verbs:
`docs/conventions/local-integrations.md`.

## Cycling

The hub's prefix only grows, so the session **cycles**: close
out, exit, and the `plan` helper relaunches it in the same tab
under a fresh id and the same name. Compaction is a manual
operator lever, never this mechanism.

- **The operator types `cycle`** → run the procedure now, no ask.

- **The trigger.** At each idle point — turn done, nothing
  queued, no consult half-answered — run:

  ```sh
  python3 .claude/tools/plan_cycle.py check
  ```

  `due` fires on dollars (250k at the advisor read rate; tokens
  if unpriced). On `"due": true`, ask once: cycle now? — yes
  recommended. On no, run `plan_cycle.py snooze`.

- **The procedure.** A one-line FYI to every live worker with an
  unanswered consult — planning is cycling, re-send anything
  unanswered; then step 6's close-out rewrite, echo verified;
  then `plan_cycle.py cycle`, which leaves the marker and ends
  the client.

- **The warm bootstrap.** A full bootstrap writes a
  `Bootstrap stamp: <YYYY-MM-DD>` line to the document, and every
  close-out rewrite keeps it. A bootstrap finding today's date
  there is **warm**: read `rulings.md` and the document (step 1's
  spill and slice), run
  `ListAgents` — and skip the once-a-day duties (the fold line,
  the audit heartbeat, the parked counts, the upkeep report, the
  launcher's message, step 8). No stamp
  means a full bootstrap.

## Steps

**1. Bootstrap.** Read
[`rulings.md`](rulings.md) first — the rules this session obeys.
Then read the Planning document — the last session's handoff —
with `python3 .claude/tools/planning_doc.py --out <scratch>`,
which spills it and prints only the heading map, and slice it by
`--section`. **The document and `rulings.md` are each read
once**; a later check is a `--section` read, never a second whole
fetch. Then, warm or full:

- **Act on each `Discuss with operator` item first**, before the
  board — one needing the operator's word joins step 8's ask.
- **Recite no operator list.** An operator action is asked for
  **when it binds**, in the tab that needs it, through
  `AskUserQuestion` — the scheduler for a Done that releases an
  edge or a login the gate needs (during a bootstrap, joining
  step 8's ask), a worker for a ratification, the curator for a
  kill. One that cannot be done in the moment is filed at Urgent
  in Backlog with no milestone, and the curator surfaces it
  once: `[Operator]`, which Todo holds until the operator's Done,
  or a chore when a session can do the surrounding work, which
  runs to In Review like any chore. There is no third path.
- **Verify overnight landings with `git log`** and the In Review
  listing, never the Done column; release chains as blockers land.

**Say the upkeep report's highlights, with the parked counts.**
The pre-launch ran `.claude/tools/upkeep.py` before this
session started and spliced its report under the document's
machine-written `Upkeep report` heading, so the fetch above holds
it; never re-run the pass here. Say its `parked` line — each
pool's count and direction (ruling 23) — what was pruned and dismissed
(or would have been, unarmed), each refusal (a live session
lock, an open PR — reported, never retried) and each pruned
name's ahead count, and any inert guard,
dangling reference or file near its size cap. Say the
`fleet-cost` day lines too, any flagged day first. A run stamp
older than today means the pass did not reach the document —
say so. Its **destructive items** (the prune, the transcript
purge, allowlist cruft, refresh candidates, stale memories) are
the launcher's one ask (ruling 16) — never asked here.

**Say the fold line.** The lever fold runs in the launcher's own
tab (`launch`), alongside this bootstrap, never here:

```sh
python3 .claude/tools/upkeep.py fold-today
```

Say its line verbatim — `fold ran at HH:MM, N batches` or
`no fold today`. The fold runs alongside this bootstrap, so
`no fold today` with a live `launch-<day>` means not yet stamped,
likely still folding: say that, not "no fold". The new batches
reach the inbox, where the curator reviews them.

**The bootstrap runs unattended until its one ask.** Do every duty
that asks nothing — `fleet resume`, the overnight landing check,
the board read, the Queue's honesty — then stop at step 8's
consolidated ask. A Discuss item or duty needing the operator's
word joins that ask.

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

The first keeps parked issues, tagged, for step 8's counts.
**Say three counts**: the inbox (untagged Backlog), its Urgent
subset, and Todo. Pull a body only when a decision turns on it.

**The fold protocol**, which every reviser reuses: fold the
comment into the body, reply saying how it was addressed, then
resolve the thread (`linear_gate.py resolve <comment-id>`).
Resolved means addressed-and-how, never approval. Never fold an
In Progress spec; a fold into an approved (Todo) body moves it
back to Backlog, priority and edges unchanged.

**The curator reviews the inbox; the launcher dispatches it.**
After step 8's answer (or straight away when it has nothing to
ask) and the bootstrap's write-back of the state of play, run
`ListAgents` and message the live `launch-<day>` exactly
`state of play written`, once — its cue to dispatch the curator
with the fold's hand-off. No launcher live: say
`launcher: not live`, tell the operator to type `launch` in a new
tab, and dispatch no curator from here. Todo is
dispatched as ever, by step 5 after the Next read below. The scheduler
still keeps the inbox stocked: when it runs thin, stage more
specs before anything else.

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
authorizes, fan-out included; findings are ratified in its tab
(`ratify-staged`).

**Title it `[Explore/Bedrock]`** (`[Explore/Anthropic]` when the
audit needs the web): it runs as `explore`, never `task` (operator
rule, 2026-09-11) — a temporary worktree, no PR; see
`docs/conventions/local-integrations.md` → "Architect and explore
sessions: temporary worktree, no PR". An
audit sits **In Progress** until its findings are ratified and
folded, so an old In Progress audit is live, not stalled.

**Bound the Planning document.** It grows between close-out
rewrites: other sessions append, and an abrupt close skips the
rewrite. So:

- **If it carries foreign notes or a prior session's
  unconsolidated notes, the bootstrap's first write is the
  wholesale rewrite**, not another append.
- **This session's incremental notes** live under one
  `In-session notes` heading, so a rewrite sweeps one section.
- **Sessions other than the scheduler append only under**
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
- **Priorities are the scheduler's alone; edges are the
  scheduler's and the curator's** (ruling 2); implementation
  sessions never place edges. The curator messages a priority it
  believes wrong. Message the curator every direction change, and
  any write touching an edge it placed, with the reason, in the
  same turn.
- **A hold is an edge or it does not exist** (ruling 2). An issue
  held behind an in-flight surface or a sequencing reason gets a
  blocking edge on that blocker, asked in step 8's ask at
  bootstrap or as one question when found mid-session, so it
  shows as Blocked, not Next; a row awaiting that answer is not
  dispatched. A completed blocker stops blocking on its own;
  removing a hold edge on a re-sequence is the operator's call
  too, asked the same way and run through step 3.

An issue that isn't startable gets reordered behind what it
depends on, re-scoped, or split so its first actionable unit is
its own task. "Startable if you also do X first" is a scheduling
bug.

**Propose merge groups aggressively**, to minimize open PRs:
Backlog clusters that would land as one PR (same subsystem,
crate, language-domain) fold via `/merge-tasks`. Step 1's
collision clusters are the raw material. The **coherence floor**
binds: never across separate apps, languages, or deploy units.

**3. Curate blocking edges.** Edges mean **semantic rollout
ordering** or **the scheduler's own dispatch hold** (step 2),
and are human-curated end to end
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
  never a point. A seat-bound issue (`architect`)
  states its seat and a token profile instead: dollar bands bind
  Bedrock workers only.

- **The Todo / Backlog split** (step 1), **the `Claude:` prefix**,
  and **the mandatory
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
release work that needs the whole. The re-point is the
scheduler's write, never the worker's; message the curator the
filing.

**Dispatching is the other half; Todo is its standing
authorization.** The gate is two checks: state Todo
(`linear_gate.py approvals ENG-<n>…`, exit 1 if any is not; In
Progress or In Review is a resume) and unblocked (Next, no
pending hold: step 2). An open
thread on a Todo issue is no refusal — only the operator makes
that move (2026-10-21); the probe's count feeds a later fold.
Dispatch unblocked Todo as the dispatcher's reading admits, in
file-disjoint sets judged here on the collision clusters, overlap
paid as a rebase, with no per-dispatch ask. Moving an issue back
to Backlog is the operator's "not now".

```sh
python3 .claude/tools/session_dispatch.py task 1234
```

It opens a **tab in this session's own window** (operator ruling,
2026-09-10) and types the verb. Several verbs go in one call,
separated by a bare `+`:

```sh
python3 .claude/tools/session_dispatch.py task 1234 + plan
```

It prints one `verb -> tty` line each, a `held` line per verb the
machine refused, then a roll-call of open sessions — the
roll-call and the `held` lines are the confirmation to relay; a
held verb rides the next dispatch, when a session hands off.

**Type the line the title token says** (`linear-automation.md` →
"The verb/substrate title token"): `[Chore/Bedrock]` on 1234 is
`chore bedrock 1234`, no token is `task 1234`. **Refuse `task <n>`**
on a token naming another verb, and dispatch nothing for
`[Operator]` — the session helpers' title guard refuses each of these too.
Until the title sweep, judge a token-less legacy issue by its body,
and fix its title before dispatching anything but `task <n>`.
File operator-attended agent work (a key file, a local deploy)
`[Chore/Bedrock]` — helper tier, no PR, In Review at handoff — and
an item the operator executes by hand `[Operator]`.

- **Best effort.** If iTerm's Python API or Automation access is
  off, the tool fails loudly and prints the verbs — relay them
  rather than retrying. `--dry-run` shows them.
- **A batch is all-or-nothing on validation** — fix the typo and
  re-run the whole call.
- **A kick-off wave or a fleet-wide notice is one turn.** Compose
  the text once, name the recipients from one `ListAgents` read,
  and send every `SendMessage` in the same turn — never one
  recipient per turn. The turn count is the cost; there is no
  broadcast tool, and typing into tabs is ruled out (it lands as
  operator input and can clobber a half-typed prompt).
- **An `architect` kick-off is this fixed text, never a
  paraphrase:** "Two stages: agree the spec and file nothing,
  then run `ratify-staged`; open your handoff with the task list
  and where it stands." Never call either stage optional. A
  handoff lacking that line is unratified. The dispatch gate is
  unchanged (state Todo plus unblocked); an architect's Todo
  moves are its `ratify-staged` batch.
- **A ratified spec FILE is folded in first** (usually the
  design landed as tasks, and there is none): an implementer
  cannot read an architect or explore worktree, so **copy it into the
  implementing issue's body, then delete the file.** One still in
  its read-back is live work.

**6. Write back — incrementally, then rewritten at the gate.**
Write after each major decision: scheduler sessions are often
closed abruptly, and an unwritten decision is lost. A session
that decides without writing back has dropped its main
deliverable — the board shows *what*, only the document carries
*why*.

**Gate the close-out; don't wait to be told.** When the operator
signals wrap-up, or the conversation lulls after significant
decisions, offer via `AskUserQuestion` — "close out now?
(rewrites the Planning doc to the current state of play)" —
recommended first.

**On the yes, stop the curator first.** `ListAgents`; a live
curator gets one `SendMessage`, "closing for the day", and its
`curate` stop tail ends in `CLEAR TO CLOSE`. Do not wait for an
answer. None live: report `curator: not live`.

**The close-out deliverable is the whole document, rewritten**
to the ephemeral shape above and nothing else: superseded
phases, dated addenda and the session's incremental notes
disappear, and no ruling, doctrine or method text survives
outside `In-session notes`. **Restore `Discuss with operator`
to a single blank checkbox**, once its items are acted on, and
carry no operator-actions list: an open operator action is an
`[Operator]` issue, never document text. **Read the document back**
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

**Dismiss the settled PRs' notifications at close-out**, one
bare call, so the day's merges leave no inbox behind them:

```sh
python3 .claude/tools/pr_notifications.py dismiss --settled --apply
```

It selects every thread whose PR is merged or closed, whatever
its issue's state, never an open PR's, unsubscribes from each
merged one so a later bot comment cannot re-create it (a closed
PR keeps its subscription, since a reopen must resurface), marks
each done, and prints counts, not threads; say its `summary` line in the
close-out report. A failure is that line, never a blocker.

**7. Run `/session-metrics` at close-out**, the shared tail,
before the final document write. Its id comes from
`resolve_session.py --daily-id plan` (`--date YYYYMMDD`,
`--cycle N` for an earlier one; `history.md`), and the rewrite
carries its `session_metrics.py --cost-line` output as one line
under `Session-metrics state`.

**A planning-derived lever must be quality-preserving.** Fidelity
is the point and is never traded for tokens: a cheaper fetch, a
narrower echo, a hoisted survey or a removed re-read qualifies; a
model downgrade, a skipped bootstrap read, or anything that thins
the decision context does not — the analyst rejects those.

**8. Drain the legacy findings, and ask once.** **At bootstrap,
say each count, never a listing.** **The consolidated ask**
carries only what needs the operator's word — the
`Audit findings` kill list, the holds' edges and Discuss items.

**`Audit findings` is a legacy pool, drained one finding at a
time**: each bootstrap promotes what it holds, or lists it for a
kill by ruling 3's necessity test, closing each `Canceled` with
its reason on the operator's yes; the milestone leaves the parking
set once empty. List
it **field-selected** (title, priority); promote by **clearing
the milestone** in one `board_batch.py fields` write, never by
closing and refiling. **Do not re-adjudicate a finding** —
`audit-scope` cross-checked it before filing.

**`Session pending` is never promoted here**: a count is a seat
session that died mid-review — offer to resume it.

## Notes

- **No source edits.** This skill writes to Linear and the
  Planning document; it authors no diff and commits nothing.
- **It is not a one-shot** — a scheduler session spans hours or
  days.
- **The board is shared with the curator alone**, under ruling
  2's coordination rule: the curator judges (promote, kill,
  merge), the scheduler schedules and owns priorities and the
  document; each messages the other any edge write touching what
  the other placed.
