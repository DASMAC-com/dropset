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
file-overlap edges), and that's the accepted choice — one task / one
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
  **append** the new levers' sections to its description, extend its
  `**Touches**:` union, and re-save (`save_issue` with that issue's
  `id` and the full edited `description`) rather than opening a second.
  **Diff against the live body** you just read so existing bullets
  aren't clobbered. If more than one such issue somehow exists, append
  to the **lowest-ENG** one and note the others in the report for hand
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

**4. Write the disposition back** with
`mcp__claude_ai_Linear__save_document` (id = the resolved value,
literal newlines): tick each consumed entry (`- [ ]` → `- [x]`) and add
a nested note — `✓ filed: ENG-### (<lever>)` for one that drove a task,
or `⚠ noted: <reason>` for a one-off that implied no change. Build the
new body from the body you just fetched in step 1, changing only those
lines; if the doc `updatedAt` is newer than your fetch (a concurrent
edit), re-fetch and rebuild rather than clobbering it.

**5. Offer to clear the processed entries** so the inbox doesn't grow
unbounded. After the dispositions are written, ask via
**`AskUserQuestion`** whether to clear the now-**checked** (`- [x]`,
processed) entries, with the recommended default **first**: "yes, clear
the processed entries (Recommended)" and "no, leave them". Clear **only
on an explicit yes** — on "no" (or if nothing is checked) leave the doc
as written and move on. This step applies whether the skill runs
standalone or under `housekeeping` (which inherits it through the
delegation rather than re-implementing it). To clear: rebuild the body
from the **live** doc (re-fetch first, as in step 4) and drop only the
lines of entries that are checked **and** carry a disposition note,
collapsing to the empty-inbox template when none remain. Diff against
the live body, not your step-1 snapshot, so an unprocessed entry the
user (or a concurrent `session-metrics` run) added mid-pass is never
dropped. Write it back with `save_document`. When this runs right before
a `session-metrics` producer step (e.g. under `housekeeping`), evaluate
the clear against the inbox state **before** that step appends a fresh
entry.

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
