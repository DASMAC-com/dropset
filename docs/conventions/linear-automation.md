<!-- cspell:word basenames -->

<!-- cspell:word strikethrough -->

# Linear automation

Skills that **file** Linear issues (`linear-task`, `audit`,
`audit-scope`, `trim-context`, `housekeeping`, `plan`) resolve the
filing destination — team, project, assignee — from **environment
variables**, never hard-coded UUIDs. (Skills that only **update**
an existing issue by id — `init-pr`, `review-pr` — need no
destination.) Set them once in your
shell profile (`~/.zshrc`):

```sh
export LINEAR_TEAM_ID=…
export LINEAR_PROJECT_ID=…
export LINEAR_ASSIGNEE_ID=…
# Used by the plan skill — the "Planning" document a planning
# session bootstraps from and writes its decisions back into:
export LINEAR_PLANNING_DOC_ID=…
# NOTE: LINEAR_API_KEY belongs here by rights — the Python board
# tools need it, because a script can't use the OAuth-based
# claude.ai Linear MCP and must authenticate with a personal key
# sent as the Authorization header. It is NOT set here, though: it
# is a secret, so it is resolved from 1Password at session launch.
# See local-integrations.md, "Session secrets".
#
# RETIRED: LINEAR_SESSION_METRICS_DOC_ID. The "Session Metrics"
# inbox document is gone — trim levers are parked issues under the
# `Trim levers` milestone now. See the trim-lever pipeline below.
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

**The trim-lever pipeline runs on a milestone, not a document.**
`session-metrics` is the **producer**: at session end it files each
trim lever as its own small **parked issue** — state `Todo` plus the
`Trim levers` project milestone, keyed by a `**Fingerprint**:` — and on
a fingerprint hit it **appends that session's evidence** to the lever
that already exists rather than filing a duplicate, so recurrence
becomes an accumulating fact on one issue. `trim-context` is the
**consumer**: it sweeps the milestone, folds the parked levers into
**one** propose-only `Claude:` Backlog task — always one, whatever the
lever count or surface spread, with one section per lever, each keeping
its own `**Fingerprint**:` line — and closes the parked originals. The
coherence floor still governs audit findings and product filings; the
meta-work fold is the named exemption (operator ruling, 2026-08-25).
A lever judged not
worth acting on is **closed with its reason**, which suppresses
refiling permanently. `housekeeping` drives `trim-context` as its
Session Metrics step; both skills also run standalone.

Every write in that pipeline goes through
`.claude/tools/trim_levers.py` rather than the MCP — see the carve-out
under "Partial edits" for why, and note the `Trim levers` milestone
must exist before the first `file` call (the tool refuses by name
rather than filing into the wrong place).

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
over what a planning session already knows: that session reads the
implementation specs into context anyway, so it judges overlap on
content rather than on exact glob matching. **Reconciling overlap is
planning-session work**, done from the ordinary board read — including
the merge-group proposals and the scheduling smells the tool used to
report. The `**Touches**:` field it consumed went with it — see
"Structured filing fields" below. Relations already on the board are
left alone as inert history: nothing deletes a relation.

## Structured filing fields

Every filed issue carries one machine-readable field the automation
reads back, on top of the human prose — `**Fingerprint**:`. It was two
until `**Touches**:` was retired (see below). Keep the field **name**
stable: the filing skills emit it and the dedup probes match on it.

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
direction is content-based judgement in a planning session — that
session is holding each issue's actual prose, which is strictly more
than a glob list tells it — not exact glob matching. If a future pass
wants declared scope back, that is a decision to take deliberately, not
a habit to drift into one filing at a time.

**Existing lines stay, and the fold tools still carry them.** Bodies
filed under the old convention keep their `**Touches**:` line as inert
history, exactly like the relations. `merge_tasks.py` and
`trim_levers.py` both emit a line only when given globs and tolerate
their absence by construction, so folding an old issue preserves
whatever it declared without requiring a new one to declare anything.

**Be honest about what the whole retirement costs.** Automated collision
recording was **dropped**, not replaced: a planning session's read is
the successor and its coverage is unmeasured. That is an accepted trade
— a spurious edge costs more than a missing one (see "Blocking
relations") — not a guarantee of equivalence. Losing the declared-scope
field widens that gap slightly, since there is no longer a cheap
cross-check on an issue whose prose quietly outgrew its stated scope.
Accepted on the same reasoning: the amendment rule (see the `plan`
skill, step 4) addresses scope drift at the source, and a field nothing
reads does not catch it either.

### Overlap is a cluster, never an ordering

When a planning session judges that two issues will edit the same code,
the conclusion it records is a **cluster** — the issues that collide on
one shared area — never an ordering. A cluster is the candidate set for
"these would land as one PR", which is what the merge-group proposal
step consumes (it also picks the parallelizable batch when promoting
parked audit findings).

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
(one per line, so per-finding dedup still matches).

**The coherence floor — do not fold across deploy units.** Never merge
findings a single PR can't sensibly review or land as a whole: different
apps, languages, or deploy units stay apart. A TUI Rust rendering fix, a
frontend TS hook fix, and an on-chain program refactor are **three** PRs,
not one — even though all three are "cleanup from the same rotation".
Fold *within* a coherent PR boundary; never across one. "Aggressive"
means minimize PR count up to that floor, not build an incoherent
mega-PR past it.

**The `Claude:` meta class has no size bound — operator-ratified,
2026-08-24.** Claude meta work aggregates into **one** batch issue
regardless of body size. The reasoning is scheduling, not tidiness: at
most one meta task ever runs in flight (they contend on the same skill
files), so a second meta issue buys no parallelism and costs a merge
conflict — and churn speed through the self-improvement loop matters
more than per-issue readability. So `merge-tasks`' oversized-survivor
warning carries an explicit **exemption for this class**, and the `plan`
skill's fold doctrine states it: one batch, one in flight, no size
bound. The split recommendation stays in force for **product** issues,
where it is about reviewability of work that genuinely can run in
parallel.

This exemption is about **size only**. The coherence floor above still
binds: meta work never folds together with product code.

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
  merge — session metrics, perms firming, post-merge tidy, feedback to
  hand to a planning session — and that work is invisible if the issue
  reads Done.
- **Done means operator-ratified complete.** Never merely merged.

**The team's Git workflow automation makes no state transition on
merge**, which states this reading natively: the **In Review** written
at `review-pr`'s enqueue handoff is simply the state the issue keeps
through the merge and after it. `review-pr`'s outcome-watch step runs
its follow-up tail and moves to Done only on an explicit
`AskUserQuestion` approval. Two consequences elsewhere:

- **`housekeeping` prunes on the issue's status TYPE, not on
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
document — that path is retired, along with `housekeeping`'s only
Planning-document read.

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
  places none, and this one is filed by a planning session, which
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

## Parked findings sit in **Todo**, never Backlog

An issue stamped with a parking milestone — `Audit findings` for audit
output, `Trim levers` for session trim levers — is **parked**: filed so
it is not lost, deliberately *not* in the pull queue. Backlog means
pullable, and the operator's "Next" view is the unblocked Backlog, so a
parked finding filed as Backlog surfaces as available work and has to be
moved by hand. One audit rotation filed fifteen findings that way in a
single pass.

So a filing skill sets **state `Todo` plus the milestone, in the creating
call** (per "Relations and state belong in the CREATING call"). Promotion
is then a planning-session act with two halves: **clear the milestone and
move Todo → Backlog**. Doing only one leaves the board lying about
whether the work is available.

Parked findings are also **exempt from the meta batch and its edge**
until they are promoted: that edge governs work which is queued, and
parked is not queued. Nothing about parking places or implies a blocking
edge — see "Blocking relations".

**This gives `Todo` two meanings, so every Todo read must say which it
wants.** Todo is both the board's *umbrella / initiative* tier (what a
planning bootstrap surfaces unprompted) and now the *parked* tier. The
discriminator is the **milestone**: an umbrella carries none, a parked
finding carries one. So a Todo read that means "the umbrellas" must
exclude milestone-carrying issues — otherwise one audit rotation's
findings (fifteen, in one measured pass) pollute the bootstrap read that
is supposed to show the standing tracks.

That already holds by construction, and it is worth knowing *why* rather
than rediscovering it: `board_batch.py list` drops milestoned issues
**by default** (`--include-milestoned` opts back in), and the `plan`
bootstrap says so where it reads the Todo set. Its step 8 then handles
parked findings deliberately and separately. The rule is written down
here because the *default* is what makes it true — a hand-rolled Todo
read that skips the tool would not inherit it.

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

- **Filing skills emit it.** `linear-task`, `audit`, `audit-scope`,
  `housekeeping`, and `plan` prepend `Claude:` to a title when **every
  path the fix will edit** is on the meta surface above. `/merge-tasks`
  applies it when every issue it consolidates is meta. (`plan` matters
  here in particular: a planning session is where most meta-work issues
  are actually filed.)
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

**With `replace_range`, the `to` anchor stays in place — never repeat it
in `new_string`.** The range is exclusive of `to`: only the text between
the anchors is replaced, and `to` itself survives. Restating it in the
replacement therefore stores it twice. One planning session did exactly
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
write. `.claude/tools/trim_levers.py` is that path for the session
trim-lever pipeline: `probe` / `file` / `append-evidence`, authenticating
with `LINEAR_API_KEY` exactly as the other Python board tools do, and
selecting only identity fields so no body is ever returned.
`append-evidence` does its read-modify-write **inside the tool process**,
so a growing accumulator body never enters a transcript at all — which is
the compounding cost the section above measures at ≈53k over five
touches.

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
worst (44 saves for ~153.2k on one planning session, its eight largest
results all echoes of the same growing batch).

### Field-only writes go through `board_batch.py`, not the MCP

There is one way to make the echo vanish entirely, and it is to leave
the MCP. Linear's `issueUpdate` returns whatever the caller **selects**,
so a mutation selecting `success` alone returns a single boolean:

```sh
python3 .claude/tools/board_batch.py fields --updates <file>
```

Use it for **every non-body issue field** — priority, state, parent,
milestone, labels, assignee. (Relations are not issue fields; they are a
separate mutation pair, so adding or removing a blocking edge is the
`edges` subcommand below, not `fields`. Passing a relation key to
`fields` is rejected.) One planning
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
here: one planning session made six appends to a single accumulating
issue as decisions arrived across hours, echoes growing to 6.2k, 6.9k,
7.1k and twice 7.7k — five of that session's seven largest single
results, all the same issue, roughly 35k combined. Each append recorded
something that had just arrived from another session, so deferring one
risked losing it to a restart.

Reach for the tool once a session's appends to one issue exceed two.
`trim_levers.py` already solved this for one issue family by doing the
read-modify-write inside its own process; `linear_issue.py` is that
mechanism over an arbitrary issue, and its `find` subcommand is the
title-only projection the MCP does not offer — a compliant dedup probe
with `limit: 5` still returns five whole issue objects (~1.9k) to answer
a question the five titles answer alone.

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

**The nominal budget is three writes on the worktree's own issue**: In
Progress at `init-pr` bootstrap, ticks-plus-state at `review-pr`, and In
Review at the merge-queue handoff. Stating the number is what makes an
overrun *detectable* — without one, a session can have the MCP writer as
its second-costliest tool while every individual skill step is
compliant, which is exactly how ≈20k went unremarked. `review-pr`
reports the count in its summary, so a fourth write is visible rather
than merely regrettable.

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

**The worst measured case was a planning session: 97 `save_issue` calls,
≈164k.** Its top five results were all late folds onto the same
aggregated survivor body. A burst of peer folds is the shape to watch —
several handoffs for one issue arriving within minutes, each taking its
own echo. Buffer them and write once.

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
  writes were forced apart because a planning session appended mandate
  checkboxes *after* `init-pr` had read the body, so the second batch was
  invisible until the first write's echo returned it. Structural, not
  indiscipline.
- The accumulator cost **compounds rather than being flat per write**.
  Five touches on one issue measured ≈53k, and per-touch cost rose
  monotonically — 8.4k read, then 9.7k, 11.0k, 11.5k, 12.4k appends —
  because each append enlarged what the next would echo.
- **Durability can outrank the echo, and does.** One planning session
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

**Two is the floor WITHIN `review-pr` for an issue with no checklist.**
Note the scope, because it is a different quantity from the three-write
budget above: that one counts the whole session including `init-pr`'s
bootstrap write, this one counts only `review-pr`'s own pair. They are
not in tension and neither supersedes the other.

The folding advice above quietly assumes a checklist exists. `review-pr`
folds the In-Progress move into the box-tick write — but an issue with
no checkboxes has nothing to fold it into, and the In-Review move is
gated on CI going green at a different point in the flow, so it cannot
fold backwards at all. Do not re-derive that pair as fixable waste.

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

**No automated writer files a blocking edge — ever.** Not a filing
skill (`linear-task`, `audit`, `audit-scope`, `trim-context`,
`housekeeping`, `merge-tasks`, `plan`),
not an autonomous audit rotation. This holds for edges an agent believes are
genuinely semantic, not just for file-overlap ones.

**Where they *are* placed: a planning session.** A blocking edge
expresses semantic rollout ordering, which is a scheduling decision, so
it is made where somebody is actually deciding the order — the `plan`
skill's session in the base repo, human-directed, one edge at a time.
That is the whole of the exception: not "a skill that is allowed to",
but "the place a human does it".

**One standing edge class, operator-ratified: the meta batch's edge.**
The goal has not changed — exactly one meta issue unblocked at a time,
so agent-infra work lands one batch at a time rather than several
sessions rewriting the same skills at once. The **shape** has: a
planning session now **folds** the open `Claude:`-prefixed issues into a
single batch issue at bootstrap (see the `plan` skill, step 1), and that
batch needs at most **one** edge — behind whatever meta issue is
currently **In Progress *or* In Review**.

**Both states, deliberately** — this is where the edge rule meets the
session-state convention above, and taking only In Progress leaves the
rule with no anchor exactly when it is needed. An In Review meta issue
is a session that has merged its code but still owes follow-up, and the
fleet-resume launcher already treats it as live (it resumes on state
type `started`, which covers both). The edge is about *scheduling the
next batch*, so it should key on the same "is a session still working
this?" question the rest of the convention keys on — not on whether the
code happens to have landed.

*Superseded (2026-08-18 → 2026-08-20): the serial chain.* The earlier
form kept every open meta issue blocking the next, which needed an edge
per issue and left the board carrying a long chain to maintain. The
batch form reaches the same one-at-a-time property with one edge, and
the fold is itself the bookkeeping.

Either way it is **routine bookkeeping**: a planning session places that
edge without a fresh per-edge proposal, because the operator ratified
the class rather than the instance.

This narrows nothing else. It is still the planning session placing it,
still one edge, and **automated filers still place no edges at all** —
a filing skill that notices a new meta issue does not chain it. Treat
any other edge, semantic ones included, under the one-at-a-time rule
above.

**The mechanism is `board_batch.py edges`**, and it changes none of the
above. There is no MCP path for relations at all, so a planning session
needs *some* tool to execute the operator's decision:

```sh
python3 .claude/tools/board_batch.py edges --pairs <file>
```

Add `--remove` to delete the named edges instead — retiring an edge is
the same human decision in reverse, and it is the other half of what a
planning session does to the blocking graph. Rehearse either direction
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
have: in a planning session, human-directed, one at a time.

The mechanics, for when a human does ask for an edge: `save_issue`
takes `blockedBy` (the `ENG-###`s that must land first) and `blocks`
(the `ENG-###`s this one gates), both by identifier; they are
**append-only** — they add edges and never clear existing ones, so use
`removeBlockedBy` / `removeBlocks` to drop one. When the blocker is
filed in the same run, file it first so its `ENG-###` exists, then
reference it.

Two things that are **not** blocking edges:

- **File overlap.** Two issues that will edit the same code are
  reconciled in a planning session and recorded as a cluster (see
  "Overlap is a cluster, never an ordering"); the automated
  related-linking is retired.
- **Coupling that belongs in one PR.** That is handled by combining
  into a single issue (see "Fold coupled findings into one issue"),
  not a relation.

Since nothing automated writes an edge any more, every blocking edge on
the board *is* a human scheduling decision, and the blocked set *is* the
intended order of attack. Read it that way.
