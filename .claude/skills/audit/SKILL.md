---
name: audit
description: One bounded platform-audit rotation, run once to completion — a fixed 7-unit pass that interleaves four randomly-chosen non-generated files (each audited via the `audit-scope` engine) with one randomly-chosen subsystem (internal-architecture lens), one randomly-chosen inter-subsystem interface (seam / contract-drift lens), and one repo-layout + spec-health pass, each adversarially cross-checked. Dedups against open or resolved Linear issues, files one self-contained Backlog issue per confirmed finding, syncs each new issue's file-overlap blocking edges via sync-blockers `--for`, announces, and stops. No loop, no finding cap, no re-invocation — run it again for another rotation.
disable-model-invocation: false
user-invocable: true
---

<!-- cspell:word landable -->

# `audit`

Run **one bounded audit rotation** and exit. A rotation is a fixed
sequence of **seven units** — four random files plus three structural
passes — each audited across the dimensions its subject calls for, with
adversarial cross-checking, filing one self-contained **Backlog** issue
per confirmed finding (no parent — the same no-parent Backlog
`linear-task` files into) so the work can be picked up in parallel
without blocking the repo. What gates what is recorded as native Linear
blocking edges; keeping those edges honest against file overlap is a
separate job, owned by `sync-blockers` (this skill calls it per finding
— see the File and Done steps).

Invoke it directly — `/audit` — when you want a fresh batch of findings.
It is **finite**: it runs the seven units once, files what they surface
(syncing each new issue's overlap edges as it goes), and **stops** with
a single `DONE` line. There is **no `/loop`, no finding cap, and no
re-invocation** — the rotation *is* the bound. To audit more, run
`/audit` again; each run is one independent rotation. `housekeeping`
runs exactly one rotation inline when invoked with its `audit` flag
(`/housekeeping audit`).

This skill sets `disable-model-invocation: false`, so it can be invoked
through the Skill tool directly (e.g. by `housekeeping`).

## The rotation

Seven units, run in order. **FILE is the connective tissue** — four of
the seven — so every rotation samples real code between the structural
passes:

```txt
1. FILE       — a random non-generated file        → audit-scope
2. SUBSYSTEM  — one random subsystem               → internal-architecture lens
3. FILE       — another random non-generated file  → audit-scope
4. INTERFACE  — one random inter-subsystem seam     → seam / contract-drift lens
5. FILE       — another random non-generated file  → audit-scope
6. LAYOUT     — repo-layout + spec-health pass      → one agent
7. FILE       — another random non-generated file  → audit-scope
→ per unit: cross-check, dedup, file findings, sync-blockers --for each
→ at the end: DONE
```

The structural units (2, 4, 6) each pick **one** subject **at random**,
not a fan-out over all of them — sampling one per rotation reaches the
tail over repeated runs, and keeps a rotation bounded (one agent each,
not ≈15). Run `/audit` again to sample a different subsystem, seam, and
the layout pass anew.

## How it relates to `audit-scope`

`audit-scope` is the shared audit engine: given a scope, it classifies
the platform kind, fans out the dimensions, and adversarially
cross-checks. The four FILE units delegate to it (FILE mode): each
self-selects one file, calls `audit-scope` to audit it, then hands the
findings back here to dedup and file. Run directly, `audit-scope` is
one-shot, plan-gated, and files its own findings; driven here it skips
the gate and returns findings for this skill to dedup and file. The
three structural units (SUBSYSTEM / INTERFACE / LAYOUT) are **not**
file-scoped, so they run their own single sub-agent here rather than
going through `audit-scope`.

## Read-only guarantee

This skill **never authors source edits and never writes to the
worktree**. Its only writes are the Linear issues it files and the
file-overlap `blocks` relations `sync-blockers` materializes for each
(via its `--for` incremental mode). It produces no source diff of its
own, so it must never commit or push. The one repo operation it does
perform is fast-forwarding the throwaway worktree to upstream `main` at
the **start of the rotation** (step 1) — that pulls in others' merged
work but introduces no change this skill wrote, and it never commits,
pushes, or force-updates.

**Where to run it.** Run `/audit` from a dedicated, throwaway worktree
you never commit in. Because the skill keeps **no local state** (see
below), the worktree stays pure scratch space while findings land in
the Dropset Linear Backlog.

## No local state — Linear is the source of truth

There is **no state directory** and no gitignored bookkeeping.
Everything the rotation needs lives in either **Linear** or the run's
working context:

- **The dedup set** is rebuilt **live from Linear** at the start of the
  rotation (step 1): the set of every `**Fingerprint**:` line on every
  Dropset issue, open or resolved. A finding whose fingerprint is
  already on an issue — in any state — is never refiled. Linear is the
  durable record, so a wiped or recreated worktree loses nothing.
- **The subsystem registry, the inter-subsystem interfaces, and the
  skip-globs** live in `docs/conventions/audit-registry.md` (committed,
  shared), refreshed on the PR path by `review-pr`. This skill
  **reads** them (step 1); it never edits them.

## Fingerprints (dedup keys)

A finding's `fingerprint` is its dedup key. Sub-agents return only a
`fingerprint_slug`; the skill derives the stored `fingerprint`
deterministically (no randomness, no line numbers, so the same issue
isn't refiled when surrounding code moves):

- **FILE findings.** The agent's `fingerprint_slug` is
  `<topic-slug>:<detail-slug>` (the *what*, with no file component).
  The stored fingerprint prepends the file's basename:
  `fingerprint = "<basename>:<fingerprint_slug>"`
  = `<basename>:<topic-slug>:<detail-slug>`, e.g.
  `swap.rs:slippage:no-min-out`. `<basename>` is the final path
  component including extension (`swap.rs`), **not** the full path — so
  a moved file keeps its key.
- **Structural findings** (SUBSYSTEM / INTERFACE / LAYOUT). There is no
  single basename, so the agent's `fingerprint_slug` is
  `<lens>:<topic-slug>` where `<lens>` is the subsystem name
  (`program`), the interface pair (`program-sdk-clients`), or the
  layout lens (`layout`, `docs`). The stored fingerprint is
  fixed-prefixed `arch:`:
  `fingerprint = "arch:<fingerprint_slug>"` = `arch:<lens>:<topic-slug>`,
  e.g. `arch:program-sdk-clients:idl-event-drift`. The `arch:` prefix
  is shared across all three structural units, so it stays continuous
  with fingerprints filed by earlier rotations.
- **Slugging.** Every `<…-slug>` is lowercased, with each run of
  non-alphanumeric characters collapsed to a single `-` and
  leading/trailing `-` trimmed, so the transform is deterministic and
  two passes over the same issue produce the same key.

## Steps

**1. Sync to main, rebuild the dedup set from Linear, and read the
registry.** First, fast-forward the throwaway worktree to upstream
`main` so the rotation audits current code. Run two bare commands (no
compound, no `$(…)`):

```sh
git fetch origin main
git merge --ff-only origin/main
```

If the fast-forward fails (the throwaway worktree has drifted), warn
and continue — never force or rebase; this skill never mutates source.

Then **rebuild the dedup set from Linear** — the durable record lives
on the issues themselves, so a wiped worktree recovers. List **all
Dropset-project issues** with `mcp__claude_ai_Linear__list_issues`
(same team / project IDs as the File step, `includeArchived: true`) and
keep those whose description carries a `**Fingerprint**:` line — those
are the audit-filed issues. For each kept issue, parse **every**
`**Fingerprint**:` line — a normal issue has one, but a **combined**
issue (one filed for several coupled findings) carries the **union** of
their fingerprints, so read them all — and note its current state
(resolved vs. open). The collected `{fingerprint → state}` map is this
rotation's dedup set (the Dedup step).

Then **read the Audit registry** from
`docs/conventions/audit-registry.md`: the **subsystems**
(`name (kind, risk): roots`), the **inter-subsystem interfaces**
(`A <-> B: contract`), and the **skip-globs**. These drive selection
for the FILE, SUBSYSTEM, and INTERFACE units. Read them with the Read
tool; do not edit them — `review-pr` maintains the registry on the PR
path.

**2. Run the seven-unit rotation.** Execute the units in order
(FILE, SUBSYSTEM, FILE, INTERFACE, FILE, LAYOUT, FILE), each per its
unit step below. As **each** unit's findings come back, run them
through the shared **Cross-check** (structural units only), **Dedup**,
**File**, and **Announce** steps before moving to the next unit — so
findings land promptly as the rotation proceeds. There is no
cap and no cadence counter: the seven units *are* the rotation. When
all seven are done, go to **Done**.

**Prepend the standing sub-agent brief from
`docs/conventions/sub-agent-brief.md`** to every sub-agent prompt the
structural units spawn (the lens agents and the cross-check agent).
They don't inherit the project instructions, and a structural pass
explores widely, so the brief's shell discipline (Read/Grep/Glob, one
bare globbable command per Bash call) is what keeps that exploration
from re-prompting. Don't narrow it — a structural unit is *meant* to
range over its subject.

**FILE unit (units 1, 3, 5, 7) — one randomly-chosen non-generated
file.** Selection is **random**, by design — there is no coverage
cursor to walk (no local state), so randomization is what reaches the
tail of every subsystem over repeated rotations. Vary your choice each
time; don't anchor the four FILE units on the same subsystem.

- **Pick a random subsystem** from the registry (step 1), leaning
  toward higher-`risk` subsystems but never excluding a low-risk one.
- **Pick a random eligible file within it.** Enumerate that
  subsystem's tracked files from its `roots` globs with `git ls-files`
  (one bare command per root, e.g. `git ls-files programs/dropset/src`),
  then drop any path matching a registry **skip-glob**.
- **Screen for generated content.** Read the first ~15 lines of the
  candidate (Read with `limit: 15`) for a generated marker
  (`@generated`, `DO NOT EDIT`, `Code generated by`, `AUTOGENERATED`)
  or binary / non-text content. If it matches, **do not audit it** —
  pick a different candidate. (This skill is read-only, so it does
  **not** persist a new skip-glob; that upkeep belongs to `review-pr`.
  If a whole family keeps getting screened out, note it so a skip-glob
  can be added on the PR path.)
- **Audit it via `audit-scope` (FILE mode).** Invoke the `audit-scope`
  skill (through the Skill tool) on the one file, in its **delegated**
  mode. It classifies the file's platform kind against the registry,
  runs the per-kind dimensions (security / comment accuracy /
  magic-numbers-DRY / modularity / hierarchical-organization / naming /
  doc-freshness), adversarially cross-checks, screens the findings
  against the linters (dropping anything `make lint` already catches,
  tagging a `**Lint**:` line on a class a linter *could* catch), and
  **returns the confirmed findings** — each with `file`, `line`,
  `dimension`, `severity` (high/med/low), `fingerprint_slug`, `title`,
  `rationale`, and `fix_sketch`. In delegated mode it does **not**
  file; this skill dedups and files. Because the cross-check ran inside
  `audit-scope`, a FILE unit's findings skip the **Cross-check** step
  and go straight to **Dedup**.

**SUBSYSTEM unit (unit 2) — one random subsystem, internal
architecture.** Pick **one** subsystem from the registry at random
(weight by `risk`, never exclude a low-risk one). Spawn **one**
sub-agent (with the sub-agent brief) that audits that subsystem's
**internal** architecture:

- layering & dependencies within it (a layer reaching into another's
  internals — e.g. the matcher knowing `Market`'s storage layout);
- paradigm & consistency (drift from its intended model — the on-chain
  eCLOB, the shared-math contract — divergent idioms, incoherent
  structure);
- invariant coverage (each invariant the subsystem relies on — the
  program's `I1`–`I6` share-accounting set, treasury-vs-vault, vault
  lifecycle — traced across its paths);
- internal duplication, and naming.

The agent returns findings with `path:line` anchors, severity, a
one-line rationale, and a `fingerprint_slug` shaped
`<subsystem>:<topic-slug>`. Then run the **Cross-check** step.

**INTERFACE unit (unit 4) — one random inter-subsystem seam.** Pick
**one** interface from the registry's interface list at random. Spawn
**one** sub-agent (with the sub-agent brief) that audits the **seam**,
since seams are where contract drift hides:

- schema / contract drift across the boundary (e.g. the on-chain
  `FillEvent` ↔ the IDL ↔ the generated clients; `sdk/math-core` ↔ the
  program's math ↔ the conformance vectors);
- end-to-end data-flow across the boundary;
- shared types / constants duplicated across the seam instead of
  shared.

The registry's interface entry names the seam and the contract that
crosses it. The agent returns findings with `path:line` anchors,
severity, rationale, and a `fingerprint_slug` shaped
`<interface-pair>:<topic-slug>` (e.g. `program-sdk-clients:…`). Then
run the **Cross-check** step.

**LAYOUT unit (unit 6) — repo layout + spec health.** Spawn **one**
sub-agent (with the sub-agent brief) for a repo-wide pass on
hierarchical organization and spec health — the remnant of the old
system-wide lenses:

- **Repo layout / hierarchical organization** — directory, module, and
  crate placement; a file or module in the wrong tree; orphaned or
  redundant directories; a helper that belongs elsewhere.
- **Spec health** of `docs/` — sections over-specified,
  under-specified, in the wrong document, or that should be split /
  merged. Read the specs (`architecture.md`, `interface.md`,
  `market-making-mvp.md`) as **subject matter, not just ground
  truth** — the specs are themselves in scope.

The agent returns findings with `path:line` anchors, severity,
rationale, and a `fingerprint_slug` shaped `layout:<topic-slug>` (for
an organization finding) or `docs:<topic-slug>` (for a spec-health
finding). Then run the **Cross-check** step.

**Cross-check (structural units only).** FILE findings were already
cross-checked inside `audit-scope`, so this applies to the SUBSYSTEM /
INTERFACE / LAYOUT findings: spawn a fresh skeptic sub-agent (brief it
with the same sub-agent brief) with the unit's findings. It must kill
false positives, challenge weak rationale, and surface anything the
first pass missed. On material disagreement, re-spawn the unit's lens
agent to defend or retract. Iterate at most 2 more rounds, then accept
the survivors. This is the primary noise gate before anything reaches
Linear.

**Dedup against live Linear.** For each surviving finding, compute its
`fingerprint` with the transform under **Fingerprints** above (prepend
the basename for FILE findings; `arch:` for structural findings; slug
deterministically). Then check it against the dedup set rebuilt from
Linear in step 1: if any Dropset issue — **open or resolved** —
already carries that fingerprint (checking **every** `**Fingerprint**:`
line, since a combined issue holds several), **skip
filing**. A resolved match means the finding was already triaged
(Done / Won't-fix / Canceled); refiling it would reopen settled noise.
Only findings that survive the check proceed.

**File one Linear issue per finding.** File exactly as the
`linear-task` skill does: a **plain Backlog issue with no parent**,
assigned to the configured assignee, into the shared destination.
There is **no umbrella issue** — the project Backlog is the queue, and
`sync-blockers` keeps its blocking edges honest against file overlap.
Resolve the destination
IDs from the environment exactly as `linear-task` does — never
hard-code them — with a bare `printenv` per variable (each reduces to
the same `Bash(printenv:*)` allow-rule):

```sh
printenv LINEAR_TEAM_ID
printenv LINEAR_PROJECT_ID
printenv LINEAR_ASSIGNEE_ID
```

Query each variable on its own line — macOS / BSD `printenv` honors
only its first operand, so a combined `printenv A B C` returns just `A`
and you'd wrongly conclude the rest are unset.

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

**Meta-work prefix.** When a finding's `**Touches**:` sit entirely under
the meta surface (`.claude/**`, `CLAUDE.md`, `docs/conventions/**`),
prepend the **`Claude:`** token to the title (per
`CLAUDE.md` → "Claude: meta-work prefix") so it reads
`Claude: <file>: <imperative fix>`. This composes with the `arch:`
fingerprint prefix above (a fingerprint-slug convention, not a title
one). A finding touching product / on-chain / SDK / frontend code gets
no prefix.

**Dependencies.** Set a `blockedBy` / `blocks` edge per the **Blocking
relations** brief in `docs/conventions/linear-automation.md` —
autonomous, so only on concrete evidence, never speculatively, and
coupling that belongs in **one PR** is the combined-issue case below,
not a relation. One audit-specific detail: a finding that carries a
`blockedBy` is **not** "safe to fix in isolation", so drop that body
line and replace it with `**Depends on**: <ENG-###> — <one line why>`
so the description doesn't contradict the relation.

**File obviously-coupled findings together up front.** If a unit
surfaced more than one finding that plainly belongs in the **same PR**
— same file or symbol, the work would obviously land as one change —
file them as **one combined Backlog issue** instead of several: one
title, the per-finding notes under per-source sub-headings, and a
`**Fingerprint**:` line for **each** finding (the union). Nothing
**merges or closes issues** for you, so coupled findings only
become one issue if you file them that way — combining at file time is
the only way. Findings that don't obviously share a PR stay separate;
`sync-blockers` then materializes any file-overlap between them into a
`blocks` edge (Linear's blocking icon).

The description must let a cold agent act on it in its own worktree
(literal newlines, not `\n`):

- `**File**: <path>:<line>` (clickable)
- `**Dimension**: <dimension>` / `**Severity**: <high|med|low>`
- `**What**:` the precise problem.
- `**Why it's safe to fix in isolation**:` it touches only this
  file/symbol and does not depend on other open findings — so it can
  land independently.
- `**Evidence**:` the offending snippet (+ the doc or comment it
  contradicts, for those dimensions).
- `**Fix sketch**:` the concrete suggested change.
- `**Lint**:` *(when applicable)* the lint rule or config that would
  catch this class going forward, per the linter-coverage screen in the
  FILE unit — so the fix prevents recurrence rather than being a
  one-off.
- `**Fingerprint**: <fingerprint>` — the exact dedup key (e.g.
  `swap.rs:slippage:no-min-out`). This line is **mandatory**: it is
  what makes dedup durable. Step 1 reads it back to rebuild the dedup
  set, so a wiped worktree recovers dedup state from Linear instead of
  refiling everything.
- `**Touches**: <glob>[, <glob>…]` — the machine-readable list of path
  globs the fix will edit, comma-separated (for a single-file nit, just
  that file). `sync-blockers` reads this to detect file collisions
  deterministically. **Mandatory** — see
  `docs/conventions/linear-automation.md` → "Structured filing fields".
- `**Discovered by**: audit <unit> @ <commit SHA>`

**Sync overlap edges for each finding as you file it.** Right after
`save_issue` returns a new identifier, file that issue's file-overlap
`blocks` edges against the open Backlog with the incremental sweep — one
bare command that reduces to the
`Bash(python3 .claude/tools/sync_blockers.py:*)` allow-rule (the
scan runs in the tool's own process, so nothing enters context):

```sh
python3 .claude/tools/sync_blockers.py --for <ENG-###>
```

Filing in `ENG-###` order means the later filer always sees the earlier
sibling, so an intra-rotation overlap pair is filed by the second of
the two — no end-of-rotation full sweep is needed. Best-effort: it needs
`LINEAR_API_KEY`; if unset the tool says so — note it and continue.

**Structural findings** (SUBSYSTEM / INTERFACE / LAYOUT) are filed the
same way (plain Backlog issue, same IDs, no parent) but as **one
detailed proposal issue each** — they are not atomically fixable, so
don't pretend otherwise. Don't include the "safe to fix in isolation"
line; use this body instead:

- `**Concern**:` what's structurally wrong and why it matters.
- `**Evidence**:` the files / instructions / spec sections involved,
  with `path:line` anchors across the codebase.
- `**Options**:` the candidate approaches with their trade-offs.
- `**Recommended**:` the approach you'd take, and why.
- `**Likely decomposition**:` a sketch of the smaller,
  independently-landable steps it splits into, so triage can break it
  up.
- `**Fingerprint**: <fingerprint>` — the `arch:<lens>:<topic-slug>`
  dedup key (mandatory, same role as for FILE findings).
- `**Touches**: <glob>[, <glob>…]` — the path globs the proposal's work
  would span (often several dirs for an `arch:` finding),
  comma-separated. `sync-blockers` reads it for collision detection.
  **Mandatory** — see `docs/conventions/linear-automation.md` →
  "Structured filing fields".
- `**Discovered by**: audit <unit> @ <commit SHA>`

Priority 3; these are proposals for the user to triage, not
pre-approved work. Title them by area, e.g.
`arch: decouple the matcher from Market storage layout`.

**Announce.** For each newly filed issue, print a prominent line:
`FILED ENG-### [<dimension>/<severity>] <subject> — <title>`. If at
least one **high-severity** issue was filed this rotation, send exactly
**one** `PushNotification` summarizing the top one — so a background or
inline run interrupts you only when it matters. If nothing was filed,
send no notification.

**Done.** After all seven units have been processed (each cross-checked
where applicable, deduped, filed, and its overlap edges synced via
`sync-blockers --for`), the rotation is complete. The blocking edges are
already current — the per-finding `--for` calls filed them at file time,
so there is no end-of-rotation sweep. Print a single final line and
**stop** — there is no re-invocation:

```txt
DONE audit | filed <t> (h/m/l) | deduped <d> | edges synced
```

To run another rotation later, just invoke `/audit` again — it samples
a different subsystem, seam, and layout pass, and four new random
files.

## Notes

- The skill is read-only with respect to source **and** the worktree
  (no local state). Its noise control is layered: adversarial
  cross-check (per structural unit, and inside `audit-scope` for
  files), live-Linear dedup, a bounded seven-unit rotation, and
  high-severity-only push notifications.
- Filing is autonomous by design. The project Backlog is the review
  queue; triage there, not at file time.
- The subsystems, interfaces, and skip-globs are not pinned in this
  skill — they live in `docs/conventions/audit-registry.md` and grow on
  the PR path (`review-pr`) as new subsystems, seams, and
  generated-file families appear.
- Shell discipline (per `docs/conventions/shell-commands.md`): every
  command here is a single bare call that reduces to an allow-glob — no
  `&&`, pipes, `$(...)`, or redirects; content search routes to the
  Grep tool (never `git grep`), per the sub-agent brief.
- To graduate this to a scheduled cloud routine later, first confirm
  the `claude.ai` Linear MCP authenticates in headless runs — if it
  doesn't, filing breaks and the skill is best left in an interactive
  session.
