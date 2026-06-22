---
name: audit-scope
description: Audit a defined scope — one file, a PR's files, a subsystem, or the whole codebase — across the dimensions its platform kind calls for (security, comment accuracy, DRY, modularity, naming, doc-freshness), with adversarial sub-agent cross-checking, and file each confirmed finding as a Linear Backlog issue. The shared audit engine that audit-loop drives one unit at a time.
disable-model-invocation: false
user-invocable: true
---

<!-- cspell:word unvalidated -->

# `audit-scope`

Audit a defined scope of the codebase and file each
confirmed finding as its own Linear **Backlog** issue —
the same destination and format `linear-task` and
`audit-loop` use. Use when a milestone lands, a feature
ships, or before declaring a subsystem "stable", and as
the engine `audit-loop` calls for its per-unit passes.

This replaces the old `audit-codebase`, which wrote a
gitignored checklist. Findings now live in the Backlog,
so they're picked up as normal PRs (staged by
`stage-backlog`) instead of a scratch file.

## Two ways it runs

- **Directly (you invoke it).** Plan-gated: you give a
  scope, confirm the plan, and the skill files the
  surviving findings as Backlog issues itself.
- **Delegated (`audit-loop` invokes it).** The loop has
  already picked the unit and owns dedup + state, so
  audit-scope skips the plan gate, runs the audit, and
  **returns the confirmed findings** to the loop, which
  dedups them against its ledger and files. It does
  **not** file in this mode — the loop does.

The work in between — classify the scope, fan out the
dimensions, adversarially cross-check — is identical
either way.

## Input

Required:

- **Scope** — the paths, feature, module, or component
  to audit (e.g. "the swap flow", `src/picker/`, one
  file, a PR's touched files, "the whole codebase"). If
  missing on a direct run, stop and ask.

Optional (ask on a direct run if not provided):

- **Extra focus areas** — anything to weight heavily
  (e.g. "error handling in the RPC path", "race
  conditions in the balance fetch").

## Steps

1. **(Direct runs only) Collect scope and plan.** Gather
   the scope and any extra focus areas, draft an audit
   plan (the exact paths in scope, the dimensions below,
   the per-kind security checklist the scope selects, the
   extra focus areas, and the sub-agents that will run),
   show it to the user, and wait for confirmation before
   searching. Delegated runs skip this — `audit-loop`
   supplies the scope and there's no one to gate.

1. **Classify the scope by platform kind.** Match the
   scope's paths to a platform/subsystem so the right
   checklist runs — this is the subsystem-scope logic the
   audit shares with `audit-loop`. Read the **Audit
   registry** in `CLAUDE.md` (→ "Audit registry") and take
   the `kind` of the subsystem whose `roots` the paths map
   to; if the paths match no registered subsystem (or on a
   direct run over something new), infer the kind from the
   paths and build manifests (`Cargo.toml`,
   `package.json`, `Dockerfile`, `.github/workflows/`):

   - **on-chain program** (Rust / Anchor / Solana)
   - **frontend** (TS / React)
   - **indexer or backend**
   - **infra** (Docker, CI)
   - **docs** (`docs/**` and other prose)

   The kind selects the security checklist below; the
   other dimensions apply to every kind.

1. **Run the dimensions in parallel.** Spawn sub-agents
   via the `Agent` tool (single message, multiple calls),
   each scoped to the subject. **Prepend the standing
   sub-agent brief from `CLAUDE.md`** (→ "Briefing
   sub-agents") to every one — these agents don't inherit
   `CLAUDE.md`, and an audit legitimately ranges across
   the codebase, so they need the brief's shell discipline
   (Read/Grep/Glob over shell, one bare globbable command
   per Bash call) precisely *because* they explore widely.
   Don't narrow the brief — unlike a diff review, an
   audit is meant to look anywhere in the repo. At
   minimum:

   - **Security / pen-testing** — use the checklist for
     the scope's kind:
     - *Program (Rust):* missing signer / owner / PDA /
       `has_one` checks, unchecked arithmetic, CPI to
       unverified programs, slippage / min-out gaps,
       freeze / authority gating, integer truncation,
       reinitialization (may consult
       `mcp__solana-mcp__Solana_Expert__Ask_For_Help`).
     - *Frontend:* unvalidated input into swap params,
       secret leakage, unsafe HTML, trusting RPC
       responses.
     - *Indexer / backend:* reorg & finality handling,
       idempotent / exactly-once processing, dedup-key
       correctness, SQL / command injection, unsafe
       deserialization, secret handling, migration
       safety.
     - *Infra:* unpinned base images or actions, secrets
       baked into layers or logs, running as root,
       non-reproducible builds, over-broad token scopes.
   - **Comment accuracy** — comments and doc-comments
     that contradict, overstate, or no longer match the
     code they annotate.
   - **Magic numbers / DRY / duplication** — unnamed
     values that should be named or configured; repeated
     logic, parallel branches that should share a helper,
     copy-pasted constants or shapes. Flag the opposite
     too: premature or speculative abstractions with one
     caller that add indirection without payoff.
   - **Modularity / extensibility** — coupling,
     abstractions in the wrong layer, hidden
     dependencies, seams that force editing many files to
     extend.
   - **Hierarchical organization** — for every directory
     in scope, count the immediate children. A directory
     with, say, more than ~15 files and no subdirectories
     is a strong signal to break it up. Propose the actual
     split — the subdirectory names and which files land
     where — following groupings the existing names
     suggest (by-feature, by-layer, by-shape). Applies
     even to directories that aren't growing, when the
     groupings are visible.
   - **Naming conventions** — names follow the casing and
     idioms already established in sibling files (don't
     invent a house style); names describe what a thing
     *is or does*, not how it's implemented; no
     misleading, abbreviated-past-recognition, or
     stale-after-refactor names (a `*_temp` / `*_new` /
     `*_v2` that outlived its reason). Flag a rename only
     when it genuinely improves clarity.
   - **Doc-freshness vs code** — when the scope is a
     `docs/**` file (or code a doc describes): Grep the
     doc's named symbols (structs / fields / invariants /
     events / endpoints / env vars) against the code and
     flag drift (renamed field, changed size assert,
     dropped event field, stale status line).
   - **One sub-agent per extra focus area.**

   Each sub-agent returns findings with `file`, `line`,
   `dimension`, `severity` (high/med/low), a deterministic
   `fingerprint_slug` (`<topic>:<detail>`, lowercased,
   each run of non-alphanumeric characters collapsed to a
   single `-`), a `title`, a one-line `rationale`, and a
   `fix_sketch`.

1. **Adversarial cross-check.** Spawn a fresh skeptic
   sub-agent (brief it with the same `CLAUDE.md`
   sub-agent brief) with the collected findings and the
   scope. It kills false positives, challenges weak
   rationale, and surfaces anything the first pass missed. On material
   disagreement, re-spawn the relevant dimension agent to
   defend or retract; iterate at most 2 more rounds, then
   accept the survivors. This is the primary noise gate.

1. **Linter screen.** Drop any finding an existing lint
   rule (`make lint` — clippy, eslint, prettier, cspell, …)
   already catches; it'll surface in the normal flow. For
   a finding that's a *class* a linter could enforce but
   doesn't yet, keep it and note the rule or config that
   would catch the family.

1. **Hand off the survivors.**

   - **Delegated run:** return the confirmed findings
     (their `fingerprint_slug`s, titles, bodies, and
     severities) to `audit-loop`. Do **not** file — the
     loop dedups against its ledger first. Stop here.

   - **Direct run:** file each surviving finding as its
     own Linear **Backlog** issue, exactly as `linear-task`
     does. Resolve the destination IDs from the
     environment (never hard-code them) with a bare
     `printenv` per variable (each reduces to the same
     `Bash(printenv:*)` allow-rule):

     ```sh
     printenv LINEAR_TEAM_ID
     printenv LINEAR_PROJECT_ID
     printenv LINEAR_ASSIGNEE_ID
     ```

     Query each on its own line — macOS / BSD `printenv`
     honors only its first operand, so a combined
     `printenv A B C` returns just `A`.

     Before filing, dedup against the live Backlog with
     `mcp__claude_ai_Linear__list_issues` (same
     destination) so a re-run doesn't refile a finding
     already captured — match on the `**Fingerprint**:`
     line. Then `save_issue` (no `id`):

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

     **Dependencies.** Set a `blockedBy` / `blocks`
     edge per the **Blocking relations** brief in
     `CLAUDE.md` (→ "Linear automation") — as an
     autonomous filer, only on concrete evidence of a
     real ordering dependency, never speculatively.
     (Coupling that means *one PR* is the merged-issue
     case in Notes, not a relation.)

     The body must let a cold agent act on it in its own
     worktree (literal newlines, not `\n`):

     - `**File**: <path>:<line>` (clickable)
     - `**Dimension**: <dimension>` / `**Severity**: <high|med|low>`
     - `**What**:` the precise problem.
     - `**Evidence**:` the offending snippet (+ the doc or
       comment it contradicts, where relevant).
     - `**Fix sketch**:` the concrete suggested change.
     - `**Lint**:` *(when applicable)* the rule or config
       that would catch this class going forward.
     - `**Fingerprint**: <basename>:<fingerprint_slug>` —
       the dedup key (e.g. `swap.rs:slippage:no-min-out`),
       so `audit-loop` and re-runs recognize it. Mandatory.

1. **Report.** Print a short tally — findings by
   dimension and severity, deduped count, and (direct run)
   the filed issue identifiers, or (delegated run) a note
   that the findings were handed back to the loop.

## Notes

- **Read-only with respect to source.** This skill never
  edits source files; it only files Linear issues (or
  returns findings). Fixes happen in normal PRs picked up
  from the Backlog.
- If a finding spans multiple files that obviously belong
  in one PR, file them as one combined issue with a
  `**Fingerprint**:` line per finding (the union), the way
  `audit-loop` does — it saves `stage-backlog` from
  re-deriving the grouping.
- Shell discipline (per `CLAUDE.md`): every command is a
  single bare call that reduces to an allow-glob — no
  `&&`, pipes, `$(…)`, redirects, or heredocs; content
  search routes to the Grep tool (never `git grep`), per
  the sub-agent brief.
