---
name: audit-loop
description: One iteration of a continuous background platform audit — pick a unit of work (a randomly-chosen non-generated file, or a whole-system architecture pass fanned out per subsystem and per inter-subsystem interface), audit files by delegating to the `audit-scope` engine and run the whole-system lenses with an adversarial cross-check, dedup against open or resolved Linear issues, then file one self-contained Backlog issue per finding, fold it into the Task Staging document via stage-backlog's incremental mode, and announce. Stops itself after a configurable number of filings (default 20). Drive it with `/loop audit-loop`.
disable-model-invocation: false
user-invocable: true
---

<!-- cspell:word landable -->

# `audit-loop`

Run **one iteration** of a continuous, unattended
platform audit and exit. Each iteration picks a
single unit of work, audits it across several
dimensions with adversarial cross-checking, and
files one self-contained **Backlog** issue per
confirmed finding (no parent — the same no-parent
Backlog `linear-task` files into) so the work can be
picked up in parallel without blocking the repo.
Staging those issues into a PR plan is a separate
job, owned by `stage-backlog`, not this loop.

This skill is meant to be driven by the built-in
loop harness:

```sh
/loop audit-loop
```

Invoked with no interval, `/loop` re-invokes this
skill **continuously** — back-to-back, with **no
timer or wait between iterations**. As soon as one
iteration closes out (step 10), begin the next; do
not `ScheduleWakeup`, sleep, or otherwise pace
between cycles. The skill self-terminates once it has
filed its **finding cap** (default 20 — step 2) —
that, not a timer, is what ends the loop. Run it in a
Claude Code session separate from the one you develop
in. The skill itself contains **no** scheduling — it
does exactly one iteration per invocation.

This skill sets `disable-model-invocation: false`,
so the `/loop` harness can invoke it through the
Skill tool directly.

## Input

Optional: a single **finding cap** (an integer), the
number of confirmed findings to file before the loop
self-terminates. **Defaults to 20.** `housekeeping`
passes it for the morning campaign so the yield is
tunable without editing this skill. Any other argument
is ignored.

## How it relates to `audit-scope`

`audit-scope` is the shared audit engine: given a
scope, it classifies the platform kind, fans out the
dimensions, and adversarially cross-checks. This loop
is the unattended **driver** around it — it
self-selects scope one unit at a time, calls
`audit-scope` to audit that unit (FILE mode), then
dedups the returned findings against Linear and files
them. Run directly, `audit-scope` is one-shot,
plan-gated, and files its own findings; driven here it
skips the gate and hands findings back for the loop to
dedup and file. WHOLE-SYSTEM mode keeps its own
fan-out (step 4) — those lenses aren't scoped to one
unit, so they don't go through `audit-scope`.

## Read-only guarantee

This loop **never authors source edits and never
writes to the worktree**. Its only writes are the
Linear issues it files and the **Task Staging**
document it folds each finding into incrementally as it
files (step 8, via `stage-backlog`). It produces no
source diff of its own, so it must never commit or
push. The one repo operation it does perform
is fast-forwarding the throwaway worktree to upstream
`main` at the **start of each iteration** (step 1) —
that pulls in others' merged work but introduces no
change the loop wrote, and it never commits, pushes,
or force-updates.

**Where to run it.** Run `/loop audit-loop` from a
dedicated, throwaway worktree you never commit in.
Because the loop keeps **no local state** (see below),
the worktree stays pure scratch space while findings
land in the Dropset Linear Backlog.

## No local state — Linear is the source of truth

There is **no `.audit-loop/` state directory** and no
gitignored bookkeeping. Everything the loop needs
across its back-to-back iterations lives in either
**Linear** or the loop's **working context** (the
`/loop` driver holds context between iterations):

- **The dedup set** is rebuilt **live from Linear**
  every iteration (step 1): the set of every
  `**Fingerprint**:` line on every Dropset issue,
  open or resolved. A finding whose fingerprint is
  already on an issue — in any state — is never
  refiled. Linear is the durable record, so a wiped
  or recreated worktree loses nothing.
- **The finding-cap counter** (`filed_total`) is
  tracked in working context across the continuous
  iterations, or re-derived from this campaign's filed
  issues. It drives the stop-after-cap condition
  (step 2).
- **The 5:1 cadence** (step 2) is likewise tracked in
  working context — how many FILE iterations have run
  since the last whole-system pass.
- **The subsystem registry, the inter-subsystem
  interfaces, and the skip-globs** live in `CLAUDE.md`
  → "Audit registry" (committed, shared), refreshed on
  the PR path by `review-pr`. This loop **reads** them
  (step 1); it never edits them.

## Fingerprints (dedup keys)

A finding's `fingerprint` is its dedup key.
Sub-agents return only a `fingerprint_slug`; the skill
derives the stored `fingerprint` deterministically (no
randomness, no line numbers, so the same issue isn't
refiled when surrounding code moves):

- **FILE findings.** The agent's `fingerprint_slug` is
  `<topic-slug>:<detail-slug>` (the *what*, with no
  file component). The stored fingerprint prepends the
  file's basename:
  `fingerprint = "<basename>:<fingerprint_slug>"`
  = `<basename>:<topic-slug>:<detail-slug>`, e.g.
  `swap.rs:slippage:no-min-out`. `<basename>` is the
  final path component including extension
  (`swap.rs`), **not** the full path — so a moved file
  keeps its key.
- **WHOLE-SYSTEM findings.** There is no single
  basename, so the synthesis agent's `fingerprint_slug`
  is `<lens>:<topic-slug>` where `<lens>` is the
  subsystem, the interface pair, or the global lens
  that surfaced it (e.g. `program`,
  `program-sdk-clients`, `spec-health`). The stored
  fingerprint is fixed-prefixed:
  `fingerprint = "arch:<fingerprint_slug>"`
  = `arch:<lens>:<topic-slug>`, e.g.
  `arch:program-sdk-clients:idl-event-drift`.
- **Slugging.** Every `<…-slug>` is lowercased, with
  each run of non-alphanumeric characters collapsed to
  a single `-` and leading/trailing `-` trimmed, so the
  transform is deterministic and two passes over the
  same issue produce the same key.

## Steps

**1. Sync to main, rebuild the dedup set from Linear,
and read the registry.** First, fast-forward the
throwaway worktree to upstream `main` so this iteration
audits current code. Run two bare commands (no
compound, no `$(…)`):

```sh
git fetch origin main
git merge --ff-only origin/main
```

If the fast-forward fails (the throwaway worktree has
drifted), warn and continue — never force or rebase;
this loop never mutates source.

Then **rebuild the dedup set from Linear** — the
durable record lives on the issues themselves, so a
wiped worktree recovers. List **all Dropset-project
issues** with `mcp__claude_ai_Linear__list_issues`
(same team / project IDs as step 8,
`includeArchived: true`) and keep those whose
description carries a `**Fingerprint**:` line — those
are the audit-filed issues (step 8 writes the line onto
every one). For each kept issue, parse **every**
`**Fingerprint**:` line — a normal issue has one, but
an issue that `stage-backlog` merged carries the
**union** of its group's fingerprints, so read them
all — and note its current state (resolved vs. open).
The collected `{fingerprint → state}` map is this
iteration's dedup set (step 7).

Then **read the Audit registry** from `CLAUDE.md` →
"Audit registry": the **subsystems**
(`name (kind, risk): roots`), the **inter-subsystem
interfaces** (`A <-> B: contract`), and the
**skip-globs**. These drive selection (step 3) and the
whole-system fan-out (step 4). Read them with the Read
tool; do not edit them — `review-pr` maintains the
registry on the PR path.

**2. Check the stop condition, then pick this
iteration's mode.** Resolve the finding cap from the
argument (default 20). **If `filed_total >= cap`, stop
the loop**: do no auditing this iteration and jump
straight to the wind-down (step 11). Otherwise pick the
mode on a **5:1 cadence** — five **FILE** iterations
for every one **WHOLE-SYSTEM** iteration:

- If five FILE iterations have run since the last
  whole-system pass (tracked in working context), run a
  **WHOLE-SYSTEM** iteration (step 4) and reset the
  cadence counter.
- Otherwise run a **FILE** iteration (step 3) and
  increment the cadence counter.

A fresh campaign starts on FILE, so the first
whole-system pass lands on the sixth iteration. This is
about the *ratio*, not the number of files per pick —
each FILE iteration audits exactly one file.

**Dictionary hygiene is not part of this loop.** The
periodic `cspell-audit` check lives in `housekeeping`
(opt-in), which runs it read-only and files the drift
as Backlog tasks. Do **not** re-add a cspell pass here.

**3. FILE mode — one randomly-chosen non-generated
file.** Selection is **random**, by design — there is
no coverage cursor to walk (the loop keeps no local
state), so randomization is what reaches the tail of
every subsystem over many iterations. Vary your choice
run to run; don't anchor on the same subsystem or file.

- **Pick a random subsystem** from the registry (step
  1), leaning toward higher-`risk` subsystems but never
  excluding a low-risk one. The whole-system pass
  (step 4) is the *comprehensive* one; FILE mode is the
  *random* sampler.
- **Pick a random eligible file within it.** Enumerate
  that subsystem's tracked files from its `roots`
  globs with `git ls-files` (one bare command per
  root, e.g. `git ls-files programs/dropset/src`), then
  drop any path matching a registry **skip-glob**.
- **Screen for generated content.** Read the first ~15
  lines of the candidate (Read with `limit: 15`) for a
  generated marker (`@generated`, `DO NOT EDIT`,
  `Code generated by`, `AUTOGENERATED`) or binary /
  non-text content. If it matches, **do not audit it** —
  pick a different candidate. (The loop is read-only, so
  it does **not** persist a new skip-glob; that upkeep
  belongs to `review-pr`. If a whole family keeps
  getting screened out, note it so a skip-glob can be
  added on the PR path.)
- The subject is that one file; go to step 5.

**4. WHOLE-SYSTEM mode — fan out per subsystem and per
interface.** Unlike FILE mode, this audits the *whole*
system at once to catch cross-cutting issues no single
file reveals, and its coverage is **comprehensive**,
not random: it visits **every** subsystem and **every**
interface in the registry. Read the specs in `docs/`
(`architecture.md`, `interface.md`,
`market-making-mvp.md`) for intent — but treat them as
**subject matter, not just ground truth**: the specs
are themselves in scope.

**Prepend the standing sub-agent brief from
`CLAUDE.md`** (→ "Briefing sub-agents") to every agent
prompt below — the lens agents, the synthesis agent,
and the cross-check agent (step 6). They don't inherit
`CLAUDE.md`, and a whole-system pass explores widely, so
the brief's shell discipline (Read/Grep/Glob, one bare
globbable command per Bash call) is what keeps that
exploration from re-prompting. Don't narrow it — this
pass is *meant* to range over every subsystem.

Spawn parallel sub-agents (single message, multiple
`Agent` calls):

- **One agent per subsystem** — audits that subsystem's
  **internal** architecture: layering & dependencies
  within it (a layer reaching into another's internals,
  e.g. the matcher knowing `Market`'s storage layout),
  paradigm & consistency (drift from its intended model
  — the on-chain eCLOB, the shared-math contract —
  divergent idioms, incoherent structure), invariant
  coverage (each invariant the subsystem relies on —
  the program's `I1`–`I6` share-accounting set,
  treasury-vs-vault, vault lifecycle — traced across its
  paths), internal duplication, and naming.
- **One agent per inter-subsystem interface** — audits
  the **seam**, since seams are where issues commonly
  arise: schema / contract drift across the boundary
  (e.g. the on-chain `FillEvent` ↔ the IDL ↔ the
  generated clients; `sdk/math-core` ↔ the program's
  math ↔ the conformance vectors), end-to-end data-flow
  across the boundary, and shared types / constants
  duplicated across the seam instead of shared. The
  registry's interface list names each seam and the
  contract that crosses it.
- **Genuinely system-wide lenses** that belong to
  neither one subsystem nor one pair — e.g. **spec
  health** of `docs/` (sections over-specified,
  under-specified, in the wrong document, or that should
  be split / merged) — run as their own agent(s).

Each agent returns findings with `path:line` anchors,
severity, and a one-line rationale.

Then spawn a **synthesis** sub-agent that reconciles
all the agents' findings into a small set of distinct
architecture-level findings — merging overlaps and
dropping anything that's really a single-file nit
(that's the FILE loop's job). Each carries a
`fingerprint_slug` shaped `<lens>:<topic-slug>` (the
subsystem, interface pair, or global lens that surfaced
it, per the whole-system fingerprint format above — no
basename, since these span files), a `title`, and the
proposal material step 8 files. Go to step 6 with these.

This fan-out scales with the registry (≈ one agent per
subsystem + one per interface + a few global) — bounded
by the registry, comparable to a fixed set of lenses.

**5. Audit the file via `audit-scope` (FILE mode).**
WHOLE-SYSTEM mode does its own fan-out in step 4 and
skips here. Invoke the `audit-scope` skill (through the
Skill tool) on the one file, in its **delegated** mode.
It classifies the file's platform kind against the
registry, runs the per-kind dimensions (security /
comment accuracy / magic-numbers-DRY / modularity /
hierarchical-organization / naming / doc-freshness),
adversarially cross-checks, screens the findings against
the linters (dropping anything `make lint` already
catches, tagging a `**Lint**:` line on a class a linter
*could* catch), and **returns the confirmed findings** —
each with `file`, `line`, `dimension`, `severity`
(high/med/low), `fingerprint_slug`, `title`, `rationale`,
and `fix_sketch`. In delegated mode it does **not** file;
this loop dedups and files (steps 7–8).

This is the abstraction boundary: the per-file audit
engine lives in `audit-scope`, and the loop owns only
selection, dedup, and filing.

**6. Adversarial cross-check (WHOLE-SYSTEM mode).** FILE
findings were already cross-checked inside `audit-scope`
(step 5), so this step applies to the whole-system lens
findings: spawn a fresh skeptic sub-agent (brief it with
the same `CLAUDE.md` sub-agent brief, per step 4) with
the collected findings. It must kill false positives,
challenge weak rationale, and surface anything the first
pass missed. On material disagreement, re-spawn the
relevant lens agent to defend or retract. Iterate at
most 2 more rounds, then accept the survivors. This is
the primary noise gate before anything reaches Linear.

**7. Dedup against live Linear.** For each surviving
finding, compute its `fingerprint` with the transform
under **Fingerprints** above (prepend the basename for
FILE findings; `arch:` for whole-system findings; slug
deterministically). Then check it against the dedup set
rebuilt from Linear in step 1: if any Dropset issue —
**open or resolved** — already carries that fingerprint
(checking **every** `**Fingerprint**:` line, since a
`stage-backlog`-merged issue holds several), **skip
filing**. A resolved match means the finding was already
triaged (Done / Won't-fix / Canceled); refiling it would
reopen settled noise. Because step 1 rebuilds the set
live every iteration, this single check is current even
for findings closed since the last pass.

Only findings that survive the check proceed.

**8. File one Linear issue per finding.** File exactly
as the `linear-task` skill does: a **plain Backlog issue
with no parent**, assigned to the configured assignee,
into the shared destination. There is **no umbrella
issue** — the project Backlog is the queue, and
`stage-backlog` turns
it into a PR plan later. Resolve the destination IDs
from the environment exactly as `linear-task` does —
never hard-code them — with a bare `printenv` per
variable (each reduces to the same `Bash(printenv:*)`
allow-rule):

```sh
printenv LINEAR_TEAM_ID
printenv LINEAR_PROJECT_ID
printenv LINEAR_ASSIGNEE_ID
```

Query each variable on its own line — macOS / BSD
`printenv` honors only its first operand, so a combined
`printenv A B C` returns just `A` and you'd wrongly
conclude the rest are unset.

Then call `mcp__claude_ai_Linear__save_issue` (no `id`):

```txt
mcp__claude_ai_Linear__save_issue(
  team: "<$LINEAR_TEAM_ID>",
  project: "<$LINEAR_PROJECT_ID>",
  assignee: "<$LINEAR_ASSIGNEE_ID>",
  state: "Backlog",
  title: "<file>: <imperative fix, no trailing period>",
  description: "<markdown body, literal newlines>",
  priority: 3,  // 2 for high-severity security
  blockedBy: ["<ENG-###>"]  // omit unless a real dependency (see below)
)
```

**Dependencies.** Set a `blockedBy` / `blocks` edge per
the **Blocking relations** brief in `CLAUDE.md`
(→ "Linear automation") — autonomous, so only on
concrete evidence, never speculatively, and coupling
that belongs in **one PR** is the combined-issue case
below, not a relation. One audit-loop-specific detail: a
finding that carries a `blockedBy` is **not** "safe to
fix in isolation", so drop that body line and replace it
with `**Depends on**: <ENG-###> — <one line why>` so the
description doesn't contradict the relation.

**File obviously-coupled findings together up front.**
If this iteration surfaced more than one finding that
plainly belongs in the **same PR** — same file or
symbol, the work would obviously land as one change —
file them as **one combined Backlog issue** instead of
several: one title, the per-finding notes under
per-source sub-headings, and a `**Fingerprint**:` line
for **each** finding (the union). You already hold the
context here, so combining now saves `stage-backlog`
from re-deriving the grouping and re-merging later.
Count each fingerprint toward `filed_total`. Findings
that don't obviously share a PR stay separate — let
`stage-backlog` decide those.

The description must let a cold agent act on it in its
own worktree (literal newlines, not `\n`):

- `**File**: <path>:<line>` (clickable)
- `**Dimension**: <dimension>` / `**Severity**: <high|med|low>`
- `**What**:` the precise problem.
- `**Why it's safe to fix in isolation**:` it touches
  only this file/symbol and does not depend on other
  open findings — so it can land independently.
- `**Evidence**:` the offending snippet (+ the doc or
  comment it contradicts, for those dimensions).
- `**Fix sketch**:` the concrete suggested change.
- `**Lint**:` *(when applicable)* the lint rule or
  config that would catch this class going forward, per
  the linter-coverage screen in step 5 — so the fix
  prevents recurrence rather than being a one-off.
- `**Fingerprint**: <fingerprint>` — the exact dedup key
  (e.g. `swap.rs:slippage:no-min-out`). This line is
  **mandatory**: it is what makes dedup durable. Step 1
  reads it back to rebuild the dedup set, so a wiped
  worktree recovers dedup state from Linear instead of
  refiling everything.
- `**Discovered by**: audit-loop iteration <n> @ <commit SHA>`

After each `save_issue`, increment the in-context
`filed_total` by one — it drives the stop-after-cap
condition in step 2 — and **fold the new issue into the
Task Staging document incrementally**: invoke the
`stage-backlog` skill (via the Skill tool) in its
**incremental** mode, passing the just-filed `ENG-###`
id(s) (the whole union, when you filed a combined
issue). This keeps the plan roughly current as findings
land, instead of waiting for one heavy re-stage at the
end. It is best-effort chip placement, not a global
regroup — the next full `stage-backlog` reconciles
(step 11).

**WHOLE-SYSTEM findings** are filed the same way (plain
Backlog issue, same IDs, no parent) but as **one
detailed proposal issue each** — they are not atomically
fixable, so don't pretend otherwise. Don't include the
"safe to fix in isolation" line; use this body instead:

- `**Concern**:` what's structurally wrong and why it
  matters.
- `**Evidence**:` the files / instructions / spec
  sections involved, with `path:line` anchors across the
  codebase.
- `**Options**:` the candidate approaches with their
  trade-offs.
- `**Recommended**:` the approach you'd take, and why.
- `**Likely decomposition**:` a sketch of the smaller,
  independently-landable steps it splits into, so triage
  can break it up.
- `**Fingerprint**: <fingerprint>` — the
  `arch:<lens>:<topic-slug>` dedup key (mandatory, same
  role as for FILE findings: step 1 rebuilds the dedup
  set from it).
- `**Discovered by**: audit-loop iteration <n> @ <commit SHA>`

Priority 3; these are proposals for the user to triage, not
pre-approved work. Title them by area, e.g.
`arch: decouple the matcher from Market storage layout`.

**9. Announce.** For each newly filed issue, print a
prominent line:
`FILED ENG-### [<dimension>/<severity>] <file> — <title>`.
If at least one **high-severity** issue was filed this
iteration, send exactly **one** `PushNotification`
summarizing the top one — ideal for the background
morning campaign, so you are interrupted only when it
matters. If nothing was filed, send no notification.

**10. Close out and continue.** Update the in-context
counters: `filed_total` was already incremented per
filing in step 8; advance the iteration count and the
5:1 cadence counter (reset it after a whole-system
pass). No per-file coverage and no cursor are
recorded — there is no local state, and dedup (step 7)
prevents refiling. The next iteration's step 1
fast-forwards the worktree to latest `main` before it
audits, so changed code is re-eligible after a merge.

Then print the tally:

```txt
iteration <n> | mode <file|whole-system> | subject <…>
filed <k> (h/m/l) | deduped <d> | filed_total <t>/<cap>
```

If `filed_total >= cap`, proceed to step 11 now (don't
wait for the next invocation). Otherwise stop so `/loop`
re-invokes immediately for the next iteration — no
timer, no wait.

**11. Wind down and stop.** Reached only when
`filed_total >= cap` (from the step 2 gate, or directly
from step 10 when the capping filing just landed). This
is the loop's terminal step: it does no auditing.

The Task Staging document was kept roughly current
**incrementally** as each finding was filed (step 8), so
the loop does **not** run a heavy full re-stage here.
The authoritative reconcile — a full `stage-backlog`
that re-derives the grouping and merges the issues that
belong in one PR — is the **next `housekeeping` morning
pass**'s job (or a manual `/stage-backlog` on demand);
incremental placement is the fast in-between, and the
full pass is the source of truth.

- **Stop the loop.** Print a final line —
  `DONE audit-loop | filed_total <t> | staged incrementally`
  — and do **not** begin another iteration. The loop is
  complete; `/loop` should not re-invoke. To run another
  campaign later, just invoke `/loop audit-loop` again —
  `filed_total` starts fresh in working context.

## Notes

- The skill is read-only with respect to source **and**
  the worktree (no local state). Its noise control is
  layered: adversarial cross-check (step 6), live-Linear
  dedup (step 7), one unit of work per iteration, and
  high-severity-only push notifications (step 9).
- Filing is autonomous by design — it must be, to run
  unattended. The project Backlog is the review queue;
  triage there, not at file time.
- The subsystems, interfaces, and skip-globs are not
  pinned in this skill — they live in `CLAUDE.md` →
  "Audit registry" and grow on the PR path (`review-pr`)
  as new subsystems, seams, and generated-file families
  appear. A new `indexer/`, `docker/`, or `.github/`
  tree becomes auditable the moment `review-pr` adds it
  to the registry.
- Shell discipline (per `CLAUDE.md`): every command here
  is a single bare call that reduces to an allow-glob —
  no `&&`, pipes, `$(...)`, or redirects; content search
  routes to the Grep tool (never `git grep`), per the
  sub-agent brief.
- To graduate this to a fully detached schedule (cron
  cloud routine) later, first confirm the `claude.ai`
  Linear MCP authenticates in headless runs — if it
  doesn't, filing breaks and the loop is best left in an
  interactive `/loop` session.
