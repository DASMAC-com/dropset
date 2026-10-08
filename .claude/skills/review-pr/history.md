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
  re-ran the full suite each time, twice provably redundantly. The
  committed reporter's first version then read the tip with
  `git rev-parse origin/<base>` "before the fetch" and got it wrong on
  its own first run: worktrees share one `.git`, so a sibling's fetch (or this
  session's `init-pr` fetch hours earlier) had already advanced the
  ref, and the tool reported a 0-commit delta for a base that had
  demonstrably moved. The merge-base is correct regardless of who
  fetched when.
- **Empty-overlap re-runs.** On one docs-only PR the base moved four
  times with an empty overlap every time, and the session still paid
  three `review_diff.py` re-runs and two full lints.
- **Lint-config base delta.** A mid-review rebase pulled in a 42-file
  delta that added a `rustdoc` hook to the lint config while the branch
  touched only `decks/**`: the overlap was empty and the rule as written
  said assert. That run re-ran lint on judgement and passed; had it not,
  the PR would have gone to CI red with a local green in hand. It is a
  correctness carve-out to a skip rule, the same shape as the
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
  (≈675 tokens; four runs ≈2k). What finally moved the reflex was
  `lint_paths.py --changed`: the full sweep needed no arguments while
  the scoped one needed a hand-built file list.
- **Serial spelling rounds.** Session a252a9d3 (PR #391): five Rust
  crates plus one doc, adding several hundred lines of doc comments (a
  module header, seventeen documented public constants, a new error enum
  with per-variant docs). It tripped cspell on two full runs — three
  unknown words, then two, one British — so two of its five
  `make lint` calls were spelling alone; it skipped the pre-flight
  because the diff did not look like prose. Others: two rounds of three
  British `-our` / `-ise` variants each; four words then two more, all
  six authored by the session and three of the first four British;
  three serial rounds for three words; 26 `make lint` runs (~12.8k of
  failure tails) on one prose-heavy change. Coinages: four rounds on one
  change, every word a self-inflicted verb form, each reword producing
  the next round's word (round trips, not context — the runs were
  wrapped). Three British words tripped the hook while the rule itself
  was being written.
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
  rounds, including after comment-only changes. After grep, it was the
  top repeated shape in both of those sessions. The narrow
  `tools-tests` form cost 32 calls / ≈7.1k against 15 calls / 516
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
