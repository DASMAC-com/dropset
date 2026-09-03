# Context economy

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

## Total spend and main-loop pressure are different costs

Before the levers: "cheaper" is ambiguous, and conflating the two
readings has produced a wrong call at least once.

**Total spend** is every token the turn burns, wherever it burns.
**Main-loop pressure** is only what lands in *this* transcript — and
that is what gets replayed on every later turn, occupies the finite
window, and eventually forces a compaction.

A sub-agent fan-out spends heavily in **throwaway contexts that
evaporate on completion**: the main loop sees only the findings. Doing
the same work inline spends *less in total* — no fan-out appears in the
rollup at all — while every byte of it lands in the main loop and is
replayed forever. So the inline version is cheaper in one sense and
strictly more exposed in the other, and the exposure compounds in
exactly the sessions that run long.

Say which one you mean. "It's cheaper inline" is true of total spend
and false of context pressure, and a decision resting on the
unqualified claim is resting on nothing. (`review-pr` step 5 is the
concrete case: its inline-lens fallback was removed, and the cost
argument was *not* the reason — see that step for why.)

## The levers

- **Ask for the narrowest thing that answers the question.** Use the
  narrowest method / subcommand, field-select where the transport
  allows it (`gh … --json <fields>`, a GraphQL projection), paginate
  instead of dumping, and **never re-fetch what's already in context**.

  This binds the **main loop's own discovery greps**, not just the
  sweeps it hoists for sub-agents — the rule is usually read as being
  about material handed onward, so it gets skipped for a search you run
  for yourself. When the question is *which files* reference a symbol,
  ask `--files-only` (or `grep -l`) and stop: one consumer-discovery
  sweep returned 39 full match lines for a question that was one bit per
  file. Take full lines only once you need to read the surrounding code.

  **Match the search shape to the question type, in every phase.** This
  is the single most recurring lever across mined sessions — seven of
  them answered a *location* or *existence* question with a full
  `--context N` sweep, one paying ~3.6k to find a three-line function.
  The rule already existed but was written into `review-pr`'s
  hoisted-grep step, so the implement phase read as exempt. It is not:

  1. **Locate** with `--files-only` (or `--glob <file>`) — one line per
     file, no bodies.
  1. **Then read** the one region you actually want, sliced.

  Take context lines only when the question is genuinely *what does
  this code do*, never when it is *where is it* or *does it exist*.

  **Narrow by SCOPE as well as by output form — they are separate
  axes.** The rule above narrows *what each match prints*; it says
  nothing about *how much tree gets searched*, and three sessions paid
  for the gap. One session's single largest result (~3.6k) was a
  repo-wide sweep for identifiers that were entirely frontend-local: it
  returned generated conformance vectors, committed wasm glue and a Rust
  example generator, and the very next call with `--dir frontend`
  answered the real question for a fraction. Another paid ~4.0k
  sweeping 11 frontend files for one function in one known area. When
  the claim is confined to one tree, pass `--dir <tree>` or
  `--glob <file>`; reserve the unscoped sweep for a claim that really is
  repo-wide.

  **`--context N` scales with match DENSITY, not match count.** Context
  windows around clustered matches overlap, so they approach buying the
  file outright. A 3-term alternation at `--context 3` over one
  ~100-line workflow returned 24 matches whose windows largely
  duplicated each other (~3.3k); a `--context 3` sweep hitting 21
  matches inside a single file bought that file roughly twice (~3.1k)
  *after* `--files-only` had already identified it. So `--context` is
  for **sparse** matches spread across files. When matches cluster in
  one file, take `--files-only` and then slice-read the region; when the
  target is a single named file, drop `--context` altogether and slice.
  `search_source.py` now says this in its own summary line when a
  context sweep spans more than a handful of files, or piles up in one —
  the discipline fails at the moment of typing, not the moment of
  reading this doc, so the reminder is attached to the result.

  **Once the scope is a single named file, the tool CLAMPS a wide
  `--context`.** A sweep over one file buys its matched regions at an
  N-line markup and, on clustered matches, at a *higher* price than
  reading the file. Measured: the three largest single results of one
  session were exactly this shape — a `--context 6` constants probe over
  one file (39 matches, overflowed the result cap, spilled 32KB to disk,
  three constants actually wanted), a `--context 12` struct probe, and a
  `--context 40` single-symbol probe that is a whole-file read with extra
  steps — ~11.9k together, roughly **60% of that session's entire Bash
  cost**. `search_source.py` detects a wildcard-free single-file scope
  from the *arguments* and narrows the window, saying so on its summary
  line; take `--files-only` and slice-read the region it names. It
  clamps rather than refusing because a refusal keys on **scope** while
  the cost is a function of **match count** — it rejected genuinely cheap
  calls, and an unanswered question just costs a second one.

  **Past a size threshold, any `--context` sweep degrades to
  `--files-only` by itself**, whatever its scope, printing the file list
  plus a note saying that it degraded and how to override. This covers
  what the single-file clamp structurally cannot, since the clamp keys on
  scope: the worst measured run was a `--context 2` sweep over one crate
  *directory* — 71 matches across 9 files, ~5.6k, roughly **40% of that
  session's entire Bash cost** — fired to answer a pure location
  question, and no scope rule would have caught it. `--force-context` is
  the escape hatch, for an adjudication read where the surrounding lines
  genuinely are the question.

  **`--force-context` lifts the size degrade and NEVER the single-file
  clamp**, and the asymmetry is deliberate rather than an oversight. The
  degrade fires on size alone, where the surrounding lines may really be
  the question and nothing substitutes for them. The clamp fires only
  once the caller has *already named one file* — and for that case a
  slice `Read` answers the same question strictly better, so an override
  would buy nothing but a way to pay more.

  So **a clamp is information, not an obstacle.** When it fires, it has
  told you the scope is one file; slice-read the region it names. Reaching
  for `--force-context` there does not work and should not: one session
  with the rule available, operating carefully, still fired two
  `--force-context --context 3` single-file sweeps in one run (≈2.6k and
  ≈2.1k, ~15% of its entire Bash cost) — both carrying the clustering
  advisory in the very result it then consumed, and both asked a pure
  *location* question. That signature — right rule, careful session,
  twice in one run — is why this one is enforced in the tool rather than
  restated here.

  **Treat the helper's advisory line as a DIRECTIVE, not a note.** When
  the summary reports clustering or a many-file spread, do not consume
  that result: re-issue `--files-only` (or add a `--glob`) and slice-read
  the named region. The detection already works — one session got the
  correct advisory on its top two sinks (~3.1k, 39% of its Bash cost) and
  consumed both results anyway. Obedience is the missing half, which is
  why both cases above were promoted out of advice and into the tool: the
  single-file case into a clamp, the size case into a degrade. What
  remains advisory is the shape neither threshold catches, and there the
  NOTE is a finding about the call you just made — not boilerplate, and
  not something to re-run in the same shape.

  **Enumeration is a third case, beside existence and adjudication.**
  The split above is location (`--files-only`) versus reading what code
  does (`--context`), and a real third shape fits neither: *retrieving
  several known blocks from one file*. That is a slice-read — one read
  spanning them, or a bounded read per block — never a grep, however
  tempting the single call looks. Measured: a `--context 3` sweep for
  this answered at ~2.0k, the run's largest single result, and **still**
  needed four separate slice reads afterwards, because the context width
  truncated the very bullets the sweep was meant to retrieve. It bought
  overlapping windows *and* the reads that replaced them. This is also
  the case the tool's own thresholds cannot save you from: they fire once
  the call is made, and here the right move is not to make it.

  **If an earlier call this session already named the file, pass
  `--glob`.** Narrowing the SCOPE is a separate axis from narrowing the
  output, and this one has a concrete trigger: you already know where it
  is. Measured in one session, two of its three largest Bash results were
  unscoped sweeps fired when the target was already known — ~3.7k for a
  context sweep over four alternated identifiers to settle a *one-bit*
  question, returning 31 matches across 4 files that were mostly test
  assertions when two regions were wanted; and ~1.8k sweeping a whole
  crate for one config field, when a section map earlier in the same
  session had already named the file. The same tool scoped to one file
  cost ~200–500 tokens per call elsewhere in that run.

  **When you already know where a symbol is DEFINED and are sweeping for
  its consumers, exclude the defining file.** The definition is the one
  match you are guaranteed not to need, and it is usually still in
  context from the read that raised the question — so a `--context`
  sweep re-buys it. Measured: a session that had just read a constant's
  definition swept the identifier with `--context 12`, its largest
  `search_source` call at ~1.5k, and roughly half the payload was the
  definition site it had read moments earlier. The question was solely
  how the caller passes the value.

  This is a third axis, distinct from both the scope rule above and the
  output-width rule: the sweep here was *already* narrow — two files,
  both genuine hits — and the tool's advisory correctly did not fire,
  since four matches across two files is neither clustered nor spread.
  What was wasteful was one **specific known file**, which no
  density heuristic can know is redundant.

  **A structure map matches TOP-LEVEL declarations only.** Anchor at
  column zero and never add a leading-space alternative (`^ *fn`,
  `^ *pub fn`): in Rust that turns the map into a dump of the
  `#[cfg(test)]` module, which is routinely more than half the file. One
  correctly-`--glob`-scoped map over a ~2140-line file returned **97
  matches at ~1.9k** — roughly 80 of them test-module functions from a
  block spanning lines ~810–2140 — to choose which regions of a ~17-line
  type surface to read. `^pub enum|^pub struct|^impl` alone would have
  cost about a fifth. The scope was right and the output width was wrong,
  which is exactly why `--glob` did not save it. This is the same failure
  as the comment-alternation case below, but that one names comment lines
  specifically, so it does not fire here.

  **Before the third slice of one file, sum what you have already
  read.** Past roughly half the file, take one bounded read covering the
  remaining regions instead of continuing to slice. The accumulating case
  looks compliant at every individual step, which is why it needs its own
  trigger: one session ran the prescribed declaration map over a ~650-line
  `Makefile` and then sliced it **five** times off that map — 225 lines,
  98, 50, 26, 14, about 413 lines in total, the first slice alone its
  largest single result at ~3.3k. Both adjacent rules were followed and
  neither applies: "a planned multi-region read is ONE bounded read"
  governs regions planned up front, and these were discovered
  incrementally; "if you already ran the map, slice from it" fires when a
  map is followed by a *whole-file* read, not by many slices. Be honest
  about the size of the win — a whole read of that file is ~6k, so five
  slices at ~413 lines were not obviously worse in raw tokens. The
  stronger case is fewer round trips.

  **An identifier that names something on both sides of the chain
  boundary spans two domains.** At least `leader`, `market` and `vault`
  each name a host-side Rust binding *and* an on-chain account or field
  — `leader` worst of all, being a `Keypair` in the bot, a `Pubkey` on
  vault state, and a parameter name in the chain helpers. A host-side
  sweep for the maker bot's leader keypair returned **92 matches across
  36 files**, mostly on-chain state unrelated to the signing key; the
  next call, anchored to the host-side use, answered it in one file.
  Anchor to the use (`ctx.leader`) or scope with `--dir` to the crate.
  This is pattern precision, a separate axis from output width.

  **A whole-issue read is for an issue's CONTENT; a decision that turns
  on a field is a field-selected `list_issues`.** Reading an issue whole
  to learn one enum cost ~6.0k in one measured case, and a second
  **overflowed the tool-result cap** (64.3KB, spilled to disk) so the
  field was not even readable — then a recovery `list_issues` with
  `fields: ["title", "status", "statusType"]` answered it for ~2.5k. That
  is ~8.5k plus a disk spill to classify two issues, and the failure
  scales the wrong way: the longer an issue's decision history, the
  likelier a whole read overflows, and long-running issues are exactly
  the ones that reach a merged PR.

  **A pattern you have not searched before starts `--files-only`.** The
  advisory is post-hoc by construction — it can only arrive with a
  payload already paid for — so the first call needs a rule of its own,
  and this is it: locate first, earn context on a narrowed second call.
  Five of one session's seven largest results were context-bearing
  sweeps (~7.4k of a ~25k session), and the instructive one was purely
  locational. This does **not** override the location-vs-adjudication
  distinction: the same session's adjudication sweeps took context and
  were right to, finding three real stale-comment defects.

  **Enumeration-for-edit is a THIRD case, and it takes
  `--files-only`.** A sweep whose purpose is to enumerate the sites of a
  rename or a removal produces a **work list** you will open one by one —
  so the context it buys is redundant by construction: every site
  returned is a file you are about to open and edit anyway. Measured: a
  removal sweep with `--context 4` returned 8 matches across 7 files at
  ~1.7k, the session's fourth-largest result; the tool printed its own
  advisory recommending `--files-only`, the result was consumed anyway,
  and all seven files were then opened and edited — so the payload was
  bought twice.

  The reason the two-case split kept missing this is that a rename
  genuinely *feels* like adjudication, so the rule read as licensing
  context and the advisory read as a false positive. The distinction is
  **when** you need to see the code: for an enumeration you need each
  site *while editing it*, at which point the file is open; for a true
  adjudication you need it *to decide whether to act at all*, and you may
  never open the file. Only the second earns context in the sweep. This
  also removes the standing reason to override the tool's advisory,
  which fires correctly on rename sweeps — and overriding it there
  trains the habit of overriding it everywhere.

  **Verify a list-producing flag with a count, not the list.** One
  session's largest single result (~5.8k, ~35% of its Bash cost) was a
  new tool's `--print` dumping ~600 repo paths to answer the yes/no
  question "did the flag work". Pipe to a count, or check one line.

- **Read large known files by slice.** Grep to locate, then `Read`
  with `offset`/`limit`; don't pull a 1000-line file to use 80 lines
  of it. This is **main-loop** discipline during a study phase too, not
  just a sub-agent rule: when you only need a few symbols from a
  reference or generated file, Grep to them first and slice-read — and
  skip a file's trailing `#[cfg(test)]` / `mod tests` when you need its
  API, not its tests. Whole-file `Read` is consistently the single
  largest token sink across review/build sessions. Brief review
  sub-agents to do the same. Families this bites repeatedly:

  - **Codama-generated SDK instruction files** (`sdk/rs/src/generated/**`,
    e.g. `set_reference_price.rs`, `set_liquidity_profile.rs`). To wire
    a CPI you need only the `Accounts` struct and the `InstructionArgs`
    fields; the CPI-`Builder` bulk below them is ~80% of the file and
    usually unread. Grep to `InstructionArgs` (and the accounts struct),
    then `Read` that slice — don't pull the whole ~4k-line-equivalent
    file.
  - **A config / workflow file read for one narrow question.** Grep to
    the block that answers it and slice-read that, not the whole file —
    e.g. to check whether CI's path filter excludes a file, Grep
    `.github/workflows/test.yml` to the `code:` / `predicate-quantifier`
    block (~20 lines) rather than reading all ~4k of it.
  - **A large test-fixture file** (e.g. a 1600-line `fixture.rs`) opened
    for a few helpers. Grep to the helpers you need (the `poke_*`
    builders, a specific `fn`) and slice-read those, rather than
    paginating the whole fixture.
  - **A sibling skill or convention doc.** This rule reads as being about
    large *source* files, but the files a mid-session handoff actually
    reaches for are skill and convention docs — several of which run past
    1800 lines. One two-line copy PR spent ~93% of its whole Read cost on
    two `SKILL.md` files, one of them read whole, when all it needed was
    the title/description format section. Grep the doc's headings
    (`^#`), then slice-read the step you want.

  **Ask who else will use the content, not how big the file is.** The
  license below is about **amortization**, and stating it as a list of
  conditions makes it read as being about *large* files — so a small one
  passes an imagined size test and the license itself is never run. A
  192-line whole read for a two-line edit is proportionally worse than a
  600-line read that five lens briefs then reuse. Measured in one
  session beside two correctly-licensed reads: a 192-line module read
  whole (~2.3k) for one call site plus one import, never briefed to any
  agent — and a sweep minutes earlier had already named the exact line.
  So before reading whole, **name who else will use this content**; if
  the answer is "only this edit", slice, whatever the size. The
  operational corollary: **if a search has already named the line you
  are about to edit, slice from that line** — that sweep was a section
  map, and it counts as one even though it came from
  `search_source.py` rather than a deliberate structure grep.

  **Four sufficient conditions for reading a file whole.** Any one of
  them is enough on its own — they are alternatives, not a single
  conjunction. This used to read as one condition ("only when you will
  BOTH edit the file and brief agents on it") sitting beside a sibling
  rule that favoured exactly what it forbade, and neither cited the
  other. Four sessions landed in that gap and each resolved it by
  guessing: one whole-read of a workflow touched six separate regions
  with no fan-out at all, another read two substantially-rewritten files
  whole where slicing would have cost four separate reads, a third read
  one exemplar whole in order to write three new files in its shape.
  All three were right, and the doc said all three were violations.

  1. **You will BOTH edit the file and brief agents on it.** The whole
     file is cheaper *overall* here. One session's top five main-loop
     results were whole-file `Read`s (~23k) of the crate it was about to
     modify — and those same excerpts then went inline into all five
     review-lens briefs, which is what held every lens under its turn
     cap. Paid once, amortized five times. Absent that second use,
     slice.

  1. **You have planned a multi-region read, and the regions add up to
     most of the file.** Slicing is only cheaper when you are reading
     *less*. One run read `swap.rs` across four separate slices
     totalling **more** than a single whole-file read; another spent a
     whole-file `Read` (~4.4k) on a dispatcher to find one append point.
     Decide the regions first, then pick — but a planned multi-region
     read is ONE bounded read, never several.

  1. **The file is an exemplar you are about to imitate N times.**
     Reading one file whole to write three new files in its shape
     amortizes across the N outputs rather than across a fan-out, which
     is a different denominator the first condition does not cover. Two
     or more imitations is enough; for exactly one, slice.

  1. **You are proving an ABSENCE, and the absence is the answer.** A
     negative claim about a file cannot be established from a slice: a
     slice shows a thing missing from the lines you read, never from the
     file. Grepping for it only proves you guessed every spelling the
     absence could take. One session read a 118-line workflow whole
     (~1.6k) to establish that a job carries no paths-filter, no
     `needs:` gate and no `if:` condition, and therefore that a new hook
     in it reaches the merge queue — not sliceable, and the read was the
     cheapest sound way to it.

     Two guards ship with this one, because it is the easiest to
     over-claim. It licenses the read only when the absence **is the
     answer being reported**, not when it is a passing assumption; and
     only for a **bounded** file — a negative about a
     multi-thousand-line file wants a tool or a structured query, not a
     read. Say in the report that the read was for a negative, so the
     claim and its evidence travel together.

  And two clarifications that do *not* authorize reading whole:

  - **Citing a file is not a use.** Condition 1 asks for an edit **and**
    a brief; naming the file in prose satisfies neither, and it is the
    reading that keeps getting stretched to cover it. One session read a
    230-line module whole (~2.6k) under the edit-plus-brief license when
    neither happened — the file was only *cited by name*, and roughly
    forty percent of it is test code. A grep to the signatures being
    cited does the whole job. This is deliberately distinct from the
    rejected argument that defended genuinely amortized whole reads
    (edited **and** quoted into five briefs); that rejection stands, and
    this tightens the same boundary from the other side.

  - **"Reading 3+ files to orient" is the trigger for slicing, not an
    exception to it.** Survey-time whole-file reads were the single
    largest sink of one session (top five, ~15k). The crate was small,
    so no per-file budget felt warranted — yet `model.rs` is ~40%
    `#[cfg(test)]` and only two signatures were needed. Grep for the
    structure first — the language's **top-level declaration** shape
    (`^pub enum|^pub struct|^impl`, or its equivalent), anchored at
    column zero and never with a leading-space alternative; the map
    tells you which slice you actually want.

    **Trigger that on the QUESTION you are asking, not the line
    count.** Written as a size threshold ("any `Read` over ~300 lines")
    it becomes a judgement call exactly at the boundary, and it silently
    exempts the files most often read to *learn a convention*. Measured:
    a **310-line** module — one line past the threshold, and reading as
    a small file — was read whole at **~3.4k**, that session's single
    largest result of any kind, when what was needed was three regions
    totalling ~60 lines and none of the whole-read conditions applied.
    The same session sliced correctly whenever the purpose *was*
    editing, which is what identifies the culprit: the size trigger is
    not what failed, the purpose framing is. **Reading to learn** —
    types, conventions, an exemplar you will not imitate N times — is
    always a slice, at any size. Keep ~300 lines as a secondary
    backstop for edit-shaped reads.

    **Scope that structure-map grep to the file(s) you are about to
    read.** The instruction names a pattern but no scope, and aimed at
    the whole source set it *becomes* the sink: an
    `^export|^function|^const` probe returned 747 matches across 75
    files, and that sweep was one session's single largest result
    (~4.5k) — fired to map the structure of two files it had already
    identified, and answering nothing the run went on to use. Pass
    `--glob <the file>`; the map you want is of the file you are opening,
    not of the repo.

    **Match the declaration shape only — and on a prose file, that means
    the heading marker alone.** A section map's whole value is that it is
    much smaller than the file, so an alternation whose widest branch
    matches ordinary content destroys the point. Two measured shapes:
    a `Makefile` map spelled `^[a-zA-Z0-9_-]*:|^# |^##` returned every
    comment line in a file that is mostly prose comments (**~5.4k**, that
    session's single largest result, to answer "where are the rules");
    and a map over a 1,275-line doc that matched headings **and every
    table row** returned ~2.6k for a where-question two headings
    answered — the widest branch bought the file's entire tabular
    content. For a Makefile match `^[a-zA-Z0-9_-]+:`; for a prose doc
    match the heading marker (`^#`) and nothing else; the committed
    `read_result.py --headings` already does exactly this. The general
    form, which is the part worth carrying: **ask what the widest branch
    of your alternation matches on its own.** If you genuinely want the
    prose headings *and* the declarations, ask for them as two narrow
    queries.

    **On a RECORDS-shaped data file, match one field, not an
    alternation — even when every branch is a legitimate
    declaration.** The rule above catches a branch that matches ordinary
    *content*; this is the case where none of them do and the map is
    still several times too big, because in a file of like objects each
    branch fires **once per record** rather than once per section. The
    branches do not partition the file, they multiply it: three branches
    over N records return 3N lines to answer what one branch answers.
    Measured: a 394-line JSON array of stablecoins mapped with
    `'"symbol"\|"mint"\|"name"'` returned ~88 lines at **~1.0k**, that
    session's largest Bash result, where `'"symbol"'` alone returns ~25
    and answers the same navigational question — the mint and name
    values were never used from the map and both came from the slice
    that followed. Pick the single field that **identifies** a record,
    and take everything else from the slice.

- **Don't read a file you are about to delete, or one you just
  authored.** Two cases adjacent to "never re-fetch what's already in
  context" — the existing rule warns against re-reading a file to
  *verify an edit*; these are the two neighbors it doesn't name:

  - **The file is being deleted.** A whole-file `Read` (≈2.8k) of a hook
    the diff removes outright bought a body no one would edit. What a
    deletion needs is the *exported symbol names* other modules import,
    so the callers can be found — Grep for those and skip the body.
  - **The file was just authored in this session.** Re-reading a file you
    wrote a few turns ago, to recover exact strings for `Edit` anchors,
    re-buys content already in context (one run spent two calls and ~110
    lines doing exactly that). Anchor from what you wrote; if the stored
    text is genuinely uncertain, slice the one region.

- **The narrowest-form rule covers listing and blob commands too, not
  just greps.** Four instances across three sessions, all the same
  shape: an existence, location, or single-field question answered with
  a full listing or a whole blob. The rule reads as being about search,
  so these slipped past it.

  - **`git show <ref>:<path>` prints the whole blob** (~3.8k) for what
    was meant as a metadata peek. `--no-patch` does not help — it
    suppresses a *diff*, not a blob dump. Grep at the ref, or use
    `--stat`.
  - **`git ls-files '*.json' -- sdk idl target`** returned ~2.2k to
    locate one known file. `git ls-files sdk/idl` was the question.
  - **`gh cache list --limit 30`** cost ~1.1k and answered nothing;
    querying by exact key cost ~200 tokens **and** returned a field the
    listing does not carry. When the question names a key, query by key
    — a listing is for when you do not know the key.
  - **`mcp__github__get_latest_release`** returned 60,413 characters,
    overflowed the tool-result cap and had to be redone: release
    payloads embed every asset object. For a version or tag lookup use
    the field-selected `gh api` form — see
    `docs/conventions/github-mcp.md`, which now names single-scalar
    lookups as a documented exception to the all-through-MCP rule.

- **When a change renumbers or renames a referenced identifier, write
  the annotate script first — don't open with a pattern sweep.** One
  session put 38 greps (~8.2k) into sweeping page cross-references
  during a slide renumber, and the sweep was structurally wrong-shaped
  twice over: a line-oriented grep cannot see a reference that wraps a
  line break, and it silently misses a stale number that is the *second*
  entry in a list. Both classes escaped the manual sweep and were caught
  only by a scratchpad script that joined the file and annotated each
  reference with its target's title. The lever is **ordering**, not
  volume: the failure mode is a reference that still matches the pattern
  while pointing at the wrong thing, which no pattern sweep can detect,
  so the exploratory grep round buys nothing you can trust.

- **When the question is "what top-level things exist", list one level.**
  `ls -1R sdk/` returned every generated client file (≈1.4k) to settle
  whether `sdk/rs` was a plausible home for one function — a question the
  top level alone answered. Reach for `ls -1` (or a depth-bounded Glob)
  and recurse only into the one subtree that turns out to matter.

- **Reach for the Grep tool first, and hoist a repeated sweep into one
  call.** `grep` has been the single most-repeated Bash shape in four
  consecutive sessions (×26, ×29, ×63, ×54) — almost all of them
  one-off single-symbol lookups issued in the shell while the Grep tool
  was available, and almost none of them reused. Grep is the cheaper
  transport (it honors gitignore, so it never hands back a match from
  build output), and it is the default even for a lookup you only make
  once. When a reference sweep genuinely does recur — the same symbol
  across a subsystem, the same rule across the convention docs — run it
  **once** and reuse the result set instead of re-issuing a variant per
  question; that hoisted result is also what a sub-agent brief should
  carry rather than the instruction to sweep. The bare-`grep` fallback,
  for when the Grep tool isn't present, and the scope it must be held
  to are in `docs/conventions/shell-commands.md`.

  **Scope a hoisted sweep to non-generated paths.** Hoisting is correct
  and *unscoped* hoisting is expensive: one session's largest single
  main-loop result was exactly the right sweep, returning the whole
  regenerated SDK surface (a 658-line generated instruction file) no
  reader needed. Use the committed wrapper, which prunes the generated
  families and the never-search trees (`grep -r` does not honor
  gitignore, and `target/` alone is multi-GB) and reduces to one stable
  allow-rule however the pattern varies:

  ```sh
  python3 .claude/tools/search_source.py '<pattern>' --context 2
  ```

  It shares its exclude lists with `review_diff.py`, so there is one
  owner rather than a set re-derived per run;
  `review_diff.py --print-grep-excludes` prints them as `grep` flags for
  the bare-`grep` fallback. The wrapper also **states its truncation**
  when a cap trims the output — a silent cap reads as "searched
  everything", which is worse than no search.

  **Ask for a sweep's narrowest form, too.** Scoping bounds *where* a
  search looks; narrowness bounds *what it hands back*. A correctly
  hoisted "are each of these 7 moved symbols still referenced?" sweep
  came back as ~130 full match lines (≈4.2k, that session's single
  largest result), most of them one file repeating one constant 40 times
  — for a question that is one bit per symbol. Use `-l` (files) or `-c`
  (counts) when the question is existence, and full `-n` lines only when
  the surrounding code actually has to be read. Hoisting a *verbose*
  sweep merely relocates the sink from a sub-agent into the main loop,
  where it is replayed on every later turn.

- **Query an indexing MCP before grepping a vendored dependency
  checkout — and never with a wide `-A` window.** Answering "does this
  framework support optional accounts, and how is `None` encoded" cost
  ≈6.2k + ≈2.9k across two context-padded sweeps of
  `~/.cargo/git/checkouts`, the larger being that session's single
  biggest result — while four `search_code_advanced` calls against the
  same dependency's MCP answered adjacent questions for **1.3k total**.
  Where an MCP indexes the dependency, it is the cheaper transport by an
  order of magnitude.

- **Once you know the page, read it rather than searching for it.** The
  same principle inside an MCP: one broad documentation `search` returned
  ~89k characters (large enough to be persisted to disk) and another
  ≈5.6k, for facts that amounted to two table rows. A `search` is for
  *locating* a page; a targeted read (`query_docs_filesystem`,
  `read_documentation`, `read_sections`) is for taking what is on it.

- **A `list_issues`-style call is a titles-only call, and you will pay
  twice.** Listing 9 children of a parent cost 3.3k and returned every
  description **truncated anyway**, after which the one body actually
  needed still took its own `get_issue`. When the target id is already
  known, skip the list entirely; when genuinely scanning, budget for the
  follow-up fetch rather than hoping the rows will suffice. Each Linear
  echo is a fixed cost per call, and the budget it belongs to is stated
  in `docs/conventions/linear-automation.md`.

  **Field-select it, always — the `fields` argument exists.** For a
  dedup scan or a board read, `identifier`, `title` and `priority`
  suffice; nothing else is looked at. An unfiltered `list_issues`
  measured **~11k per call**, twice in one session, against **~600
  tokens** for the same Backlog listed compactly — an 18-fold difference
  on a call the planning and filing skills make every pass. Better
  still, for a whole-board read use the tool that does this by
  construction:

  ```sh
  python3 .claude/tools/board_batch.py list
  ```

  **For the dedup question specifically, probe the fingerprint — don't
  read bodies.** A fingerprint search answers "does an issue already
  cover this?" at roughly **2% of the cost** of reading the body (~200
  tokens against 8.4k) and answers it *more* reliably than skimming a
  long issue. Two sessions instead paid ~5.6k and ~3.0k for dedup
  searches that returned full issue objects — truncated descriptions,
  project / team / assignee ids, timestamps — to settle a yes/no. Cap
  `limit` at what a human would actually scan (~5), request the
  narrowest field set, and reserve the body read for a decision that
  genuinely turns on surrounding text.

- **Never `Read` a harness-persisted tool result whole — extract the one
  field.** When a result exceeds the inline cap the harness writes it to
  disk and shows a preview, which is a *saving*; reading that file back
  whole re-buys everything the spill saved, envelope and metadata
  included. One session's single largest result (≈13.7k) was exactly
  this: a `Read` of the JSON the Linear MCP had spilled, when only the
  `description` field was wanted out of a 53KB envelope. This is the same
  "never re-fetch what's already in context" rule as above, applied to a
  payload the *session itself* produced — which is why it gets missed.

  **Use the committed slice-reader rather than re-authoring one.** Two
  sessions mined an oversized spill with hand-written grep-and-slice
  scratchpad scripts before the shape was extracted; the second wrote
  essentially the same script from scratch. It now exists:

  ```sh
  # Navigate first — a 40-part body becomes a few hundred bytes of map.
  python3 .claude/tools/read_result.py <file> --field description --headings
  # Then take only the part you want.
  python3 .claude/tools/read_result.py <file> --field description \
      --section 'Part 24'
  ```

  `--field` walks the MCP envelope for you, and `--grep` / `--slice` /
  `--count` cover the rest. `--diff <older-file>` answers "what changed
  since I last read this" against a previous spill of the same object —
  worth knowing because re-reading an amended body otherwise costs as
  much as the first read did. One live use of it reported an amendment in
  **11 diff lines** instead of a second 52KB read.

- **Take a file's structure map once, then keep it.** A section map
  (`grep -n '^fn \|^impl '`, or `^#` for a doc) is cheap, but re-deriving
  it on every edit is not: one session re-grepped the same file's map
  several times as editing moved through it. When a file will be edited
  in more than two places, map it once up front and work from that map.

- **Route verbose build logs away from context.** Prefer `-q` /
  `--quiet` so a `cargo` / `make` "Compiling …" cascade doesn't land
  inline. For a noisy target with no quiet flag, run it through the
  quiet runner, `python3 .claude/tools/run_quiet.py -- CMD ARGS…`
  (with optional `--tail N` / `--label L`): it captures the output to a
  temp log and prints only a one-line summary on success, or — on
  failure — an index of every `…Failed` hook-result line found anywhere
  in the log, then the failing tail plus the exit code and log path (so
  you can `Read` more by slice). A green build is then paid once, not
  replayed every later turn. (Do this within the shell rules — the
  runner captures inside Python, so the command line carries no
  redirect.)

  **The rule is about any repeated command that is verbose on success —
  not about build cascades, and not only about `cargo` and `make`.**
  Framing it around "Compiling …" is what let a whole runner slip
  through: one session ran the frontend test script **12 times** and a
  frontend `exec` **9 times**, all unwrapped, for ~5.2k combined on
  output that is one summary line when it passes. Every entry in that
  session's hardening table was a `pnpm` shape. So `pnpm test` /
  `pnpm exec` / `pnpm install` / `pnpm build` route through the runner
  exactly as `cargo` and `make` do, as does any other tool you are about
  to invoke repeatedly.

  The runner also **surfaces the failing hook's spelling offenders**
  (`Unknown word (…)`, with the file each was found in) above the tail.
  That is worth knowing because cspell runs the tree in *chunks*, so the
  tail window routinely showed a later, *passing* chunk — reporting
  `Issues found: 0 in 0 files` directly beside a `Failed` hook — while
  the real failure sat in an earlier one. Three sessions paid a
  follow-up grep over the captured log to find the word, one of them
  four separate times.

  **Nothing is printed until the command exits.** Output is captured, so
  polling the log while a *backgrounded* run is still in flight returns
  nothing — one session made seven such `tail` calls, all empty. Wait
  for the completion notification instead.

- **Verify at checkpoints, not after every edit — whatever the runner
  is.** The 12 test runs above were a fix-verify loop after single-file
  edits, which `review-pr` already forbids; it slipped because that rule
  is written in terms of the Rust suites, so a frontend test script read
  as out of scope. It is not. Batch a logical change, then verify once.

- **When a suite is fast enough to run whole, run it whole — through the
  wrapper.** This is a **granularity** rule, and it is a different axis
  from the two above it: they cover how *verbose* a command is (wrap it)
  and how *often* to run it (at checkpoints). Neither says anything about
  how much of a suite to run, which is why the narrow form keeps getting
  reached for on the reasonable-sounding grounds that it gives a faster
  signal on the one module being edited.

  Measured, and the reasoning does not survive it: one session made **32**
  `python3 -m unittest discover … -p test_X.py` calls costing **≈7.1k** —
  its single largest hardening candidate — against **15**
  `make tools-tests` calls costing **516 tokens in total**. The per-module
  form is ~14× the cost *per call* for a **narrower** answer, because it
  is not wrapped by default while the `make` target is. It also missed a
  sibling test the edit had just broken, twice in that one session (a
  named-tuple refactor and a changed error string each broke tests in a
  file other than the one being edited).

  So for anything under `.claude/tools/`, the post-edit check is:

  ```sh
  python3 .claude/tools/run_quiet.py -- make tools-tests
  ```

  Reserve a `-p test_X.py` discover run for a suite that is genuinely
  slow — this one runs in well under a second. The rule generalizes to
  any test target whose whole-suite runtime is a fraction of a round
  trip: the narrow form only pays when the suite is slow enough that the
  *wall-clock* saving exceeds the context it costs, and a sub-second
  suite never clears that bar.

- **Match the build to the iteration, not to CI.** A production-build
  target exists to *mirror CI* — a full dependency install, a wiped
  output directory, an optimizing compile — which makes it a
  **pre-commit / pre-push check, not an inner-loop tool**. One layout
  session ran `make decks-build` **×27**, once after nearly every
  micro-edit, where the running dev server (`make decks`) hot-reloads
  the same change instantly and for free. Iterate against the dev
  target; run the production build **once**, before committing.

- **Inspect a run_quiet log by its printed path, not a glob.** When you
  need more than the summary, filter the **specific log path the runner
  printed** for that run — never a `*.log` / `make-*.log` wildcard,
  which matches every historical run in the temp dir and balloons the
  result with cross-run noise. (The runner's `inspect` subcommand
  refuses a second positional for exactly this reason, and offers no
  `--latest`.) And when the run is a **background** quiet-runner task,
  wait for its completion notification, then read the summary **once**
  — don't poll the interim log (it suppresses output mid-run, so
  repeated reads just return "(no output)").

  **Filter in a process, not into the result.** Either
  `run_quiet.py inspect <log> --grep <re>` or the Grep tool; both return
  the answer rather than the matched region. Never a shell `grep` /
  `tail` / `head` on a log — those print the matched region into the
  tool result, where it is then replayed on every later turn. To read a
  *region* rather than search for one, use the Read tool's `offset` /
  `limit`.

  **The reason is context cost, and only that.** Do not argue it from
  permission churn: `firm_core.NO_BARE_WILDCARD` is a deny-list of
  *hazardous programs* and `grep` / `tail` / `head` were never members,
  so the permission floor does **not** refuse them — `Bash(grep:*)` is
  already granted here and prompts for nothing. A lever argued from
  "unfirmable churn" is arguing from something that was measured and
  found false.

- **Scope a sub-agent fan-out.** Inlining the same large diff into N
  reviewers pays for N resident copies; scope each agent to its files,
  or have them read one shared file, rather than inlining N times.

- **Polls multiply payload.** A read issued once is cheap; the same
  read polled across a CI / merge wait is paid per poll *and* per
  later turn — that's why `review-pr`'s waits use the compact `gh`
  reads above rather than the full-object MCP calls.

- **When the question is "does property P hold across N sites",
  write the checker — do not dump the sites.** This is the one lever
  here that improves **correctness**, not merely cost, which is why
  it is worth stating separately from the narrowness rules above.

  Measured: verifying that no TSDoc link target was unresolvable was
  first attempted by listing all **74** link sites (≈1.8k) and
  adjudicating a bounded subset by eye. That **missed a real site**,
  which a review lens then caught. A 15-line stdlib script comparing
  each link target against its file's scope answered the question
  definitively for ~0 context and found the miss.

  The failure is structural, not a lapse of attention: an eyeball
  sweep over a long list **silently self-limits** — it degrades into
  spot-checking exactly when the list is long enough to matter, and
  reports the same confident verdict either way. A checker does not.
  So report the checker's verdict, not the list it read; and if the
  property is worth verifying more than once, the script belongs
  under `.claude/tools/` with a test (see
  [skill tooling](skill-tooling.md)).

- **Never read a verbose-by-refresh log whole — tail it.** This is a
  distinct class from a build cascade, and it bites hardest because the
  output *looks* like a normal log. A `--watch`-style command re-prints
  its whole table on **every refresh**, so the file grows per poll
  rather than per fact, and only the **last** state informs anything.
  One session's single largest result (~13.1k) was a background
  `gh pr checks --watch` log read whole, for a status that is one line.

  So: for CI waiting use `python3 .claude/tools/wait_for_checks.py`,
  which reports the terminal state and nothing else; for anything else
  in this class — container logs, a progress-bar installer, a
  re-rendering status table — tail it, don't read it. The same shape
  covers a cold `pnpm install`, whose output is nearly all registry
  retries and peer-dependency trees: route it through the quiet runner
  (`python3 .claude/tools/run_quiet.py -- pnpm --dir frontend install`).

- **Verifying a UI change: assert programmatically, screenshot
  CLIPPED.** A screenshot read back is a top-tier context sink and
  nothing here said so. Measured: five full-viewport PNGs cost **≈105k
  of one session's ≈115k total Read cost — 91%** — and took the top five
  places in its largest-results table (≈37.0k, ≈20.8k, ≈17.7k, ≈15.2k,
  ≈14.4k), while every other Read in that session combined came to under
  11k. A 1280×800 capture read back at ≈37k is worth about twenty-five
  whole-file source reads.

  The same session demonstrates the fix, which is what makes this
  concrete rather than speculative: two **clipped** screenshots, taken
  with puppeteer's `clip` against an element's measured bounding box,
  cost **≈1.4k each** — ~15× cheaper — and answered "does this control
  look right" completely. So:

  - **Assert first.** A bounding-box query plus an
    intersection/geometry check is a few hundred bytes and is *stronger*
    evidence than an image, because it is exact. In that session a
    programmatic rectangle-intersection test had already proven the
    overlay did not collide with anything, which is what made the
    full-frame shots confirmatory.
  - **Clip by default.** `clip` takes the rect you just measured, so it
    costs nothing extra to author.
  - **Reserve a full viewport** for when the composition itself is the
    question, take **at most one**, and consider a smaller viewport or a
    reduced `deviceScaleFactor`.

  The lever is *not* "don't screenshot" — those full-frame images were
  shown to the operator and drove real design decisions. It is that the
  default should be clipped and the count deliberate.

- **After a formatter rewrites a file you are still editing, take ONE
  bounded read covering all the remaining edits' regions.** A formatter
  invalidates every line the session holds for that file, so the next
  anchored `Edit` needs its exact text again — and the
  "before the third slice, sum what you have read" rule does not fire,
  because each re-read *feels* individually licensed: something just
  invalidated the previous one, so it reads as a fresh **first** read
  rather than a third slice.

  Measured: one source file slice-read **five times** in a session
  (offsets 210, 160, 418, 226, 486) and a second file twice, together
  that run's five largest single results and the whole of its Read cost
  (5.8k) — a round trip per edit. Cheaper still where possible: order
  the edits **before** the formatter runs, then let the hook reformat
  once at the end.

- **A DRY RUN or expansion is verbose-on-success too.** The rule above
  is stated by naming runners, and a reader checking `make -n` against
  that list sees `make` and then reasons that `-n` exempts it. It does
  not. The class by **shape** is *any command whose output is a
  program's own text rather than its result* — `make -n`, `make -p`,
  `git config --list`, `docker compose config` — and the follow-up is a
  grep of the captured log for the one line in question, never a read of
  the result.

  Why it sticks: you are asking a **targeted** question of a
  **whole-program** dump, so the ratio of wanted to bought lines is
  worst precisely when the target is most recursive. Measured:
  `make -n demo` run four times, the two unwrapped calls landing the
  entire expanded recipe cascade — one the session's 6th-largest single
  result at ~972 tokens, ~1.1k across the shape — to learn whether one
  recipe line still carried a teardown. The same question asked later
  through the wrapper plus a two-pattern grep of the log cost ~50
  tokens.

  Two things specific to a dry run, both counterintuitive: `make -n`
  still **executes** `$(MAKE)` sub-make lines, so it is not
  side-effect-free either (that run's first `make -n demo` really did
  tear down the keyless collectors), and the cascade scales with the
  target's recursion depth rather than with the question — so the
  cheapest-looking target produced the fattest result.

- **Bound a probe or extraction script's output at both ends.** When
  probing an unfamiliar external API, go **through a filtering script**
  rather than a bare fetch: one bare `curl` of an FX feed catalogue cost
  ~5.3k returning ~60 currencies with full attribute blocks, to extract
  seven ids — the same session then filtered in Python for ~200 tokens
  per probe and answered more. The mirror-image failure is a script
  whose anchors are too loose: a CSS extraction slicing from a start
  marker to a far-off end marker returned ~6.0k, ~90% unrelated
  stylesheet, for a question about two selectors. Print what you need,
  not the region it lives in.

- **A script you write is a tool-result generator, so give it the same
  narrowest-form discipline you give a search.** The rules above are
  written as if every payload arrives from someone else's command; the
  expensive ones increasingly arrive from a script authored moments
  earlier, and that authoring step is where the blind spot lives. The
  shape is always the same: the script correctly keeps a huge input out
  of context, then **dumps its own intermediate** instead of the answer.

  Two measured instances. An ad-hoc probe printed full per-pool tables
  for 19 tokens (**~4.6k**, plus 2.3k on a re-run) when the decision
  needed per-token totals plus the busiest pool — it had already kept
  300KB of JSON out of context and then spent the saving on its own
  output. And a throwaway analysis script dumped a ~200-line
  chronological series **three times** as it was corrected — the top
  three Bash results of that session, ~8.7k combined, to answer three
  sentences.

  So, when you write one:

  - **Default to a ranked one-line-per-item summary**, and make detail
    an explicit flag. Not the reverse: a flag that must be remembered to
    *suppress* output is one that will be forgotten.
  - **Emit the answer shape, not the series.** If the question is "when
    did this change", print the change-point and its neighbors; if it is
    "what values occur", print the distinct values with counts.
  - **Park the full series in a scratchpad file** and slice it if the
    summary turns out to be wrong. That keeps the second look cheap
    instead of making the first one expensive.

- **When replacing a binary asset, compare metadata before content.**
  Reading the outgoing artifact to compare it against the incoming one
  costs a full binary Read — one run paid ~8.8k re-reading an old
  committed capture, where `file` dimensions on both answered the actual
  question for ~337 tokens. Ask what you are really comparing:
  dimensions, byte size, and format usually settle it, and looking is
  only warranted once the metadata says they match and you still
  disagree.

- **Treat a screenshot as a 25–60k-token result class.** An image is
  not a cheap glance: a full-viewport (2560×1440) screenshot `Read`s at
  ~30–50k tokens, and on a visual-iteration run image Reads have been
  the top sink outright (~180k, ~88% of all Read in one session — three
  separate captures ≈94k of it answering a single question).

  **The price is raw bytes ÷ 4, and that arithmetic is what makes it
  predictable — so do it before the call, not after.** Pixel dimensions
  are the wrong intuition: a *250px* PNG icon at 75–85KB is
  **~20–28k tokens each**, which is why one session spent ~60k reading
  three icons (out of 64.4k of total Read cost), and another spent
  ~40.4k — 72% of all its Read — on one 3840×2160 slide capture to
  answer "does the tagline fit on one line". Small on screen does not
  mean small in context; check the file size.

  Request one deliberately, and:

  - **Never re-`Read` an image already in context.** Like every tool
    result it is replayed each turn; a second Read buys the same
    ~40k twice.
  - **Prefer ONE composite capture per round** over several separate
    ones — a single frame showing the whole state beats a gallery of
    partial views at the same total question answered.
  - **Capture only the frames that prove the claim** — the
    broken→fixed pair, not a gallery — at a **reduced resolution**
    (≤1280-wide, or JPEG), so each costs a few k rather than ~45k.

  This is the live-verification discipline (the `/verify` and `/run`
  flows); a proof needs two frames, not four full-res ones.

- **Downscale a *supplied* screenshot before reading it, when the
  question is layout.** The rule above governs captures you take; a
  screenshot the user hands over arrives at full resolution, and cost
  scales with pixel count. A misaligned caption, an overflowing panel,
  or a chart standing on a divider is all legible at half resolution —
  so copy it down first and read the copy. One bare, globbable command:

  ```sh
  sips -Z 1200 <in> --out <copy>
  ```

  Two sessions rest on this: eight supplied screenshots were ≈693k of
  one session's ≈934.7k total `Read`, and two more were ≈104k of
  ≈132.3k (79%) in another. Both were design-iteration loops where
  every read was single-use, so repetition wasn't the problem —
  **resolution** was, and downscaling is roughly a 4× saving on the
  dominant sink of such a session. Keep full resolution only when the
  question really is about pixels (anti-aliasing, a hairline, an exact
  color).

- **When a supplied screenshot carries on-chain data, decode the source
  instead of reading the picture.** Distinct from the rule above: this
  one is about the transport of *evidence*, not resolution. An 88.8k
  screenshot of an explorer token-balance table was one session's
  single largest `Read`; the same numbers were then established
  **exactly** by a ~40-line probe that decoded the transaction's fill
  legs and reconciled them against its token deltas. The image was the
  *prompt*; the probe was the *evidence*. So when a screenshot shows
  on-chain state — balances, a transaction, an account — ask for the
  signature or address and decode that: cheaper, and strictly more
  precise. Downscaling remains the rule for layout screenshots, except
  where the next rule finds a cheaper source of truth.

- **When the question is geometry, measure the screenshot — don't read
  it.** The rules above govern *resolution* and *evidence*; this one
  governs **repetition across rounds**, which "never re-`Read` an image
  already in context" does not catch: each round of a debugging loop
  supplies a **fresh** capture, so that rule never fires. One centring
  bug ran four such rounds, and their Reads (≈36.5k / 34.8k / 29.4k /
  21.8k ≈ 122k) were ~73% of the entire session's Read cost. The first
  Read earned its keep — orienting, and catching a vertical-misalignment
  regression that only reads visually. Rounds 2–4 were pure geometry, and
  paid in pixels for an answer that is a number.

  **So: orient once, then measure.** Re-reading pixels is warranted only
  when the *content* is in doubt. For "did the crop land where I meant",
  image dimensions settle it outright — a one-line `file` call, against
  the ≈247k one run spent re-`Read`ing the same scratchpad crop twice for
  exactly that question. For "is this centred" or "did the layout shift",
  the answer is an ink-band measurement: find the horizontal bands of
  non-background pixels and compare the element's ink centre against a
  known block-centred reference to recover the midline. One session
  rebuilt that measurement three separate times from scratch; a committed
  helper for it is filed separately.

- **Inspecting rendered output — contact-sheet first, and never at
  native resolution.** The rules above cover screenshots you *take* and
  screenshots you are *handed*. A third class is output your own build
  renders — deck pages, exported frames, generated diagrams — and it is
  the most expensive of the three, because it arrives many-at-once and at
  print resolution. Two measured sessions were **882.7k of `Read` (≈98%
  of all tool-result cost)** and **≈1.1M (≈97%)**, every top result a
  5760×3240 deck page, one of them re-read four times for ≈321k combined.
  Both followed every other rule on this page.

  Capture resolution is a *product* decision — 3× is right for the
  deliverable. The copy you **look at** is the thing to shrink. So:

  - **Contact-sheet the recurring question.** "Did any page clip?" is one
    thumbnail-grid read, not N full-page reads. Use
    `python3 .claude/tools/render_review.py --montage`, which extracts
    the page images, scales them down, and writes one grid — collapsing
    a 12-read sweep to 1.
  - **Read a single page downscaled** (`--page N`), never the native
    render. Every judgment these sessions actually made — does the page
    overflow, does the headline wrap, is the footer crowded — is
    identical at ~1200px and costs roughly a tenth.
  - **Re-read only the page a change can touch.** After editing one
    slide, re-render and re-read *that* slide; a fresh full-deck sweep
    per round is what turned one page into ≈321k.
  - **Measure before you look.** If the question can be stated as a
    number — content bounding box, footer clearance, distinct-image
    count, whether two exports are byte-identical — the tool's
    `--measure` mode answers it for a few hundred tokens. In one session
    the cheap checks did the real work and *disproved* two hypotheses
    eyeballing had suggested; that same session then paid ≈284k
    re-reading a frame to confirm a spacing change the bounding-box
    measurement had already reported. **Never re-`Read` a frame to
    confirm an effect you have already measured.**

- **Route Docker image operations away from context too.** A
  `docker compose pull` / `up` / `build` dumps a per-layer
  "Downloading / Extracting / Waiting" progress cascade — the same
  noise class as an unwrapped `cargo` build log, and on one run the
  single largest result of the session purely from progress lines.
  Pass **`--quiet-pull`** to `docker compose up` / `create` (it keeps
  the final per-image line and drops the layer churn); a bare
  `docker compose pull` takes plain **`--quiet`** instead. Note
  `--quiet-pull` silences only the *pull* — a target that falls back
  to **building** the image still emits a full BuildKit log, so route
  that one through `python3 .claude/tools/run_quiet.py -- …` (or add
  `--quiet-build`) when you're the one invoking it. This applies to
  shell you author in **skills and Makefile targets**, not just
  ad-hoc calls — though a Makefile recipe a human watches is the one
  place to leave a slow build's output visible, since a silent
  multi-minute build reads as a hang.

- **Don't hand-run a check a hook already owns.** `make lint`
  enforces line length (MD013 for Markdown, the "Lines over 80
  columns" hook for code); a manual `grep -nE '^.{81,}$'` pre-check
  over a doc / Markdown diff just re-buys that result into context.
  Trust the lint hook's output instead of a manual over-80 grep.

  **And a hand-rolled width check is not merely redundant — on this
  repo it is wrong.** `awk 'length($0)>80'` counts **bytes**, and an
  em-dash is three of them, so a compliant line of this repo's
  em-dash-heavy prose is flagged LONG. One session ran five such checks
  against a gate it was already running, then had to spend further calls
  *refuting* the false signal they produced. The gate owns line width;
  if you ever genuinely want a width probe outside it, it must be
  character-aware — never a byte-length tool.

  Same
  for validating edited JSON: an exit-code-only check
  (`python3 -m json.tool … >/dev/null`, or the check routed through
  `run_quiet`) confirms the file still parses without a full pretty-print
  echo — `json.tool` re-emits the **whole file** into context, and on
  a large `settings.local.json` that dump has landed twice in one
  pass.

**Track consumption ideas as you go.** When something reads as
wasteful mid-session — a payload you only needed a slice of, a call
that repeated, an avoidable fan-out — keep a running note of it. At
session end `/session-metrics` pairs those observations with the
tool's ranked token sinks to emit *grounded* trim recommendations
(the lever, and the concrete skill / convention-doc edit it implies),
filing each as its own **parked lever issue** under the `Trim levers`
milestone through the zero-echo writer; `trim-context` later folds
them. (The old "Session Metrics" inbox document is **retired** — it
outgrew the tool-result cap between mining passes.) The tool says
*where* the tokens went; your running notes say *why* and *what to
change*.
