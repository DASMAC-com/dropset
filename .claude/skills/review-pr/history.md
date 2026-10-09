# `review-pr` history ledger

The measured incidents behind the rules in [`SKILL.md`](SKILL.md),
kept here so the entry file states each rule once. Provenance only;
not loaded on invocation. The entry file grew 60k → 84k → 212k → 292k
bytes from July to October across 47 commits with nothing pushing back
(measured by the design thread that filed the size gate), which is what
the resident-size gate (`docs/conventions/context-economy.md` →
"Resident size has a hard cap") was built to stop.

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
  rules and no suppression list. Per-lens files: three of four spawns
  in another review died on an upstream 529 before a single turn, and
  each retry re-sent the full ≈6k inline brief for zero work.
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
  lenses. Yield gating came from a pass running the fan-out at ~95% of
  session cost while fully compliant.
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
