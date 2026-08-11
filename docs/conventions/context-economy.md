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

- **Ask for the narrowest thing that answers the question.** Use the
  narrowest method / subcommand, field-select where the transport
  allows it (`gh … --json <fields>`, a GraphQL projection), paginate
  instead of dumping, and **never re-fetch what's already in context**.

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

  Three refinements, because the flat rule "never read whole" mis-scores
  two real situations and misses a third:

  - **Read whole only when you will BOTH edit the file and brief agents
    on it.** That is the one case where the whole file is cheaper
    *overall*, and the flat rule calls it a violation. One session's top
    five main-loop results were whole-file `Read`s (≈23k) of the crate it
    was about to modify — and those same excerpts then went inline into
    all five review-lens briefs, which is what held every lens under its
    turn cap. Paid once, amortized five times. Absent that second use,
    slice.
  - **A planned multi-region read is ONE bounded read, not several.**
    Slicing is only cheaper when you are reading *less*. One run read
    `swap.rs` across four separate slices totalling **more** than a
    single whole-file read; another spent a whole-file `Read` (≈4.4k) on
    a dispatcher to find one append point. Decide the regions first: if
    they add up to most of the file, read it once.
  - **"Reading 3+ files to orient" is the trigger, not an exception.**
    Survey-time whole-file reads were the single largest sink of one
    session (top five, ≈15k). The crate was small, so no per-file budget
    felt warranted — yet `model.rs` is ~40% `#[cfg(test)]` and only two
    signatures were needed. Before any `Read` over ~300 lines, Grep for
    the structure (`^fn |^impl |^pub`, or the language's equivalent);
    the map tells you which slice you actually want.

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

- **Route verbose build logs away from context.** Prefer `-q` /
  `--quiet` so a `cargo` / `make` "Compiling …" cascade doesn't land
  inline. For a noisy target with no quiet flag, run it through the
  quiet runner, `python3 .claude/tools/run_quiet.py -- CMD ARGS…`
  (with optional `--tail N` / `--label L`): it captures the output to a
  temp log and prints only a one-line summary on success, or — on
  failure — an index of every `…Failed` hook-result line found anywhere
  in the log, then the failing tail plus the exit code and log path (so
  you can `Read` more by slice). A green build is then paid once, not
  replayed every later turn. This works for ad-hoc `cargo` /
  `pnpm` / any command, not just `make` — route a bare
  `cargo check` / `cargo test` / `cargo clippy` verification through
  it too, since those emit the same "Compiling …" cascade. (Do this
  within the shell rules — the runner captures inside Python, so the
  command line carries no redirect.)

- **Match the build to the iteration, not to CI.** A production-build
  target exists to *mirror CI* — a full dependency install, a wiped
  output directory, an optimizing compile — which makes it a
  **pre-commit / pre-push check, not an inner-loop tool**. One layout
  session ran `make decks-build` **×27**, once after nearly every
  micro-edit, where the running dev server (`make decks`) hot-reloads
  the same change instantly and for free. Iterate against the dev
  target; run the production build **once**, before committing.

- **Inspect a run_quiet log by its printed path, not a glob.** When you
  need more than the summary, grep the **specific log path the runner
  printed** for that run — never a `*.log` / `make-*.log` wildcard,
  which matches every historical run in the temp dir and balloons the
  result with cross-run noise. And when the run is a **background**
  quiet-runner task, wait for its completion notification, then tail
  **once** for the summary — don't poll the interim log (it suppresses
  output mid-run, so repeated tails just return "(no output)").

- **Scope a sub-agent fan-out.** Inlining the same large diff into N
  reviewers pays for N resident copies; scope each agent to its files,
  or have them read one shared file, rather than inlining N times.

- **Polls multiply payload.** A read issued once is cheap; the same
  read polled across a CI / merge wait is paid per poll *and* per
  later turn — that's why `review-pr`'s waits use the compact `gh`
  reads above rather than the full-object MCP calls.

- **Treat a screenshot as a 25–60k-token result class.** An image is
  not a cheap glance: a full-viewport (2560×1440) screenshot `Read`s at
  ~30–50k tokens, and on a visual-iteration run image Reads have been
  the top sink outright (~180k, ~88% of all Read in one session — three
  separate captures ≈94k of it answering a single question). Request
  one deliberately, and:

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
  Trust the lint hook's output instead of a manual over-80 grep. Same
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
(the lever, and the concrete skill / convention-doc edit it implies)
into the Linear "Session Metrics" inbox, which `housekeeping` later
mines. The tool says *where* the tokens went; your running notes say
*why* and *what to change*.
