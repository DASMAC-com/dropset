---
name: audit-loop
description: One iteration of a continuous background platform audit — pick a unit of work (a non-generated file, the oldest unaudited PR, or a whole-system architecture pass), audit files/PRs by delegating to the `audit-scope` engine and run whole-system lenses (layering / invariant-coverage / paradigm / data-flow / duplication / cross-platform-seams / spec-health) for architecture, all with an adversarial cross-check, dedup against prior findings and open or resolved Linear issues, then file one self-contained Backlog issue per finding and announce. Stops itself after 20 filings and hands staging to `stage-backlog`. Drive it with `/loop audit-loop`.
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
iteration closes out (step 11), begin the next; do
not `ScheduleWakeup`, sleep, or otherwise pace
between cycles. The skill self-terminates after it
has filed 20 findings (step 2) — that, not a timer,
is what ends the loop. Run it in a Claude Code
session separate from the one you develop in. The
skill itself contains **no** scheduling — it does
exactly one iteration per invocation.

This skill sets `disable-model-invocation: false`,
so the `/loop` harness can invoke it through the
Skill tool directly. (It was previously disabled,
which forced the loop to hand-execute the steps;
enabling invocation makes the `/loop` entry point
work as written.)

## How it relates to `audit-scope`

`audit-scope` is the shared audit engine: given a
scope, it classifies the platform kind, fans out the
dimensions, and adversarially cross-checks. This loop
is the unattended **driver** around it — it
self-selects scope one unit at a time, calls
`audit-scope` to audit that unit (FILE / PR mode),
then dedups the returned findings against its ledger
and files them, persisting coverage + dedup state
across iterations. Run directly, `audit-scope` is
one-shot, plan-gated, and files its own findings;
driven here it skips the gate and hands findings back
for the loop to dedup and file. ARCH mode keeps its
own whole-system lenses (step 5) — those aren't scoped
to one unit, so they don't go through `audit-scope`.

## Read-only guarantee

Like `audit-scope`, this loop **never authors
source edits**. The only writes it makes are the
gitignored `.audit-loop/` state and the Linear
issues it files. It produces no source diff of its
own, so it must never commit or push. The one repo
operation it does perform is fast-forwarding the
throwaway worktree to upstream `main` at the **start
of each iteration** (step 1) — that pulls in others'
merged work but introduces no change the loop wrote,
and it never commits, pushes, or force-updates.

**Where to run it.** Run `/loop audit-loop`
from a dedicated, throwaway worktree you never commit
in. The `.audit-loop/` state lives and accumulates
inside whatever worktree you run it from, and because
it is gitignored nothing is ever staged or pushed —
so the worktree stays pure scratch space while
findings land in the Dropset Linear Backlog.

## State

All state lives under `.audit-loop/` (gitignored).
Read and write it only with
the Read / Edit / Write tools — never shell it
through `jq` / `python` / `sed`.

- `.audit-loop/state.json` — coverage + cursor:

  ```json
  {
    "iteration": 0,
    "last_mode": "arch",
    "filed_total": 0,
    "covered": {
      "programs/dropset/src/instructions/swap.rs": {
        "audited_at": "2026-06-12",
        "commit_sha": "abc1234"
      }
    },
    "pr_cursor": { "last_pr_number": 0 }
  }
  ```

  A file is eligible for re-audit when it has never
  been covered, or its current last-commit **commit
  SHA** differs from the recorded one (it changed
  since the last audit). `filed_total` is the running
  count of findings this loop has filed across all
  iterations; the loop stops itself once it reaches
  20 (step 2). "commit SHA" / `commit_sha` is the one
  term this skill uses for a git commit hash — never
  "git sha".

- `.audit-loop/findings.json` — dedup ledger:

  ```json
  {
    "findings": [
      {
        "fingerprint": "swap.rs:slippage:no-min-out",
        "file": "programs/dropset/src/instructions/swap.rs",
        "dimension": "security",
        "linear_id": "ENG-512",
        "filed_at": "2026-06-12",
        "status": "open"
      }
    ]
  }
  ```

  This ledger is **not** the source of truth — it is
  a cache of Linear, rebuilt from Linear on startup
  (step 1) so a wiped/throwaway worktree never loses
  dedup state. `status` mirrors the linked Linear
  issue's state (`open` for any unresolved state;
  `closed` for Done / Won't-fix / Canceled / any
  resolved state) and is refreshed from Linear each
  iteration (step 1). A finding is deduped against on
  **either** status — a resolved finding must not be
  refiled.

  **Fingerprint format.** The `fingerprint` is the
  dedup key. Sub-agents return only a
  `fingerprint_slug`; the skill derives the stored
  `fingerprint` from it deterministically (no
  randomness, no line numbers, so the same issue
  isn't refiled when surrounding code moves):

  - **FILE / PR findings.** The agent's
    `fingerprint_slug` is `<topic-slug>:<detail-slug>`
    (the *what*, with no file component). The stored
    fingerprint prepends the file's basename:
    `fingerprint = "<basename>:<fingerprint_slug>"`
    = `<basename>:<topic-slug>:<detail-slug>`, e.g.
    `swap.rs:slippage:no-min-out`. `<basename>` is the
    final path component including extension
    (`swap.rs`), **not** the full path — so a moved
    file keeps its key.
  - **ARCH findings.** There is no single basename, so
    the synthesis agent's `fingerprint_slug` is
    `<lens>:<topic-slug>` where `<lens>` is the lens
    that surfaced it (`layering`, `invariant`,
    `paradigm`, `data-flow`, `duplication`,
    `cross-platform`, `spec-health`). The stored
    fingerprint is fixed-prefixed:
    `fingerprint = "arch:<fingerprint_slug>"`
    = `arch:<lens>:<topic-slug>`, e.g.
    `arch:layering:matcher-knows-market-layout`.
  - **Slugging.** Every `<…-slug>` is lowercased, with
    each run of non-alphanumeric characters collapsed
    to a single `-` and leading/trailing `-` trimmed,
    so the transform is deterministic and two passes
    over the same issue produce the same key.

- `.audit-loop/skip.txt` — the **living**
  autogenerated skip-list, one glob per line,
  seeded from the static list below and grown by
  the skill itself (see step 3).

- `.audit-loop/components.json` — the **discovered
  platform registry**: which components /
  architectures actually exist in this repo. Like
  the skip-list, it is *learned and updated*, not
  hardcoded in this skill — so the platform list
  isn't pinned in the prose and grows as the system
  does.

  ```json
  {
    "components": [
      {
        "name": "program",
        "kind": "solana-program",
        "roots": ["programs/dropset/src/**"],
        "risk": "high",
        "discovered_at": "2026-06-12"
      }
    ]
  }
  ```

  `kind` selects the per-platform security / paradigm
  checklist; `risk` weights file-mode selection. New
  components (an indexer, a Docker build, CI) are
  appended as they first appear.

## Steps

**1. Sync to main, ensure state exists, rebuild the
ledger from Linear, and refresh discovery.**
First, fast-forward the throwaway worktree to upstream
`main` so this iteration audits current code — this is
what makes even the **very first** iteration start
from the latest merged work, and what lets a file's
`commit_sha` change and re-trigger its audit after a
merge. Run two bare commands (no compound, no `$(…)`):

```sh
git fetch origin main
git merge --ff-only origin/main
```

If the fast-forward fails (the throwaway worktree has
drifted), warn and continue — never force or rebase;
this loop never mutates source.

Then glob for `.audit-loop/state.json`. If the
directory is missing, Write the four state files
with empty skeletons (`iteration: 0`,
`last_mode: "arch"`, `filed_total: 0`, empty
`covered`, `pr_cursor.last_pr_number: 0`; empty
`findings`; the static skip seed; empty
`components`). Read `.gitignore`; if it has no
`.audit-loop/` line, Edit it to add one (do not
`echo >>`).

Then **rebuild and refresh the dedup ledger from
Linear** — the ledger is a cache, and the durable
record lives on the issues themselves (so a wiped
worktree recovers). List **all Dropset-project
issues** with `mcp__claude_ai_Linear__list_issues`
(same team / project IDs as step 9,
`includeArchived: true`) and keep those whose
description carries a `**Fingerprint**:` line — those
are the audit-filed issues (step 9 writes the line
onto every one). For each kept issue, parse **every**
`**Fingerprint**:` line — a normal issue has one, but
an issue that `stage-backlog` merged carries the
**union** of its group's fingerprints, so read them
all — and read its current state. Reconcile into
`findings.json`:

- Any **fingerprint** (on any kept issue, including
  each one in a merged issue's union) that is
  **missing** from the ledger → add it, with the
  owning issue's `linear_id` and `status` =
  `closed` if that issue is in a resolved state
  (Done / Won't-fix / Canceled / Duplicate) else
  `open`.
- Any ledger entry that **does** have a `linear_id`
  → overwrite its `status` from that issue's live
  state, so a finding closed since the last pass is
  marked `closed` and won't be refiled.

This makes Linear the source of truth and the
local ledger a fast cache. Then **refresh the
component registry**: infer the repo's platforms
from its structure — top-level dirs and build
manifests (`Cargo.toml`, `package.json`,
`Dockerfile`, `docker-compose.yml`,
`.github/workflows/`) — and append any newly-seen
component to `components.json` with its `kind`,
`roots`, and a `risk` weighting. Like `skip.txt`,
this registry only grows; the skill never hardcodes
the platform list.

**2. Check the stop condition, then pick this
iteration's mode.** Read `state.json`. **If
`filed_total >= 20`, stop the loop**: do no auditing
this iteration and jump straight to the wind-down in
step 12 (hand staging to `stage-backlog` and
terminate). Otherwise advance a fixed
3-way rotation off `last_mode` so every job runs
regularly without randomness in the control flow:

| `last_mode`    | this iteration |
| -------------- | -------------- |
| `arch` / unset | `file`         |
| `file`         | `pr`           |
| `pr`           | `arch`         |

`arch` is the heaviest pass (it scans the whole
system); it fires once every three iterations.

**Dictionary hygiene is not part of this loop.** The
periodic `cspell-audit` check moved to the
`housekeeping` skill, which runs it read-only and
files the drift as Backlog tasks. Do **not** re-add a
cspell pass here — `audit-loop` audits source files,
PRs, and architecture; dictionary upkeep is
`housekeeping`'s job.

**3. FILE mode — one non-generated file (deterministic pick).**

- Read `.audit-loop/skip.txt`. Enumerate **all
  tracked source files repo-wide** with
  `git ls-files`, then drop any path matching a
  `skip.txt` glob. Discovery is repo-wide on
  purpose: a new platform (an `indexer/`, a
  `docker/` dir, `.github/` CI) is covered the
  moment it's committed — nothing here is pinned to
  a fixed set of roots.
- For each surviving candidate, screen the first
  ~15 lines (Read with `limit: 15`) for a generated
  marker (`@generated`, `DO NOT EDIT`,
  `Code generated by`, `AUTOGENERATED`) or binary /
  non-text content. If it matches, **do not audit
  it**: derive a durable glob for its family (the
  directory, e.g. `…/generated/**`, or the
  extension, e.g. `**/*.types.ts`), Edit `skip.txt`
  to append that glob if absent, note
  `skip.txt += <glob>` for the tally, and pick a
  different candidate.
- Choose the subject **deterministically** — a
  deterministic agent has no RNG, and step 2 already
  committed to control flow "without randomness", so
  selection must not say "at random". Rank the
  surviving candidates and take the first:
  1. **Never-covered files first** (absent from
     `covered`). Within this bucket, order by the
     `risk` of the component each file maps to in
     `components.json` (high before low; for this
     repo the on-chain program outranks the frontend,
     and trust boundaries in an indexer / backend if
     present), then by path lexicographically to
     break ties. The risk weighting orders the
     bucket; it is not a gate — every tracked source
     file is eligible.
  1. **Then changed-since-audit files** — those whose
     current `git log -1 --format=%H -- <path>` commit
     SHA differs from the recorded `commit_sha`.
     Order by least-recently-audited (oldest
     `audited_at` first), tie-broken by path.
  1. **Otherwise** (everything covered and unchanged),
     fall back to the least-recently-audited file
     overall (`audited_at` oldest first, tie-broken
     by path).
     This ordering is total and reproducible, so the
     tail of each bucket is reached instead of being
     starved by `git ls-files` ordering.
- The subject is that one file; go to step 6.

**4. PR mode — oldest unaudited PR (FIFO).**

All GitHub reads go through the **GitHub MCP**; this repo
is `DASMAC-com/dropset`, so every call takes
`owner: "DASMAC-com"`, `repo: "dropset"`.

- List merged PRs with
  `mcp__github__list_pull_requests`. There's no "merged"
  filter, so list **closed** PRs newest-first and keep
  only those with a non-null `merged_at`
  (a closed-unmerged PR has `merged_at: null`):

  ```txt
  mcp__github__list_pull_requests(
    owner: "DASMAC-com",
    repo: "dropset",
    state: "closed",
    sort: "updated",
    direction: "desc",
    perPage: 50,
  )
  ```

- Pick the **oldest** PR whose number exceeds
  `pr_cursor.last_pr_number` — i.e. the *minimum*
  `number` among those `> last_pr_number`, **not**
  the newest. Auditing oldest-first and advancing the
  cursor to exactly that PR's number (step 11) means
  no PR between the old cursor and the newest is ever
  skipped; PR mode walks the merge history in order.
  Note `perPage: 50` fetches the 50 *most-recently*
  updated closed PRs, so the oldest PR above the cursor
  must fall within that window for FIFO to see it. The
  cursor advances one PR per `pr` cycle, so a modest
  backlog stays in view — but if merges ever outrun
  the loop by more than ~50, raise `perPage` (or page
  with `page: 2`, …) so the oldest unaudited PR isn't
  missed.

- If **no** PR is newer than the cursor, there is
  nothing to audit in PR mode: treat this iteration
  as FILE mode instead (step 3), but in step 11 still
  set `last_mode: "pr"` (the *intended* mode, not the
  fallback). Recording the slot the rotation actually
  reached keeps the 3-way rotation turning so `arch`
  still fires next cycle — otherwise a PR slot that
  always falls back to file would trap the loop in
  file mode and `arch` would never run again.

- Fetch the diff with `mcp__github__pull_request_read`
  (`method: "get_diff"`; `method: "get_files"` gives the
  changed-file list if you want it separately):

  ```txt
  mcp__github__pull_request_read(
    owner: "DASMAC-com",
    repo: "dropset",
    pullNumber: <number>,
    method: "get_diff",
  )
  ```

  The subject is the non-generated files that PR touched,
  reviewed in the context of the diff. Go to step 6 (the
  audit shared with FILE mode).

**5. ARCH mode — holistic, whole-system audit.**
Unlike FILE / PR mode, this looks at the *whole*
system at once to catch cross-cutting issues no
single file reveals. Read the specs in `docs/`
(`architecture.md`, `interface.md`,
`market-making-mvp.md`) for intent — but treat them
as **subject matter, not just ground truth**: the
specs are themselves in scope. Spawn parallel
sub-agents, one per lens, **across the whole repo —
every component in `components.json`** (for this
repo: the on-chain program, the frontend, any
indexer / backend, infra such as Docker and CI, and
the docs). **Prepend the standing sub-agent brief from
`CLAUDE.md`** (→ "Briefing sub-agents") to each lens
prompt, and to the synthesis and cross-check agents
below — they don't inherit `CLAUDE.md`, and a
whole-system pass explores widely, so the brief's
shell discipline (Read/Grep/Glob, one bare globbable
command per Bash call) is what keeps that exploration
from re-prompting. Don't narrow it — ARCH mode is
*meant* to range over every component. The lenses
below use the program as the running example, but
apply each to whatever components the registry
lists:

- **Layering & dependencies** — dependency
  direction, leaky abstractions, a layer reaching
  into another's internals (e.g. the matcher knowing
  `Market`'s storage layout).
- **Invariant coverage** — take each invariant a
  component relies on (the program's documented
  `I1`–`I6` share-accounting set, treasury-vs-vault,
  vault lifecycle states; an indexer's
  exactly-once / dedup-key guarantees; any the code
  assumes but no doc states) and trace it across
  *every* relevant path; flag where it's enforced
  inconsistently or not at all.
- **Paradigm & consistency** — drift from each
  component's intended model (the on-chain eCLOB; an
  indexer's reorg / finality model), divergent idioms
  within a platform, structure that has grown
  incoherent — judged on its own merits, not only
  against the spec.
- **State ownership & data-flow** — who owns and
  mutates each piece of state, and whether that flow
  is coherent end to end, including across platform
  boundaries (on-chain → events → indexer → API).
- **Cross-module duplication / seams** — the same
  idea open-coded across modules, or a missing seam
  that forces shotgun edits.
- **Cross-platform seams** — coherence *between*
  platforms: does an indexer's decoded event schema
  still track the on-chain `FillEvent` / the
  `interface.md` contract? Are shared constants /
  types duplicated across platforms instead of
  shared? Do Docker / CI pin the same toolchain the
  program and build actually need?
- **Spec health** — the docs themselves: sections
  that are **over-specified** (detail that
  over-constrains or has already rotted),
  **under-specified** (behavior the code had to
  invent with no spec), content sitting in the wrong
  document, and specs that should be **split or
  merged** for a saner boundary. The spec is a
  first-class artifact to audit, not just a
  yardstick.

Then spawn a **synthesis** sub-agent that reconciles
the lenses into a small set of distinct
architecture-level findings — merging overlaps and
dropping anything that's really a single-file nit
(that's the file loop's job). Each carries a
`fingerprint_slug` shaped `<lens>:<topic-slug>` (the
lens that surfaced it, per the ARCH fingerprint
format above — no basename, since arch findings span
files), a `title`, and the proposal material step 9
files. Go to step 7 with these.

**6. Audit the unit via `audit-scope` (FILE / PR
mode).** ARCH mode does its own fan-out in step 5 and
skips here. Invoke the `audit-scope` skill (through
the Skill tool) on the subject — the one file, or the
PR's non-generated files reviewed in the context of
the diff — in its **delegated** mode. It classifies
the subject's platform kind against `components.json`,
runs the per-kind dimensions (security / comment
accuracy / magic-numbers-DRY / modularity /
hierarchical-organization / naming / doc-freshness),
adversarially cross-checks, screens the findings
against the linters (dropping anything `make lint`
already catches, tagging a `**Lint**:` line on a class
a linter *could* catch), and **returns the confirmed
findings** — each with `file`, `line`, `dimension`,
`severity` (high/med/low), `fingerprint_slug`,
`title`, `rationale`, and `fix_sketch`. In delegated
mode it does **not** file; this loop dedups and files
(steps 8–9). In PR mode, tell it the subject is in PR
context so the PR link rides along.

This is the abstraction boundary: the per-unit audit
engine lives in `audit-scope`, and the loop owns only
selection, dedup, filing, and state.

**7. Adversarial cross-check (ARCH mode).** FILE / PR
findings were already cross-checked inside
`audit-scope` (step 6), so this step applies to ARCH
mode's lens findings: spawn a fresh skeptic sub-agent
(brief it with the same `CLAUDE.md` sub-agent brief, per
step 5) with the collected findings and the subject. It
must kill false positives, challenge weak rationale, and
surface anything the first pass missed. On material
disagreement, re-spawn the relevant lens agent to
defend or retract. Iterate at most 2 more rounds, then
accept the survivors. This is the primary noise gate
before anything reaches Linear.

**8. Dedup (two layers).** For each surviving
finding, compute its `fingerprint` with the
transform defined under **State → Fingerprint
format** (prepend the basename for FILE / PR
findings; `arch:` for ARCH findings; slug
deterministically). Then, in order:

- **Layer 1 — ledger (any status).** If
  `findings.json` already has that fingerprint,
  **skip** — regardless of whether its `status` is
  `open` or `closed`. The ledger was rebuilt and
  status-refreshed from Linear in step 1, so a
  `closed` match means the finding was already filed
  and triaged (Done / Won't-fix / Canceled); refiling
  it would reopen settled noise. Match on status is
  *presence*, not openness.
- **Layer 2 — live Linear (open and resolved).**
  Otherwise query live Linear with
  `mcp__claude_ai_Linear__list_issues`, scoped to the
  Dropset project (same team / project IDs as step 9),
  **without** filtering to Backlog/open — include
  resolved states so a finding closed since the last
  ledger rebuild is still caught. Key the query by the
  same dimensions the fingerprint uses: for FILE / PR
  findings a `<basename + topic>` query; for ARCH
  findings a `<lens + topic>` query (there is no
  basename). If any project issue, **open or
  resolved**, already covers the same subject + topic —
  equivalently, carries a matching `**Fingerprint**:`
  line (check **every** fingerprint line, since a
  `stage-backlog`-merged issue holds several) — record
  its id and current status into `findings.json` and
  **skip filing**.

Only findings that survive both layers proceed.

**9. File one Linear issue per finding.** File
exactly as the `linear-task` skill does: a **plain
Backlog issue with no parent**, assigned to Alex, into
the shared destination. There is **no umbrella issue** —
the project Backlog is the queue, and `stage-backlog`
turns it into a PR plan later. Resolve the
destination IDs from the environment exactly as
`linear-task` does — never hard-code them — with a
bare `printenv` per variable (each reduces to the
same `Bash(printenv:*)` allow-rule):

```sh
printenv LINEAR_TEAM_ID
printenv LINEAR_PROJECT_ID
printenv LINEAR_ASSIGNEE_ID
```

Query each variable on its own line — macOS / BSD
`printenv` honors only its first operand, so a
combined `printenv A B C` returns just `A` and you'd
wrongly conclude the rest are unset.

Then call `mcp__claude_ai_Linear__save_issue` (no
`id`):

```txt
mcp__claude_ai_Linear__save_issue(
  team: "<$LINEAR_TEAM_ID>",
  project: "<$LINEAR_PROJECT_ID>",
  assignee: "<$LINEAR_ASSIGNEE_ID>",
  state: "Backlog",
  title: "<file>: <imperative fix, no trailing period>",
  description: "<markdown body, literal newlines>",
  priority: 3,  // 2 for high-severity security
  links: [{ url: "<pr-url>", title: "<pr-title>" }],  // PR mode only
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

The description must let a cold agent act on it in
its own worktree (literal newlines, not `\n`):

- `**File**: <path>:<line>` (clickable)
- `**Dimension**: <dimension>` / `**Severity**: <high|med|low>`
- `**What**:` the precise problem.
- `**Why it's safe to fix in isolation**:` it
  touches only this file/symbol and does not depend
  on other open findings — so it can land
  independently.
- `**Evidence**:` the offending snippet (+ the doc
  or comment it contradicts, for those dimensions).
- `**Fix sketch**:` the concrete suggested change.
- `**Lint**:` *(when applicable)* the lint rule or
  config that would catch this class going forward, per
  the linter-coverage screen in step 6 — so the fix
  prevents recurrence rather than being a one-off.
- `**Fingerprint**: <fingerprint>` — the exact dedup
  key (e.g. `swap.rs:slippage:no-min-out`). This line
  is **mandatory**: it is what makes dedup durable.
  Step 1 reads it back to rebuild the ledger, so a
  wiped/throwaway worktree recovers dedup state from
  Linear instead of refiling everything.
- `**Discovered by**: audit-loop iteration <n> @ <commit SHA>`

After each `save_issue`, append the finding
(`fingerprint`, `file`, `dimension`, `linear_id`,
`filed_at`, `status: "open"`) to `findings.json` and
increment `state.json`'s `filed_total` by one (Read
then Edit/Write) — `filed_total` drives the
stop-after-20 condition in step 2.

**ARCH-mode findings** are filed the same way (plain
Backlog issue, same IDs, no parent) but as **one
detailed proposal issue each** — they are not
atomically fixable, so don't pretend otherwise.
Don't include the "safe to fix in isolation" line;
use this body instead:

- `**Concern**:` what's structurally wrong and why
  it matters.
- `**Evidence**:` the files / instructions / spec
  sections involved, with `path:line` anchors across
  the codebase.
- `**Options**:` the candidate approaches with their
  trade-offs.
- `**Recommended**:` the approach you'd take, and
  why.
- `**Likely decomposition**:` a sketch of the
  smaller, independently-landable steps it splits
  into, so triage can break it up.
- `**Fingerprint**: <fingerprint>` — the `arch:<lens>:<topic-slug>`
  dedup key (mandatory, same role as for FILE / PR
  findings: step 1 rebuilds the ledger from it).
- `**Discovered by**: audit-loop iteration <n> @ <commit SHA>`

Priority 3; these are proposals for Alex to triage,
not pre-approved work. Title them by area, e.g.
`arch: decouple the matcher from Market storage layout`.

**10. Announce.** For each newly filed issue,
print a prominent line:
`FILED ENG-### [<dimension>/<severity>] <file> — <title>`.
If at least one **high-severity** issue was filed
this iteration, send exactly **one**
`PushNotification` summarizing the top one. If
nothing was filed, send no notification.

**11. Close out and continue.** Update
`state.json`:

- **FILE mode** — mark the subject covered, recording
  today's date as `audited_at` and its current
  `commit_sha`.
- **PR mode** — set `pr_cursor.last_pr_number` to
  **exactly the audited PR's number** (the oldest one
  picked in step 4), not the newest seen. Because
  step 4 picks the minimum number above the cursor,
  setting the cursor to that number leaves every
  later PR still `> cursor`, so none is skipped.
- **PR fallback** (no PR newer than the cursor, so
  this ran as FILE mode) — mark the audited file
  covered exactly as the FILE-mode branch above (it
  *was* a file audit, so record its `audited_at` and
  `commit_sha` or it'll be re-picked), leave
  `pr_cursor` unchanged, but set `last_mode: "pr"`
  anyway, so the rotation advances to `arch` next
  cycle and the PR slot can't trap the loop in file
  mode.
- ARCH mode records no per-file coverage (it scans
  the whole system, and dedup prevents refiling).

Set `last_mode` to this iteration's **intended** mode
(see the PR-fallback case above) and increment
`iteration`. `filed_total` was already incremented
per filing in step 9.

The next iteration's step 1 fast-forwards the worktree
to latest `main` before it audits, so a file's
`commit_sha` changes and re-triggers its audit after a
merge — this step does not sync again here.

Then print the tally:

```txt
iteration <n> | mode <file|pr|arch> | subject <…>
filed <k> (h/m/l) | deduped <d> | filed_total <t>/20 | skip.txt += <globs>
```

If `filed_total >= 20`, proceed to step 12 now
(don't wait for the next invocation). Otherwise stop
so `/loop` re-invokes immediately for the next
iteration — no timer, no wait.

**12. Wind down — hand staging to `stage-backlog` and
stop.** Reached only when `filed_total >= 20` (from
the step 2 gate, or directly from step 11 when the
20th filing just landed). This is the loop's terminal
step: it does no auditing, and it does **not** author
the PR plan itself — staging is `stage-backlog`'s job.

- **Invoke the `stage-backlog` skill** (via the Skill
  tool) to (re)stage the Backlog onto the
  **Task Staging** document: it reads every
  Backlog issue — including the ones this campaign just
  filed — groups them into the fewest parallel,
  file-disjoint PR sessions, merges the issues that
  belong in one PR, and rewrites the document. All the
  wave/session/merge logic lives there now, not here.

- **Stop the loop.** Print a final line —
  `DONE audit-loop | filed_total <t> | staged via stage-backlog`
  — and do **not** begin another iteration. The loop
  is complete; `/loop` should not re-invoke. (To run
  another audit campaign later, reset `filed_total`
  to 0 in `state.json`.)

## Autogenerated skip-list (self-updating)

Static seed for `.audit-loop/skip.txt`:

```txt
target/**
**/node_modules/**
Cargo.lock
**/pnpm-lock.yaml
**/package-lock.json
**/yarn.lock
**/*.gen.*
**/idl/**
target/types/**
frontend/lib/data/*.json
frontend/public/**
**/*.png
**/*.svg
**/*.min.*
.audits/**
.audit-loop/**
```

The eligible universe is **everything `git ls-files`
returns, minus the skip-list and the generated-file
content heuristic** — i.e. all hand-authored,
committed source, whatever platform it belongs to.
There is no allowlist of roots to maintain: a new
`indexer/`, `docker/`, or `.github/` tree is in
scope the moment it's committed.

The seed is only a starting point. Whenever step 3
catches a generated file the path globs missed
(via the content heuristic), the skill appends a
durable glob for that family to `skip.txt`, so the
list grows as new generated shapes appear and
future iterations skip them by path alone.

## Notes

- The skill is read-only with respect to source.
  Its noise control is layered: adversarial
  cross-check (step 7), two-layer dedup (step 8),
  one unit of work per iteration, and high-severity-
  only push notifications (step 10).
- Filing is autonomous by design — it must be, to
  run unattended. The project Backlog is the review
  queue; triage there, not at file time.
- Shell discipline (per `CLAUDE.md`): every command
  here is a single bare call that reduces to an
  allow-glob — no `&&`, pipes, `$(...)`, or
  redirects. Use Glob / Grep / Read for file
  discovery and slicing.
- To graduate this to a fully detached schedule
  (cron cloud routine) later, first confirm the
  `claude.ai` Linear MCP authenticates in headless
  runs — if it doesn't, filing breaks and the loop
  is best left in an interactive `/loop` session.
