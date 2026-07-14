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
- **Minimize live-verification screenshots.** A full-viewport
  (2560×1440) screenshot `Read`s at ~30–50k tokens, so a set of them to
  prove one visual fix dominates the run's Read cost. Capture **only the
  frames that prove the claim** — the broken→fixed pair, not a gallery —
  at a **reduced resolution** (≤1280-wide, or JPEG), so each screenshot
  costs a few k rather than ~45k. This is the live-verification
  discipline (the `/verify` and `/run` flows); a proof needs two frames,
  not four full-res ones.
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
