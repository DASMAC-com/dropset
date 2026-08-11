---
name: trim-context
description: Mine the Linear "Session Metrics" inbox into a propose-only skill-improvement Backlog task — the consumer half of the `session-metrics` producer. Reads the inbox document live, synthesizes the trim levers that recur across sessions (a verbose build log, a whole-file Read where a slice would do, a repeated full-PR read, an inlined-diff fan-out), and files them as a single aggregated propose-only task — one bullet per lever, each with its own `**Fingerprint**:` line under a combined `**Touches**:` (so one mining pass yields one issue / one PR, not a batch to consolidate later). Dedups each lever against the open Backlog, appends to the open aggregated task rather than opening a second, then drains every consumed entry out of the doc — filing the task is what discharges it — recording each one's disposition in the drain history, so the inbox cannot grow unbounded. Never edits a skill or convention doc — filing a task is the proposal. Runs standalone or as `housekeeping`'s Session Metrics step.
disable-model-invocation: false
user-invocable: true
---

# `trim-context`

The **consumer** half of the context-economy feedback loop.
`session-metrics` is the producer: at the end of a session it appends
one dated entry to the Linear "Session Metrics" inbox document — the
measured token sinks plus tailored trim recommendations.
`trim-context` drains that inbox: it reads the unprocessed entries,
finds the trim levers that **recur** across sessions, and files them as
a **single aggregated propose-only** skill-improvement Backlog task —
one bullet per lever, so a mining pass yields one issue (one PR) rather
than a batch that has to be hand-consolidated later — then records each
consumed entry's disposition back into the doc.

This is the same job `housekeeping` used to do inline as its "Mine the
Session Metrics inbox" step; it now lives here as its own skill, and
`housekeeping`
delegates to it. It runs identically whether invoked standalone or by
`housekeeping` — there is **no** propose-only vs. apply split, because
filing a task *is* the proposal: this skill never
edits a skill or convention doc, so an unattended pass and a hand run do
exactly the same thing.

## Linear destination

This skill reads one inbox document and **files** Backlog tasks, so it
needs the inbox id plus the env-resolved filing destination (the same
one `linear-task` / `housekeeping` use). Resolve each variable with its
**own** bare `printenv` (one `Bash(printenv:*)` allow-rule covers them
all) — never a combined `printenv A B C`, which on macOS / BSD prints
only the first value:

```sh
printenv LINEAR_SESSION_METRICS_DOC_ID
printenv LINEAR_TEAM_ID
printenv LINEAR_PROJECT_ID
printenv LINEAR_ASSIGNEE_ID
```

If `LINEAR_SESSION_METRICS_DOC_ID` is empty, **no-op cleanly**: say the
inbox isn't configured and stop — don't guess an id. If any of the
filing-destination variables is empty, say so and stop before filing.

## Steps

**1. Read the inbox doc live.** Fetch it fresh with
`mcp__claude_ai_Linear__get_document` (id = the resolved
`LINEAR_SESSION_METRICS_DOC_ID`); never reuse a stale snapshot, since
`session-metrics` adds entries between runs. Collect every
**unprocessed** entry — an unchecked `- [ ]` with **no** disposition
note (a nested line beginning `✓ filed:` or `⚠ noted:`). Skip entries
that already carry one, so a repeat pass doesn't re-file.

Every entry you collect here will be **drained** in step 5 — filing the
task is what discharges it — so this read is also the list of what to
delete.

**Count them, and note the body's size**, for the step-6 report. The
asymmetry worth knowing: `session-metrics` appends with a `patch` op and
pays only the length of its entry (per
`docs/conventions/linear-automation.md` → "Partial edits — the `patch`
argument"), so it appends happily forever, while **mining** reads the
whole body every pass. That is what made an unbounded inbox this skill's
problem rather than the producer's — and why draining on file, rather
than on a threshold, is the fix.

**2. Synthesize across sessions, don't transcribe.** Look for the trim
levers that **recur** across the unprocessed entries — a verbose build
log inflating several runs, a whole-file Read where a slice would do, a
repeated full-PR read, an inlined-diff fan-out across sub-agents, a
polled MCP call paid per poll. File one skill-improvement task **per
distinct lever** (citing the sessions that motivate it), not one task
per session. A one-off that appears in a single session and implies no
skill change isn't filed — just note it consumed.

**3. File propose-only, as a single aggregated task.** All the levers a
pass synthesizes go into **one** `Claude:` Backlog task, not one issue
per lever — so a mining pass yields **one issue (one PR)** that doesn't
have to be hand-consolidated with `/merge-tasks` afterward. This mirrors
the **cspell-aggregation pattern** in `housekeeping` step 3 ("file the
drift as a single aggregated Backlog issue … each finding is a bullet
carrying its own `**Fingerprint**:` line"). The trade-off is
intended: aggregating means the levers can't run as independent
parallel PRs (separate issues would otherwise carry their own
file-collision links), and that's the accepted choice — one task / one
PR for these skill tweaks over parallelism. Per-lever fingerprints
preserve independent dedup regardless.

A trim lever always edits a skill or convention doc, so the aggregated
task is meta-work — prepend the **`Claude:`** prefix to its title, per
`CLAUDE.md` → "Claude: meta-work prefix". The task body is **one
`# Part N — <title>` section (or bullet) per lever**, and carries:

- one **`**Fingerprint**: session-metrics:<lever-slug>`** line **per
  lever** (the dedup key — later passes match on it individually), and
- a single **`**Touches**:`** line that **unions** every lever's globs
  (per `docs/conventions/linear-automation.md` → "Structured filing
  fields"), so `sync-blockers` sees the whole task's footprint.

**Dedup, then append or create — never duplicate:**

- **Collect the fingerprints already open.** List the open Backlog
  (`mcp__claude_ai_Linear__list_issues`, same destination) and gather
  every `**Fingerprint**:` line present across the open aggregated
  trim-context issue(s). Only **new** levers — fingerprints not already
  open — are filed; drop the rest.
- **Append to the open aggregated task if one exists.** If an open
  Backlog issue already carries any `session-metrics:` fingerprint
  (going forward there is at most one aggregated trim-context task),
  add the new levers to it rather than opening a second — with a
  **`patch`** on that issue's `id`, not a full `description` rebuild
  (per `docs/conventions/linear-automation.md` → "Partial edits — the
  `patch` argument"). Two ops in one call: an `append` carrying the new
  levers' `# Part` sections, and a `replace` on the existing
  `**Touches**:` line with the extended union. The append can't clobber
  an existing bullet, and the `**Touches**:` anchor is tag-free, so it
  matches cleanly. If more than one such issue somehow exists, append to
  the **lowest-ENG** one and note the others in the report for hand
  consolidation.
- **Otherwise create one** aggregated task, one section per new lever.
- **File nothing** when every lever is already open (neither create nor
  append).

**Autonomy bound:** filing a task *proposes* a fix — this skill
**never** edits a skill, a convention doc, or `CLAUDE.md`; that lands
later through a normal PR.

```txt
mcp__claude_ai_Linear__save_issue(
  team: "<$LINEAR_TEAM_ID>",
  project: "<$LINEAR_PROJECT_ID>",
  assignee: "<$LINEAR_ASSIGNEE_ID>",
  state: "Backlog",
  title: "Claude: <umbrella summary of this pass's trim levers>",
  description: "<one `# Part N — <title>` section per lever — each the
    lever, the sessions that motivate it, the concrete skill /
    convention-doc edit it implies, and its own **Fingerprint**:
    session-metrics:<lever-slug> line>\n\n**Touches**: <combined globs>",
  priority: 3,
)
```

**4. Filing the task discharges the entry — so drain it, don't tick
it.** There is no clear *decision* to make, and no question to ask.
Once an entry's levers are in a Backlog task, the entry has no further
job: its disposition belongs in the drain history as one line, not as a
retained body.

This replaces an earlier design that ticked entries and asked, via
`AskUserQuestion`, whether to clear them. That question is **retired**,
along with `housekeeping`'s inherited-decision hook — because the
combination failed structurally rather than through any missed
approval. `housekeeping`'s one-shot path (the default path, and the only
one the morning driver ever takes) fixed the answer to *leave*, so on
the path that actually runs the clear **never fired**. Nobody declined
it; it was never asked. The inbox filled with checked-off entries that
had already been filed, each re-read in full by every later pass.

So the rule now:

- **Delete on file, unconditionally.** The consumed-entry write-back in
  step 5 is a **deletion**, not a tick-and-annotate.
- **The `✓ filed:` note moves into the drain history**, which is where
  a human goes to ask "what happened to session X".
- **An entry that yielded no lever is still drained** — recorded as
  `⚠ noted: <reason>` in the drain history rather than filed. Consumed
  means consumed; "produced no task" is not a reason to retain a body.

**Why this is safe to do unattended.** The worry the clear question
encoded was losing measurement data, and it does not survive scrutiny:
the levers are preserved in the filed task with their fingerprints, the
drain history keeps the per-session disposition, and the underlying
transcript is still on disk and re-measurable with
`make session-metrics SESSION=<uuid>`. Deleting a filed entry destroys
nothing that isn't recoverable or already recorded elsewhere.

Step 5 still makes exactly **one** `save_document` write.

**The step-1 drain trigger measured the wrong thing, too.** It counted
*unprocessed* entries, so a doc holding twenty consumed-but-retained
entries read as a healthy inbox while costing the full body on every
read. With draining-on-file, unbounded growth is impossible by
construction — so the trigger is now just a **size** observation for the
step-6 report (how long the body is getting), not a threshold anything
branches on.

**5. Write the doc back once** with
`mcp__claude_ai_Linear__save_document` (id = the resolved value,
literal newlines) — as a **`patch`**, not a full `content` rebuild.
This skill's write-back is a *targeted* edit (it removes specific
entries and appends to the drain history), so it is exactly the case
`patch` serves: one call carrying one op per touched entry, keyed off
text from your step-1 read, per
`docs/conventions/linear-automation.md` →
"Partial edits — the `patch` argument". Two kinds of op:

- **One deletion per consumed entry** — a `replace` with an empty
  `new_string`, or a `replace_range` between two tag-free anchors,
  spanning the entry's whole block **through its trailing blank line**
  so no stray separator is left for the next pass to accumulate. Prefer
  `replace_range` where the block is long: it bounds the deletion
  explicitly at both ends, rather than resting on one enormous copied
  span matching exactly once.

  Delete only the entries this pass actually consumed — do **not** also
  collapse the doc to an empty-inbox placeholder. Whether any remain is
  a *count* from your step-1 read, and a concurrent `session-metrics`
  append can make that count wrong; exactly-once anchors stop you
  clobbering that entry, but they can't stop a stale count from stamping
  "inbox empty" over it.

- **One append to the drain history** recording each consumed entry's
  disposition on a line: `✓ filed: ENG-### (<lever>)` for one that drove
  a task, `⚠ noted: <reason>` for one that implied no change. This is
  the entry's surviving record, so name the session there.

**The deletion spans must come verbatim from the step-1 read.** This is
the safety property that made the old tick-path safe, and it carries
over unchanged: an exactly-once anchor copied from what you actually
read cannot enclose an entry appended since. That is not hypothetical —
the 2026-08-11 drain re-fetched before writing and found **two entries
appended mid-pass** that a snapshot-based wholesale rewrite would have
destroyed.

**Build every anchor by copying the stored text, not by composing it.**
A `replace` rewrites *its own anchor*, so its `old_string` has to span
exactly the text being changed — a delete spans the entry's whole block
including its `Measured:` / `Recommends:` sub-bullets. There is no way
to express that as a short tag-free fragment, so don't try: take the
span **verbatim from your step-1 read**, or bound it with
`replace_range` between two tag-free anchors. `append` is the one op
that needs no span, which is why the drain-history record uses it.

The one thing that can defeat a copied span is an **`ENG-###` inside
it** — Linear stores that as an issue-mention node rather than the
literal characters, so a span quoting one may not match (same
convention section). What an op *writes* is unconstrained: a
disposition note may contain an `ENG-###` freely. If a needed span is
tag-bearing and won't match, fall back to a full `content` rebuild for
that write and say so in the report — but **re-fetch the doc first**
and rebuild from *that* body. The no-re-fetch rule below is earned
by `patch`'s exactly-once anchors; a wholesale rebuild has no such
protection, so writing one from the step-1 snapshot would silently
erase any entry appended since.

**No `updatedAt` check.** The ops are applied atomically against the
live body and each anchor must match **exactly once**, so a concurrent
`session-metrics` append can't be clobbered (it isn't one of your
anchors) and an entry that shifted underneath you fails the save loudly
instead of being silently overwritten — a better guarantee than
comparing a timestamp that isn't reliable anyway. On that failure,
re-read the doc, rebuild the ops, and write once more; don't retry
blindly. That second write is the **only** licensed exception to the
one-write rule above.

When this runs right before a `session-metrics` producer step (e.g.
under `housekeeping`), the entries you drain are the ones read in step
1 — an entry that step appends afterwards is simply next pass's work,
and the exactly-once anchors make that safe without any ordering rule.

**6. Report** in one line: the aggregated skill-improvement task —
whether new levers were filed into a fresh one or appended to the open
one (with its ENG-###), and how many levers — for the recurring trim
levers, how many session entries were consumed **and drained**, any
levers skipped as already-handled, and the inbox's remaining size — or
that the skill no-op'd because the inbox id was unset.

## Notes

- **No source edits.** This skill writes only to Linear — the filed
  Backlog tasks and the inbox doc's dispositions — and never authors a
  code or skill diff, never commits, never pushes. The improvements it
  proposes are applied later by a human through a normal PR.
- **Runs standalone or as housekeeping's step.** `housekeeping`
  delegates its Session Metrics step to this skill; it runs just as well
  by hand any time the inbox has unprocessed entries. Either way the
  behavior is identical — there is no attended / propose-only mode.
- **Shell discipline** (per
  `docs/conventions/shell-commands.md`): every
  command is a single bare call that reduces to an allow-glob — no
  `&&`, pipes, `$(…)`, or redirects; resolve each id with a bare
  `printenv`, one variable per call.
