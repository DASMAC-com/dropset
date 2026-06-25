---
name: stage-backlog
description: One iteration of keeping the Dropset Task Staging document in sync with the Linear Backlog. The whole job is deterministic and lives in a committed, dependency-free Python tool (`tools/stage-backlog/stage_backlog.py`, run via `make stage-backlog`): read every open Backlog issue, build the dependency tree from declared blockedBy/blocks edges + file overlap, and rewrite the chips-only Task Staging document. The skill is a thin wrapper — it runs the tool and reports the one-line tally; it never reads the document body into context. The one optional manual touch is backfilling a `**Touches**:` line on an issue the dry-run flags as missing one. Drive the full re-stage with `/loop stage-backlog` or run it once.
disable-model-invocation: false
user-invocable: true
---

# `stage-backlog`

Run **one iteration** of staging the Dropset Linear
Backlog onto the **Task Staging** document and exit.
Agent-filed findings (`audit`) and hand-filed
to-dos (`linear-task`) all land as plain **Backlog**
issues with no parent. This skill turns that flat queue
into a plan: it groups the issues into the fewest
parallel, file-disjoint PRs and rewrites the document so
you can see — at a glance, with live Linear statuses —
what can run in parallel right now.

It replaces the old umbrella-issue plan: there is no
ENG-452 anymore. The Backlog is the queue; this
document is the plan.

## Deterministic core: the stage-backlog Python tool

The whole job is pure mechanism — string and graph work
plus two HTTP calls — so it lives in a committed,
dependency-free Python tool
(`tools/stage-backlog/stage_backlog.py`, run via
`make stage-backlog`) rather than being re-derived by
hand each run. The tool:

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
  and writes it to the Task Staging document.

That is the whole of the old "read → group → cross-check
→ write" core, now reproducible and lint-clean under the
repo's existing `ruff` hooks. It is **render-only**:
the tool never merges or closes issues — a same-file
collision is represented as a serial **nesting**, which
preserves the parallelism guarantee without mutating
Linear. Run it:

```sh
make stage-backlog
```

Add `ARGS=--dry-run` to print the tree to stdout without
writing the document:

```sh
make stage-backlog ARGS=--dry-run
```

The tool resolves its configuration from the environment
via `os.environ` (never a hard-coded id): `LINEAR_API_KEY`
(a personal API key — a script can't use the OAuth
`claude.ai` MCP) and `LINEAR_PROJECT_ID` are needed for
every run; `LINEAR_TASK_STAGING_DOC_ID` is read only for a
real write, so `--dry-run` doesn't require it. A missing
required variable errors and exits; export them in your
shell profile (`~/.zshrc`) — see `CLAUDE.md` → "Linear
automation".

Its unit tests (Python's `unittest`, no third-party test
dependency) run with `make stage-backlog-test`.

## Context economy

The skill **never reads the Task Staging document body
into context**. The tool reads the Backlog, renders the
tree, and writes the document in its own process; the
skill only runs it and reports the one-line tally (plus
any short stderr warnings). There is no model judgment
in the loop and no document body replayed across turns.

## The one manual touch: missing `**Touches**:`

An issue filed before the `**Touches**:` convention has
no file globs, so the tool can place it only by declared
edges / parent and prints a `warning:` naming it. The
only optional manual action is to backfill a
`**Touches**:` line on such an issue with a plain
`save_issue` (step 2) so the next run places it by file
collision too — it is a data fix, not a grouping
judgment.

## Configuration

The tool resolves everything via `os.environ` (never a
hard-coded id): `LINEAR_API_KEY` (a personal key — a
script can't use the OAuth `claude.ai` MCP) and
`LINEAR_PROJECT_ID` for every run, and
`LINEAR_TASK_STAGING_DOC_ID` only for a real write
(`--dry-run` doesn't need it). A missing required
variable errors and exits; export them in your shell
profile (`~/.zshrc`) — see `CLAUDE.md` → "Linear
automation".

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
commits or pushes. Its only write is to Linear: the
tool's rewrite of the **Task Staging** document. It
produces no source diff of its own.

## Steps

**1. Preview the current plan.** Run the tool in dry-run
to see the tree it would write and the missing-`Touches:`
warnings:

```sh
make stage-backlog ARGS=--dry-run
```

The warnings (on stderr) name every open issue with no
`**Touches**:` field, and flag any blocker **cycle** the
render had to break at its lowest-numbered member; the
stdout tree shows the grouping. Read both before deciding
whether anything needs the one manual touch.

**2. Backfill missing `Touches:` (optional).** For each
issue the dry-run flagged as missing `**Touches**:`, add
the field via `save_issue` (id = that issue) — the path
globs its work will edit, comma-separated, per `CLAUDE.md`
→ "Structured filing fields". This is best-effort; an
issue still missing it is placed by declared edges /
parent only. Skip this step when nothing is flagged.

**3. Render and write the document.** Run the tool
(no `--dry-run`):

```sh
make stage-backlog
```

It reads the Backlog, builds the deterministic tree, and
rewrites the **Task Staging** document in full (idempotent
— never appended to, so it never stacks duplicates).
**Open issues only**: a closed / resolved issue (Done /
Won't-fix / Canceled / Duplicate) is omitted entirely, so
anything closed or merged since the last run simply drops
off. This also makes the document safe for you to
hand-trim — deleting a line for a task you can see has
closed will not be undone, because a closed issue is
excluded from regeneration anyway.

**4. Report.** The tool prints its own one-line tally:

```txt
stage-backlog | <n> backlog issues | staging document updated
```

When invoked once by hand (not under `/loop`), the single
iteration runs and the skill exits; under `/loop` it
re-invokes immediately (no timer, no wait).

## Notes

- **The chips-only format.** Below the tally (next bullet),
  the document is bare
  `ENG-###` tags nested by blocker, under `# Skills`,
  `# ENG-###` parent headings, and `# Standalone` — **no**
  per-issue summary, file globs, or merge notes, **no**
  preamble or legend, and **no** "Wave N" / "start now"
  labels. The chip renders the issue's live title and
  status; the nesting is the ordering. The only inline
  annotation is a trailing `(after ENG-###)` /
  `(also after ENG-###)` for a blocker the tree can't
  show.
- **The `# Most blocking` tally.** The document opens with
  a `# Most blocking` section ranking every issue that
  blocks at least one other by **how many** it blocks
  (descending, ties broken by lowest `ENG-###` first), as
  `- ENG-### — blocks <n> issues`. It tells you which
  issue to start on first — the one at the top unblocks
  the most downstream work. The count is **direct** (the
  number of issues that list it as a blocker, declared or
  file-overlap), so it matches the `(after …)` edges the
  tree shows and stays meaningful inside a blocker cycle.
  The section is omitted entirely when nothing blocks
  anything.
- **Relations are read, honoured, and preserved — never
  manufactured.** The tool treats a declared `blockedBy`
  / `blocks` edge as authoritative input to the tree, but
  never writes the *inferred* file-overlap nesting back as
  a relation — that's a scheduling artifact, not a true
  dependency. The durable record of real dependencies is
  what the filing skills (`linear-task`, `audit-scope`,
  `audit`) set at file time.
- **No issue folding.** The tool renders the Backlog as it
  finds it; it never merges or closes issues. Two issues
  that belong in one PR render as nested serial chips, not
  a single folded issue. (A merge capability, if ever
  wanted, will be requested separately.)
- **No umbrella issue.** This skill, the plain Backlog, and
  the document fully replace the old ENG-452 parent.
- Shell discipline (per `CLAUDE.md`): every command is a
  single bare call that reduces to an allow-glob — no
  `&&`, pipes, `$(...)`, or redirects; content search
  routes to the Grep tool (never `git grep`).
