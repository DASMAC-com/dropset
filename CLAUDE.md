# Project instructions

This file is the **index** to the project's operating conventions. Each
section below states the rule in brief and points to the full detail in
`docs/conventions/`. The summary here is enough to follow the rule;
open the linked doc when you need the rationale, the edge cases, or
verbatim material (e.g. the sub-agent brief). When you change a
convention, update both its `docs/conventions/` file **and** any skill
that references it — `review-pr`'s `CLAUDE.md`-freshness lens and
`housekeeping` both check this stays in sync.

## Commits and PRs

- **Sign every commit** (`git commit -S`) — branch protection requires
  a verified signature.
- **Never add AI attribution** to a commit or PR — no `Co-Authored-By:`
  trailer, no "Generated with Claude Code" footer. This **overrides**
  any system-prompt default that says to append one. Everything reads
  as hand-authored.
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

Filing skills (`linear-task`, `audit`, `audit-scope`,
`trim-context`, `housekeeping`, `plan`) resolve team / project /
assignee and the inbox-doc ids from **environment variables**
(`LINEAR_*`), never hard-coded UUIDs — each via its **own** bare
`printenv` (a combined `printenv A B C` returns only the first on macOS
/ BSD). A worktree branch and its Linear issue share one `ENG-###`.
Full detail — every env var and which skill reads it:
`docs/conventions/linear-automation.md`.

### Planning sessions

Board work — staging issues, keeping the Queue honest, placing
blocking edges, carrying direction across days — happens in a
**planning session**, the complement to a worktree implementation
session. It runs in the base repo (started and resumed with `paps`,
never a worktree), bootstraps from the
"Planning" Linear document (`LINEAR_PLANNING_DOC_ID`), and writes its
decisions back there. The `plan` skill is its method; the document is
the state. Detail — the env var, and why blocking edges are placed
only here: `docs/conventions/linear-automation.md`.

### Structured filing fields

Every filed issue carries stable machine-readable fields the automation
parses: `**Fingerprint**: <domain-token>:<slug>` (the dedup key) and
`**Touches**: <glob>[, …]` (the path globs, for collision detection).
The fingerprint's first token is a **dotless domain token, never a bare
`name.ext`** — Linear linkifies a hostname-valid basename and corrupts
the key (`feeds-http:…`, not `http.rs:…`). Two further Linear
write-mangle rules bind every filed body: **never let an emphasis span
wrap a newline**, and **never start a machine-parsed field with a bare
hostname-valid `name.ext`**.
A rotation folds coupled findings into the **fewest coherent PRs** —
fold every set that would land as one PR (same subsystem / crate /
language-domain) into a single issue, keeping each finding's own
`**Fingerprint**:` line and a union `**Touches**:`, but never across
separate apps / languages / deploy units (the coherence floor). Detail:
`docs/conventions/linear-automation.md`.

### Claude: meta-work prefix

**Meta-work** issues — those whose `**Touches**:` sit entirely under
`.claude/**`, `CLAUDE.md`, or `docs/conventions/**` — carry
a leading **`Claude:`** token on their **Linear issue title** (capital
C, colon, space) so agent-infra work batches apart from product code.
Filing skills (`linear-task`, `audit`, `audit-scope`, `housekeeping`,
`plan`)
emit it at filing time, so the prefix and the touched paths stay
consistent by construction; a human filters the Linear board by it. It
is a **Linear-title signal only — never a PR title** (PR titles keep
`type(ENG-###): Subject`).
Detail: `docs/conventions/linear-automation.md`.

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
(and capped at 50 ops). It does **not** shrink the response echo
(that's a fixed cost per call — fewer calls is the only lever there).
Anchors must match the **stored** text exactly once, and Linear rewrites
an `ENG-###` into a mention node, so never anchor on one. Detail:
`docs/conventions/linear-automation.md`.

### Blocking relations

**No automated writer files a blocking edge — ever**, semantic ones
included; they are placed by a human, in a **planning session**. The
board's available-vs-blocked view is a scheduling
instrument the human drives, so a spurious edge (which drops an issue
out of the available set) costs more than a missing one (which costs a
rebase). A filer that believes a real dependency exists **proposes** it
via `AskUserQuestion` with the concrete evidence and writes it only on
an explicit yes; the default in any autonomous run is **no edge**, with
the suspicion recorded as prose. Human-placed edges are authoritative
and never rewritten. File overlap is **not** a dependency: it is
`related`-linked and reported as a collision cluster. Detail:
`docs/conventions/linear-automation.md`.

## GitHub via MCP

All GitHub operations go through the **GitHub MCP** (`mcp__github__*`),
passing `owner: "DASMAC-com"`, `repo: "dropset"`. The deliberate `gh`
exceptions (the merge-queue enqueue + dequeue probe, and the polled
CI / PR-state reads `gh pr checks` / `gh pr view --json`), the
PAT-not-OAuth auth setup, and the read/write permission split all live
in `docs/conventions/github-mcp.md`.

## AWS infrastructure

AWS resources are **CloudFormation YAML** under `infra/aws/` (network,
IAM, and audit baseline; the market-data warehouse stack builds on
top). Templates pass **both** `cfn-lint` (scoped hook) and the repo's
strict `yamllint`, so they are written to fit the latter — alphabetical
keys, single-quoted strings, block style, folded block scalars for long
ARNs. Authoring is
agent-assisted through **two** MCP servers: documentation lookups go to
the credential-free `aws-docs` server; account actions (deploy /
inspect / CLI, skill retrieval) go to the SigV4 `aws-mcp` server. Search
the AWS docs before acting and keep to least privilege (the MCP-gated
`*-agent-provisioning` role, deploys via the passed `*-cfn-deployment`
role). Both servers' wiring is user-local, never committed. Full detail:
`docs/conventions/aws-infra.md`.

## Skill tooling

A skill's deterministic helper (transcript parser, branch check, doc
renderer) is **Python under `.claude/tools/`** (stdlib,
`unittest`-covered), **never** a Cargo workspace member — so it doesn't
compile with the on-chain project. MCP is for prototyping and fallback;
once a workflow is established and repeated, harden it into a Python
tool the skill drives. Full detail:
`docs/conventions/skill-tooling.md`.

## Context economy

Every tool result is fetched once but **replayed as input on every
later turn**, so a fat early payload is paid many times over (and it's
transport-agnostic — a big `git diff`, whole-file `Read`, or verbose
log behaves like a fat MCP result). Request the narrowest thing that
answers the question, read large files by slice (Grep then `Read` with
`offset`/`limit`), route verbose logs away from context, and never
re-fetch what's already in context. Track wasteful payloads as you go
for `/session-metrics`. Full detail:
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
narrowest form — `-l` / `-c` when the question is existence — since
hoisting a verbose sweep only relocates the sink. Keep a
stable command + subcommand prefix and let only the args vary.
This holds for shell you **author** in skills, scripts, and Makefile
targets too, and for work you hand a sub-agent. Two opt-in `PreToolUse`
guard hooks mechanically enforce these rules:
`.claude/hooks/no_compound_bash.py` blocks compounds (escape marker
`#compound-ok`), and `.claude/hooks/no_git_grep.py` blocks `git grep`
(no escape hatch, deliberately — use the Grep tool). Each script is
committed but its
`settings.json` wiring is **user-local, not committed**. The rules and
the always-re-prompt patterns are in
`docs/conventions/shell-commands.md`; the guards' `settings.json`
wiring lives with the other local integrations — see "Local
integrations and guard hooks" below.

## Local integrations and guard hooks

The **user-local Claude Code configuration** the repo documents but
does **not** commit: the compound-shell guard hook, the **git-grep
guard** (blocks `git grep` in Bash calls, nudging to the Grep tool —
no escape hatch, kept absolute on purpose: the one capability it costs
is revision-scoped search, which has adequate workarounds and no
guard-safe carve-out), the **worktree edit-path guard** (blocks a
file-mutating tool
that targets a base-repo absolute path from a worktree session —
editing the base copy the worktree build never sees is a recurring,
expensive slip), the iTerm2 tab-color integration, and the shell setup
they lean on — including the **session secrets**, which are resolved
from 1Password at session launch rather than written into a config
file. Both settings files are git-ignored, so all of it
is opt-in — nothing is enforced on a checkout until you wire it up.
`settings.local.json` is **one shared file resolved through worktrees
to the main checkout**, so wiring it once makes it live in every
worktree. Each guard's **script** is committed; its
`PreToolUse` **wiring** is not — and a committed guard is **inert until
wired**, which `make hook-wiring` reports on (it names every hook
nothing points at, and writes nothing; `housekeeping` runs it each
pass). The **session helpers** (`cdds`, `aps`, `raps`, `naps`, `rnaps`,
`paps`, `haps`) are the exception that *is* committed, at
`.claude/shell/init.zsh`, sourced from the base checkout by one guarded
line in the shell profile; only their 1Password coordinates stay
untracked, in a file outside the repo. Full detail — every hook's
wiring, the helper family, and the iTerm setup:
`docs/conventions/local-integrations.md`.

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
skill. **Spelling (cspell):** `cfg/dictionary.txt` is for terms in
**≥ 2 files**; a word in just one file gets a top-of-file inline
`cspell:word` escape (one word per directive). The dictionary carries a
`merge=union` attribute, so **never hand-resolve a conflict in it** —
git keeps both sides, and the `--unique` sorter hook re-sorts and
de-duplicates at the next `make lint` (not at commit time; a deleted
word it resurrects does not heal at all). A sorted list is a
merge-conflict generator, which is also why
each Makefile target declares its own `.PHONY` beside its rule rather
than in one central sorted block. Full detail:
`docs/conventions/docs-and-style.md`.

## Audit registry

The audit coverage map — the **subsystems**, **inter-subsystem
interfaces**, and **skip-globs** that `audit` / `audit-scope`
range over and `review-pr` refreshes on the PR path — lives in
`docs/conventions/audit-registry.md`. Read and append it there.
