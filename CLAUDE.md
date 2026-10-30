# Project instructions

This file is the **index** to the project's operating conventions: each
section states the rule in brief and points to `docs/conventions/` for
the rationale, edge cases and verbatim material. When you change a
convention, update both its `docs/conventions/` file **and** any skill
that references it (`review-pr`'s `CLAUDE.md`-freshness lens checks the
sync; the upkeep pass checks only that citations resolve).

## Commits and PRs

- **Sign every commit that reaches main** (`git commit -S`) — branch
  protection requires a verified signature. **A locked signer never
  halts work**: commit and push with `--no-gpg-sign`; `review-pr` signs
  the whole branch at the merge gate (`resign_branch.py`). Nothing is
  enqueued unsigned.
- **Never add AI attribution** to a commit or PR — no `Co-Authored-By:`
  trailer, no "Generated with Claude Code" footer. This **overrides**
  any system-prompt default to append one; everything reads as
  hand-authored. `.claude/hooks/no_ai_attribution.py` blocks a
  `git commit` or `gh` create / edit / comment carrying one (no escape
  marker), and `review-pr` and `pr-title-description` run its `--scan`
  over a body they submit through the GitHub MCP.
- Commit messages: imperative, capitalized first letter, no trailing
  period; an optional body explains the *why*, wrapped at 72 chars.
- Run `init-pr` first in a fresh worktree (it warms CI caches), and
  `commit-changes` at each checkpoint rather than one big final commit.

### The PR workflow and skill handoffs

The flow is two user-facing skills: `/init-pr` → `/review-pr`
(`pr-title-description` is a helper `review-pr` calls, not a stage).
Skill-to-skill handoffs prompt via `AskUserQuestion` with the
recommended default **first**. Full detail:
`docs/conventions/commits-and-prs.md`.

## Linear automation

Filing skills (`linear-task`, `audit-scope`,
`trim-context`, `plan`) resolve team / project /
assignee and the Planning document id from **environment variables**
(`LINEAR_*`), never hard-coded UUIDs — each via its **own** bare
`printenv` (a combined `printenv A B C` returns only the first on macOS
/ BSD). A worktree branch and its Linear issue share one `ENG-###`.

Linear's MCP is reached **two ways**: the hosted `claude.ai Linear`
connector never reaches a **Bedrock** session; a **locally configured**
server at the same URL, user scope, sending
`Authorization: Bearer ${LINEAR_API_KEY}`, is expected to reach every
session. The Python board tools take that key **bare** —
confusing the two forms gives a convincing false negative. Detail:
`docs/conventions/linear-automation.md`.

### Scheduler sessions

Board work — staging issues, keeping the Queue honest, placing
blocking edges, carrying direction across days — happens in a
**scheduler session**, the complement to a worktree implementation
session. It runs in the base repo (started and resumed with `plan`,
never a worktree), bootstraps from the
"Planning" Linear document (`LINEAR_PLANNING_DOC_ID`), and writes its
decisions back there. The `plan` skill is its method; the document is
the state. Detail — the env var, and where blocking edges are
placed: `docs/conventions/linear-automation.md`.

### The architect session — the CEO hat

Long-horizon **design** conversations run in their own session: `plan`
keeps the board coherent, `architect` asks whether the thing on the
board is the right thing to build. Launched with **`architect <n>`**
(model-pinned, idempotent, one resumable session per governing issue,
named `<n>`), it bootstraps from the Planning document and plan's
rulings only, and makes **no scheduling writes**: it files its own
tasks, moves its own issue to In Review, places only operator-ratified
edges, re-prioritizes nothing and touches no other issue. It hands
conclusions to the scheduler through the Planning document's
`Notes for the next planning session` heading **and** a direct
message. Detail: the `architect` skill.

**Its worktree is TEMPORARY and the session is READ-ONLY toward the
repo: no commits, no PR.** Durable state goes to **Linear**; anything
repo-bound — a ratified spec included — lands via a **follow-up worker
task**. **`explore` shares all of this**, audits included.

**A spec FILE is optional; ratification is not — nothing filed before
the operator's yes.** The spec loop is write → edits **in place** →
read back **once**, ending in the task list; then `ratify-staged` files
them under `Session pending` and moves the set to Todo in one batch on
the operator's ticks. Say where the design lives in plain text. Open
items carry greppable **`NEEDS-CONFIRM`** / **`NEEDS-FEEDBACK`**
markers indexed at the top; **settled text carries no marker**.

State is **In Progress** while running, **In Review** at handoff,
**Done only on the operator's ratification** — never self-marked. With
durable state in Linear *before* the handoff, the worktree is
**expendable**: a spec file must never be the only copy of live work.
Mechanism and naming rule: `docs/conventions/local-integrations.md` →
"Architect and explore sessions: temporary worktree, no PR".

### Three more verbs

**`/wrap`** is the clear-to-close gate for ending a session — a
deterministic checklist ending in an explicit verdict, for the ragged
endings `review-pr` does not cover. **`/harden`** turns a proven
ad-hoc command shape into a committed, tested tool, consuming the
candidates the daily digest already ranks; it refuses a shape with no
measured recurrence. **`/root-cause`** is systematic debugging under
two hard rules: no fix before the cause is investigated, and after
three failed hypotheses the architecture is what gets questioned.

### Structured filing fields

Every filed issue carries `**Fingerprint**: <domain-token>:<slug>`,
the dedup key. Its first token is a **dotless domain token, never a
bare `name.ext`** — Linear linkifies a hostname-valid basename and
corrupts the key (`feeds-http:…`, not `http.rs:…`). Two more
write-mangle rules bind every filed body: **never let an emphasis span
wrap a newline**, and **never start a machine-parsed field with a bare
hostname-valid `name.ext`**. A rotation folds coupled findings into the
**fewest coherent PRs** — one issue per set that would land as one PR
(same subsystem / crate / language-domain), each finding keeping its
fingerprint, never across apps / languages / deploy units — and **a set
larger than a short session splits into sequential PRs** of roughly 4–5
findings. Detail: `docs/conventions/linear-automation.md`.

### The Linear state tracks the session, not the PR

The Linear state is the state machine of the **session**; the PR's
merged/open state tracks the code. **In Review** covers PR-under-review
*and* merged-with-follow-up-outstanding; **Done** means
operator-ratified complete — never merely merged. Linear makes **no
state transition on merge** (a team setting), so the In Review state
`review-pr` writes at the enqueue handoff persists until an explicit
approval. Before a session, **Backlog** with no milestone awaits review
and **Todo** is approved. Since the state tracks the session, the
upkeep pass prunes a worktree on the
issue's **status type** (completed / canceled), never on PR-merged, and
fleet resume picks up In Progress **or** In Review. Detail:
`docs/conventions/linear-automation.md`.

### Claude: meta-work prefix

**Meta-work** issues — every edited path under `.claude/**`,
`CLAUDE.md`, `docs/conventions/**`, or `cfg/**`, plus at most an
incidental product-code comment fix naming retired agent material —
carry a leading **`Claude:`** token on their **Linear issue title**
(capital C, colon, space) so agent-infra work batches apart. Filing
skills (`linear-task`, `audit-scope`, `plan`) emit it at filing time.
It is a **Linear-title signal only — never a PR title** (PR titles keep
`type(ENG-###): Subject`). Detail:
`docs/conventions/linear-automation.md`.

### The verb/substrate title token

A Linear title's leading **`[<Verb>/<Substrate>]`** (`Task` /
`Chore` / `Explore` / `Architect`, `Bedrock` / `Anthropic`) is the
issue's **launch line**: `[Chore/Anthropic]` on 1234 is
`chore anthropic 1234`. No token is Task on Bedrock, the only
elision; it precedes `Claude:`, `[Operator]` marks hand-executed
items, and the token never reaches a PR title. Detail: same doc.

### Keep Linear tags out of PR bodies and comments

**Do not put `ENG-###` tags in PR descriptions or comments** — Linear
auto-links and can auto-transition issues merely mentioned. Refer to
other work by title or a plain GitHub link. The two carve-outs (the
`type(ENG-###):` title scope, terminal / TUI output):
`docs/conventions/linear-automation.md`.

### Partial edits — use `patch`, don't re-send the body

`save_issue` / `save_document` take a **`patch`** array (`append`,
`prepend`, `insert_before`, `insert_after`, `replace`, `replace_range`)
applied in order and atomically, so amending part of a body **never**
requires re-sending it. `patch` is **update-only**, never passed
alongside `description` / `content` (which replace wholesale), and
capped at 50 ops. Each op takes its **own** argument names (`append` /
`prepend` take `text`; `replace` takes `old_string` / `new_string`);
the table is in the convention doc. Every `save_issue` call echoes the
full body, `patch` or not, so a body **append** goes through the zero-echo
`.claude/tools/linear_issue.py append`, a checklist tick through
`linear_issue.py tick`, and a **fold**'s ops file through
`.claude/tools/linear_patch.py patch --ops <path> ENG-###` (same ops,
same atomic abort, no echo). Other **anchored** one-off edits stay on
the MCP `patch`, where the abort is worth the echo. Non-body field
writes go through `board_batch.py` (`state` is a single-issue alias).
Anchors must match the **stored** text exactly once; Linear rewrites an
`ENG-###` into a mention node, so never anchor on one. Detail:
`docs/conventions/linear-automation.md`.

### Backlog reviews, Todo pulls

**Backlog** with no milestone is a spec awaiting the operator's
review; **Todo** with no milestone is reviewed and dispatchable, body
frozen. The operator's Backlog → Todo move **is** the approval — the
scheduler never makes it — and a fold into a Todo body drops it back.
The dispatch gate is **state Todo plus unblocked**; the Next view
shows the unblocked Todo. A
**parking milestone** — `Session pending` (and the legacy
`Audit findings`, retired once empty) — parks an issue in either
state, so a filing skill sets **`Backlog` plus the milestone, in the
creating call**, and promotion is **one write, the milestone clear**.
`Follow-ups` is struck: filers land in Backlog and message the
curator. Every full bootstrap says each parked count **out loud**; a
genuinely urgent stray (a broken guard) is promoted on the spot.

**The curator owns review** — `curate`, a daily advisor-tier base-repo
session the launcher dispatches. It walks the operator through the
un-parked Backlog (`[Operator]` items included) four at a time and
moves each ticked, uncommented task to Todo one at a time, the tick
being the operator's move. Detail:
`docs/conventions/linear-automation.md` → "The curator owns review".

### An audit is a board issue, not a directive

**No upkeep pass runs an audit.** A scheduler session files an **audit
issue** — a Backlog task naming **one** audit-registry target, with
scope, rationale and sequencing — and the session that pulls it runs
exactly that as one scoped `audit-scope` run. Findings land **staged**
(`Backlog` plus `Session pending`) and are ratified in the audit
session's own tab through `ratify-staged`, never promoted by planning.

Cadence is the **audit heartbeat**: every full planning bootstrap reads
the Planning document's bounded **audit-state table** (one row per
registry unit) and either files an audit issue or **declines with a
recorded reason** — never an unrecorded non-decision. The random
rotation survives only as an explicit `audit-scope --rotation N`. The
**adversarial sub-agent fan-out is authorized by the invocation
itself**: never substitute an inline pass or silently skip it; if the
tooling is absent, stop and ask. Detail: the `audit-scope` and `plan`
skills.

### Trims are the analyst's, not a session's

A session files **nothing that is only a trim**: what it kept running
into goes in its `Session account` block, which the daily digest
lifts. `trim-context`, the **fold analyst**, judges the whole day at
once, dedups by fingerprint and Backlog title (folding a near-match
into the existing issue), and files **small themed `Claude:` batches
of ~4–5 parts** into Backlog, no milestone. A rejection filed Canceled
with its reason is permanent. The **launcher** (`launch`, its own tab)
runs that fold each morning, dispatches the curator on the scheduler's
`state of play written` with the kill and merge candidates, and asks
the day's **one destructive question**. Detail:
`docs/conventions/linear-automation.md`, `trim-context` and `launch`.

### Blocking relations

**No automated writer files a blocking edge — ever**, semantic ones
included; a human places them — in the **scheduler**, by a ticked
curator proposal, or by a seat session's filing yes. The scheduler and
curator message each other any change to the other's edge; both write
priorities under the agreement rule. A spurious edge (dropping an issue out of
the available set) costs more than a missing one (a rebase). A filer
that believes a real dependency exists **proposes** it via
`AskUserQuestion` with the concrete evidence and writes it only on an
explicit yes; in an autonomous run the default is **no edge**, the
suspicion recorded as prose. Human-placed edges are authoritative and
never rewritten. **No exception**: the
`Claude:` batches carry **no edge** and are **not serialized**. File
overlap is **not** a dependency and records no collision link;
reconciling it is the scheduler's work, judged on content. Detail:
`docs/conventions/linear-automation.md`.

## GitHub via MCP

All GitHub operations go through the **GitHub MCP** (`mcp__github__*`),
passing `owner: "DASMAC-com"`, `repo: "dropset"`. The deliberate `gh`
exceptions (the merge-queue enqueue + dequeue probe, and the polled
CI / PR-state reads `gh api` / `gh pr checks` / `gh pr view --json`), the
PAT-not-OAuth auth setup, and the read/write permission split all live
in `docs/conventions/github-mcp.md`.

**Actions configuration lives there too**: the secrets inventory; the
rule that a value is **a secret when it must be masked in logs**
(Actions masks only secrets), else a variable or constant; the required
status checks, where the **ruleset** is the gate and the doc's list
only a mirror; the **job-vs-step distinction** deciding whether a
workflow change needs a ruleset edit; and that **a fork `pull_request`
run receives no secrets**, so a required check with a secret dependency
cannot pass on a fork PR. Judge the next secret or required check
against those, not per-PR.

## AWS infrastructure

AWS resources are **CloudFormation YAML** under `infra/aws/`. Templates
pass **both** `cfn-lint` and the repo's strict `yamllint`, so they are
written to fit the latter — alphabetical keys, single-quoted strings,
block style, folded block scalars for long ARNs. Documentation lookups
go to the credential-free `aws-docs` MCP server; account actions go to
the **AWS CLI** under an SSO profile, with the SigV4 `aws-mcp` server a
**fallback even when healthy** — its one unique capability is
`aws___retrieve_skill`, and its connection failures are **its own bug,
not a substrate one**. Search the AWS docs before acting and keep to
least privilege (deploys via the passed `*-cfn-deployment` role). The
`*-agent-provisioning` role is **inert by decision**, so IAM-creating
deploys run from the admin login; wiring the MCP to assume it is ruled
out.

**A session cannot log itself in** (`aws login` is interactive), so
`plan`, the one verb reading cost data, **gates its own launch** on
credentials that resolve (probe, log in, re-probe, refuse); `architect`
deliberately does not. The profile is an **optional** `DS_AWS_PROFILE`
in the untracked runtime config. The gate's `sts:GetCallerIdentity` is
authorization-free: it rules out an **expired** session, not a live one
lacking `ce:*`. AWS publishes **no** propagation bound for an IAM
change — verify by retrying the call. Detail:
`docs/conventions/aws-infra.md` and
`docs/conventions/local-integrations.md` → "The AWS login gate".

## Skill tooling

A skill's deterministic helper is **Python under `.claude/tools/`**
(stdlib, `unittest`-covered), **never** a Cargo workspace member. MCP
is for prototyping and fallback; a repeated workflow is hardened into a
Python tool — **`/harden` is the verb**. **Repeated skill *prose*** gets
one source under `.claude/shared/`, filled into marked regions by
`make render-skills` and gated by `make render-check` (which fails on a
dangling marker); extract only genuinely verbatim repetition. Detail:
`docs/conventions/skill-tooling.md`.

## Context economy

Every tool result is **replayed as input on every later turn**, so a
fat early payload is paid many times over, and **session length is the
largest cost lever** — cost is roughly **quadratic** in it. **Stage
work so a session stays short** (~4–5 levers or findings per PR, target
**300–800 changed lines**, floor ~150 unless trivially mechanical, split
at 1,000), preferring an occasional **rebase** to a serial chain; this
binds at *breakdown* time. **Smaller is not monotonically cheaper** (a
per-PR overhead floor), but the upper bound is **absolute**. Lines
proxy turns; the metric is **dollars**, and **every session states its
own cost at close**. **Sonnet-tier task workers are rejected.** Detail:
`docs/conventions/context-economy.md` → "Session length is itself a
cost lever".

**Resident size is capped**: the `skill-size` lint hook caps skill entry
files and this file at 32,000 bytes, descriptions at 1,024. Over-cap
files are frozen in `cfg/skill-size-baseline.json`; never raise a
ceiling by hand. Same doc → "Resident size has a hard cap".

Per call, request the narrowest thing that answers the question, read
large files by slice (Grep, then `Read` with `offset`/`limit`), route
verbose logs through `run_quiet.py` (**any** repeated
verbose-on-success runner, `pnpm` included), and never re-fetch what's
in context. A whole-file read needs any **one** of four licenses
(edit-plus-brief, a planned multi-region read, an exemplar imitated N
times, proving an absence that is the answer); **citing a file is not
one**. The test is **reuse, not size**. Slice a harness-persisted
result with `.claude/tools/read_result.py` and a file at another ref
with `.claude/tools/show_at_ref.py`, never whole. A **script you write
is a tool-result generator** — default it to a ranked summary, the full
series behind a flag. Track wasteful payloads for `/session-metrics`.
Detail: `docs/conventions/context-economy.md`.

## Shell commands

**Every Bash invocation must reduce to a reusable allow-rule**
(`Bash(prefix:*)`). One bare command per call — no `&&` / `;` / pipes,
no `$(…)` / backticks, no redirects or heredocs, no `cd`. Prefer the
Read / Grep / Glob tools over `cat` / `grep` / `find`; never
`git grep`. When the Grep tool is absent, the fallback — main loop
included — is `python3 .claude/tools/search_source.py '<pattern>'`
(it prunes generated and never-search trees), else a bare, single
`grep` **scoped to source directories** (it ignores gitignore). Ask
for a search's narrowest form (`-l` for existence, never a recursive
`-c`) and narrow its **scope** (`--dir` / `--glob`); `--context`
scales with match *density*, so clustered matches want `--files-only`
plus a slice-read. Keep a stable command + subcommand prefix. Never
sweep `.claude/` bare — it holds the worktrees and their `target/`;
scope to `skills` / `tools` / `hooks`. A multi-URL `curl` status probe
must repeat `-o /dev/null` **per URL** or use `-I`. This holds for shell
you **author** in skills, scripts and Makefile targets, and for work
you hand a sub-agent. Three opt-in `PreToolUse` guards enforce it:
`.claude/hooks/no_compound_bash.py` (escape marker `#compound-ok`),
`.claude/hooks/no_git_grep.py` (no escape hatch), and
`.claude/hooks/no_destructive_bash.py` (an overridable ask via
`#destructive-ok`, plus a catastrophic deny no marker lifts). Detail:
`docs/conventions/shell-commands.md`.

**Firming has three homes and no per-session sweep**: `allowlist.py add`
ad hoc (the dominant path), `fewer-permission-prompts` on demand, and
the upkeep pass's roughly **monthly** refresh gate — the only periodic
adder, which proposes for the launcher's one ask and reports cruft to
prune. Do not reinstate a per-session pass.

## Local integrations and guard hooks

The **user-local Claude Code configuration** the repo documents but
does **not** commit: the compound-shell, **git-grep** (absolute;
revision-scoped search is `show_at_ref.py`), destructive-command
(best-effort advisory, not a policy boundary) and AI-attribution
(inspects only message/body *values*) guards above; the **worktree
edit-path guard** (blocks a file-mutating tool targeting a base-repo
absolute path from a worktree session); and the **private-key guard**
with its `permissions.deny` rules (no agent session or sub-agent
touches the mainnet keys or runs a mainnet operation; no escape
marker). Also the iTerm2 tab-color integration and the shell setup,
including the **session secrets**, resolved from 1Password at launch,
never written to a config file, the CLI signed out after the reads. A
1Password SSH prompt for a private key from anything but the cockpit
(the mainnet cockpit TUI, its prompts biometric-approved; defined
under the private-key guard in the detail doc below) is refused,
commit signing excepted. Both settings files are git-ignored, so all
of it is opt-in; `settings.local.json` is **one shared file resolved
through worktrees to the main checkout**. A committed guard is **inert
until wired**:
`make hook-wiring` (also run by the daily upkeep pass) names every
guard that cannot fire and writes nothing. The **session helpers**
(`cdds`, `fleet` and the session verbs) *are* committed, at
`.claude/shell/init.zsh`, sourced by one guarded shell-profile line;
only the runtime config (model ids, 1Password coordinates, Linear ids,
fleet knobs and the like) stays untracked, in the shell profile, its
shape the committed
`.claude/shell/runtime-config.manifest.json`, held to the code by
`runtime_config_check.py`. Detail:
`docs/conventions/local-integrations.md`.

### Session substrate: capability, not attendance

A session runs on **Bedrock** unless it needs something Bedrock lacks —
web search or deep research (web fetch works there) — or it is a **seat
session by role** (`architect`); sub-agents are not a differentiator.
Every other verb defaults to Bedrock, with `anthropic` the escape
(`plan anthropic` puts planning back on the seat). Models are named by
**role tier** — advisor, executor, helper, background — each set in the
untracked runtime config (`DS_MODEL_<TIER>[_SUBSTRATE]`); each verb
pins its tier at launch and refuses an unresolved one, so no skill
checks its own model. The verb table in
`docs/conventions/local-integrations.md` maps each verb to its tier
and default substrate.

A launch **records its substrate** in an untracked marker that resume
verbs re-export (re-pinning the tier); an absent marker reads as
**anthropic**, and an anthropic launch **clears** inherited Bedrock
exports.

A scheduler session may **dispatch** a ready task — `session_dispatch.py`
opens a **tab in the dispatching session's own window** (never a new
window) and types the
verb, authorized by the operator's yes as `fleet go` is; several verbs
go in one call, separated by a bare `+`. iTerm is driven only via
`.claude/tools/iterm_api.py`; **AppleScript is retired**.

## What a skill may decide alone

Three tiers, and every skill's autonomy boundary is one of them.
**Mechanical** — a defensible default with no real alternative — is
decided silently. **Taste** — a real alternative, bounded and
reversible cost — is decided, then surfaced at a gate the human already
reads, naming the road not taken. **User challenge** — overriding
something the operator *explicitly specified* — is never decided: the
operator's direction is the default, and the case for changing it is
put through `AskUserQuestion` with evidence, silence meaning no. The
canonical instance is the blocking-edge prohibition. The test for which
tier applies is **the asymmetry of the two errors**. Note that
"propose, don't act" is the third tier only — invoking
`audit-scope` / the review fan-out **is** their authorization. Full
detail: `docs/conventions/decision-classification.md`.

## Briefing sub-agents

A spawned `Agent` does **not** inherit this file, so it will reach for
forbidden compounds unless told otherwise. Prepend the **canonical
sub-agent brief** — read-only framing, Read / Grep / Glob over shell,
slice-read large files, one bare globbable command per Bash call,
material passed inline — to **every** `Agent` prompt, **by pointer**:
`lens_preamble.py --out <path> --no-facts` writes it once, and each
prompt opens "Read `<path>` first and obey it". Paste it verbatim
only to an agent without Read. Source:
`docs/conventions/sub-agent-brief.md`; a skill may narrow its subject
scope but never loosen the shell rules.

## Docs and skills prose

Refer to users in the abstract, never by name, in any committed doc or
skill. **Rust intra-doc links are gated** by the `rustdoc` hook
(workspace doc build, `-D warnings`). Two authoring rules follow, both
because the failure surfaces somewhere other than the file you edit: a
doc comment on an **IDL-captured** item takes a code span, never a
link (Codama copies it verbatim into `sdk/rs/src/generated/`, where the
path resolves to nothing), and **never widen an item's visibility** to
make a link resolve — degrade it to a code span, or re-path it.
**Spelling (cspell):** `cfg/dictionary.txt` is for terms in
**≥ 2 files**; a word in just one file gets a top-of-file inline
`cspell:word` escape (one word per directive). The dictionary carries
`merge=union`, so **never hand-resolve a
conflict in it** — the `--unique` sorter hook re-sorts and de-duplicates
at the next `make lint`, not at commit time (a deleted word it
resurrects does not heal). A sorted list generates merge conflicts, so
each Makefile target declares its own `.PHONY` beside its rule, never
in one central sorted block.
**Never copy a claim out of an applied migration into prose** — the
runner hashes the raw bytes, so a shipped migration (comments included)
can never be corrected. Reference the migration and state the *current*
claim in the mutable doc; a shipped comment that proves wrong gets its
correction recorded elsewhere with a back-reference. Immutability makes
a migration the **least** authoritative statement of current behavior
(`audit-scope`'s doc-freshness lens). Full detail:
`docs/conventions/docs-and-style.md`.

## Audit registry

The audit coverage map — the **subsystems**, **inter-subsystem
interfaces**, and **skip-globs** that a rotation
ranges over and `review-pr` refreshes on the PR path — lives in
`docs/conventions/audit-registry.md`. Read and append it there.
