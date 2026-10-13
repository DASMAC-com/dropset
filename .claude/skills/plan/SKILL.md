---
name: plan
description: Run a planning session — board work, the complement to a worktree implementation session, in the base repo and never a worktree. Bootstraps from the "Planning" Linear document (`LINEAR_PLANNING_DOC_ID`), surfaces the Todo umbrellas, runs the audit heartbeat (file an audit issue or decline with a recorded reason), and folds the parked `Claude meta` pool into small themed edge-free batches, promoting a file-disjoint subset. Then keeps the board coherent: the Queue honest, blocking edges curated by a human, file collisions reconciled by reading, parked findings offered for promotion, issues filed and amended to house convention, and architect or explore specs folded into their implementing issues. Writes decisions back to the Planning document as it goes and rewrites it at close-out.
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

This file carries only the **method**; the phase, standing
decisions and sequencing live in the document and change weekly.
Work items live in issues, never in the document, and the
document is **not** an append log: it carries the current state
of play, kept so by step 6's close-out rewrite.

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

  On `"due": true` (a 250k prefix or 40 inbound messages), ask
  once: cycle now? — yes recommended. On no, run
  `plan_cycle.py snooze` (+50k tokens / +15 messages).

- **The procedure.** A one-line FYI to every live worker with an
  unanswered consult — planning is cycling, re-send anything
  unanswered; then step 6's close-out rewrite, echo verified;
  then `plan_cycle.py cycle`, which leaves the marker and ends
  the client.

- **The warm bootstrap.** A full bootstrap writes a
  `Bootstrap stamp: <YYYY-MM-DD>` line to the document, and every
  close-out rewrite keeps it. A bootstrap finding today's date
  there is **warm**: read the document, run `ListAgents`, surface
  the outstanding list — and skip the once-a-day duties (the meta
  fold, the audit heartbeat, the parked counts, step 8). No stamp
  means a full bootstrap.

## Steps

**1. Bootstrap, and surface the umbrellas unprompted.** Fetch the
Planning document with `mcp__claude_ai_Linear__get_document` and
read it first — it is the last session's handoff.

Then read the board. **Which tier an issue sits in is the whole
scoping signal:**

- **Backlog is the pullable set** — the Queue (step 2). A
  genuinely startable work item belongs here and nowhere else.
- **Todo holds the live tracks** — umbrellas and initiatives,
  parents that accrue children. Not a staging area: a work item
  stranded in Todo is invisible to the operator.
- **A project milestone means parked** — not this phase. Out of
  scope for a bootstrap read unless the operator names it.
  Structure is no proxy for this: child count was considered and
  rejected (`history.md`).

Linear's list API has no milestone filter, so the drop is
client-side. Two calls, since `list` defaults to Backlog:

```sh
python3 .claude/tools/board_batch.py list
```

```sh
python3 .claude/tools/board_batch.py list --state Todo \
  --include-milestoned --show-milestone
```

The Todo call keeps parked issues, tagged, for the meta review
below. Pull a full body only when a decision turns on it. Every
un-milestoned Todo item is either **acted on** or **parked** —
say that discipline out loud when the set has grown.

**Then say the umbrellas out loud, unasked** — "these tracks are
open: …" — and **fire step 8's parked-findings offer now**, with
them; it is numbered 8 because `audit-scope` cites it so.

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
authorizes, fan-out included; findings file parked (`Todo` plus
`Audit findings`).

It dispatches as **`explore <tag>`, never `task <tag>`**
(operator rule, 2026-09-11) — a temporary worktree, no PR; see
`docs/conventions/local-integrations.md` → "Architect and explore
sessions: temporary worktree, no PR". **Name the verb in the
sequencing prose**, since a later puller reads only the body. An
audit sits **In Progress** until its findings are ratified and
folded, so an old In Progress audit is live, not stalled.

**Fold the `Claude meta` pool into SMALL THEMED batches — by
default, unprompted.** Every `Claude:` filing lands parked (`Todo`
plus `Claude meta`). The pool is that milestone **plus every
open, unblocked, unpulled `Claude:` Backlog issue** (not In
Progress or In Review) — without that clause an unpulled batch is
duplicated rather than absorbed. Group by the surface each issue
changes and fold each group into its lowest-numbered survivor via
`/merge-tasks`. Routine bookkeeping; no per-fold proposal.

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

- **Fold strays, promote batches.** An already-assembled parked
  batch is a promotion candidate, never a fold input —
  `/merge-tasks` cannot split, so a re-fold only grows it.

- **Survivors file parked**; only the ones you promote go to
  Backlog, Urgent. Assembling and promoting are two acts.

- **Nothing serializes batches** — no edge, no in-flight gate.
  Several may be open at once; their file contention is paid as
  a rebase. What bounds Next is promoting only a
  **file-disjoint** set (step 8's selection procedure, on the
  collision clusters), never a state check or a count.

- **Assembly consumes the pool as of this bootstrap.** A later
  stray waits for the next one.

- **Never fold an issue that is In Progress** — that mutates a
  spec a session is implementing. **Never fold across the meta /
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
   edge and is never slated.
1. Mark each group **free**, or **in flight** when an In Progress
   / In Review meta issue (`list --state`) already rewrites that
   surface.
1. Offer the slate (from the free groups) and the kill list in
   step 8's `AskUserQuestion`, recommended set first.
1. On a yes, promote each with both halves in one
   `board_batch.py fields` write (milestone `null`, Todo →
   Backlog, Urgent), and close each kill `Canceled` with its
   reason, as `trim-context` step 6 does.
1. Record promoted, killed, blocked, each held group with why,
   and the parked count under the `In-session notes` heading.

**Promotion is always BOTH halves** — clear the milestone *and*
move Todo → Backlog. One half alone leaves an issue parked and
pullable at once and corrupts the parked count.

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

The whole board read costs ~2–3k tokens, against ~3k for one
field-change echo — narrowing the read buys judgment, not tokens;
the token lever is batching writes (step 4).

**2. Keep the Queue honest.** The Queue — the unblocked Backlog,
which the operator calls **"Next"** — is the safe-to-start set:
an implementation session pulls from it and runs to completion
without re-planning, so every member must be startable *now*,
with no refactor-later burden a later issue will undo.

- An issue is in Next **iff** it is in Backlog with no live
  blocking edge. "Put it in Next" means resolving or re-scoping
  what blocks it — **never** silently dropping a real dependency.
- **Priority within Next is the pull order** — the lever, not a
  label.
- **Edges and priorities are curated here**; implementation
  sessions never place edges.
- **`Claude:` meta-work is Urgent once unblocked** (operator
  rule, 2026-08-17), so agent-infra is the next pull. Meta lands
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
Edges mean **semantic rollout ordering** and are human-curated
end to end (`docs/conventions/linear-automation.md` → "Blocking
relations"). Place the operator's decided edges in one batch:

```sh
python3 .claude/tools/board_batch.py edges --pairs <file>
```

`--remove` retires one. **Rehearse with `--dry-run` first** — a
wrong edge drops an issue out of the available set. The
subcommand takes an explicit pair list, has no discovery mode and
refuses an empty one: it executes a human decision, never derives
one.

Not edges: **file overlap** (a collision cluster — it costs a
rebase, not an ordering); **coupling that belongs in one PR**
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

- **Fewest coherent PRs, each sized to a SHORT session** — never
  across apps, languages or deploy units, and a coherent set
  larger than a short session splits into sequential PRs of
  roughly 4–5 findings. The `Claude:` class is bound too
  (operator rule, 2026-09-11; `history.md`), so `merge-tasks`
  warns on its oversized survivors as well.

- **State a scope posture when staging an issue** — **hold**
  (build what is specified), **expand** (under-scoped; here is
  what is missing), or **cut** (over-scoped; here is the smaller
  thing). Default it from context and **say which you took**: an
  unstated posture is hold by inertia, which silently endorses
  whatever the filer wrote. It is one issue at staging time — the
  whether-to-build-it question is `architect`'s.

- **Todo / Backlog split** — tracks in Todo, pullable work in
  Backlog (step 1).

- **`Claude:` prefix** on meta-work titles (every edited path
  under `.claude/**`, `CLAUDE.md`, `docs/conventions/**` or
  `cfg/**`). Each such filing lands **parked** (`Todo` plus
  `Claude meta`) in the creating call; only a promoted batch goes
  to Backlog.

- **The `**Fingerprint**:` line is mandatory** — the dedup key,
  its first token a **dotless domain token**, never a bare
  `name.ext` (Linear linkifies it). It is the only structured
  field. See `CLAUDE.md` → "Structured filing fields".

- **Field-only writes go through the tool**, never `save_issue`,
  which echoes the whole body for each (one session paid ≈40k for
  17 field-only writes):

  ```sh
  python3 .claude/tools/board_batch.py fields --updates <file>
  ```

  Body edits stay on the MCP `patch` path
  (`docs/conventions/linear-automation.md`).

- **Buffer folds onto one issue; write them once.** Each echo
  returns the whole body, so a survivor costs most exactly when
  it is folded into most (one session: 97 `save_issue` calls,
  ≈164k, its five costliest results all late folds onto one
  survivor). Accumulate several folds and an amendment into a
  **single** `patch` call (up to 50 ops). See
  `docs/conventions/linear-automation.md` → "Partial edits".

- **File no relations.** Overlap is step 1's prose; edges are
  step 3's.

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

**Dispatching is the other half.** When a task is unblocked and
ready, offer it ("1234 is unblocked; start it?") and on the
operator's yes:

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

- **The ask-then-yes IS the authorization**, as for `fleet go`;
  the tool never dispatches unasked.
- **The substrate stays at the call site** — `task anthropic 1234`
  for work needing web research; see
  `docs/conventions/local-integrations.md`.
- **Best effort.** If iTerm's Python API or Automation access is
  off, the tool fails loudly and prints the verbs — relay them
  rather than retrying. `--dry-run` shows them.
- **A batch is all-or-nothing on validation** — fix the typo and
  re-run the whole call.
- **A ratified spec FILE is folded in first.** `architect` and
  `explore` sessions are read-only toward the repo, and usually
  their design landed as tasks, so there is nothing to fold. When
  one iterated a spec file, it sits untracked in that session's
  temporary worktree, which a dispatched implementer cannot read:
  **copy it into the implementing issue's body, then delete the
  file.** No PR to close, no branch to delete. Fold only a spec
  its close-out note reports as **ratified**; one still in its
  read-back loop is live work.

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

**The close-out deliverable is the whole document, rewritten**,
not an entry appended: superseded phases, dated addenda and the
session's incremental notes disappear. A wholesale replacement
re-parses on the round trip, so compose it:

- **without inline code spans** — plain names, not backticked;
- with **no emphasis span crossing a newline**;
- with **no machine-parsed field starting with a bare
  hostname-valid `name.ext`**.

The document's header paragraph states this convention; **keep
the two consistent** when editing either.

**The close-out carries the audit-state table forward — and
writes no directive** (why: `history.md`). The table is
state-shaped: a row is **replaced** when its unit is audited
again, so its size is bounded by the registry, and the board
holds the history. A table growing rows per audit has become the
log it exists to avoid. **Update the rows for whatever landed
this cycle** — an audit issue pulled and completed sets its
unit's date, count and pointer; one filed but unpulled changes
nothing, since the row records what was audited, not scheduled.

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

**8. Offer the parked audit findings — don't wait to be asked.**
`audit-scope` files confirmed findings under the **Audit
findings** milestone: parked, searchable, costing a bootstrap
nothing. This step is how planning absorbs them.

**At bootstrap, a count and a prompt, never a listing** — "N
audit findings are parked — slate any in?", asked with step 1's
meta slate and kill list. On a yes:

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
- **Promote with both halves** — clear the milestone and move
  Todo → Backlog — in one `board_batch.py fields` write. Never
  close it and file a fresh copy.

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
