---
name: sync-blockers
description: Keep the Dropset Linear Backlog's blocking edges in sync with file overlap. The whole job is deterministic and lives in a committed, dependency-free Python tool (`.claude/tools/sync_blockers.py`, run directly with `python3`): read the open Backlog, find every `**Touches**:` file-overlap collision with no declared blockedBy/blocks edge, and file a real `blocks` relation (lower ENG-### blocks higher) so Linear's native blocking icons reflect it. Two write modes — `--for ENG-###` (incremental, file-time: just the named issue vs. the backlog) and a bare full sweep (reconciliation) — plus a read-only `--report-todo-blocks` scan that flags any Todo-state issue blocking a Backlog issue (a scheduling smell). It never renders or writes a document, and never merges or closes issues. The filing skills call `--for` after `save_issue`; run the full sweep by hand to reconcile after backfilling a `**Touches**:` line on an older issue.
disable-model-invocation: false
user-invocable: true
---

# `sync-blockers`

Keep the Dropset Linear Backlog's **blocking edges** in
sync with file overlap. Agent-filed findings (`audit`)
and hand-filed to-dos (`linear-task`) land as plain
**Backlog** issues; two of them that edit the same files
can't run in parallel. This skill makes that constraint
visible where you actually read it — **Linear's native
`blocked`/`blocking` icons** — by materializing an
undeclared same-file collision into a real `blocks`
relation.

There is **no Task Staging document** anymore. Linear's
blocking icons are the source of truth for what gates
what; navigate the plan there. This skill's only job is
to keep those edges honest.

## Deterministic core: the sync-blockers Python tool

The whole job is pure mechanism — string/glob work plus a
couple of HTTP calls — so it lives in a committed,
dependency-free Python tool
(`.claude/tools/sync_blockers.py`, run directly with
`python3`) rather than being re-derived by hand. The tool:

- reads every **open** Backlog issue for the project (with
  its `**Touches**:` globs and its declared `blockedBy` /
  `blocks` edges);
- finds each pair whose touch-globs collide and that has
  **no** declared edge either direction;
- files a real `blocks` relation (lower `ENG-###` blocks
  higher) for each such collision — a genuine Linear write,
  so the blocking icon is durable rather than re-inferred.

It **only** writes `blocks` relations. It never renders a
document, never ranks anything, and never merges or closes
issues. The filing skills (`linear-task`, `audit`,
`audit-scope`) still set the real semantic dependencies at
file time; this only fills the gap for an undeclared
same-file collision.

### Two modes

**Incremental — `--for ENG-###` (the file-time path).**
Compares *only* the named, just-filed issue's touches
against the rest of the open Backlog and files its overlap
edges. Bounded work — one node vs. the backlog, not an N×N
re-scan — so each filing skill calls it right after
`save_issue`:

```sh
python3 .claude/tools/sync_blockers.py --for ENG-###
```

No race gap: if A then B are filed, B's file-time check
sees A and files the single symmetric edge; A's earlier
check simply didn't see B yet. The later filer always
covers the pair. Because the edge is maintained at file
time, no periodic run is required.

**Full sweep — bare (reconciliation).** Compares every
pair. No longer load-bearing cadence — run it by hand to
reconcile after backfilling a `**Touches**:` line on an
*older* issue, or as an occasional catch-up:

```sh
python3 .claude/tools/sync_blockers.py
```

Add `--dry-run` (either mode) to print the edges it *would*
file and write nothing:

```sh
python3 .claude/tools/sync_blockers.py --dry-run
```

**Report-only — `--report-todo-blocks`.** A read-only scan
(not a sweep — it files nothing) that surfaces a scheduling
smell: a **`Todo`-state issue blocking a `Backlog` issue**.
Per the Todo/Backlog convention initiatives / meta sit in
`Todo` and pullable work in `Backlog`, so a Todo blocker
gating a Backlog item means the pullable item can't
actually start. It prints the pairs as JSON —
`{todo_blocks_backlog: [{blocker, blocker_state, blocked}]}` — and
`housekeeping` drives it after the sweep, deciding what to do with each
pair. Cannot combine with `--for`:

```sh
python3 .claude/tools/sync_blockers.py --report-todo-blocks
```

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
any short stderr warnings). The Backlog read and the edge
writes happen in the tool's own process; nothing about the
backlog body is replayed across turns.

## The one manual touch: missing `**Touches**:`

An issue filed before the `**Touches**:` convention has no
file globs, so the tool can't check it for overlap and
prints a `warning:` naming it. The only optional manual
action is to backfill a `**Touches**:` line on such an issue
with a plain `save_issue` — a data fix — then run the full
sweep (or `--for` on that issue) so its edges get filed.

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
or pushes. Its only writes are to Linear: the overlap
`blocks` relations the tool materializes. It produces no
source diff of its own.

## Steps

**1. Preview (optional).** Run the tool in dry-run to see
the overlap edges a real sweep would file and the
missing-`Touches:` warnings:

```sh
python3 .claude/tools/sync_blockers.py --dry-run
```

The stderr output names every open issue with no
`**Touches**:` field and prints a `would file:` line for
each overlap edge a real run would materialize.

**2. Backfill missing `Touches:` (optional).** For each
issue the dry-run flagged as missing `**Touches**:`, add the
field via `save_issue` (id = that issue) — the path globs
its work will edit, comma-separated, per `CLAUDE.md` →
"Structured filing fields". Skip when nothing is flagged.

**3. Sweep.** Run the tool (no `--dry-run`):

```sh
python3 .claude/tools/sync_blockers.py
```

It reads the Backlog and files any undeclared overlap edges.
Idempotent — a pair already linked (declared, or filed
earlier) is skipped, so a relation is never duplicated.

**4. Report.** The tool prints its own one-line tally:

```txt
sync-blockers | <n> backlog issues | <k> overlap edges filed
```

(In `--for` mode the tally names the issue instead of the
count, e.g. `sync-blockers | ENG-663 | 1 overlap edge filed`.)

## Notes

- **Linear icons, not a document.** The blocking `blocks`
  relations this tool files show up as Linear's native
  `blocked`/`blocking` icons on each issue. There is no
  rendered tree, no `# Most blocking` ranking, and no
  document to read — navigate dependencies in Linear.
- **Edges are authoritative; an overlap collision is
  materialized into one.** A declared `blockedBy` / `blocks`
  edge is authoritative and suppresses an overlap edge for
  that pair. A `**Touches**:` collision between two Backlog
  issues with **no** declared edge either direction is
  turned into a real `blocks` relation (lower number blocks
  higher) — a genuine Linear write, so the constraint is
  durable rather than re-inferred each run.
- **No issue folding.** The tool never merges or closes
  issues; consolidation is a separate job, owned by
  `merge-tasks`.
- Shell discipline (per `CLAUDE.md`): every command is a
  single bare call that reduces to an allow-glob — no `&&`,
  pipes, `$(...)`, or redirects; content search routes to
  the Grep tool (never `git grep`).
