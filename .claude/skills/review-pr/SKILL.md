---
name: review-pr
description: Adversarial pre-review — mark the Linear issue In Progress on invocation, verify its checklist is fully addressed, lint, catalogue issues, fix what's mechanical, ready the PR, wait for GitHub CI to pass, print the review summary, then at the merge-queue handoff re-check the PR still merges cleanly before moving the issue In Review and offering to enqueue the PR, then watch the queue itself through to merged or dequeued and, on a merge, run the post-merge tail — session metrics filed unprompted, then one mark-Done-and-close question.
user-invocable: true
---

<!-- cspell:word unstarted -->

<!-- cspell:word pathspec -->

<!-- cspell:word retarget -->

<!-- cspell:word sortedness -->

# `review-pr`

Review adversarially before the human does: lint, audit the diff,
catalogue every issue, fix what is mechanical, mark the PR ready only
when clean, wait for green CI, print the summary. Run it once the work
is committed and pushed. The Linear issue goes **In Progress** on
invocation (even from In Review) and **In Review** only at the
merge-queue handoff.

**Transport.** GitHub goes through the MCP (`owner: "DASMAC-com"`,
`repo: "dropset"`) bar the `gh` exceptions in
`docs/conventions/github-mcp.md`: the merge-queue enqueue and dequeue
probe, and the repeated `gh pr checks` / field-selected `gh pr view`
reads.

## The entry gate: one opt-down question

Step 0, and the only interactive gate before the merge-queue handoff.
**Invoking this skill pre-approves the regular sub-agent fan-out**, the
same way invoking `audit-scope` authorizes its own
(`docs/conventions/decision-classification.md`). No spawn question is
asked, here or later. The one `AskUserQuestion` exists only so the
operator can opt **down** to an abridged tier (`history.md` → "Fan-out
without a yes"); reached through `init-pr`'s handoff, that question is
this gate.

Scope the tier from `review_diff.py`'s verdict, not vibes: files
changed, total lines, whether it touches program code, the SDK surface,
migrations or generation inputs, and whether it spans crates.

- **The default is the regular tier**: the full adversarial suite,
  offered first and labeled recommended, never inline, never silently
  reduced. An unanswered gate proceeds at the regular tier; it does not
  wait.
- **Offer the abridged tier only under the small-diff threshold**: ≤ 5
  files, ≤ 60 changed lines, none of those four surfaces, one crate. It
  is a starting calibration (drawn from a four-line single-file fix
  whose full suite the operator had to talk down), so state the signals
  in the question. Above the threshold there is nothing to offer, so
  ask nothing and proceed.
- The abridged tier still runs at least one independent sub-agent;
  zero-agent inline is a stop condition (step 5), not a tier.
- Tier scoping changes how many lenses run on a trivially small diff.
  It never trims the cross-check's depth or turn cap on an ordinary one
  — a proposal to trim that cap was rejected (closed record under the
  `Trim levers` milestone).

## Steps

1. **Locate the PR.**

   ```sh
   gh pr view --json number,title,state,isDraft,baseRefName
   ```

   No PR (`gh` exits non-zero, "no pull requests found") → stop and
   tell the user to run `/init-pr`. **`baseRefName` is `<base>`** in
   every base-relative step below (`origin/<base>`): `main` normally,
   another PR's branch when stacked.

   **If `<base>` is not `main`, check whether it already merged —
   before anything expensive:**

   ```sh
   gh pr view <baseRefName> --json number,state,mergedAt
   ```

   On `MERGED` (almost certainly squashed, so it no longer shares this
   branch's ancestry), retarget — now, or whenever it merges
   mid-review: `mcp__github__update_pull_request` with `base: "main"`,
   `git rebase --onto origin/main <old-base>`, and `main` as `<base>`
   from then on, never a per-command patch. Step 2's rebase and step 5's
   `base_fresh` gate both miss this: they compare against a fresh
   `origin/<base>`, and it is the base itself that is stale. (A review
   that missed it went green and turned `CONFLICTING` only at step 16,
   costing ≈25 min of rebase and full re-runs; `history.md` → "Merged
   stacked base".)

1. **Clean tree, then rebase onto the PR's base.** If `git status`
   shows uncommitted changes, stop and tell the user to commit (or
   `/commit-changes`). Capture the merge-base **before** fetching (the
   triage needs it), then rebase:

   ```sh
   git merge-base HEAD origin/<base>
   ```

   ```sh
   git fetch origin <base>
   ```

   **Prove signing first if the session has idled since its last signed
   commit** — here, and at every later re-rebase (steps 5, 6 and 11)
   after a fan-out, CI wait or pause. With `commit.gpgsign` set (it is
   here), a rebase re-signs every commit it replays, so a locked signer
   kills it part-way and leaves the worktree mid-rebase (`history.md` →
   "Signer locked mid-rebase"). A dangling signed commit proves the
   path and moves no ref; on failure, stop and ask for the unlock:

   ```sh
   git commit-tree -S -p HEAD -m signing-probe HEAD^{tree}
   ```

   Then rebase:

   ```sh
   git rebase origin/<base>
   ```

   - **On conflict: `git rebase --abort`, catalogue it as blocking
     (step 7), and tell the user to resolve and re-run. Never
     auto-resolve** — not even an adjacent insertion into a sorted
     list: a linter verifies sortedness, not completeness, and a list
     that silently lost one side lints clean. `merge_completeness.py`
     makes a *manual* resolution auditable (per-side survival of each
     parent's added lines); it never licenses an automatic one, since a
     resolution can keep every line and still be wrong. Any future
     carve-out must check completeness against both merge parents
     mechanically, never "the linter passed" (`history.md` →
     "Merge-completeness evidence"):

     ```sh
     python3 .claude/tools/merge_completeness.py --path <file>
     ```

     Remove collision classes at the source where a layout allows
     (per-target `.PHONY`, the dictionary's `merge=union`); a single
     ordered file, such as yamllint's sorted keys, stays manual. List
     the conflicted files with `git diff --name-only --diff-filter=U` —
     never a repo-wide `grep '<<<<<<<'`, which is gitignore-blind — and
     inspect each hunk with `Read` `offset` / `limit`, not a
     context-flagged `grep` (`history.md` → "Conflict inspection").

   - **A clean rebase that integrated base commits** can still hide a
     *semantic* conflict (the base renamed something this branch
     calls): flag it for steps 5 and 11. History was rewritten, so step
     12 force-pushes with `--force-with-lease`.

   **Triage what the base gained**, from the merge-base captured above
   — never `git rev-parse origin/<base>`, a shared ref another session's
   fetch may already have advanced, which reports a false 0-commit
   all-clear (`history.md` → "Merge-base, not the tip"):

   ```sh
   python3 .claude/tools/rebase_overlap.py --from <mb> --to origin/<base>
   ```

   It prints the base delta's commits and `base_files`, this branch's
   own files, their **overlap**, and the `runs_artifact_gates` /
   `runs_rust_suites` predicates steps 9 and 11 use to decide what a
   rebase-forced re-run may skip (`history.md` → "Hand-rolled rebase
   triage"). This step's rebase and every freshness gate always run;
   only the re-runs vary:

   - **Empty overlap → assert, don't re-run.** `review_diff.py` and
     `make lint` provably cannot change their answer on files the delta
     did not touch. Note "base moved to `<sha>`; overlap empty; lint
     and suite results from `<sha>` still stand" and move on. Any
     non-empty overlap re-runs everything (`history.md` →
     "Empty-overlap re-runs").
   - **Unless `base_files` includes `cfg/pre-commit-lint.yml` or
     `.pre-commit-config.yaml`** — the base changed *which checks run*,
     so re-run `make lint` whatever the overlap (`history.md` →
     "Lint-config base delta").
   - **`base_files` touches `programs/**`** → run
     `python3 .claude/tools/run_quiet.py -- make program` before any
     suite, or a scoped `cargo test` runs the stale `.so` and its
     failures read as your own regressions — the mirror of step 11's
     `test-no-teardown` trap (`history.md` → "Stale program after
     rebase").
   - **`base_files` includes `pnpm-lock.yaml` or a
     `frontend/package*.json`** → run
     `python3 .claude/tools/run_quiet.py -- pnpm --dir frontend install`
     before any typecheck or lint. An install earlier in the session
     does not count (`history.md` → "Stale node_modules after rebase").
   - **A hot surface** (overlap with several open PRs, per
     `review_diff.py --overlap`; agent-infra is one) predicts repeated
     rebases and re-runs. Advisory, for a narrower *next* PR; the signal
     is overlap, not size (`history.md` → "Hot-surface tails").

   **Key a skip to content, not a SHA** (an amend or no-overlap rebase
   moves the SHA, not the bytes): record after a green `make lint`, check before
   any re-run:

   ```sh
   python3 .claude/tools/tree_fingerprint.py record --check lint
   ```

   ```sh
   python3 .claude/tools/tree_fingerprint.py check --check lint
   ```

   `fresh` (exit 0) → assert and skip; `stale` → re-run; `missing` →
   run and record (three-way: a boolean would re-run `missing`). Use it
   for `tools-tests` and the artifact gates too, any check that is a
   function of tree content alone; it says nothing about CI, the queue
   or a live service.

1. **Check the Linear task, mark it In Progress, and tick what's
   done.** Autonomous runs ship partial checklists; confirm the diff
   delivers all of it before reviewing anything else.

   - **Resolve the tag** from the PR title scope (`type(ENG-###): …`),
     falling back to the branch name (the two share one number). If
     neither yields one, skip this step and say so in the report.

   - **Hold the body once per session.** If `init-pr` bootstrapped this
     session, work from the body it already has. Invoked cold, call
     `mcp__claude_ai_Linear__get_issue` once, plus `list_comments`
     (criteria sometimes live in an anchored comment). The echo budget
     is per issue, per session, across skills
     (`docs/conventions/linear-automation.md`; `history.md` →
     "Cross-skill echo budget").

   - **Grade every requirement** — checkboxes (an `[x]` is a claim to
     verify) and requirement-phrased scope bullets — from
     `git diff origin/<base>..HEAD` and the commits, as **addressed**,
     **partial** or **missing**. A deferral is addressed only if it is
     recorded (a commit, a PR-body note, a `/linear-task` follow-up);
     a silent omission is missing.

   - **At most one write**: the addressed ticks together with
     `state: "In Progress"`, ticks as one `replace` op per box:

     ```txt
     mcp__claude_ai_Linear__save_issue(
       id: "<ENG-###>",
       state: "In Progress",
       patch: [
         { op: "replace",
           old_string: "- [ ] <the box's exact text>",
           new_string: "- [x] <the box's exact text>" }
       ]
     )
     ```

     Set In Progress even when reclaiming from In Review; never regress
     Done / Canceled. Never invent a box for a non-checkbox
     requirement. Nothing to tick and already In Progress → write
     nothing. Every call echoes the full body whatever it sends, so no
     separate state write, no write per box; if a state echo comes back
     unchanged, verify once and report rather than retry.

   - **Patch cautions.** Each `old_string` must match exactly once —
     extend a non-unique box with surrounding text. A box holding an
     `ENG-###` never matches (Linear stores a mention node), and `patch`
     excludes `description`, so if any box is tag-bearing send the whole
     call as full-body `description` + `state`. The save is atomic: one
     stale anchor aborts the state move too, and the error names only
     the first failing op. Then re-`get_issue` once (the one licensed
     re-fetch), rebuild the ops, and write once more — never rebuild
     from the stale snapshot, which would overwrite whatever diverged.
     A second abort → report it and leave the issue alone.

   - **Ticks that must wait for the fan-out ride step 20's In Review
     write**, in its `patch` — never a save of their own (`history.md`
     → "Deferred-tick echoes").

   - **A body addition that ticks nothing** — a deferral note, a
     remaining-scope line — is an **append**, and goes through the
     zero-echo writer, never `save_issue`:

     ```sh
     python3 .claude/tools/linear_issue.py append --id <ENG-###> \
       --file <scratchpad>/note.md
     ```

     An append has no anchor, so `patch`'s atomic abort protects
     nothing and its full-body echo buys nothing; **anchored** edits
     (the ticks above) stay on `patch`. On a growing body each echo is
     larger than the last — four MCP body writes to one issue once cost
     ≈20k, a fifth of that session's tool-result spend.

   - Catalogue every **partial** or **missing** requirement as
     **blocking** (step 7), quoting the checklist text and the deciding
     `file:line` or absence.

   - **"This will not fit in one PR" is not a second PR.** When the
     missing scope would carry the diff past the split line
     (`docs/conventions/context-economy.md`), **never open a second PR
     on the same issue**. Finish this PR as a self-contained unit,
     **message the live planning session** the remainder split into
     pullable units, and end the session at this PR. Planning files
     the follow-ups and re-points the edges to the chain's tail
     (`docs/conventions/commits-and-prs.md`). Record the split as a
     PR-body deferral, which makes it addressed.

1. **Run lint**, always through the quiet runner — the cspell hook alone
   dumps a ~450-line cascade on failure:

   ```sh
   python3 .claude/tools/run_quiet.py -- make lint
   ```

   **The full sweep runs twice: at the start and at the end.** Every
   check in between is scoped, through one bare command:

   ```sh
   python3 .claude/tools/run_quiet.py -- \
     python3 .claude/tools/lint_paths.py --changed
   ```

   That runs every hook over this branch's files (the merge-base diff
   plus untracked paths); append `-- <hook-id>` to narrow it. For paths
   narrower than the branch, wrap
   `pre-commit run [<hook-id>] --config cfg/pre-commit-lint.yml --files <paths>`
   (`--config` is mandatory; the default name fails with
   `InvalidConfigError`). A full sweep in between is a mistake unless a
   hook is repo-global or a scoped run would not be representative
   (`history.md` → "Full-sweep loops").

   **Spelling pre-flight before the first full run** whenever the diff
   adds any comment, doc comment or markdown. The trigger is the SHAPE
   of what the diff adds, not the file extension, so a doc-heavy `.rs`
   change counts:

   ```sh
   python3 .claude/tools/run_quiet.py -- \
     python3 .claude/tools/lint_paths.py --prose
   ```

   Take the whole unknown-word list in one pass (`history.md` →
   "Serial spelling rounds"), then:

   - The dictionary is **US spelling**: write the US form of an `-our`
     / `-ise` variant, never add the variant.
   - A word you **coined** → reword once, to plain English with no
     coined morphology; a neighboring coinage re-enters the loop.
   - Place a real term per `docs/conventions/docs-and-style.md` →
     "Spelling (cspell)": ≥ 2 files → `cfg/dictionary.txt`; one file →
     a top-of-file `cspell:word` escape.

   **Fail-then-fix hooks are design, not findings.** `mdformat`,
   `markdownlint-fix`, `ruff format`, `biome` and the dictionary's
   `--unique` sorter (also tripped by a rebase that leaves the
   union-merged dictionary unsorted) rewrite the file and fail: stage, re-run
   **once** scoped, and treat only a second failure as real (e.g. an
   MD013 overlong line `mdformat` made by collapsing a wrap inside a
   code span). So an `.md` edit costs two scoped runs, never two full
   sweeps.

   **A formatter invalidates every line you hold for that file.** Order
   edits before it runs; after it, take one bounded read covering every
   remaining region once the session's reads pass about half the file.
   That budget is session-cumulative and an autofix does not reset it;
   to edit a markdown table, change every row
   in one `Edit`; `cat -n` is a whole-file read too
   (`docs/conventions/context-economy.md` → "The levers"; `history.md`
   → "Formatter re-read loop"). To see what a formatter changed, read
   the step-5 slice files, never `git diff` the file.

   **The `Lint` workflow is more than `make lint`**: it also runs
   `make tools-tests` and `make decks-build`, and a failure there is not
   a hook, so step 18's reproduce-the-hook advice does not apply. When
   the diff touches
   anything a `.claude/tools/` checker validates (`.claude/tools/**`
   itself, `market-data/grafana/**` via `grafana_check.py`), run
   `python3 .claude/tools/run_quiet.py -- make tools-tests` before
   pushing. That suite runs on Linux with a much newer Python, so a
   test shelling out to `zsh`, BSD `stat` or any macOS-only integration
   needs a `unittest.skipUnless` guard (`history.md` → "Lint workflow
   beyond the hooks"). A fast suite runs whole, never per module
   (`docs/conventions/context-economy.md` → "When a suite is fast
   enough to run whole").

   **Reading lint output and config:**

   - Filter a captured log, never `Read` it whole:
     `python3 .claude/tools/run_quiet.py inspect <log> --grep 'Failed|error\['`
     (with no flags it reprints the failure summary), or the Grep tool;
     a region by `Read` `offset` / `limit`. Never a shell `grep` /
     `tail` / `head` on it — the objection is context cost, since the
     shell prints the region into the result; `Bash(grep:*)` is already
     granted, so permissions are no argument (`history.md` → "Log
     filtering").
   - "Which hooks cover this file?" / "how does CI run X?" → grep
     `cfg/pre-commit-lint.yml` and the workflow (`id:`, `files:`,
     `types_or:`, the invocation). Both are indexes; never read either
     whole (`history.md` → "Config-index reads").
   - Never hand-verify what the gate owns — least of all an `awk`
     width check, which counts bytes (an em-dash is three).

   **Verify at checkpoints, matched to the edit.** An unchanged tree
   since the last lint → assert, don't re-run. A copy- or comment-only
   change needs lint, not a build or typecheck. `tsc --noEmit` is
   whole-project, so its lever is frequency: once, after the TypeScript
   edits are done (`history.md` → "Inner-loop checks").

   **On failure, separate three cases:**

   - **An autofix** — above.

   - **A missing binary** (`biome` / `tsc` report "Command … not found"
     without frontend `node_modules`) → run
     `python3 .claude/tools/run_quiet.py -- pnpm --dir frontend install`
     and re-run. If the install fails, the hook is unverifiable locally,
     like an absent Solana toolchain (steps 9–10): when the diff touches
     none of its files, note that CI runs it. Never gate the PR on a
     hook that couldn't run.

   - **A real violation** → fix everything mechanical (formatting,
     imports, whitespace, spelling), stage by explicit path and commit
     signed, then confirm with the scoped run:

     ```sh
     git add <fixed files...>
     ```

     ```sh
     git commit -S -m "Fix lint violations"
     ```

     Whatever still fails is **blocking** (step 7); don't mark the PR
     ready.

1. **Adversarial diff review.** Write the diff to **one file in the
   session scratchpad** and hand each agent a path, never N inlined
   copies (`CLAUDE.md` → "Context economy"). The scratchpad is shared
   with the sub-agents; `/tmp` is shared across *sessions*, so a
   sibling's stale `review-diff.txt` can sit at the same path and the
   fan-out reviews the wrong diff (`history.md` → "Diff hygiene").
   One `review_diff.py` call does the fetch, range checks, excluded
   diff and stat, and returns one verdict:

   - **Ref `origin/<base>...HEAD`, from the merge base** — the local
     base in a worktree is stale by already-merged PRs, so the tool
     fetches it itself; three dots keep a base that moves later out of
     the diff.
   - **Generated families excluded** by `:(exclude)` path patterns from
     the tool's `DIFF_EXCLUDES` (`--print-grep-excludes` lists them); step
     9 regenerates and gate-checks them; a family not yet listed gets the
     hand-written-slice rule below. The
     verdict's `files` list stays **unfiltered**, which is how step 9
     sees it must run.

   **Finish your own edits before generating slices.** Composing the
   briefs is when you notice your own stale prose: take one
   read-through for self-inflicted staleness and fix it in one pass
   first. A slice regenerated mid-composition invalidates every brief
   written against it, and a lens handed a stale slice fails
   **silently**.

   ```sh
   python3 .claude/tools/review_diff.py --base <base> --split \
     --out <scratchpad>/review-diff.txt
   ```

   ```json
   {
     "fetched": true,             // false if the fetch failed or was skipped
     "fetch_error": null,         // non-null → freshness is UNVERIFIED
     "base_fresh": true,          // false iff base_ahead is non-empty
     "base_ahead": [],            // commits the base has that HEAD lacks
     "base_drift": null,          // when stale: overlap, lint_config,
                                  //   generation_input, disjoint
     "commits": ["abc1234 Subject"],
     "diff_path": "…/review-diff.txt",
     "diff_lines": 1234,
     "diff_empty": false,
     "files": [{"path": "…", "changes": 12}],
     "runs_rust_suites": false,   // any path outside CI's code filter?
     "runs_artifact_gates": false,// any generation input touched?
     "ready": true,               // exactly `not blockers`
     "blockers": [],              // why ready is false, if it is
     "slices": {                  // only with --split
       "source": {"path": "…/review-diff-source.txt", "lines": 900},
       "tests":  {"path": "…/review-diff-tests.txt",  "lines": 300},
       "docs":   {"path": "…/review-diff-docs.txt",   "lines": 34}
     }
   }
   ```

   **`ready` is the gate: nothing is spawned until it is `true`.** It
   is defined as `not blockers`, and the tool exits non-zero when it is
   false. The four blockers: a stale base **whose delta interacts
   with the branch**; a **failed fetch**
   (freshness unverified — `--no-fetch` accepts the local ref
   deliberately); nothing changed; or every changed path is an
   excluded generated family (no source to review, though the step
   9/10 gates still apply) — the last two each with an `--only`
   variant. The base can advance *past* the step-2 rebase
   mid-review — worktrees share one `.git` — but the merge-base diff
   no longer shows it as a **phantom deletion** (`history.md` →
   "Phantom deletion"), so drift blocks only when `base_drift` says
   it interacts: a file both sides changed, the lint config, or a
   generation input. Then rebase onto `origin/<base>`, triage as in
   step 2, and re-run the tool. **A disjoint drift (`ready: true`,
   `base_fresh: false`) needs no rebase at all**: note "base moved;
   drift disjoint" and carry on — the merge queue tests the merged
   result. Read `files` for **sizing and tiering**, not as a second
   freshness check. Pass `commits` inline; the diff lives only in the
   file. A rebase this gate asks for takes the signing probe first
   (step 2).

   The tool **owns** three path lists — the generated-family excludes,
   the mirror of `test.yml`'s `code` filter, and the generation inputs
   — which is why steps 9 and 10 read `runs_rust_suites` and
   `runs_artifact_gates` off the verdict. Change them there, never in
   prose. `--out` is required (except with `--gate-only`, which
   writes no diff) but **not** validated against the scratchpad root,
   so substitute the real path (a guessed one is created, not
   rejected); the tool rewrites it every full run, owner-only
   (`0o600`).

   **Route by slice, and default each lens to exactly ONE**
   (`history.md` → "Slice routing"):

   | slice  | lenses                                                       |
   | ------ | ------------------------------------------------------------ |
   | source | correctness, security, style, completeness                   |
   | tests  | completeness; correctness when the diff changes pinned logic |
   | docs   | doc-freshness                                                |

   - **Each brief NAMES its slice**, and a lens scoped to one tree gets
     that tree's `--only` diff (a tools lens `--only '.claude/tools/**'`;
     a conventions lens `docs/conventions/**` plus `CLAUDE.md`).
   - **A second slice needs a one-line justification** in the brief —
     why its question spans the seam. Print each lens's **slice count
     and handed-in byte count** beside the brief, so an over-broad
     hand-in shows at fan-out time. This is scoping only; nothing here
     argues for fewer or shallower lenses.
   - **When generated output dominates the diff**, the completeness
     lens and the cross-check get the hand-written slices, not
     `diff_path` (`history.md` → "Generated output dominating the
     diff"). The step-6 cross-check otherwise gets the **full**
     `diff_path`: it is the one pass that sees everything.
   - **Rust inline tests are already extracted**: `--split` routes
     `#[cfg(test)]` hunks into the tests slice, so an empty tests slice
     on a Rust diff means something. An empty slice is still written,
     so absent never reads as "nothing here".
   - **Never tell a lens its slice holds a whole function.** A slice is
     hunks with N lines of context; say so, and inline any surrounding
     region the question needs (the excerpt rule below).

   **When a slice is still huge, split the slice — not the prompt.**
   **The ceiling is roughly 500 lines
   per lens slice**; past it, subdivide with `--only`, which takes
   repeatable path globs (never hand-rolled
   `git diff` calls) — by crate, by directory, or by the diff's own
   seam — running it once per sub-slice and giving each its own lens
   instance and `--out` path (`history.md` → "Slice size"):

   ```sh
   python3 .claude/tools/review_diff.py --base main \
     --out <scratchpad>/review-diff-programs.txt \
     --only 'programs/**' --split
   ```

   - **Decide PER LENS, not per fan-out.** The test: does this lens's
     question span the seam between crates, or sit inside one?
     Correctness and
     security usually need the cross-crate chain; style, naming and
     new-public-surface questions usually do not.
   - **A prose or cross-file-consistency lens usually keeps the whole
     slice** — splitting destroys the relation it checks. That
     exemption **lifts its turn cap with it**, roughly in proportion
     (≈3× the ceiling, ≈3× the turns), stated in the brief; otherwise
     it overruns by construction and a hard stop discards the findings
     least recoverable by a narrower re-run.
   - **The exemplar band shifts with size**: a 509-line slice
     fully disciplined costs ≈440k, not the ≈200k of the
     small-diff exemplars.
   - **Slices are namespaced off the `--out` stem**
     (`--out review-diff-tools.txt` writes `review-diff-tools-source.txt`
     and siblings), so each lens gets its own path and a scoped run
     cannot overwrite the unscoped set (`history.md` → "Fixed slice
     names").

   **Once written, the slice files ARE the diff — for the main loop
   too.** Do not re-derive their content with `git diff`; read the
   slice or `diff_path`. But a **per-file** question wants a per-file
   scope: `--only '<path>'` or a path-limited `git diff`, never a
   `^[+-]` grep over a whole slice, which re-buys every file in it
   (`history.md` → "Re-deriving the sliced diff").

   **The spawn is already authorized — do not ask again.** The
   invocation **is** the spawn approval (the entry gate only offers an
   opt-down), including under a standing "don't spawn agents unless
   asked" default; a mid-flight
   fan-out prompt is ceremony (`history.md` → "The second spawn
   question"). **The one hard gate is tooling**: if the harness offers
   no `Agent` tool, do not spawn, do not silently skip, and **do not
   substitute an inline pass** — stop, say so, and let the user
   decide. An inline lens shares the author's blind spots, so it is
   structurally a self-review at any diff size (`history.md` → "Why
   the inline path was removed").

   **Emit the standing preamble with the tool, never by hand.** The
   byte-identical standing half of every brief is written once and
   handed by path; the tool reads `docs/conventions/sub-agent-brief.md`
   in its own process so the skill reads nothing (`history.md` →
   "Preamble economics"). It reaches the step-6 cross-check too:

   ```sh
   python3 .claude/tools/lens_preamble.py \
     --out <scratchpad>/lens-preamble.md \
     --append .claude/skills/review-pr/lens-standing.md \
     --facts-file <scratchpad>/facts.md
   ```

   It composes the canonical shell rules with `lens-standing.md` beside
   this file — the scope and negative scope, the budget and hard stop,
   the pre-emit gate, the standing suppressions. **That template is the
   agent-facing wording; this step is its rationale.** When you change
   one, check the other still describes it.

   **The facts are the highest-leverage input in this step.** The tool
   refuses a run with neither facts (`--facts-file`, or the equivalent
   repeatable `--fact`) nor an explicit `--no-facts`. The
   excerpt rule covers what you have read; the facts cover **what you
   know isn't there** — "no test harness here", "zero call sites" — and
   a lens cannot tell "nobody told me" from "I had better go check"
   (`history.md` → "Facts-block evidence"). Compose
   `<scratchpad>/facts.md` from what **this** run verified, one claim
   per line, negatives included. Never paste a worked example: the
   block tells every lens its contents are binding, so a borrowed
   example injects *false* facts into the whole fan-out. `--no-facts`
   only states on the record that nothing was verified.

   - **A formula, expression or conversion states its width** (`u64`,
     `u128`, an `f64` intermediate), its **rounding direction**, and
     whether it **saturates or wraps**; an incomplete one sends the lens
     back to the source it was meant to settle.
   - **Quote an expression WHOLE, guard included**, or label it a
     paraphrase — a lens bound by the block cannot notice a dropped
     null guard, default or short-circuit.
   - **For a key, id or wire format, hoist both ends**: the writer-side
     constants that compose it, not only the reader-side schema.

   **Emit the preamble at the spawn gate; a missing file is a stop —
   re-emit it and re-check.** Run `lens_preamble.py` immediately before
   spawning, in the same gate that checks `review_diff.py`'s `ready`,
   and check the path in the same command sequence that spawns, so it
   fails loudly there — a session resume can strip a file emitted
   earlier, and the fan-out then runs with no shell rules and no
   suppressions while reading as normal (`history.md` → "Missing
   preamble").

   **The per-lens material travels by file too.** Write each lens's
   block — handler semantics, fixture helpers, known negatives,
   measured values — to `<scratchpad>/lens-<name>.md`, so a retry after
   a transient failure re-sends a path, not the brief (`history.md` →
   "Per-lens retries"). Keep inline only
   what identifies the lens — its dimension, scope line and cap:

   ```txt
   Read <scratchpad>/lens-preamble.md first — it is the
   standing brief for this review. Then read
   <scratchpad>/lens-<name>.md, which holds the material
   gathered for your lens. Then: <per-lens scope>
   ```

   **Excerpts inline: if the main loop has read it, the excerpt goes
   in the brief.** This is the single highest-value lever in the step,
   for **every** lens, and it is *input*-scoping, not fewer lenses
   (`history.md` → "Cold-reading held context"). A brief never names a
   file path the main loop could have quoted — reference and prior-art
   files included (the module the change imitates, the type it must
   stay compatible with), since a named path is an instruction to go
   read. Pass any file→symbol map the implement phase produced, to
   every lens reasoning about the surrounding code — on a concurrency-
   or invariant-heavy diff that map is what spares correctness a
   re-survey; when none exists, do not manufacture one. **Carry line
   numbers** with every excerpt: the pre-emit gate makes `file:line` a
   deliverable, so an excerpt without them makes a read mandatory.
   **Scope the whole-read license to the excerpt**: name the region you
   will excerpt before reading a file whole, and map-and-slice when it
   is a known section. When two files are near-identical in the region,
   read one and diff the other against it. Three lenses need it shaped:

   | lens      | hand it                                                                                                                        | never                                       |
   | --------- | ------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------- |
   | docs      | the docs slice plus a claim → excerpt table: the assertion, the 5–15 lines that settle it, a line-number column                | the source slices as "ground truth"         |
   | style     | the one or two sibling files that define the idiom, named **and their excerpts pasted**                                        | a discovery glob (`components/**`)          |
   | freshness | its named files (`CLAUDE.md`, the skill files, `test.yml`) as a positive scope, opened only through Read / Grep and slice-read | shell `grep` / `git grep`; whole-file Reads |

   **Bound every lens; the template carries the wording.** In the
   per-lens half, state:

   - **The scope line, verbatim**:
     *"adjudicate from the provided diff + excerpts; cold-read only a
     file no excerpt covers."* The freshness lenses get their positive
     scope (their named files) alongside the template's negative one.
   - **A cap in turns AND tool calls, called a hard stop** — *"≤ 4
     turns / ≤ 6 tool calls, hard stop"* — in **every** brief, not one
     lens's. A turn is not a unit an agent can count, and a soft "≈6"
     is read as a suggestion (`history.md` → "Hard-stop wording").
     Default to **4 turns**, or 6 for a lens holding a section map. A
     hard stop, the template's negative-scope line and its
     do-not-re-open negative bind the step-6 cross-check too, but at its
     own higher cap (step 6), never this one.
   - **At most three enumerated sub-questions.** The wording is
     necessary and not sufficient; sub-question count is where the
     remaining variance lives (`history.md` → "Sub-question count").
     Any sub-question the main loop can answer gets *answered* in the
     brief — a known answer is a fact with a question mark on it. More
     than three means you are surveying, or the lens should split.
   - **A cap the ask can satisfy.** When the question is genuinely "do
     these two large files agree in their entirety", either **hoist the
     extraction** into the main loop and hand over the pairs
     (preferred), or state the higher cap and why. Silently keeping ≈6
     is the one option always wrong.
   - **A self-report of the files it opened**, so an overrun is visible
     next time.
   - **The exemplar to match**: read once, adjudicate, report. The
     current targets — security **90.4k / 2 turns, zero cold reads**;
     correctness **102.9k / 2 turns**; style **81.7k / 2 turns / 1 tool
     call** — all credit facts and excerpts inlined (`history.md` → "Exemplar
     figures"). They are **summed per-turn input** from
     `session-metrics`' rollup, not the Agent tool's `subagent_tokens`;
     the gap grows with turns (≈1.7–2.5× at 2–3 turns, near 10× for
     long lenses).
   - **A closing "checks to run" list**: concerns it could not settle
     in scope, each phrased as the check that would. Without that slot
     a lens can only speculate or stay silent. Run them in the main
     loop before step 7 and fold each result into the catalogue as one
     line. **Re-shape an inherited check first**: the lens phrased it
     for adjudication, the main loop usually needs existence, and the
     right tool may differ (`history.md` → "Checks to run").

   **Gate a lens on its historical YIELD too.** A lens returning zero
   findings across **ten or more** dispatches is auto-skipped, and the
   skip is named in the summary. **Insurance lenses are exempt** —
   security canonically, and any lens whose absence would surface only
   in production (`history.md` → "Yield gating").

   **A resume is a report-now order.** A `SendMessage` resume re-sends
   the lens's whole context, so it carries **"report now, at most 2
   more tool calls"**, never an open continuation; a lens that cannot
   conclude under that is reported unfinished, not resumed again
   (`history.md` → "Resumed lenses").

   **On a transient fan-out failure, back off once, then probe with
   one lens.** On repeated upstream 529s, relaunch a single lens; if it
   returns, relaunch the rest; if the fan-out still fails, **park the
   review** — report the state, leave the PR as it is, and pick it up
   later. Never an
   inline pass, never the full batch on a loop. It is a wall-clock
   finding, not a token one (`history.md` → "529 fan-out failures").

   **Check for in-flight overlapping PRs BEFORE spending the
   fan-out.** The base-freshness gate is satisfied whenever the base has
   not moved, so an overlapping PR that **lands during the review**
   passes every check and still invalidates it (`history.md` →
   "Overlap invalidation"):

   ```sh
   python3 .claude/tools/review_diff.py --base main \
     --out <scratchpad>/review-diff.txt --split --overlap
   ```

   It intersects the open PRs' files in-process and returns only the
   overlap; it **reports, never
   blocks** (a missing or unauthenticated `gh` is noted). On a
   substantial overlap, put
   the decision before spawning — wait for the other PR, or proceed
   with the re-run cost recorded. **Gate a consolidation's straggler
   fixes on that report**: the last references to a renamed or retired
   thing are what an in-flight PR most likely reintroduces, so decide
   after reading it whether they are yours or the second-landing PR's.

   **Answer a peer's "do we collide?" from the slices, never
   `git diff`.** The question *what exactly did I change in this file*
   does not feel like re-deriving a diff, but it is: locate the file's
   hunks in the existing slice with
   `read_result.py --grep '<path>' <slice>` and `Read` that range, or
   take `review_diff.py --only '<path>'` with an `--out` stem of its
   own, so the full slices a lens may still be reading survive.
   Path-scoped `git diff` calls made after the slices existed once cost
   ≈2.7k for bytes already on disk.

   **Ask whether an AMENDMENT is pending before you spawn.** If a
   planning session, a peer or the operator has work queued for this
   branch, the fan-out is about to review a diff that changes;
   `ListAgents` shows whether a planning session is live, and one
   message costs a fraction of one lens. Fold the amendment in first,
   or spawn and say a re-spawn is coming. One that lands after the
   fan-out gets a **scoped re-review of just the amendment**, never a
   full re-fan-out (`history.md` → "Late amendment").

   **Scale the fan-out to the diff.** The full lens set below plus the
   step-6 cross-check is the right spend for a substantial diff with
   real new logic (a new instruction, a non-trivial refactor, new
   on-chain or SDK surface), and near-pure fixed cost on a **trivial**
   one (`history.md` → "Trivial-diff spend"). Size the diff from the
   verdict's `commits`, `diff_lines` and per-file `files`, check the
   trivial short-circuit first, then take the tier that fits; a diff
   confined to the meta-work surfaces never takes the infra / ops tier.
   When in doubt, fan out:
   the scale-downs are for the clearly narrow, not the
   merely-small-but-subtle. Name in the summary any tier below the full
   one and every lens a gate below skips.

   - **Trivial short-circuit — one scoped reviewer, no cross-check.**
     Small and confined to one of: comment / doc / Markdown only, a
     config or workflow tweak, a rename, or a handful of lines with no
     new control flow. The one reviewer covers correctness plus what the
     diff's nature calls for (the freshness lenses for a `CLAUDE.md` or
     skill edit).
   - **Reduced fan-out — one scoped lens matched to the risk, no
     cross-check.** A diff that may be *large* but whose risk is
     **narrow** (`history.md` → "Reduced-fan-out band"):
     - **infra / ops** touching no program, SDK or app control flow
       (Dockerfile, compose, a make target, CI YAML, docs) — one
       ops-correctness lens; building or running the image verifies it
       better than prose.
     - **faithful extraction / move refactor** — code deleted in one
       place reappears verbatim elsewhere. One move-fidelity +
       straggler-refs lens: is the move faithful (no dropped or altered
       lines), and does every reference reach the new home? **Name the
       Grep tool** for the straggler search and hand over the hit-list:
       Grep honors gitignore, a bare recursive shell `grep` over a
       package root does not (`history.md` → "Straggler grep into
       build output"). Add **one** small new-logic lens only when
       genuinely new additions rode along, and never turn correctness /
       completeness / style loose on unchanged logic. This holds at any
       size: it overrides the full-tier "four substantive lenses"
       default.
     - **mechanical repo-wide reformat** — discount the reformat noise
       from the sizing; one lens spot-checks that no semantic change
       rode along.
     - **value / default rewiring with no new control flow** — one lens
       confirms the new values and their call sites.
     - **test-only** (`#[cfg(test)]`, `tests/`, no production change) —
       one **test-validity** lens: do the tests assert the right thing?
   - **Small single-crate — correctness + completeness, no
     cross-check.** Small, one crate, **self-contained** (its own tests,
     no new cross-cutting control flow), even at a couple hundred lines;
     its real defects are the mechanical ones lint catches
     (`history.md` → "Small single-crate escalations").
   - **Meta-work — skip security AND style.** Confined to `.claude/**`,
     `CLAUDE.md`, `docs/**`, plus at most small Python glue: no trust
     boundary and no product idioms. Run correctness + completeness +
     the surface-gated freshness lenses, nothing else — unless a Python
     helper carries **real logic** (parsing, control flow, filesystem
     mutation): review it under correctness / completeness, and add
     security back if it reads untrusted input. **A `.claude/hooks/**`
     guard always adds it back**: it parses the agent's own tool calls
     and decides what may run.
   - **Full** — cross-cutting or multi-crate, or anything not above:
     every lens below, under the gates that follow, plus step 6.

   **A reduced tier is sufficient BECAUSE of the excerpt rule, not
   instead of it.** Scaling down tightens the per-lens discipline:
   excerpts inline, hard-stop cap, comparison files named **and their
   excerpts pasted** (`history.md` → "Reduced tiers that held").

   **"Multi-crate" means multiple crates with a SOURCE or TEST change —
   read `code_crates`, not the crate count.** `code_crates == 1` is
   single-crate. A **docs-PATH** change in a second crate drops out of
   the crate count and nothing else: it is still fully reviewed. Tests
   count as code, so a second crate changing only its tests escalates.
   The rollup classifies by path, so a doc-comment-only `.rs` change
   reads as source and escalates too: it over-counts crates, the safe
   direction (`history.md` → "Docs-path crate escalation").

   The four substantive lenses (correctness, security, style,
   completeness) are unconditional only on the full tier, and even there
   the security gate below applies; a tier above that already narrowed
   them is not re-expanded by that default.

   **Gate the security lens on the diff's trust surface.** Spawn it only
   when the diff touches a program handler (a consensus-critical
   instruction such as `deposit` / `withdraw_leader` / `close_vault`),
   account or argument deserialization, an auth path (a
   `.claude/hooks/**` guard is one, not a dev-only script), or external
   I/O (network, untrusted files, user input). Skip it on host / localnet
   tooling — a TUI, a bot's display code, a localnet bootstrap, a
   dev-only script (`history.md` → "Security lens on tooling"). **An
   off-tier security lens is justified, not a trim target**: when a
   tier says skip but that surface is present, add it back, and a
   no-findings verdict on a trust boundary is a result (`history.md` →
   "Off-tier security lenses").

   **Gate the two freshness lenses on the diff's touched surfaces.**
   Spawn them only when the diff edits `CLAUDE.md`,
   `docs/conventions/**` or `.github/**`, **or adds a new top-level
   tree**. The new-tree case is load-bearing — a new subsystem is
   source the audit registry must learn about — so never blanket-skip
   on "source-only". Otherwise skip both (`history.md` → "Freshness
   lens yield"). This composes with the trivial short-circuit: a
   trivial `CLAUDE.md` edit still runs them, a large pure-source
   refactor still skips them.

   **A fifth trigger is a SHAPE, not a path: `prose_heavy`.** Read it
   off the verdict (a 0.5 comment-line ratio **and** a floor of 20
   comment lines); never re-derive it from an impression. When true,
   run the doc-freshness lens as a **comment-accuracy** pass over the
   diff's own added comments, at zero changed markdown lines: is each
   claim the new prose makes true of the code in the same diff? Hand it
   the source slice's comment hunks and the items they document, under
   the two-section cap below. It is step 4's spelling-pre-flight
   principle one stage later. **This is a QUALITY gate, not a token
   lever** — it spends a lens, so no trim pass will ever surface it;
   don't optimize it out on cost grounds (`history.md` → "The
   prose-heavy gate").

   **Scope every broad-scan lens to the diff**, never the whole
   convention set (`history.md` → "Broad-scan sweeps"):

   - **Probe first whether ANY convention governs the surface, and
     skip the lens when none does.** One main-loop
     `search_source.py --files-only` over `docs/conventions/` and
     `CLAUDE.md` for the diff's subsystem terms; an empty probe goes
     into the catalogue as "no convention governs this surface".

   - **Run the citation check in the main loop** — the same tool
     `housekeeping` step 5 calls, so the two paths cannot drift on what
     dangles. It separates `missing-file` from `missing-anchor` and
     exits 1 on any; fold the result into the catalogue and hand the
     lens only the judgement half, whether the prose still describes
     the code:

     ```sh
     python3 .claude/tools/convention_refs.py
     ```

   - **Cap the freshness lens at two named sections, excerpts inline.**
     A brief naming several files is a reading list, not a scope. State
     it as a **restriction**: *"adjudicate from the inlined block; open
     a convention file only to resolve a named dispute"* — never a
     permission to open N files. A permission phrasing in your own
     draft brief is the tell that this lens will run long.

   - **Inline the implicated passage — whichever convention file the
     diff's rule lives in**, not only the audit registry (~20 lines
     each). The **main loop** locates it before spawning, with one
     `search_source.py --ext md` over the named file (`history.md` →
     "Audit-registry inlining"). A lens left to find the passage itself
     spends its calls doing so: one handed a docs slice and leave to
     open two convention files used 10 calls and ≈502k input, 3.5× the
     cheapest lens on the same diff.

   - **Assert the known negatives** — "this struct has no validator",
     "no other call site constructs this" (`history.md` → "Known
     negatives"). The standing instance: before a security lens, sweep
     the diff's files for credential-shaped strings and pass the result
     as a negative fact ("no real vault name, item name or key id
     appears; only placeholder shapes, at these lines").

   - **Hoist every repo-wide grep into the main loop and hand over the
     result set** — unconditional for any "verify X across the repo"
     ask, a dictionary ≥ 2-files census included. The lens is capped
     to "adjudicate from the diff + the provided grep — don't
     re-derive". **Hoist it and narrow it**: a hoisted sweep whose
     result becomes a path list defaults to `--files-only`, widened
     only for adjudication. Prose lives in `.md`, so a skill or
     convention sweep needs `--ext md`. Width, scope, match density,
     enumeration-for-edit and identifiers spanning the chain boundary
     (`leader`, `market`, `vault`) follow
     `docs/conventions/context-economy.md` → "The levers"; a refused
     sweep leaves its question unanswered
     (`docs/conventions/shell-commands.md`). Those rules are
     phase-neutral (`history.md` → "Hoisted sweeps").

   - **Hoist for what a lens will predictably need**, not only what the
     main loop happened to read: name the invariant the diff changes,
     grep its consumers once, hand over the result. Those reads are
     not waste; pre-pay them (`history.md` → "Consumers never hoisted").

   - **Confirm a rule's presence or absence by `Read`ing the current
     file**, never from the diff's `-`/`+` lines. Any lens's "violates
     a convention" claim cites the convention's definition site or is
     dropped (`history.md` → "Convention by analogy").

   - With **no new top-level tree or build manifest**, fold or skip the
     CI-skip-list and audit-registry checks.

   **Run a uniqueness / straggler sweep before the fan-out whenever the
   diff introduces OR consolidates a named identifier** — a label,
   error code, feature flag, discriminator, predicate, constant or
   helper. A lens sees only the diff, so it cannot ask a repo-scope
   question. The sweep is **unconditional for a consolidation**, whose
   claim "there is now exactly one" is repo-scope by construction
   (`history.md` → "Straggler sweeps"). The source-search tool scopes
   out the generated families and never-search trees
   (`review_diff.py --print-grep-excludes` prints them for a
   bare-`grep` fallback);
   hand its result set to the lenses:

   ```sh
   python3 .claude/tools/search_source.py '<identifier>' --files-only
   ```

   **The lint gate already owns the compile-time facts — say so in the
   completeness and cross-check briefs** (`history.md` → "Re-deriving
   compile facts"):

   - **Unused-import / does-it-resolve / does-it-compile /
     unused-symbol checks are OWNED by the lint gate** this skill ran
     green; never cold-read a file to confirm a symbol resolves. **Name
     the hook covering this diff's languages**: for Rust
     `cargo clippy --all-targets -- -D warnings` **plus** `rustdoc` (a
     full-workspace `cargo doc` under `-D warnings`, settling intra-doc
     links in the program crate and its generated SDK copy);
     `ruff check` for Python; `biome` and `tsc` for TS/JS. If that hook didn't
     run (a fresh worktree has no frontend deps, step 4), say so: the
     gate proves nothing there.
   - **Run the dead-code / unused-symbol grep ONCE, in the main loop**,
     and hand the result set to both lenses.
   - **What is left for the lens is judgment, not compilation**: test
     adequacy, code that compiles yet is dead by design, and leftover
     artifacts (TODO/FIXME, debug prints, stray fixtures) — from the
     diff plus one read of each touched file.

   This holds because step 4 ran **before** the fan-out and passed; if
   lint was skipped or is failing, the lens reads for itself — say
   which case applies in the brief.

   **Standing suppressions — drop these before emitting.** Give every
   lens this repo-specific "do NOT flag" list beside the template's
   own, so known noise never reaches the cross-check (`history.md` →
   "Security lens on tooling"):

   - **pre-existing code the diff doesn't touch** — judge the `+`/`-`
     lines and their immediate context;
   - **tuning constants / threshold values or their "why this value"
     comments**;
   - **an assertion that "could be tighter"** when the existing one
     covers the behavior under test;
   - **harmless redundancy that aids readability** (an explicit match
     arm, a clarifying local binding);
   - **consistency-only nits** ("wrap X the way Y is guarded") when
     both forms are already correct.

   It is **not** a free pass: a real bug on a suppressed *line* is
   still a finding.

   **Pre-emit gate — quote the changed line, or drop it.** Every lens
   and the step-6 cross-check quotes the exact `+`/`-` line each
   finding rests on; one that can't is **dropped, not emitted**
   (`history.md` → "Speculative findings"). The principle only — **no**
   numeric confidence scale; severity stays **blocking** / **warning**
   / **nit**.

   Spawn the lenses as parallel `Agent` calls in a single message, each
   with the brief above prepended.

   **While the fan-out is in flight, do not re-read git state, CI
   state, or agent liveness.** The harness notifies on completion and
   the base-freshness re-check comes after. Do genuinely independent
   work — an existence probe for the catalogue, the migration-collision
   check, a title draft — or say once that you are waiting and stop
   calling tools. Still allowed: the single post-push
   `wait_for_checks --no-watch` (`init-pr`'s draft PR starts CI at
   push), and `ListAgents` when the question is which **peer sessions**
   are live.

   **A trim proposal against any rule in this step argues against
   `history.md` → "Exemplars"**, naming the figure it expects to move
   and by how much.

   At minimum:

   - **Correctness** — logic errors, off-by-ones, unhandled edge cases,
     incorrect assumptions, broken invariants. **Standing sub-question,
     mandatory on any diff touching dashboards, alerts, health tables,
     status displays, or watchers: "if this were broken right now,
     would anything actually emit?"** On an observability diff it is
     *the* lens: rendering healthy while broken is a defect in the
     deliverable. Ask it of the watcher and the alert, not only the
     panel (`history.md` → "Healthy while broken").

   - **Security** (conditional on the trust-surface gate above) —
     injection, unchecked input, missing validation, unsafe operations,
     secrets in code. **On a `.claude/hooks/**` diff, open the brief —
     and any step-6 brief on that diff — as defensive verification of
     the repo's own guard**: the goal is to
     prove a real command is still flagged at its tier, never "find a
     command that slips past it". The offensive framing got a lens
     stopped by a safety classifier before its first probe (`history.md`
     → "Guard-bypass lens framing"). Keep the constraint beside it:
     build probe strings inside a scratchpad script, never on a Bash
     line.

   - **Style & consistency** (skipped on a meta-work diff) — naming,
     patterns, idioms that diverge from the rest of the codebase.

   - **Completeness** — missing tests, TODO/FIXME left behind, partial
     implementations, and code the diff introduces that is dead **by
     design** (reachable-nowhere logic, a parameter nothing supplies).
     Not unused imports or does-it-resolve: the lint gate owns those
     (name the hook, per the block above). **Blocking**: a rule removed
     from `CLAUDE.md` or a skill entry file that points to neither its
     ledger entry nor its new home, and any raised ceiling in
     `cfg/skill-size-baseline.json`; an added one is a warning.

   - **`CLAUDE.md` + `docs/conventions/` freshness** (conditional on the
     surface gate above) — does the convention set still match reality
     after this diff? `CLAUDE.md` is the index; the rules live in
     `docs/conventions/**`. Check the inlined sections' rules, command
     examples, paths and tooling references against the codebase and
     the diff. Flag guidance the diff outdates (a command, path, target
     or convention it renames, moves or removes), any rule gone silently
     stale, **and any skill citing a `CLAUDE.md` section or convention
     doc the diff renamed or moved** (the index ↔ doc ↔ skill sync). A
     rule the diff **directly violates or invalidates**, or a dangling
     reference, is **blocking**; merely-stale prose a **warning** with
     the suggested correction.

   - **CI skip-list freshness** (conditional on the surface gate above)
     — the `Tests` workflow's `changes` job is a fail-**closed** `code`
     filter: a catch-all `'**'` minus **negated** excludes
     (`'!frontend/**'`, `'!docs/**'`, …) under
     `predicate-quantifier: 'every'`, so an unlisted new path runs the
     suite automatically. The lens is about the opposite: a diff that
     **adds or renames a test-IRRELEVANT tree** (a frontend-like dir, a
     TS-only SDK package, a docs or config tree, a non-test workflow)
     leaves the exclude-list stale. Grep `.github/workflows/test.yml`
     to the `code:` / `predicate-quantifier` block and slice-read just
     that (~20 lines), then flag the one-line `'!tree/**'` addition or
     rename. Always a **warning**: a stale list only over-runs tests.

   Each sub-agent returns findings with file path, line number, **the
   exact `+`/`-` diff line the finding rests on** (the pre-emit gate),
   severity (**blocking** / **warning** / **nit**), and a one-line
   rationale.

1. **Adversarial cross-check.** (Skipped for a trivial
   diff that took the scaled-down path above.)

   **Re-check base freshness first** — the fan-out window is where
   `main` moves (`history.md` → "Drift during the fan-out"), and where
   the signer locks, so a forced re-rebase takes step 2's signing probe
   first:

   ```sh
   python3 .claude/tools/review_diff.py --base <base> --gate-only
   ```

   Pass `--base <base>` **bare**: the tool prepends `origin/` itself,
   so `--base origin/main` exits 2 on `HEAD..origin/origin/main`
   (`history.md` → "Origin-prefixed base"). **`--gate-only` is the
   default for every freshness probe in this skill**: it emits only
   the verdict fields, with the gating and exit status unchanged, and
   writes **no** diff, so the probe can never overwrite the
   `review-diff.txt` the cross-check is about to read. A full run with
   `--split` is the exception, for when slices must be regenerated for
   an agent (`history.md` → "Gate-only verdicts"). If the probe blocks
   and a rebase changes the diff, re-run the affected lenses and then
   a full `--split --out <scratchpad>/review-diff.txt` run — not
   before.

   Spawn a fresh sub-agent, the `CLAUDE.md` sub-agent brief
   prepended, with the collected findings and the diff **by path** —
   `<scratchpad>/review-diff.txt`, the **full** diff rather than a
   per-lens slice, since this is the one pass that should see
   everything — told to act adversarially:

   - Challenge weak or speculative findings.
     Flag false positives.
   - Identify issues the first pass missed.
   - Push back on rationale that doesn't hold up.
   - Apply the **pre-emit gate** to its own new findings
     too: quote the exact `+`/`-` diff line, or drop it —
     and drop any inherited finding that cites no concrete
     changed line.

   **When it runs regardless of tier.** The reduced tiers drop it for
   a trivial or small-single-crate diff, and that stands. It is
   unconditional on a diff spanning crates or languages, where it
   earns its keep most reliably; and whenever the fan-out's own **fix
   commits** span artifact types (code, SQL, dashboard JSON, YAML,
   prose), run it **whatever the tier** and point it at the fix
   commits too — every other gate keys on
   the original diff, and a defect a fix creates post-dates every lens
   report (`history.md` → "Cross-check value"). **Defend the
   checks-to-run list against any future trim**: most of one review's
   blocking findings came from following a lens's unresolved-check
   items, not its findings (`history.md` → "Checks-to-run").

   Put these in its brief:

   - **A cap of 8–10 tool calls, hard stop** — higher than the
     primary lenses, because it is the pass that overruns most and
     returns most per turn (`history.md` → "Cross-check cap").
   - **Challenge from what it was given.** Reason from the findings
     and the diff plus one up-front read of any file a finding cites,
     shelling out again only for a **genuine** dispute. **Do NOT
     re-open a file a finding already cites** unless resolving a
     specific, named dispute about it; a cross-check that re-derived
     the diff's files from scratch cost 676.7k input.
   - **The completeness lens's lint carve-out, verbatim**:
     unused-import / does-it-resolve / does-it-compile /
     unused-symbol are settled by the green step-4 gate — Rust
     `cargo clippy --all-targets -- -D warnings` **plus** `rustdoc`
     (intra-doc links), Python `ruff check`, TS/JS `biome` / `tsc`.
     Name the hook covering **this** diff and say plainly when it
     couldn't run. Where the gate holds, it must **not** cold-read a
     transitive dependency to re-derive it, and should **refute** an
     inherited finding resting on such a claim. Hand it the same
     hoisted grep result set the lenses got.
   - **Adjudicate a stale-prose finding against the file as the lens
     saw it.** A fix may have landed between the two reads, so when a
     quote no longer matches, judge it against the version the lens
     read, or re-read and **say the text changed** — never score it as
     a fabricated quote (`history.md` → "Refuted against the fixed
     text").

   On material disagreements, iterate: re-spawn the relevant topic
   agent with the challenge to defend or retract, at most 2
   additional rounds, then accept the surviving findings.

   **Checkpoint the findings catalogue before going further** — each
   surviving finding's file, line, severity and one-line claim, via
   `Write(<scratchpad>/review-findings.md)`. The tail to CI green
   (fixes, regeneration, suites, rebase churn) is where a compaction
   lands, and the catalogue is the one not-yet-acted-on artifact whose loss
   means re-running the fan-out. Later, re-read it rather than
   re-deriving.

1. **Fix blocking issues** that are mechanical
   (e.g. unused imports, missing error handling,
   trivial bugs). For each fix, commit signed:

   ```sh
   git add <files...>
   git commit -S -m "<description of fix>"
   ```

   Do **not** fix issues that require design
   decisions — leave those as warnings for the
   human reviewer.

   - **A doc fix is a NEW claim.** Verify a corrected comment against
     the code before committing, as you would the claim it replaces —
     one such fix added a false "impossible by construction" clause
     that only the cross-check caught (`history.md` → "Doc fix, new
     false claim"). Likewise, a fix that relies on a CLI default is
     a claim about someone else's tool: check it against `--help`.
   - **Group the surviving findings BY FILE before applying
     anything.** A file with three or more findings gets **one**
     section-map read (`read_result.py --headings` for markdown, a
     declaration grep for source) and every slice comes off it
     (`history.md` → "By-file grouping"). The slice tally is **per
     file per session**: a file sliced during the study phase carries
     those slices forward.
   - **A changed signature, public type or field shape sweeps its
     call sites before compiling**
     (`search_source.py '<symbol>' --files-only`), fixed in one pass.
     Consecutive builds returning one error class at different sites
     mean the compiler is *enumerating* rather than *checking*.
   - **Every inline quick-check confirming a fix goes through the
     quiet runner** — usually `cargo` (`check`, a scoped `test`,
     `clippy`), though the runner wraps any command
     (`python3 .claude/tools/run_quiet.py -- cargo check`, per
     `CLAUDE.md` → "Context economy"). A **deliberate negative test**
     proving a new guard fires is worth running, and is included: the
     failure is the point, and what you want is the one assertion
     line, not each guarded test's copy of it (`history.md` →
     "Unwrapped cargo runs").
   - **Mutate files with `Edit` / `Write`, never an inline
     interpreter** (`python3 -c`). The shape cannot reduce to an
     allow-rule, so each repetition is an unfirmable prompt that no
     size-ranked metric shows. A rename's count is reported by
     `replace_all: true`; a perturbation's revert is the inverse
     `Edit` (`history.md` → "Inline-interpreter churn").

1. **Re-lint after fixes.** If the previous step committed fixes,
   re-run the lint **scoped to the changed files**:

   ```sh
   python3 .claude/tools/run_quiet.py -- \
     python3 .claude/tools/lint_paths.py --changed
   ```

   Apply step 4's fix-and-retry logic, scoped per-hook re-run
   included. The **full** `make lint` belongs to the two checkpoints
   (before committing, at the end), never to this loop. Run a fast
   suite **whole, through the wrapper, at checkpoints rather than
   after every edit** — the lever is frequency, not scope
   (`history.md` → "Fast-suite frequency").

1. **Regenerate committed generated artifacts
   (mirror the IDL / SDK / vectors / WASM CI gates).** CI fails the
   PR on a committed generated file stale against its source:
   `test.yml` regenerates the **IDL**, and `sdk.yml` the **SDK
   clients**, the **conformance vectors** and the **committed WASM
   glue**, each via a `git diff --exit-code`. Refresh them here and
   commit any diff. **Run this step again after any step-7 / step-8
   fix that touches a generation input** — it is not one-shot (`history.md` →
   "Stale twice").

   **First, the path gate.** Step 5's `runs_artifact_gates` answers
   whether the diff touches a generation input, from the inputs the
   tool owns:

   - **IDL** ← `programs/**`, **doc comments included**: Anchor
     captures a `///` on an `#[derive(Accounts)]` field into the IDL.
   - **SDK clients** ← `sdk/idl/**`, `sdk/codama/**`.
   - **Conformance vectors** ← `sdk/math-core/**`,
     `sdk/interface/**`.
   - **Committed WASM glue** ← `sdk/interface/**`, built by
     `make wasm` into `sdk/ts/src/wasm/`.

   When it is **`false`**, **skip the gates and say so in the
   summary** — a rule, not a per-run judgment call. The flag keys on
   inputs, so a crate the generators depend on transitively still
   trips it.

   **A re-run forced by a rebase asks a narrower question.** The
   first run over the author's own diff stands as above; when this
   step re-runs only because `main` moved, the question is *can the
   base delta have staled anything?* Assert the gates once, skip the
   rebuild and note it in the summary when either holds:

   - step 2's `rebase_overlap.py` reports `runs_artifact_gates: false`
     — the base delta touched no generation input; or
   - `review_diff.py`'s `runs_artifact_gates` is `false` (this
     branch touches no input) **and** `rebase_overlap.py`'s
     `base_files` includes the regenerated artifacts alongside the
     inputs that changed (the base's commits merged through the
     queue, which regenerates them).

   The merge queue's fail-closed re-run covers the residual risk
   (`history.md` → "Redundant rebase rebuilds").

   **Whether CI runs a gate is per-WORKFLOW, not per-diff.**
   `make idl`'s gate sits inside `test.yml`'s path-filtered job
   (step 11 owns that filter); `make sdk`,
   `make check-conformance-vectors`, the WASM glue gate and the
   `sdk/ts` suite live in **`sdk.yml`, which has no path filter** and
   so applies to every PR (`history.md` → "Per-workflow
   re-derivation").

   Otherwise, when the diff *does* touch an input: **run all four
   gates — even when the author says they already regenerated** —
   since a subset spot-check has twice let a stale artifact reach a
   required-CI failure (`history.md` → "Partial regeneration"). Go in
   dependency order (program → IDL → SDK), each through the quiet
   runner. **A target that appears to
   hang is the cargo build lock** until shown otherwise — a
   concurrent build in another worktree; the runner echoes cargo's
   `Blocking waiting for file lock …` live and flags it in its
   summary. Wait, or ask the user to stop the other build; don't
   diagnose with `pgrep`. **An absent toolchain** (`make idl` or
   `make wasm` aborting at `check-toolchain` / `check-wasm`) makes
   that gate unverifiable locally: say so in the report, continue
   with the committed artifact, and don't gate the PR on it.

   - **IDL** (needs the Solana/Anchor toolchain):

     ```sh
     python3 .claude/tools/run_quiet.py -- make idl
     ```

     ```sh
     git diff --exit-code -- sdk/idl/dropset.json
     ```

     If the diff is non-empty, commit it:

     ```sh
     git add sdk/idl/dropset.json
     git commit -S -m "Rebuild IDL"
     ```

   - **SDK clients** (Node + pnpm + Rust; no Solana
     toolchain needed, so always runnable):

     ```sh
     python3 .claude/tools/run_quiet.py -- make sdk
     ```

     ```sh
     git add -A -- sdk/ts/src/generated sdk/rs/src/generated
     ```

     ```sh
     git diff --cached --exit-code -- sdk/ts/src/generated sdk/rs/src/generated
     ```

     If staged changes remain, commit them:

     ```sh
     git commit -S -m "Regenerate SDK clients"
     ```

   - **Conformance vectors** (Rust only; no Solana
     toolchain needed):

     ```sh
     python3 .claude/tools/run_quiet.py -- make check-conformance-vectors
     ```

     The target regenerates the price/quoting vectors, stages
     `sdk/conformance/`, then `git diff --cached --exit-code`s it —
     so a **non-zero exit means the vectors were stale and are now
     staged**. Commit them:

     ```sh
     git commit -S -m "Regenerate conformance vectors"
     ```

   - **Committed WASM glue** (needs `wasm-pack`):

     ```sh
     python3 .claude/tools/run_quiet.py -- make wasm
     ```

     ```sh
     git add -A -- sdk/ts/src/wasm
     ```

     ```sh
     git diff --cached --exit-code -- sdk/ts/src/wasm
     ```

     If staged changes remain, commit them:

     ```sh
     git commit -S -m "Rebuild WASM bindings"
     ```

     **Two gates sit on this artifact.** `sdk.yml` compares the
     committed glue (`dropset_interface.js`, `.d.ts`, `.wasm.d.ts`)
     against a fresh build, excluding the optimized `.wasm` binary
     because `wasm-opt` isn't byte-reproducible across platforms;
     `sdk/ts/src/wasm.conformance.test.ts` runs the **committed**
     binary against the conformance vectors. So an interface change
     that skips `make wasm` fails CI twice, and a stale binary is a
     real behavioral divergence, not a formatting nit.

   If any artifact commit was made, re-run `make lint` through the
   quiet runner (`python3 .claude/tools/run_quiet.py -- make lint`) —
   a regenerated file can still trip whitespace / EOF hooks — with
   step 4's fix-and-retry logic.

1. **Refresh the Audit registry if the diff changed the
   platform shape.** Rotations read the subsystems,
   inter-subsystem interfaces and skip-globs from
   `docs/conventions/audit-registry.md`; that registry is
   kept current on the PR path — here, on every run.
   Inspect the diff for any of three additions and, when
   one is present, **append** the matching entry:

   - a **new subsystem / platform** (a new top-level tree
     or build manifest the registry doesn't list) → add a
     `name (kind, risk): roots` line to the subsystems
     block;
   - a **new seam between subsystems** (a new contract
     crossing a boundary — an event schema, a generated
     surface, a documented interface) → add an
     `A <-> B: contract` line to the interfaces block;
   - a **new generated-file family** (a tree or extension
     the audit should never pick) → add its glob to the
     skip-globs block. This also covers **data-only or
     fixture JSON with no auditable logic** — e.g. the
     committed `keys/*.json` throwaway localnet keypairs
     (skipped as a family, while `keys/**` stays a
     `ci-infra` root so `keys/README.md` keeps
     doc-freshness coverage).

   **Append only** — never drop an existing entry — and
   keep the three blocks lint-clean (MD013, mdformat). If
   the diff introduces none of these, this is a no-op.
   Commit any change signed:

   ```sh
   git add docs/conventions/audit-registry.md
   git commit -S -m "Update audit registry"
   ```

   **A named gate with no committed artifact is a finding.** When a
   spec, doc or issue in this diff designates a **re-runnable
   acceptance check** ("the query in §6.1 is the gate"), that
   artifact must exist as a **file in the repo**: prose describing a
   gate is not a gate, since nothing re-runs it. Flag it as blocking
   and land the artifact with the spec — a query in the market-data
   analytics directory's `psql -v` idiom, a check under
   `.claude/tools/` with a test (`history.md` → "Gate in prose
   only").

1. **Run the test suite (mirror CI).** The `Tests` workflow runs
   `make test` and `make test-no-teardown`; run both locally so the
   green checks GitHub needs for auto-merge are already verified
   here.

   **Re-check base freshness immediately before the run.** Step 5's
   `base_fresh` is several steps old, and a sibling session merging
   its PR moves `origin/<base>` underneath this one (worktrees share
   one `.git`):

   ```sh
   python3 .claude/tools/review_diff.py --base <base> --gate-only
   ```

   If `ready` is `false`, rebase onto `origin/<base>` and *then* run
   the suites, so the expensive builds run once, against the tree that
   will merge (`history.md` → "Suites before the rebase"). A disjoint
   drift (`ready: true`, `base_fresh: false`) needs no rebase. Switch
   to a full `--split --out` run only when the slices need
   regenerating (after a rebase, for a re-spawn).
   Any rebase here takes the signing probe first (step 2).

   **Choose the targets from step 5's flags — read them, never
   re-derive them** — and record every skip with its reason in the
   summary:

   - **`runs_rust_suites`** mirrors `test.yml`'s `code` path filter
     (frontend, decks, brand assets, docs, markdown, `.claude`, the
     TS SDK and several non-test workflows excluded, under
     `predicate-quantifier: every`) through
     `CODE_FILTER_EXCLUDES` in `review_diff.py` — change that when
     the workflow's filter changes. **`false` → skip both Rust
     targets**: CI isn't running them either. The filter is
     **`pull_request`-only**, so a `merge_group` run executes the full
     suite regardless and a green path-filtered PR check proves
     nothing about leaving the queue.
   - **`rust_reachable` `false` → skip them even when
     `runs_rust_suites` is `true`.** That flag fails open by design;
     this one measures whether the diff touches anything a `cargo`
     build consumes (a `.rs` file, a `Cargo.toml`, the lockfile, a
     toolchain file or `.cargo/config.toml`, a `build.rs`, an `.s`
     source, a migration, a
     conformance vector, **or a fixture a Rust test reads**). Mirroring
     buys something only when the local
     run could fail because of the diff (`history.md` → "Unreachable
     suites").
   - **`rust_fixture_hits` non-empty → run the full local Rust
     suite**, naming the paths in the summary. The tool scans every
     tracked `.rs` file for `include_str!` / `include_bytes!`
     targets and for runtime reads of a `CARGO_MANIFEST_DIR` path or
     a bare-literal `fs::read`, so it sees a path CI's filter excludes
     but a Rust test reads — every PR-side Tests job no-ops while the merge
     queue runs the full suite (`history.md` → "Fixture-coupled
     dequeue"). This is the one case where the path lists say not
     to.
   - **The inverse half is a judgement, not a flag**: run a check
     when some changed path is reachable from it even if the diff
     misses its trigger's enumerated tree list (`history.md` →
     "Missed trigger").
   - **The CI-mirror carve-out**: when the diff is confined to
     `.github/**` and CI is already green on the head commit, treat
     CI as the mirror and skip the local suite (`history.md` →
     "Half-applied build").
   - When both flags are `true`, run both targets.

   Run them through the quiet runner, **`make test` first**:

   ```sh
   python3 .claude/tools/run_quiet.py -- make test
   python3 .claude/tools/run_quiet.py -- make test-no-teardown
   ```

   `test-no-teardown` leaves a `--no-default-features` program build,
   so a later bare `cargo test -p dropset` fails **15 unrelated**
   admin-teardown tests with `Custom(6043)` — the anchor-v2 runtime
   feature guard, reading exactly like a regression (`history.md` →
   "Teardown-feature build"). Keep this order, or re-run
   `make program` before any scoped `cargo test`.

   **Batch independent program mutations into one sBPF build.**
   Verifying a finding by mutating the program costs a
   `make program` per state, so apply every independent mutation,
   build once, run once, revert once; only a mutation another would
   mask needs its own cycle.

   **When the FIX is a new test, mutation-verify it** — a green
   suite is exactly what a worthless test produces. Reintroduce the
   defect and confirm the new test fails **on its behavioral
   assertion**, not on a precondition:

   - **Verify by test count, not exit code.** Equal counts plus a
     failure is the signal; a **lower** count means the invocation
     excluded the target, so the run is *inconclusive*. A
     feature-gated target needs `--all-features` (or the explicit
     `--features`) — the default run is a different, empty one. Take
     the invocation CI uses; if you must narrow, narrow `--files` /
     `--test`, never the package set. After adding tests, confirm
     `N passed` moved by the number added. This holds for any
     conditional compilation, not only cargo features.
   - **Expect the first fixture not to bite**; budget one rewrite per
     mutation-verified test.
   - **A mutation that passes means the TEST is wrong until proven
     otherwise, not the fix** — never remove a correct fix on that
     reading.
   - **This lever deliberately adds cost** and must not be folded
     into a saving: budget two extra suite runs per mutation and say
     so, so a later metrics pass does not read them as churn. Batch
     test mutations the same way as program ones (`history.md` →
     "Mutation verification").

   **If the diff edits both a source file and a doc citing
   `file:line` in it, citations are a FINAL step**: apply every
   source edit first, then derive and write all citations in one
   pass. A citation written before a later edit to the same file is
   stale — re-derive it, never arithmetic it forward (`history.md` →
   "Forward-computed citations"; `docs/conventions/docs-and-style.md`).

   **When the diff adds or changes an `#[ignore]`d test, run the
   Tests (Postgres) job's own target before pushing** — the targets
   above skip
   ignored tests, so one can otherwise only ever fail in CI
   (`history.md` → "Ignored test reddened CI"). The job runs exactly
   this target, so it carries the job's crate selection and its
   credential-gated exclusion; a bare workspace-wide
   `--run-ignored all` selects tests no CI run can satisfy:

   ```sh
   python3 .claude/tools/run_quiet.py -- make test-postgres
   ```

   **One cargo target at a time per worktree** — a blocked build is
   **waited on, never raced**: two builds on one `target/` produced a
   spurious `E0463 can't find crate for dropset_sdk` doctest failure
   (`history.md` → "Raced cargo builds").

   **Background the suites and wait on CI concurrently** — they test
   the same commit, so finishing them before step 18's CI wait
   serializes two ~20–40 minute waits. Launch with
   `run_in_background: true`, push, and collect both as they land.
   **But only once the fix commits have landed**: the build compiles
   the tree as it finds it, so hold the suites while the fan-out is
   still reporting or fixes are still going in (`history.md` →
   "Half-applied build"). Read CI state right after the push —
   `init-pr`'s draft PR may already have settled it.

   - Both depend on the Solana/Anchor toolchain via
     `check-toolchain`. If the toolchain is absent
     and a target aborts before any test runs, you
     **cannot** verify that workflow locally — say
     so explicitly in the report (do not claim CI
     will pass), rather than counting it as green.

   - **Also run the `sdk/ts` node test suite** — the SDK job's
     mirror, wanted even when `runs_rust_suites` is `false`, since
     `sdk.yml` has no path filter. It is **node-only**, so **always
     runnable locally**, and
     the *only* local gate that catches an on-chain layout change
     breaking the SDK's hand-built fixture (`market.test.ts`).
     Install the workspace deps once if needed
     (`pnpm --filter @dropset/sdk install`), then:

     ```sh
     python3 .claude/tools/run_quiet.py -- pnpm --filter @dropset/sdk test
     ```

   - If a test fails, fix it when the fix is
     mechanical (commit signed, then re-run the
     failed target), otherwise catalogue it as a
     **blocking** issue. Never mark the PR ready
     with a failing or unverified test target.

1. **Push the branch.** The step-2 rebase rewrote history, so push
   with lease — it lands the rebased history with any review-fix
   commits and refuses to clobber a concurrent push:

   ```sh
   git push --force-with-lease
   ```

1. **Update the PR title and description.** Invoke
   `/pr-title-description` so both reflect the branch's final state
   (after lint and review fixes). **No Linear tags in the body or in
   any PR comment this skill posts** (`CLAUDE.md` → "Keep Linear tags
   out of PR bodies and comments"); the title's `ENG-###` scope is the
   one exception, and the terminal `AskUserQuestion` prompts below
   print the tag deliberately, as terminal chrome.

1. **Scan the final PR body for AI attribution.** The rule is
   absolute (`CLAUDE.md` → "Commits and PRs"): no Claude / Anthropic
   `Co-Authored-By:` trailer and no "Generated with Claude Code"
   footer, in the title, the description, or any comment this skill
   posts. Ask the guard, so the patterns have one owner — write the
   stored body to a scratchpad file and scan it:

   ```sh
   python3 .claude/hooks/no_ai_attribution.py --scan <scratchpad>/body.md
   ```

   Exit 0 is clean; **exit 1 means attribution is present** — strip
   it, re-run `/pr-title-description`, re-scan. Exit 2 is a usage
   error (a missing path), so a typo cannot read as clean. It is a
   step, not a habit, because the `PreToolUse` guard sees only Bash
   while the title, body and comments go through the GitHub MCP —
   against a live harness instruction to append both
   (`history.md` → "Harness attribution conflict"). This is the
   *opposite* posture from the standing lens, which correctly does
   **not** report absence as a finding: this step checks the
   convention **held**.

1. **Verify the PR title passes `Semantic PR`.** The workflow needs a
   Conventional-Commits type, a scope matching `^ENG-[0-9]+$`, and a
   subject matching `^[A-Z].*$` — `feat(ENG-451): Add …`. If it
   doesn't conform, re-run `/pr-title-description`. It also sets
   `validateSingleCommit`: with exactly one commit on the branch, that
   commit's message must match the title — squash or reword.

1. **Confirm the PR still merges cleanly.** The base can advance again
   during a long review. Fetch it, then read only the two merge fields
   (`docs/conventions/github-mcp.md` → "The CI-wait and PR-state
   reads"):

   ```sh
   git fetch origin <base>
   ```

   ```sh
   gh pr view <number> --json mergeable,mergeStateStatus
   ```

   **The mergeability tri-state** — key on `mergeable`, the gh twin of
   the MCP `mergeable_state`; this step and step 20 both branch on it:

   - `CONFLICTING` (or `mergeStateStatus: "DIRTY"`) → **blocking**.
     Don't mark the PR ready; tell the user to rebase onto
     `origin/<base>`, resolve, and re-run `/review-pr`. This skill
     **never auto-resolves a conflict** — step 2 argues why that holds
     even for a trivial sorted-list collision.
   - `UNKNOWN` → GitHub is still computing; re-read after a few seconds
     until it settles.
   - `MERGEABLE`, any `mergeStateStatus` → no conflict. `BLOCKED` /
     `UNSTABLE` mean protection, checks or review are outstanding (a
     draft mid-review); `BEHIND` means the base moved, which step 2
     handled.

1. **Gate.** Mark the PR ready only when **every** local CI-mirroring
   check is green: **zero blocking issues** (every Linear checklist
   item addressed); `make lint` passes, with a hook that couldn't run
   locally (`biome` / `tsc` without frontend deps) noted as
   unverifiable, not gated; the generated artifacts are fresh and
   committed (IDL — or honestly reported unverifiable without the
   toolchain — SDK clients, conformance vectors); `make test` and
   `make test-no-teardown` pass or are honestly reported unverifiable;
   the **`sdk/ts` node suite passes** (always runnable); the title
   passes `Semantic PR`; and `mergeable` is not `CONFLICTING`. Then:

   ```txt
   mcp__github__update_pull_request(
     owner: "DASMAC-com",
     repo: "dropset",
     pullNumber: <number>,
     draft: false,
   )
   ```

   Ready lets the human enable "Merge when ready" while CI finishes,
   but it does **not** end the run: success is reported only once real
   CI is green and the summary is printed. **The Linear state** for
   the rest of the run: **In Progress** through the gate, the CI wait
   and the summary; **In Review** only at the step-20 handoff; and
   nothing after that writes a state except the operator-approved
   close (`CLAUDE.md` → "The Linear state tracks the session, not the
   PR").

   On any remaining blocker — an unaddressed checklist item, failing
   or unverified tests, a non-conforming title, a conflict — leave the
   PR in draft and the issue where it is, **skip the CI wait**, and
   report the blockers.

1. **Wait for GitHub CI to pass.** Local checks only *mirror* CI; the
   real run on the pushed commits is authoritative, and the only signal
   when the toolchain was absent locally. `init-pr`'s draft PR started
   it at push time, so it is already in flight.

   **Make the working state survivable before any long unattended
   wait** — this one and the step-5 fan-out. A worktree has been
   deleted under a review, losing a verified-but-uncommitted fix, and
   the checkpoint rule that should have saved it did not fire because
   signing was down (`history.md` → "Worktree deleted under a wait").
   So:

   - **A signing failure is an unpushed-state alarm**, not a nuisance:
     the checkpoint did not happen. Get signing working before
     starting a wait.

   - **Probe signing BEFORE the wait** by making the checkpoint commit
     immediately before it (and before the step-5 fan-out). `init-pr`
     step 0b cannot cover this: the agent locks while the session sits
     idle, which is what a long wait produces.

   - **Write the working diff to the scratchpad**, so recovery is one
     `git apply` even if the commit could not be made:

     ```sh
     python3 .claude/tools/review_diff.py --base <base> \
       --out <scratchpad>/pre-wait.diff
     ```

   **Wait with the tool — never poll.** One call blocks until the
   checks settle and prints one compact verdict:

   ```sh
   python3 .claude/tools/wait_for_checks.py --pr <number>
   ```

   ```json
   {
     "conclusion": "pass",  // pass fail pending none conflicting timeout
     "settled": true,
     "elapsed_seconds": 127,
     "watch_rounds": 1,       // >1 means a check registered late
     "counts": {"pass": 12, "fail": 0, "skipping": 3},
     "failing": [{"name": "…", "workflow": "…", "link": "…", "run_id": "…"}],
     "pending_checks": [],
     "log_path": "…/wait-for-checks-<number>.log"
   }
   ```

   Inside, gh paces a `gh pr checks --watch` whose live table goes to
   `log_path`, then one `--json` read *is* the verdict — not gh's exit
   code, which overloads "failed", "pending" and "no checks". It exits
   0 only on `pass`, and `failing` carries each `run_id`.

   **The never-poll rule, for every wait in this skill** (this one,
   the step-20 mergeability re-read, the step-22 queue watch): no
   shell `while … sleep … done` loop and no `jq` filter — compounds
   that cannot reduce to an allow-rule. Never a **bare**
   `gh pr checks --watch` or `gh run watch` either: called directly
   each re-prints its whole table every refresh into one tool result,
   replayed on every later turn, and the cost grows with CI's length
   (`history.md` → "Bare watch cost"). `--watch` is right only inside
   the tool. Without the tool, the acceptable substitutes are a
   narrowed one-line-per-poll read or a backgrounded watcher whose
   exit status alone is read.

   **Say once, then silent** — the same rule for the step-22 queue
   watch: tell the human once that CI is in flight, then say nothing
   until the verdict. If the session was interrupted and the wait is
   gone, run the tool with `--no-watch` for one current read, then
   plain to resume.

   Branch on `conclusion` — it is exhaustive:

   - **`none`** with `blocked_by_conflict: false` → no checks apply (a
     path-filtered diff); treat it as green. The tool re-reads a
     `none` over a few short rounds before believing it, because a
     wait started right after a push can beat the runs' registration,
     and `none` fails **open** where `pending` fails safe
     (`history.md` → "Registration race"). If a `none` still looks
     wrong, one `gh run list --branch <branch>` filtered to the head
     SHA tells the two apart.
   - **`conflicting`** → **blocking**: a CONFLICTING PR gets no
     `pull_request` run at all, so nothing will ever arrive. Rebase,
     push, re-run the wait. Never green.
   - **`timeout`** → the bound (default one hour) or the re-watch
     rounds ran out with checks unsettled; a `none` arriving after any
     `pending` round reports this too. It never claims `pass` from a
     snapshot it stopped waiting on — treat it as unverified, and read
     `pending_checks`.
   - **`pending`** → only from a `--no-watch` read (under `--watch` a
     pending read re-enters the watch). Re-run the tool plain.
     `watch_rounds` > 1 means a check registered after gh's census —
     routine, and the reason the tool re-enters rather than believing
     one read.
   - **`pass`** → ready and CI-green; print the summary.
   - **`fail`** → catalogue each `failing` entry as **blocking**, with
     its name and `link`, then diagnose as follows.

   **Read the failing job's STEP CONCLUSIONS before any log** — bounded
   by step count, not log volume:

   ```sh
   gh api repos/DASMAC-com/dropset/actions/runs/<run_id>/jobs \
     --jq '.jobs[] | "\(.conclusion)\t\(.id)\t\(.name)"'
   ```

   ```sh
   gh api repos/DASMAC-com/dropset/actions/jobs/<job_id> \
     --jq '.steps[] | "\(.conclusion)\t\(.name)"'
   ```

   A tail is an offset guess, and the post-steps epilogue fills the
   last several dozen lines (`history.md` → "Tail landed in the
   epilogue"). The step's **name** often is the diagnosis, says whether
   a log is cascade-shaped or single-command, and separates "ran and
   failed" from "never ran". **Route on what the failing step IS:**

   - **A test runner or anything with its own trailing summary**
     (`make tools-tests`, `cargo nextest`, `pnpm test`) → skip
     `get_job_logs`: the summary prints after the failure and the
     epilogue after that. Capture the log through the quiet runner
     and Grep it:

     ```sh
     python3 .claude/tools/run_quiet.py -- \
       gh run view <run-id> --repo DASMAC-com/dropset --log-failed
     ```

   - **A single command whose last output is its error** → the MCP
     tail is fine:

     ```txt
     mcp__github__get_job_logs(
       owner: "DASMAC-com",
       repo: "dropset",
       run_id: <run_id>,
       failed_only: true,
       return_content: true,
       tail_lines: 100,
     )
     ```

   - **The pre-commit `Lint` job** → never re-fetch a larger tail: it
     runs `--all-files`, so the failing batch sits above any tail.
     Reproduce the hook locally and trust its exit code:

     ```sh
     python3 .claude/tools/run_quiet.py -- \
       python3 .claude/tools/lint_paths.py -- <hook-id>
     ```

     Through `lint_paths.py`, **not** bare
     `pre-commit run <hook-id> --all-files`, which enumerates via
     `git ls-files` and so cannot see an un-`git add`ed file — exactly
     the one CI's checkout fails on. `run_quiet` names the offending
     files (PR #235). Skip a hook that can't run here, as step 4 does.
     **A red CI `Lint` with a GREEN local reproduce is probably
     toolchain skew**, not a flake — a clippy failure in a file this
     branch never touched almost certainly is. Confirm it on another
     recent branch's `Lint` job; **never "fix" it by editing unrelated
     code** — pin or report the toolchain.

   **Never `gh api` a job log**: Actions logs are colorized and
   `gh api` refuses escape sequences without its allow flag, so the
   fetch returns **empty stdout**. Always distinguish *fetch failed*
   from *fetched and found nothing* (`history.md` → "Empty-stdout
   scan").

   **Need the PR's file list to place a failure?** Take
   `gh pr view <number> --json files --jq '.files[].path'`, never
   `pull_request_read` `get_files`: `minimal_output: true` does not
   suppress its per-file patch text (`docs/conventions/github-mcp.md`).

   Then re-draft the PR, which also cancels a pending "Merge when
   ready", leave the issue where it is, report the failures, and do
   **not** report the run finished — the user fixes them and re-runs
   `/review-pr`:

   ```txt
   mcp__github__update_pull_request(
     owner: "DASMAC-com",
     repo: "dropset",
     pullNumber: <number>,
     draft: true,
   )
   ```

1. **Print the review summary.** With CI green, print it now —
   *before* the merge-queue prompt, so the human sees the full picture
   at the moment they decide. The queue outcome is not known yet; it
   is reported by step 22.

   - Linear coverage: the resolved tag, and each checklist item marked
     addressed / partial / missing (or "no Linear task checked" if none
     was resolvable).
   - Fan-out: which tier the diff took (full, small single-crate,
     move-refactor / other reduced-fan-out, meta-work, or trivial
     short-circuit) and, for a scaled-down run, which lenses were
     skipped and why (security lens skipped — no trust surface; style
     skipped — meta-work; cross-check skipped; freshness skipped —
     surface gate; **a lens skipped on yield** — name it, and never let
     a yield skip be silent).
   - In-flight overlap: open PRs whose files intersect this diff (from
     `review_diff.py --overlap`), and what was decided about them —
     waited, or proceeded with the re-run cost accepted. Or that there
     were none, or that the probe could not run.
   - Lint status: pass, fail with details, or which hooks were
     unverifiable locally (deps not installed) and why that's safe for
     this diff.
   - Generated artifacts: IDL / SDK clients / conformance vectors —
     regenerated clean, committed a refresh, or (IDL only)
     unverifiable without the toolchain.
   - Test status: `make test` and `make test-no-teardown` — pass,
     fail, or unverified locally (toolchain absent) — plus the `sdk/ts`
     node suite (always runnable): pass or fail.
   - Title status: passes `Semantic PR` or not.
   - Merge status: `mergeable` — `CONFLICTING` vs. `MERGEABLE` (no
     conflict).
   - CI status: all GitHub checks green, or "no checks" treated as
     green — by this point CI has passed (a failure would have stopped
     the run at the CI wait).
   - Linear status: currently **In Progress**; it moves to **In
     Review** at the merge-queue handoff that follows (or stays put if
     no tag was resolvable).
   - `CLAUDE.md` + `docs/conventions/` freshness: in sync, or each
     stale rule / dangling skill reference the diff outdated, with the
     suggested correction — or **skipped** (the surface gate didn't
     fire: the diff touched no `CLAUDE.md` / `docs/conventions/` /
     `.github/` surface and added no new top-level tree).
   - CI skip-list freshness: the `test.yml` `code`-filter exclude-list
     is in sync, or each test-irrelevant tree the diff added/renamed
     that should be excluded, with the suggested one-line edit (warning
     only) — or **skipped** (surface gate didn't fire, as above).
   - Issues found / fixed / remaining.
   - Remaining warnings and nits for human review, each with
     `file:line` and rationale.
   - Whether the PR was marked ready.

1. **Move the issue to In Review and offer to add the PR to the merge
   queue.** Only when the CI wait took the **all-checks-green** path
   and the summary is printed; otherwise skip this entirely.

   **The handoff is one `AskUserQuestion`, unconditionally** (the
   skill-handoff convention, `docs/conventions/commits-and-prs.md`):
   it authorizes both the In Review move and the enqueue. Never
   advance the issue and enqueue without it, and never substitute a
   plain-text "shall I enqueue?". The only paths that skip it are the
   non-happy ones below.

   **First, re-read mergeability** — a PR `MERGEABLE` at the gate can
   turn `CONFLICTING` while CI ran:

   ```sh
   gh pr view <number> --json mergeable,mergeStateStatus
   ```

   Apply step 16's tri-state, with the stakes raised: on `CONFLICTING`
   neither enqueue nor advance the issue — report it, tell the human to
   rebase, leave the issue In Progress, and stop. On an `UNKNOWN` that
   survives a few spaced re-reads, say so and hold. Only `MERGEABLE`
   proceeds.

   **If this branch adds a migration, check for a cross-branch number
   collision before enqueueing.** The ascend guard and the schema-fence
   tests see one tree from an empty database, so a duplicate number is
   invisible until both files meet on the merge-group branch — and an
   applied migration is **immutable**, so renumbering the **wrong**
   branch wedges the shared dev Postgres, fixable only by manual
   surgery or a wipe that destroys collected market data
   (`history.md` → "Migration collisions"). One call; the tool does
   the open-PR read itself:

   ```sh
   python3 .claude/tools/migration_collisions.py --others-from-gh
   ```

   It compares **numbers, not filenames**. **Gate on all three exit
   statuses:** `1` is a collision; `0` is any other status, including
   `nothing_claimed`; `2` means the tool **could not answer** (bad
   input, a `gh` failure, a truncated listing, a pathspec matching
   nothing) — a **hold, not a pass**. Gating on non-zero reports an
   operational failure as a collision; gating on `== 1` enqueues
   through an unanswered question. Never adapt to the guard by ignoring
   its exit status.

   Read the summary line: it says how this branch's own PR was
   excluded from the comparison. Three cases block, for two reasons. A
   **detached HEAD** (no branch to resolve) and an exclusion that
   **matched nothing** (this PR open under a different head ref) fail
   **closed** — the self-match survives, so a collision naming one PR
   whose file matches this branch's is *probably* that self-match, a
   hint to confirm, not an identity. A **fork** PR sharing this
   branch's name is never treated as ours; its collision is **real** —
   don't wave it through. `--self-branch` overrides the resolution and
   the summary records it. `--others <file>.json` serves a caller
   already holding the inventory and keeps tests network-free.

   **On a collision, stop and coordinate the renumber: the branch whose
   number is already applied to the shared dev DB keeps it; the other
   renumbers.** No migration → `nothing_claimed`, exit 0 — an empty
   verdict, not a clear one, which is why `init-pr` reads
   `next_free_number` from the same call.

   **A rebase-forced re-run may enqueue BEFORE re-verifying.** `main`
   lands about as often as a full local re-verify takes, so re-running
   locally after every rebase invites the next one (`history.md` →
   "Rebase race"). When the **only** reason for a re-run is a rebase,
   push, enqueue, and let the queue verify — it re-runs the whole Rust
   suite against the merged result, fail-closed. All three conditions
   required: the branch's own diff is **unchanged** since the last
   green local pass; `rebase_overlap.py` reports **no overlap** between
   the base delta and this branch's files; and `runs_rust_suites` on
   that report says the queue's suite covers the delta. Otherwise
   verify locally. Say in the summary that the re-verify was skipped
   and why.

   Then move the issue (the step-3 tag) to **In Review**, with or just
   before the prompt — skip only if no tag resolved. A state-only write
   takes the zero-echo path
   (`docs/conventions/linear-automation.md` → "Field-only writes go
   through `board_batch.py`, not the MCP"); the session has held the
   body since step 3:

   ```sh
   python3 .claude/tools/board_batch.py state \
     --id <ENG-###> --state "In Review"
   ```

   **One write, no retry loop**: if it disagrees, verify once, report
   the discrepancy and move on (`history.md` → "State-write echo").

   Ask with `AskUserQuestion`, printing **both identifiers** — the
   Linear tag (`ENG-536`) and the PR number (`#138`) — with "yes, add
   it to the merge queue" **first** and "skip, I'll merge by hand"
   second.

   - **On approve**, enqueue — the skill's one `gh` **write**; the MCP
     has no merge-queue tool and `merge_pull_request` merges
     immediately, bypassing the queue:

     ```sh
     gh pr merge <number> --auto
     ```

     **Pass no strategy flag** — the merge queue sets it, and a
     `--squash` only warns. **Expect no output at all**: stdout is not
     a TTY, so a silent exit 0 is the success shape. **But exit 0 is
     NOT proof of enqueue** — it shows auto-merge was *enabled*, and a
     PR has gone 9/9 green, exited 0 and never been queued
     (`history.md` → "Exit 0, never queued"). Step 22's watch is the
     verification: its `not_queued` verdict is exactly that case.
     **The already-clean remedy, which is not a bypass:** where
     `merge_queue` is an active branch rule (checkable once with
     `gh api repos/{owner}/{repo}/rules/branches/main --jq '[.[] | .type]'`),
     a plain `gh pr merge <n>` **enqueues** rather than merging, so it
     is the correct fallback when `--auto` did not take.

   - **On decline**, leave the PR ready and the issue In Review, and
     note they can merge it (or enable "Merge when ready") themselves.

1. **Capture session metrics** — unprompted, once, as the second-last
   act of the step-22 tail: after a confirmed merge, immediately
   **before** the close question, so the capture covers the whole
   task. **No approval and no question**; the enqueue-time "shall I
   file session metrics?" ask is retired (`history.md` → "Metrics
   ask retired"). If the PR is taken back out of the queue, nothing
   files — the capture waits for the next close attempt. On every path
   that ends without a merge (a declined enqueue, a `closed` PR, a
   dequeue left for the user), say in the report that session metrics
   were **not** captured this run.

   Invoke `/session-metrics`. It derives the session id from the
   scratchpad path, ranks the run's sinks and hardening candidates in
   its own process (the transcript never enters context), and files
   each trim lever as a **parked issue** under `Trim levers` through
   the zero-echo `trim_levers.py` writer, appending evidence to a lever
   that already exists. It authors no source edit. If the milestone is
   missing, the writer says so — note it in the report. **Ground it in
   this run:** carry the wasteful payloads you noticed (a whole-file
   Read, a verbose log, a repeated PR read) into it, per
   `docs/conventions/context-economy.md` → "Track consumption ideas as
   you go", so its levers are concrete.

1. **Surface the merge-queue outcome.** Only if the enqueue was
   approved. **Watch the queue yourself: "come back and tell me when
   it merged" is never an acceptable ending for a queued PR** — the
   enqueue yes authorizes the watch, and the wait is never handed to
   the human (`history.md` → "Queue wait handed to the operator").

   Immediately after the enqueue, start **one backgrounded** command
   (`run_in_background`) — the harness re-invokes the session when it
   exits, and that is the wake-up:

   ```sh
   python3 .claude/tools/wait_for_checks.py --queue <number>
   ```

   ```json
   {
     // merged dequeued closed not_queued timeout probe_failed
     "conclusion": "merged",
     "seen_queue_entry": true,
     "last": {"gql_state": "MERGED", "view_state": "MERGED", …},
     "queue_runs": [{"run_id": "…", "name": "…", "conclusion": "…"}],
     "failing_runs": [],
     "log_path": "…/wait-for-queue-<number>.log"
   }
   ```

   It polls the GraphQL `mergeQueueEntry` probe, the
   `gh pr view --json state,mergedAt` read, and this PR's
   `gh-readonly-queue/…/pr-<number>-…` runs each minute, stays silent
   while queued, and exits 0 only on `merged`. It owns the reasoning
   this step used to spell out
   (`docs/conventions/github-mcp.md` → "The merge-queue handoff"):
   key on `mergeQueueEntry` (`autoMergeRequest` only keeps a classic
   auto-merge repo waiting, never outranking a failed queue run);
   **a terminal `MERGED` from either source is authoritative**, and
   nothing concludes "dequeued" until both agree the PR is not merged
   (`history.md` → "The two sources disagree both ways"); an all-null
   probe is the registration window **or** a removal, and the newest
   queue SHA's runs tell them apart. Say once that the PR is queued,
   then stay silent until the exit.

   Where a background command is unavailable, fall back to a
   session-scoped cron running the same command about every 12 minutes
   with `--timeout 660` — above the tool's 600-second grace — and
   `--since <the enqueue time, UTC ISO-8601>`, which every firing needs:
   each one is a fresh call, and without the anchor a dequeue older
   than the tool's 15-minute run window reads as `not_queued`. It
   deletes itself on a terminal verdict; a `timeout` from a cron firing
   is silent and the cron stays. If the classifier denies the cron, ask
   the operator to approve it in this tab; never silently hand over the
   wait. A resumed session re-runs the command, and a fresh poll
   returns the current snapshot.

   **The terminal-MERGED invariant also binds you**: if your own read
   says `MERGED`, never act on an older non-merged verdict, and never
   re-read a file at the merge ref to confirm the change landed — the
   verdict is the confirmation.

   Branch on `conclusion`:

   - **`merged`** → report it; the Linear issue **stays In Review** and
     nothing here writes a state. Then run the post-merge tail below.

   - **`closed`** → closed **without** merging: abandoned, not landed.
     Report it and skip the tail; the worktree may still be wanted.
     The close is `/wrap`'s, which owns closed-not-merged endings.

   - **`not_queued`** → no entry and no queue run ever appeared: the
     enqueue did not take. Apply step 20's already-clean remedy and
     restart the watch. On any fresh call without `--since` (a resumed
     watch), a dequeue older than the tool's run window reads the same —
     check `gh run list` before re-enqueueing.

   - **`dequeued`** → diagnose from the **queue branch's** runs, never
     the PR's own checks: the queue builds `main` plus this PR on
     `gh-readonly-queue/<base>/pr-<number>-<sha>`, so a failure there
     dequeues the PR while every PR check stays green. The tool reads
     those runs on every poll because concluding without them once
     cost a blind re-enqueue: on PR #311 the first probe *was* a
     terminal dequeue — the queue's full run set had completed and
     `SDK` failed transiently while all 8 PR checks stayed green — and
     the re-enqueue was right only by luck. Key on **job**
     conclusions (a run can read `in_progress` with a job already
     failed), then pull the failing job with the step-18 failure path.
     Split by what failed:

     - **Transient infrastructure** — a toolchain, network or cache
       failure **before any test ran**, e.g.
       `Cache not found for input keys: toolchain-solana-…` then
       `Failed to install platform-tools` → re-enqueue, saying why,
       and restart the watch.
     - **A real `main`-integration conflict** — an assertion, compile
       error or lint violation on the queue branch → do **not**
       re-enqueue. Catalogue it as blocking, re-draft, and tell the
       user to rebase on `main` and re-run `/review-pr`.

     Report the removal either way, naming the job. An empty
     `failing_runs` means the entry vanished with every run green for
     the tool's whole grace window — check the PR's timeline before
     re-enqueueing.

   - **`timeout`** / **`probe_failed`** from the backgrounded watch →
     report the `last` snapshot (and `error`), then restart the watch;
     never report it as queued-and-fine.

   **The post-merge tail, on `merged` only — one batched sequence with
   a single operator touch at the very end.** Work it as a checklist;
   **a diversion does not discharge it** — another skill, a peer
   session's message, a fresh request or a long wait closes nothing
   (`history.md` → "Diverted tail"). Come back and finish it, or say
   which item you are skipping and why. An unanswered
   `AskUserQuestion` is a deferral the user chose; an unasked one is a
   dropped step.

   - [ ] **Dismiss this PR's notification**, `state: "done"`. Read the
     thread id with a field-selected `gh` (a documented exception,
     `docs/conventions/github-mcp.md`) — the MCP list embeds a full
     repository object per notification (`history.md` → "Notification
     list cost"):

     ```sh
     gh api /notifications --jq '.[] | {id, url: .subject.url}'
     ```

     Pick the thread whose `subject.url` ends in this PR's number, then
     dismiss it — never `mark_all_notifications_read`:

     ```txt
     mcp__github__dismiss_notification(
       threadID: "<this PR's notification id>",
       state: "done",
     )
     ```

     `"done"`, **not** `"read"`, which leaves the thread in the inbox.
     The list is a fixed cost read once — never re-fetch it. No match
     (already cleared) → skip.

   - [ ] **Reclaim the worktree's disk** — the multi-GB `target/` and
     `node_modules` are dead weight until `housekeeping` prunes it:

     ```sh
     make clean
     ```

     Only on a confirmed merge; a dequeued PR keeps its build tree.

   - [ ] **Working tree clean** — `git status --short` is empty. A
     stray change can come from anywhere (a partial edit, a scratch
     file written into the repo), nothing else at the end of a review
     reads tree state, and the evidence is deleted with the worktree.
     After `make clean` on purpose, since it can touch the tree.
     Surface anything modified and decide where it goes; never report
     the run complete with a dirty tree.

   - [ ] **Session metrics** — step 21, unprompted.

   - [ ] **Close** — the follow-up ledger, then the one question.
     Name every operator-facing item as addressed, handed to a
     planning session, or filed as a task. Then ask via
     `AskUserQuestion` — *"all follow-up complete: metrics filed,
     feedback forwarded — mark Done?"* — offering **"Mark Done and
     remove the worktree (Recommended)"** first and **"Mark Done, keep
     the worktree"** second. Only this approval moves the issue to
     Done. Both run one tool as the session's **last** tool call, so
     the state write lands before any removal:

     ```sh
     python3 .claude/tools/close_session.py --state Done
     ```

     (`--keep-worktree` for the second option). It refuses with
     nothing changed when it cannot prove the close safe — a dirty
     tree, an unpushed commit, an open or unreadable PR, a lock another
     session holds, a failed state write — naming the reason in
     `refused`. Exit 2 means Done landed and the removal did not; relay
     its note.

   If you want the landed content anyway — a merge resolution, say —
   slice it with `python3 .claude/tools/show_at_ref.py <ref> <path>`;
   **never `git show <ref>:<path>`**, which prints the whole blob
   (`--no-patch` suppresses a diff, not a blob). That restates a
   general rule **at the call site on purpose**: a correct rule stated
   in one phase of a long skill has repeatedly failed to fire in
   another, and the cheap fix is the narrow form where it is needed —
   so a later de-duplication pass must leave it.
