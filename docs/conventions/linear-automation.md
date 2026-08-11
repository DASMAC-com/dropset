# Linear automation

Skills that **file** Linear issues (`linear-task`, `audit`,
`audit-scope`, `trim-context`, `housekeeping`) resolve the filing
destination — team, project, assignee — from **environment
variables**, never hard-coded UUIDs. (Skills that only **update**
an existing issue by id — `init-pr`, `review-pr` — need no
destination; `sync-blockers` only files `related` relations between
Backlog issues, reading `LINEAR_PROJECT_ID` as a query filter — see its
own paragraph below.) Set them once in your
shell profile (`~/.zshrc`):

```sh
export LINEAR_TEAM_ID=…
export LINEAR_PROJECT_ID=…
export LINEAR_ASSIGNEE_ID=…
# Used by session-metrics (producer) and housekeeping (consumer) —
# the "Session Metrics" inbox document one appends to and the other
# mines into propose-only skill-improvement tasks:
export LINEAR_SESSION_METRICS_DOC_ID=…
# Used only by the sync-blockers Python tool (the deterministic
# core of the sync-blockers skill) — a personal Linear API key. A
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

`session-metrics`, `trim-context`, and `housekeeping` share
`LINEAR_SESSION_METRICS_DOC_ID` — the id of the "Session Metrics"
inbox document — each resolving it with its own bare `printenv`, on
the same rule. `session-metrics` is the **producer**: it appends one
dated entry per session (the measured token sinks plus tailored trim
recommendations). `trim-context` is the **consumer**: it mines the
unprocessed entries for the trim levers that recur across sessions and
files them as a **single aggregated** propose-only skill-improvement
Backlog task — one bullet per lever, each carrying its own
`**Fingerprint**:` line under a combined `**Touches**:` — so a mining
pass yields one issue (one PR) rather than a batch to consolidate
later, never editing a skill itself, then writes each consumed entry's
disposition back. `housekeeping` drives `trim-context` as its Session
Metrics step,
and the skill also runs standalone. Each no-ops with a clear message
when the variable is unset. The `session-metrics` skill
drives its tool via `make session-metrics`, which reduces to a
`Bash(make session-metrics:*)` allow-rule. This tool needs **no**
`LINEAR_API_KEY` — it only parses the local transcript and makes no
network call; the skill does the one Linear write (the doc append)
over the MCP.

The **sync-blockers Python tool** (the deterministic core of the
`sync-blockers` skill — see "Structured filing fields" below) is a
single, dependency-free `python3` script at
`.claude/tools/sync_blockers.py`, run directly (the
`Bash(python3 .claude/tools/sync_blockers.py:*)` allow-rule —
there is no `make` target). Its one job is **relation maintenance**: it
reads the open Backlog's `**Touches**:` globs and existing relations,
and files a real `related` relation for each `**Touches**:` collision
that has no relation yet, naming the paths the pair collides on. It
files **no blocking edge** — see "Blocking relations" below. That
relation write is its **only** Linear write — it renders no document,
ranks nothing, and never folds or closes an issue (consolidation is
`merge-tasks`' job). It runs in **four modes**: `--for ENG-###`
compares just the named, just-filed issue against the backlog (the
bounded file-time path the filing skills call after `save_issue`); a
bare invocation is the full pairwise sweep for occasional
reconciliation, reporting **collision clusters** (the input to
`housekeeping`'s merge-group proposal), the surviving human-declared
**semantic blocks**, and the two scheduling **smells**;
and `--report-todo-blocks` prints those smells alone as JSON. (A
`--demote` migration mode also existed; it ran once, is spent, and was
removed — see "Blocking relations" below.) It uses the
standard library only (`urllib` + `json`) for its GraphQL calls, so it
adds no dependency to the Rust build and inherits the repo's `ruff`
hooks; its unit tests run under `make tools-tests`. It reads
`LINEAR_PROJECT_ID` plus its own `LINEAR_API_KEY` (a personal Linear
API key, because a script can't ride the OAuth-based `claude.ai` Linear
MCP); `--dry-run` prints the links it *would* file and writes nothing.
It resolves all of these via `os.environ`, never a hard-coded id, and
the key is never committed.

## Structured filing fields

Every filed issue carries machine-readable fields the automation reads
back, on top of the human prose. Keep the field **names** stable — the
filing skills emit them and `sync-blockers` parses them:

- `**Fingerprint**: <basename>:<slug>` — the dedup key `audit`
  matches on so a finding is never refiled. Mandatory on audit
  findings; one line per finding (a merged issue carries several).
- `**Touches**: <glob>[, <glob>…]` — the path globs the fix will
  edit, comma-separated. Declare the **directory** when the work
  spans a dir (`tui/`), the **file** when it's one file
  (`programs/dropset/src/swap.rs`); list every glob for a multi-file
  finding. The `sync-blockers` tool reads this to detect file
  collisions **deterministically** — a directory glob collides with
  any path under it, and two issues that collide are coupled. Such a
  pair is **related-linked**, and the tool reports the paths they
  collide on. This runs at **filing time**: each filing skill calls
  `sync_blockers.py --for <new-id>` right after `save_issue`, so a new
  issue's collisions are recorded the moment it lands. A collision is
  explicitly **not** a blocking edge — see "Blocking relations". An
  issue that predates the `**Touches**:` convention has no globs to
  check; backfill one and re-run the sweep.

### Collision clusters, not serial chains

File overlap is reported as a **cluster** — the issues that collide on
one shared path — rather than as an ordering. A cluster is the candidate
set for "these would land as one PR", which is what `housekeeping`'s
merge-group proposal step consumes. Grouping is **per path**, not by
connected component: coupling chains through shared files, so the
transitive reading collapsed 25 of 27 open issues into one cluster,
which proposes nothing. Clusters therefore overlap — an issue appears
under every path it touches.

The previous design turned each collision into a `blocks` edge instead,
and paid for it: because `**Touches**` globs are coarse (crate-level),
the orientation was arbitrary (lower number blocks higher), and block
semantics are binary, the board grew giant serial chains — a day-1
mainnet param-channel issue sat behind **eight** overlap blockers, and
a docs-only pair was block-linked because both touched
`docs/market-making-mvp.md` in unrelated sections. A cluster carries the
same information without asserting an order nobody decided.

### Fold coupled findings into one issue

A rotation should yield the **fewest coherent PRs**, not one issue per
finding. When a run turns up multiple findings, fold together every set
that would sensibly land as **one PR** and file each set as a single
issue — *before* the dedup check and the `save_issue`.

The bar is **same-PR coherence**, not same-file: fold findings that
share a subsystem, crate, or language-domain and that a reviewer would
naturally review and land together. So every doc-/comment-freshness fix
across the run → one issue; every low-risk refactor within one crate →
one issue. This is wider than "same file or symbol" and spans **units
within a rotation**, not only findings within a single unit.

A folded issue keeps **every** finding's own `**Fingerprint**:` line
(one per line, so per-finding dedup still matches) and a **union** of
their `**Touches**:` globs.

**The coherence floor — do not fold across deploy units.** Never merge
findings a single PR can't sensibly review or land as a whole: different
apps, languages, or deploy units stay apart. A TUI Rust rendering fix, a
frontend TS hook fix, and an on-chain program refactor are **three** PRs,
not one — even though all three are "cleanup from the same rotation".
Fold *within* a coherent PR boundary; never across one. "Aggressive"
means minimize PR count up to that floor, not build an incoherent
mega-PR past it.

A worktree branch and its Linear issue **share one `ENG-###`
number**: branch `eng-499` ↔ issue `ENG-499`. Skills resolve the
issue from the branch (or the PR title scope) on that basis —
`init-pr` moves it to In Progress at bootstrap, `review-pr` ticks the
delivered checklist items and moves it to In Review at the merge-queue
handoff — once the PR is ready, CI is green, and the review summary has
been printed for the human.

## The `Claude:` meta-work prefix

**Meta-work** is agent-infra change — work whose touched paths sit
**entirely** under `.claude/**`, `CLAUDE.md`, or `docs/conventions/**`.
Anything that also touches product / on-chain / SDK / frontend code is
**not** meta — including the shared build scripts under `brand-assets/`,
which are product-adjacent, not agent-infra. Every meta-work Linear issue title
carries a leading **`Claude:`** token (capital C, colon, space) —
e.g. `Claude: Add a /merge-tasks skill` — so all agent-infra work
batches together and can be filtered, staged, and reviewed apart from
product code on the board.

- **Filing skills emit it.** `linear-task`, `audit`, `audit-scope`,
  and `housekeeping` prepend `Claude:` to a title when the issue's
  `**Touches**:` globs are all on the meta surface above. `/merge-tasks`
  applies it when every issue it consolidates is meta.
- **It batches meta-work on the board.** The prefix is the signal a
  human filters and groups by in Linear to see all agent-infra work at
  once, apart from product code. It is applied at **filing time** — the
  filing skills add it exactly when the issue's `**Touches**:` globs are
  all on the meta surface, so the prefix and the touched paths stay
  consistent by construction. No tool re-derives or re-checks the
  bucket; there is no rendered `# Claude` heading to keep in sync.
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

## Partial edits — the `patch` argument

`save_issue` and `save_document` both accept a **`patch`** array as an
alternative to the full `description` / `content` field, so a caller can
add to or amend part of a body without re-sending it. Passing
`description` / `content` **does** replace the body wholesale — that
much is true — but it is not the only option. The ops are `append`,
`prepend`, `insert_before`, `insert_after`, `replace` and
`replace_range`; they apply in order and **atomically** (one failing op
aborts the whole save), up to 50 per call. `patch` is **update-only**
and mutually exclusive with the full-content field — pass one or the
other, never both.

Each op carries `op` plus its own fields: `append` / `prepend` take
just `text`; `insert_before` / `insert_after` take `anchor` + `text`;
`replace` takes `old_string` + `new_string` (and an optional
`replace_all`); `replace_range` takes `from` + `to` + `new_string`.
Every one of those strings takes **literal newlines**, never the
escape sequence `\n`.

**Insertions include no separator of their own.** `append`, `prepend`,
`insert_before` and `insert_after` splice the text in *immediately*
adjacent to the anchor (or the end / start of the body), so the `text`
has to carry whatever blank line or indent the surrounding markdown
needs. An anchor that sits mid-line — the tail of a list item, say —
will otherwise leave the inserted text glued onto that line instead of
starting its own.

What `patch` buys:

- **The write payload scales with the edit, not the body.** A wholesale
  save of a 60KB document spends 60KB of input tokens to add one entry;
  the equivalent `append` spends the length of that entry.
- **No transcription risk.** The server applies the edit, so a
  hand-rebuilt body can't garble the parts it wasn't meant to touch.
- **A pure `append` needs no prior read.** There is nothing to anchor
  against, so the fetch a full-body rebuild depends on can be skipped
  outright.
- **Failure is cheap and safe.** A non-matching anchor aborts the whole
  save and returns a short error, so a stale anchor costs one small
  error rather than a mangled body.

What `patch` does **not** buy is **a smaller response echo**. Both calls
echo the saved object back regardless of how the write was expressed:
`save_issue` returns the **full** `description` whether it was sent
wholesale, sent as a `patch`, or not sent at all — a state-only
transition echoes the whole body too — and `save_document` returns a
**truncated** `content` in every one of those cases. The echo is a fixed
cost per call, so the lever on it is **fewer calls**, not `patch`.

### The echo budget is per issue, per **session** — not per skill

"Fewer calls" is easy to satisfy inside one skill and still lose, because
a single issue is touched by **two** skills across a worktree session:
`init-pr` reads the body and moves it to In Progress, `review-pr` moves
it to In Review at the handoff. Each of those echoes the entire body.

One measured session paid **24.5k across three calls on one issue** —
`get_issue` 8.2k, `save_issue` 8.2k, `save_issue` 8.1k, the top three
single results of the run — for three state transitions. Every skill
involved was individually compliant. Nothing budgeted the handoff.

So the budget is **per issue, per session**, with three consequences that
each skill must honor:

- **`init-pr` has already read the body**, so `review-pr` must **not**
  re-`get_issue` it. Work from what the session already holds.
- **A write that changes nothing is skipped.** If the state is already
  correct, don't re-assert it just because a step says to set it.
- **Fold a state transition into a write you are already making** rather
  than issuing it on its own.

Two concrete call sites the rule above catches, both measured:

- **Don't re-fetch a body a write just returned.** A state-only
  `save_issue` returns the complete issue, so a `get_issue` right after
  it buys the same payload twice — two ≈1.1k echoes for one body. Read
  the description out of the write's response.
- **Batch checklist ticks into one call, at the end.** One run made four
  `save_issue` calls (≈1.4k, its largest MCP cost), each re-echoing the
  whole body for a one-field write; two were separate checkbox ticks that
  belonged in a single `patch` with two ops. Accumulate the ticks and
  write them once. The **state** transitions are load-bearing and stay
  where they are — it is the per-item ticking that batches.

**The consolidated-spec case is the expensive one.** The larger the
survivor body a `/merge-tasks` consolidation produced, the more *every*
later transition on that issue costs — so a heavily-folded issue is
exactly where re-reading its body is least affordable.

### Anchors must match the *stored* text

Every anchor (`old_string`, `anchor`, `from` / `to`) must match the
current content **exactly once** — `replace` also takes `replace_all`
when a unique match isn't wanted. The trap is that stored text can
differ from what was written: **Linear rewrites an `ENG-###` in content
into an issue-mention node**, so text saved as `ENG-123` reads back as
an `<issue id="…" href="…">ENG-123</issue>` element and an anchor
carrying a Linear tag **will not match**. Anchor on tag-free text — a
date, a session id, a heading — or use `append`, which needs no anchor
at all. (This is **not** the "Keep Linear tags out of PR bodies and
comments" rule above — that one governs GitHub surfaces, and a tag in a
Linear body is perfectly fine. The corollary here is narrower: prefer
tag-free text for anything you expect to *anchor* on later.)

Related: **`updatedAt` in a `save_document` response is not a reliable
concurrency signal** — it has been observed unchanged across
successive successful writes. Don't gate a write on comparing it against
an earlier fetch. `patch`'s atomicity and exactly-once anchors are the
real protection: they fail loudly instead of clobbering a concurrent
edit.

## Blocking relations

**No automated writer files a blocking edge — ever.** Not
`sync_blockers.py`, not a filing skill (`linear-task`, `audit`,
`audit-scope`, `trim-context`, `housekeeping`, `merge-tasks`), not an
autonomous audit rotation. This holds for edges an agent believes are
genuinely semantic, not just for file-overlap ones.

The reason is that the board's **available-vs-blocked view is a
scheduling instrument the human drives**: a hand-built blocking queue
expressing intended order of attack, from which the *available* set is
then sorted by priority. An auto-filed edge silently makes that view
untrustworthy, and a wrongly-blocked issue drops out of the available
set altogether — so a spurious edge is strictly worse than a missing
one. A missing edge costs at most a rebase; a spurious one costs
scheduling.

**Propose, don't file.** Where a filer believes a real dependency
exists, it proposes at filing time via `AskUserQuestion` — naming the
candidate blocker and the **concrete evidence** ("this consumes the
output of X — should it be blocked by it?") — and writes the edge only
on an explicit yes. The default, **including in any non-interactive or
autonomous run where nobody can answer, is no edge**; the suspected
dependency is recorded as prose in the issue body instead, so the
reasoning is never lost.

**Human-placed edges are authoritative.** The automation never
rewrites, redirects, or removes one — with **no** exception. There was
one: `sync-blockers` carried a `--demote` mode, a one-time migration that
converted pre-existing auto-filed `blocks` edges to `related` under
explicit confirmation. It ran on 2026-08-10 and is **spent**, so it has
been removed. Every candidate it could still find is a false positive:
the six legitimate hand-placed blocking edges collide on files, and the
tool cannot distinguish them from artifacts, so a second run under
`--apply` would delete the intended blocking graph in one command. It was
dead code and a live foot-gun, so it is gone rather than guarded.
Blocking-edge changes now happen only where they always should have: in
a planning session, human-directed, one at a time.

The mechanics, for when a human does ask for an edge: `save_issue`
takes `blockedBy` (the `ENG-###`s that must land first) and `blocks`
(the `ENG-###`s this one gates), both by identifier; they are
**append-only** — they add edges and never clear existing ones, so use
`removeBlockedBy` / `removeBlocks` to drop one. When the blocker is
filed in the same run, file it first so its `ENG-###` exists, then
reference it.

Two things that are **not** blocking edges:

- **File overlap.** A `**Touches**:` collision is related-linked and
  reported as a cluster (see "Collision clusters, not serial chains").
- **Coupling that belongs in one PR.** That is handled by combining
  into a single issue (see "Fold coupled findings into one issue"),
  not a relation.

`sync-blockers` reads whatever edges exist to avoid double-linking a
pair — a declared edge suppresses the related link it would otherwise
file — and reports them back under the sweep's *semantic blocks*
section, which, with nothing automated writing one, *is* the intended
scheduling order.
