---
name: sync-blockers
description: Keep the Dropset Linear Backlog's file-overlap links in sync with `**Touches**:`. The whole job is deterministic and lives in a committed, dependency-free Python tool (`.claude/tools/sync_blockers.py`, run directly with `python3`): read the open Backlog, find every `**Touches**:` file-overlap collision with no relation yet, and file a real `related` relation naming the paths the pair collides on. It files no blocking edge — blocking is human-curated, so an agent may only suggest one with its evidence. Two write modes — `--for ENG-###` (incremental, file-time: just the named issue vs. the backlog) and a bare full sweep (reconciliation) whose report adds collision clusters, human-declared semantic blocks, and the two scheduling smells — plus a read-only `--report-todo-blocks` JSON scan. It never renders or writes a document, never merges or closes issues, and never deletes a relation. The filing skills call `--for` after `save_issue`; run the full sweep by hand to reconcile after backfilling a `**Touches**:` line on an older issue.
disable-model-invocation: false
user-invocable: true
---

# `sync-blockers`

Keep the Dropset Linear Backlog's **file-overlap links**
in sync with each issue's `**Touches**:` globs.
Agent-filed findings (`audit`) and hand-filed to-dos
(`linear-task`) land as plain **Backlog** issues; two of
them that edit the same files are coupled, and worth
seeing as coupled. This skill makes that visible as a real
Linear `related` relation naming the paths they collide
on.

There is **no Task Staging document** anymore, and no
rendered tree — navigate the plan in Linear.

## Blocking is human-curated — this skill never files one

A `blocks` edge and a file collision are two different
claims, and conflating them was the defect this skill was
reworked to fix:

- A **semantic dependency** — B consumes A's output —
  genuinely orders work.
- A **mechanical collision** — two PRs touch the same glob
  — costs at most a rebase.

Coarse crate-level `**Touches**` globs, binary block
semantics, and an arbitrary lower-number-blocks-higher
orientation produced giant serial chains: a day-1 mainnet
param-channel issue sat behind **eight** overlap blockers,
and a docs-only pair was block-linked because both touched
`docs/market-making.md` in unrelated sections.

The deeper reason **no automated writer may file a
blocking edge — not this tool, not a filing skill, not an
autonomous `audit` rotation** — is that the board's
available-vs-blocked view is a **scheduling instrument the
human drives**: a hand-built blocking queue expressing
intended order of attack, from which the *available* set
is then sorted by priority. An auto-filed edge silently
makes that view untrustworthy, and a wrongly-blocked issue
drops out of the available set altogether. A missing edge
costs at most a rebase; a spurious one costs scheduling.

So an agent may **suggest** a blocking edge — naming the
candidate blocker and the concrete evidence — and a human
approves it or places it by hand. Edges a human placed are
**authoritative**: the automation never rewrites,
redirects, or removes one, with no exception (there was one
— see "The retired `--demote` migration" below). See
`CLAUDE.md` → "Blocking relations" and
`docs/conventions/linear-automation.md` for the rule as it
binds every filing skill.

## Deterministic core: the sync-blockers Python tool

The whole job is pure mechanism — string/glob work plus a
couple of HTTP calls — so it lives in a committed,
dependency-free Python tool
(`.claude/tools/sync_blockers.py`, run directly with
`python3`) rather than being re-derived by hand. The tool:

- reads every **open** Backlog issue for the project (with
  its `**Touches**:` globs and its existing relations);
- finds each pair whose touch-globs collide and that has
  **no** relation yet, in either direction;
- files a real `related` relation for each such collision,
  and reports the paths the pair collides on.

It **only** writes `related` relations. It never renders a
document, never ranks anything, and never merges or closes
issues.

### Four modes

**Incremental — `--for ENG-###` (the file-time path).**
Compares *only* the named, just-filed issue's touches
against the rest of the open Backlog and relates its
collisions. Bounded work — one node vs. the backlog, not
an N×N re-scan — so each filing skill calls it right after
`save_issue`:

```sh
python3 .claude/tools/sync_blockers.py --for ENG-###
```

Each collision prints the paths it collides on, so the
filing skill can report the coupling it just recorded:

```txt
related-linked: ENG-806 ~ ENG-810 (overlaps on .claude/tools)
```

No race gap: if A then B are filed, B's file-time check
sees A and files the single symmetric link; A's earlier
check simply didn't see B yet. The later filer always
covers the pair. Because the link is maintained at file
time, no periodic run is required.

**Full sweep — bare (reconciliation).** Compares every
pair, then prints the three report sections below. Run it
by hand to reconcile after backfilling a `**Touches**:`
line on an *older* issue, or as an occasional catch-up:

```sh
python3 .claude/tools/sync_blockers.py
```

Add `--dry-run` (either **sweep** mode) to print the links it
*would* file and write nothing:

```sh
python3 .claude/tools/sync_blockers.py --dry-run
```

**Report-only — `--report-todo-blocks`.** A read-only scan
(it files nothing) that prints the two scheduling smells as
**JSON** — the machine-readable contract `housekeeping`
consumes. Cannot combine with `--for`:

```sh
python3 .claude/tools/sync_blockers.py --report-todo-blocks
```

```txt
{
  todo_blocks_backlog: [{blocker, blocker_state, blocked}],
  urgent_gated_by_non_urgent: [
    {blocker, blocker_priority, blocked_urgent}
  ]
}
```

### The retired `--demote` migration

There was a fourth mode. `--demote` was a one-time,
propose-then-confirm migration that listed every `blocks`
edge between two open Backlog issues and, under `--apply`,
converted them to `related` — the cleanup for edges the
automation had filed before blocking became human-curated.

**It ran on 2026-08-10, and it has been removed.** It is not
deprecated, hidden, or guarded — the flags are gone and a
stale invocation now fails as an unknown argument.

The reason it could not simply be left in place: it is now
**spent**, and every candidate it can still find is a false
positive. Linear records no author on a relation, so the tool
never could distinguish a hand-placed edge from an auto-filed
one — it relied entirely on the human confirm gate for that.
The six legitimate hand-placed edges that remain all collide
on files, which is exactly the signature the tool reads as
"the sweep would re-derive this". So a second
`--demote --apply` would delete the intended blocking graph
in a single command, with the confirm gate showing a list
that looks correct.

That made it dead code *and* a live foot-gun, so removing it
beat guarding it. Blocking-edge changes now happen only where
they belong: in a planning session, human-directed, one edge
at a time.

### The full sweep's three report sections

The bare sweep prints these to stderr after filing its
links. They are text, not JSON, because every consumer is a
reader — a human, or the model driving `housekeeping`.

- **collision clusters** — issues grouped **per shared
  path**. This is the direct input to the `plan` skill's
  merge-group proposal step: a cluster is the candidate set
  for "these would land as one PR", e.g. *these three issues
  all touch `bots/maker-bot/src/model/feeds.rs`*. Paths whose
  member set is identical are merged into one entry listing
  both, so the same group isn't proposed twice.

  It is deliberately **not** grouped by connected component.
  That reading is the intuitive one and it is useless here:
  run over the real Backlog it put **25 of 27** issues in a
  single cluster, because coupling chains through shared files
  (everything touches a `Cargo.toml`; several things touch
  `bots/maker-bot`). "Merge everything" is no proposal — and
  the coherence floor forbids it anyway, since a component
  spans separate apps and languages.

  One consequence: an issue appears in **every** cluster whose
  path it touches, so clusters overlap and the member lists do
  not partition the Backlog. That is correct for a proposal —
  the reader picks which grouping to act on.

- **semantic blocks** — the surviving human-declared
  `blockedBy` edges. With no automated writer filing one,
  this section *is* the intended scheduling order, worth
  reading against the collision clusters.

- **smells** — the two scheduling smells, now scanning those
  human-declared edges only. A **`Todo`-state issue blocking
  a `Backlog` issue** (per the Todo/Backlog convention,
  initiatives / meta sit in `Todo` and pullable work in
  `Backlog`, so a Todo blocker means the pullable item can't
  actually start), and a **non-Urgent issue blocking an
  Urgent one** (which usually wants the reverse edge, so the
  Urgent work lands first).

### No priority floor any more

Earlier versions refused to file an edge that would gate an
Urgent issue behind a non-Urgent one. That floor is **gone,
because what it guarded against is gone**: a `related` link
is symmetric and gates nothing, so it has no orientation for
an inversion to get wrong. Priority is still read, but only
so the smells section can report inversions among the
human-declared edges. An unreadable priority no longer
suppresses anything — the tool warns, since it would
otherwise silently under-report.

Its unit tests (Python's `unittest`, no third-party test
dependency) run with `make tools-tests`, the shared target
that runs every Python skill-tool's tests.

## Configuration

The tool resolves everything via `os.environ` (never a
hard-coded id): `LINEAR_API_KEY` (a personal key — a script
can't use the OAuth `claude.ai` MCP) and `LINEAR_PROJECT_ID`.
There is **no** document-id variable — the tool writes no
document. A missing required variable errors and exits;
export them in your shell profile (`~/.zshrc`) — see
`CLAUDE.md` → "Linear automation".

## Context economy

The skill runs the tool and reports its one-line tally (plus
the short stderr sections and any warnings). The Backlog read
and the relation writes happen in the tool's own process;
nothing about the backlog body is replayed across turns.

## The one manual touch: missing `**Touches**:`

An issue filed before the `**Touches**:` convention has no
file globs, so the tool can't check it for overlap and
prints a `warning:` naming it. The only optional manual
action is to backfill a `**Touches**:` line on such an issue
with a plain `save_issue` — a data fix — then run the full
sweep (or `--for` on that issue) so its links get filed.

## How it's driven

The **file-time** path needs no driving — the filing skills
call `--for` for you. Invoke this skill directly only to run
the **full sweep** (reconciliation). It can run under the
loop harness for a periodic reconcile, though that is no
longer required:

```sh
/loop sync-blockers
```

Invoked with no interval, `/loop` re-invokes this skill
**continuously** — back-to-back, with **no timer or wait
between iterations**. The skill itself contains **no**
scheduling — it does exactly one sweep per invocation, and
runs just as well invoked once by hand. Run it from a
throwaway worktree you never commit in; it never authors a
source edit.

## Read-only with respect to source

This skill **never authors source edits** and never commits
or pushes. Its only writes to Linear are the `related` links
the tool materializes — it deletes nothing and files no
blocking edge. It produces no source diff of its own.

## Steps

**1. Preview (optional).** Run the tool in dry-run to see
the collision links a real sweep would file and the
missing-`Touches:` warnings:

```sh
python3 .claude/tools/sync_blockers.py --dry-run
```

The stderr output names every open issue with no
`**Touches**:` field and prints a `would relate:` line for
each collision a real run would materialize.

**2. Backfill missing `Touches:` (optional).** For each
issue the dry-run flagged as missing `**Touches**:`, add the
field via `save_issue` (id = that issue) — the path globs
its work will edit, comma-separated, per `CLAUDE.md` →
"Structured filing fields". Skip when nothing is flagged.

**3. Sweep.** Run the tool (no `--dry-run`):

```sh
python3 .claude/tools/sync_blockers.py
```

It reads the Backlog and files any unlinked collision.
Idempotent — a pair already linked (a declared block either
direction, an existing `related`, or one filed earlier in
this pass) is skipped, so a relation is never duplicated.

**4. Report.** The tool prints its own one-line tally:

```txt
sync-blockers | <n> backlog issues | <k> collision links filed
```

(In `--for` mode the tally names the issue instead of the
count, e.g.
`sync-blockers | ENG-663 | 1 collision link filed`.)

Relay the **collision clusters** section too when the sweep
found any — that is the input a human or `housekeeping` uses
to decide whether coupled issues should merge into one PR.

**5. Surface a suspected dependency, don't file it.** If the
sweep's clusters or smells make you believe a real semantic
dependency exists, say so and offer it — via
`AskUserQuestion`, naming the candidate blocker and the
concrete evidence ("this consumes the output of X — should
it be blocked by it?"). Write the edge **only** on an
explicit yes. In any non-interactive or autonomous run where
nobody can answer, the default is **no edge**; record the
suspicion as prose in the issue body instead, so the
reasoning is never lost.

## Notes

- **Linear relations, not a document.** The `related`
  relations this tool files show up on each issue in Linear.
  There is no rendered tree, no `# Most blocking` ranking,
  and no document to read.
- **A declared edge wins.** A human-declared `blockedBy` /
  `blocks` edge already expresses the coupling more strongly
  than a related link would, so the pair is skipped rather
  than double-linked. The collision still appears in the
  cluster report, which is about file coupling regardless of
  how the pair is linked.
- **No blocking edge, ever, from automation.** Not from this
  tool, not from a filing skill, not from an autonomous
  rotation. Suggest and let a human approve.
- **No issue folding.** The tool never merges or closes
  issues; consolidation is a separate job, owned by
  `merge-tasks`.
- Shell discipline (per `CLAUDE.md`): every command is a
  single bare call that reduces to an allow-glob — no `&&`,
  pipes, `$(...)`, or redirects; content search routes to
  the Grep tool (never `git grep`).
