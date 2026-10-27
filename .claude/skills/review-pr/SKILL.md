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
  — a proposal to trim that cap was rejected (a canceled record the
  fingerprint probe finds).

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
   `/commit-changes`). Then one call, **before** the rebase, captures
   the merge-base, fetches and triages what the base gained:

   ```sh
   python3 .claude/tools/rebase_overlap.py --base <base>
   ```

   **Probe signing first if the session has idled since its last signed
   commit** — here, and at every later re-rebase (steps 5, 6 and 11)
   after a fan-out, CI wait or pause: a rebase re-signs every commit it
   replays, so a locked signer leaves the worktree mid-rebase
   (`history.md` → "Signer locked mid-rebase"). Exit 2 is git failing,
   not the signer — report its line:

   ```sh
   python3 .claude/tools/signing_probe.py
   ```

   Then rebase — on `signed`:

   ```sh
   git rebase origin/<base>
   ```

   **On `LOCKED`, never stop: rebase unsigned and carry on**
   (`docs/conventions/commits-and-prs.md` → "Signing defers to the merge
   gate"). The whole flow — lint, fixes, CI — runs on unsigned commits,
   and step 20 signs the branch once:

   ```sh
   git rebase --no-gpg-sign origin/<base>
   ```

   Every "commit signed" below means the same: `-S` while the signer
   answers, `--no-gpg-sign` once it does not (`commit-changes`).
   **State the unsigned count at every gate** — this step, the step-12
   push, the CI wait, the handoff — in one line, so the exposure stays
   visible:

   ```sh
   python3 .claude/tools/resign_branch.py count --base <base>
   ```

   - **On conflict: `git rebase --abort`, catalogue it as blocking
     (step 7), and tell the user to resolve and re-run. Never
     auto-resolve** — not even an adjacent insertion into a sorted
     list, which lints clean after silently losing one side.
     `merge_completeness.py` makes a *manual* resolution auditable and
     never licenses an automatic one (`history.md` →
     "Merge-completeness evidence"):

     ```sh
     python3 .claude/tools/merge_completeness.py --path <file>
     ```

     List the conflicted files with
     `git diff --name-only --diff-filter=U`, then size each one's hunks
     before reading any:

     ```sh
     python3 .claude/tools/merge_completeness.py --path <file> --inspect
     ```

     A hunk with an **empty side** is the delete/modify shape — one
     side deleted a region the other edited inside — and the tool
     prints the surviving side's own diff for the file in its place
     (the base's, when this branch did the deleting): carry those few
     lines forward and never read the conflicted block (once 146 lines,
     for a 2-line change). Inspect the other hunks with `Read`
     `offset` / `limit` (`history.md` → "Conflict inspection").

   - **A clean rebase that integrated base commits** can still hide a
     *semantic* conflict (the base renamed something this branch
     calls): flag it for steps 5 and 11. History was rewritten, so step
     12 force-pushes with `--force-with-lease`.

   **Act on the triage's `rerun` block**: each of `lint`, `program`,
   `frontend_install`, `artifact_gates` and `may_enqueue_unverified`
   is a `run` boolean with its `reason` (the rules and their incidents:
   the tool's docstring; `history.md` → "Hand-rolled rebase triage").
   The rebase and every freshness gate always run; only re-runs vary.
   `lint: false` means assert — note "base moved to `<sha>`; overlap
   empty; results from `<sha>` still stand"; `lint: true` re-runs
   `review_diff.py` too. A `warning` field means the triage ran after a
   rebase that was **not triaged first** — re-run with the `--from` it
   names; after one already triaged here, it is noise. `program` →
   `python3 .claude/tools/run_quiet.py -- make program` before any
   suite; `frontend_install` →
   `python3 .claude/tools/run_quiet.py -- pnpm --dir frontend install`
   before any typecheck or lint. `runs_artifact_gates` stays for step
   9's rebase re-run question (step 11 reads `review_diff.py`'s
   `targets` instead); `--full` adds the file
   lists. A **hot surface** (overlap with several open PRs, per
   `review_diff.py --overlap`) predicts repeated rebases — advisory, for
   a narrower *next* PR (`history.md` → "Hot-surface tails").

   **Key a skip to content, not a SHA** (an amend or no-overlap rebase
   moves the SHA, not the bytes): record after a green lint, check before
   any re-run:

   ```sh
   python3 .claude/tools/tree_fingerprint.py record --check lint
   ```

   ```sh
   python3 .claude/tools/tree_fingerprint.py check --check lint
   ```

   `fresh` (exit 0) → assert and skip; `stale` → re-run; `missing` →
   run and record. It serves any check that is a function of tree
   content alone (`tools-tests`, the artifact gates), never CI or the
   queue.

1. **Check the Linear task, mark it In Progress, and tick what's
   done.** Autonomous runs ship partial checklists; confirm the diff
   delivers all of it before reviewing anything else.

   - **Resolve the tag** from the PR title scope (`type(ENG-###): …`),
     falling back to the branch name (the two share one number). If
     neither yields one, skip this step and say so in the report.

   - **Hold the body once per session**: the one `init-pr` already
     has, else one `get_issue` plus `list_comments` (`history.md` →
     "Cross-skill echo budget").

   - **Grade every requirement** — checkboxes (an `[x]` is a claim to
     verify) and requirement-phrased scope bullets — from
     `git diff origin/<base>..HEAD` and the commits, as **addressed**,
     **partial** or **missing**. A deferral is addressed only if it is
     recorded (a commit, a PR-body note, a `/linear-task` follow-up);
     a silent omission is missing.

   - **One write**: the addressed ticks and the move to In Progress,
     zero-echo, one `--box` per box (its text as written; a duplicate
     box is named once per tick):

     ```sh
     python3 .claude/tools/linear_issue.py tick --id <ENG-###> \
       --box "<box text>" --box "<box text>"
     ```

     It reclaims In Review, never regresses Done / Canceled, writes
     nothing when there is nothing to do, matches tag-bearing boxes,
     and retries a failed write once from a fresh read before stopping
     — its one line says which. A refusal (an unknown or ambiguous box)
     writes nothing: quote more of the box and re-run. Never invent a
     box for a non-checkbox requirement.

   - **Ticks that must wait for the fan-out ride step 20's In Review
     write** (`tick --state "In Review"`) — never a write of their own
     (`history.md` → "Deferred-tick echoes").

   - **A body addition that ticks nothing** — a deferral note, a
     remaining-scope line — goes through
     `linear_issue.py append --id <ENG-###> --file <path>`, never
     `save_issue` (`CLAUDE.md` → "Partial edits").

   - Catalogue every **partial** or **missing** requirement as
     **blocking** (step 7), quoting the checklist text and the deciding
     `file:line` or absence.

   - **"This will not fit in one PR" is not a second PR** on the same
     issue: finish this one self-contained, message the live planning
     session the remainder, and record the split as a PR-body deferral,
     which makes it addressed (`docs/conventions/commits-and-prs.md`).

1. **Run lint — every lint point is one call** that races the abridged
   lint and tests against CI for HEAD (`docs/ci.md` → "Racing local
   against CI"):

   ```sh
   python3 .claude/tools/verify_head.py
   ```

   Run it with `run_in_background: true` (it can outlast a foreground
   call). Its one line gives each check's settling side and log or job;
   act on it only when its `sha` is HEAD's. A local `make lint` / `make test`
   (through the quiet runner) runs only when `ci=` says
   `not-consulted(unreachable)` — `dirty` / `unpushed` heal by committing and
   pushing. To narrow one hook, call
   `lint_paths.py --changed -- <hook-id>`, or `--files <paths> --`; a
   local full sweep in between is a mistake unless a hook is
   repo-global (`history.md` → "Full-sweep loops").

   **The tool absorbs the two by-design failures**: when a hook
   rewrites files (`mdformat`, `ruff format`, `biome`, the dictionary
   sorter) it re-runs once and reports only the second result; when
   `biome` / `tsc` lack frontend `node_modules` it installs them, and
   if that fails, skips them and prints `UNVERIFIABLE locally` — never
   gate on such a hook; CI runs it. A rewrite that then passed is still
   uncommitted, and the quiet runner hides the notice on a pass, so
   check `git status --short` after every lint run and commit any
   rewrite signed.

   **Spelling pre-flight before the first full run** whenever the diff
   adds any comment, doc comment or markdown. The trigger is the SHAPE
   of what the diff adds, not the file extension, so a doc-heavy `.rs`
   change counts:

   ```sh
   python3 .claude/tools/run_quiet.py -- \
     python3 .claude/tools/lint_paths.py --prose
   ```

   Take the whole unknown-word list in one pass (`history.md` →
   "Serial spelling rounds"): write the US form of a variant, reword a
   coined word once to plain English, and place a real term per
   `CLAUDE.md` → "Docs and skills prose".

   **A formatter invalidates every line you hold for that file**:
   order edits before it runs, then one bounded read
   (`docs/conventions/context-economy.md` → "The levers"; `history.md`
   → "Formatter re-read loop"); see what it changed in the step-5
   slices, never a `git diff` of the file.

   **The `Lint` workflow also runs `make tools-tests` and
   `make decks-build`** — not hooks, so step 18's reproduce-the-hook
   advice does not apply. When the diff touches what a `.claude/tools/`
   checker validates (`.claude/tools/**`, `market-data/grafana/**`),
   run `python3 .claude/tools/run_quiet.py -- make tools-tests`, whole,
   before pushing; it runs on Linux, so a macOS-only test needs a
   `unittest.skipUnless` guard (`history.md` → "Lint workflow beyond
   the hooks").

   **Reading lint output and config**: filter a captured log with
   `python3 .claude/tools/run_quiet.py inspect <log> --grep 'Failed|error\['`,
   never a whole `Read` or a shell `tail`; grep
   `cfg/pre-commit-lint.yml` and the workflow as indexes, never read
   whole (`history.md` → "Log filtering", "Config-index reads"). Never
   hand-verify what the gate owns — an `awk` width check counts bytes.

   **Verify at checkpoints, matched to the edit.** An unchanged tree
   since the last lint → assert, don't re-run. A copy- or comment-only
   change needs lint, not a build or typecheck. `tsc --noEmit` is
   whole-project, so its lever is frequency: once, after the TypeScript
   edits are done (`history.md` → "Inner-loop checks").

   **A failure the tool reports is a real violation** → fix everything
   mechanical (formatting, imports, whitespace, spelling), stage by
   explicit path, commit signed (`git commit -S`), and confirm with the
   scoped run. Whatever still fails is **blocking** (step 7); don't
   mark the PR ready.

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
                                  //   generation_input, fixture_readers,
                                  //   disjoint
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
   it interacts: a file both sides changed, the lint config or a hook
   script, a generation input both sides touched, or a Rust file the
   base added or changed that reads a file this branch changed. Then
   rebase onto
   `origin/<base>`, triage as in
   step 2, and re-run the tool. **A disjoint drift (`ready: true`,
   `base_fresh: false`) needs no rebase at all**: note "base moved;
   drift disjoint" and carry on — the merge queue tests the merged
   result. Read `files` for **sizing and tiering**, not as a second
   freshness check. Pass `commits` inline; the diff lives only in the
   file. A rebase this gate asks for takes the signing probe first
   (step 2).

   The tool **owns** three path lists — the generated-family excludes,
   the mirror of `test.yml`'s `code` filter, and the generation inputs
   — which is why step 9's `regen_artifacts.py` imports its path gate
   and step 11 reads `targets` off the verdict. Change them there, never in
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

   - **Each lens's material NAMES its slice**, and a lens scoped to one
     tree gets that tree's `--only` diff (a tools lens
     `--only '.claude/tools/**'`; a conventions lens
     `docs/conventions/**` plus `CLAUDE.md`).
   - **A second slice needs a one-line justification** in the material —
     why its question spans the seam. Print each lens's **slice count
     and handed-in byte count** at spawn, so an over-broad
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
     (≈3× the ceiling, ≈3× the turns), passed as `--cap`; otherwise
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

   **Compose each lens's brief with the tool, never by hand.** Every
   agent-facing rule lives in committed files the tool assembles in its
   own process, so this step holds none of it (`history.md` →
   "Preamble economics"): the canonical shell rules;
   `lens-standing.md` (scope and negative scope, budget and hard stop,
   the lint carve-out, the pre-emit gate, the standing suppressions,
   the report shape with its checks-to-run slot and self-report); and
   the lens's own file under `lenses/` (its dimension, standing
   sub-questions, what it is handed and never handed, its default
   cap). One call per lens, the step-6 cross-check included; `--lens`
   refuses a name with no file:

   ```sh
   python3 .claude/tools/lens_preamble.py \
     --out <scratchpad>/brief-<lens>.md --lens <lens> \
     --facts-file <scratchpad>/facts.md
   ```

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

   **Emit the briefs at the spawn gate; a missing file is a stop —
   re-emit it and re-check.** Run `lens_preamble.py` immediately before
   spawning, in the same gate that checks `review_diff.py`'s `ready`,
   and check the paths in the same command sequence that spawns, so it
   fails loudly there — a session resume can strip a file emitted
   earlier, and the fan-out then runs with no shell rules and no
   suppressions while reading as normal (`history.md` → "Missing
   preamble").

   **The per-lens material travels by file too.** Write each lens's
   block — handler semantics, fixture helpers, known negatives,
   measured values, the diff path it reads — to
   `<scratchpad>/lens-<name>.md`, so a retry after a transient failure
   re-sends a path, not the brief (`history.md` → "Per-lens
   retries"). The spawn prompt is three lines:

   ```txt
   Read <scratchpad>/brief-<name>.md first — it is your brief.
   Then read <scratchpad>/lens-<name>.md, the material for your lens.
   Then: <scope line, and at most three sub-questions>
   ```

   **Excerpts inline: if the main loop has read it, the excerpt goes
   in the material.** This is the single highest-value lever in the step,
   for **every** lens, and it is *input*-scoping, not fewer lenses
   (`history.md` → "Cold-reading held context"). Material never names a
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
   read one and diff the other against it. **Each lens file's "What you
   are handed" section is the shaping rule for its material** — the
   docs lens's claim → excerpt table, the style lens's sibling excerpts
   pasted, the freshness lenses' positive scope — and its "never" half
   is what the material must not hand over.

   **Bound every lens; the brief files carry the wording.** The scope
   line, the hard-stop cap in turns **and** tool calls (`history.md` →
   "Hard-stop wording"), the checks-to-run slot and the self-report are
   in every brief already. The material and prompt supply what varies:

   - **The cap, when not the lens file's default** — `--cap '<cap>'`:
     6 turns for a lens holding a section map, the prose-lens exemption
     above. The cross-check's higher cap is its file's default.
   - **At most three enumerated sub-questions.** The wording is
     necessary and not sufficient; sub-question count is where the
     remaining variance lives (`history.md` → "Sub-question count").
     Any sub-question the main loop can answer gets *answered* in the
     material — a known answer is a fact with a question mark on it. More
     than three means you are surveying, or the lens should split.
   - **A cap the ask can satisfy.** When the question is genuinely "do
     these two large files agree in their entirety", either **hoist the
     extraction** into the main loop and hand over the pairs
     (preferred), or state the higher cap and why. Silently keeping ≈6
     is the one option always wrong.

   **Run each lens's "checks to run" list** in the main loop before
   step 7 and fold each result into the catalogue as one line.
   **Re-shape an inherited check first**: the lens phrased it for
   adjudication, the main loop usually needs existence, and the right
   tool may differ (`history.md` → "Checks to run"). Exemplar figures
   are **summed per-turn input** from `session-metrics`' rollup, not
   the Agent tool's `subagent_tokens` (`history.md` → "Exemplar
   figures").

   **Gate a lens on its historical YIELD too.** A lens returning zero
   findings across **ten or more** dispatches is auto-skipped, and the
   skip is named in the summary. **Insurance lenses are exempt** —
   security canonically, and any lens whose absence would surface only
   in production (`history.md` → "Yield gating").

   **A resume is a report-now order.** A `SendMessage` resume re-sends
   the lens's whole context, so it says **"report now"** (the brief
   bounds that to two more tool calls), never an open continuation; a
   lens that cannot conclude is reported unfinished, not resumed again
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

   - **Trivial short-circuit — one scoped reviewer (plus the gated
     freshness lenses), no cross-check.**
     Small and confined to one of: comment / doc / Markdown only, a
     config or workflow tweak, a rename, or a handful of lines with no
     new control flow. The one reviewer is the `correctness` lens,
     scoped to what the diff's nature calls for, plus the freshness
     lenses for a `CLAUDE.md` or skill edit.
   - **Reduced fan-out — one scoped lens matched to the risk, no
     cross-check.** A diff that may be *large* but whose risk is
     **narrow** (`history.md` → "Reduced-fan-out band"):
     - **infra / ops** touching no program, SDK or app control flow
       (Dockerfile, compose, a make target, CI YAML, docs) — one
       `ops-correctness` lens.
     - **faithful extraction / move refactor** — code deleted in one
       place reappears verbatim elsewhere. One `move-fidelity` lens,
       handed the Grep-tool straggler hit-list (`history.md` →
       "Straggler grep into build output"). Add **one** small new-logic
       lens only when genuinely new additions rode along, and never turn
       correctness / completeness / style loose on unchanged logic. This
       holds at any size: it overrides the full-tier "four substantive
       lenses" default.
     - **mechanical repo-wide reformat** — discount the reformat noise
       from the sizing; one `move-fidelity` lens, scoped to whether a
       semantic change rode along.
     - **value / default rewiring with no new control flow** — one
       `correctness` lens, scoped to the new values and their call
       sites.
     - **test-only** (`#[cfg(test)]`, `tests/`, no production change) —
       one `test-validity` lens.
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
   run the `comment-accuracy` lens over the diff's own added comments,
   at zero changed markdown lines, handing it the source slice's
   comment hunks and the items they document, under the two-section cap
   below. It is step 4's spelling-pre-flight
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
     the upkeep pass calls, so the two paths cannot drift on what
     dangles. It separates `missing-file` from `missing-anchor` and
     exits 1 on any; fold the result into the catalogue and hand the
     lens only the judgement half, whether the prose still describes
     the code:

     ```sh
     python3 .claude/tools/convention_refs.py
     ```

   - **Cap the freshness lens at two named sections, excerpts inline.**
     A material file naming several files is a reading list, not a
     scope; the lens file states the restriction, and a permission
     phrasing in your own material is the tell that this lens will run
     long.

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
     ask, a dictionary ≥ 2-files census included; the brief tells the
     lens not to re-derive it. **Hoist it and narrow it**: a hoisted sweep whose
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

   **The lint gate owns the compile-time facts** (`history.md` →
   "Re-deriving compile facts"); every brief carries the carve-out, so
   the facts supply only **which hook covered this diff's languages**
   and whether one could not run (a fresh worktree has no frontend
   deps, step 4). It holds because step 4 passed **before** the
   fan-out; if lint was skipped or is failing, say so in the facts.
   **Run the dead-code / unused-symbol grep ONCE, in the main loop**,
   and hand the result set to completeness and the cross-check.

   The standing suppressions (`history.md` → "Security lens on
   tooling") and the pre-emit gate — quote the changed line or drop
   it; severity **blocking** / **warning** / **nit**, no numeric scale
   (`history.md` → "Speculative findings"), a convention claim citing
   its definition site (`history.md` → "Convention by analogy") — are in
   every brief too.

   Spawn the lenses as parallel `Agent` calls in a single message, each
   with its three-line prompt.

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

   At minimum, each lens's dimension being its file under `lenses/`:
   `correctness` (its observability sub-question: `history.md` →
   "Healthy while broken"); `security` under the trust-surface gate
   (defensive framing on a hooks diff: `history.md` → "Guard-bypass
   lens framing"); `style`, skipped on a meta-work diff;
   `completeness`; and `doc-freshness` and `ci-skip-list` under the
   surface gate.

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

   Spawn a fresh sub-agent on the `cross-check` brief
   (`--lens cross-check`), its material naming the **full**
   `<scratchpad>/review-diff.txt` rather than a per-lens slice, the
   collected findings in a file of their own, the hoisted grep result
   set, and the fix commits when they apply. The brief orders it to read
   the diff and list its own findings **before** opening the findings
   file, then adjudicate, so independent misses and false positives are
   both caught in the one agent; its cap, the lint carve-out and the
   stale-prose rule are in it too (`history.md` → "Cross-check cap",
   "Refuted against the fixed text").

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
   push and re-run step 4's `verify_head.py` call — it re-tests the
   affected crates too — with step 4's fix-and-retry logic, scoped
   per-hook re-run included. Run a fast
   suite **whole, through the wrapper, at checkpoints rather than
   after every edit** — the lever is frequency, not scope
   (`history.md` → "Fast-suite frequency").

1. **Regenerate committed generated artifacts
   (mirror the IDL / SDK / vectors / WASM CI gates).** CI fails the
   PR on a committed generated file stale against its source:
   `test.yml` regenerates the **IDL**, and `sdk.yml` the **SDK
   clients**, the **conformance vectors** and the **committed WASM
   glue**. One call applies step 5's path gate, runs all four targets
   in dependency order through the quiet runner's capture, and commits
   any diff signed under its canonical message:

   ```sh
   python3 .claude/tools/regen_artifacts.py --base <base>
   ```

   Read each family's `status` and record it in the summary:
   `fresh`, `committed`, `skipped` (no generation input changed — a
   rule, not a judgment call), `failed` (a build or commit error:
   read its `log`, and it is blocking), or **`unverifiable`** (a
   toolchain check aborted the target: name it, continue with the
   committed artifact, and never count it as green — the tool exits
   3). It runs all four whenever any input moved, even when the
   author says they regenerated, since a subset spot-check has twice
   let a stale artifact reach a required-CI failure (`history.md` →
   "Partial regeneration"). **Run it again after any step-7 / step-8
   fix that touches a generation input** — it is not one-shot
   (`history.md` → "Stale twice"). A run that seems hung is the cargo
   build lock until shown otherwise (its `⏳` line names it): wait, or
   ask the user to stop the other build.

   **When the `vectors` family reports `committed`**, learn whether
   book behavior moved or only the embedded market bytes from the
   one-line-per-file verdict (exit 1 when an expected output changed),
   never a `git diff` of the byte arrays. Compare against the parent
   of the family's reported `commit` — the default `HEAD` already holds
   the regenerated vectors, so it reads `unchanged` whatever moved:

   ```sh
   python3 .claude/tools/vector_diff.py --ref <commit>~1
   ```

   When `lint_rerun` is `true`, re-run `verify_head.py` with step 4's
   fix-and-retry logic — a regenerated file
   can still trip the whitespace / EOF hooks.

   **A re-run forced by a rebase asks a narrower question.** The
   first run over the author's own diff stands as above; when this
   step re-runs only because `main` moved, the question is *can the
   base delta have staled anything?* Assert the gates once, skip the
   rebuild and note it in the summary when either holds:

   - step 2's `rebase_overlap.py` reports `runs_artifact_gates: false`
     — the base delta touched no generation input; or
   - `review_diff.py`'s `runs_artifact_gates` is `false` (this
     branch touches no input) **and** `rebase_overlap.py --full`'s
     `base_files` includes the regenerated artifacts alongside the
     inputs that changed (the base's commits merged through the
     queue, which regenerates them).

   The merge queue's fail-closed re-run covers the residual risk
   (`history.md` → "Redundant rebase rebuilds").

   **Whether CI runs a gate is per-WORKFLOW, not per-diff.**
   `make idl`'s gate sits inside `test.yml`'s path-filtered job
   (whose filter step 11's `targets` mirrors); `make sdk`,
   `make check-conformance-vectors`, the WASM glue gate and the
   `sdk/ts` suite live in **`sdk.yml`, which has no path filter** and
   so applies to every PR (`history.md` → "Per-workflow
   re-derivation").

   **Two gates sit on the WASM glue.** `sdk.yml` compares the
   committed glue against a fresh build (the optimized `.wasm` binary
   excluded, since `wasm-opt` isn't byte-reproducible), and
   `sdk/ts/src/wasm.conformance.test.ts` runs the **committed**
   binary against the conformance vectors — so a stale binary is a
   real behavioral divergence, not a formatting nit.

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

1. **Run the test suite (mirror CI).** Read the suites from a
   base-freshness re-check — **its own call, awaited before the push
   or any suite launch**, since step 5's verdict is several steps old
   and a sibling session merging its PR moves `origin/<base>`
   underneath this one (worktrees share one `.git`):

   ```sh
   python3 .claude/tools/review_diff.py --base <base> --gate-only
   ```

   **`targets` is `null` while a rebase is due** (a stale base that
   interacts with the branch, or an unverified one): rebase onto
   `origin/<base>` — signing probe first (step 2) — and re-check, so
   the expensive builds run once, against the tree that will merge
   (`history.md` → "Suites before the rebase"). Switch to a full
   `--split --out` run only when the slices need regenerating.
   Otherwise `targets` lists every suite in run order as
   `{target, run, reason}`. A `make test` entry is step 4's
   `verify_head.py` call after step 12's push (plain `make test` only
   on `not-consulted(unreachable)`); run each other `run: true` entry
   through the quiet runner (`python3 .claude/tools/run_quiet.py -- <target>`),
   and record every skip with its `reason` in the summary — never
   re-derive the list. It resolves CI's `code` filter
   (`CODE_FILTER_EXCLUDES`, kept in parity with `test.yml`), whether
   a cargo build can reach the diff, the fixtures a Rust test reads,
   an asm relayout (`make test-parity` — plain `make test` silently
   skips `asm_parity` without its oracle, so only a `test-parity` pass
   is the evidence), a file holding an `#[ignore]`d test
   (`make test-postgres`), and a tools / Grafana change
   (`make tools-tests`) — the provenance is in `history.md` →
   "Unreachable suites", "Fixture-coupled dequeue" and "Ignored test
   reddened CI". The `code` filter is **`pull_request`-only**, so a
   `merge_group` run executes the full suite regardless.

   Two calls stay judgement, outside the list:

   - **The inverse half**: run a check when some changed path is
     reachable from it even if the diff misses its trigger's
     enumerated tree list (`history.md` → "Missed trigger").
   - **The CI-mirror carve-out**: when the diff is confined to
     `.github/**` and CI is already green on the head commit, treat
     CI as the mirror and skip the local suite (`history.md` →
     "Half-applied build").

   **Batch independent program mutations into one sBPF build.**
   Verifying a finding by mutating the program costs a
   `make program` per state, so apply every independent mutation,
   build once, run once, revert once; only a mutation another would
   mask needs its own cycle.

   **When the FIX is a new test, mutation-verify it** — a green
   suite is exactly what a worthless test produces. Reintroduce the
   defect and confirm the new test fails **on its behavioral
   assertion**, not on a precondition:

   - **Verify by test count, not exit code.** The runner's summary
     line carries the counts, and this prints the deltas between two
     runs' logs:

     ```sh
     python3 .claude/tools/run_quiet.py inspect <mutated> --compare <baseline>
     ```

     Its verdict is `caught` (more tests failed — a fail-fast run that
     stopped early still counts), `not-caught`, or `inconclusive` (no
     new failure and fewer ran — the invocation excluded the target).
     A feature-gated target needs `--all-features` (or the explicit
     `--features`), and the same holds for any conditional
     compilation; take the invocation CI uses, and narrow `--files` /
     `--test`, never the package set. After adding tests, confirm
     `passed` moved by the number added.

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

   **One cargo target at a time per worktree** — a blocked build is
   **waited on, never raced**: two builds on one `target/` produced a
   spurious `E0463 can't find crate for dropset_sdk` doctest failure
   (`history.md` → "Raced cargo builds"). Other worktrees sharing the
   cargo pool can still race it. **Recover with `cargo test --doc`,
   never a second `make test`**: on an `E0463` doctest failure with
   zero test failures in the log, run it once, through the quiet
   runner, and read the pair as the green `make test`. A full re-run
   only races again, and cargo stops at the first failing doctest
   crate, so a second full run still never reaches the later crates'
   doctests:

   ```sh
   python3 .claude/tools/run_quiet.py -- cargo test --doc
   ```

   **Run the suites only once the fix commits have landed**: the build
   compiles the tree as it finds it, so hold them while the fan-out is
   still reporting or fixes are still going in (`history.md` →
   "Half-applied build"). Background any other suite alongside the
   `verify_head.py` call (`run_in_background: true`).

   - Both depend on the Solana/Anchor toolchain via
     `check-toolchain`. If the toolchain is absent
     and a target aborts before any test runs, you
     **cannot** verify that workflow locally — say
     so explicitly in the report (do not claim CI
     will pass), rather than counting it as green.

   - The `sdk/ts` suite is the *only* local gate that catches an
     on-chain layout change breaking the SDK's hand-built fixture
     (`market.test.ts`); install its deps once if needed
     (`pnpm --filter @dropset/sdk install`).

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

   **If the branch carries unsigned commits and the signer answers
   now, sign here instead** — the tool pushes with its own pinned
   lease, so the CI run that follows is on signed commits and step 20
   has nothing to do. On `LOCKED` (exit 1) it moved nothing: push as
   above and leave the signing to step 20. On exit 2, read the refusal:
   a merge commit or a dirty tree still pushes as above, unsigned —
   never hold work back before the CI wait; only **"never held"** (the
   remote carries a commit this branch lacks) holds, since a forced
   push of any kind would drop it — report it and reconcile:

   ```sh
   python3 .claude/tools/resign_branch.py resign --base <base>
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
   during a long review. One call reads the mergeability tri-state,
   settling an `UNKNOWN` itself; step 20 makes the same call:

   ```sh
   python3 .claude/tools/wait_for_checks.py --mergeable <number> \
     --verdict-out <scratchpad>/mergeable.json
   ```

   Act on `hint.gate`: `green` (`mergeable`) proceeds; `blocking`
   (`conflicting`) means don't mark ready — tell the user to rebase onto
   `origin/<base>`, resolve, and re-run `/review-pr`; this skill **never
   auto-resolves a conflict** (step 2 says why, even for a trivial
   sorted-list collision); `unverified` (`unknown`, `probe_failed`)
   holds.

1. **Gate.** Mark the PR ready only when **every** local CI-mirroring
   check is green: **zero blocking issues** (every Linear checklist
   item addressed); the last `verify_head.py` verdict is `GREEN` for
   HEAD's `sha` (with a local `make lint` / `make test` passing where it
   said `not-consulted(unreachable)`), any hook `lint_paths.py` printed
   `UNVERIFIABLE` noted, not gated; the generated artifacts
   are fresh and committed (every `regen_artifacts.py` family fresh,
   committed or skipped, or an `unverifiable` one honestly reported);
   every `run: true` entry of step 11's `targets` passes or is
   honestly reported unverifiable, and the **`sdk/ts` node suite
   passes** (always runnable); the title
   passes `Semantic PR`; and step 16's gate is `green`. Then:

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
   wait** — this one and the step-5 fan-out (`history.md` → "Worktree
   deleted under a wait"). Make the checkpoint commit and push it
   immediately before the wait — **unsigned if the signer is locked**,
   never held back for an unlock: an unpushed commit is the alarm, an
   unsigned one is not.

   **Wait with the tool — never poll.** One call snapshots the working
   diff (uncommitted and untracked work included, so recovery is one
   `git apply` on `snapshot.merge_base` even when the commit could not
   be made), blocks until the checks settle, and prints one compact
   verdict; the tool's docstring has the schema:

   ```sh
   python3 .claude/tools/wait_for_checks.py --pr <number> \
     --snapshot <scratchpad>/pre-wait.diff --base <base> \
     --verdict-out <scratchpad>/ci.json
   ```

   A `snapshot.error` means the spill did not happen — say so: the wait
   is running with that work unprotected.

   **The never-poll rule, for every wait in this skill** (this one,
   the step-20 mergeability re-read, the step-22 queue watch): no
   shell `while … sleep` loop, no `jq` filter, and never a **bare**
   `gh pr checks --watch` or `gh run watch`, which re-prints its whole
   table every refresh into one replayed result (`history.md` → "Bare
   watch cost"); `--watch` is right only inside the tool.

   **Say once, then silent** — the same rule for the step-22 queue
   watch: tell the human once that CI is in flight, then say nothing
   until the verdict. If the session was interrupted and the wait is
   gone, run the tool with `--no-watch` for one current read, then
   plain to resume.

   **Act on `hint`** — `gate` is exhaustive and `do` names the action:
   `green` (`pass`, or a `none` whose conflict probe read `MERGEABLE`)
   prints the summary; `blocking` is `conflicting` (no run will ever
   arrive — rebase) or `fail`; `unverified` (`timeout`, an unsettled
   `none`, any other conclusion) is never a pass; `retry` (a
   `--no-watch` `pending`) re-runs plain.

   **On `fail`**, catalogue each `failing` entry as **blocking** with
   its `link`, then run each non-null `failed_steps[].next_command` (an
   `external` check's third-party `link` is opened, never run) — the tool
   has read the step conclusions and routed on the step's `kind`
   (`note` says how to read the result). Never `gh api` a job log, and
   a `fetch_failed` record is not "nothing found". A `lint_hook`
   reproduces through `lint_paths.py`, never bare `pre-commit run`,
   which cannot see an un-added file; skip a hook that can't run here,
   as step 4 does.

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

   Write the facts only this session holds to
   `<scratchpad>/summary-facts.json` — `review_summary.py --schema`
   prints its shape, so never grep the tool for it — then render; the
   CI and merge lines come from the verdict files, never from the
   facts:

   ```sh
   python3 .claude/tools/review_summary.py \
     --facts <scratchpad>/summary-facts.json \
     --ci <scratchpad>/ci.json --mergeable <scratchpad>/mergeable.json
   ```

   The facts carry: each checklist item addressed / partial / missing;
   the tier that ran (and the proposed one, so an override is named),
   the lenses that ran, and each skipped lens with its reason — **a
   yield skip is never silent**; the cross-check; in-flight overlap
   and what was decided; lint and artifacts as pass / fail /
   **unverified**, never a guessed pass; each `targets` entry likewise,
   or `skipped: <reason>` for a suite step 11 skips **by rule** — a
   by-rule skip is not a gap, so never mark it unverified, and never
   run a suite the rule skips just to have a value to report (`tests`
   is never `{}`: a prose-only review lists each skipped suite); the
   freshness and skip-list
   verdicts, or "skipped (surface gate)"; findings found / fixed /
   blocking-open, the fix-cycle count, and each warning or nit with
   `file:line` and rationale. Write the **findings paragraph** — the
   one part in prose. Print the output; a `Converged: NO` (exit 1)
   means a blocking finding, a missing item or a red gate is still
   open, and the run does not proceed to step 20.

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
   python3 .claude/tools/wait_for_checks.py --mergeable <number>
   ```

   With the stakes raised: on `blocking` neither enqueue nor advance
   the issue — report it, tell the human to rebase, leave the issue In
   Progress, and stop. On `unverified`, say so and hold. Only `green`
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
   suite against the merged result, fail-closed. Both required:
   `rebase_overlap.py`'s `rerun.may_enqueue_unverified` is `true` (no
   overlap; the queue's full run covers any delta), and the branch's
   own diff is **unchanged** since the last green local pass. Otherwise
   verify locally. Say in the summary that the re-verify was skipped
   and why.

   **Then the final signing pass — nothing is enqueued unsigned.**
   Branch protection requires every commit on main signed, so a branch
   still carrying unsigned commits (step 2's count) is signed here, in
   one pass, before the enqueue:

   ```sh
   python3 .claude/tools/resign_branch.py resign --base <base>
   ```

   It replays the linear branch onto its own merge base, signing each
   commit (tree unchanged, so no conflict), and force-pushes with a
   pinned lease — routine, no approval needed. Exit 0 with
   `nothing to sign` is the common case. **On `LOCKED` (exit 1) it
   moved nothing**, and the handoff question below becomes the one
   operator turn the deferral costs: "ready to merge — unlock
   1Password and I sign and enqueue", re-running this after the yes.
   Exit 2 is a refusal (a dirty tree, a merge commit on the branch, a
   remote commit this branch never held) — report it and hold. **A
   re-sign changes every SHA, so the required checks re-run** on the
   new head; `gh pr merge --auto` below waits for them, and step 22's
   watch covers the enqueue.

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

   With step-3 ticks deferred to here, the same move carries them in
   one write instead:
   `linear_issue.py tick --id <ENG-###> --state "In Review" --box …`.

   **One write, no retry loop**: if it disagrees, verify once, report
   the discrepancy and move on (`history.md` → "State-write echo").

   Ask with `AskUserQuestion`, printing **both identifiers** — the
   Linear tag (`ENG-536`) and the PR number (`#138`) — with "yes, add
   it to the merge queue" **first** and "skip, I'll merge by hand"
   second. With the signer locked, the first option reads "unlocked —
   sign and enqueue", and the unsigned count is in the question.

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
   its own process (the transcript never enters context), and writes
   each trim lever into the final message's `Session account` block,
   filing nothing — the digest lifts it for the fold analyst. It
   authors no source edit. **Ground it in
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
     "enqueued_at": "…",
     // on dequeued only:
     "dequeue": {"dequeue_reason": "merge_conflict", "base_gained": […], …},
     "log_path": "…/wait-for-queue-<number>.log"
   }
   ```

   It polls the GraphQL `mergeQueueEntry` probe, the
   `gh pr view --json state,mergedAt` read, and this PR's
   `gh-readonly-queue/…/pr-<number>-…` runs each minute, stays silent
   while queued, and exits 0 only on `merged` — having first marked the
   PR's notification threads done (`notifications` in the verdict). It
   owns the reasoning
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

   - **`dequeued`** → read the cause off the verdict's `dequeue`
     first. **`dequeue_reason: merge_conflict`** means a PR that
     landed first conflicts with this one; the queue never built, so
     empty `queue_runs` / `failing_runs` are expected, and
     `base_gained` lists the base's commits the head lacks — the ones
     the rebase must absorb. Go straight to step 2's rebase and
     re-run from there: no timeline query, mergeability read or
     `git log` to rediscover what the verdict already says. When
     `dequeue.error` is set, read by hand only what is `null` — the
     reason, or just `base_gained` after a failed compare.
     Any other reason → diagnose from the **queue branch's** runs, never
     the PR's own checks: the queue builds `main` plus this PR on
     `gh-readonly-queue/<base>/pr-<number>-<sha>`, so a failure there
     dequeues the PR while every PR check stays green. The tool reads
     those runs on every poll because concluding without them once
     cost a blind re-enqueue: on PR #311 the first probe *was* a
     terminal dequeue — the queue's full run set had completed and
     `SDK` failed transiently while all 8 PR checks stayed green — and
     the re-enqueue was right only by luck. Key on **job**
     conclusions (a run can read `in_progress` with a job already
     failed), then read the failed run's log through the quiet runner:

     ```sh
     python3 .claude/tools/run_quiet.py -- \
       gh run view <run_id> --repo DASMAC-com/dropset --log-failed
     ```

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

     Report the removal either way, naming the job (or the reason). An
     empty `failing_runs` with a non-conflict reason means the entry
     vanished with every run green for the tool's whole grace window —
     `dequeue_reason` is the timeline's answer; weigh it before
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

   - [ ] **The PR's notification is already dismissed** — the watch
     marked its threads done on `merged`, before it exited, so no turn
     is spent on it. Read the verdict's `notifications`: `error: null`
     needs nothing (`dismissed: 0` just means already cleared). On an
     error, say so in the report and leave it; the upkeep pass's
     finished-PR notification sweep is the fallback. Never dismiss by hand or
     `mark_all_notifications_read`.

   - [ ] **Reclaim the worktree's disk** — the multi-GB `target/` and
     `node_modules` are dead weight until the worktree is pruned:

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
     planning session, or filed as a task — each filing messaged to
     the curator (`curate-<day>`, via `ListAgents`; with none live,
     the scheduler). Then ask via
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
