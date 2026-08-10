---
name: trim-context
description: Mine the Linear "Session Metrics" inbox into a propose-only skill-improvement Backlog task — the consumer half of the `session-metrics` producer. Reads the inbox document live, synthesizes the trim levers that recur across sessions (a verbose build log, a whole-file Read where a slice would do, a repeated full-PR read, an inlined-diff fan-out), and files them as a single aggregated propose-only task — one bullet per lever, each with its own `**Fingerprint**:` line under a combined `**Touches**:` (so one mining pass yields one issue / one PR, not a batch to consolidate later). Dedups each lever against the open Backlog, appends to the open aggregated task rather than opening a second, writes each consumed entry's disposition back into the doc, and offers (via AskUserQuestion) to clear the processed entries so the inbox doesn't grow unbounded. Never edits a skill or convention doc — filing a task is the proposal. Runs standalone or as `housekeeping`'s Session Metrics step.
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

**Count them, and note the body's size.** More than **~3** unprocessed
entries is a **drain trigger** — act on it rather than waiting to be
asked. The reason is this skill's own cost, not the producer's:
`session-metrics` appends with a `patch` op and pays only the length of
its entry (per `docs/conventions/linear-automation.md` → "Partial edits
— the `patch` argument"), so it appends happily forever. **Mining** is
what gets more expensive — step 1 reads the whole body every pass, and
a long inbox is harder to synthesize honestly and stales the older
entries. Carry the **count** into step 4 — that's the number the clear
question and the step-6 report both name.

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

**4. Decide the clear first — before writing anything back.** The two
outcomes want **different** write-back ops (step 5), and on a "yes,
clear" every tick-and-annotate op is thrown away — the annotations are
composed, then immediately deleted. Cheaper than it used to be (a
`patch` write-back doesn't re-author the whole body), but still pure
waste, and the ordering costs nothing. So resolve the clear decision
**up front**, via
**`AskUserQuestion`**, recommended default **first**: "yes, clear the
processed entries (Recommended)" and "no, leave them". Clear **only on
an explicit yes**; on "no" (or if nothing was consumed this pass) the
entries stay. When a caller has already fixed the decision — e.g.
`housekeeping`'s one-shot pass defaults to *leave* and passes that in —
take the inherited answer and don't re-ask. Whichever way it resolves,
step 5 makes exactly **one** `save_document` write.

**Past the step-1 drain trigger, say so in the question.** When the
inbox held more than ~3 unprocessed entries, the clear is no longer a
neutral tidy-up — it's what keeps the next mining pass tractable, so
name the count in the question text so the human is choosing with that
in front of them. And when a caller has fixed the decision to *leave*,
honor it (don't override an inherited answer) but **flag the backlog in
the step-6 report**: how many entries the inbox is now carrying and that
it is past the drain threshold. That way an inbox growing past the point
where it can be mined honestly is visible rather than silent.

**5. Write the doc back once, per the step-4 decision** with
`mcp__claude_ai_Linear__save_document` (id = the resolved value,
literal newlines) — as a **`patch`**, not a full `content` rebuild.
This skill's write-back is a *targeted* edit (it ticks and annotates
specific entries, or removes them), so it is exactly the case `patch`
serves: one call carrying one op per touched entry, keyed off text from
your step-1 read, per `docs/conventions/linear-automation.md` →
"Partial edits — the `patch` argument". Per the decision:

- **Clear = yes:** one `replace` op per consumed entry, `new_string`
  empty, deleting its lines — take the span through the entry's
  **trailing blank line** so a deleted entry doesn't leave a stray
  separator behind for the next pass to accumulate. Delete only the
  entries this pass actually consumed — do **not** also try to collapse
  the doc to an empty-inbox placeholder. Whether any remains is a *count*
  from your step-1 read, and a concurrent `session-metrics` append can
  make that count wrong; exactly-once anchors stop you clobbering that
  entry, but they can't stop a stale count from stamping "inbox empty"
  over it.
- **Clear = no:** leave every entry in place but tick each consumed one
  and add a nested disposition note — a `✓ filed: ENG-### (<lever>)`
  for one that drove a task, or a `⚠ noted: <reason>` for a one-off
  that implied no change. Two ops per consumed entry: a `replace`
  flipping its box (`- [ ]` → `- [x]`, with enough of the entry's own
  header line after the box to match **once**), and an `insert_after`
  keyed on that entry's `session <short-uuid>` fragment carrying the
  note. `insert_after` splices in **immediately** after its anchor, and
  that anchor sits mid-line, so the note's `text` must **open with a
  real newline plus the nesting indent** — otherwise the note lands
  glued to the end of the header rather than on its own nested line.
  **Skip the annotation work entirely** on a "yes, clear" (this is the
  whole reason the clear is decided first).

**Build every anchor by copying the stored text, not by composing it.**
A `replace` rewrites *its own anchor*, so its `old_string` has to span
exactly the text being changed — the box-flip spans the entry's header
line, and a delete spans the entry's whole block including its
`Measured:` / `Recommends:` sub-bullets. There is no way to express
either op as a short tag-free fragment, so don't try: take the span
**verbatim from your step-1 read**. `insert_after` is the one op that
needs no span, which is why the disposition note keys off the
`session <short-uuid>` fragment above.

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
under `housekeeping`), evaluate the clear against the inbox state
**before** that step appends a fresh entry.

**6. Report** in one line: the aggregated skill-improvement task —
whether new levers were filed into a fresh one or appended to the open
one (with its ENG-###), and how many levers — for the recurring trim
levers, how many session entries were consumed, any levers skipped as
already-handled, whether the processed entries were cleared — or that
the skill no-op'd because the inbox id was unset.

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
