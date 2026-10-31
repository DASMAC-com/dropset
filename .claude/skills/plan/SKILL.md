---
name: plan
description: Run a scheduler session — board work, the complement to a worktree implementation session, in the base repo and never a worktree. Warm-boots from the state of play the curator's start-of-day bootstrap writes to the "Planning" Linear document (`LINEAR_PLANNING_DOC_ID`) — no board read, no audit heartbeat — and holds the fleet resume and first wave for the operator's start trigger in its one ask. Then keeps the board coherent: Todo dispatched as standing authorization once the day is started, blocking edges curated by a human, file collisions reconciled by reading, the inbox kept stocked with staged specs, issues filed and amended to house convention, and architect or explore specs folded into their implementing issues. Writes decisions back to the Planning document as it goes and rewrites it at close-out.
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
document is **not** an append log; the curator's bootstrap
rewrite and step 6's close-out each produce exactly these
headings, in this order, and nothing else:

1. **Discuss with operator** — a single blank checkbox the
   operator fills before a session (Enter in Linear adds the
   next).
1. **Current direction and checkpoints** — the phase, the live
   tracks, and the next bootstrap's first moves.
1. **Fleet state and chains** — in flight, in review, queued,
   held with reasons, and the parked counts.
1. **Collision clusters** — the curator's first cut, revised here
   (step 1).
1. **Audit state** — the bounded table, the curator's heartbeat
   (`curate` → "Bootstrap: the state of play").
1. **Session-metrics state** (step 7).
1. **In-session notes** — this session's incremental writes;
   after a close-out, only the rulings not yet landed in
   `rulings.md`.
1. **Notes for the next planning session** — the only heading
   other sessions write under. The upkeep tool's machine-written
   report is its level-2 subsection, carried verbatim.

Above them, a header of one line — which also states the
ownership: the curator's bootstrap rewrite, then the scheduler's
every write after it — plus the `Bootstrap stamp` and, once the
day is started, the `Fleet start:` line (step 5).

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

- **Every bootstrap is warm.** The curator's start-of-day
  bootstrap writes the `Bootstrap stamp: <YYYY-MM-DD>` line with
  the state of play, and every close-out rewrite keeps it, so a
  first launch and a cycle relaunch boot alike: step 1. A relaunch
  asks the start trigger alone, and only when the document carries
  no `Fleet start:` line for today or `ListAgents` shows no worker
  session (`task`, `chore`, `explore`) live (a machine restart;
  step 5). That `ListAgents` read finding no `curate-<day>` while
  the board holds `Curator pending` issues means the curator
  stopped mid-day: say it needs relaunching.

## Steps

**1. Warm-boot from the state of play.** The start-of-day
bootstrap — the landing check, the board read, Next and the
holds, the parked counts, the audit heartbeat, the first cut of
the collision clusters and the wholesale rewrite — is the
**curator's** (`curate` → "Bootstrap: the state of play"; ruling
2), so this session reads the result and keeps the live day. Read
[`rulings.md`](rulings.md) first — the rules this session obeys.
Then read the Planning document with
`python3 .claude/tools/planning_doc.py --out <scratch>`,
which spills it and prints only the heading map, and slice it by
`--section`. **The document and `rulings.md` are each read
once**; a later check is a `--section` read, never the whole body
in context a second time.

**No `Bootstrap stamp` of today** means the curator's write has not
landed. Run `ListAgents`: with a live `curate-<day>`, say
`waiting on curate-<day>'s state of play` and end the turn; its
`state of play written` resumes this session, which spills the
document again — the one licensed second read, since the first
held yesterday's. No curator live: tell the operator to type
`curate` in a new tab. **The start trigger waits for the write**;
nothing is dispatched before it.

**No board read, no PR read, no audit heartbeat, no parked
counts** at boot: each is in the curator's write — the fleet
state, the counts, the clusters, the heartbeat's decision, the
holds pending the operator's tick in the curator's tab. Then:

- **Act on each `Discuss with operator` item first** — one
  needing the operator's word joins step 8's ask.
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
- **Release chains** the state of play names as landed.

**Say the fold line.** The lever fold runs in the launcher's own
tab (`launch`), never here:

```sh
python3 .claude/tools/upkeep.py fold-today
```

Say its line verbatim — `fold ran at HH:MM, N batches` or
`no fold today`; `no fold today` with a live `launch-<day>` means
not yet stamped, likely still folding: say that, not "no fold".
The new batches reach the inbox, where the curator reviews them.

**The boot runs to its one ask.** Then stop at step 8's
consolidated ask. **`fleet resume` and every dispatch wait for that
ask's start trigger** (ruling: fleet doctrine 1). A Discuss item
or duty needing the operator's word joins that ask. A question
open in the curator's tab never holds the trigger or a dispatch.

**Which tier an issue sits in is the whole scoping signal:**

- **Backlog is the review inbox** — specs awaiting the
  operator's review. Priority there is review order.
- **Todo is the approved queue** — the operator's move to Todo is
  standing authorization to dispatch (step 5); priority there is
  advisory. Planning never moves an issue to Todo.
- **A project milestone means parked** — Backlog plus a parking
  milestone, not this phase. Out of scope unless the operator
  names it.

Pull a body or a listing only when a decision turns on it; the
board read itself is the curator's.

**The fold protocol**, which every reviser reuses: fold the
comment into the body, reply saying how it was addressed, then
resolve the thread (`linear_gate.py resolve <comment-id>`).
Resolved means addressed-and-how, never approval. Never fold an
In Progress spec; a fold into an approved (Todo) body moves it
back to Backlog, priority and edges unchanged.

The scheduler still keeps the inbox stocked: when it runs thin,
stage more specs before anything else. Todo is dispatched by step
5, after the start trigger.

**Bound the Planning document.** It grows between rewrites: other
sessions append, and an abrupt close skips the close-out. So:

- **The bootstrap rewrite is the curator's**, and sweeps foreign
  and unconsolidated notes; this session writes everything after
  it — one write each, which the document's header line states.
- **This session's incremental notes** live under one
  `In-session notes` heading, so a rewrite sweeps one section.
- **Sessions other than the scheduler and the curator's bootstrap
  append only under** `Notes for the next planning session` —
  stated wherever those sessions are told to write here.

**Reconcile file collisions as you dispatch — by reading.** The
curator's bootstrap records a **first cut** of the collision
clusters per `docs/conventions/linear-automation.md` → "Overlap is
a cluster, never an ordering" — per shared area, never by
connected component, never as an ordering, as prose, never a
`related` link. The automated machinery and the `**Touches**:`
field are retired. **Revise the cut** from the prose you hold as
each wave is chosen; the revision lands at the next write-back.
Step 2 uses the clusters for merge proposals, step 5 for
file-disjoint waves.

**2. Keep the Queue honest.** The Queue — the unblocked Todo,
which the operator calls **"Next"** — is the safe-to-start set:
an implementation session pulls from it and runs to completion
without re-planning, so every member must be startable *now*,
with no refactor-later burden a later issue will undo.

- An issue is in Next **iff** it is in Todo with no live
  blocking edge. "Put it in Next" means resolving or re-scoping
  what blocks it — **never** silently dropping a real dependency.
- **Priorities and edges are the scheduler's and the curator's**
  (ruling 2); implementation sessions never place edges, and a
  filer only proposes a priority on its own tasks at filing.
  Priorities move under the **agreement rule**
  (`docs/conventions/linear-automation.md` → "Priorities: shared
  authority, the agreement rule"), which governs the messaging,
  the counter-write and the escalation to the operator; follow it
  there. A filer's one-line priority message is confirmed by
  silence or rewritten under the same rule. The scheduler still
  owns dispatch order as read: a wave dispatches on the priorities
  as they stand at that moment, and the next write-back records the
  result. Message the curator every direction change, and any write
  touching an edge it placed, with the reason, in the same turn.
- **A hold is an edge or it does not exist** (ruling 2). An issue
  held behind an in-flight surface or a sequencing reason gets a
  blocking edge on that blocker, proposed in the curator's first
  ask at bootstrap or asked here as one question when found
  mid-session, so it
  shows as Blocked, not Next; a row awaiting that answer is not
  dispatched. A completed blocker stops blocking on its own;
  removing a hold edge on a re-sequence is the operator's call
  too, asked the same way and run through step 3.
- **A hold message to a reviewed, green PR says park.** Tell the
  worker to append its resume brief and end the session with
  `close_session.py --park` (`review-pr` step 20), not to wait
  resident; dispatch the resume
  once the PR it waits on lands, as a fresh session.

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

**Size each filed half by its parts, never as a fraction of the
parent.** "Each half about 400 lines" is checked by nothing before
dispatch. A write-path half is its transaction builder, status
hook, UI wiring and tests, so sum those. When a half's honest sum
passes the band, split it at the seam before filing (a shared
helper plus a pure module, then the wiring). One half filed at
≈400 landed at ≈1,300 and 154 turns, and a split by then would
have shipped inert code.

**Dispatching is the other half; Todo is its standing
authorization, once the day is started.** The start is the
operator's: `fleet resume` and the first wave wait for step 8's
start trigger, then `fleet resume` runs first and the wave after
it. On "start now and dispatch at will" the day runs with no
per-wave ask; on "this wave only" each later wave is one ask; on
"not yet" nothing is dispatched and the curator runs regardless,
until the operator's word in this tab starts the day. **Record the
answer** as one `Fleet start: <YYYY-MM-DD>, at will` (or
`, per wave`) line beside the bootstrap stamp, kept by every
close-out rewrite, so a cycled session knows the day is started
and in which mode. A curator tick moving an issue to Todo before
the trigger is dispatchable at the trigger like any Todo. A warm
bootstrap finding today's line, with the fleet live, asks no
trigger; one finding no line, or no worker session live (a machine
restart mid-day), asks it again. The gate is two checks: state Todo
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

**8. Drain the legacy findings, and ask once.** The counts are
the curator's, said in its tab and read here from the state of
play — never re-listed. **The consolidated ask** carries only what
needs the operator's word, **led by the start trigger** (skipped
only when step 5's `Fleet start:` line for today is present and
the fleet is live): "ready to resume the fleet (N sessions) and
dispatch the first wave (the file-disjoint set, named)?", answered
**"Start now and dispatch at will today (Recommended)"**, **"Start
now, this wave only; ask before later waves"** or **"Not yet"**,
the question naming the count of Todo `[Operator]` items the state
of play records, which the curator carries one per batch until
Done (step 5 runs each answer). Then the `Audit findings` kill
list and Discuss items; the holds' edges are the curator's first
ask, never this one.

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
  merge), bootstraps the day and rules on rebases; the scheduler
  schedules, keeps the live day and owns the document after the
  bootstrap; both hold priorities under the agreement rule; each
  messages the other every priority write and any edge write
  touching what the other placed. A curator rebase ruling that
  changes an in-flight issue's scope arrives as a message: re-plan
  around it like any re-scope (step 5).
