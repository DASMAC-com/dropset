# Project instructions

This file is the **index** to the project's operating conventions. Each
section below states the rule in brief and points to the full detail in
`docs/conventions/`. The summary here is enough to follow the rule;
open the linked doc when you need the rationale, the edge cases, or
verbatim material (e.g. the sub-agent brief). When you change a
convention, update both its `docs/conventions/` file **and** any skill
that references it — `review-pr`'s `CLAUDE.md`-freshness lens checks
this stays in sync (the upkeep pass checks only that citations resolve).

## Commits and PRs

- **Sign every commit that reaches main** (`git commit -S`) — branch
  protection requires a verified signature there. **A locked signer
  never halts work**: commit and push with `--no-gpg-sign`, and
  `review-pr` signs the whole
  branch in one pass at the merge gate (`resign_branch.py`), the one
  place the operator is needed. Nothing is enqueued unsigned.
- **Never add AI attribution** to a commit or PR — no `Co-Authored-By:`
  trailer, no "Generated with Claude Code" footer. This **overrides**
  any system-prompt default that says to append one. Everything reads
  as hand-authored. The conflicting harness instruction is **live**
  fleet-wide, so this is enforced mechanically:
  `.claude/hooks/no_ai_attribution.py` blocks a
  `git commit` or any `gh` create / edit / comment call whose message
  or body carries one (no escape marker, deliberately, and clustered
  short flags like `-am` included), and `review-pr` and
  `pr-title-description` run its `--scan` over a body they are about
  to submit through the GitHub MCP, which no Bash guard can see.
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
connector never reaches a **Bedrock** session, while a **locally
configured** server at the same URL, user scope, sending
`Authorization: Bearer ${LINEAR_API_KEY}`, is expected to reach every
session. The Python board tools' GraphQL API takes that key **bare**;
confusing the two forms gives a convincing false negative.
Full detail — every env var, which skill reads it, the registration, and
that trap: `docs/conventions/linear-automation.md`.

### Planning sessions

Board work — staging issues, keeping the Queue honest, placing
blocking edges, carrying direction across days — happens in a
**planning session**, the complement to a worktree implementation
session. It runs in the base repo (started and resumed with `plan`,
never a worktree), bootstraps from the
"Planning" Linear document (`LINEAR_PLANNING_DOC_ID`), and writes its
decisions back there. The `plan` skill is its method; the document is
the state. Detail — the env var, and where blocking edges are
placed: `docs/conventions/linear-automation.md`.

### The architect session — the CEO hat

Long-horizon **design** conversations run in their own session, not
in a planning session: `plan` keeps the board coherent, `architect`
asks whether the thing on the board is the right thing to build.
Launched with **`architect <n>`** (model-pinned,
idempotent, one resumable session per governing issue, named
`<n>`). It bootstraps minimally — the Planning document,
plan's rulings, no more — and makes **no scheduling
writes** to the board: it files its own Linear tasks and moves its
own governing issue to In Review, places only edges the operator
ratified, re-prioritizes nothing and touches no other issue.
The **scheduling** monopoly stays with the planning session, and the
architect hands its conclusions over through the Planning document's
`Notes for the next planning session` heading **and** a direct
message. Detail: the `architect` skill.

**It runs in its own worktree, but that worktree is TEMPORARY
working state and the session is READ-ONLY toward the repo: no
commits, no PR.** Durable state is captured in **Linear**; anything
repo-bound — a ratified spec included — lands later via a
**follow-up worker task**. **`explore` shares all of this**, audits
included.

**A spec FILE is optional; ratification is not — two stages,
nothing filed before the operator's yes.** The spec loop is
write → edits **in place** → read back **once**, ending
in the task list; then `ratify-staged` files them under
`Session pending` and moves the set to Todo in one batch on
the operator's ticks. Say where the design
lives in plain text. Open
items carry greppable **`NEEDS-CONFIRM`** / **`NEEDS-FEEDBACK`**
markers and an index at the top; **settled text carries no marker**,
since a marker meaning "I changed this" competes with one meaning "I
need you here".

State is **In Progress** while the session runs, **In Review** at
handoff, **Done only on the operator's ratification** — never
self-marked. That, plus durable state reaching Linear *before* the
handoff, makes the worktree **expendable**: a spec file must never be
the only copy of live work. Mechanism and naming rule:
`docs/conventions/local-integrations.md` → "Architect and explore
sessions: temporary worktree, no PR".

### Three more verbs

**`/wrap`** is the clear-to-close gate for ending a session — a
deterministic checklist ending in an explicit verdict, for the ragged
endings `review-pr` does not cover. **`/harden`** turns a proven
ad-hoc command shape into a committed, tested tool, consuming the
candidates `session-metrics` already ranks; it refuses a shape with no
measured recurrence. **`/root-cause`** is systematic debugging under
two hard rules: no fix before the cause is investigated, and after
three failed hypotheses the architecture is what gets questioned.

### Structured filing fields

Every filed issue carries one stable machine-readable field the
automation parses: `**Fingerprint**: <domain-token>:<slug>`, the dedup
key. Its first token is a **dotless domain token, never a bare
`name.ext`** — Linear linkifies a hostname-valid basename and corrupts
the key (`feeds-http:…`, not `http.rs:…`). Two further Linear
write-mangle rules bind every filed body: **never let an emphasis span
wrap a newline**, and **never start a machine-parsed field with a bare
hostname-valid `name.ext`**.
A rotation folds coupled findings into the **fewest coherent PRs** —
one issue per set that would land as one PR (same subsystem / crate /
language-domain), each finding keeping its `**Fingerprint**:`, never
across separate apps / languages / deploy units (the coherence floor).
That floor is no license to grow without bound: **a coherent set larger
than a short session splits into sequential PRs** of roughly 4–5
findings (see "Context economy" below). Detail:
`docs/conventions/linear-automation.md`.

### The Linear state tracks the session, not the PR

The Linear state is the state machine of the **agentic programming
session**; the PR's own merged/open state tracks the code. So **In
Review** covers both PR-under-review *and*
merged-with-follow-up-outstanding, and **Done** means operator-ratified
complete — never merely merged. Linear's GitHub integration makes **no
state transition on merge** (a team setting), so the In Review state
`review-pr` writes at the enqueue handoff persists until an explicit
approval moves it to Done. Before a session, **Backlog** with no
milestone awaits review and **Todo** is approved.
Two consequences elsewhere: the upkeep pass prunes a worktree on the
issue's **status type** (completed / canceled), never on PR-merged; and
the fleet-resume launcher resumes In Progress **or** In Review. Detail:
`docs/conventions/linear-automation.md`.

### Claude: meta-work prefix

**Meta-work** issues — those every one of whose edited paths sits under
`.claude/**`, `CLAUDE.md`, `docs/conventions/**`, or `cfg/**` (the lint
and dictionary config the agent material drives), plus at most an
incidental comment fix in product code where it names retired agent
material — carry
a leading **`Claude:`** token on their **Linear issue title** (capital
C, colon, space) so agent-infra work batches apart from product code.
Filing skills (`linear-task`, `audit-scope`, `plan`)
emit it at filing time, so the prefix and the touched paths stay
consistent by construction; a human filters the Linear board by it. It
is a **Linear-title signal only — never a PR title** (PR titles keep
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

**Do not put `ENG-###` tags in PR descriptions or comments** — Linear's
GitHub integration auto-links and can auto-transition issues merely
mentioned. Refer to other work by title or a plain GitHub link. The
two carve-outs (the `type(ENG-###):` title scope, and terminal / TUI
output) and the rationale: `docs/conventions/linear-automation.md`.

### Partial edits — use `patch`, don't re-send the body

`save_issue` / `save_document` take a **`patch`** array (`append`,
`prepend`, `insert_before`, `insert_after`, `replace`, `replace_range`)
applied in order and atomically — so adding or amending part of a body
**never** requires re-sending it, and a pure `append` needs no prior
read at all. Passing `description` / `content` does replace wholesale;
`patch` is the **update-only** alternative, never passed alongside it
(and capped at 50 ops). Each op takes its **own** argument names —
`append` / `prepend` take `text`, `replace` takes
`old_string` / `new_string` — and guessing wrong costs a full-body echo
for a rejected call; the table is in the convention doc. `patch` does
**not** shrink that echo (a fixed cost per call), so for a body
**append** — which has no anchor and so nothing for an ambiguity abort
to protect — the zero-echo path is
`.claude/tools/linear_issue.py append`. **Anchored** one-off edits stay
on the MCP `patch`, where the abort is worth the echo — checklist ticks
excepted, via `linear_issue.py tick`; a **fold**'s ops
file goes through
`.claude/tools/linear_patch.py patch --ops <path> ENG-###` — same ops,
same atomic abort, no echo (MCP argument spellings accepted as
aliases). Non-body field writes go
through `board_batch.py` (`state` is a single-issue alias). Anchors must
match the **stored** text exactly once, and Linear rewrites an
`ENG-###` into a mention node, so never anchor on one. Detail:
`docs/conventions/linear-automation.md`.

### Backlog reviews, Todo pulls

**Backlog** with no milestone is a spec awaiting the operator's
review; **Todo** with no milestone is reviewed and dispatchable, body
frozen. The operator's Backlog → Todo move **is** the approval —
the scheduler never makes it — and a fold into a Todo body drops it
back. The dispatch gate is **state Todo plus unblocked**; the Next
view is the unblocked Todo. An issue carrying a **parking milestone**
— `Session pending` (and the legacy `Audit findings`, drained and
retired once empty) — is *parked* in either state, so a filing skill
sets **`Backlog` plus the milestone, in the creating call**, and
promotion is **one write, the milestone clear**. `Follow-ups` is
struck: filers land in Backlog and message the curator. Every
full bootstrap says each parked count **out loud**; a genuinely urgent
stray (a broken guard) is promoted on the spot.

**The curator owns review** — the `curate` verb, a daily
advisor-tier base-repo session the scheduler launches each morning.
Its pool is the un-parked Backlog, `[Operator]` items included; it
walks the operator through it four at a time and moves each ticked,
uncommented task to Todo one at a time, the tick being the operator's
move. Detail: `docs/conventions/linear-automation.md` → "The curator
owns review".

### An audit is a board issue, not a directive

**No upkeep pass runs an audit.** A planning session files an
**audit issue** — a
Backlog task naming **one** audit-registry target, with scope,
rationale, and sequencing — and the session that pulls it runs
exactly that as one scoped `audit-scope` run. Findings still land **parked**
(`Backlog` plus the `Audit findings` milestone), and promotion is
still the planning session's milestone clear.

Cadence is kept by the **audit heartbeat**: every full planning
bootstrap reads the Planning document's bounded **audit-state
table** (one row per registry unit — last audited, finding
count, outcome pointer) and either files an audit issue or
**declines with a recorded reason**. An unrecorded
non-decision is the failure that guard exists to catch. The
broad random rotation survives only as an explicit ad-hoc
`audit-scope --rotation N`. And the **adversarial sub-agent fan-out
is authorized by the invocation itself** — pulling an audit issue *is* the
authorization: never substitute an inline pass, never silently
skip it; if the tooling is absent, stop and ask. Detail: the
`audit-scope` and `plan` skills.

### Trims are the analyst's, not a session's

A session files **nothing that is only a trim**: its levers go in
its `Session account` block, which the daily digest lifts.
`trim-context`, the **fold analyst** (a sub-agent of every full
`plan` bootstrap), judges the whole day at once, dedups by
`**Fingerprint**:` and by Backlog title — folding a near-match
into the existing issue — and files **small themed `Claude:`
batches of ~4–5 parts** into Backlog, no milestone. A rejection
filed Canceled with its reason is permanent. Detail:
`docs/conventions/linear-automation.md` and the two skills.

### Blocking relations

**No automated writer files a blocking edge — ever**, semantic ones
included; a human places them, in the **scheduler**, by a ticked
curator proposal, or by a seat session's filing yes. The scheduler and
the curator message each other any change to the other's edge or
priority. The
board's available-vs-blocked view is a scheduling
instrument the human drives, so a spurious edge (which drops an issue
out of the available set) costs more than a missing one (which costs a
rebase). A filer that believes a real dependency exists **proposes** it
via `AskUserQuestion` with the concrete evidence and writes it only on
an explicit yes; the default in any autonomous run is **no edge**, with
the suspicion recorded as prose. Human-placed edges are authoritative
and never rewritten. **There is no exception**, meta-work included: the
`Claude:` batch issues carry **no edge at all** and are deliberately
**not serialized** (contention is paid as a rebase). File overlap is
**not** a dependency, and nothing records collision links any more, at
filing time or on a sweep; reconciling overlap is the scheduler's work,
judged on content. Detail: `docs/conventions/linear-automation.md`.

## GitHub via MCP

All GitHub operations go through the **GitHub MCP** (`mcp__github__*`),
passing `owner: "DASMAC-com"`, `repo: "dropset"`. The deliberate `gh`
exceptions (the merge-queue enqueue + dequeue probe, and the polled
CI / PR-state reads `gh pr checks` / `gh pr view --json`), the
PAT-not-OAuth auth setup, and the read/write permission split all live
in `docs/conventions/github-mcp.md`.

**Actions configuration lives there too** — the repository-secrets
inventory, the secret-vs-variable-vs-constant rule (**a secret when the
value must be masked in logs**, since Actions masks only secrets), the
nine required status checks and the ruleset they come from, the
**job-vs-step distinction** that decides whether a workflow change needs
a ruleset edit at all — and the fact that the **ruleset** is the gate
while the list recorded in the doc is only a mirror of it — and the
standing consequence that **a fork `pull_request` run receives no
secrets**, so any required check with a secret dependency cannot pass
on a fork PR. Add — or deliberately withhold — the next secret or
required check against those, rather than per-PR.

## AWS infrastructure

AWS resources are **CloudFormation YAML** under `infra/aws/` (network,
IAM, and audit baseline; the market-data warehouse stack builds on
top). Templates pass **both** `cfn-lint` (scoped hook) and the repo's
strict `yamllint`, so they are written to fit the latter — alphabetical
keys, single-quoted strings, block style, folded block scalars for long
ARNs. Authoring is
agent-assisted: documentation lookups go to the credential-free
`aws-docs` MCP server; account actions go to the **AWS CLI** under an
SSO profile, with the SigV4 `aws-mcp` server a **fallback even when
healthy** — its one unique capability is `aws___retrieve_skill` (no CLI
equivalent), and its connection failures are **its own bug, not a
substrate one**. Search the AWS docs before acting and keep to least
privilege (deploys via the passed `*-cfn-deployment` role). The
MCP-gated `*-agent-provisioning` role is **inert by decision**: its
grants need two AWS-MCP keys the wiring never satisfies, so
IAM-creating deploys run from the admin login, and wiring the MCP to
assume it is ruled out. Both servers' wiring is user-local.

**A session cannot log itself in** — `aws login` is interactive — so the
one verb that reads cost data, `plan`, **gates its own launch** on
credentials that resolve (probe, log in, re-probe, refuse); `architect`
deliberately does not. The profile is an **optional**
`DS_AWS_PROFILE` in the untracked runtime config; unset means the CLI's
own default. Note the gate's bound: `sts:GetCallerIdentity` is
authorization-free, so it rules out an **expired** session and not a live
one lacking `ce:*`. Treat credentials as a launch-time
precondition, and note that AWS publishes **no** propagation bound for
an IAM change — verify by retrying the call rather than waiting a folk
interval. Full detail:
`docs/conventions/aws-infra.md` and
`docs/conventions/local-integrations.md` → "The AWS login gate".

## Skill tooling

A skill's deterministic helper (transcript parser, branch check, doc
renderer) is **Python under `.claude/tools/`** (stdlib,
`unittest`-covered), **never** a Cargo workspace member — so it doesn't
compile with the on-chain project. MCP is for prototyping and fallback;
once a workflow is established and repeated, harden it into a Python
tool the skill drives — **`/harden` is the verb** that does it, and it
refuses a shape with no measured recurrence. **Repeated skill *prose*
gets the same treatment**: one source under `.claude/shared/`, filled
into marked regions by `make render-skills` and gated by
`make render-check`, which also fails on a dangling marker. Extract
only genuinely verbatim repetition; no block is extracted today. Full
detail: `docs/conventions/skill-tooling.md`.

## Context economy

Every tool result is fetched once but **replayed as input on every
later turn**, whatever its transport, so a fat early payload is paid
many times over.

**Session length is therefore itself a cost lever, and the largest
one** — cost is roughly **quadratic in session length**, since a
payload's price is its size times the turns that follow it. So **stage
work so a session stays short** (~4–5 levers or findings per PR, target
**300–800 changed lines**, floor ~150 unless trivially mechanical, split
at 1,000) and prefer an occasional **rebase** to a serial chain. This
binds at *breakdown* time, the only place it can. **Smaller is not
monotonically cheaper** — a per-PR overhead floor means a tiny PR costs
far more per line — but the upper bound is **absolute**, not per-line.
Lines are a **proxy** for turns. The metric is **dollars**, and **every
session states its own cost at close**. **Sonnet-tier task workers are
rejected.** Measurements, the standing cost rulings, and the
findings-vs-lines tie-break: `docs/conventions/context-economy.md` →
"Session length is itself a cost lever".

**Resident size is capped**: the `skill-size` lint hook caps skill entry
files and this file at 32,000 bytes, descriptions at 1,024. Over-cap
files are frozen in `cfg/skill-size-baseline.json`; never raise a
ceiling by hand. Same doc → "Resident size has a hard cap".

Per call, request the narrowest thing that
answers the question, read large files by slice (Grep then `Read` with
`offset`/`limit`), route verbose logs away from context — through
`run_quiet.py`, for **any** repeated verbose-on-success runner, `pnpm`
included — and never re-fetch what's already in context. Reading a file
whole is licensed by any **one** of four conditions (edit-plus-brief,
a planned multi-region read, an exemplar you will imitate N times, or
proving an absence that is itself the answer),
not by all of them together — and **citing a file is not one of them**.
The test is **reuse, not size**: name who else will use the content, and
if the answer is "only this edit", slice however small the file.
A harness-persisted tool result is sliced
with `.claude/tools/read_result.py`, never `Read` whole; a file at
another git ref is sliced with `.claude/tools/show_at_ref.py`, never a
whole-blob `git show`. A **script you write is a tool-result
generator** — default it to a ranked summary and gate the full series
behind a flag. Track wasteful
payloads as you go for `/session-metrics`. Full detail:
`docs/conventions/context-economy.md`.

## Shell commands

**Every Bash invocation must reduce to a reusable allow-rule**
(`Bash(prefix:*)`). One bare command per call — no `&&` / `;` / pipes,
no `$(…)` / backticks, no redirects or heredocs, no `cd`. Prefer the
Read / Grep / Glob tools over `cat` / `grep` / `find`; never
`git grep` — and when the Grep tool is absent (it isn't always
present), the fallback is
`python3 .claude/tools/search_source.py '<pattern>'`, which already
prunes the generated families and the never-search trees, or failing
that a bare, single `grep`, on the **main-loop** path too, not only in
the sub-agent brief, and a recursive one is **scoped to source
directories** (it doesn't honor gitignore). Ask for a search's
narrowest form — `-l` for existence, never a recursive `-c` — since
hoisting a verbose sweep only relocates the sink; narrow the **scope**
too (`--dir` / `--glob`), an independent axis, and remember
`--context` scales with match *density*, so clustered matches want
`--files-only` plus a slice-read instead. Keep a
stable command + subcommand prefix and let only the args vary.
Never sweep `.claude/` bare — the worktrees live under it, so a
recursive grep there walks every checkout's `target/` (one such pass
timed out at 120s); scope to `skills` / `tools` / `hooks`. And a
multi-URL `curl` status probe must repeat `-o /dev/null` **per URL**
(the flag binds to one) or use `-I`.
This holds for shell you **author** in skills, scripts, and Makefile
targets too, and for work you hand a sub-agent. Three opt-in
`PreToolUse` guard hooks mechanically enforce these rules:
`.claude/hooks/no_compound_bash.py` blocks compounds (escape marker
`#compound-ok`), `.claude/hooks/no_git_grep.py` blocks `git grep`
(no escape hatch, deliberately — use the Grep tool), and
`.claude/hooks/no_destructive_bash.py` blocks destructive commands in
two tiers (an overridable ask via `#destructive-ok`, and a small
catastrophic deny no marker lifts). Their wiring is user-local (see
"Local integrations and guard hooks" below). The rules and the
always-re-prompt patterns: `docs/conventions/shell-commands.md`.

**Firming has three homes and no per-session sweep.** `allowlist.py add`
ad hoc (the dominant path), `fewer-permission-prompts` on demand, and
the upkeep pass's refresh gate on a roughly **monthly** clock — the only
periodic adder, because the auto permission-mode classifier that made a
per-session sweep unnecessary is also what makes the remaining churn
invisible. It proposes; `plan`'s bootstrap gate approves. The same pass
reports cruft to prune. Do not reinstate a per-session pass;
the bar is net-negative instruction cost.

## Local integrations and guard hooks

The **user-local Claude Code configuration** the repo documents but
does **not** commit: the compound-shell, **git-grep** (absolute;
revision-scoped search is served by `show_at_ref.py`), destructive-command
(best-effort advisory, not a policy boundary) and AI-attribution
(inspects only message/body *values*, so searching for those strings
stays legal) guards described above; the **worktree edit-path guard**
(blocks a file-mutating tool targeting a base-repo absolute path from a
worktree session — a recurring, expensive slip); and the **private-key
guard** with its `permissions.deny` rules (no agent session or
sub-agent touches the mainnet keys or runs a mainnet operation; no
escape marker). Also the iTerm2
tab-color integration and the shell setup they lean on — including the
**session secrets**, resolved from 1Password at session launch, never
written into a config file, the CLI signed out right after the reads.
A 1Password SSH prompt for a private key from anything but the cockpit
(the deploy ceremony's key commands) is refused, commit signing
excepted. Both settings files are git-ignored, so all
of it is opt-in. `settings.local.json` is **one shared file resolved
through worktrees to the main checkout**, so wiring it once makes it
live everywhere. A committed guard is **inert until wired**:
`make hook-wiring` (its tool also runs in the daily upkeep pass) names every
guard that
cannot fire — unwired, under a `matcher` that never selects its tools,
or pointed at a missing path — and writes nothing. The **session
helpers** (`cdds`, `task`, `explore`, `chore`, `plan`,
`architect`, `fleet`) are the exception that *is* committed, at
`.claude/shell/init.zsh`, sourced from the base checkout by one guarded
line in the shell profile; only their 1Password coordinates stay
untracked — defined as env vars in the runtime config, or in an
optional file outside the repo. Full detail — every hook's
wiring, the helper family, and the iTerm setup:
`docs/conventions/local-integrations.md`.

### Session substrate: capability, not attendance

A session runs on **Bedrock** unless it needs something Bedrock lacks —
web search or deep research (web fetch works there) — or it is a **seat
session by role** (`architect`). Sub-agents are not a differentiator.
`task`, `chore`, `explore` and `plan` are Bedrock, with `anthropic`
the escape (`plan anthropic` puts planning back on the seat).

Models are named by **role tier** — advisor (`plan`, `architect`,
`explore`), executor (`task`), helper
(`chore`), background — each set in
the untracked runtime config (`DS_MODEL_<TIER>[_SUBSTRATE]`). Each verb
pins its tier at launch and refuses an unresolved one, so no skill
checks its own model.

A launch **records its substrate** in an untracked marker that resume
verbs re-export (re-pinning the tier); an absent marker reads as
**anthropic**, and an anthropic launch **clears** inherited Bedrock
exports. Detail and the verb table:
`docs/conventions/local-integrations.md`.

A planning session may **dispatch** a ready task — `session_dispatch.py`
opens a **tab in the dispatching session's own window** (never a new
window) and types the verb, authorized by the operator's yes as
`fleet go` is; several verbs go in one call, separated by a bare `+`.
iTerm is driven only via `.claude/tools/iterm_api.py` (its Python API);
**AppleScript is retired**.

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
material passed inline — to **every** `Agent` prompt. The brief is
verbatim (copy it to prepend) in
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
