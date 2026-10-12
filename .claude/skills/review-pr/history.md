# `review-pr` history ledger

The measured incidents behind the rules in [`SKILL.md`](SKILL.md),
kept here so the entry file states each rule once. Provenance only;
not loaded on invocation. The entry file grew 60k → 84k → 212k → 292k
bytes from July to October across 47 commits with nothing pushing back
(measured by the design thread that filed the size gate), which is what
the resident-size gate (`docs/conventions/context-economy.md` →
"Resident size has a hard cap") was built to stop.

## The entry gate

- **Fan-out without a yes.** The entry question once approved the run
  and its tier together, on the reasoning that the fan-out is one step
  of many and an operator might want lint and CI without eight agents
  and ~2.9M of sub-agent input. Ruled otherwise on 2026-10-12, via the
  planning session: invoking the skill pre-approves the regular fan-out,
  as invoking `audit-scope` does its own, and the question survives
  only to offer an opt-down.

## Step 1: locating the PR

- **Merged stacked base.** A review ran end-to-end against a stacked
  base that had already merged, went green on CI, and only turned
  `CONFLICTING` at the step-15 merge-clean check. Recovery cost a
  `git rebase --onto` plus a full re-run of `make lint`, `make test`
  and `make test-no-teardown` (~25 minutes) — which is why the check
  sits in step 1, before anything expensive.

## Step 2: rebase and base-delta triage

- **Merge-completeness evidence.** `merge_completeness.py` caught a doc
  comment reading "all four kinds" that merged cleanly when the true
  post-merge answer was five. In the same resolution both branches
  added a variant to one enum and an arm to one match: every line from
  both sides survived, completeness passed trivially, and the result
  was still wrong in a way only reading revealed — so a carve-out keyed
  on "the tool passed" would have shipped it. The rule had already
  rejected "resolve it and let the linter verify". yamllint's
  alphabetical keys (`cfg/**`, `infra/aws/**`) were examined for a
  structural fix and left as they are; the dictionary's `merge=union`
  leaves one residual, a resurrected deleted word
  (`docs/conventions/docs-and-style.md` → "Spelling (cspell)").
- **Conflict inspection.** A repo-wide `grep '<<<<<<<'` over
  `programs sdk bots frontend docs` walked `frontend/.next/` and
  returned 79.2KB for a short file list. Separately, 14 bare `grep`
  calls cost 2.5k inspecting one `Makefile` conflict and hook wiring:
  `grep -A/-B` around the markers prints overlapping windows. Those
  calls were once recorded as unfirmable prompt churn; they are not
  (`Bash(grep:*)` is granted), so the context cost is the whole finding.
- **Hand-rolled rebase triage.** One session ran the same `fetch` →
  `log` → two `diff --name-only` → intersect-by-eye chain three times
  as `main` moved 15 commits (≈10k of deterministic git output), and
  re-ran the full suite each time, twice provably redundantly.
- **Merge-base, not the tip.** A separate incident: the step's own
  instruction once captured the tip with `git rev-parse origin/<base>`
  "before the fetch" and handed that to the reporter's `--from`, and it
  got it wrong on its own first run: worktrees share one `.git`, so a
  sibling's fetch (or this session's `init-pr` fetch hours earlier)
  had already advanced the ref, and the tool reported a 0-commit delta
  for a base that had demonstrably moved. The merge-base is correct
  regardless of who fetched when.
- **Empty-overlap re-runs.** On one docs-only PR the base moved four
  times with an empty overlap every time, and the session still paid
  three `review_diff.py` re-runs and two full lints.
- **Lint-config base delta.** A mid-review rebase pulled in a 42-file
  delta that added a `rustdoc` hook to the lint config while the branch
  touched only `decks/**`: the overlap was empty and the rule as written
  said assert. That run re-ran lint on judgement and passed; had it not,
  the PR would have gone to CI red with a local green in hand. It is a
  correctness carve-out to a skip rule (re-running lint costs
  wall-clock, not tokens), the same shape as the
  `programs/**` and lockfile carve-outs, and not a loosening of the
  freshness gate, which caught a real defect in that same session.
- **Stale program after rebase.** A scoped `cargo test` after a rebase
  that pulled new program source ran against the old `.so`: 8 failures
  in tests the session had never touched (`Custom(6037)` where
  `Custom(6048)` was expected), read as regressions from its own edits.
  Diagnosing, rebuilding and re-running was that session's largest
  wall-clock detour.
- **Stale node_modules after rebase.** Three sessions. `tsc` reported
  three `TS2307: Cannot find module 'vitest'` errors that read as code
  faults; a base commit had added a test runner. One session hit it
  after a successful install earlier in the same session, because the
  rebase moved the lockfile underneath it.
- **Hot-surface tails.** One review saw `main` gain four issues, three
  in the same subsystem, forcing three conflict resolutions — one
  semantic, a trait the branch referenced deleted upstream — plus three
  full test runs and repeated lints; the freshness machinery correctly
  refused to fan out on a 3,783-line phantom-deletion diff, so the cost
  was structural. Another saw `main` move four times, three with an
  empty overlap, the fourth overlapping on `cfg/dictionary.txt` and
  forcing a full re-lint. That one was 37 files with only one costly
  rebase, which is why the signal is overlap and not size.

## Step 3: Linear echoes

- **Cross-skill echo budget.** One session paid 24.5k across three calls
  on one issue for three state transitions, with every skill
  individually compliant: `init-pr` had already spent a `get_issue` and
  the In Progress write before `review-pr` ran. Each echo is a fixed
  cost per call that `patch` does not reduce, and worst on a large
  consolidated-spec body.
- **Deferred-tick echoes.** Boxes deferred past the fan-out fired their
  own `save_issue` beside the In Review transition — two full-body
  echoes (~1.2k per review) where one write served both.

## Step 4: lint

- **Full-sweep loops.** The scoped re-run was already the stated rule
  each time: ten full sweeps across ~5 fix-and-retry cycles (≈5.3k);
  thirteen (≈5.8k) while editing that very rule; fifteen, each surfacing
  one violation class, with three cspell words found on three separate
  sweeps; six full `--all-files` sweeps after edits confined to one
  crate, because only the failure case read as in scope; eight on a
  markdown-only PR that read "two runs by construction" as two full
  sweeps; 6 invocations walking cspell, line-length and cspell again for
  one class of problem; 12 invocations (≈4.4k) on a spec edit that read
  the markdown fail-then-fix as a failure; seven full runs after a
  scoped run without `--config` failed with `InvalidConfigError` and was
  read as "the scoped path doesn't work here". In one run, immediate
  identical re-runs after `biome` reformatting were a meaningful share
  of 3.6k / 10 scoped-lint and 900 / 5 `make lint` calls. An unwrapped
  scoped `pre-commit run` prints all 24 hook lines, ~20 of them skipped
  (≈675 tokens; four runs ≈2k). The runs were already wrapped, so the
  cost was failure tails and wrapping harder buys nothing; it is the
  most-missed rule in step 4. What finally moved the reflex was
  `lint_paths.py --changed`: the full sweep needed no arguments while
  the scoped one needed a hand-built file list.
- **Serial spelling rounds.** Session a252a9d3 (PR #391): five Rust
  crates plus one doc, adding several hundred lines of doc comments (a
  module header, seventeen documented public constants, a new error enum
  with per-variant docs, long comments on two failure modes). It
  tripped cspell on two full runs — three unknown words, then two, one
  British — so two of its five `make lint` calls were spelling alone;
  it skipped the pre-flight because the diff did not look like prose.
  Others: two rounds of three British `-our` / `-ise` variants each;
  four words then two more, all six authored by the session and three
  of the first four British; three serial rounds for three words; 26
  `make lint` runs (~12.8k of failure tails) on one prose-heavy change.
  The pre-flight is a different lever from scoping the lint: a scoped
  run fails identically on an unknown word, so only finding them all at
  once helps. Coinages: four rounds on one change, every word a
  self-inflicted verb form, each reword producing the next round's word
  (round trips, not context — the runs were wrapped). Three British
  words tripped the hook while the rule itself was being written.
- **Lint workflow beyond the hooks.** Two failures in opposite
  directions. A diff added a sixth Grafana alert rule and left two stale
  counts in the file's comments: `make lint` passed, and CI came back
  8 pass / 1 fail on a `tools-tests` checker that had landed on `main`
  while the branch was in flight. A branch adding two shell-driving test
  files passed on macOS and failed CI with 14 errors and 3 failures: no
  `zsh` on the Linux runner, and BSD `stat -f %m` meaning something else
  under GNU coreutils (Python 3.14 in CI against 3.9 locally at the
  time). In neither case was the failure a hook, so "reproduce the
  failing hook locally" did not apply.
- **Log filtering.** A whole-file read of a captured lint log made a
  500-line cspell dump the single largest result of a run (PR #207);
  another session spent 58 shell `grep` calls (≈8.7k) on these logs,
  which is why `run_quiet.py inspect` exists. The paragraph twice
  carried a false permissions argument. `firm_core.NO_BARE_WILDCARD`
  lists hazardous programs, and `grep` / `tail` / `head` were never
  members, so `is_bareverb_wildcard("Bash(grep:*)")` is `False` and the
  rule is granted. The first version argued against `grep` on
  permissions and then said "or read only its tail", which a perms sweep
  harvested; the second fixed the tail and kept the false rationale.
- **Config-index reads.** `cfg/pre-commit-lint.yml` (~190 lines) was
  read whole (≈1.7k) to learn which hooks cover `.sh` and
  the `.github/` tree. The lint workflow was read whole (~770 tokens, 40% of
  that session's Read cost) to learn one line, which then sent the
  session to grep the hook config anyway.
- **Formatter re-read loop.** PR #396: one source file slice-read five
  times (offsets 210, 160, 418, 226, 486) and a second file twice — the
  session's five largest results and all of its 5.8k Read cost, a round
  trip per edit, each re-read feeling like a fresh first read because a
  formatter had just invalidated the last. A 99-line file was bought
  whole with `cat -n` for a one-region edit. `git diff` showed up at
  ≈1.0k over 2 calls, the only context-cost shape in one hardening
  table, both after the slices existed: a `--stat` the verdict already
  reported, and a reflow `review-diff-docs.txt` already held in 67
  lines. Five hand-rolled `awk 'length($0)>80'` width checks were
  redundant with the lint run and wrong, flagging compliant em-dash
  lines as long.
- **Inner-loop checks.** `make lint` re-run on a byte-identical tree;
  `tsc --noEmit` four times and `biome check` five, several after
  single-file edits that could not change a type; `make decks-build`
  ×11 as an inner-loop check (its README calls it a pre-commit check),
  mostly after copy-only edits; `pnpm -C decks check` ×19 across ~15
  rounds, including after comment-only changes — both on visual PRs.
  After grep, it was the top repeated shape in both of those sessions.
  The narrow `tools-tests` form cost 32 calls / ≈7.1k against 15 calls / 516
  tokens for the whole suite, and missed two sibling tests the edits
  had broken (`docs/conventions/context-economy.md`).

## Step 5: slicing the review diff

- **Generated output dominating the diff.** A conformance-vectors PR,
  before `sdk/conformance` joined `DIFF_EXCLUDES`: a 5783-line diff of
  which ~3460 lines were vector JSON, so even the tests slice came out
  at 4872 lines, and the category split could not isolate the 1532
  hand-written lines either. The two lenses handed the full diff were
  that review's two most expensive, at 2.6–2.9x the cheapest, and the
  ordering tracked handed-in size almost monotonically. Both had been
  told to "skim past" the JSON — prompt discipline standing in for a
  slice that should not have contained it.
- **Diff hygiene.** A 6001-line diff was ~3607 lines of
  `pnpm-lock.yaml`, read by all 7 agents and replayed per turn — the
  case for the generated-family excludes. A sibling session's stale
  `/tmp/review-diff.txt` once cost an entire 6-agent pass reviewing
  the wrong diff — the case for the scratchpad. On one run a
  read-through for self-inflicted staleness before generating would
  have caught three of four slice regenerations in one go.
- **Slice routing.** One fan-out cost ≈2.68M across five lenses on a
  ~1.5k-line diff with every brief already inlining excerpts, naming
  comparison files, stating a cap and handing the diff by path: all
  five read the same whole file, which carried 212 lines of
  `docs/architecture.md` and comment reflows only one lens needed.
  Another run, seven sub-agents summing ≈2.28M — cross-check
  504.8k/6, test adequacy 502.8k/6, conventions freshness 357.7k/5,
  correctness-modified 304.6k/4, skill prose 289.4k/4,
  correctness-new 207.8k/3, security 110.0k/2 — with four of seven at
  1.7–2.8× the top of the exemplar band: scope, not depth, since input
  scales with material handed in × turns taken. The diff was 60 files,
  +8656/−532, and neither correctness lens got a narrowed slice. The
  one-slice default came from a run with all five lenses at or under
  cap and two convergent blocking defects, so it argues for nothing
  fewer or shallower. A doc-accuracy lens handed the docs slice plus
  both full source sub-slices as ground truth ran 504.4k over 6 turns,
  2.5× the cheapest substantive lens on the same diff (249.6k / 4);
  every finding it returned turned on a handful of lines the main loop
  already held.
- **Slice size.** A fully compliant fan-out ran at ~95% of session
  cost with turn counts holding and input still 1.3–5× the exemplars,
  because the source slice was 3,297 lines. The ceiling used to read
  "roughly a thousand lines"; one run cleared that on two slices with
  every prompt discipline applied and still ran 2.2–3.6× the
  exemplars, four of six agents overrunning their cap by exactly one —
  tracking slice size, not brief quality. That off-by-one is the
  exemption-lifts-the-cap case: the prose lens in that run was
  deliberately handed the whole 1,536-line prose slice and found six
  real contradictions. Before `--only` existed, 3,156- and 4,148-line
  source slices went to every lens whole (the costliest single agent
  took 634.7k), and a second session hand-rolled the split as three
  `git diff` calls with literal path lists. The small-diff exemplars
  are 90.4k / 102.9k / ~145k. Per-lens subdivision (session a252a9d3,
  PR #391): a 1,888-line diff across five crates gave a 1,070-line
  source slice to every lens; all came in at or under cap with 45
  established facts, verbatim excerpts, three sub-questions and caps in
  both units, and input still ran 1.4–3.3× the efficient exemplars.
  The style lens's three questions concerned one file's new public
  surface; an `--only` on the SDK crate would have cut it to about a
  third.
- **Fixed slice names.** Slice names were once fixed, so a scoped
  `--only` run silently overwrote the unscoped run's slices, and a
  tools-only run with no docs hunks left `review-diff-docs.txt` empty;
  the lens handed it correctly reported nothing to review — a silent,
  total failure of that lens. Caught once by a `wc -l` before
  spawning. Inline tests: before `--split` routed `#[cfg(test)]`
  hunks, a 4,351-line Rust diff produced a zero-line tests slice and
  the test-adequacy lens read three source slices to become that run's
  costliest agent (473.6k).
- **Re-deriving the sliced diff.** A bare `git diff` over a range
  already sliced was the second-largest single result of two sessions
  (≈3.4k, and ≈3.1k for a self-review). Per-file scope (session
  08c0ae6b, PR #383): a `read_result.py --grep '^[+-][^+-]'` over a
  450-line source slice returned ≈4.7k, rank 3 of that session,
  printing 306 of 447 lines for a question about one file with 31
  changed lines — the other ~275 lines were files the main loop had
  just authored and already held; a path-limited `git diff` cost a
  fraction.
- **Phantom deletion.** A newly landed test showed up as a phantom
  deletion after the base moved under the review, and both the
  correctness and completeness lenses independently flagged it as a
  blocking coverage regression. The whole fan-out went on a false
  positive and was re-run from scratch on a corrected base along with
  the full test suite; the line count passed throughout.

## Step 5: briefing the lenses

- **The second spawn question.** A review under a standing "don't
  spawn agents unless asked" default paid a round trip to ask, was
  authorized, and found six blocking defects — the question was
  ceremony, since the user had just typed the skill name.
- **Why the inline path was removed.** The objection is to the
  assurance property, so it does not soften with diff size: an earlier
  proposal to keep the inline path behind a diff-size or rebase-risk
  ceiling is superseded. Cost is not the argument and the naive reading
  is backwards — an inline pass is cheaper in total tokens, but every
  byte lands in the main loop and is replayed every later turn, while
  a fan-out spends in throwaway contexts and returns only findings;
  that exposure compounds under post-review rebase churn. One inline
  run did find two real defects, which is not evidence the path was
  sound: it declared its reduced assurance and still produced a review
  no fresh context checked.
- **Preamble economics.** The standing half ran ~1.5–2k tokens per
  lens in one review; across six lenses at 6–14 turns each that was a
  meaningful slice of a 5.4M fan-out, paid to say the same thing
  forty-odd times. Composing it by hand meant reading
  `docs/conventions/sub-agent-brief.md` whole (≈1.7k, measured on two
  runs) to copy it verbatim.
- **Facts-block evidence.** One run brought all five lenses in at or
  under cap — 2, 2, 3, 4, 4 turns against 5/5/4/5/8 — crediting not
  the hard-stop wording (already standard) but an ad-hoc block of
  three pre-run grep results, the lint gate's coverage and explicit
  negatives; two lenses said outright they needed no further reads.
  Another reproduced it: a security lens at 90.4k over 2 turns with
  zero cold reads, that review's sharpest findings. The width rule: a
  facts block gave the flush-level factor formula without its width,
  so the claim-accuracy lens cold-read the matching-math module to
  check whether a PPM multiply plus a max offset could trap — it
  computes in `u128`, and one word would have closed it.
- **Missing preamble.** All four lenses of one review independently
  reported `lens-preamble.md` missing, and nothing in the skill
  noticed: the preamble was emitted before a session restart cleaned
  the scratchpad, while the slices survived because a mid-review
  rebase regenerated them. The fan-out ran with no standing shell
  rules and no suppression list.
- **Per-lens retries.** Three of four lens spawns in one review died on
  an upstream 529 before a single turn, and each retry re-sent the full
  ≈6k inline brief for zero work — the case for per-lens files.
- **Cold-reading held context.** The largest sink across ten
  consecutive PR runs (freshness 379.3k; completeness 653.1k and
  cross-check 631.0k on one PR; style 485.8k on another), in fan-outs
  that each caught real blocking bugs — the waste was inputs, not
  lens count. A correctness lens ran 683.5k / 10 turns against a ≤ 6
  cap, about 4× the cross-check, by cold-reading two named reference
  files (`sdk/rs/src/events.rs`, `tui/src/fills.rs`) the main loop had
  already read. Without a section map, a correctness lens re-derived a
  shared-state invariant from scratch (923k / 12 turns on a 944-line
  TUI diff) despite read-once. A style lens globbing the components
  tree for the local idiom reached 485.8k. An impossible cap: a
  714-line component against an ~850-line spec ran two lenses 8 and 11
  turns against a stated ≤ 6; neither disobeyed.
- **Hard-stop wording.** Within one session, same diff and model, the
  one lens given an explicit hard stop plus inline material
  (freshness) was cheapest at 241.8k / 6 turns with the two best
  findings, while a lens given a soft "≈6 turns" ran 850.2k / 15 —
  2.5× its cap, 3.5× the cost. Another: security 323.2k and
  completeness 349.9k, each 7 turns against a soft "~6". Confirming: a
  five-lens tier with every lens under cap (314.3k/5, 249.4k/4,
  227.4k/4, 382.6k/6, 448.6k/7) once every lens got the freshness
  treatment. Units: one completeness lens ran 939 seconds and 17 tool
  calls while the rollup scored 7 turns — compliant to the harness, a
  3× overrun to whoever pays. Re-reading files the diff already handed
  over (`swap.rs`, `matching.rs`) has run one lens past 700k, and lenses
  re-reading or re-grepping a file each turn have run 197k–469k input
  apiece — the case for read-once. Not
  solved: four sessions with the verbatim wording and inlined excerpts
  overran anyway — three of six over by exactly one turn in one, and in
  another a byte-identical brief bound completeness (4 turns / 222.5k)
  and not correctness (9 turns / 486.6k).
- **Sub-question count.** The two most expensive lenses of one review
  (463.1k and 447.4k, 8 turns each) were each handed six enumerated
  sub-questions, while the cheapest (255.3k, 5 turns) got five scoped
  to two named files; both expensive ones spent turns re-deriving
  layout facts (a struct's fields, a guard in `create_market`) the
  main loop could have inlined in two lines. Turn bound: on a 52-file
  meta PR the lenses ran 4–5× the exemplars (~3.24M total), 3 turns =
  162.8k against 8 turns = 797.6k. That evidence does not touch the
  rejected cross-check cap: the cross-check ranked only fourth there,
  and its synthesis reframed the whole finding set.
- **Exemplar figures.** 90.4k / 2 turns, zero cold reads (security,
  crediting the facts block); 102.9k / 2 turns (correctness, all six
  lenses on that review under cap); ~145k / 3 turns (two lenses on one
  review); 180.5k / 4 turns (correctness); 202.3k (correctness /
  move-fidelity, about a quarter of the completeness lens on the same
  PR); and a style lens at 81.7k / 2 turns / 1 tool call, below the
  85.8k this skill used to name as its best, distinguished only by
  receiving its comparison files and their excerpts inline. Two lenses
  whose Agent results read ≈102k and ≈104k had per-turn input summing
  to 911.6k and 604.3k. The divergence used to read "roughly an order
  of magnitude" unconditionally; measured at 1.7–2.5× on 2–3-turn
  lenses.
- **Yield gating.** Yield is a second axis beside the diff's shape, and
  it reaches what prompt discipline cannot: one pass ran the fan-out at
  ~95% of session cost while fully compliant with every tightening rule
  in the step.
- **Resumed lenses.** Two lenses overran a stated 5-turn hard stop (9
  and 8 turns) only because they were resumed; the caps held
  everywhere else (correctness 141.8k/5, cross-check 269.1k/5). The
  resume demanding "report now, at most 2 more tool calls" got both
  back in 0.
- **Checks to run.** A security lens's most valuable output was not a
  finding: it flagged that it could not verify whether a reader in
  another language re-derived the same gate, and one main-loop grep
  resolved it (the TS reader already handles the sentinel, so the demo
  UI renders no ladder for a dark vault). Re-shaping (session
  dacc811a, PR #347): the run's largest result (≈5.0k) was a
  `search_source.py --context 3` settling one inherited yes/no
  question that `--files-only` plus one slice-read answers for a
  fraction — the third session in a row for that lever. A lens's
  "verify urllib's redirect scheme allowlist", run as posed, meant
  reading CPython; re-shaped, it was one introspection call.
- **529 fan-out failures.** Upstream 529s took out eight spawns across
  two attempts in one review and six across ~15 minutes in another,
  and the response was re-derived under pressure both times. Token cost
  was near zero (a spawn dying before its first turn records 0 input).
- **Overlap invalidation.** A five-lens pass (~1.23M input, ~4.36M
  total sub-agent input for the session) was invalidated by an
  overlapping PR landing mid-review, an overlap written into the issue
  days earlier. A `gh pr list --json files` costs ~4.0k for the
  two-line answer the tool returns.
- **Late amendment.** A planning session sent a rewrite of a change's
  core query after the five-lens fan-out and the cross-check had both
  finished. It was correct and landed, but cost a sixth sub-agent
  (316.5k / 5 turns), two more full re-verify cycles and a lint failure
  cycle each — ~13% of a 2.41M fan-out across seven agents, spent on
  arrival order. The scoped re-review of only the amendment worked.

## Step 5: tiering and scope gates

- **Trivial-diff spend.** The full set plus cross-check earned its keep
  on substantial diffs (PRs #178, #184). On trivial ones it re-read the
  same few lines for nothing: a 4-line reword spawned a 70.4k-input
  agent, a 3-file doc-only diff a 375.4k one, and a 24-line infra diff
  four agents including a 277.8k cross-check for a single nit.
- **Reduced-fan-out band.** Large diffs with narrow risk drew the full
  multi-lens spend and returned only nits, each ≈0.5M–2.3M combined
  sub-agent input (PRs #202/#203/#204/#206/#210). The move-refactor case
  was written so a 1000-line extraction reaches the same scale-down the
  small single-crate tier already gives such diffs (~196k–240k) rather
  than the full 4-lens spend.
- **Straggler grep into build output.** A straggler check aimed at
  `decks/` with a bare recursive shell `grep` matched a minified webpack
  chunk under the gitignored `decks/.next/` and returned a ≈5.1k grammar
  blob — 48% of that session's entire Bash spend, from one call — for a
  question whose answer was zero hits.
- **Reduced tiers that held.** Every "spend less" rule can be misread as
  "and skip the inlined excerpts too", which inverts the result: the
  excerpts are what let a smaller lens set reach the same verdict. Five
  measured cases, the excerpt discipline fully applied. Two lenses at
  ≈569.4k combined (against ≈1.6M–2.8M for a full tier) still caught a
  real warning. The one-lens trivial short-circuit caught a blocking
  self-contradiction. A move-refactor tier at 503.5k combined (against
  ≈0.8M–2.8M for full tiers) — two lenses, no cross-check, freshness
  gated off, excerpts inlined and greps hoisted — landed both lenses
  inside cap (5 and 4 against 6) and returned a warning plus five nits
  with zero blocking misses. The small single-crate tier on a 264-line
  one-crate diff cost only the main loop adjudicating one lens's false
  positive (≈1.5k across three calls), evidence its cross-check skip is
  correctly placed, not merely cheap. And a three-lens tier where all
  three independently found the same real defect and two the same test
  gap: that convergence made three sufficient without a cross-check,
  and is the signal to look for when judging whether a reduced tier
  held. The rules compose: the cheapest lens measured (85.8k / 2 turns /
  1 tool call) came from a full tier with excerpts inlined.
- **Small single-crate escalations.** Small, self-contained one-crate
  diffs, even at a couple hundred lines, repeatedly drew the full 4-lens
  plus cross-check fan-out (~0.8M–1.4M sub-agent input) only to surface
  nits the lint pass already owned.
- **Docs-path crate escalation.** The multi-crate trigger once weighed a
  second crate's presence, so a seven-line docs-path change in a second
  tree escalated a diff to the full fan-out on its own; `review_diff.py`
  then gained `code_crates` and a per-crate `crates` rollup of source /
  docs / tests lines. That doc fix was itself a real finding from an
  earlier PR gone stale, which is why the docs and completeness lenses
  still see such a change. The rule once said "doc-comment fix", which
  the path-based bound contradicts: a reader took the example as
  license, then found the tool disagreeing.
- **Security lens on tooling.** On consensus-critical handlers,
  deserialization, auth or external I/O the lens earns its ~195k. On
  host tooling it is near-pure fixed cost: one run spent 148.8k only to
  flag a pre-existing non-saturating `pow` on trusted decimals — also
  the case for the standing suppressions, which drop such noise before
  the cross-check has to refute it.
- **Off-tier security lenses.** A meta-work diff, whose tier skips
  security, carried a permission-allowlist writer — real logic mutating
  what commands may run; added back per the Python-logic carve-out, the
  lens returned the sharpest finding of the review. Another security
  lens returned no findings and so independently validated an economic
  invariant: refusing a leg can never flip to favor the vault.
- **Freshness lens yield.** On a pure source diff the two freshness
  lenses near-always return "in sync, no-op", yet each costs ~100k+ of
  sub-agent input. The new-tree trigger exists because a new subsystem
  (a new `indexer/` tree the audit registry must learn about) is source
  that does need the conventions lens.
- **The prose-heavy gate.** Measured on the ENG-1279 review, 2026-09-10:
  the diff tripped no path trigger, so both freshness lenses were
  skipped, yet roughly 60% of its added lines were new Rust doc-comment
  prose, and both genuine defects found were comment-accuracy defects
  in that prose (a false claim about where all eight roster constants
  live, and a self-contradicting venue comment). Correctness,
  completeness and style looked straight past them; only the
  cross-check caught them, and flagged the gate as the cause. The
  20-line floor keeps a three-line diff that adds two comments from
  buying a ~100k lens. The crate rollup accepts the same blind spot (a
  doc-comment-only `.rs` change reads as source) and is right to,
  because over-counting crates is safe; skipping the lens that reads
  the prose is not.
- **Broad-scan sweeps.** The freshness / conventions / completeness
  lenses repeatedly dominated sub-agent input (≈471k, ≈627k, and one
  ≈5.4M / 71-turn run) by re-reading the whole of `CLAUDE.md`, the
  convention docs and `test.yml` and re-running repo-wide greps. "No
  convention governs this" is a negative, and establishing a negative
  is what sends a lens sweeping: a one-line finding in the main loop, a
  six-figure sweep in a sub-agent, and no two sections exist for the cap
  to name. Three sessions blew a stated freshness cap on open-ended
  sweeps (648.3k / 9 turns, 826.9k / 15 turns, 2.0M / 19 turns) while
  lenses handed excerpts came in 3–4× cheaper. A positive scope ("read
  these named files") reads as a grant: one brief was written as "you
  may open at most TWO of the named convention files", a permission.
- **Audit-registry inlining.** A conventions-freshness lens spent ≈578k
  adjudicating a verdict that turned on two registry lines; it caught a
  half-stale entry the diff introduced, so this is scoping, not
  skipping. Same review, model and diff: correctness and style with
  excerpts inlined at 106.7k / 2 turns each, and the freshness lens
  given file names at 336.2k / 6 turns, at cap (3.2×), spent grepping
  the registry and slice-reading two regions to find one `ci-infra`
  root line. Its finding was real (`.dockerignore` falls outside every
  subsystem root). The rule said "inline the implicated block" without
  saying who finds it, which is what made it aspirational.
- **Known negatives.** A completeness lens spent tool calls establishing
  that a config struct had no `validate()` or clamp path the new field
  could skip — a fact the main loop knew from writing the change. The
  credential negative came from a security lens on an infra diff that
  was right to run and returned a clean verdict on a genuine trust
  boundary; it is input scoping, not lens-count scoping.
- **Hoisted sweeps.** One run briefed a lens to confirm each new
  dictionary word appears in ≥ 2 files — a repo-wide census in a diff
  review's clothing. Hoisting without narrowing made one session's
  single largest result (≈4.2k): "are these 7 moved symbols still
  referenced?" came back as ~130 match lines, one file repeating one
  constant 40 times. PR #396: a hoisted `DATABASE_URL` sweep returned 32
  matches across 13 files at ≈868 tokens to establish six file paths,
  then compressed by hand; that session's other sweeps were shaped
  right (≈200 tokens for a `--files-only` existence question). The
  default exists because the rule was stated in three places and lapsed
  at a fourth. Other measured shapes: `--dir docs --ext md` returned
  ~200 headings across 18 files (3.0k) for three named docs; a
  `--context 3` sweep with 21 matches in one file bought it twice
  (≈3.1k); a removal sweep at `--context 4` returned 8 matches across 7
  files (≈1.7k) and then all seven were opened anyway; seven sessions
  answered a location question with a context sweep, one paying ≈3.6k
  for a three-line function, several noting the rule read as belonging
  to this step; a refused sweep was never retried and its question — a
  page's time budget contradicting the ledger by four seconds — was the
  review's most consequential finding; and a host-side sweep for the
  bot's `leader` keypair got 92 matches across 36 files, mostly
  on-chain state.
- **Consumers never hoisted.** One run inlined the two predicates the diff
  turns on (`has_valid_reference_price`, `is_matchable`) and nothing for
  the program-side gate consumers, so the lens cold-read `swap.rs`,
  `deposit.rs`, `set_liquidity_profile.rs` and `errors.rs` across ~5
  extra turns. Not waste: those reads produced the review's most
  valuable output, an outside-depositor constraint
  (`ReferencePriceNotSet`) no other lens found.
- **Convention by analogy.** On a removal diff the freshness lens read
  `-` lines as still present and returned false "stale doc" findings
  the cross-check had to refute. A cross-check flagged a `WARNING`
  comment prefix as a leaked review artifact, by analogy from the real
  ban on `ENG-###` / TODO refs in comments; one grep showed the
  `WARNING 1a`, `1d` and `1e` labels established, pre-existing and
  untouched.
- **Straggler sweeps.** Two real bugs were each found by one main-loop
  grep and by no lens: a new guard label (`WARNING 1e`) that already
  existed in another file for an unrelated invariant, missed by all five
  lenses; and a predicate hoisted out of four open-coded sites with a
  fifth, in a crate the diff didn't touch, left behind — a
  silently-diverging copy of a consensus-critical matching gate. An
  unscoped hoisted grep once returned the regenerated SDK surface (a
  658-line generated instruction file) that no lens needed.
- **Re-deriving compile facts.** The completeness lens (815.3k on one
  PR) and the cross-check (631.0k on another) ballooned by cold-reading
  transitive dependency files to re-derive what a green clippy run
  already proved.
- **Speculative findings.** The pre-emit gate kills the speculative
  findings the cross-check would otherwise refute, in ~676.7k
  re-derivation rounds.
- **Healthy while broken.** All three blocking defects in one
  observability review were this species: an action string written by
  no code path while four readers matched on it; two of six reachable
  regime/health pairs unmapped and falling through to the base style,
  one the standing condition for most of the roster; a paused market
  that quotes nothing, goes dark, and trips no rule. Twice more outside
  the diff: a merge-queue watcher looping on a quoting error looks like
  one patiently waiting, and alert rules that correctly treat no-data as
  healthy prove nothing until rows are known to arrive.

## Step 5: Exemplars — measured cases where a rule paid off

Recorded because trim proposals arrive continuously and a rule that is
*working* leaves no evidence of its own. Six sessions volunteered a
measurement to protect something and had nowhere to put it; this is
that place. A future trim pass has to argue against these figures rather
than against silence.

- **The excerpt rule delivering.** Lens briefs that inlined their
  excerpts instead of naming files held their turn counts across several
  runs, and produced the cheapest lens measured here (85.8k / 2 turns /
  1 tool call). One session noted its own exemplar figures may be
  *understated*.

- **A reduced-tier fan-out staying inside its cap.** 190.6k over 4
  turns, and 217.2k over 5 — both under cap, on a reduced tier, with the
  per-lens discipline held tighter rather than looser.

- **The cross-check earning an overrun.** The adversarial pass overran
  its cap and caught **two blocking data-loss bugs** that every cheaper
  tier would have missed. It is the pass that overruns most often *and*
  returns the most value per turn — which is why its cap is deliberately
  higher than the primary lenses', not lower.

- **The two-write Linear floor.** Sessions that hit the documented
  per-issue write floor recorded it as compliant but still non-trivial —
  the floor is real, and the cost that remains under it is not a lapse.

- **Whole-file Reads that were correct.** One session's whole-file
  Reads were *sanctioned*, not a slip: it both edited those files and
  pasted their excerpts into five lens briefs, so the read was paid once
  and amortized five times. That is the documented exception, and it
  held.

- **A four-lens fan-out, every lens under cap.** Zero overruns, measured
  as per-turn input (the `session-metrics` rollup figure — *not* the
  Agent tool's `subagent_tokens`, which reads about an order of
  magnitude lower):

  | lens                    | turns | cap | ≈input |
  | ----------------------- | ----- | --- | ------ |
  | Correctness             | 5     | 5   | 265.3k |
  | Security                | 4     | 5   | 210.6k |
  | Adversarial cross-check | 3     | 6   | 152.9k |
  | Completeness            | 3     | 5   | 144.8k |

  What produced it: an established-facts block composed for the run and
  passed via `lens_preamble.py --facts-file` — **92 facts**, carrying
  exact code excerpts, the consumer map with line numbers, the CI path
  inventory, and (the part that appears to do the work) the explicit
  **negatives and limits**, closing with a section naming what the main
  loop had *not* verified and leaving those open. The completeness lens
  opened its report with "adjudicating from the diff plus the
  established facts; no code re-derivation needed" and made **zero cold
  reads**.

  Read the priciest lens correctly: Correctness at 265.3k was the only
  one that cold-read source, and it did so because its brief asked two
  questions the facts deliberately did not answer — and it returned the
  review's blocking finding. It was priced by its *unanswered
  questions*, not by brief slippage, and a future trim pass would
  otherwise misread that as an overrun to cap harder. One honest caveat:
  the diff was docs-only and 301 lines, so the absolute figures do not
  transfer to a large source diff. What generalizes is the **ratio** —
  lenses given complete facts made no cold reads at all, and the one
  given genuine open questions spent its turns on exactly those.

- **A second facts-block fan-out holding under cap, with its
  mutation-verify cost.** Recorded because a rule that is working leaves
  no evidence of its own, so the only way the facts-block discipline
  accumulates a defense is for each run that holds to be written down.
  The pairing matters as much as the figures: composing and verifying
  the facts block is not free, and a future trim pass eyeing that cost
  should have to argue against the lens figures it bought rather than
  treating the spend as unexplained overhead.

  This is a **protective** entry, not a trim — it proposes no change. If
  a later pass wants to cut the facts-block work, the burden it has to
  meet is naming which lens figure it expects to move.

- **A FIVE-lens fan-out with zero overruns, on a diff that found two
  blocking defects.** Every lens came in at or under its turn cap. The
  one thing done differently was a **40-fact** established-facts block
  passed via `lens_preamble.py --facts-file`. Recorded so a future trim
  pass has to argue against these figures rather than against silence —
  and note this is the second independent run where a facts block is
  what held the caps, which is the pattern rather than a single lucky
  pass.

  Protective, proposing no change. The burden on a pass that wants to
  cut the facts-block work is to name which lens figure it expects to
  move.

- **The zero-cold-read shape, stated as a target.** Across the runs
  above, the lenses handed complete facts made **no cold reads at all**
  — one opened its report saying so explicitly — and the expensive lens
  in each run was priced by its genuinely *unanswered* questions rather
  than by brief slippage. That is the ratio worth aiming at and the one
  a trim pass most easily misreads: a lens that cold-read source is not
  evidence the brief failed if its brief deliberately left a question
  open.

When proposing a trim against any of these, say which figure you expect
to move and by how much.
