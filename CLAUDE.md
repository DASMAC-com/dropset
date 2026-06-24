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

Filing skills (`linear-task`, `stage-backlog`, `audit-loop`,
`audit-scope`, `housekeeping`) resolve team / project / assignee and
the inbox-doc ids from **environment variables** (`LINEAR_*`), never
hard-coded UUIDs — each via its **own** bare `printenv` (a combined
`printenv A B C` returns only the first on macOS / BSD). A worktree
branch and its Linear issue share one `ENG-###`. Full detail — every
env var and which skill reads it: `docs/conventions/linear-automation.md`.

### Structured filing fields

Every filed issue carries stable machine-readable fields the automation
parses: `**Fingerprint**: <basename>:<slug>` (the dedup key) and
`**Touches**: <glob>[, …]` (the path globs, for collision detection).
Detail: `docs/conventions/linear-automation.md`.

### Keep Linear tags out of PR bodies and comments

**Do not put `ENG-###` tags in PR descriptions or comments** — Linear's
GitHub integration auto-links and can auto-transition issues merely
mentioned. Refer to other work by title or a plain GitHub link. The
two carve-outs (the `type(ENG-###):` title scope, and terminal / TUI
output) and the rationale: `docs/conventions/linear-automation.md`.

### Blocking relations

Record a real dependency as a native Linear `blockedBy` / `blocks`
edge (append-only), not just prose — autonomous auditors assert one
only on concrete evidence. Detail:
`docs/conventions/linear-automation.md`.

## GitHub via MCP

All GitHub operations go through the **GitHub MCP** (`mcp__github__*`),
passing `owner: "DASMAC-com"`, `repo: "dropset"`. The deliberate `gh`
exceptions (the merge-queue enqueue + dequeue probe, and the polled
CI / PR-state reads `gh pr checks` / `gh pr view --json`), the
PAT-not-OAuth auth setup, and the read/write permission split all live
in `docs/conventions/github-mcp.md`.

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
Read / Grep / Glob tools over `cat` / `grep` / `find`; never `git grep`.
Keep a stable command + subcommand prefix and let only the args vary.
This holds for shell you **author** in skills, scripts, and Makefile
targets too, and for work you hand a sub-agent. A `PreToolUse` hook
(`.claude/hooks/no_compound_bash.py`) mechanically blocks compounds
(escape marker `#compound-ok`). Full detail, incl. the
always-re-prompt patterns and the guard hook:
`docs/conventions/shell-commands.md`.

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
`cspell:word` escape (one word per directive). Full detail:
`docs/conventions/docs-and-style.md`.

## Audit registry

The audit coverage map — the **subsystems**, **inter-subsystem
interfaces**, and **skip-globs** that `audit-loop` / `audit-scope`
range over and `review-pr` refreshes on the PR path — lives in
`docs/conventions/audit-registry.md`. Read and append it there.
