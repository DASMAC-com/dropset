# Linear automation

Skills that **file** Linear issues (`linear-task`, `audit`,
`audit-scope`, `trim-context`, `housekeeping`) resolve the filing
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

`session-metrics`, `trim-context`, and `housekeeping` share
`LINEAR_SESSION_METRICS_DOC_ID` — the id of the "Session Metrics"
inbox document — each resolving it with its own bare `printenv`, on
the same rule. `session-metrics` is the **producer**: it appends one
dated entry per session (the measured token sinks plus tailored trim
recommendations). `trim-context` is the **consumer**: it mines the
unprocessed entries for the trim levers that recur across sessions and
files them as propose-only skill-improvement Backlog tasks (never
editing a skill itself), then writes each consumed entry's disposition
back. `housekeeping` drives `trim-context` as its Session Metrics step,
and the skill also runs standalone. Each no-ops with a clear message
when the variable is unset. The `session-metrics` skill
drives its tool via `make session-metrics`, which reduces to a
`Bash(make session-metrics:*)` allow-rule. This tool needs **no**
`LINEAR_API_KEY` — it only parses the local transcript and makes no
network call; the skill does the one Linear write (the doc append)
over the MCP.

The **stage-backlog Python tool** (the deterministic core of the
`stage-backlog` skill — see "Structured filing fields" below) is a
single, dependency-free `python3` script at
`tools/stage-backlog/stage_backlog.py`, run directly (the
`Bash(python3 tools/stage-backlog/stage_backlog.py:*)` allow-rule —
there is no `make` target). Blocking is **edge-driven**: it reads the
open Backlog plus the state of every issue its relations reach, and
builds the tree from declared `blockedBy` / `blocks` edges alone. It
makes **two kinds** of Linear write: it materializes each undeclared
`**Touches**:` collision between two Backlog issues into a real
`blocks` relation (lower number blocks higher — see "Structured filing
fields"), then rewrites the Task Staging document. It still has **no**
`merge` subcommand and never folds or closes an issue; two issues that
belong in one PR render as nested serial chips. A blocker is honoured
until it reaches a terminal state (`completed` / `canceled`), so a
Backlog issue gated by a live In-Progress / In-Review issue keeps that
blocker as a tag, and the blocker ranks in the transitive
`# Most blocking` tally. It uses the standard library only
(`urllib` + `json`) for its GraphQL calls, so it adds no dependency to
the Rust build and inherits the repo's `ruff` hooks; its unit tests run
under `make tools-tests`. It reads `LINEAR_PROJECT_ID` plus its own
`LINEAR_API_KEY` (a personal Linear API key, because a script can't
ride the OAuth-based `claude.ai` Linear MCP) for every run; for a real
write it also reads `LINEAR_TASK_STAGING_DOC_ID` (the document it
rewrites), while `--dry-run` prints the tree to stdout, prints the
overlap edges it *would* file, and doesn't require it. It resolves all
of these via `os.environ`, never a hard-coded id, and the key is never
committed.

## Structured filing fields

Every filed issue carries machine-readable fields the automation reads
back, on top of the human prose. Keep the field **names** stable — the
filing skills emit them and `stage-backlog` parses them:

- `**Fingerprint**: <basename>:<slug>` — the dedup key `audit`
  matches on so a finding is never refiled. Mandatory on audit
  findings; one line per finding (a merged issue carries several).
- `**Touches**: <glob>[, <glob>…]` — the path globs the fix will
  edit, comma-separated. Declare the **directory** when the work
  spans a dir (`tui/`), the **file** when it's one file
  (`programs/dropset/src/swap.rs`); list every glob for a multi-file
  finding. The `stage-backlog` tool reads this to detect file
  collisions **deterministically** — a directory glob collides with
  any path under it, and two issues that collide can't run in
  parallel. When such a pair has no declared edge either direction,
  the tool **materializes** the constraint into a real `blocks`
  relation (the lower-numbered issue blocks the higher), so the higher
  nests under the lower and Linear carries the edge durably. Moving
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

## The `Claude:` meta-work prefix

**Meta-work** is agent-infra change — work whose touched paths sit
**entirely** under `.claude/**`, `CLAUDE.md`, `docs/conventions/**`, or
`tools/**`. Anything that also touches product / on-chain / SDK /
frontend code is **not** meta. Every meta-work Linear issue title
carries a leading **`Claude:`** token (capital C, colon, space) —
e.g. `Claude: Add a /merge-tasks skill` — so all agent-infra work
batches together and can be filtered, staged, and reviewed apart from
product code on the board.

- **Filing skills emit it.** `linear-task`, `audit`, `audit-scope`,
  and `housekeeping` prepend `Claude:` to a title when the issue's
  `**Touches**:` globs are all on the meta surface above. `/merge-tasks`
  applies it when every issue it consolidates is meta.
- **`stage-backlog` buckets by it.** Issues whose title starts with
  `Claude:` group under a single `# Claude` heading (the deterministic
  batch signal — the bucket keys on the **prefix**, not on file globs).
  The tool also runs a **prefix↔touched-paths consistency check** and
  warns on drift: a `Claude:`-prefixed issue whose `**Touches**:` reach
  outside the meta surface, or a meta-only-touches issue with no
  prefix. `housekeeping` surfaces those warnings for a human to
  reconcile; the tool never retitles an issue itself.
- **It is a Linear-title signal only — never a PR title.** The prefix
  lives on the **issue** title for board recognition and batching. PR
  titles keep the standard `type(ENG-###): Subject` semantic-pr format
  (see "Keep Linear tags out of PR bodies and comments" below for the
  title-scope rule); the `Claude:` token is **not** added to a PR
  title, where the conventional type and `ENG-###` scope already apply.

## Keep Linear tags out of PR bodies and comments

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

## Blocking relations

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
auditors (`audit-scope`, `audit`) work under a tighter rule:
they may assert a relation **only on concrete evidence** that one
finding's fix cannot land until another issue resolves (e.g. a nit
that depends on an `arch:` proposal filed the same run), never a
speculative "these feel related" edge. Mere coupling — work that
belongs in *one PR* — is handled by combining into a single issue,
not a relation. When the blocker is filed in the same run, file it
first so its `ENG-###` exists, then reference it.
