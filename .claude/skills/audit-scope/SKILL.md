---
name: audit-scope
description: Use to audit a defined scope — one file, a PR's files, a subsystem, or the whole codebase — and when pulling a planning-filed audit issue, which names its target and runs as an `explore` task in its own worktree with the issue held In Progress until its findings are ratified in the tab. Audits across the dimensions the scope's platform kind calls for (security, comment accuracy, DRY, modularity, naming, doc-freshness) with an adversarial sub-agent cross-check this invocation authorizes. Files confirmed findings as the fewest coherent Linear issues, each sized to a short session, STAGED (Backlog plus the `Session pending` milestone) with no relations or edges, then ratified in this session's tab through `ratify-staged` before it closes. With `--rotation N`, runs N bounded RANDOM seven-unit rotations instead (four random files plus a subsystem, an interface and a repo-layout pass) — an ad-hoc breadth sweep, never a default.
disable-model-invocation: false
user-invocable: true
---

<!-- cspell:word Gstack -->

<!-- cspell:word unvalidated -->

# `audit-scope`

Audit a defined scope of the codebase and file the
confirmed findings as Linear issues **staged under the
`Session pending` project milestone** — folded
into the fewest coherent PRs (coupled findings that share a
PR become one issue, up to the size of a short session; see
Notes), in the same destination and
format `linear-task` uses. Use when a project
milestone lands, a feature ships, or before declaring a
subsystem "stable" — or, with `--rotation N`, for a broad
random sweep.

This replaces the old `audit-codebase`, which wrote a
gitignored checklist. Findings now live as real Linear
issues — so they dedup and search like any other — but
**parked**, so filing them does not put work in the pull
queue. **The operator ratifies every finding with this
session, in its tab** (operator ruling, 2026-10-23), through
the shared `ratify-staged` skill (step 8); planning is
consulted for feedback, never for ratification.

**Staged means state Backlog plus `Session pending`**, both set
in the creating call — the staging lane every seat session's
filings share. The milestone keeps a finding out of the review
inbox until the operator has ticked it here; only the
operator's final answer moves it to Todo, in `ratify-staged`'s
one batch write. `Audit findings` takes no new filings: it is a
legacy pool planning drains. See
`docs/conventions/linear-automation.md` → "Backlog reviews, Todo
pulls".

**The adversarial sub-agent cross-check is authorized by the
invocation.** Invoking this skill (scoped or `--rotation`,
or by pulling an audit issue) **is** the request for the fan-out — it is
the engine's mechanism, not an optional extra, so a
session-level "don't spawn agents unless asked" default is
*satisfied* by that authorization rather than in tension
with it. If sub-agent tooling is genuinely unavailable,
**stop and ask**: never substitute an inline pass, which is
the trade `review-pr` deleted its inline path to prevent,
and never silently skip the fan-out.

**`review-pr` works the same way.** Invoking it pre-approves
its regular fan-out; its one entry question exists only to
offer an abridged tier on a small diff, and an unanswered one
proceeds at the regular tier.

**This skill is what an audit issue runs.** Auditing reaches
the board as a first-class Backlog issue filed by the
scheduler's audit heartbeat, naming one target from
`docs/conventions/audit-registry.md` with its scope and
rationale; the session that pulls it invokes this skill once,
scoped to that target; no upkeep pass runs one. The broad random
rotation is `--rotation N`, an ad-hoc invocation.

## Two ways it runs

- **Scoped (the default).** Plan-gated: you give a scope,
  confirm the plan, and the skill files the surviving
  findings as staged issues itself.
- **Rotation (`--rotation N`).** No scope and no plan
  gate: each of `N` rotations runs seven units — four
  random files through steps 2–5 below, plus one random
  subsystem, one interface and one repo-layout pass — and
  dedups, files and announces as each unit finishes.
  **Read [`rotation.md`](rotation.md) before the first
  unit**; it owns the unit sequence, selection, the
  structural lenses, cross-unit folding and the `DONE`
  line. Everything shared lives here.

Classifying the scope, fanning out the dimensions and the
adversarial cross-check are identical either way.

## Where it runs

**A session pulling an audit issue runs as an `explore`
task, in its own worktree** — operator rule, 2026-09-11,
covering every audit however it was targeted. Launch it
with the plain verb naming the issue's tag (`explore 1196`);
the worktree is implicit in the verb, and it takes the
issue's own tag for both worktree and branch. A fresh
worktree runs on Bedrock with sub-agents defaulting to the
executor tier (an existing one keeps the substrate its
marker records); `explore anthropic <tag>` is for an audit
that needs web research. The mechanism, its reasoning, and what the worktree's
protection does and does not cover are owned by
`docs/conventions/local-integrations.md` → "Architect and
explore sessions: temporary worktree, no PR". Don't restate
them here. Every explore session's filed tasks, audit findings
included, stage under `Session pending` and run `ratify-staged`,
as an architect's do.

Four duties this skill owns directly:

- **A findings or spec file is never the only copy** —
  durable state is the filed issue. Writer body files
  (`<scratchpad>/finding-N.md`, a rotation's
  `part-N.md`) are transient input, not
  findings state.
- **A findings file passes
  `python3 .claude/tools/spec_markers.py <file>` before any
  draft of it is announced**, ratification included —
  its open items listed at the top and marked inline, both,
  per the `architect` skill's marker rule.
- **The audit issue stays In Progress while the findings
  loop is open** — In Review only after step 8's close gate
  — and reaches Done only on the operator's sign-off. Marking
  it Done at the handoff is the measured mistake that
  produced the rule — under the no-PR shape the
  issue state is the only signal that the work is live, so
  an honest state is the whole of this skill's obligation.
  Run `/session-metrics`, the shared tail, immediately before
  the final state write, and at ratification plus fold close with
  `close_session.py --state Done --issue ENG-### --session-id <uuid>`,
  which appends the cost line.
- **Read-only toward source.** No source edit, no commit, no
  push, no local state — Linear is the record, so a wiped
  worktree loses nothing. A rotation's ff-only sync to
  `main` is the one repo operation (`rotation.md`). Note the
  transcript is *not* covered by the issue's state: the
  conversation purge protects only a transcript on an open
  PR, which an audit never has.

## Input

Required on a scoped run (a `--rotation` run takes none):

- **Scope** — the paths, feature, module, or component
  to audit (e.g. "the swap flow", `src/picker/`, one
  file, a PR's touched files, "the whole codebase"). If
  missing, stop and ask.

Optional (ask on a scoped run if not provided):

- **Extra focus areas** — anything to weight heavily
  (e.g. "error handling in the RPC path", "race
  conditions in the balance fetch").

## Steps

1. **(Scoped runs only) Collect scope and plan.** Gather
   the scope and any extra focus areas, draft an audit
   plan (the exact paths in scope, the dimensions below,
   the per-kind security checklist the scope selects, the
   extra focus areas, and the sub-agents that will run),
   show it to the user, and wait for confirmation before
   searching. A rotation skips this — it picks its
   own files and there's no one to gate.

   **The plan prints one row per lane: lane, tier.** A
   lane is one spawned agent — each dimension (after any
   split below), each focus area, and the skeptic, its
   re-spawn rounds counted within it. Dimension agents and
   the skeptic judge, so they pass **no** `model`: they run
   on the launcher's sub-agent default, the executor tier.
   A lane
   that enumerates or cross-references rather than judges —
   the hierarchy dimension's directory child counts,
   doc-named symbols still existing, unreferenced dictionary
   words — is marked **mechanical** and runs on the helper
   tier (spelled `sonnet` in the Agent call; the launcher
   binds that slot to it). That is not the standing rejection of Sonnet-tier
   task workers (2026-09-11), which covers implementers: a
   read-only lane whose error mode is a false positive the
   skeptic kills downstream is not one (operator-confirmed,
   2026-10-22). No lane pins `haiku` — a lane that cheap is
   a Python tool.

1. **Classify the scope by platform kind.** Match the
   scope's paths to a platform/subsystem so the right
   checklist runs. Read the **Audit
   registry** in `docs/conventions/audit-registry.md` and
   take the `kind` of the subsystem whose `roots` the paths
   map
   to; if the paths match no registered subsystem (or on a
   scoped run over something new), infer the kind from the
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
   A dimension with a **mechanical half** (hierarchy's child
   counts, doc-freshness's symbol sweep) splits the same
   way: spawn the mechanical lane first and pass its output
   inline to the judging lane, spawned with the rest.

   At minimum (spelling and term consistency are **not**
   lanes — cspell runs under `make lint`, and step 5 drops
   what it catches):

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
   `dimension`, `severity` (high/med/low), an
   `evidence_state`, a deterministic
   `fingerprint_slug` (`<topic>:<detail>` with no file
   component — the *what*; each part lowercased, every run
   of non-alphanumeric characters collapsed to a single
   `-`, leading/trailing `-` trimmed, so two passes over
   one issue yield one key), a `title`, a one-line
   `rationale`, and a `fix_sketch`. `evidence_state` is
   **observed** (the agent read the offending text at the
   cited site), **reproduced** (it ran the failing path) or
   **inferred** (reasoned to, not read or run) — a separate
   judgement from severity, with no numeric confidence
   scale, matching `review-pr`.

   **Every dimension prompt ends with the self-deflation
   clause** from `docs/conventions/sub-agent-brief.md`,
   appended verbatim — not optional boilerplate but the
   cheapest noise gate in this skill. Measured: five
   dimension agents returned **46** findings and two
   skeptics killed **37**, an ~80% false-positive rate
   upstream of the skeptic, which earns its cost.

1. **Adversarial cross-check.** Spawn a fresh skeptic
   sub-agent (brief it with the same sub-agent brief; no
   `model`, per step 1) with the collected findings and the
   scope. It kills false positives, challenges weak
   rationale, and surfaces anything the first pass missed. On material
   disagreement, re-spawn the relevant dimension agent to
   defend or retract; iterate at most 2 more rounds, then
   accept the survivors. This is the primary noise gate.

   Brief the skeptic with two further duties, both
   borrowed from Gstack's review and cso skills:

   - **Sideways sweep.** For each finding it upholds, search
     the fingerprinted pattern — the offending code or text,
     not the slug — at every other site in scope, once,
     paths only (`search_source.py --files-only`, or Grep's
     files-with-matches mode), then widen the finding's
     prose to name the siblings — or record that it is a
     single instance. A pattern found at one site and fixed
     there leaves its siblings to the next audit.
   - **Evidence state.** Kill an **inferred** high it cannot
     raise to observed or reproduced.

1. **Linter screen.** Drop any finding an existing lint
   rule (`make lint` — clippy, eslint, prettier, cspell, …)
   already catches; it'll surface in the normal flow. For
   a finding that's a *class* a linter could enforce but
   doesn't yet, keep it and note the rule or config that
   would catch the family.

1. **Dedup, then file the survivors.** (A rotation's FILE
   unit reaches here per unit, its structural units after
   their own cross-check — see `rotation.md`.)

   - **Dedup against live Linear, open *and* resolved.**
     Build the set once per run: list every Dropset-project
     issue with `mcp__claude_ai_Linear__list_issues`
     (`includeArchived: true`) and collect **every**
     `**Fingerprint**:` line on each — a combined issue
     carries several — with its state. A finding whose
     fingerprint is already present, in **any** state, is
     not refiled: a resolved match (Done / Won't-fix /
     Canceled) was already triaged, and refiling it would
     reopen settled noise.

   - **File** the surviving findings as Linear
     issues **staged under the `Session pending` project
     milestone**, otherwise exactly as `linear-task` does —
     **folding coupled findings into the fewest coherent
     PRs** (see the folding rule in Notes), one issue per
     PR-group rather than one per finding — and splitting a
     PR-group too large for a short session into sequential
     issues rather than letting one grow without bound.

     Parked means *not in the pull queue*: a first-class
     open issue for dedup and search, held out of the
     inbox until this session's review (step 8) clears the
     milestone — never by closing the finding and filing a
     fresh copy. Planning never promotes one. The
     milestone already exists; never create a second.

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

     File through the **zero-echo writer**, one
     call per issue, body on disk:

     ```sh
     python3 .claude/tools/linear_issue.py create \
       --title '<file>: <imperative fix, no trailing period>' \
       --body-file <scratchpad>/finding-N.md \
       --state Backlog --milestone 'Session pending' --priority 3
     ```

     `--priority 2` for a high-severity security finding.
     Team, project and assignee resolve from the `LINEAR_*`
     environment by default, so they are never spelled here.

     **Not `save_issue`.** The MCP echoes the entire stored
     body back on a *create* as well, which makes it worst
     for precisely this caller: an audit files folded issues
     with a part and a fingerprint per finding, so the bodies
     are long **by design** and the cost grows with exactly
     the thing the fold rule encourages. Measured (session
     fa6fc519, the ENG-978 seam audit): `save_issue` was the
     top main-loop sink at **9 calls / ≈22.8k**, supplying
     six of the session's eight largest single results, while
     the entire rest of the main loop's tool results came to
     roughly 23k combined — one shape, about half the cost.
     Every echoed byte was content the session had just
     authored, so nothing decision-relevant is lost.

     State, milestone and priority are writer *parameters*,
     which is what lets one filer serve this skill's
     Backlog + `Session pending` staging and the
     scheduler's plain Backlog follow-up filings
     (27 calls / ≈30.1k in one measured pass) without a
     second implementation.

     **Meta-work prefix.** If every path the finding's fix
     will edit sits under the meta surface (`.claude/**`,
     `CLAUDE.md`, `docs/conventions/**`, `cfg/**`),
     prepend the **`Claude:`** token to the title —
     `Claude: <file>: <imperative fix>` — per `CLAUDE.md` →
     "Claude: meta-work prefix". A finding touching product
     / on-chain / SDK / frontend code gets no prefix.

     **A meta finding stages under `Session pending`, not
     `Follow-ups`.** Audit output stages the same way
     whatever surface it names — state Backlog plus
     `Session pending` — and is ratified or rejected in
     step 8 like every other finding.

     **Dependencies — file none.** A blocking edge is
     **human-curated** (`CLAUDE.md` → "Blocking
     relations"), and filing here is **autonomous**:
     there is nobody to answer an `AskUserQuestion`
     mid-rotation, and step 8's review asks about
     findings, never edges, so the default is **no
     edge**. So pass no `blockedBy` /
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
       / `**Evidence state**: <observed|reproduced|inferred>`

     - `**What**:` the precise problem.

     - `**Why it's safe to fix in isolation**:` it touches
       only this file/symbol and depends on no other open
       finding. Omit it when a `**Suspected dependency**:`
       line is present — the two contradict.

     - `**Evidence**:` the offending snippet (+ the doc or
       comment it contradicts, where relevant).

     - `**Fix sketch**:` the concrete suggested change.

     - `**Lint**:` *(when applicable)* the rule or config
       that would catch this class going forward.

     - `**Fingerprint**: <domain-token>:<fingerprint_slug>`
       — the dedup key (e.g. `swap:slippage:no-min-out`),
       so re-runs and rotations recognize it. Mandatory,
       one line per finding on a combined issue.

       `<domain-token>` is the file's basename with the
       **extension dropped** and any remaining `.` replaced
       by `-` (`swap.rs` → `swap`) — derived from the
       basename so a moved file keeps its key; prefix the
       parent directory when the stem is generic (`mod`,
       `main`, `lib`, `index`: `feeds/src/mod.rs` →
       `feeds-mod`). It must be **dotless**: Linear
       linkifies a hostname-valid `name.ext` at the start of
       the line and corrupts the key. (A rotation's
       structural findings use an `arch:` prefix instead —
       `rotation.md`.)

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

     - `**Discovered by**: audit-scope <scope> @ <SHA>` —
       a rotation writes `rotation <unit>` as the scope;
       the SHA is what
       dates every citation above it.

     **No `**Touches**:` line.** The declared-scope glob
     field is retired (see `CLAUDE.md` → "Structured filing
     fields"): nothing consumed it once the collision
     machinery went. Name the affected files in the finding's
     prose instead, where an implementer will read them.

   **Record no collision links, and no relations of any
   kind.** The automated file-collision machinery is
   **retired**: there is no per-issue sweep here, nothing to
   build around one, and nothing to reintroduce. Collision
   reconciliation belongs to a scheduler session, judged on
   content from its own board read. Blocking edges remain
   human-curated (`CLAUDE.md` → "Blocking relations").

1. **Report.** Print a short tally — findings by
   dimension and severity, deduped count, and the filed
   issue identifiers. A rotation announces per unit and
   closes with its `DONE` line instead (`rotation.md`).

   **Every audit record opens with a coverage line per
   planned lane** — the tally here, and the record a pulled
   audit issue receives (`linear_issue.py append`) —
   borrowed from Gstack's cso skill:

   ```txt
   COVERAGE <lane> (<tier>): complete
   COVERAGE <lane> (<tier>): partial — <what was left>
   COVERAGE <lane> (<tier>): not assessed — <not spawned, why>
   ```

   A run that ends early still writes it: "ten lanes, none
   complete" is a record, and silence is not.

1. **Ratify in the tab: run `ratify-staged` from its step 3.**
   The findings are already filed, with no edges, so there is
   no filing ask and no second filing: say "filed these N
   findings; let me know what you think" (its step 2's
   announcement), then run its review rounds, its one batch
   write and its close gate unchanged — the shared skill is the
   one source for that wording. A rotation runs it once, after
   its last `DONE` line. Before presenting a finding the
   session may message a live scheduler to check it
   against open or in-flight work; that answer is feedback, and
   planning never promotes a finding itself.

   **The close gate precedes In Review.** Once
   `ratify_staged.py gate` passes over every filed id, move a
   pulled audit issue to In Review; one still under
   `Session pending` means the review is unfinished, so ask
   once more. If that ask also goes unanswered (an unattended
   run), stop there: the findings stay staged, the audit issue
   stays In Progress, and `plan`'s count-and-offer line is the
   backstop. The heartbeat row updates when the audit issue
   completes, as before.

## Notes

- **Read-only with respect to source.** This skill never
  edits source files; it only files Linear issues. Fixes
  happen in normal PRs picked up from the Backlog.

- **Fold coupled findings into the fewest coherent issues.**
  When findings share a PR — same subsystem, crate, or
  language-domain, and they would land as one change (e.g.
  all doc-/comment-freshness fixes, or all low-risk refactors
  in one crate) — file them as **one** combined issue, with a
  `**Fingerprint**:` line per finding (the union). The bar
  is **same-PR
  coherence**, not same-file; but never fold across separate
  apps, languages, or deploy units (the **coherence floor**).
  Nothing merges issues for you, so coupled findings become
  one issue only if you file them that way.

  **That floor bounds what may be *separated*, not how large
  one issue may grow.** A coherent set larger than a short
  session splits into **sequential PRs** of roughly 4–5
  findings each, because session cost is roughly quadratic in
  session length — see `docs/conventions/context-economy.md`
  → "Session length is itself a cost lever". Where the two
  pull against each other, coherence wins at the floor and
  the size bound wins at the ceiling: never fold across a
  deploy unit to reach 4–5, and never carry a coherent set
  past a short session to avoid a second issue. Full rule:
  `CLAUDE.md` → "Structured filing fields" /
  `docs/conventions/linear-automation.md` → "Fold coupled
  findings into one issue".

- Shell discipline (per `CLAUDE.md`): every command is one
  bare call reducing to an allow-glob — no `&&`, pipes,
  `$(…)`, redirects, or heredocs; content search goes to
  Grep or its fallback, never `git grep`, per the
  sub-agent brief.
