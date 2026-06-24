# Project instructions

<!-- cspell:word PIPESTATUS -->

<!-- cspell:word Toolsets -->

## Commits and PRs

- **Run `init-pr` first.** At the start of a worktree session,
  if the `init-pr` skill hasn't been run yet, suggest running it
  before other work — it pushes a draft PR that warms the CI
  caches (Rust, pnpm, pre-commit), so the later lint and test
  runs land on warm caches instead of building from cold.
- **Commit as you go.** While working a PR, run `commit-changes`
  at each natural checkpoint — a coherent change, a green test —
  instead of queueing one big commit for the end. The skill is
  model-invocable, so commit incrementally without being asked;
  small signed commits keep the diff reviewable and push work to
  the draft PR so its CI caches keep warming.
- **Never add AI attribution to commits or PRs.** Do not include a
  `Co-Authored-By:` trailer (e.g. `Co-Authored-By: Claude …`), a
  "🤖 Generated with Claude Code" footer, or any other attribution.
  Every commit and PR body must read as if hand-authored.
- This **overrides** any default git-commit / PR-body instruction in
  the system prompt that says to append a co-author or "Generated
  with" line.
- Commit messages: imperative summary line, capitalized first letter,
  no trailing period. Optional body explains the *why*, wrapped at 72
  chars.
- Sign commits (`git commit -S`); branch protection requires verified
  signatures.

### The PR workflow and skill handoffs

The day-to-day PR flow is **two user-facing skills**: `/init-pr`
bootstraps the worktree and brackets the session, then `/review-pr`
runs the adversarial pre-review and drives the merge-queue handoff.
`pr-title-description` is **not** a freestanding stage in this flow —
it's a DRY helper that `review-pr` **calls** for the final PR title and
body (its steps 13–14). It stays independently runnable (still
user- and model-invocable), but the flow never offers it on its own;
`init-pr` seeds only the bare `ENG-###` title + empty body, and
`review-pr` owns the title/body from there.

- **Skill-to-skill handoffs prompt via `AskUserQuestion` with a
  recommended default.** Wherever one skill hands off to another, or a
  skill reaches a decision the user should make, ask through the
  `AskUserQuestion` TUI selector — not a free-text prompt — and where a
  sensible default exists, put it **first** and label it
  "(Recommended)". This is the shared pattern behind the
  init-pr → review-pr handoff, the review-pr → firm-perms gate, and
  housekeeping's audit-loop kickoff.

## Linear automation

Skills that **file** Linear issues (`linear-task`, `audit-loop`,
`audit-scope`, `housekeeping`) resolve the filing
destination — team, project, assignee — from **environment
variables**, never hard-coded UUIDs. (Skills that only **update**
an existing issue by id — `init-pr`, `review-pr` — need no
destination; `stage-backlog` only rewrites the Task Staging document,
reading `LINEAR_PROJECT_ID` as a query filter — see its own paragraph
below.) Set them once in your
shell profile (`~/.zshrc`):

```sh
export LINEAR_TEAM_ID=…
export LINEAR_PROJECT_ID=…
export LINEAR_ASSIGNEE_ID=…
# Used only by stage-backlog — the "Task Staging" document:
export LINEAR_TASK_STAGING_DOC_ID=…
# Used only by firm-perms (and housekeeping, which calls it) —
# the "Permissions" inbox document it drains:
export LINEAR_PERMISSIONS_DOC_ID=…
# Used by session-metrics (producer) and housekeeping (consumer) —
# the "Session Metrics" inbox document one appends to and the other
# mines into propose-only skill-improvement tasks:
export LINEAR_SESSION_METRICS_DOC_ID=…
# Used only by the stage-backlog Python tool (the deterministic
# core of the stage-backlog skill) — a personal Linear API key. A
# script can't use the OAuth-based claude.ai Linear MCP, so it
# authenticates with this key, sent as the Authorization header.
# Never commit it.
export LINEAR_API_KEY=…
```

Skills read these at run time with a bare `printenv`, **one variable
per call** — `printenv LINEAR_TEAM_ID`, then
`printenv LINEAR_PROJECT_ID`, then `printenv LINEAR_ASSIGNEE_ID`. Do
**not** fold them into one
`printenv LINEAR_TEAM_ID LINEAR_PROJECT_ID LINEAR_ASSIGNEE_ID`: macOS /
BSD `printenv` honors only its **first** operand, so the combined form
returns just `LINEAR_TEAM_ID` and the skill wrongly concludes the
other two are unset and halts. Each bare
call still matches the same `Bash(printenv:*)` allow-rule, so none of
them re-prompt. A new Linear-filing skill must follow the same
pattern: reference the variable **names**, and keep the resolved
UUIDs out of every committed file.

`stage-backlog` additionally resolves `LINEAR_TASK_STAGING_DOC_ID`
— the id of the Linear document it rewrites each run (the "Task
Staging" document) — with its own bare `printenv`, on the same rule.
It is not a filing destination, so the other skills don't need it.

`firm-perms` likewise resolves `LINEAR_PERMISSIONS_DOC_ID` — the id
of the "Permissions" inbox document it drains in its `doc` mode (and
that `housekeeping` drains via `firm-perms` each pass) — with its own
bare `printenv`, on the same rule. It too is not a filing
destination.

`session-metrics` and `housekeeping` share
`LINEAR_SESSION_METRICS_DOC_ID` — the id of the "Session Metrics"
inbox document — each resolving it with its own bare `printenv`, on
the same rule. `session-metrics` is the **producer**: it appends one
dated entry per session (the measured token sinks plus tailored trim
recommendations). `housekeeping` is the **consumer**: each pass it
mines the unprocessed entries for recurring trim levers and files them
as propose-only skill-improvement Backlog tasks (never editing a skill
itself). Either skill no-ops with a clear message when the variable is
unset. It is not a filing destination. The `session-metrics` skill
drives its binary via `make session-metrics`, which reduces to a
`Bash(make session-metrics:*)` allow-rule. Like the stage-backlog
tool, this binary needs **no** `LINEAR_API_KEY` — it
only parses the local transcript and makes no network call; the skill
does the one Linear write (the doc append) over the MCP.

The **stage-backlog Python tool** (the deterministic core of the
`stage-backlog` skill — see "Structured filing fields" below) is a
single, dependency-free `python3` script at
`tools/stage-backlog/stage_backlog.py`, driven by `make stage-backlog`
(which preserves the `Bash(make stage-backlog:*)` allow-rule). It is
**render-only**: it reads the open Backlog and
rewrites the Task Staging document, and that document write is its
**only** Linear write — no `merge` subcommand, no issue-folding or
duplicate-closing. Two issues that belong in one PR render as nested
serial chips, never a folded issue. It uses the standard library only
(`urllib` + `json`) for its two GraphQL calls, so it adds no
dependency to the Rust build and inherits the repo's `ruff` hooks. It
reads `LINEAR_PROJECT_ID` plus its own `LINEAR_API_KEY` (a personal
Linear API key, because a script can't ride the OAuth-based
`claude.ai` Linear MCP) for every run; for a real write it also reads
`LINEAR_TASK_STAGING_DOC_ID` (the document it rewrites), while
`--dry-run` prints the tree to stdout and doesn't require it. It
resolves all of these via `os.environ`, never a hard-coded id, and the
key is never committed.

### Structured filing fields

Every filed issue carries machine-readable fields the automation reads
back, on top of the human prose. Keep the field **names** stable — the
filing skills emit them and `stage-backlog` parses them:

- `**Fingerprint**: <basename>:<slug>` — the dedup key `audit-loop`
  matches on so a finding is never refiled. Mandatory on audit
  findings; one line per finding (a merged issue carries several).
- `**Touches**: <glob>[, <glob>…]` — the path globs the fix will
  edit, comma-separated. Declare the **directory** when the work
  spans a dir (`tui/`), the **file** when it's one file
  (`programs/dropset/src/swap.rs`); list every glob for a multi-file
  finding. The `stage-backlog` renderer reads this to detect file
  collisions **deterministically** — a directory glob collides with
  any path under it, and two issues that collide can't run in
  parallel, so the higher-numbered one nests under the lower. Moving
  this structure to **filing time** is what lets the tool skip the
  prose-reading sub-agent it used to need; an issue that predates the
  field falls back to declared-edge/parent placement, and the skill's
  agent step reconciles it.

A worktree branch and its Linear issue **share one `ENG-###`
number**: branch `eng-499` ↔ issue `ENG-499`. Skills resolve the
issue from the branch (or the PR title scope) on that basis —
`init-pr` moves it to In Progress at bootstrap, `review-pr` ticks the
delivered checklist items and moves it to In Review at the merge-queue
handoff — once the PR is ready, CI is green, and the review summary has
been printed for the human.

### Keep Linear tags out of PR bodies and comments

**Do not put Linear issue tags (`ENG-###`, e.g. `ENG-513`) in PR
descriptions or PR comments.** Linear's GitHub integration auto-links
any `ENG-###` it finds in a PR's body or comments, which can attach the
PR to — and even auto-transition — issues it merely *mentions* (a
"follow-up to ENG-512" note wrongly pulls that issue into this PR's
lifecycle). The branch name already carries the tag and links the PR to
its own issue, so tags in the prose are redundant and risk spurious
cross-links. Refer to other work by **title** or a **plain GitHub
link**, never its Linear tag, in PR prose.

Two carve-outs:

- **The PR *title* keeps its scope.** `semantic-pr` requires the title
  to be `type(ENG-###): Subject`, and the branch ↔ issue convention
  depends on it, so the `ENG-###` in the **title scope** stays. The
  rule is about the **body and comments only**, never the title.
- **Terminal / TUI output is exempt.** `review-pr`'s `AskUserQuestion`
  prompts deliberately print the Linear tag + PR number so the human
  can pull up the PR. That's terminal chrome, not PR content, so it's
  unaffected.

The skills that author PR prose follow this: `pr-title-description`
(the PR body) and `review-pr` (any PR comment it posts, and the body
refresh) keep `ENG-###` in the title scope and omit it from
body/comments; `init-pr` seeds only the bare-`ENG-###` title + an empty
body, so it already complies.

### Blocking relations

When one issue genuinely depends on another, record it as a **native
Linear relation**, not just prose. `save_issue` takes `blockedBy`
(the `ENG-###`s that must land first) and `blocks` (the `ENG-###`s
this one gates), both by identifier; they are **append-only** — they
add edges and never clear existing ones, so use `removeBlockedBy` /
`removeBlocks` to drop one. Recording a real edge keeps the blocker
visible and prioritized so dependent work doesn't rot waiting on an
upstream nobody remembers, and `stage-backlog` reads these edges and
nests its dependency tree on them. Assert only a dependency you
actually know to be real; omit it when unsure.

`linear-task` sets these from a person's call. The **autonomous**
auditors (`audit-scope`, `audit-loop`) work under a tighter rule:
they may assert a relation **only on concrete evidence** that one
finding's fix cannot land until another issue resolves (e.g. a nit
that depends on an `arch:` proposal filed the same run), never a
speculative "these feel related" edge. Mere coupling — work that
belongs in *one PR* — is handled by combining into a single issue,
not a relation. When the blocker is filed in the same run, file it
first so its `ENG-###` exists, then reference it.

## GitHub via MCP

All GitHub operations — opening PRs, updating titles and bodies,
reading the diff, watching checks, pulling failing-job logs — go
through the **GitHub MCP server** (`mcp__github__*`), not the `gh`
CLI, **with the deliberate exceptions below**. The skills (`init-pr`,
`pr-title-description`, `review-pr`, `housekeeping`, `linear-task`)
are written against it. `gh` survives in two places, both in
`review-pr`:

- **The merge-queue handoff** — the enqueue (a `gh pr merge --auto`
  write, **no** strategy flag: this repo's merge queue sets the
  strategy, so a `--squash` only warns) and a read-only dequeue probe
  (a `gh api graphql … mergeQueueEntry` read). The enqueue stays on
  `gh` because the server exposes no auto-merge / merge-queue tool
  (`merge_pull_request` does an *immediate* merge, which bypasses the
  queue); the probe stays on `gh` because the hosted MCP's
  `pull_request_read` omits the merge-queue state — and on a
  merge-queue repo a still-queued PR reports `autoMergeRequest: null`,
  so the probe must read `mergeQueueEntry` (non-null while queued)
  over GraphQL to tell a still-queued PR from one that was dequeued.

- **The CI-wait and PR-state reads** — `gh pr checks <number>` for the
  CI-wait poll, and `gh pr view <number> --json <fields>` for the
  one-shot `mergeable` / PR-lookup reads. These reads are **polled
  repeatedly** across the CI and merge-queue waits, and the MCP
  equivalents (`pull_request_read` `get` / `get_check_runs`) return
  the **full** PR object or check array on every poll — a fat payload
  that, because a tool result is replayed as input on every later
  turn (see "Context economy" below), is paid many times over.
  `gh pr checks` is one compact line per check, and `--json <fields>`
  selects only the fields the decision needs. `--json` / `--jq` are
  command **flags**, not shell pipes, so they stay shell-rule-clean
  and reduce to `Bash(gh pr checks:*)` / `Bash(gh pr view:*)`
  allow-rules. This is the one place a `gh` read is preferred *over*
  the MCP: when the call repeats and the payload — not the transport —
  is the cost. Keep the poll **model-driven** (a fresh call paced by
  `ScheduleWakeup`), never a shell `while … sleep` loop or a `jq`
  filter; the failure path still pulls logs via `get_job_logs`.

Everything else stays MCP-first; `gh` is not a general-purpose escape
hatch.

Every tool takes `owner` and `repo`. This repo is
`DASMAC-com/dropset`, so pass `owner: "DASMAC-com"`, `repo: "dropset"`
on every call. The server collapses most reads into a single tool
dispatched by a `method` enum — `pull_request_read` covers `get` /
`get_diff` / `get_files` / `get_check_runs` / …, and `actions_list` /
`actions_get` do the same for Actions — so one tool name covers many
reads.

### Authentication (PAT header, not OAuth)

The server is added at **user scope** with a PAT in an `Authorization`
header, on the same env-var convention as the `LINEAR_*` ids — the
token lives in `~/.zshrc`, never in a committed file or
`~/.claude.json`:

```sh
export GITHUB_MCP_PAT=…
```

```sh
claude mcp add --transport http --scope user github \
  https://api.githubcopilot.com/mcp/ \
  --header 'Authorization: Bearer ${GITHUB_MCP_PAT}' \
  --header 'X-MCP-Toolsets: all'
```

Two gotchas, both learned the hard way:

- **OAuth doesn't work.** Claude Code's built-in OAuth needs dynamic
  client registration, which this server doesn't support
  (`does not support dynamic client registration`). The PAT header is
  the only path; a classic `repo` token already covers PRs and Actions
  read+write, so nothing extra is needed.
- **A newly added or reconfigured server loads on the next
  conversation, not mid-session.** After `claude mcp add` (or any
  header change), relaunch and start a fresh chat before the
  `mcp__github__*` tools appear.

The `X-MCP-Toolsets: all` header exposes the `actions` toolset (check
runs, job logs) alongside the defaults. The tradeoff: it also surfaces
write tools across every toolset (Dependabot, secret-scanning,
notifications, …); per-tool permission prompts are the backstop.

### Permission rules

Pre-approve the **reads** *and* the routine **PR-authoring writes** so
they don't re-prompt, and leave the genuinely destructive / irreversible
writes to confirm-on-use:

- **Pre-approve (reads):** `pull_request_read`, `list_pull_requests`,
  `actions_list`, `actions_get`, `get_job_logs`, `get_me`, and the
  `search_*` family.
- **Pre-approve (the companion `gh` reads, as `Bash(…)` rules):**
  `Bash(gh pr checks:*)`, `Bash(gh pr view:*)`, and
  `Bash(gh api graphql:*)` — the polled / field-selected reads
  `review-pr` uses in place of the full-object MCP calls (see "GitHub
  via MCP" above and "Context economy"). These are Bash globs, not
  `mcp__github__*` entries, but they're pre-approved on the same
  rationale (routine, low-blast-radius reads) and propagated to the
  base repo so future worktrees inherit them.
- **Pre-approve (routine PR-authoring writes):** `create_pull_request`
  (init-pr) and `update_pull_request` (pr-title-description, review-pr).
  The skills call these on every run to open and maintain the draft PR,
  and they touch only the PR's own title / body / draft-state — low
  blast radius — so gating them behind a confirm prompt each run buys no
  safety. Pre-approving them is deliberate.
- **Confirm-on-use (merges, deletes, pushes, issue/actions
  mutations):** `merge_pull_request`, `delete_file`, `push_files`,
  `create_or_update_file`, `issue_write`, `actions_run_trigger`. These
  either land code, delete content, or mutate issues/workflows — the
  irreversible or far-reaching writes that warrant a per-use confirm.

The split, in one line: **pre-approve reads + the routine PR-authoring
writes; confirm-on-use for merges, deletes, pushes, and issue/actions
mutations.**

These are `mcp__github__<tool>` permission entries, not `Bash(…)`
globs — and because of the single-tool-many-methods shape, one
allow-rule per read tool covers all of its methods. Propagate the
pre-approved allow-rules (reads *and* the PR-authoring writes) to the
**base-repo** settings so future worktrees inherit them (per the
per-worktree settings rule); `firm-perms` does this at session end.

## Context economy

**Request less; you usually can't trim more.** An LLM is stateless, so
every turn re-sends the whole conversation as *input*. A tool result
is fetched **once** but **replayed as input on every later turn** for
the rest of the session — the MCP server (or shell, or file) is not
re-queried; it's the transcript replay that recurs. The prompt cache
discounts the replay (~10%) but the tokens are still counted and still
occupy the finite window. So a fat payload early in a long session is
paid many times over. **This is transport-agnostic** — a large
`git diff`, a whole-file `Read`, or a verbose build log behaves
exactly like a fat MCP result; `gh` vs. the MCP is token-neutral for
the same data. The only durable lever is **how much each call returns
into the transcript**:

- **Ask for the narrowest thing that answers the question.** Use the
  narrowest method / subcommand, field-select where the transport
  allows it (`gh … --json <fields>`, a GraphQL projection), paginate
  instead of dumping, and **never re-fetch what's already in context**.
- **Read large known files by slice.** Grep to locate, then `Read`
  with `offset`/`limit`; don't pull a 1000-line file to use 80 lines
  of it. Brief review sub-agents to do the same.
- **Route verbose build logs away from context.** Prefer `-q` /
  `--quiet` so a `cargo` / `make` "Compiling …" cascade doesn't land
  inline; for a noisy target, surface only the tail / the result, not
  the whole stream. (Do this within the shell rules — no redirect; a
  quiet flag is a flag.)
- **Scope a sub-agent fan-out.** Inlining the same large diff into N
  reviewers pays for N resident copies; scope each agent to its files,
  or have them read one shared file, rather than inlining N times.
- **Polls multiply payload.** A read issued once is cheap; the same
  read polled across a CI / merge wait is paid per poll *and* per
  later turn — that's why `review-pr`'s waits use the compact `gh`
  reads above rather than the full-object MCP calls.

**Track consumption ideas as you go.** When something reads as
wasteful mid-session — a payload you only needed a slice of, a call
that repeated, an avoidable fan-out — keep a running note of it. At
session end `/session-metrics` pairs those observations with the
binary's ranked token sinks to emit *grounded* trim recommendations
(the lever, and the concrete skill / `CLAUDE.md` edit it implies) into
the Linear "Session Metrics" inbox, which `housekeeping` later mines.
The binary says *where* the tokens went; your running notes say *why*
and *what to change*.

## Docs and skills prose

**Refer to users in the abstract, never by name.** Committed docs and
skills (`.claude/skills/**`, `CLAUDE.md`, `docs/**`) should read as if
written for any user of the tool, so a particular individual's name
never appears in the prose — write "the user", "you", or "whoever runs
it" instead. The skill suite is general-purpose tooling; hard-coding
one person's name makes it read as bespoke and dates poorly. This is
about **prose only** — the env-var-resolved assignee / filing-destination
ids (`LINEAR_ASSIGNEE_ID`, etc.) are configuration, not prose, and are
unaffected.

## Spelling (cspell)

`cfg/dictionary.txt` is the **project-wide** spelling allow-list —
reserve it for terms that recur across the codebase. The rule: a word
belongs in `dictionary.txt` only if it appears in **≥ 2 files**. A term
used in just one file gets an inline escape in that file instead, by
comment style:

- Rust / TS / JS — `// cspell:word foo`
- Markdown — `<!-- cspell:word foo -->`
- YAML / TOML / shell — `# cspell:word foo`

The lone exception is a file that can't carry a comment (e.g.
`.json`), where the dictionary is the only option.

**Placement: one block at the top of the file, one word per line.**
All of a file's inline escapes go together in a single block at the
very top, never scattered beside each usage, and **each escaped word
gets its own directive on its own line** — never pack multiple words
into one comment. In **line-comment** files (Rust / TS / JS `//`, YAML
/ TOML / shell `#`) that's one directive per word on consecutive lines
with no blank lines between. In **Markdown** it's one
`<!-- cspell:word foo -->` per word, but mdformat inserts a blank line
between adjacent HTML comments, so the block is a blank-line-separated
stack of single-word comments — that's expected and stable, not drift.
"Top" means the first line, except where syntax forces something else
to lead: after a `---` YAML frontmatter block, after a `#!` shebang, or
after a leading module doc-comment / inner-attribute header. One known
place, one word per line, means a reader — and the audit — finds every
escape at a glance instead of hunting the file.

The `cspell-audit` skill reconciles the dictionary against actual usage
**and** normalizes escape placement on this rule; run it when the
dictionary grows or escapes drift. `housekeeping` runs the same check
read-only and files any drift — a dictionary entry to move, or
mis-placed escapes to regroup — as a Backlog task.

## Shell commands

The guiding rule: **every Bash invocation should reduce to a
reusable allow-rule** (`Bash(prefix:*)`). A call that can't —
because of a compound, a substitution, a pipe, or a one-off
literal — is unique, so the user must approve it by hand *every
single time*. When you catch yourself about to run something that
won't generalize, stop and reshape it (split it, hoist the dynamic
part into a prior step or a tool, pass values literally) before
running it.

This applies to shell you **author**, not just shell you type
ad-hoc: snippets in skills, scripts, Makefile targets, and docs get
executed verbatim, so the same patterns below re-prompt forever when
baked into them. Write committed shell to the same standard — prefer
a sequence of bare commands that each reduce to a glob (or "run X,
read its output, then run Y with the value inline") over a clever
one-liner.

It applies to work you hand to a **sub-agent**, too. The whole
objective is **the fewest permission prompts possible** across the
session, and a spawned agent's Bash calls surface to you for approval
exactly like your own — but the agent doesn't inherit this file, so it
will reach for the forbidden compounds unless told not to. Brief every
agent you spawn on these rules (see "Briefing sub-agents" below) so its
calls reduce to allow-rules too. A session that follows the rules and
briefs its agents on them prompts only for a genuinely novel command —
which `firm-perms` then memorializes so it never prompts again.

**The dedicated Grep / Glob tools aren't always present.** Native macOS
Claude Code builds (>= 2.1.117) drop them from the default tool palette
in favor of embedded Bash search
(<https://github.com/anthropics/claude-code/issues/52004>), and we do
**not** force them back on via `--tools` (that flag is replace-not-add,
so it would mean enumerating the whole built-in set in every launcher —
too brittle). So the "use the Grep tool" guidance below is conditional:
use Grep / Glob **when they exist**, but where they don't, fall back to
a **bare, single** `grep` / `find` Bash command — never a piped
compound. Bare `grep` / `find` reduce to the retained `Bash(grep:*)` /
`Bash(find:*)` allow-rules and prompt once; it's the `grep … | head` /
`find … | xargs` **pipes** that can't generalize and re-prompt forever.
The `Bash(grep:*)`, `Bash(find:*)`, `Bash(head:*)`, and `Bash(tail:*)`
allow-rules are kept for exactly this fallback.

Concrete rules:

- Prefer the dedicated tools — Read, Grep, Glob — over `cat`, `grep`,
  `find`, `ls` in Bash. They don't prompt for in-workspace paths. This
  includes *slicing* a file: use Read with `offset`/`limit` instead of
  `sed -n 'X,Yp'`, `awk 'NR>=X'`, `head`, or `tail`. Never shell out to
  `python3` / `node` / `jq` to read or edit JSON/config (including
  `.claude/settings.local.json`) — use Read + Edit/Write. Each such
  one-liner is unique and re-prompts forever. To find **over-length
  lines** for the MD013 80-col rule, don't reach for
  `awk 'length>80'` / `sed` either — run the markdownlint hook
  (`pre-commit run markdownlint-fix … --files <path>`, with
  `--config cfg/pre-commit-lint.yml`); it reports every MD013
  violation with its line number and reduces to the existing
  `Bash(pre-commit run:*)` rule.
- Searching file *contents* — prefer the **Grep tool**; where it's
  absent (the Grep / Glob caveat above) a **bare, single** `grep` is
  the fallback, but **never** `git grep`. This is the same rule the
  sub-agent brief carries
  (see "Briefing sub-agents" below); it holds for the main agent too,
  so the convention is one and the same — the brief just restates it
  because a sub-agent doesn't inherit this file. Grep takes a real
  regex (alternation is `a|b|c`, not a shell-quoted `a\|b\|c`), reads
  any path you point it at, and prompts zero times. `git grep` looks
  blessed — it's a git subcommand, so it seems covered by the
  `git -C <path> <sub>` cross-checkout rule below — but it isn't: a
  clean single pattern only re-prompts until firmed, and a quoted `\|`
  alternation trips the per-subcommand `|` guard and can't be firmed
  at all. Reserve `git -C <path>` for **metadata** subcommands
  (`log` / `show` / `diff` / `status` / `ls-files`), never `grep`.
- One command per Bash call. Avoid `&&`, `;`, and pipes when separate
  calls work; a chained command can't be generalized into a glob and
  always re-prompts.
- No command substitution. `$(...)` and backticks block globbing —
  compute the value in a prior step (or a tool) and pass it literally.
- Avoid redirects (`>`, `<`, here-strings). Use the Write tool to
  create files rather than `echo … > file`.
- Pass large or special-character arguments through a **file**, not
  inline on the command line. A multi-paragraph commit message — its
  backticks, braces, and quotes trip the "brace with quote character
  (expansion obfuscation)" guard and force manual approval *every
  time*, even though the command prefix is allow-listed. Write the
  content to a throwaway file with the Write tool (e.g. under `/tmp`)
  and hand the command its path via the matching `--*-file` flag —
  `git commit -F /tmp/<f>.txt` — so only a stable, globbable path rides
  the command line and the call reduces to a `prefix:*` rule. (PR
  titles and bodies are **no longer** a shell concern: they go through
  the GitHub MCP as structured tool arguments — see "GitHub via MCP" —
  so there is no `--body-file` workaround to manage.)
- Keep a stable command + subcommand prefix (`pnpm lint …`,
  `cargo test …`, `git log …`) and put only the variable parts in the
  arguments, so the call matches a `:*` allow-glob.
- Stay in your worktree. The shell already starts at the worktree
  root — never `cd` into it (`cd <worktree> && …`). That compound
  forces manual approval every time (path-resolution bypass) and
  can't reduce to a glob. Run commands bare from the cwd.
- No status banners or exit-code plumbing. Don't append
  `; echo "=== exit $? ==="`, pipe through `tail` / `grep`, redirect
  `2>&1`, or read `${PIPESTATUS[0]}`. Run the bare command
  (`make lint`, `cargo fmt -p dropset`) — its full output and exit
  status already come back. Pipes and `$(…)` / `${…}` expansion
  force re-approval on every call.
- Inspect the base repo by path, not by `cd`. To read another branch
  or the base checkout from a worktree, run
  `git -C <base-repo-path> <subcommand>` with a *literal*, stable path
  (no `$(…)`). Keep the subcommand immediately after the path so the
  call reduces to a `Bash(git -C <base-repo-path> <sub>:*)` rule —
  then pre-approve the read-only subcommands (`log`, `show`, `diff`,
  `status`, `rev-parse`) once in your local `settings.local.json` so
  they never prompt again.
- Operate on a *sibling worktree* by its real path, but approve it
  with a worktree **glob**. A command like
  `git -C <base-repo-path>/.claude/worktrees/<tag> status --short`
  has to name the real worktree to run, but the allow-rule it matches
  against should be the generalized
  `Bash(git -C <base-repo-path>/.claude/worktrees/* status:*)` — the
  mid-path `*` covers every sibling tag and the `:*` covers the args,
  so one rule firms the whole family. Don't approve the per-tag,
  per-arg variant; it only ever matches that one call.
- When per-worktree or per-arg approvals have already piled up in
  `settings.local.json`, run the `firm-perms` skill. It collapses the
  one-off entries into globs (per the rules above), dedupes them, and
  writes the firmed allowlist to **both** this worktree and the base
  repo so future worktrees inherit it — proposing the changes for
  your approval before it writes. That's the full sweep; a bare
  `/firm-perms` run right after a one-time approval instead takes the
  **fast path** — it firms just that single just-approved command into
  both files immediately, with no propose-then-confirm gate.

### Patterns that always re-prompt — never author these

The rules above each rule out a class of command. These are the
specific forms that have actually slipped through and forced a manual
approval *every time*, because none can reduce to an allow-rule —
don't write them, in ad-hoc shell or in committed skills/scripts:

- **Heredocs** (`cat > file << 'EOF' … EOF`, `python3 << 'EOF' … EOF`).
  A heredoc is a redirect plus inline content; when the body contains
  braces it also trips the "brace with quote character (expansion
  obfuscation)" guard, which forces approval regardless of the
  allowlist. To **create a file**, use the Write tool. To **read or
  parse** one (including JSON/IDL), use Read / Grep — never `python3` /
  `node` / `jq`.
- **Ad-hoc compile-and-run scratch** — e.g. a
  `cat > /tmp/x.rs << EOF` heredoc piped into
  `rustc … && /tmp/x`. To check a language or layout question, Write a
  throwaway file and drive it with the normal target (`cargo test`, a
  `#[test]`), or reason it out — don't synthesize a one-off program
  through a heredoc-and-`&&` chain.
- **`cd <path> && <cmd>`** (e.g. `cd <repo> && git -C <worktree> …`).
  The `cd &&` compound re-prompts as a path-resolution bypass. Run
  bare from the cwd, or address another checkout with `git -C <path>`
  alone — no `cd`, no `&&`.

If a one-off like these still gets approved during a session, do
**not** allow-list it (a `*` can't generalize a compound): the
`firm-perms` skill flags it and points back here so the source stops
emitting it.

### The compound-shell guard hook

These rules are enforced **mechanically**, not just by convention. A
`PreToolUse` Bash hook (`.claude/hooks/no_compound_bash.py`, wired in
the committed `.claude/settings.json` so every worktree inherits it)
inspects each Bash command before it runs and **blocks** any that
contains an unquoted shell compound / redirect operator — a pipe, `>`,
`<`, `;`, `&&`, `||`, `&`, a backtick, or `$(` — telling the model to
split the call and use the Write / Read / Grep tools instead. The scan
is **quote-aware**: an operator inside a single- or double-quoted
string (a commit message's `;`, a regex's `|`) is legitimate text and
passes; command substitution (`` ` `` and `$(`) is caught even inside
double quotes, mirroring real shell. The guard fails *open* — any
payload it can't parse is allowed — so it never wedges a session.

**Escape hatch.** A genuinely-unavoidable compound (rare) is let
through by adding the literal marker `#compound-ok` anywhere in the
command. It's deliberately visible in the transcript so the bypass is
auditable; reach for it only when the work truly can't be split.

## Briefing sub-agents

A sub-agent you spawn (via the `Agent` tool) does **not** inherit this
`CLAUDE.md`, so left to itself it reaches for `find / …`,
`sed -n '…p' … | grep`, `cat`, and other compounds that can't reduce
to an allow-rule and re-prompt on **every** run — the exact churn the
shell rules above exist to avoid. So whenever a skill spawns a
sub-agent, it must carry the conventions into the agent itself. This
is the single canonical brief; skills reference it by name ("prepend
the sub-agent brief from `CLAUDE.md`") rather than each pasting their
own copy, so the wording stays in one place.

**Prepend this standing brief to *every* `Agent` prompt:**

> - You are a **read-only** agent. The material you need to reason
>   over — a diff, a commit log, a set of issues — is included in this
>   prompt; start there, and you often won't need a shell at all.
> - To inspect files, prefer the **Read / Grep / Glob** tools over
>   `cat` / `head` / `tail` / `sed` / `awk` / `find` / `grep` in Bash —
>   they don't prompt for in-workspace paths, and they search other
>   directories too.
> - **Searching file *contents* — prefer the Grep tool; where it's
>   absent (native macOS builds), a bare single `grep` is the fallback,
>   but never a pipe and never `git grep`.** When Grep is present it
>   holds in-workspace *and* cross-path: Grep reads
>   any directory you point it at, takes a real regex (so an
>   alternation is `a|b|c`, not a shell-quoted `a\|b\|c`), and prompts
>   **zero** times. Do **not** reach for `git -C <path> grep …` to
>   search contents — `grep` is a git subcommand, so it looks blessed
>   by the cross-checkout rule below, but it isn't: a clean single
>   pattern only re-prompts until firmed, and a quoted `\|` alternation
>   trips the harness's per-subcommand `|` guard and **can't be firmed
>   at all**. Grep sidesteps both.
> - **Exploring another repo or path is fine** — reach outside this
>   worktree when the task needs it; approving a one-off read of a
>   different repo is expected, not something to avoid. Just keep each
>   access **globbable** so it approves once and won't re-prompt: use
>   Read / Grep / Glob for files and their contents, or read another
>   checkout's **metadata** with `git -C <path> <subcommand>` — `log` /
>   `show` / `diff` / `status` / `ls-files`, *not* `grep` (the subcommand
>   immediately after the path, no `cd`). What to avoid is the
>   **un-globbable** shape — a `find / …` sweep, or several `git -C …`
>   calls strung together with `&&` / `|` / `;` into one compound that
>   can't reduce to a rule.
> - **One bare command per Bash call** — no pipes, `&&`, `;`, command
>   substitution `$(…)`, redirects, or heredocs. Each call must reduce
>   to a `prefix:*` allow-rule.

**Pass the material inline.** Whatever the agent must reason over —
the diff, the commit log, the issue set — goes **in the prompt**, so
no agent re-fetches it by shelling out. (For content too large or
special-character-laden to sit inline cleanly, use the file-handoff
pattern from the shell rules — write it out and pass the path.)

**A skill may narrow this scope, never loosen it.** The brief is the
floor: shell discipline plus the freedom to explore. A spawning skill
is free to add a tighter *subject* scope on top — a diff reviewer, for
instance, should be told to "review only from the diff and commit log
below; dependency and toolchain sources are out of scope — flag it in
your findings instead of scanning." That narrows *where the agent
looks*; the shell rules stay exactly as written. An audit agent, by
contrast, is *meant* to range over the whole codebase, so it gets the
brief without any narrowing.

A sub-agent approval that still re-prompts despite this brief means
the brief **leaked** — the agent emitted shell the brief forbids.
That's a prompt to tighten, not a rule to allow-list; `firm-perms`
sets such approvals aside and names the emitting agent so its prompt
gets fixed at the source.

## Audit registry

`audit-loop` reads its coverage map from here — the **subsystems**
to range over, the **interfaces** between them where contract drift
hides, and the **skip-globs** of generated / vendored paths never
worth auditing (`audit-scope` reads just the subsystem `kind`). These
lists live in `CLAUDE.md` (committed, shared) rather than in
per-worktree state, and `review-pr`
refreshes them on every run: when a diff introduces a new subsystem, a
new seam between subsystems, or a new generated-file family, it
appends the entry here so the registry stays current as the system
grows. Keep all three blocks lint-clean (MD013 80-col, mdformat).

**Subsystems** — `name (kind, risk): roots`. `kind` selects the
per-platform audit checklist; `risk` weights selection.

```txt
program (solana-program, high): programs/dropset/src/**
sdk-math (rust-lib, high): sdk/math-core/src/**, sdk/interface/src/**
sdk-clients (gen-client, med): sdk/rs/src/**, sdk/ts/src/**, sdk/codama/**
frontend (web-app, med): frontend/**
tui (rust-lib, low): tui/**
docs (specs, med): docs/**
ci-infra (ci, low): .github/**, cfg/**, infra/**, Makefile, Anchor.toml
tools (rust-tool, low): tools/**
maker-bot (rust-tool, low): bots/maker-bot/**
```

**Inter-subsystem interfaces** — the seams where contract drift
hides; `A <-> B: the contract that crosses the boundary`.

```txt
program <-> sdk-clients: the Anchor IDL (sdk/idl/dropset.json) is
  generated from the program; the Rust/TS clients are generated from
  the IDL — accounts, instructions, and on-chain events (FillEvent)
  must stay in lockstep.
program <-> sdk-math: the program depends on the shared math
  (sdk/math-core, sdk/interface) and must compute identically to it;
  the conformance vectors (sdk/conformance) pin price/share/quoting
  parity across the boundary.
program <-> frontend: the on-chain account/instruction contract in
  docs/interface.md, which the frontend builds transactions against
  through the generated clients.
sdk-math <-> frontend: no live wiring today. The frontend imports no
  sdk-math (a grep of frontend/ for @dropset/sdk is empty) and consumes
  no WASM; it quotes via DFlow's API (frontend/lib/dflow/) and shows a
  display-only float PnL re-implementation (frontend/lib/data/pnl.ts) of
  the share kernels that no conformance vector pins, over static mock
  data. The drift to watch is the day pnl.ts is wired to live on-chain
  reserves: its float math can then diverge from the integer engine.
tui <-> sdk-math: the resting-book matcher surface (sdk/interface
  matching `resting_levels` / `BookLevel`) the TUI's order-book pane
  reconstructs depth from — the SDK normalizes a bid's quote leg to base
  at the level price, and the pane (tui/src/book.rs) de-scales by mint
  decimals, so the two must agree on the base-atom denomination.
maker-bot <-> program: the bot quotes and submits against the on-chain
  account/instruction contract (docs/interface.md) through the generated
  SDK clients (sdk/rs) — instruction args and accounts must match.
```

**Skip-globs** — generated / vendored / binary paths the file audit
never picks. One glob per line.

```txt
target/**
**/node_modules/**
Cargo.lock
**/pnpm-lock.yaml
**/package-lock.json
**/yarn.lock
**/*.gen.*
**/generated/**
**/idl/**
sdk/conformance/**
target/types/**
frontend/lib/data/*.json
frontend/public/**
**/*.png
**/*.svg
**/*.min.*
.audits/**
```
