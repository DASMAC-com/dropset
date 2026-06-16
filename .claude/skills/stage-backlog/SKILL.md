---
name: stage-backlog
description: One iteration of keeping the Dropset implementation-sequence document in sync with the Linear Backlog — read every Backlog issue, group them into the fewest parallel, file-disjoint PR sessions, merge issues that belong in one PR into a single canonical issue (write-before-close so no state is dropped), adversarially cross-check the grouping, then rewrite the implementation-sequence document with live-status issue links. Open issues only; closed ones drop off. Drive it with `/loop stage-backlog` or run it once.
disable-model-invocation: false
user-invocable: true
---

# `stage-backlog`

Run **one iteration** of staging the Dropset Linear
Backlog onto the **implementation-sequence document**
and exit. Agent-filed findings (`audit-loop`) and
hand-filed to-dos (`linear-task`) all land as plain
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

The plan is the Linear document **implementation-sequence**:

- URL: `https://linear.app/dasmac/document/implementation-sequence-9564c1e7cb56`
- id: `dbc36954-3269-4ea6-8651-c4d6ef5344bf`

It is rewritten in full each run (`save_document` with
that `id`) — never appended to, so the skill is
idempotent and never stacks duplicates.

## Filing destination (shared with `linear-task`)

The same fixed destination every Dropset issue uses —
use the IDs, not the names:

| Field    | Value       | ID                                     |
| -------- | ----------- | -------------------------------------- |
| Team     | Engineering | `84659a7c-5ea3-47b1-b2bd-c531e3721d6b` |
| Project  | Dropset     | `d505fe50-cc8b-41ca-be93-6215d9adcea0` |
| Assignee | Alex        | `b3ec6d9f-3c78-48da-8b4e-042176e8c579` |

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
marking duplicates) and rewrites the
implementation-sequence document. It produces no source
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
  recognised).

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
  single-file nits comes before those nits.

- **Keep the wave count minimal.** **Waves** are
  barriers — a later wave must not start until an earlier
  one merges. But waves are expensive to read and the top
  of the plan gets trimmed constantly as PRs land, so
  **default to a single wave** of parallel, file-disjoint
  sessions. Introduce a Wave 2 / 3 **only** when a
  genuine barrier exists — a later session would collide
  with files an earlier session must merge first (e.g. a
  DRY extraction over handlers a Wave-1 correctness fix
  is still editing, or a big cross-cutting refactor —
  `arch:` / slab-layout / de-fork — that touches nearly
  everything and must run solo in its own late wave).
  Never segment into waves for tidiness; deep wave
  numbering churns every time the front of the plan is
  trimmed.

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
  one line each, deduped). Confirm the save succeeded
  before touching any other issue. This ordering is the
  safety guarantee: if the run is interrupted here, the
  member issues still exist and still hold their own
  state, so nothing is lost.
- **Then, and only then, close the other members.** For
  each non-canonical member call
  `mcp__claude_ai_Linear__save_issue(id: "<member>",
  duplicateOf: "<canonical>")` — this marks it Duplicate
  (a resolved state) and links it to the survivor. Do
  this **after** the canonical save is confirmed, never
  before.

A single-issue session needs no merge — it already maps
to one PR.

**4. Adversarially cross-check the grouping.** The
grouping is the part most likely to be silently wrong,
and a bad grouping costs a merge conflict or a
serialized session. Spawn a fresh skeptic sub-agent
(`Agent` tool) with the drafted waves / sessions and
each issue's files, told to hunt for:

- two "parallel" sessions in the same wave that actually
  share a file (a hidden collision that would conflict on
  merge);
- a dependency ordered backwards — a fix placed before
  the foundational change or contract (doc/spec) it
  relies on;
- an issue in the wrong wave — one that could run a wave
  earlier, or one that will collide with file sets still
  in flight if it runs where it's placed;
- over- or under-compression — issues split across PRs
  that should share one (per the minimal-PR rule), or
  disjoint issues merged into one session that needlessly
  serializes them, or an unnecessary extra wave.

Apply what survives; iterate at most 2 rounds, then
write the plan. If the cross-check forces a regrouping
that changes which issues merge, redo step 3 for the
affected sessions (write-before-close) before writing
the document.

**5. Rewrite the document.** Replace the
implementation-sequence document in full:
`mcp__claude_ai_Linear__save_document(id:
"dbc36954-3269-4ea6-8651-c4d6ef5344bf", content: "…")`.
Use literal newlines, not `\n`. Shape:

- A short one-line **"How to read it"** preamble: each
  line is one issue = one PR; Wave = barrier (don't start
  a wave until the prior wave's overlapping PRs merge);
  open issues only, delete a line once its PR lands.
- Then the waves (usually just Wave 1), as a **plain
  bullet list** — one bullet per PR, no checkboxes, no
  per-session sub-blocks:

  ```txt
  ### Wave 1 — start now

  - ENG-### — <imperative summary>. `<file globs>`. <dependency / "Absorbs ENG-###…" note, if any>
  ...
  ```

- **Write every issue reference as the bare tag `ENG-###`
  in plaintext — never a markdown link.** Linear
  auto-resolves a bare identifier into a live issue
  mention that renders its current status (In Progress /
  Done / …); a `[ENG-###](url)` markdown link does not.
  This applies everywhere, including "Absorbs ENG-### …"
  notes.
- Don't annotate waves with how many sessions are
  parallel or that files are disjoint — that's the
  expected default and just adds noise. State a
  dependency only when one genuinely exists ("After
  ENG-###").
- Close with a one-line severity / compression footer.

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
stage-backlog | backlog <b> open | sessions <s> | merged <m> issues → <k> canonical | waves <w>
```

When invoked once by hand (not under `/loop`), the same
single iteration runs and the skill simply exits.

## Notes

- **Reversible-safe merges.** Step 3's write-before-close
  ordering means the union of fingerprints lands on the
  survivor before any member is closed, so `audit-loop`
  dedup (which reads every `**Fingerprint**:` line on
  every project issue) keeps recognising a folded-in
  finding and never refiles it.
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
