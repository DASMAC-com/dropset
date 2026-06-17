---
name: stage-backlog
description: One iteration of keeping the Dropset Task Staging document in sync with the Linear Backlog — read every Backlog issue, group them into the fewest parallel, file-disjoint PR sessions, merge issues that belong in one PR into a single canonical issue (write-before-close so no state is dropped), adversarially cross-check the grouping, then rewrite the Task Staging document with live-status issue links. Open issues only; closed ones drop off. Drive it with `/loop stage-backlog` or run it once.
disable-model-invocation: false
user-invocable: true
---

# `stage-backlog`

Run **one iteration** of staging the Dropset Linear
Backlog onto the **Task Staging** document and exit.
Agent-filed findings (`audit-loop`) and hand-filed
to-dos (`linear-task`) all land as plain
**Backlog** issues with no parent. This skill is the
thing that turns that flat queue into a plan: it
groups the issues into the fewest parallel,
file-disjoint PRs, **merges issues that should be one
PR into one issue**, and rewrites the document so Alex
can see — at a glance, with live Linear statuses —
what can run in parallel right now.

It replaces the old umbrella-issue plan: there is no
ENG-452 anymore. The Backlog is the queue; this
document is the plan.

## Where the plan lives

The plan is the Linear document **Task Staging**. Its
id is **not** hard-coded here —
resolve it at run time from the environment, on the
same bare-`printenv` rule as the filing destination
(see "Filing destination" below):

```sh
printenv LINEAR_TASK_STAGING_DOC_ID
```

If it's empty, stop and tell the user to export it in
their shell profile (`~/.zshrc`); don't guess the id.

It is rewritten in full each run (`save_document` with
that id) — never appended to, so the skill is
idempotent and never stacks duplicates.

## Filing destination (shared with `linear-task`)

The same fixed destination every issue uses. Resolve
the IDs at run time from the environment exactly as
`linear-task` does — never hard-code them — with a
bare `printenv` per variable (each reduces to the
same `Bash(printenv:*)` allow-rule):

```sh
printenv LINEAR_TEAM_ID
printenv LINEAR_PROJECT_ID
printenv LINEAR_ASSIGNEE_ID
printenv LINEAR_TASK_STAGING_DOC_ID
```

| Field       | Env var                      |
| ----------- | ---------------------------- |
| Team        | `LINEAR_TEAM_ID`             |
| Project     | `LINEAR_PROJECT_ID`          |
| Assignee    | `LINEAR_ASSIGNEE_ID`         |
| Staging doc | `LINEAR_TASK_STAGING_DOC_ID` |

Query each variable on **its own** `printenv` line.
Do **not** combine them into one `printenv A B C`:
macOS / BSD `printenv` honors only its **first**
operand, so the combined form prints just the first
value and you'd wrongly conclude the rest are unset.

If any variable is empty, stop and tell the user to
export it in their shell profile (`~/.zshrc`).

## How it's driven

This skill is meant to run under the built-in loop
harness so the document stays continuously
cross-referenced against the Backlog:

```sh
/loop stage-backlog
```

Invoked with no interval, `/loop` re-invokes this skill
**continuously** — back-to-back, with **no timer or
wait between iterations**. As soon as one iteration
finishes (step 6), begin the next; do not
`ScheduleWakeup`, sleep, or otherwise pace between
cycles. The skill itself contains **no** scheduling —
it does exactly one iteration per invocation, and runs
just as well invoked once by hand to restage on
demand. Run it from a throwaway worktree you never
commit in; it never authors a source edit.

## Read-only with respect to source

This skill **never authors source edits** and never
commits or pushes. Its only writes are to Linear: it
merges Backlog issues (updating descriptions and
marking duplicates) and rewrites the **Task
Staging** document. It produces no source
diff of its own.

## Steps

**1. Read the Backlog.** List the Dropset project's
issues with `mcp__claude_ai_Linear__list_issues`
(same team / project IDs as above):

- the **open** queue — `state: "Backlog"` — is the work
  to stage;
- a second pass **including resolved states**
  (`includeArchived: true`) is read only for awareness
  (so a merge never picks a canonical issue that's
  already closed, and so already-merged duplicates are
  recognized).

For every issue, parse from its description:

- the **files it touches** — its `**File**: <path>:<line>`
  line(s) and, for arch proposals, the `path:line`
  anchors under `**Evidence**:`;
- **all** of its `**Fingerprint**:` lines. A normal
  issue has one; a previously-merged issue carries
  **several** (the union folded in by step 3). Keep the
  full set per issue — it's what makes the merge
  reversible-safe and what `audit-loop` reads back for
  dedup.

Also read each issue's **declared blocking relations** —
the native `blockedBy` and `blocks` edges Linear stores
on the issue. `list_issues` does **not** return relations,
so fetch them per issue with
`mcp__claude_ai_Linear__get_issue` passing
`includeRelations: true` (for the open issues being
staged). These are *authoritative* dependencies a human
or a filing skill asserted on purpose, distinct from any
ordering step 2 infers from overlapping files. Keep the
full set per issue — step 2 nests the tree on them and
step 3 carries them across a merge.

**2. Group into the fewest parallel, file-disjoint
sessions.** Arrange the open issues for **concurrent
Claude sessions** — the plan's whole purpose is to show
what can run in parallel, not just a linear order:

- **Sessions** are the unit of parallelism. Each session
  owns a **disjoint set of files**; sessions with
  non-overlapping file sets run **at the same time**,
  one Claude session each. Group issues into a session
  by the files they touch so two parallel sessions never
  edit the same file.

- **Items inside a session are serial** — they touch the
  same files, so they're ordered for one session to do
  in sequence.

- **Each session is one PR — compress to the fewest
  PRs.** A session maps to a single PR; its serial items
  are commits within that one PR, never a PR apiece.
  Fold every issue whose files fall in a session into
  that session (subject to the disjoint-file rule)
  rather than spinning up a new session. Don't merge
  issues whose file sets *don't* overlap just to cut the
  count, though — that would serialize work that could
  have run in parallel.

- **Honour dependencies.** A foundational fix others
  build on goes first and is flagged; an issue that
  defines a contract (doc/spec) precedes code that
  depends on it; an `arch:` proposal that subsumes
  single-file nits comes before those nits. A
  **declared** `blockedBy` / `blocks` relation (step 1)
  is the strongest signal of all: it's an authoritative
  edge, so honour it **even when the two sessions' file
  sets are disjoint** — a human or filing skill asserted
  the order on purpose, and unlike file-overlap it
  doesn't go away just because the work doesn't collide.

- **Express dependencies as a tree, not waves.** Don't
  group into numbered waves — that barriers a whole level
  behind the slowest item. Instead build a **dependency
  tree**: a session with no open blocker is a **top-level**
  node (ready to start now); a session blocked by another
  is a **child** nested under the single blocker it most
  directly follows. A child can start the moment its
  parent's PR merges — independent of the parent's
  siblings — which is the point of nesting over waves. A
  blocker is real when **any** of three holds: the two
  sessions' file sets collide (e.g. a DRY extraction over
  handlers a correctness fix is still editing), one
  defines a contract the other consumes, or one issue
  carries a **declared** `blockedBy` / `blocks` edge to
  the other (step 1). A declared edge nests regardless of
  files; absent all three — disjoint files, no contract,
  no declared edge — they're both top-level. When a
  session has **several**
  blockers across different branches, nest it under the
  last-to-settle one and note the others inline ("also
  after ENG-###"); a big cross-cutting refactor
  (`arch:` / slab-layout / de-fork) that touches nearly
  everything is the deepest leaf, run solo and last.

**3. Merge each multi-issue session into one canonical
issue — write-before-close so no state is ever
dropped.** A session that holds more than one issue
becomes a **single** Backlog issue (one PR = one
issue). For each such session:

- **Pick the canonical issue**: the lowest ENG number in
  the session (stable across reruns). Its priority
  becomes the **max** priority of the group.

- **First, fold everything into the canonical issue and
  save it.** Rewrite the canonical issue's description
  (`mcp__claude_ai_Linear__save_issue` with
  `id: "<canonical>"`) to contain every member's notes
  under a per-source sub-heading (e.g.
  `### From ENG-465`) **and a `**Fingerprint**:` line for
  every fingerprint across the whole group** (the union —
  one line each, deduped). In the **same** save, fold in
  the group's **declared blocking relations** (step 1):
  union every member's `blockedBy` / `blocks` edges onto
  the canonical issue (`blockedBy: [...]`, `blocks: [...]`
  — both append-only), so an edge a member carried isn't
  silently lost when that member becomes a Duplicate.
  Because a block is **symmetric** (member B `blockedBy`
  outsider X *is* the same edge as X `blocks` B), unioning
  each member's own `blockedBy` **and** `blocks` already
  captures every edge incident to the group — including
  ones pointing *at* a member from outside it, which land
  on the canonical as the surviving endpoint. Drop only
  the pairs whose **other** endpoint is *also* in the
  group (both halves of an intra-group edge) — they'd
  become self-edges once the members are one issue. (A
  stale edge left on the closed Duplicate itself is
  harmless: a resolved issue never blocks.) Confirm the save
  succeeded before touching any other issue. This ordering
  is the safety guarantee: if the run is interrupted here,
  the member issues still exist and still hold their own
  state and relations, so nothing is lost.

- **Then, and only then, close the other members.** For
  each non-canonical member, mark it a duplicate of the
  canonical issue:

  ```txt
  mcp__claude_ai_Linear__save_issue(
    id: "<member>",
    duplicateOf: "<canonical>",
  )
  ```

  This marks it Duplicate (a resolved state) and links it
  to the survivor — do it **after** the canonical save is
  confirmed, never before.

A single-issue session needs no merge — it already maps
to one PR.

**4. Adversarially cross-check the grouping.** The
grouping is the part most likely to be silently wrong,
and a bad grouping costs a merge conflict or a
needlessly serialized session. Spawn a fresh skeptic
sub-agent (`Agent` tool), **prepending the standing
sub-agent brief from `CLAUDE.md`** (→ "Briefing
sub-agents") to its prompt — the skeptic doesn't
inherit `CLAUDE.md`, so without the brief it reaches
for the `find` / `sed … | grep` / `cat` compounds that
re-prompt every run. Pass the drafted dependency tree,
each issue's files, **and each issue's declared
`blockedBy` / `blocks` edges** (step 1), **inline** in the
prompt (as the brief requires), so it never shells out to
re-fetch them. Tell it to hunt for:

- two **top-level** (or sibling) sessions that actually
  share a file — they'd conflict on merge, so one must
  nest under the other;
- a **spurious** nesting — a child whose files are
  disjoint from its parent's **and** that carries no
  declared `blockedBy` edge to it, so it was never blocked
  and should be top-level (ready to start now). A declared
  edge is authoritative: a disjoint-file child is **not**
  spurious when it declares the parent as a blocker;
- a dependency ordered backwards — a fix nested above the
  foundational change or contract (doc/spec) it relies on,
  or nested opposite to a declared `blockedBy` edge;
- a **missing** cross-branch blocker — a child with
  another open blocker not captured by its nesting or an
  "also after ENG-###" note, including a **declared**
  `blockedBy` edge the tree failed to nest on;
- over- or under-compression — issues split across PRs
  that should share one (per the minimal-PR rule), or
  disjoint issues merged into one session that needlessly
  serializes them.

Apply what survives; iterate at most 2 rounds, then
write the plan. If the cross-check forces a regrouping
that changes which issues merge, redo step 3 for the
affected sessions (write-before-close) before writing
the document.

**5. Rewrite the document.** Replace the **Task
Staging** document in full (replace `content`,
never append), passing the id resolved from
`LINEAR_TASK_STAGING_DOC_ID`:

```txt
mcp__claude_ai_Linear__save_document(
  id: "<$LINEAR_TASK_STAGING_DOC_ID>",
  content: "…",
)
```

Use literal newlines, not `\n`. Shape:

- A short **"How to read it"** preamble: each line is one
  issue = one PR; **start any top-level item now**; an
  indented item is blocked by the one it sits under —
  start it as soon as that parent's PR merges (its
  parent's siblings needn't be done); a trailing note
  flags any extra cross-branch blocker; delete a line once
  its PR lands.

- Then the **dependency tree** as a nested bullet list —
  one bullet per PR, no headings, no checkboxes. Indent a
  blocked session under its blocker (4 spaces per level):

  ```txt
  - ENG-### — <summary>. `<file globs>`. <"Absorbs ENG-###" note, if any>
      - ENG-### — <summary>. `<file globs>`. <why blocked; also after ENG-###>
  ```

- **Write every issue reference as the bare tag `ENG-###`
  in plaintext — never a markdown link.** Linear
  auto-resolves a bare identifier into a live issue
  mention that renders its current status (In Progress /
  Done / …); a `[ENG-###](url)` markdown link does not.
  This applies everywhere, including "Absorbs ENG-### …"
  and "also after ENG-### …" notes.

- The nesting **is** the ordering — don't add "Wave N"
  headings, "start now" / "after Wave 1" labels, or
  parallel/disjoint annotations. The dependency a child
  expresses is the nesting itself; only add a short why
  when it isn't self-evident from the tree. Distinguish
  the **kind** of blocker in that why: a **declared**
  `blockedBy` edge reads "blocked by ENG-### (declared)",
  an inferred one names the cause ("shares the withdraw
  handlers", "consumes the interface.md contract"). Use
  "also after ENG-###" when a second blocker isn't visible
  from the tree.

- No footer — drop the severity / compression summary
  lines; the issue tags carry severity, and the tree
  speaks for itself.

**Open issues only.** A closed / resolved issue (Done /
Won't-fix / Canceled / Duplicate) is **omitted
entirely** — never listed with a struck-through or
"closed" status. The plan shows only live remaining
work.

**6. Keep current, then stop.** Because step 1 reads the
Backlog fresh and step 5 regenerates the whole document
from **open** issues each run, the document
self-maintains: anything closed or merged since the last
run simply drops off the next time the loop comes
around. This also makes the document safe for Alex to
hand-trim — deleting a line for a task he can see has
closed will not be undone, because a closed issue is
excluded from regeneration anyway.

Print a tally and stop so `/loop` re-invokes immediately
(no timer, no wait):

```txt
stage-backlog | <b> backlog | <s> PRs | merged <m>→<k> | <t> top, <bl> blocked
```

When invoked once by hand (not under `/loop`), the same
single iteration runs and the skill simply exits.

## Notes

- **Reversible-safe merges.** Step 3's write-before-close
  ordering means the union of fingerprints **and declared
  relations** lands on the survivor before any member is
  closed, so `audit-loop` dedup (which reads every
  `**Fingerprint**:` line on every project issue) keeps
  recognizing a folded-in finding and never refiles it,
  and no `blockedBy` / `blocks` edge is dropped when a
  member becomes a Duplicate.
- **Relations are read, honoured, and preserved — never
  manufactured.** This skill treats a declared `blockedBy`
  / `blocks` edge as authoritative input to the tree
  (step 2) and carries edges across a merge (step 3), but
  it does **not** write its *inferred* nesting back as
  relations. File-overlap nesting ("these two can't run
  concurrently") is a scheduling artifact, not a true
  dependency, so persisting it as a Linear `blockedBy`
  would assert a blocker that doesn't really exist. The
  durable record of real dependencies is what the filing
  skills (`linear-task`, `audit-scope`, `audit-loop`) set
  at file time; the tree is this document's rendering of
  them, not their source.
- **No umbrella issue.** This skill, plus the plain
  Backlog and the document, fully replace the old ENG-452
  parent. Nothing here parents issues to anything.
- Shell discipline (per `CLAUDE.md`): every command is a
  single bare call that reduces to an allow-glob — no
  `&&`, pipes, `$(...)`, or redirects. Use Glob / Grep /
  Read for any file discovery.
- To graduate this to a fully detached schedule (cron
  cloud routine) later, first confirm the `claude.ai`
  Linear MCP authenticates in headless runs — if it
  doesn't, the document write breaks and the skill is
  best left in an interactive `/loop` session.
