---
name: stage-backlog
description: One iteration of keeping the Dropset Task Staging document in sync with the Linear Backlog. The deterministic core — read every open Backlog issue, build the dependency tree from declared blockedBy/blocks edges + file overlap, rewrite the chips-only Task Staging document, and fold a same-PR group onto its canonical (closing the rest as duplicates) via the merge subcommand — runs as the committed `dropset-stage-backlog` binary (`make stage-backlog`). The skill drives that binary and keeps the agent for the two judgment calls a tool can't make: deciding which issues belong in one PR, and the fallback for legacy issues that predate the `**Touches**:` field. Also runs an incremental mode (given just-filed ENG-### ids) that slots new findings into the current document in place — the fast in-between update audit-loop drives as it files. Drive the full re-stage with `/loop stage-backlog` or run it once.
disable-model-invocation: false
user-invocable: true
---

<!-- cspell:word startable -->

# `stage-backlog`

Run **one iteration** of staging the Dropset Linear
Backlog onto the **Task Staging** document and exit.
Agent-filed findings (`audit-loop`) and hand-filed
to-dos (`linear-task`) all land as plain **Backlog**
issues with no parent. This skill turns that flat queue
into a plan: it groups the issues into the fewest
parallel, file-disjoint PRs and rewrites the document so
you can see — at a glance, with live Linear statuses —
what can run in parallel right now.

It replaces the old umbrella-issue plan: there is no
ENG-452 anymore. The Backlog is the queue; this
document is the plan.

## Deterministic core: the `dropset-stage-backlog` binary

Most of this skill is pure mechanism, so it lives in a
committed Rust binary (`tools/stage-backlog`, run via
`make stage-backlog`) rather than being re-derived by
hand each run. The binary:

- reads every **open** Backlog issue for the project
  (with its `parentId` and its declared `blockedBy` /
  `blocks` edges);
- builds the dependency tree from those declared edges
  **plus** file overlap — two issues whose `**Touches**:`
  globs collide can't run in parallel, so the
  higher-numbered one nests under the lower;
- buckets issues under `# Skills` (pure skill-suite
  work), a `# ENG-###` heading per parent with 2+ Backlog
  subtasks, and a trailing `# Standalone`;
- renders the chips-only tree (bare `ENG-###` tags,
  4-space nesting, `(after …)` / `(also after …)` notes)
  and writes it to the Task Staging document;
- under its `merge` subcommand, folds a same-PR group onto
  its lowest-numbered canonical — merging descriptions,
  `**Fingerprint**:` / `**Touches**:` fields, and
  `blockedBy` / `blocks` edges — and closes the rest as
  duplicates, write-before-close (see step 2).

That is the whole of the old "read → group → cross-check
→ write" core, now reproducible and lint-clean under the
workspace's `cargo fmt` + `clippy` hooks — and it retires
the adversarial sub-agent cross-check that used to run
every iteration. Run it:

```sh
make stage-backlog
```

Add `ARGS=--dry-run` to print the tree to stdout without
writing the document:

```sh
make stage-backlog ARGS=--dry-run
```

The binary resolves its configuration from the
environment via `std::env::var` (never a hard-coded id):
`LINEAR_API_KEY` (a personal API key — the headless
binary can't use the OAuth `claude.ai` MCP) and
`LINEAR_PROJECT_ID` are needed for every run;
`LINEAR_TASK_STAGING_DOC_ID` is read only for a real
write, so `--dry-run` doesn't require it. A missing
required variable errors and exits; export them in your
shell profile (`~/.zshrc`) — see `CLAUDE.md` → "Linear
automation".

## What stays with the agent

The binary is deterministic "where there's data". Two
**judgment calls** genuinely need a model, and stay here:

1. **The grouping decision** (optional consolidation).
   Deciding *which* issues should land as **one PR** — they
   touch the same files, or are otherwise one unit of work —
   is a prose judgment the binary doesn't make. Once the
   group is chosen, the **mechanical fold is the binary's**:
   `merge` folds the members onto the lowest-numbered
   canonical (their descriptions under per-source
   sub-headings, the union of their `**Fingerprint**:` /
   `**Touches**:` / `blockedBy` / `blocks` fields), then
   closes the others as duplicates — write-before-close. The
   merge is an **optimization**, not a correctness
   requirement: the binary already prevents a parallel file
   conflict by **nesting** colliding issues (serial), so
   skipping it just means two chips instead of one, never a
   conflict. Decide the group when it meaningfully cuts PR
   count and hand it to `merge` (step 2 below); the binary
   renders whatever Backlog remains.
1. **The missing-`Touches:` fallback.** An issue filed
   before the `**Touches**:` convention has no file globs,
   so the binary can place it only by declared edges /
   parent and prints a `warning:` naming it. Add a
   `**Touches**:` line to each such issue (step 3) so the
   next run places it by file collision too.

## Two modes

- **Full re-stage** (default — no argument): the
  authoritative reconcile. Optionally merge same-PR issues
  (step 2), backfill any missing `**Touches**:` (step 3),
  then run the binary to rebuild and write the whole
  document (step 4). This is the source of truth.
- **Incremental** (argument: one or more just-filed
  `ENG-###` ids): slot each given issue into the
  **current** document in place — without a full
  re-derive. This is the fast in-between update
  `audit-loop` calls as it files findings, so the plan
  stays roughly current between full passes. See
  **Incremental mode** at the end. Incremental insertion
  can drift from what a full re-stage produces, so the
  **next full re-stage reconciles it** — `housekeeping`'s
  morning pass runs that full re-stage.

## Configuration

Both the render and the `merge` subcommand are the binary,
so they share its environment — no separate Linear MCP
filing destination. The binary resolves everything via
`std::env::var` (never a hard-coded id): `LINEAR_API_KEY`
(a personal key — the headless binary can't use the OAuth
`claude.ai` MCP) and `LINEAR_PROJECT_ID` for every run, and
`LINEAR_TASK_STAGING_DOC_ID` only for a real render write
(`--dry-run` and `merge` don't need it). The `merge`
subcommand resolves the **team** for the duplicate state
from the group's own issues, so it needs no `LINEAR_TEAM_ID`
/ `LINEAR_ASSIGNEE_ID`. A missing required variable errors
and exits; export them in your shell profile (`~/.zshrc`) —
see `CLAUDE.md` → "Linear automation".

## How it's driven

This skill is meant to run under the built-in loop
harness so the document stays continuously
cross-referenced against the Backlog:

```sh
/loop stage-backlog
```

Invoked with no interval, `/loop` re-invokes this skill
**continuously** — back-to-back, with **no timer or wait
between iterations**. As soon as one iteration finishes,
begin the next; do not `ScheduleWakeup`, sleep, or
otherwise pace between cycles. The skill itself contains
**no** scheduling — it does exactly one iteration per
invocation, and runs just as well invoked once by hand to
restage on demand. Run it from a throwaway worktree you
never commit in; it never authors a source edit.

## Read-only with respect to source

This skill **never authors source edits** and never
commits or pushes. Its only writes are to Linear: the
optional merge of Backlog issues, and the binary's
rewrite of the **Task Staging** document. It produces no
source diff of its own.

## Steps (full re-stage)

**1. Preview the current plan.** Run the binary in
dry-run to see the tree it would write and the
missing-`Touches:` warnings:

```sh
make stage-backlog ARGS=--dry-run
```

The warnings (on stderr) name every open issue with no
`**Touches**:` field; the stdout tree shows the grouping.
Read both before deciding what, if anything, needs the
agent.

**2. Merge same-PR issues (optional consolidation).**
Decide *which* issues should land as a **single PR** (they
touch the same files, or are otherwise one unit of work) —
that grouping is your call. The **fold itself is the
binary's**: hand it each group, lowest-ENG first or in any
order (it sorts), as a comma-separated list. Preview before
writing:

```sh
make stage-backlog ARGS="merge --dry-run ENG-465,ENG-470"
```

The dry-run prints, per group, the chosen canonical (lowest
ENG number, taking the group's **max** priority), the
members that will close as duplicates, the external
`blockedBy` / `blocks` edges it will add, and the full
folded description — each member's body under a
`### From ENG-###` sub-heading plus the deduped union of
every `**Fingerprint**:` and `**Touches**:`. Review it,
then drop `--dry-run` to perform the merge:

```sh
make stage-backlog ARGS="merge ENG-465,ENG-470"
```

Pass several groups in one call by listing each as its own
argument (`ARGS="merge ENG-1,ENG-2 ENG-8,ENG-9"`). The
binary is **write-before-close**: it folds everything onto
the canonical and confirms that write before closing any
member, so an interruption never drops state. Intra-group
edges (a member blocking the canonical) are dropped rather
than becoming self-edges.

This step is **optional** — skip it when no group clearly
belongs in one PR. The binary nests colliding issues
serially regardless, so skipping never risks a merge
conflict; it just leaves them as separate chips.

**3. Backfill missing `Touches:` (fallback).** For each
issue the dry-run flagged as missing `**Touches**:`, add
the field via `save_issue` (id = that issue) — the path
globs its work will edit, comma-separated, per `CLAUDE.md`
→ "Structured filing fields". This is best-effort; an
issue still missing it is placed by declared edges /
parent only.

**4. Render and write the document.** Run the binary
(no `--dry-run`):

```sh
make stage-backlog
```

It reads the now-merged Backlog, builds the deterministic
tree, and rewrites the **Task Staging** document in full
(idempotent — never appended to, so it never stacks
duplicates). **Open issues only**: a closed / resolved
issue (Done / Won't-fix / Canceled / Duplicate) is
omitted entirely, so anything closed or merged since the
last run simply drops off. This also makes the document
safe for you to hand-trim — deleting a line for a task
you can see has closed will not be undone, because a
closed issue is excluded from regeneration anyway.

**5. Report.** The binary prints its own one-line tally:

```txt
stage-backlog | <n> backlog issues | staging document updated
```

When invoked once by hand (not under `/loop`), the single
iteration runs and the skill exits; under `/loop` it
re-invokes immediately (no timer, no wait).

## Incremental mode

Given one or more just-filed `ENG-###` ids (typically
from `audit-loop` as it files), fold each into the
**current** Task Staging document in place. This does
**not** run the binary or regenerate the whole plan — it
edits the live body around the existing tree via the MCP,
so it's cheap enough to run after every finding (and
works inside an `audit-loop` session, which has the
Linear MCP but not necessarily the binary's API key).

1. **Resolve the staging doc id** from
   `LINEAR_TASK_STAGING_DOC_ID` (bare `printenv`). If
   empty, stop and say so.

1. **Read the document live** with
   `mcp__claude_ai_Linear__get_document` (id = the
   resolved value). Never reuse a stale snapshot — the
   loop edits it repeatedly, and `housekeeping` may too.

1. **For each given `ENG-###`:** fetch it with
   `mcp__claude_ai_Linear__get_issue`
   (`includeRelations: true`) and parse its **`**Touches**:`
   globs**, its Linear **parent** (`parentId`), and its
   declared **`blockedBy` / `blocks`** edges. Skip an id
   that is already a chip in the document, or that isn't
   open (a just-filed Backlog issue always is).

1. **Place the chip with the same grouping rules the
   binary uses** — best-effort, since the global regroup
   is the next full pass's job:

   - **Pure skill-suite work** (touches only
     `.claude/skills/**` and/or `CLAUDE.md`) → nest under
     the `# Skills` heading (add it at the very top if it
     doesn't exist yet).
   - **Shares a `parentId`** with an issue already in the
     tree → place it under that parent's `# ENG-###`
     heading (promoting the parent to a heading if this is
     the second same-parent subtask).
   - **Blocked** by a chip already in the tree — a declared
     `blockedBy` edge, or a `**Touches**:` overlap with it
     — → nest it under that blocker (4-space indent per
     level); add an `(also after ENG-###)` note for a
     second blocker the nesting can't show, and an
     `(after ENG-###)` note for a cross-heading blocker
     (never nest across headings).
   - **Otherwise** → a top-level chip under `# Standalone`
     (or under its parent heading if one applies).

   Write the reference as the **bare `ENG-###` tag** in
   plaintext, never a markdown link, so Linear renders the
   live title and status.

1. **Save the document.** Build the new body from the body
   you just read, changing only the lines you're
   inserting — never reorder or rewrite the rest. Save with
   `mcp__claude_ai_Linear__save_document` (id = the
   resolved value, literal newlines). If the doc
   `updatedAt` is newer than when you fetched it, re-fetch
   and rebuild rather than overwriting a concurrent edit.

This is a **fast, lossy-by-design** update: it places
chips but does **not** merge issues (step 2) or globally
regroup. The **next full re-stage reconciles** — the
binary re-derives the whole tree — and that pass is the
source of truth, run by `housekeeping` each morning. Print
a one-line tally:

```txt
stage-backlog incremental | +<n> chips placed
```

## Notes

- **The chips-only format is shared.** Both the binary
  (full mode) and the incremental hand-edits target the
  same shape: bare `ENG-###` tags nested by blocker, under
  `# Skills`, `# ENG-###` parent headings, and
  `# Standalone` — **no** per-issue summary, file globs, or
  merge notes, **no** preamble or legend, and **no**
  "Wave N" / "start now" labels. The chip renders the
  issue's live title and status; the nesting is the
  ordering. The only inline annotation is a trailing
  `(after ENG-###)` / `(also after ENG-###)` for a blocker
  the tree can't show.
- **Reversible-safe merges.** The `merge` subcommand's
  write-before-close ordering means the union of
  fingerprints, touches, and declared relations lands on the
  survivor before any member is closed, so `audit-loop`
  dedup keeps recognizing a folded-in finding and no edge is
  dropped when a member becomes a Duplicate.
- **Relations are read, honoured, and preserved — never
  manufactured.** The binary treats a declared `blockedBy`
  / `blocks` edge as authoritative input to the tree and
  `merge` carries edges across, but neither writes the
  *inferred* file-overlap nesting back as a relation —
  that's a scheduling artifact, not a true dependency. The
  durable record of real dependencies is what the filing
  skills (`linear-task`, `audit-scope`, `audit-loop`) set
  at file time.
- **No umbrella issue.** This skill, the plain Backlog, and
  the document fully replace the old ENG-452 parent.
- Shell discipline (per `CLAUDE.md`): every command is a
  single bare call that reduces to an allow-glob — no
  `&&`, pipes, `$(...)`, or redirects; content search
  routes to the Grep tool (never `git grep`).
- To graduate this to a fully detached schedule (cron
  cloud routine) later: both the render and `merge`
  authenticate headless via `LINEAR_API_KEY`, so the only
  thing still needing an interactive session is the
  **grouping decision** that feeds `merge` — the mechanical
  fold itself is cron-ready.
