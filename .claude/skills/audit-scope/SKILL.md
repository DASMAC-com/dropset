---
name: audit-scope
description: Audit a defined scope — one file, a PR's files, a subsystem, or the whole codebase — across the dimensions its platform kind calls for (security, comment accuracy, DRY, modularity, naming, doc-freshness), with adversarial sub-agent cross-checking, folding coupled findings and filing the fewest coherent Linear issues, each parked in state Todo under the `Audit findings` project milestone rather than dropped into the pull queue, and with no relations or collision links filed at all. The sub-agent fan-out is authorized by the invocation itself — never substitute an inline pass, never silently skip it. The shared audit engine that `audit` drives one file at a time, and that a session pulling a planning-filed audit issue runs once against that issue's named target.
disable-model-invocation: false
user-invocable: true
---

<!-- cspell:word unvalidated -->

# `audit-scope`

Audit a defined scope of the codebase and file the
confirmed findings as Linear issues **parked under the
`Audit findings` project milestone** — folded
into the fewest coherent PRs (coupled findings that share a
PR become one issue; see Notes), in the same destination and
format `linear-task` and `audit` use. Use when a project
milestone lands, a feature ships, or before declaring a
subsystem "stable", and as the engine `audit` calls for its
per-file passes.

This replaces the old `audit-codebase`, which wrote a
gitignored checklist. Findings now live as real Linear
issues — so they dedup and search like any other — but
**parked**, so filing them does not put work in the pull
queue. A planning session decides which get slated in
(`plan` step 8).

**Parked means state `Todo` plus the milestone**, both set in
the creating call. The milestone alone is not enough: the
operator's "Next" view is the *unblocked Backlog*, so a
finding filed as Backlog shows up there as available work
whatever milestone it carries — one rotation filed fifteen
that way and they had to be moved by hand. Promotion is then
two halves, clear the milestone **and** move Todo → Backlog.
See `docs/conventions/linear-automation.md` → "Parked
findings sit in **Todo**, never Backlog".

**The adversarial sub-agent cross-check is authorized by the
invocation.** Invoking this skill (or `audit`, or pulling an
audit issue) **is** the request for the fan-out — it is
the engine's mechanism, not an optional extra, so a
session-level "don't spawn agents unless asked" default is
*satisfied* by that authorization rather than in tension
with it. If sub-agent tooling is genuinely unavailable,
**stop and ask**: never substitute an inline pass, which is
the trade `review-pr` deleted its inline path to prevent,
and never silently skip the cross-check.

**`review-pr` asks and this skill does not, deliberately.**
It puts one question at its entry approving both "run now"
and at which tier, and the tier choice is its spawn
authorization — it asks because it does many things and the
fan-out is one step, so an operator can reasonably want its
lint-and-CI half alone. Here the fan-out **is** the
deliverable and there is no earlier gate to ride, so a
question would be ceremony. Recorded so the difference does
not read as an oversight.

**This skill is what an audit issue runs.** Auditing reaches
the board as a first-class Backlog issue filed by a planning
session's audit heartbeat, naming one target from
`docs/conventions/audit-registry.md` with its scope and
rationale; the session that pulls it invokes this skill once,
scoped to that target. `housekeeping` runs no audit and reads
no directive — that path is retired. The broad random
rotation is `/audit`, an ad-hoc invocation.

## Two ways it runs

- **Directly (you invoke it).** Plan-gated: you give a
  scope, confirm the plan, and the skill files the
  surviving findings as parked issues itself.
- **Delegated (`audit` invokes it).** The caller has
  already picked the file and owns selection + dedup, so
  audit-scope skips the plan gate, runs the audit, and
  **returns the confirmed findings** to the caller, which
  dedups them against live Linear and files. It does
  **not** file in this mode — the caller does.

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
   searching. Delegated runs skip this — `audit`
   supplies the scope and there's no one to gate.

1. **Classify the scope by platform kind.** Match the
   scope's paths to a platform/subsystem so the right
   checklist runs — this is the subsystem-scope logic the
   audit shares with `audit`. Read the **Audit
   registry** in `docs/conventions/audit-registry.md` and
   take the `kind` of the subsystem whose `roots` the paths
   map
   to; if the paths match no registered subsystem (or on a
   direct run over something new), infer the kind from the
   paths and build manifests (`Cargo.toml`,
   `package.json`, `Dockerfile`, `.github/workflows/`):

   - **on-chain program** (Rust / Anchor / Solana)
   - **frontend** (TS / React)
   - **indexer or backend**
   - **infra** (Docker, CI)
   - **docs** (`docs/**` and other prose)
   - **agent infra** (`.claude/**`, `CLAUDE.md`,
     `docs/conventions/**`) — the instructions and helper
     tools that drive agent sessions. It is half Markdown
     prose and half stdlib Python, and neither existing kind
     fits: `docs` / `specs` asks whether prose still matches
     the *code* it describes, which is the wrong question for
     a rule whose implementation is a skill step. Per the
     registry, this kind wins over `docs` on
     `docs/conventions/**`.

   The kind selects the security checklist below; the
   other dimensions apply to every kind.

1. **Run the dimensions in parallel.** Spawn sub-agents
   via the `Agent` tool (single message, multiple calls),
   each scoped to the subject. **Prepend the standing
   sub-agent brief from `docs/conventions/sub-agent-brief.md`**
   to every one — these agents don't inherit that brief (or
   `CLAUDE.md`), and an audit legitimately ranges across
   the codebase, so they need the brief's shell discipline
   (Read/Grep/Glob over shell, one bare globbable command
   per Bash call) precisely *because* they explore widely.
   Don't narrow the brief — unlike a diff review, an
   audit is meant to look anywhere in the repo.

   **Split a dimension that would invite
   self-decomposition, at plan time.** The brief now tells
   every agent not to spawn sub-agents of its own, but a
   dimension whose scope spans obviously separable lanes
   invites exactly that: comment-accuracy over source
   comments *plus* a spec doc *plus* five READMEs is three
   jobs wearing one name. One such agent fanned out four
   grandchildren for ≈8.8M of per-turn input that no skill
   asked for, and the fan-out was invisible to the
   rotation's own plan. If a dimension reads as lanes,
   make it separate agents here, where they are counted.

   At minimum:

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
     - *Agent infra:* a guard hook that can be bypassed —
       an escape marker broader than its stated rationale,
       or a pattern that misses an equivalent spelling of
       the thing it blocks; a helper tool interpolating
       unvalidated input into a shell command; credentials,
       tokens, or account identifiers committed into a
       skill or settings file; and a skill step that tells
       an agent to weaken or route around a guard rather
       than satisfy it.

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

     **A quoted MIGRATION inverts the usual instinct.**
     When a doc quotes or paraphrases a claim from
     `db-schema/migrations/**`, check it against HEAD
     *behavior*, never against the migration text. An
     applied migration is immutable — the runner hashes the
     raw bytes and rejects any change, so even a comment
     cannot be reworded — which makes it the **least**
     authoritative statement of current behavior while
     looking like the most authoritative one. Measured
     instances: an asymmetry claim in migrations 0003 and
     0008 was copied into two prose docs and both went
     stale silently when the world it described was
     corrected, and migration 0009 states a false invariant
     (that a source writes bars or ticks and never both,
     untrue for the coinbase source) whose correction lives
     in 0010's text instead. Anything reasoning off a
     migration's stated invariant may be reasoning off a
     false premise that can never be fixed in place.

   - **Instruction integrity** — when the scope is agent
     infra, this is the dimension that matters most, and it
     replaces the code-facing reading of doc-freshness
     above. Look for: a rule stated in `CLAUDE.md` or a
     convention doc that **no skill implements**; a skill
     step prescribing a command, flag, or output field its
     tool doesn't actually support (a real instance: the
     sub-agent brief named `--count` / `--files` on
     `search_source.py`, which has neither); prose that
     contradicts a sibling convention; and index↔doc↔skill
     cross-references left dangling by a rename. Verify a
     prescribed command against the tool's **argument
     parser**, never against its prose — a docstring and an
     `add_argument` call drift independently.

   - **One sub-agent per extra focus area.**

   Each sub-agent returns findings with `file`, `line`,
   `dimension`, `severity` (high/med/low), a deterministic
   `fingerprint_slug` (`<topic>:<detail>`, lowercased,
   each run of non-alphanumeric characters collapsed to a
   single `-`), a `title`, a one-line `rationale`, and a
   `fix_sketch`.

   **Every dimension prompt ends with the self-deflation
   clause**, and it is not optional boilerplate — it is the
   cheapest noise gate in this skill. Measured: five
   dimension agents returned **46** findings and two
   skeptics (~2.7M input) killed **37** — an ~80%
   false-positive rate at the dimension stage, with three
   findings refiling the audited document's own stated
   design intent. The skeptics earn their cost and are not
   the problem; the problem is upstream, in what gets
   returned. Tell each agent, in these terms:

   - **Drop what the artifact states as deliberate.** If the
     file, its doc comment, or a sibling convention names
     the thing as an intentional tradeoff, it is not a
     finding — re-filing an artifact's own stated intent is
     the single most common false positive.
   - **Drop what hurts no reader materially.** Name who is
     hurt and how; if you cannot, drop it.
   - **Return a considered-and-dropped list** — the
     candidates you rejected, one line of reason each. This
     is what makes the deflation checkable rather than a
     claim, and it lets the cross-check see what was already
     weighed.
   - **State the expected order of magnitude up front.** A
     scoped audit of one document or module returns **a
     handful** of findings, not forty. An agent returning
     dozens should re-read this clause before answering, and
     say so if it still believes the count.

1. **Adversarial cross-check.** Spawn a fresh skeptic
   sub-agent (brief it with the same sub-agent brief) with
   the collected findings and the
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
     severities) to the caller (`audit`). Do **not** file —
     the caller dedups against live Linear first. Stop here.

   - **Direct run:** file the surviving findings as Linear
     issues **parked under the `Audit findings` project
     milestone**, otherwise exactly as `linear-task` does —
     **folding coupled findings into the fewest coherent
     PRs** (see the folding rule in Notes), one issue per
     PR-group rather than one per finding.

     Parked means *not in the pull queue*: a first-class
     open issue for dedup and search,
     but out of scope for a planning bootstrap until
     somebody slates it in. That promotion is the `plan`
     skill's call (its step 8), never this one's, and it is
     done by **clearing the milestone** — not by closing the
     finding and filing a fresh copy. The milestone already
     exists; never create a second.

     Resolve the destination IDs from the
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
       state: "Todo",                 // parked, NOT pullable
       milestone: "Audit findings",   // parked — see above
       title: "<file>: <imperative fix, no trailing period>",
       description: "<markdown body, literal newlines>",
       priority: 3  // 2 for high-severity security
     )
     ```

     **Meta-work prefix.** If every path the finding's fix
     will edit sits under the meta surface (`.claude/**`,
     `CLAUDE.md`, `docs/conventions/**`, `cfg/**`),
     prepend the **`Claude:`** token to the title —
     `Claude: <file>: <imperative fix>` — per `CLAUDE.md` →
     "Claude: meta-work prefix". A finding touching product
     / on-chain / SDK / frontend code gets no prefix.

     **A meta finding stays parked under `Audit findings`,
     not `Claude meta`.** Audit output parks under its own
     milestone whatever surface it names, so file it the
     same way as every other finding here — state `Todo`
     plus `Audit findings`. Promoting a meta-flavored
     finding is then a **milestone swap** to `Claude meta`
     (a planning-session act), after which the next batch
     assembly consumes it. Filing it directly under
     `Claude meta` would skip the sequencing decision that
     promotion exists to make.

     **Dependencies — file none.** A blocking edge is
     **human-curated** (`CLAUDE.md` → "Blocking
     relations"), and this skill is an **autonomous
     filer**: there is nobody to answer an
     `AskUserQuestion` mid-rotation, and the default with
     no answer is **no edge**. So pass no `blockedBy` /
     `blocks` at all. When a finding really does look
     ordered behind another issue, write that into the
     body as prose instead: a `**Suspected dependency**:`
     line naming the issue it likely needs first and the
     evidence for it. A human can then place the edge from
     the same information you had.
     A spurious edge drops an issue out of the board's
     available set; a missing one costs a rebase.
     (Coupling that means *one PR* is the merged-issue
     case in Notes, and is unaffected by this.)

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

     - `**Fingerprint**: <domain-token>:<fingerprint_slug>`
       — the dedup key (e.g. `swap:slippage:no-min-out`),
       so `audit` and re-runs recognize it. Mandatory.

       `<domain-token>` is the file's basename with the
       **extension dropped** and any remaining `.` replaced
       by `-` (`swap.rs` → `swap`); prefix the parent
       directory when the stem is generic (`mod`, `main`,
       `lib`, `index`). It must be **dotless**: Linear
       linkifies a hostname-valid `name.ext` at the start of
       the line and corrupts the key.

       **The fingerprint is the stable key; a `file:line`
       is not.** Every citation and quoted snippet above is
       a snapshot of the commit the finding was *discovered*
       at, and an issue may sit in the Backlog for months
       while unrelated PRs rewrite around it. One session's
       surfaced task named four line-anchored citations
       across two files and **all four** had been rewritten
       away, with one named file carrying no relevant
       references at all — the work item was moot, at the
       cost of four exploratory greps to establish it. So
       write the fingerprint to name the *thing*, not its
       coordinates, and say in the body that the citations
       are as-of-discovery. The implementer re-derives the
       location; the fingerprint is what survives.

     **No `**Touches**:` line.** The declared-scope glob
     field is retired (see `CLAUDE.md` → "Structured filing
     fields"): nothing consumed it once the collision
     machinery went. Name the affected files in the finding's
     prose instead, where an implementer will read them.

   **Record no collision links, and no relations of any
   kind.** The automated file-collision machinery is
   **retired**: there is no per-issue sweep here, nothing to
   build around one, and nothing to reintroduce. Collision
   reconciliation belongs to a planning session, judged on
   content from its own board read. Blocking edges remain
   human-curated (`CLAUDE.md` → "Blocking relations").

1. **Report.** Print a short tally — findings by
   dimension and severity, deduped count, and (direct run)
   the filed issue identifiers, or (delegated run) a note
   that the findings were handed back to the caller.

## Notes

- **Read-only with respect to source.** This skill never
  edits source files; it only files Linear issues (or
  returns findings). Fixes happen in normal PRs picked up
  from the Backlog.
- **Fold coupled findings into the fewest coherent issues.**
  When findings share a PR — same subsystem, crate, or
  language-domain, and they would land as one change (e.g.
  all doc-/comment-freshness fixes, or all low-risk refactors
  in one crate) — file them as **one** combined issue, with a
  `**Fingerprint**:` line per finding (the union), the way
  `audit` does. The bar is **same-PR
  coherence**, not same-file; but never fold across separate
  apps, languages, or deploy units (the **coherence floor**).
  Nothing merges issues for you, so coupled findings become
  one issue only if you file them that way. Full rule:
  `CLAUDE.md` → "Structured filing fields" /
  `docs/conventions/linear-automation.md` → "Fold coupled
  findings into one issue".
- Shell discipline (per `CLAUDE.md`): every command is a
  single bare call that reduces to an allow-glob — no
  `&&`, pipes, `$(…)`, redirects, or heredocs; content
  search routes to the Grep tool (never `git grep`), per
  the sub-agent brief.
