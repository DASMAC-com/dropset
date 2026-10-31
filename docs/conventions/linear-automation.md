<!-- cspell:word basenames -->

<!-- cspell:word strikethrough -->

# Linear automation

Skills that **file** Linear issues (`linear-task`,
`audit-scope`, `trim-context`, `plan`) resolve the
filing destination — team, project, assignee — from **environment
variables**, never hard-coded UUIDs. (Skills that only **update**
an existing issue by id — `init-pr`, `review-pr` — need no
destination.) Set them once in your
shell profile (`~/.zshrc`):

```sh
export LINEAR_TEAM_ID=…
export LINEAR_PROJECT_ID=…
export LINEAR_ASSIGNEE_ID=…
# Used by the plan skill — the "Planning" document the scheduler
# bootstraps from and writes its decisions back into:
export LINEAR_PLANNING_DOC_ID=…
# NOTE: LINEAR_API_KEY belongs here by rights — it has two
# consumers. The Python board tools are not MCP clients: they
# talk to the GraphQL API directly, with a personal key sent
# BARE in the Authorization header. The locally configured
# Linear MCP server needs the same key, but prefixed with
# `Bearer` — see "Two ways to reach Linear's MCP" below; the
# bare form 401s there. It is NOT set here, though: it is a
# secret, so it is resolved from 1Password at session launch.
# See local-integrations.md, "Session secrets".
#
# RETIRED: LINEAR_SESSION_METRICS_DOC_ID. The "Session Metrics"
# inbox document is gone — the fold analyst reads the daily
# session digest now. See the trim-lever pipeline below.
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

**The trim-lever pipeline runs on the daily digest.** Sessions
**report**; the analyst **judges**. A session ends by writing what it
kept running into in its own `Session account` block (the
`session-metrics` tail) and the daily upkeep pass's digest lifts each
block verbatim beside
that session's numbers (`docs/conventions/context-economy.md` → "The
daily digest"). `trim-context` is the **fold analyst**: it reads the
whole day's digest at once — every session's sinks, hardening
candidates and account, plus any flagged CI lines and the ranked
`## Wait` causes — and judges which levers clear the reservoir bar
(turn-eliminating, prose-subtracting or hardening, roughly 3k per
occurrence; a planning-derived lever must be quality-preserving). A
**wait lever** subtracts a recurring, named wait cause and files under
its `wait:` fingerprint; a cause-less wait is reported, never filed.
Each candidate is deduped twice: by fingerprint
(`trim_levers.py probe` — a canceled match is a recorded rejection),
and against the open Backlog **by title** (`linear_issue.py find`),
since a near-duplicate under a different fingerprint is the likelier
miss. A near-match is **folded into the existing issue as a new part**
through the zero-echo append, never filed as a sibling, so the Backlog
does not accumulate similar trim tasks. The rest file as **small
themed** propose-only `Claude:` batches — roughly 4–5 parts each, one
`**Fingerprint**:` per part, composed by `trim_levers.py compose` —
**straight into Backlog with no milestone**: the fold runs in the
**analyst**'s own tab each morning (`.claude/skills/analyze/SKILL.md`,
or a by-hand run of the same method) and its output is already
folded, so the park-then-promote hop executor filings take does not
apply. The analyst hands the new batches, with the kill and merge
candidates it gathered, to the curator as its first message, and
writes a fold stamp the scheduler's bootstrap says
(`upkeep.py fold-today`); the batches are reviewed from the
curator's pool like any other un-parked Backlog task. A candidate
rejected on judgment is filed `Canceled` with its reason, which
suppresses it permanently. The coherence floor still governs audit
findings and product filings; the meta-work fold's **no-size-bound
exemption is retired** (operator rule, 2026-09-11), because session
cost is roughly quadratic in session length — see
`docs/conventions/context-economy.md` → "Session length is itself a
cost lever".

**Trims are the analyst's, not a session's.** A session files deferred
*implementation* work into the curator's pool and files nothing that
is only a trim — those are what the digest surfaces. This is what keeps
a session from spending context on levers other sessions already
reported.

Every full `plan` bootstrap spawns `trim-context` as a background
sub-agent — the way `review-pr` spawns its reviewers — gated on
upkeep's `digest` line having written a digest, its brief a pointer to
that file; its capped report is the completion notification, and its
reads never enter the planning prefix. It also runs standalone.

The `Trim levers` milestone and its writer (`file`, `append-evidence`,
`list`) are **retired**; `trim_levers.py` keeps only `probe` and
`compose`, and filing goes through `linear_issue.py`.

The `session-metrics` skill drives its measurement tool via
`make session-metrics`, which reduces to a
`Bash(make session-metrics:*)` allow-rule. That tool needs **no**
`LINEAR_API_KEY` — it only parses the local transcript and makes no
network call.

*Retired:* a single "Session Metrics" inbox **document** that the
producer appended to and the consumer mined. It outgrew the harness's
tool-result cap between mining passes (67.0k characters at the last
one), so each pass spilled it to disk and mined it with a throwaway
script — structural at roughly ten parallel sessions a day, not a
tidiness problem.

*Retired:* the **automated file-collision machinery** — a
`sync-blockers` skill and its `sync_blockers.py` core, which read the
open Backlog's `**Touches**:` globs and filed a `related` relation for
every glob overlap it found. Nothing files a collision link any more,
at filing time or on a sweep. The mechanical overlap web added little
over what a scheduler session already knows: that session reads the
implementation specs into context anyway, so it judges overlap on
content rather than on exact glob matching. **Reconciling overlap is
scheduler work**, done from the ordinary board read — including
the merge-group proposals and the scheduling smells the tool used to
report. The `**Touches**:` field it consumed went with it — see
"Structured filing fields" below. Relations already on the board are
left alone as inert history: nothing deletes a relation.

## Two ways to reach Linear's MCP, and the credential each takes

`https://mcp.linear.app/mcp` is **one server reachable two ways**, and
which way a session gets decides whether it has Linear at all.

- **The hosted `claude.ai Linear` connector** rides claude.ai account
  authentication. A session holding no claude.ai session — a **Bedrock**
  session authenticates to AWS — does not receive it, and no config edit
  makes the hosted connector appear. Verified by the Bedrock spike: such a
  session receives every locally configured server and neither claude.ai
  connector, with the matching "Managed settings (remote): not fetched"
  line in its MCP status output.
- **A locally configured server** at the same URL, authenticated with a
  personal API key in a `Bearer` header. Locally configured servers
  reach every session whatever the model substrate, so this is the form
  that gives a Bedrock session first-class Linear access.

Register it exactly the way the GitHub server is registered
(`github-mcp.md` → "Authentication (PAT header, not OAuth)") — user
scope, the credential referenced by **variable**, never written into a
committed file or `~/.claude.json`:

```sh
claude mcp add --transport http --scope user linear \
  https://mcp.linear.app/mcp \
  --header 'Authorization: Bearer ${LINEAR_API_KEY}'
```

`LINEAR_API_KEY` already resolves into every session from 1Password
(local-integrations.md → "Session secrets"), so this adds no secret,
no new coordinate, and no custody question.

### The `Bearer` prefix is the whole trick

Its absence is a **convincing false negative**, which is why it gets its
own heading. Linear's GraphQL API takes the key **bare** —
`Authorization: <key>`, which is what `linear_api.post` sends — but the
MCP endpoint rejects that same form with

```txt
401  WWW-Authenticate: Bearer realm="OAuth", …, error="invalid_token"
```

An OAuth challenge naming no usable key path reads like the wall the
GitHub server hit — a different mechanism (there, OAuth failing for lack
of dynamic client registration), but the same impression — and would
justify concluding that no key-based path exists and a bespoke tool must
be built instead. It does exist: the identical key sent as `Bearer <key>`
returns a clean `initialize`. Measured 2026-09-08.

Two further facts from that probe. The legacy `/sse` endpoint is **gone**
(404 under either header form), so `/mcp` is the only endpoint. And the
tool surface is identical rather than merely similar: a `tools/list` over
the API-key session returns **73** tools, one-for-one the same set the
hosted connector exposes. That is the same server answering, reached with
a different credential.

`list_comments` is the one worth naming. The committed zero-echo tools
already cover state transitions and body edits, so a session without
Linear MCP can run most of the flow — but none of them reads comments,
and acceptance criteria sometimes live in an anchored comment rather than
the issue body. Such a session can therefore believe it holds the whole
spec while missing a criterion, with nothing signalling the omission.

**What this verification does and does not establish.** The registration
above was confirmed `✔ Connected` at user scope, which proves the
credential, the endpoint and the `${LINEAR_API_KEY}` expansion. It was
first confirmed from a **subscription** session, not a Bedrock one, so
that Bedrock sessions receive it was initially an inference from the
spike's established fact that they receive all locally configured
servers.

**Confirmed directly on Bedrock, 2026-09-10.** A worktree session running
with `CLAUDE_CODE_USE_BEDROCK=1` on the `us.`-profile Opus 5 model
received the locally configured `linear` server and drove the whole
Linear half of its work through it — `save_issue` state transitions, two
anchored body `patch` writes, and `list_comments`. So the inference is now
an observation, and the `list_comments` gap named above is closed for a
Bedrock session that has this server registered: the tool the zero-echo
writers cannot replace is the one that was exercised.

That session also received all nine locally configured servers and
neither claude.ai connector, which is the same split the spike measured.

## Structured filing fields

Every filed issue carries one machine-readable field the automation
reads back, on top of the human prose — `**Fingerprint**:`. It was two
until `**Touches**:` was retired (see below). Keep the field **name**
stable: the filing skills emit it and the dedup probes match on it.

- `**Fingerprint**: <domain-token>:<slug>` — the dedup key `audit-scope`
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

  **What actually breaks is search, not a parser.** Dedup is done by a
  filing skill **searching issues** for the key, so a mangled
  fingerprint degrades that search rather than crashing anything. The
  rule stands regardless: a key that cannot be found is a key that does
  not dedup. (`trim_levers.py` now refuses a dotted domain token
  outright, so at least that pipeline cannot store a corrupted key.)

  **The fingerprint search *is* the dedup probe — don't read bodies.**
  Measured: a fingerprint search answers "does an issue already cover
  this?" for ~200 tokens against **8.4k** to read the body, and answers
  it more reliably, since skimming a long issue for a half-remembered
  claim is exactly where a duplicate slips through. Two sessions instead
  paid ≈5.6k and ≈3.0k for dedup searches returning full issue objects
  — truncated descriptions, project / team / assignee ids, timestamps —
  to settle a yes/no. So: search the key, cap `limit` at what a human
  would actually scan (~5), request the narrowest field set, and read a
  body only when the decision genuinely turns on surrounding text.

  **Search resolved and archived issues too, not just open ones.** A
  lever or finding that was **rejected** is closed *with its reason*,
  and dedup-against-resolved is what makes that rejection stick. Nine of
  thirteen mined session entries carried an explicit "do not mine this
  as waste" note, several written only because an earlier pass had
  re-proposed the very thing on intuition. An open-only dedup search
  re-opens every one of those arguments.

  **Locate a sibling issue by identifier or fingerprint — never by a
  broad listing.** Making the narrow lookup possible is what this field
  is *for*, so falling back to a wide list throws the field away: one
  session returned **ten full bodies** to find two siblings. Search the
  key, not the topic.

  **And never re-fetch a body that has already been echoed.** The same
  session followed that listing with a `get_issue` for a body the
  listing had just returned — paying twice for one payload. Every write
  echoes the full description back (see "Partial edits"), so after any
  `save_issue` the body is already in hand; read it from that response.
  For a long body, spill it once with
  `linear_patch.py read --out <file>` and slice the file thereafter
  rather than re-reading the field.

That is the whole field set. `**Fingerprint**:` is the only one.

### Retired: `**Touches**:`

A filed issue used to carry `**Touches**: <glob>[, <glob>…]`, the path
globs the fix would edit. **Nothing emits it any more.** It was the
input to the automated collision machinery (see the retired-machinery
note above), and once that was deleted nothing read it: composing a
glob list became filing overhead paid for a field with no consumer, and
the operator's ruling was to drop it rather than keep it on the chance
that something might want it later.

**Do not reintroduce it, and do not reintroduce what consumed it.** The
direction is content-based judgement in a scheduler session — that
session is holding each issue's actual prose, which is strictly more
than a glob list tells it — not exact glob matching. If a future pass
wants declared scope back, that is a decision to take deliberately, not
a habit to drift into one filing at a time.

**Existing lines stay, and the fold tools still carry them.** Bodies
filed under the old convention keep their `**Touches**:` line as inert
history, exactly like the relations. `merge_tasks.py` emits a line only
when given globs and tolerates its absence by construction, so folding
an old issue preserves
whatever it declared without requiring a new one to declare anything.

**Be honest about what the whole retirement costs.** Automated collision
recording was **dropped**, not replaced: a scheduler session's read is
the successor and its coverage is unmeasured. That is an accepted trade
— a spurious edge costs more than a missing one (see "Blocking
relations") — not a guarantee of equivalence. Losing the declared-scope
field widens that gap slightly, since there is no longer a cheap
cross-check on an issue whose prose quietly outgrew its stated scope.
Accepted on the same reasoning: the amendment rule (see the `plan`
skill, step 4) addresses scope drift at the source, and a field nothing
reads does not catch it either.

### Overlap is a cluster, never an ordering

When a scheduler session judges that two issues will edit the same code,
the conclusion it records is a **cluster** — the issues that collide on
one shared area — never an ordering. A cluster is the candidate set for
"these would land as one PR", which is what the merge-group proposal
step consumes (it also picks the parallelizable batch when promoting
the legacy `Audit findings` pool).

Group **per shared area, not by connected component.** Coupling chains
through shared files, so the transitive reading is worthless: when the
retired tool computed it that way it collapsed 25 of 27 open issues into
one cluster, which proposes nothing. Clusters therefore overlap — an
issue appears under every area it touches.

And a cluster is **not** a chain of edges. An earlier design turned each
collision into a `blocks` edge, and paid for it: because `**Touches**`
globs are coarse (crate-level), the orientation was arbitrary (lower
number blocks higher), and block semantics are binary, the board grew
giant serial chains — a day-1 mainnet param-channel issue sat behind
**eight** overlap blockers, and a docs-only pair was block-linked
because both touched `docs/market-making.md` in unrelated sections. A
cluster carries the same information without asserting an order nobody
decided.

### Fold coupled findings into one issue

A rotation should yield the **fewest coherent PRs**, not one issue per
finding — subject to the session bound below. When a run turns up
multiple findings, fold together every set that would sensibly land as
**one PR** and file each set as a single issue — *before* the dedup check
and the `save_issue`.

**"Fewest" is bounded by what a SHORT session can carry** (roughly 4–5
findings, and the line band in
`docs/conventions/context-economy.md` → "Session length is itself a cost
lever"). A coherent set larger than that splits into **sequential** PRs
rather than one oversized issue; coherence governs what may be
*separated*, never how large one unit may grow.

The bar is **same-PR coherence**, not same-file: fold findings that
share a subsystem, crate, or language-domain and that a reviewer would
naturally review and land together. So every doc-/comment-freshness fix
across the run → one issue; every low-risk refactor within one crate →
one issue. This is wider than "same file or symbol" and spans **units
within a rotation**, not only findings within a single unit.

A folded issue keeps **every** finding's own `**Fingerprint**:` line
(one per line, so per-finding dedup still matches).

**The coherence floor — do not fold across deploy units.** Never merge
findings a single PR can't sensibly review or land as a whole: different
apps, languages, or deploy units stay apart. A TUI Rust rendering fix, a
frontend TS hook fix, and an on-chain program refactor are **three** PRs,
not one — even though all three are "cleanup from the same rotation".
Fold *within* a coherent PR boundary; never across one. "Aggressive"
means minimize PR count up to that floor **and up to the session bound
above** — never build a mega-PR past either, incoherent or not.

**The `Claude:` meta class is size-bound too — operator rule,
2026-09-11, retiring the 2026-08-24 no-size-bound exemption.** Claude
meta work folds into **small themed batches of roughly 4–5 parts**, and
several may be open and worked at once. So `merge-tasks`'
oversized-survivor warning fires on this class exactly as on any other,
and the `plan` skill's fold doctrine states the new form: small themed
batches, no edge, **no in-flight gate**.

The retired exemption's reasoning is worth recording, because it
inverted rather than expired. It ran: at most one meta task ever runs in
flight (they contend on the same skill files), so a second meta issue
buys no parallelism and costs a merge conflict, and churn speed through
the self-improvement loop matters more than per-issue readability. The
contention is real — what changed is its **price relative to the
alternative**. Session cost is roughly quadratic in session length
(`docs/conventions/context-economy.md` → "Session length is itself a
cost lever"), so serializing meta work into one unbounded batch buys
that quadratic in order to avoid a **rebase** that costs cents in a
short session. Measured counterexample: the ENG-1194 batch carried the
whole parked pool and was worked in a **19-hour, \$409** run. Churn speed
still matters most; small batches are how it is now bought.

The **coherence floor** is untouched and still binds: meta work never
folds together with product code, and a set that must land as one PR is
never scattered — it splits into *sequential* PRs rather than growing
past a short session. The split recommendation likewise stays in force
for **product** issues.

A worktree branch and its Linear issue **share one `ENG-###`
number**: branch `eng-499` ↔ issue `ENG-499`. Skills resolve the
issue from the branch (or the PR title scope) on that basis —
`init-pr` moves it to In Progress at bootstrap, `review-pr` ticks the
delivered checklist items and moves it to In Review at the merge-queue
handoff — once the PR is ready, CI is green, and the review summary has
been printed for the human.

### The Linear state tracks the SESSION, not the PR

This is the framing that makes the transitions above cohere, and it is
easy to get backwards. **The Linear state is the state machine of the
agentic programming session**; the PR's own merged/open state tracks the
*code*. They are different clocks.

So:

- **In Review** covers *both* PR-under-review **and**
  merged-with-follow-up-outstanding. A session still owes work after its
  merge — session metrics, post-merge tidy, feedback to
  hand to a scheduler session — and that work is invisible if the issue
  reads Done.
- **Done means operator-ratified complete.** Never merely merged.

Before a session starts, **Backlog** and **Todo** are review states, not
session states: Backlog awaits the operator's review and Todo is
reviewed and dispatchable (see "Backlog reviews, Todo pulls").

**The team's Git workflow automation makes no state transition on
merge**, which states this reading natively: the **In Review** written
at `review-pr`'s enqueue handoff is simply the state the issue keeps
through the merge and after it. `review-pr`'s outcome-watch step runs
its follow-up tail and moves to Done only on an explicit
`AskUserQuestion` approval. Two consequences elsewhere:

- **The upkeep pass prunes on the issue's status TYPE, not on
  PR-merged** — only `completed` or `canceled`. A merged PR whose issue
  reads In Review is a live session, and pruning its worktree is how
  work has been lost before.
- **The fleet-resume launcher resumes In Progress *or* In Review**, so a
  merged-but-unfinished session reopens on machine start.

This replaced a **write-back**. The integration used to auto-transition
to Done on merge, and `review-pr` observed that and wrote the issue back
to In Review. Moving the rule into the team setting retired that write,
which cost a full-body Linear echo in every implementation session to
restore a state nothing should have moved. It also closed the gap the
write-back carried: a session dying before its outcome watch ran used to
leave the issue auto-Done with nobody to re-mark it. Nothing sets Done
now except an explicit approval.

## An audit is a board issue, not a directive

Auditing is **planning-filed board work**. It is not a step in any
automated pass, and no skill reads an audit directive from the Planning
document — that path is retired, along with the `housekeeping` skill
that once read it.

**Why the directive path went.** An audit is a real capacity spend, and
routing it through a document made it an invisible daily tax: three
handoffs across two skills and a document, plus a built-in staleness
window between writing the directive and firing it. It also failed
quietly in a way nothing could see — the skill that claimed to read the
directive never resolved `LINEAR_PLANNING_DOC_ID` anywhere, so an
unresolved document id was indistinguishable from a legitimate
"directive says none" and reported clean. And the audits that matter
keep turning out to be **issue-shaped**: they carry scope, rationale,
sequencing, and accreted operator scope the way real work does.

**The audit issue.** A first-class **Backlog** task carrying:

- a **named target** from `docs/conventions/audit-registry.md` — a
  subsystem, an inter-subsystem interface, or a random-target pass when
  breadth is what is wanted;
- the **scope** (what is in, what is out);
- the **rationale** — which criterion fired (*just settled*, *about to
  be built on*, *suspect*) and its evidence;
- the **sequencing**, as prose. No blocking edge: an automated filer
  places none, and this one is filed by a scheduler session, which
  places edges deliberately (see "Blocking relations").

The session that pulls it runs **one scoped `audit-scope`** pass
against that target. Pulling and invoking it **is** the authorization
for the adversarial sub-agent fan-out. Its findings file **parked**,
exactly as before — see the next section.

**Because a filed audit is a snapshot, re-verify before running it.**
An audit issue pulled weeks after filing states a premise about code
that may have moved; check the target still exists in the shape the
rationale describes before spending the fan-out, the same discipline a
`trim-context` part gets.

**The heartbeat is what keeps the cadence.** Every planning bootstrap
reads the Planning document's bounded **audit-state table** — one row
per registry unit: last-audited date, finding count, and an outcome
pointer (issue numbers) — and either files an audit issue or
**declines with a recorded reason**. The table is **state-shaped, never
an append log**: a row is replaced when its unit is re-audited, so the
table is bounded by the registry rather than by how long the practice
has run, and the history lives on the board. The known risk of the
issue model is cadence decay by neglect; the heartbeat is the guard,
which is why an unrecorded non-decision counts as a failure of it.

## Reading the Planning document from outside a scheduler session

A worktree session that wants to check itself against planning direction
is doing something worth **encouraging**, so the check has to be cheap.
Two rules, in order:

1. **Ask a live scheduler first.** `ListAgents` is the liveness
   check and `SendMessage` is already the documented channel in the
   other direction. A peer answers the *question* rather than the text:
   one session that read the document and then messaged the
   scheduler got the state of both blockers, the operator's posture, a ruling
   on the actual question, and an unprompted correction to a figure in
   the document that had gone stale. The read surfaced none of that,
   because **a stale line reads exactly like a current one.**

1. **Otherwise use the scoped reader**, never `get_document`:

   ```sh
   python3 .claude/tools/planning_doc.py --headings
   python3 .claude/tools/planning_doc.py --section 'Audit state'
   ```

   It fetches in its own process and prints only what was asked for,
   resolving `LINEAR_PLANNING_DOC_ID` itself so every form reduces to
   one allow-rule. `--out <file>` spills the whole content and prints a
   heading map, for when several sections are wanted.

The MCP `get_document` returns the entire content with **no slice
accessor**, and this document only grows between close-out rewrites, so
the cost rises over time. Measured: one such read was a session's
largest single main-loop result at **≈7.9k**, more than double the next
non-filing result, consumed for four short passages out of a document
also covering the board schema, four live tracks, the current phase,
feeds roster detail, migration numbering, the calendar track,
session-metrics state, standing decisions and verification debt.

## Backlog reviews, Todo pulls

The state and the milestone together say where an issue sits:

| State                        | Milestone         | Meaning                                                                    |
| ---------------------------- | ----------------- | -------------------------------------------------------------------------- |
| Backlog                      | none              | a spec awaiting the operator's review — the inbox                          |
| Backlog or Todo              | other parking     | **parked**: hidden from the operator's project view, bar `Curator pending` |
| Backlog                      | `Session pending` | an advisor session's filed tasks, under its in-tab review; short-lived     |
| Backlog                      | `Curator pending` | the curator's current batch, still in its pool; short-lived                |
| Todo                         | none              | reviewed and dispatchable; the body is frozen                              |
| In Progress, In Review, Done | —                 | the session state machine, unchanged (see above)                           |

**The operator's move from Backlog to Todo is the approval.** The
scheduler never makes that move, nor does any session on its own
judgment: the curator's one-at-a-time move executes the operator's
tick in its tab (see "The curator owns review" below), and an advisor
session's `ratify-staged` batch executes the operator's final answer in
its own. That single-writer rule is why "state is Todo" proves
approval with no reaction read. **Withdrawal** is the reverse move. A
**fold into a Todo body drops it back to Backlog**, because the body the
operator approved is no longer the body on the issue.

**The dispatch gate is two checks: state Todo, and unblocked** — the
retired thumbs-up reaction plays no part. An
unresolved comment thread on a Todo issue is *not* a refusal, since only
the operator may make that move (operator, 2026-10-21). The **fold
protocol**: fold the comment into the body, **reply in that comment's
thread** saying what changed and where it landed
(`linear_patch.py comment --reply-to <comment-id>`), then resolve the
thread with the reply as its resolving comment. A resolved thread
therefore means addressed-and-how, never approval. A fold whose reply
fails **leaves the thread open**, because a resolved thread with no
answer reads as approval with nothing behind it. The operator's **Next**
view is the
unblocked Todo; it is Linear's server-side blocker filter, which is why a
scheduler hold is expressed as an edge (see "Blocking relations").

An issue stamped with a parking milestone — `Session pending` for a
advisor session's tasks under review, and the legacy `Audit findings` —
is **parked**: filed so it is not lost,
deliberately *not* in the inbox or the queue. A filing skill sets
**state `Backlog` plus the milestone, in the creating call** (per
"Relations and state belong in the CREATING call"). **Promotion is one
write, the milestone clear**, which lands the issue in the inbox for the
operator's review. Nothing about parking places or implies a blocking
edge — see "Blocking relations". A Backlog filing is already out of the
queue by state; the milestone is what keeps a filer's own output out of
the operator's **inbox** until it has been reviewed.

**A parking milestone means "not yet reviewed", never "not now"**
(operator ruling, 2026-10-23: nothing sits in a giant backlog). Who
resolves a filing splits by the filer's tier:

- **An advisor-tier session** (architect, explore, audit) resolves its
  own proposals before it closes: they
  stage under `Session pending` and are ratified in that session's tab,
  its close gate leaving none behind.
- **An executor session** (`task`, `chore`) never holds a review: its
  follow-ups and meta filings land in the inbox (its trims it files
  not at all — see the trim-lever pipeline), where the curator reviews
  them with the rest of the pool.

**New audit findings are ratified in the audit session's tab**, never
promoted by the scheduler: `audit-scope` stages them under
`Session pending` and runs `ratify-staged` before the audit issue goes
In Review, and a scheduler session it messages about a finding answers
with feedback only. `Audit findings` is **retired as a filing target**;
its legacy contents drain one finding at a time — each scheduler
bootstrap promotes or kills what it holds — and it leaves the parking
set once empty.

**`Session pending` is never promoted by the scheduler or the
curator.** It is the unified
staging lane for every advisor session's filed tasks — architect,
explore and audit findings — so the
operator reviews them in one place, in the
filing session's own tab, through the shared `ratify-staged` skill:
one checkbox per task, and once every task is ticked and uncommented
the session clears the milestone and moves the whole set to Todo in one
batch on that answer. It is **short-lived by design**: a count at
bootstrap is a session that died mid-review, and the count-and-offer
line is the backstop — offer to resume that session, never promote its
tasks around it.

**Where an advisor session's spec lands, and the record of what it filed.**
At the filing yes, before the first create, the session posts the agreed
spec verbatim, task list included, as **one comment on its governing
issue** (`linear_patch.py comment`); a design written into the governing
issue's body itself gets no comment. Each task is then filed as a
**sub-issue of the governing issue**: `linear_issue.py create` with
`--parent ENG-###` resolves the parent and sends `parentId` in the one
`issueCreate`, so a filing costs no second write, and a parent that does
not resolve refuses before anything is filed, since a half-parented set
is worse than none. The **sub-issue relation is the filing record**; a
reject shows as a canceled child. `ratify_staged.py` takes `--parent` in
place of `--ids` on `ask`, `review`, `approve` and `gate` (and narrows
`list` to it), so a session that died mid-review resumes from its issue
number alone. An audit pulled from an audit issue parents its findings
on that issue; an ad-hoc `--rotation` run has none. The curator's
architect spin-off takes no parent: it is the future parent.

The governing issue closes **Done with its sub-issues still open**,
which holds because the Engineering team's `autoCloseParentIssues` and
`autoCloseChildIssues` settings both read `null` — unset, and Linear
treats an unset auto-close as off (read from the GraphQL `Team` type,
2026-10-29). Turning on the child setting would close every open task
when its governing issue goes Done. Sub-issues are ordinary members of
every pool: the pool reads' hand-written filters carry no parent clause
(only `--parent` adds one), and a Backlog child is returned by the same
read the inbox uses (checked on 2026-10-29 against three Backlog
sub-issues, which only their milestones kept out of the pool).

**Every bootstrap says each parked count out loud.** Parked must be
invisible to the queue without being invisible to the operator, and a
count is what keeps those two apart: an issue nobody can see is an issue
nobody can promote. A genuinely urgent stray — a broken guard, a verb
that is actively hurting — is promoted on the spot rather than waiting
for the daily rhythm.

**A blocked issue is never a fold input**, because `/merge-tasks` does
not carry an inherited blocking edge silently — it re-proposes it, and
drops it unapproved — so folding an edge-held issue would rewrite or lose
a human-placed edge.

**The review is a necessity gate, and it closes on doubt.** A pool
member that cannot name a concrete recurring cost larger than its
landing-plus-ratification cost gets the curator's cancel
recommendation, with the reason; the operator's drop comment closes it
`Canceled`. A kill candidate's body is read, because the cost a kill
turns on lives there. The asymmetry is what decides doubt: a wrongly
closed issue's cost resurfaces when it recurs and is measured again —
in the next day's digest — while a wrongly kept one spends a session
and an operator ratification on noise. Meta filings otherwise compound,
because nothing ever declines one. **Doubt means doubt the issue is
worth doing, never "not this week"**: deferral is not a kill reason,
and an inbox the operator cannot keep up with is the signal to kill
harder, never to re-park.

### The curator owns review

Everything between Backlog and Todo belongs to the **curator**, a daily
advisor-tier session in the base repo (the `curate` skill, dispatched by
the analyst each morning or by hand with `curate`). It has no governing issue:
it runs every day, named by its date. The division of labour:

- **The curator judges** — what to promote, kill or merge.
- **The scheduler schedules** — the `plan` session owns dispatch,
  holds and the direction after Todo, and owns the Planning document.
- **Both hold priorities** — under the agreement rule below.
- **The analyst starts the day** — it runs the lever fold, then
  dispatches the curator with one hand-off message carrying the batches
  filed and the kill and merge candidates with their evidence. The
  curator's first ask shows those candidates as checkboxes beside the
  inbox's first batch; a ticked kill cancels the issue with its reason,
  a ticked merge runs `merge-tasks`.

**The pool is the un-parked Backlog**: state Backlog, no milestone,
`[Operator]` items included. Todo means "should this be done", so an
operator item is reviewed and ticked like any other, surfaced once in
review in priority order, then carried on every batch until Done: the
tick moves it to Todo, where dispatch refuses it, and every later batch
ask carries one Todo operator item (Urgent first, then oldest) as a
fifth checkbox after the four review ticks. A tick there writes Done; a
comment is a deferral, answered with a dated reply in its thread whose
count the next ask shows, and from the third deferral the curator
recommends cancel under the necessity gate, executed only by the
operator's `drop`. A comment `attended chore` swaps its token
for `[Chore/Bedrock]` and leaves it in Todo for the scheduler to dispatch
under the fleet gate — the curator dispatches no chore, and a
chore-token item never takes the fifth box. With the inbox empty the
curator asks one operator item per ask until none is left or the
operator says "stop for today".
The curator's own `Curator pending` mark keeps a task in the pool (see
"The curator marks its batch" below).
`Session pending` (another session's review),
`Audit findings` (the scheduler promotes them while the lane drains) and
`Post-validation` (the operator's deferral) are out of the pool.

**The in-tab tick is the operator's move.** The curator walks the pool
four tasks at a time, one checkbox per task with the `ratify-staged`
ask text, and moves each ticked task with no open thread to Todo **one
at a time**, as the tick lands. That is the curator's carve-out beside
ruling 12's batch: an advisor session reviews a fixed set it filed and
moves it whole at the end, while the curator's pool is open-ended, so
waiting for the whole pool would hold every approval behind the
slowest comment. Either way the move executes the operator's answer,
never the session's judgment. A ticked task with a thread is folded by
the fold protocol and carried to the next batch; a comment asking to
drop it cancels it, and a deferral goes to `Post-validation` only when
the comment names that milestone.

**Retitle on the tick.** A batch member whose title is shorthand, or
no longer matches its body after a fold, gets a proposal on its
commentary line: `title → <proposed title>`. A bare tick accepts the
Todo move and the retitle in one write. A comment `keep title` keeps
the old title, and a comment giving another title uses that one. A
held task that grew a part is retitled on the same rule when it
returns. A retitle **never changes** the leading
`[<Verb>/<Substrate>]` token, `[Operator]` or `Claude:`.
`board_batch.py`'s title write refuses a title whose token run differs
from the current one unless `--retoken` is passed. The curator's path,
`ratify_staged.py approve --title`, has no `--retoken` at all. If the
body contradicts the token, the curator raises it with the operator as
a comment.

**The curator marks its batch.** Before each ask the curator stamps
the batch `Curator pending` in one write
(`ratify_staged.py mark`). The ticked move clears the mark. A held or
unticked task keeps it until it is re-presented or until the close
sweep (`unmark --all`), and the next curator's bootstrap clears any
leftover the same way. So the operator's view shows what is under
review, and a count with no live curator means the curator stopped
mid-day. Only the curator writes this milestone, and the scheduler
never promotes it.

The curator writes nothing else on its own: no priority outside the
agreement rule below, no edge, kill or merge the operator did not tick
(see "Blocking relations"), no other milestone beyond its own mark, no
title token, no other issue's state or body beyond the fold. It
messages the scheduler every Todo move, cancel, merge, priority and
edge it writes, and hands it any kill or merge that reaches outside the
pool.

**The close.** On the operator's close-out yes the scheduler lists live
sessions and messages a live curator "closing for the day", without
waiting for an answer, or reports `curator: not live`. The curator
takes no new batch, finishes one mid-tick, states the inbox remaining
(count and ids), runs its session-metrics tail with its cost, and
prints `CLEAR TO CLOSE — close this window`, so its tab goes yellow
once. Before that it sweeps its `Curator pending` marks; no other board
write accompanies the stop, and no Planning-document note: the scheduler is
rewriting the document as the curator stops.

#### Priorities: shared authority, the agreement rule

**Both advisors hold priority authority** (operator ruling,
2026-10-29; ruling 2): the curator and the scheduler each write
priorities, in Backlog and in Todo alike, because priorities shift as
things come in and both sessions hold the context to judge them. The
**agreement rule** keeps two writers coherent, and every skill that
writes a priority cites it here rather than restating it:

1. A priority write is **messaged to the other session in the same
   turn**, with the reason (the edge rule, extended).
1. The other may answer with a **counter-write** and its reason.
1. A **third exchange** on one issue goes to the operator through
   `AskUserQuestion` in the tab that last wrote.

The scheduler owns dispatch order **as read**: it dispatches on the
priorities as they stand at the moment of the wave. The Planning
document records the result at the scheduler's next write-back. Two
writers racing on one issue is last-write-wins; the message rule makes
the loser visible in the same minute, and the third-exchange rule
bounds a ping-pong.

**Every filer proposes.** The `ratify-staged` filing ask carries a
priority per task, with the filer's reasoning in a word; the yes writes
it in the creating call; the filer then messages the live scheduler and
curator the set in one line. Either confirms by silence or rewrites
under the agreement rule; with neither live the proposal stands and is
named in the handoff. `linear-task`, `audit-scope` and the curator's
architect spin-off propose the same way. `[Operator]` items stay fixed
at Urgent. The review ask shows each task's filed priority, so an
inflated proposal is a tick-and-comment away from correction.

(Struck 2026-10-29: the 2026-10-28 rule that "priorities are the
scheduler's alone", under which the curator wrote none beyond the
architect task's fixed Urgent and messaged a priority it believed
wrong, waiting while no scheduler was live.)

### Which milestones park, and which do not

The project's milestones split into two kinds, and only one kind parks.
**Parking** milestones — `Session pending`, `Curator pending`, and the
legacy `Audit findings` — mean "filed, not yet reviewed", and an issue
carrying one is read as parked by every rule above, with one exception:
`Curator pending` marks the curator's own batch **under** review, so
its issues stay in the curator's pool. It keeps the parked rhythm:
it is counted out loud at every bootstrap, and only the curator's own
tick or sweep clears it, never the scheduler. ~~`Follow-ups`~~
(into which `Claude meta` already merged) is struck: filers land in
Backlog and message the curator, and the milestone is deleted once what
it still holds is cleared. The rest —
`Integrations`, `UX features`, `Hardening`, `Mechanisms` — are ordinary
**delivery** milestones grouping pullable work; an issue under one of
those is queued like any other and is *not* parked.

Say which kind you mean when you write a rule against "a milestone",
because the two behave oppositely. The distinction is by **name**, not
by any field Linear exposes, so it lives here rather than being
derivable — and the parking names are the closed set.
`Audit findings` takes no new filings — audit findings stage under
`Session pending` like any other advisor session's output — and it retires
when its legacy contents drain.
`Trim levers` retired with the per-session lever filing and is in
neither kind: upkeep no longer counts it, so its residue of open
levers is disposed of by hand in a scheduler session (folded into one
inbox batch or closed with reasons), which then closes the milestone.

One place the two kinds *do* behave alike, which is easy to
misattribute: `board_batch.py list` drops **every** milestoned issue by
default, delivery milestones included (`--include-milestoned` opts back
in). That default once kept a Todo *umbrella* tier clean; the umbrellas
folded into the Planning document on the review-inbox ratification, and
the default now exists to separate the inbox and the queue from the
parked lanes — which it does without implying a delivery milestone parks
anything. A hand-rolled read that skips the tool does not inherit it.

## The `Claude:` meta-work prefix

**Meta-work** is agent-infra change — work whose touched paths sit
**entirely** under `.claude/**`, `CLAUDE.md`, `docs/conventions/**`, or
`cfg/**`. Anything that also touches product / on-chain / SDK / frontend
code is **not** meta — including the shared build scripts under
`brand-assets/`, which are product-adjacent, not agent-infra.

**Why `cfg/**` is in the set.** It holds the lint hook config and the
cspell dictionary, which are what the agent material *drives*: a meta
batch that wires a guard hook or adds a spelling escape edits them
necessarily. Leaving them out made the definition exclude most real meta
batches — including the one that introduced this wording, which added a
hook to `cfg/pre-commit-lint.yml` and four words to `cfg/dictionary.txt`.
A definition that its own change fails is the wrong definition.

This does **not** contradict the audit registry, which files `cfg/**`
under `ci-infra`. The two answer different questions: the registry
assigns a path to the subsystem whose *failure modes* an audit lens
should reason about, while this list decides which Linear issues batch
together as agent-infra work. `cfg/pre-commit-lint.yml` is honestly both
— audited as CI config, filed as meta — and neither classification is
downstream of the other.

**One narrow allowance beyond that**, for the same reason: an
**incidental comment fix in product code**, where that comment names
agent material the batch just retired, does not disqualify. Retiring a
tool means scrubbing references to it, and a reference can sit in a
product file's header comment. The test is that the *substance* is
agent-infra — not that no product file is touched at all. A change with
real product content is not meta, however small.

Keep `META_BASES` in `.claude/tools/merge_tasks.py` in sync with this
list; that copy is what `is_meta_glob` reads when folding a legacy
body. Every meta-work Linear issue title
carries a leading **`Claude:`** token (capital C, colon, space) —
e.g. `Claude: Add a /merge-tasks skill` — so all agent-infra work
batches together and can be filtered, staged, and reviewed apart from
product code on the board.

- **Filing skills emit it.** `linear-task`, `audit-scope`,
  and `plan` prepend `Claude:` to a title when **every
  path the fix will edit** is on the meta surface above. `/merge-tasks`
  applies it when every issue it consolidates is meta. (`plan` matters
  here in particular: the scheduler is where most meta-work issues are
  actually filed.)
- **The judgement is the filer's, not a field's.** This used to be
  derived mechanically from the issue's `**Touches**:` globs; that field
  is retired (see "Structured filing fields" above), so the filer
  decides from what it already knows it is about to change. That is not
  a loosening: a filer composing an issue knows the surface it is
  describing, and the glob list was only ever that same knowledge
  written down one step earlier.
- **It batches meta-work on the board.** The prefix is the signal a
  human filters and groups by in Linear to see all agent-infra work at
  once, apart from product code. It is applied at **filing time**, so
  the prefix and the work's actual surface stay consistent by
  construction. No tool re-derives or re-checks the bucket; there is no
  rendered `# Claude` heading to keep in sync.
- **It is a Linear-title signal only — never a PR title.** The prefix
  lives on the **issue** title for board recognition and batching. PR
  titles keep the standard `type(ENG-###): Subject` semantic-pr format
  (see "Keep Linear tags out of PR bodies and comments" below for the
  title-scope rule); the `Claude:` token is **not** added to a PR
  title, where the conventional type and `ENG-###` scope already apply.

## The verb/substrate title token

**A Linear title says which verb and substrate the issue runs on**, as
a bracket token at its head: `[<Verb>/<Substrate>] <title>`. Verb is
`Task`, `Chore`, `Explore` or `Architect`; Substrate is `Bedrock` or
`Anthropic`. **The token is the issue's launch line**: lowercased and
followed by the issue number, it is what the operator (or a dispatch)
types. `[Chore/Anthropic]` on 1234 is `chore anthropic 1234`; an
architect thread titled `[Architect/Bedrock] Linear orchestration flow`
on its governing issue launches as `architect bedrock <n>`. The verb
table it reads against, with each verb's default substrate, is in
`local-integrations.md` → "The verb table".

- **One elision.** A title with no token is `Task` on `Bedrock`. Every
  other combination is written whole — a verb's own default included
  (`[Explore/Bedrock]`) — because defaults move, and a half-token
  would silently change meaning when one does.
- **It precedes `Claude:`**: `[Chore/Anthropic] Claude: Rotate the …`.
  The two are independent — one routes the launch, the other batches
  agent-infra work.
- **`[Operator]` takes no substrate.** It marks an item the operator
  executes by hand with no agent session, so it has no launch line.
  **An operator action is asked for when it binds**: the session that
  needs it asks in its own tab through `AskUserQuestion` (the scheduler
  for a Done that releases an edge or a login the gate needs, a worker
  for a ratification, the curator for a kill). One that cannot be done
  in the moment is filed at Urgent, Backlog, no milestone, and the
  curator surfaces it once in review: `[Operator]`, which lives in Todo,
  carried one per curator batch, until the operator's Done, or a chore
  when a session can do the surrounding
  work, which runs to In Review like any chore. There is no third
  path — nothing recites an operator-actions list.
- **A close never recites a captured operator action.** An operator
  action already held by an `[Operator]` task, a chore, or a ticked
  ask is never restated at close; one not yet captured is filed first
  (Urgent, Backlog, the curator messaged) and then named with its
  number as a smell — `uncaptured operator action, filed as N`. A
  session that cannot file names the action in full and says so —
  the one exception to the no-recital rule; silence is the only
  wrong answer.
- **The close question is the last word.** Nothing operator-facing
  follows the close yes: everything a session would have said there
  is filed or asked **before** the close question, and the only text
  after a yes is the close tool's own result (an exit-2 removal note
  included, since it is a result, not a reminder). A to-do that
  surfaces after the yes is a defect of the session's ordering. The
  operator reads Linear for what they must do, never a block of
  closing prose.
- **Every leftover has a disposition before the close.** Each
  warning, nit and "left for human review" item a session surfaced
  and did not do is **filed** as a follow-up (`linear-task`, the
  curator messaged), **handed** to a live session by message, or
  **dropped** with a reason, and the session does it itself: a close
  never ends by handing the operator a list to decide or file.
  "Noted in the PR body" is a record, not a disposition. The ledger
  is mechanical: `close_session.py --leftovers-file` takes it as a
  JSON array of `{"item", "disposition", "ref"}` (`ref` the
  `ENG-###`, the session — never the operator — or the reason; `[]`
  when nothing was left over), a Done or Canceled close refuses
  without one or with any item undisposed, before any write, and
  `review-pr` and `wrap` run it as a `--dry-run` before asking the
  close question. The order is load-bearing: once the
  close removes the worktree, guard wiring that predates "Wiring
  fails open" (`local-integrations.md`) blocks every Bash and file
  write, so nothing can be filed afterwards except through raw MCP.
  An In Review handoff keeps its leftovers on the live issue and
  needs no ledger.
- **A Linear-title signal only** — never a PR title, branch or worktree
  name, which keep their own forms.
- **The title is the authority.** The operator re-routes an issue by
  editing its token, and whoever launches types the line the token
  says. No other field records the routing, so nothing can disagree
  with it.

**The defaults are binding at launch, advisory at filing.** A
token-less title launches as `task <n>`, full stop. At filing, the
filer writes the verb's **default substrate** from the verb table
unless the issue states a reason to differ — web research (Bedrock has
no web search or fetch) or subscription budget — so the substrate judgment is
made once, in the table, rather than re-argued per issue.

**Who writes it.** `linear-task` asks for the verb and substrate and
omits the token for a default task; `plan` writes `[Explore/Bedrock]`
on an audit issue (`[Explore/Anthropic]` when the audit needs the
web), `[Chore/Bedrock]` on operator-attended agent work, and
`[Operator]` on an item the operator executes; `architect` writes it on
each task it files. Audit findings, trim batches and merge outputs are
default tasks and carry no token. **The dispatch gate reads it**:
planning types the line the token says, and refuses `task <n>` on an
issue whose token names another verb or `[Operator]`, and **the
launcher reads it too**: its title guard (`verb_check.py`) refuses a
new session whose launch line the token contradicts. A token-less
legacy issue is judged by its body until the one-time title sweep.

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
body/comments; `init-pr` seeds only the
`chore(<ENG-###>): Bootstrap the worktree` title + an empty body, so it
already complies.

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

The committed **zero-echo writers** have spellings of their own, and
they differ in whether the issue is a flag or a positional. Guessed
wrong, each call exits 2, usually on the fallback path after an MCP
failure, which is the worst moment for it:

| call                       | arguments                                      |
| -------------------------- | ---------------------------------------------- |
| `board_batch.py state`     | `--id ENG-###`, `--state <name>`               |
| `linear_issue.py append`   | `--id ENG-###`, one of `--file` / `--text`     |
| `linear_issue.py read`     | `ENG-###` (positional or `--id`), `--out`      |
| `linear_issue.py create`   | `--title` + `--body-file`, or `--body-dir`     |
| `linear_patch.py read`     | `ENG-###` (positional), `--out <file>`         |
| `linear_patch.py patch`    | `ENG-###` (positional), `--ops <file>`         |
| `linear_patch.py comment`  | `ENG-###` (positional), `--body <file>`        |
| `linear_gate.py resolve`   | comment id, optional resolving id (positional) |
| `linear_gate.py approvals` | `ENG-###` … (positional, one or more)          |

Every one takes `--dry-run` except the two `read`s and
`linear_gate.py approvals`, also a read. `board_batch.py fields`,
`priorities` and `edges` each take one write as flags in place of
the file (`--id` with `--set FIELD=VALUE` / `--priority N|name`;
`--blocker` with `--blocked`). `linear_gate.py` covers the
dispatch gate's calls the MCP cannot reach: `approvals` is the
gate's read half, exiting non-zero on any state other than Todo and
printing each issue's open-thread count as information only, never a
refusal (the gate itself is state Todo plus unblocked — see "Backlog
reviews, Todo pulls"); `resolve` runs `commentResolve`, the fold's last
call.

**Insertions include no separator of their own.** `append`, `prepend`,
`insert_before` and `insert_after` splice the text in *immediately*
adjacent to the anchor (or the end / start of the body), so the `text`
has to carry whatever blank line or indent the surrounding markdown
needs. An anchor that sits mid-line — the tail of a list item, say —
will otherwise leave the inserted text glued onto that line instead of
starting its own.

**With `replace_range`, the `to` anchor stays in place — never repeat it
in `new_string`.** The range is exclusive of `to`: only the text between
the anchors is replaced, and `to` itself survives. Restating it in the
replacement therefore stores it twice. One scheduler session did exactly
that and had to spend a full extra echo (≈8k) to remove the doubled
field token it stored.

**Two newlines, never one, before an appended heading or rule.** The
separator has to clear the previous paragraph, and the count that matters
is the one in the **stored** body — which may carry one fewer trailing
newline than your local copy, because Linear strips trailing whitespace
when it stores. Getting this one short puts a `---` directly under a
paragraph, which is setext heading syntax, so the round trip re-parses
that paragraph as an `##` heading. Observed twice in one session on real
issues, each costing a follow-up patch write to repair. The asymmetry is
what decides it: one newline too many is an invisible blank line, one too
few silently rewrites prose into a heading.

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

**Patch reduces what you SEND, never what you RECEIVE — so on a large
body, new narrative goes in a COMMENT.** This is the practical rule the
sentence above implies and which kept being missed: a comment is
additive and chronological, and creating one echoes **no body at all**.
Measured: ten saves against one large-bodied issue cost **~41.1k** in
echoes to add ~2k of new content, and one of those was a bare state
transition paying the full echo for a one-field write.

So split by what the write actually is:

- **Narrative** — a progress note, a disposition, a finding, a summary
  of what a session did → a **comment**.
- **Structure** — a checklist tick, a machine-parsed field, a
  supersession, an edit to the spec itself → a **body** edit, because a
  comment cannot change what the body says.

And for the automated paths, use the zero-echo writer rather than either:
`.claude/tools/linear_patch.py` applies patch ops, adds comments, and
makes state transitions while printing **one line**. See the carve-out
below.

### Carve-out: a high-volume automated pipeline may bypass the MCP

The rule above — body edits go through the MCP `patch` path — governs
**interactive filing and planning flows**, where a human is reading along
and the echo is the confirmation that the write landed as intended. It is
deliberately not a claim that the echo is unavoidable, and stating only
the rule left the trim-lever pipeline in contradiction with it.

For a pipeline that writes many small bodies unattended, the echo is pure
waste, and the lever is a raw-GraphQL tool that prints one line per
write. The trim-lever pipeline was the first such path: a per-lever
`append-evidence` that did its read-modify-write **inside the tool
process**, so a growing accumulator body never entered a transcript —
the compounding cost the section above measures at ≈53k over five
touches. That lane is retired (the analyst reads the digest); its
fingerprint `probe` survives in `.claude/tools/trim_levers.py`, and the
analyst files through `linear_issue.py create` / `append`.

Two conditions on using this carve-out: the write must be **automated and
repeated** (a one-off interactive edit stays on the MCP path, where the
echo is worth its cost), and the tool must **print enough to confirm the
write** — identifier and url — so the saving is in the body, not in the
audit trail.

**The general writer: `.claude/tools/linear_patch.py`.** The trim-lever
tool solved this for one pipeline; every skill that accumulates
structured findings paid the echo until it was generalized. This one
works against **any** issue:

```sh
python3 .claude/tools/linear_patch.py read  ENG-942 --out <scratch>/body.md
python3 .claude/tools/linear_patch.py patch ENG-942 --ops <scratch>/ops.json
python3 .claude/tools/linear_patch.py comment ENG-942 --body <scratch>/note.md
python3 .claude/tools/linear_patch.py state ENG-942 --state 'In Review'
```

The ops file is the same vocabulary as the MCP `patch` array (`append`,
`prepend`, `insert_before`, `insert_after`, `replace`, `replace_range`),
applied in order and atomically, capped at 50.

**The pre-read stays; only the echo goes.** Anchors must match the
stored text, so `patch` still fetches the body — but into its own
process, applying the ops there and printing a size delta. `read` spills
the body to a file for slicing with `read_result.py` rather than printing
it. Anchors carrying an `ENG-###` are refused up front, since Linear
stores those as mention nodes and the anchor can never match.

Named consumers: the `plan` and `merge-tasks` amendment paths, and any
apply-and-cancel flow — the accumulator shapes where the echo compounds
worst (44 saves for ~153.2k on one scheduler session, its eight largest
results all echoes of the same growing batch).

### Field-only writes go through `board_batch.py`, not the MCP

There is one way to make the echo vanish entirely, and it is to leave
the MCP. Linear's `issueUpdate` returns whatever the caller **selects**,
so a mutation selecting `success` alone returns a single boolean:

```sh
python3 .claude/tools/board_batch.py fields --updates <file>
```

Use it for **every non-body issue field** — priority, state, parent,
milestone, labels, assignee, and **title**. (Relations are not issue
fields; they are a separate mutation pair, so adding or removing a
blocking edge is the `edges` subcommand below, not `fields`. Passing a
relation key to `fields` is rejected.)

**`title` is on that list and did not used to be**, which mattered more
than it sounds: with `linear_issue.py` offering only `append` and
`find`, retitling had exactly one route — an MCP `save_issue` that
echoes the **entire stored body** back in order to write a short string.
That is structurally worst on precisely the issues most likely to need
it. Retitling the whole-pool fold task returned **116,512 characters**
and overflowed the tool-result cap, because a fold body is proportional
to the whole parked pool and the title is the field that needs
correcting once the body has outgrown the scope the original title
described — then cost a second call to verify, since a spilled echo is
not worth reading to confirm one string. Two notes on its shape: the
id-shaped refusal does **not** apply (a title is prose, so the
whitespace heuristic that protects `milestone` and `assignee` would
reject every real value), and unlike a milestone a title **cannot be
cleared**, so `null` or blank is refused rather than sent.

One scheduler
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

**ANCHORED body edits stay on the MCP `patch` path**, and that is a
deliberate boundary rather than an unfinished job. Linear's API has no
patch primitive — `description` is a whole string — so a Python
body-writer would have to fetch, apply locally, and write back
**wholesale**. For an anchored edit that reintroduces the round-trip
corruption hazard documented above, and it throws away the part worth
keeping: the MCP `patch` does anchor matching with **atomic abort on
ambiguity**, safety that has correctly refused writes whose anchor
matched two locations rather than guessing.

**An `append` is the exception, and the distinction is anchoring.** An
append has no anchor, so there is nothing to match ambiguously and
nothing for an atomic abort to protect — "add at the end" is well
defined against any stored body. So it may leave the MCP:

```sh
python3 .claude/tools/linear_issue.py append --id ENG-### --file <path>
```

That matters most exactly where the MCP costs most. The echo is a fixed
cost per call against the body **as it now stands**, and an append
*grows* that body — so each write echoes everything the previous ones
added, and the cost is **quadratic in the number of appends, not
linear**. "Fewer calls" is the usual answer and often has no purchase
here: one scheduler session made six appends to a single accumulating
issue as decisions arrived across hours, echoes growing to 6.2k, 6.9k,
7.1k and twice 7.7k — five of that session's seven largest single
results, all the same issue, roughly 35k combined. Each append recorded
something that had just arrived from another session, so deferring one
risked losing it to a restart.

Reach for the tool once a session's appends to one issue exceed two.
The retired trim-lever writer first solved this for one issue family by
doing the read-modify-write inside its own process; `linear_issue.py` is that
mechanism over an arbitrary issue, and its `find` subcommand is the
title-only projection the MCP does not offer — a compliant dedup probe
with `limit: 5` still returns five whole issue objects (~1.9k) to answer
a question the five titles answer alone.

**A FOLD is the second exception, and it is the primary path for one.**
A `merge-tasks` fold assembles its ops as a file, so passing them through
the MCP costs *two* body-sized transits — reading the ops file into
context, then the MCP's echo of the survivor's whole stored body. Both go
away by handing the path to the committed writer:

```sh
python3 .claude/tools/linear_patch.py patch --ops <path> ENG-###
```

`patch` is a subcommand, the identifier is positional, and `--ops` takes
the ops **file path** — the tool reads it in its own process, which is
what makes the fold zero-echo rather than merely fewer calls.

Measured on one fold: the ops file was **≈9.5k** (that session's
second-largest result) and the `save_issue` echo **≈10.4k** (its
largest), with 24 `save_issue` calls totalling 35.1k as the session's
costliest tool. A later fold's ops file reached **≈53k** — 148 KB of
folded bodies — where reading it to pass it through was not merely
expensive but infeasible.

**The safety argument survives the move**, which is why this is not a
carve-out from the paragraph above: the tool applies the same ops with
the same anchor matching and the same atomic abort, refusing the whole
sequence if any single op cannot be applied. It also accepts the MCP's
argument spellings (`old_string` / `new_string`, `from` / `to`) as
aliases, so an ops file assembled for either path applies unchanged —
which it did not always: a fold was once rejected on its first op with
`'replace' needs a string 'text'`, after the fold had been composed.

**A CHECKLIST TICK is the third exception**, on the same footing as the
fold: `linear_issue.py tick` keeps the refusal, in its own form — a box
text matches exactly, else by prefix, and must match as many boxes as it
is named, never sharing one with another text — and sends back the stored
body with only the box markers changed, so no text is rebuilt by hand. Its
state move (never out of Done / Canceled) rides the same write. Its
concurrency is `append`'s: an edit landing between its read and its write
is overwritten, since the API has no conditional write.

**And a fold's CANCELS are not body writes at all.** Canceling a
non-survivor through `save_issue` re-echoes its whole body to change one
enum — ≈6.0k for one cancel plus ≈6.1k for the `get_issue` before it, and
roughly **33k** for a survivor whose body had reached 130 KB. Cancel them
through `board_batch.py fields` in a single call. The `duplicateOf`
marker is dropped with the echo: it is not an issue field, and the
survivor's `# Part` headings already name every folded issue.

`edges` is covered under "Blocking relations" below — it executes an
operator's decision and is never called by automation.

### The echo budget is per issue, per **session** — not per skill

"Fewer calls" is easy to satisfy inside one skill and still lose, because
a single issue is touched by **two** skills across a worktree session:
`init-pr` reads the body and moves it to In Progress, `review-pr` moves
it to In Review at the handoff. On the MCP, each of those echoed the
entire body (both have since moved to zero-echo tools).

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

**The nominal budget is three writes on the worktree's own issue**: In
Progress at `init-pr` bootstrap, ticks-plus-state at `review-pr`, and In
Review at the merge-queue handoff. Stating the number is what makes an
overrun *detectable* — without one, a session can have the MCP writer as
its second-costliest tool while every individual skill step is
compliant, which is exactly how ≈20k went unremarked. `review-pr`
reports the count in its summary, so a fourth write is visible rather
than merely regrettable.

**Count writes and ECHOES separately — they are no longer the same
number.** Of those three, only the second echoes: the bootstrap moves
the state with `linear_patch.py state` and reads the body to a file, and
the In-Review transition goes through `board_batch.py state`. So the
nominal profile is three writes and **one** echo, dropping to zero on
an issue with no checklist (nothing to tick, so nothing to write at
step 3).

One fourth write is **sanctioned**: an issue rewritten mid-run by an
operator scope reframe. That recurs whenever direction changes during
implementation, and naming it here is what stops a legitimate reframe
reading as an overrun.

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

**The worst measured case was a scheduler session: 97 `save_issue` calls,
≈164k.** Its top five results were all late folds onto the same
aggregated survivor body. A burst of peer folds is the shape to watch —
several handoffs for one issue arriving within minutes, each taking its
own echo. Buffer them and write once.

**Consolidation has a RECURRING cost, not only a one-time saving.**
Because the echo floor is a function of **body size**, folding issues
trades a one-time merge saving for a per-session tax on every later
transition against the survivor — and the survivor's body is by
construction the largest. Measured: four unavoidable round-trips on one
consolidated issue cost **≈30k**, the biggest concentrated sink of that
run, with every individual call compliant — the bootstrap write, one
justified `get_issue` (a scheduler session had rewritten the body after
the snapshot), the checklist-tick write, and the In-Review write. The
documented lever is "fewer calls", and there were already only four. On
that body a **state-only** transition cost ≈7.6k.

Scoped honestly, because two of those four have since moved: the
In-Review transition now goes through `board_batch.py state` and body
appends through `linear_issue.py append`, both zero-echo. What remains
size-dependent is the anchored `patch` write and any `get_issue`. So the
tax is smaller than 30k today, and it still scales with the body.

Two consequences at fold time: a very large survivor body is a reason to
prefer `patch` over a full-body write, and to **skip any transition
whose state is already correct**. That second one is prescribed generally
elsewhere; a consolidated body is where it pays most.

**None of this argues against `/merge-tasks`.** Folding is correct, and
the one-task ruling makes it *more* common — which is exactly why the
recurring side belongs in writing rather than being rediscovered per
session.

**Relations and state belong in the CREATING call.** `blockedBy`,
`blocks`, `relatedTo`, `parentId` and `state` are all accepted at
creation, so a filing that sets any of them and then issues a second
`save_issue` to add them has bought a second full body echo for nothing.
One measured session filed a single issue in two writes for exactly this
— a create, then a follow-up purely to attach `blockedBy` (≈2k). There
is no ordering constraint to respect: file it complete.

#### The floor is structural — on the MCP path

This is the single most recurrent observation across mined sessions
(six entries), and most of it is **not** a trim target, which is why it
is recorded here rather than treated as a bug to fix:

- Two *mandated* state transitions on a large consolidated body cost two
  full echoes (≈1.8k–2.4k each) with **zero body sent**. Three sessions
  independently confirmed their writers were individually compliant with
  no within-skill lever left.
- Some pairs genuinely **cannot** be collapsed. One session's two tick
  writes were forced apart because a scheduler session appended mandate
  checkboxes *after* `init-pr` had read the body, so the second batch was
  invisible until the first write's echo returned it. Structural, not
  indiscipline.
- The accumulator cost **compounds rather than being flat per write**.
  Five touches on one issue measured ≈53k, and per-touch cost rose
  monotonically — 8.4k read, then 9.7k, 11.0k, 11.5k, 12.4k appends —
  because each append enlarged what the next would echo.
- **Durability can outrank the echo, and does.** One scheduler session
  measured ~25k on a single growing survivor and *rejected* the
  buffer-and-write-once fix for the operator-directed case, with reason:
  the parts arrived hours apart in separate turns, and incremental
  write-back is what makes an abrupt close lose nothing. Do not "fix"
  that one.

So: assume a floor of **two full echoes per issue per session** on a long
consolidated body — and read that as the floor **on the MCP path**,
which is the only place it still binds. The lever that actually removes
the cost is a write path that does not echo the body at all, and for
three shapes it now exists: non-body field writes go through
`board_batch.py`, body **appends** and title-only searches through
`linear_issue.py`, and the high-volume filing pipelines have their own
carve-out at the end of "Partial edits". What remains on the MCP, and is
therefore still floored, is the **anchored** body edit — where `patch`'s
ambiguity abort is worth what the echo costs.

Say that precisely rather than as a general claim about the API. An
earlier reading of this section concluded the only lever was upstream of
anything the repo could edit, which was true of the MCP path and false
of the API, and would have left this section asserting two incompatible
things once the tools landed.

**The old two-echo floor within `review-pr` is now ZERO, and that is a
correction rather than a nuance.** The reasoning that produced it still
holds — `review-pr` folds the In-Progress move into the box-tick write,
an issue with no checkboxes has nothing to fold it into, and the
In-Review move is gated on CI going green at a different point in the
flow so it cannot fold backwards — but it counted **writes**, and what
this section is about is **echoes**. The In-Review transition now goes
through `board_batch.py state`, which echoes nothing. So on a
checklist-free issue `review-pr` makes no echoing write at all, and on
one with a checklist it makes exactly one.

Do not re-derive the old pair as fixable waste; it was fixed. The
bootstrap write no longer echoes either: `init_pr_bootstrap.py` moves
the issue with `linear_patch.py state` and spills the body to a file
with `linear_patch.py read --out`, so the spec reaches context once,
as a file the session reads to plan and slices thereafter.

Two techniques worth keeping, both measured working:

- **Bulk checkbox ticking is three `patch` ops, not one per box.** A
  `replace_all` on the bare open-box prefix, plus one flip-back per box
  that must stay open, ticked **56 of 58** boxes in a single atomic
  anchor-based call.
- **Anchor on the prose *preceding* an `ENG-###` mention.** That
  successfully ticked a tag-bearing checkbox, avoiding the full-body
  rewrite the anchor rule below would otherwise force.

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

### Write-mangle rules for every body you file

Linear's writer rewrites some markdown on the way in, so these shapes
must never appear in a filed body:

- **No emphasis span may wrap a newline.** A bolded run crossing a line
  break stores garbled — `**a\nb**` comes back as `**a****\n****b**`.
  Close the emphasis on the line that opens it.
- **No span marker may cross an inline-code boundary.** This is the
  general rule the newline case is one instance of, and it was learned
  the expensive way: a strikethrough drawn across inline code stored
  **broken at every code-span boundary**, costing one ~3.1k pure-repair
  echo. Any span marker — `**`, `_`, `~~` — that opens outside a code
  span and closes inside one (or vice versa) will not survive the round
  trip.
- **Supersede prose by prefixing or quoting it, never by striking it
  through.** That is the practical consequence of the rule above, and it
  is the shape that actually comes up: marking a paragraph obsolete.
  Write `SUPERSEDED (date):` in front of it, or move it under a
  `>` quote, and leave the text unmarked.
- **No machine-parsed field may start with a bare hostname-valid
  `name.ext`.** It gets linkified. This is the fingerprint rule in
  "Structured filing fields", stated once more here because it binds
  any field a reader or a search is expected to match on.

Two things that **do** survive, confirmed by measurement and recorded so
they are not worked around unnecessarily: a **bold** patch anchor does
match on an issue body, and a checked box stores as an uppercase
`- [X]` (so anchor on that spelling, not `- [x]`).

A **wholesale** content replacement re-parses the entire body, so it is
where every rule here bites hardest — see the `plan` skill's close-out
step, which additionally composes without inline code spans for this
reason.

### The write floor assumes the body is read once

The per-issue floor above ("buffer folds; write them once") assumes a
session reads the body at the start and holds it. **Amendment breaks
that assumption**, and the resulting extra write is expected rather
than a lapse.

Concretely: when a scheduler session appends checkboxes to an issue an
implementation session is *already working from*, that session cannot
see them until its next echo returns them — so a second write it could
not have batched is structural, not indiscipline. Do not read the floor
as blaming a session for a cost it had no way to avoid.

**The avoidance is a message, not a tighter budget.** The
scheduler amending an in-flight issue should **tell the worktree session
directly** — the `plan` skill already has a coordinate-with-in-flight-
sessions step. That turns an invisible amendment into a message and
removes the wasted round trip entirely.

## Blocking relations

**No automated writer files a blocking edge — ever.** Not a filing
skill (`linear-task`, `audit-scope`, `trim-context`,
`merge-tasks`, `plan`),
not an autonomous audit rotation. This holds for edges an agent believes are
genuinely semantic, not just for file-overlap ones.

**Where they *are* placed: the scheduler.** A blocking edge
expresses semantic rollout ordering, which is a scheduling decision, so
it is made where somebody is actually deciding the order — the `plan`
skill's session in the base repo, human-directed, one edge at a time.
That is the whole of the exception: not "a skill that is allowed to",
but "the place a human does it". **Two other such places** carry the
same human-placed form:

- **An advisor session's filing ask** (`ratify-staged`): the operator's
  in-tab yes to a named chain of proposed edges, each with its reason,
  ratifies them with the tasks, and the session places exactly those —
  an edge the answer strikes is not placed.
- **The curator's in-tab tick** (`curate`): each proposed edge is one
  checkbox with its reason, and the curator places exactly the ticked
  ones. An unticked proposal is not placed, and not
  re-asked unless its evidence changes.

Each is the human-placed form executed, never an automated filer
deciding one.

**Two edge writers, one coordination rule.** Priorities have two
writers under the agreement rule ("The curator owns review" →
"Priorities: shared authority, the agreement rule"). Edges
have two writers: the scheduler and the curator both write them,
anywhere on the board, because each sees what the other lacks — the
scheduler the fleet and the dispatch order, the curator the specs it
has just read. A write that places, changes or removes an edge **the
other session placed** is
messaged to it in the same turn, with the reason; neither deletes the
other's edge without that message; and the Planning document records the result
at the scheduler's next write-back.

**A scheduler dispatch hold is an edge too** (operator ruling,
2026-10-08). The operator's Next view means *dispatchable right now*, so
an unblocked Todo issue the scheduler is holding behind an
in-flight surface or a sequencing reason is a lie in it: the operator
sees it, expects to launch it, and cannot see a hold recorded only in
the Planning document. So the scheduler expresses its own holds
as blocking edges on the in-flight blocker, and the held issue shows as
Blocked instead. This amends only *what a scheduler-placed edge may
mean* — semantic ordering **or** a scheduler hold. An automated filer
still never places an edge, and file overlap a filer notices is still
never one. A hold placed on the operator's yes, in the dispatch ask,
stays human-directed, and so does removing one: a completed blocker
stops blocking on its own, so removal is needed only on a re-sequence,
asked in the same dispatch ask.

**There is no standing edge class any more — the meta batch's edge is
retired, and so is the goal it served.** The edge existed to keep
exactly one meta issue unblocked at a time, so agent-infra work landed
one batch at a time rather than several sessions rewriting the same
skills at once. That goal was **abandoned** on 2026-09-11, not merely
re-implemented: several small meta batches may now be unblocked and in
flight together.

The reason is a cost measurement rather than a change of mind about
contention. Session cost is roughly quadratic in session length
(`docs/conventions/context-economy.md` → "Session length is itself a
cost lever"), so serializing meta work into one long-running batch buys
that quadratic in exchange for avoiding a **rebase** on the shared skill
files — and a rebase inside a short session costs cents. The contention
is real; it is simply the cheaper of the two costs.

What matters for *this* section is that neither the edge nor its
stand-in survives. **No relation** is placed on a batch, and nothing
gates one on another meta issue being In Progress or In Review. Meta
filings wait in the curator's pool, and Backlog alone keeps them out of
the pull queue without an edge.

**Where both states still matter is the fold, not a gate.** An In
Review meta issue is a session that has merged its code but still owes
follow-up, and the fleet-resume launcher treats it as live (it resumes on
state type `started`, which covers both). So a fold still declines to
swallow an issue in either state — the "never fold a live spec" rule.

*Superseded three times, and the direction is worth noting.* The serial
chain (2026-08-18) kept every open meta issue blocking the next, an edge
per issue and a long chain to maintain. The batch form (2026-08-20)
reached the same property with one edge. The parked-milestone form
(2026-09-02) reached it with **none**, using an assembly precondition
instead. The small-batch form (2026-09-11) drops the property itself, so
there is nothing left to reach — each step removed machinery rather than
adding it, which is the right direction for a mechanism whose whole risk
is a spurious edge dropping an issue out of the available set.

So this section now has **no** exception to state: a human still places
every edge — in the scheduler, through the curator's tick, or through
an advisor session's filing ask — and **no automated
writer places one at all**. A filing skill that notices a new meta issue
neither chains it nor queues it. Treat every edge,
semantic ones included, under the one-at-a-time rule above.

**The mechanism is `board_batch.py edges`**, and it changes none of the
above. There is no MCP path for relations at all, so the scheduler
needs *some* tool to execute the operator's decision:

```sh
python3 .claude/tools/board_batch.py edges --pairs <file>
```

Add `--remove` to delete the named edges instead — retiring an edge is
the same human decision in reverse, and it is the other half of what a
the scheduler does to the blocking graph. Rehearse either direction
with `--dry-run` first (accepted in either position): a blocking edge
drops an issue out of the operator's available set, so a wrong one
costs more than the rehearsal.

Read it as the human's hands, not as a new writer. It takes an
**explicit pair list**, has **no discovery mode**, and **refuses an
empty list** — so it cannot originate an edge, only carry one out. It
is never called by a filing skill or by any automated pass. With the
collision machinery retired, it is the **only** relation writer left in
the repo at all, and it writes nothing it was not handed.

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
one, and it is worth recording why it is gone: the retired collision
tool carried a `--demote` mode, a one-time migration that converted
pre-existing auto-filed `blocks` edges to `related` under explicit
confirmation. It ran on 2026-08-10, was **spent**, and was removed
before the tool itself was. Every candidate it could still have found
was a false positive: the legitimate hand-placed blocking edges collide
on files too, and no mechanical rule distinguishes them from artifacts,
so a second run would have deleted the intended blocking graph in one
command. Blocking-edge changes happen only where they always should
have: in the scheduler or the curator, human-directed, one at a time.

The mechanics, for when a human does ask for an edge: `save_issue`
takes `blockedBy` (the `ENG-###`s that must land first) and `blocks`
(the `ENG-###`s this one gates), both by identifier; they are
**append-only** — they add edges and never clear existing ones, so use
`removeBlockedBy` / `removeBlocks` to drop one. When the blocker is
filed in the same run, file it first so its `ENG-###` exists, then
reference it.

Two things that are **not** blocking edges:

- **File overlap.** Two issues that will edit the same code are
  reconciled by the scheduler and recorded as a cluster (see
  "Overlap is a cluster, never an ordering"); the automated
  related-linking is retired. Overlap becomes an edge only when the
  scheduler decides to *hold* one issue behind the other, as
  above — the decision is the edge, not the overlap.
- **Coupling that belongs in one PR.** That is handled by combining
  into a single issue (see "Fold coupled findings into one issue"),
  not a relation.

Since nothing automated writes an edge any more, every blocking edge on
the board *is* a human scheduling decision, and the blocked set *is* the
intended order of attack. Read it that way.
