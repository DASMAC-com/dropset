<!-- cspell:word basenames -->

# Linear automation

Skills that **file** Linear issues (`linear-task`, `audit`,
`audit-scope`, `trim-context`, `housekeeping`, `plan`) resolve the
filing destination — team, project, assignee — from **environment
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
# Used by the plan skill — the "Planning" document a planning
# session bootstraps from and writes its decisions back into:
export LINEAR_PLANNING_DOC_ID=…
# NOTE: LINEAR_API_KEY belongs here by rights — the sync-blockers
# Python tool needs it, because a script can't use the OAuth-based
# claude.ai Linear MCP and must authenticate with a personal key
# sent as the Authorization header. It is NOT set here, though: it
# is a secret, so it is resolved from 1Password at session launch.
# See local-integrations.md, "Session secrets".
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
reconciliation, reporting **collision clusters** (the input to the
`plan` skill's merge-group proposal), the surviving human-declared
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

- `**Fingerprint**: <domain-token>:<slug>` — the dedup key `audit`
  matches on so a finding is never refiled. Mandatory on audit
  findings; one line per finding (a merged issue carries several).

  **The first token is a dotless domain token, never a bare
  `name.ext`.** Write `feeds-http:get-json-response-size-cap`, not
  `http.rs:get-json-response-size-cap`. Linear's writer **linkifies a
  hostname-valid basename**: a line beginning `http.rs` was stored as
  `[http.rs](<http://http.rs>):…`, corrupting the key. Underscores are
  invalid in DNS labels, which is why most multi-word snake_case
  basenames survived by accident while short ones (`http.rs`, `app.rs`,
  `main.rs`, anything ending `.io` / `.sh` / `.md`) are the hazard
  class. Derive the token from the path instead of the basename — it is
  more legible anyway, since `http.rs` alone does not say *whose*.

  **Do not escape it with backticks instead.** A code span dodges the
  linkifier but reintroduces two worse problems: it is a poor `patch`
  anchor, and it can come back corrupted from a wholesale document
  rewrite (see "Anchors must match the *stored* text"). One prescribed
  form keeps the dedup search single-shaped; two would not.

  **What actually breaks is search, not a parser.** No tool parses this
  field — `sync_blockers.py` and `merge_tasks.py` read `**Touches**:`
  only, and `merge_tasks.py` merely carries fingerprint lines through
  verbatim. Dedup is done by a filing skill **searching open issues**
  for the key, so a mangled fingerprint degrades that search rather
  than crashing anything. The rule stands regardless: a key that cannot
  be found is a key that does not dedup.

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

  **Glob-vs-diff drift is mechanically detectable, and deliberately not
  automated yet.** An issue whose merged PR touched paths outside its
  declared globs is exactly the shape of a prose-widened scope that
  never updated the field, and could be flagged after the fact. It is
  recorded here as *evaluated, not built*: the check needs the merged
  PR's file list, which means `sync_blockers.py` would take a GitHub
  dependency it currently does not have — it is Linear-only and
  stdlib-only by design. Adding one to catch a class the amendment rule
  (see the `plan` skill, step 4) already addresses at the source is the
  wrong order. Revisit if the rule proves insufficient in practice.

### Collision clusters, not serial chains

File overlap is reported as a **cluster** — the issues that collide on
one shared path — rather than as an ordering. A cluster is the candidate
set for "these would land as one PR", which is what the `plan` skill's
merge-group proposal step consumes (it also picks the parallelizable
batch when promoting parked audit findings). Grouping is **per path**,
not by
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
`docs/market-making.md` in unrelated sections. A cluster carries the
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
  `housekeeping`, and `plan` prepend `Claude:` to a title when the
  issue's
  `**Touches**:` globs are all on the meta surface above. `/merge-tasks`
  applies it when every issue it consolidates is meta. (`plan` matters
  here in particular: a planning session is where most meta-work issues
  are actually filed.)
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

Each op carries `op` plus its own fields. The literal argument names
matter and are easy to guess wrong — only the `replace` shape is
memorable, so `append` gets handed an `old_string` and is rejected.
That cost three wasted round trips across two sessions, each one
paying the fixed full-body echo for nothing:

| `op`            | arguments                                          |
| --------------- | -------------------------------------------------- |
| `append`        | `text`                                             |
| `prepend`       | `text`                                             |
| `insert_before` | `anchor`, `text`                                   |
| `insert_after`  | `anchor`, `text`                                   |
| `replace`       | `old_string`, `new_string`, optional `replace_all` |
| `replace_range` | `from`, `to`, `new_string`                         |

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

### Field-only writes go through `board_batch.py`, not the MCP

There is one way to make the echo vanish entirely, and it is to leave
the MCP. Linear's `issueUpdate` returns whatever the caller **selects**,
so a mutation selecting `success` alone returns a single boolean:

```sh
python3 .claude/tools/board_batch.py fields --updates <file>
```

Use it for **every non-body update** — priority, state, parent,
milestone, labels, assignee, and relation add/remove. One planning
session made 21 writes of which **17 touched no body at all** (a
priority change, three parent/state/priority moves, eleven milestone
stamps, one relation removal) and paid roughly **40k** echoing bodies to
confirm changes that fit on 17 lines. The same session then cleared four
priorities through a throwaway script for about **60 tokens** of output
carrying identical information.

Its `list` subcommand is the same trade for reads: a compact
`number | priority | title` listing measured ~600 tokens where the MCP
`list_issues` equivalent measured ~11k, on a call the planning and
filing skills make every pass.

**Body edits stay on the MCP `patch` path**, and that is a deliberate
boundary rather than an unfinished job. Linear's API has no patch
primitive — `description` is a whole string — so a Python body-writer
would have to fetch, apply locally, and write back **wholesale**, which
costs the read anyway and reintroduces the round-trip corruption hazard
documented above. The MCP `patch` does anchor matching with **atomic
abort on ambiguity**, and that safety is load-bearing: it has correctly
refused writes whose anchor matched two locations rather than guessing.

`edges` is covered under "Blocking relations" below — it executes an
operator's decision and is never called by automation.

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

Two generalizations of the checklist rule, both measured:

- **Accumulate a session's `patch` ops for one issue and issue them
  together.** The batching rule above reads as being about repeated
  *ticks*, so it gets skipped for writes of different kinds — but the
  echo does not care what an op says. One run made a mid-session
  narrative append and a later set of box-ticks as two separate `patch`
  calls on the same issue, ≈3.9k avoidable and about a quarter of that
  session's largest sink. A `patch` array takes up to 50 ops; hold them
  until you have them all. The genuine exception is a write **gated on a
  different event** — In Review is gated on CI going green, so it cannot
  fold backwards into a write made before that was known.
- **One call carries priority, body and relations together.** A
  priority-only `save_issue` echoes the whole body for a one-field write,
  and one planning run made ~17 of them, several on issues that took a
  second save within the hour. When a filing decision sets more than one
  thing, set them in a single call rather than one call per field.

**The worst measured case was a planning session: 97 `save_issue` calls,
≈164k.** Its top five results were all late folds onto the same
aggregated survivor body. A burst of peer folds is the shape to watch —
several handoffs for one issue arriving within minutes, each taking its
own echo. Buffer them and write once.

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

**A stale echo is not evidence the write failed.** A `description`
write has been observed returning the *pre-write* body with an
unchanged `updatedAt`; the session concluded it hadn't landed and
compensated with an append, which was that run's single largest
avoidable cost. Never compensate — re-read.

**The op payload field is `text`, not `content`.** An invalid-input
error naming the patch field is what a wrong field name looks like.

**A ticked checkbox stores as `- [X]`, uppercase.** Linear normalizes
the `x` on write, so a later op anchoring on `- [x] …` matches nothing
even though that is exactly what the previous write sent. Anchor a
re-tick or an un-tick on `- [X]`, and read the box state back from the
stored body rather than from what you wrote.

**Ticking many boxes is three ops, not one per box.** A `replace` with
`replace_all` over the bare `- [ ]` prefix ticks every open box in one
op; follow it, in the same array, with one `replace` per box that
should stay open, flipping it back. Ops apply **in order and
atomically**, so the result is "all but these". This matters at scale:
a consolidated issue can carry more boxes than the 50-op cap allows,
and it keeps the write anchor-based — which is what makes it fail
loudly instead of clobbering a concurrent amendment. A full-body
`description` write would silently overwrite one.

### Two write-mangle rules for every body you file

Linear's writer rewrites some markdown on the way in, so two shapes
must never appear in a filed body:

- **No emphasis span may wrap a newline.** A bolded run crossing a line
  break stores garbled — `**a\nb**` comes back as `**a****\n****b**`.
  Close the emphasis on the line that opens it.
- **No machine-parsed field may start with a bare hostname-valid
  `name.ext`.** It gets linkified. This is the fingerprint rule in
  "Structured filing fields", stated once more here because it binds
  any field a reader or a search is expected to match on.

A **wholesale** content replacement re-parses the entire body, so it is
where both rules bite hardest — see the `plan` skill's close-out step,
which additionally composes without inline code spans for this reason.

### The write floor assumes the body is read once

The per-issue floor above ("buffer folds; write them once") assumes a
session reads the body at the start and holds it. **Amendment breaks
that assumption**, and the resulting extra write is expected rather
than a lapse.

Concretely: when a planning session appends checkboxes to an issue an
implementation session is *already working from*, that session cannot
see them until its next echo returns them — so a second write it could
not have batched is structural, not indiscipline. Do not read the floor
as blaming a session for a cost it had no way to avoid.

**The avoidance is a message, not a tighter budget.** A planning
session amending an in-flight issue should **tell the worktree session
directly** — the `plan` skill already has a coordinate-with-in-flight-
sessions step. That turns an invisible amendment into a message and
removes the wasted round trip entirely.

## Blocking relations

**No automated writer files a blocking edge — ever.** Not
`sync_blockers.py`, not a filing skill (`linear-task`, `audit`,
`audit-scope`, `trim-context`, `housekeeping`, `merge-tasks`, `plan`),
not an autonomous audit rotation. This holds for edges an agent believes are
genuinely semantic, not just for file-overlap ones.

**Where they *are* placed: a planning session.** A blocking edge
expresses semantic rollout ordering, which is a scheduling decision, so
it is made where somebody is actually deciding the order — the `plan`
skill's session in the base repo, human-directed, one edge at a time.
That is the whole of the exception: not "a skill that is allowed to",
but "the place a human does it".

**The mechanism is `board_batch.py edges`**, and it changes none of the
above. There is no MCP path for relations at all, so a planning session
needs *some* tool to execute the operator's decision:

```sh
python3 .claude/tools/board_batch.py edges --pairs <file>
```

Read it as the human's hands, not as a new writer. It takes an
**explicit pair list**, has **no discovery mode**, and **refuses an
empty list** — so it cannot originate an edge, only carry one out. It
is never called by a filing skill or by any automated pass.
`sync_blockers.py` remains the **only** automated relation writer and
files `related` links exclusively.

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
