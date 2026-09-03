---
name: review-pr
description: Adversarial pre-review — mark the Linear issue In Progress on invocation, verify its checklist is fully addressed, lint, catalogue issues, fix what's mechanical, ready the PR, wait for GitHub CI to pass, print the review summary, then at the merge-queue handoff re-check the PR still merges cleanly before moving the issue In Review and offering to enqueue the PR, capture session metrics and then firm up permissions (the last interactive step) while it sits in the queue, and report whether it merges or gets taken back out.
user-invocable: true
---

<!-- cspell:word unstarted -->

<!-- cspell:word pathspec -->

<!-- cspell:word retarget -->

<!-- cspell:word sortedness -->

# `review-pr`

Act as an adversarial reviewer before the human
looks at the PR. Run lint, audit the diff,
catalogue every issue, fix what can be fixed
mechanically, and mark the PR ready only when
it's clean. Then wait for the real GitHub CI to go
green and print the review summary. Invoking this skill
moves the Linear issue to **In Progress** at the start
(reclaiming it even from In Review if a prior run
advanced it), and it stays In Progress through all of
this. It moves to In Review
only at the merge-queue handoff, the point at which
it's the human's turn to look at the ready, CI-green PR
and approve enqueueing it — so when this skill prompts,
the human can merge (or let "Merge when ready" land it)
with nothing left to check.

Run this after autonomous work is complete and
all changes are committed and pushed.

GitHub reads and writes go through the **GitHub MCP**, with
the deliberate `gh` exceptions in `CLAUDE.md` → "GitHub via
MCP": the merge-queue **enqueue** (a `gh pr merge --auto`
write) and **dequeue probe** (a `gh api graphql` read) at
the handoff, because the MCP exposes no merge-queue tool and
its `pull_request_read` omits `mergeQueueEntry`; plus the
**one-shot and watched reads** this skill makes with the
compact `gh pr checks` (the CI wait — step 17 runs it under
`--watch`, so gh blocks until the checks settle rather than
this skill polling) and field-selected `gh pr view --json`
(the PR lookup in step 1 and the merge-clean check) — chosen
because those reads repeat, and a full-object MCP payload
would be replayed into context on every later turn
(`CLAUDE.md` → "Context economy"). The
PR-authoring **writes** (`create_pull_request`,
`update_pull_request`) stay on the MCP. This repo is
`DASMAC-com/dropset`, so every MCP call takes
`owner: "DASMAC-com"`, `repo: "dropset"`.

## The entry gate: one question, tier and spawn together

**Step 0, and it is the only interactive gate before the
merge-queue handoff.** One `AskUserQuestion` approves *both*
"run `/review-pr` now" and **at which tier** — and that tier
choice **is** the authorization for the adversarial sub-agent
fan-out. No separate spawn question is ever asked mid-flight
(step 5 says the same from the other side).

Where it is asked depends on how the skill was reached:

- **Via `init-pr`'s handoff** — that handoff question *is* this
  gate. `init-pr` computes the signals and offers the tiers in
  the same question it uses to ask whether to run the review.
- **Invoked directly** — ask it here, before step 1.

Either way: exactly one interaction, and it carries the tier.

**Scope the tier from objective signals, not from vibes.**
Run `review_diff.py` first so the numbers are real, and read
from its verdict: files changed, total line count, and whether
the diff touches program code, the SDK surface, migrations, or
generation inputs, and whether it spans crates.

**The default is the full adversarial suite.** Assume the PR
carries a lot of context. The default is never inline and never
silently reduced — put the full suite first and label it
recommended.

**Propose a reduced tier only when the signals sit under a
small-diff threshold**: **≤ 5 files and ≤ 60 changed lines,
and** the diff touches none of program code, the SDK surface,
migrations, or generation inputs, **and** it does not span
crates. Those numbers are a starting calibration, not a law —
the case they are drawn from is a **four-line single-file
fix** whose full suite the operator had to talk down by hand.
State the signals in the question so the choice is informed.

A reduced tier still includes an **independent cross-check**.
Zero-agent inline is not a tier — it remains a stop condition
(step 5).

**This does not trim the cross-check on a normal diff.** Tier
scoping changes **how many lenses run on a trivially small
diff, on explicit approval**; it never reduces the
cross-check's depth or turn cap on an ordinary one. (A
separate proposal to trim that cap was considered and
**rejected** — see the closed record under the `Trim levers`
milestone. The two are not in tension, and this note exists so
the rejection is not read as superseded.)

**Why `review-pr` asks and `audit` does not.** Invoking
`audit` / `audit-scope` / `housekeeping` *is* the
authorization for their fan-out, with no question at all —
because the fan-out is their entire deliverable, and there is
no earlier gate for it to ride. `review-pr` does many things
and the fan-out is one step of them, so an operator could
reasonably want the lint-and-CI half without authorizing eight
agents and ~2.9M of sub-agent input. The asymmetry is
deliberate, and it costs nothing extra: the question was
already being asked to start the review.

## Steps

1. **Locate the PR.** Identify the current branch
   (`git branch --show-current`), then look it up with a
   **field-selected** `gh pr view` — passing only the fields
   the later steps need, so the lookup doesn't drag the full
   PR object into context (per `CLAUDE.md` → "Context
   economy" / "GitHub via MCP"):

   ```sh
   gh pr view <branch> --json number,title,state,isDraft,baseRefName
   ```

   That returns just `number`, `title`, `state`, `isDraft`,
   and `baseRefName`. **`baseRefName` is the branch this PR
   actually merges into** — `main` in the common case, but
   another PR's branch on a *stacked* PR. Every base-relative
   step below (the step-2 rebase, the step-5 review diff, the
   step-9/10 gates) is written against **`origin/<base>`**,
   which means `origin/` + this `baseRefName` — substitute it
   once here rather than assuming `main` and hand-correcting
   at each call site. If the branch has no PR, `gh` exits
   non-zero
   ("no pull requests found") — treat that as "no PR" and
   stop, telling the user to run `/init-pr` first. (This is a
   read; the routine PR-authoring **writes** in later steps
   stay on the MCP.)

   **If `baseRefName` is not `main`, check whether that base
   has already merged — before anything expensive.** One
   field-selected read answers it:

   ```sh
   gh pr view <baseRefName> --json number,state,mergedAt
   ```

   A `state` of `MERGED` means the stacked base landed on
   `main`, almost certainly **squashed** — so the base branch
   and `main` no longer share the commits this branch was
   rebased onto, and every base-relative step below would run
   against a branch that is no longer the merge target. When
   that is the case, retarget **now**:

   - `mcp__github__update_pull_request` with `base: "main"`,
   - `git rebase --onto origin/main <old-base>` to move this
     branch's own commits across,
   - and use `main` as `<base>` for every step below.

   This costs one cheap read and is deliberately placed
   before step 4, because the failure it prevents is
   expensive and silent: a review once ran end-to-end against
   a merged base, went green on CI, and only turned
   `CONFLICTING` at the step-15 merge-clean check — after
   which recovery cost a `git rebase --onto` **plus** a full
   re-run of `make lint`, `make test`, and
   `make test-no-teardown` (~25 minutes). Neither the step-2
   rebase nor step 5's `base_fresh` gate catches it: both
   compare against `origin/<base>`, which is perfectly fresh
   — it is the *base itself* that is stale.

1. **Clean tree, then rebase onto the PR's base.** First run
   `git status` — if there are uncommitted changes,
   stop and tell the user to commit first (or run
   `/commit-changes`). Then rebase onto the latest
   **`origin/<base>`** — the `baseRefName` step 1 read, not a
   hard-coded `main` — so the review runs on the state the
   branch will actually merge as, instead of a base that has
   drifted while the work was in flight; this is what
   minimizes file conflicts at merge time. Pass the base
   literally (it's `main` on an ordinary PR, another PR's
   branch on a stacked one):

   ```sh
   git fetch origin <base>
   git rebase origin/<base>
   ```

   Reading the base rather than assuming `main` is what keeps
   a **stacked** review from needing hand-substitution at
   every base-relative step — and, when the base PR merges
   mid-review, the fix is to retarget the PR base and re-read
   `baseRefName`, not to patch each command.

   - If the rebase **conflicts**, abort it
     (`git rebase --abort`), catalogue the conflict as
     a **blocking** issue (step 7), and tell the user
     to rebase and resolve manually, then re-run — this
     skill does not auto-resolve conflicts.

     **The rule is absolute on purpose, and the obvious
     carve-out does not hold.** It keys on "a conflict
     occurred" rather than "the conflict has semantic
     content", so it fires on an adjacent insertion into a
     sorted list, which genuinely has none — the tempting
     safeguard is "resolve it and let the linter verify".
     That gives no protection: **a linter verifies
     sortedness, not completeness.** A sorted list that
     silently lost one side's entry lints perfectly clean,
     and dropping a side is exactly the outcome this rule
     exists to prevent. If a carve-out is ever added, its
     verification must be completeness against **both merge
     parents** — every line added by either parent appears
     in the resolution, checked mechanically — not "the
     linter passed".

     **That check now exists, and the rule is still
     absolute.** `.claude/tools/merge_completeness.py` takes
     the lines each parent added relative to the merge base,
     whitespace-normalized, and reports per-side survival
     plus every apparent loss itemized:

     ```sh
     python3 .claude/tools/merge_completeness.py --path <file>
     ```

     Use it **while resolving by hand** — it is the tool that
     makes a manual resolution auditable, and it is what
     caught a doc comment reading "all four kinds" that merged
     cleanly when the true post-merge answer was five.

     It does **not** license auto-resolution, and the reason
     is in the same evidence. Its value is the check
     *failing* and forcing each loss to be explained one at a
     time — never a green light. In that same resolution both
     branches added a variant to one enum and an arm to one
     match: every line from both sides survived, completeness
     passed trivially, and the result was still wrong in a way
     only reading revealed. A carve-out keyed on "the tool
     passed" would have shipped it. So the tool raises the
     floor on manual resolution rather than moving this rule.

     The better lever is removing the collision class at
     the source, because it fixes every session at once
     rather than one conflict at a time: the Makefile
     declares each target's `.PHONY` beside its own rule
     instead of in one sorted block, and `.gitattributes`
     gives the cspell dictionary a `merge=union` driver so
     git takes both sides itself. The Makefile half needs no
     agent judgment at all; the dictionary half leaves one
     accepted residual, since union merge resurrects a
     deleted word (see `docs/conventions/docs-and-style.md`
     → "Spelling (cspell)").
     Reach for that before reconsidering this rule.

     **But that lever does not exist everywhere, and this is
     the case the rule is really for.** A single ordered file
     has no structural escape — yamllint's alphabetical keys
     (`cfg/**`, `infra/aws/**`) were examined and left as they
     are, because there is no layout that removes the shared
     insertion point. So when the conflict is an adjacent
     insertion into *those*, there is nothing to fix at the
     source and the answer is still a manual rebase, not an
     auto-resolve. Don't spend a round rediscovering that.

     **Enumerate the conflicted files with git, not a
     search.** The question is "which files conflict",
     and git already knows exactly:

     ```sh
     git diff --name-only --diff-filter=U
     ```

     Don't reach for a repo-wide `grep '<<<<<<<'`
     instead. It answers a different question, it is
     gitignore-blind, and one such sweep over
     `programs sdk bots frontend docs` walked
     `frontend/.next/` and returned a **79.2KB** blob
     for what is a short file list.

     **And read the hunk itself with the Read tool, not
     with a context-flagged grep.** Enumerating conflicts
     is a search; *inspecting* one is a read of a known
     region, and this step used to say the first and
     nothing about the second. In one session 14 bare
     `grep` calls cost **2.5k** inspecting a `Makefile`
     conflict and checking hook wiring — `grep -A/-B`
     around the markers prints overlapping windows and
     buys the region several times over. The
     `--diff-filter=U` call above names the file; `Read`
     it with `offset`/`limit` around the markers.

     (Those calls were once recorded as unfirmable prompt
     churn. They are not — `Bash(grep:*)` is already
     granted here. The context cost is the whole finding.)

   - If it **succeeds but integrated new commits from
     the base**, the diff now reflects that integration.
     A clean *textual* rebase can still leave a
     *semantic* conflict (the base renamed or changed
     something this branch still calls), so flag those
     for the adversarial review (step 5) and the test
     run (step 10) to catch. The rebase rewrote history,
     so the branch must be force-pushed — step 11 does
     this with `--force-with-lease`.

   **Triage what the base actually gained.** Capture the
   base this branch is *currently on* — its **merge-base** —
   **before** the fetch and rebase above, then hand both ends
   to the committed reporter:

   ```sh
   git merge-base HEAD origin/<base>
   ```

   ```sh
   python3 .claude/tools/rebase_overlap.py --from <mb> --to origin/<base>
   ```

   **The merge-base, not `git rev-parse origin/<base>`.**
   That distinction is the whole point and it is easy to get
   wrong — this step got it wrong on its own first run.
   `origin/<base>` is a **shared** ref: worktrees have one
   `.git`, so a sibling session's fetch (or this session's
   own `init-pr` fetch, hours earlier) can advance it long
   before this step executes. Reading it "before the fetch"
   therefore captures a tip the branch may never have been
   based on, and the tool then reports a **0-commit delta for
   a base that demonstrably moved** — a false all-clear, in
   the exact place a false all-clear licenses skipping the
   gates. The merge-base is what the branch is actually
   sitting on, and it is correct regardless of who fetched
   what when.

   It prints the commits the base gained, the files they
   touched, the files this branch's own commits touch
   (measured from the merge base, so the base's movement
   isn't folded in), their **overlap**, and the two
   predicates that decide whether a gate can be skipped —
   `runs_artifact_gates` and `runs_rust_suites`, computed
   over the *base delta* and delegating to
   `review_diff.py`'s lists so there is one owner.

   The overlap set is the honest input to the semantic-
   conflict flag above: an empty overlap means the base
   moved somewhere this branch never touches. The two
   predicates feed steps 9 and 11, which say what a re-run
   forced by a rebase may skip.

   **An EMPTY overlap discharges the re-run, and you assert
   the prior result instead of paying for it again.** The
   rebase itself stays unconditional — skipping it produces
   phantom hunks, which is a separate and worse failure — but
   when the freshness re-check shows the base moved *and* the
   overlap is empty, a re-run of `review_diff.py` and a full
   `make lint` **provably cannot change their answer**: they
   examine files the base delta did not touch. Measured on one
   docs-only PR: the base moved **four** times, the overlap
   was empty on all four, and the session still paid three
   `review_diff` re-runs and two full lints.

   So on an empty overlap, write one line into the review
   notes — "base moved to `<sha>`; overlap empty; lint and
   suite results from `<sha>` still stand" — and move on. Any
   **non-empty** overlap re-runs everything as before.

   This is deliberately **not** a loosening of the freshness
   gate itself, which caught a real defect in that same
   session. The gate still runs every round; what changes is
   only what an empty answer entitles you to skip.

   **Bind the assertion to a content fingerprint, so it is
   checkable rather than remembered.** "The prior result still
   stands" is a claim about *content*, and a commit SHA is the
   wrong key for it — a commit, an amend, a squash and a
   no-overlap rebase all change the SHA while changing no
   bytes, which is exactly why re-runs keep firing. Record the
   result against the tree's content instead:

   ```sh
   python3 .claude/tools/run_quiet.py -- make lint
   ```

   ```sh
   python3 .claude/tools/tree_fingerprint.py record --check lint
   ```

   and before any later stage that would re-run it:

   ```sh
   python3 .claude/tools/tree_fingerprint.py check --check lint
   ```

   It grades three ways and exits 0 only on the first:

   - **`fresh`** — recorded against this exact content. Assert
     it, note the assertion, skip the re-run.
   - **`stale`** — the content moved. Re-run.
   - **`missing`** — never recorded. Run it, and record it.

   The three-way answer is why this is not a boolean: a
   binary check would have to call "never recorded" stale and
   re-run, which is the behavior being replaced. Do the same
   for `tools-tests` and the artifact gates — any check whose
   answer is a function of the tree's content and nothing
   else.

   **What it does not cover:** anything depending on state
   outside the tree — CI, the merge queue, a live service. A
   green fingerprint says the content is unchanged, not that
   the world is.

   **If the base delta touched `programs/**`, run
   `make program` before any suite.** The rebase just pulled
   new program source into this worktree, and a scoped
   `cargo test` will happily run against the **stale** `.so`
   left from before it. The failure is maximally confusing:
   one run got 8 failures in tests it had never touched
   (`Custom(6037)` where `Custom(6048)` was expected), which
   read exactly like regressions from its own edits, and
   diagnosing them plus rebuilding plus re-running was that
   session's single biggest wall-clock detour.

   ```sh
   python3 .claude/tools/run_quiet.py -- make program
   ```

   This is the same hazard as the `test-no-teardown` ordering
   trap in step 11, arriving from the other direction — there
   a suite leaves a feature-off `.so` behind, here a rebase
   leaves a stale one. The trigger is already free: the tool
   printed the base delta's touched files just above, so this
   costs one bare command and no extra reads.

   **And if the base delta touched a lockfile or a package
   manifest, reinstall before any typecheck or lint.** The
   symmetric hazard, and it was unstated — a stale
   `node_modules` from before the base added a dependency:

   ```sh
   python3 .claude/tools/run_quiet.py -- pnpm --dir frontend install
   ```

   Fires when `base_files` includes `pnpm-lock.yaml` or a
   `frontend/package*.json`. Three sessions hit this. The
   failure does not look like a stale install: `tsc` reported
   three `TS2307: Cannot find module 'vitest'` errors, which
   read as code faults, and the actual cause was a base commit
   that had added a test runner. **"Already installed" is not a
   safe inference** — one session hit it after a *successful
   install earlier in the same session*, because the rebase
   moved the lockfile underneath it.

   The same reasoning as the `programs/**` case above: the tool
   has already printed the base delta's file list, so the
   condition is free to check and the whole cost is one bare
   command.

   This replaces hand-rolling the sequence. One session ran
   the identical `fetch` → `log` → two `diff --name-only`
   → intersect-by-eye chain **three times** as `main` moved
   15 commits (≈10k of deterministic git output), and
   re-ran the full suite each time — twice provably
   redundantly.

1. **Check the Linear task, mark it In Progress, and
   tick what's done.** The PR exists to satisfy a
   Linear issue, and autonomous runs have a habit of
   shipping a diff that covers only *some* of the
   task's checklist. Establish what the task asked
   for, record progress on the issue, and confirm the
   diff delivers all of it before reviewing anything
   else.

   - Resolve the tag. The branch and its Linear issue
     **share one `ENG-###` number** by convention
     (branch `eng-499` ↔ issue `ENG-499`; see
     `CLAUDE.md`), so take the `ENG-###` from the PR
     title scope (`type(ENG-###): …`), falling back to
     the branch name. If neither yields an `ENG-###`,
     skip this step and note in the report that no
     Linear task was checked.

   - Fetch the issue with `mcp__claude_ai_Linear__get_issue`
     (id = the uppercase tag, e.g. `"ENG-490"`). Read
     the description, and also pull
     `mcp__claude_ai_Linear__list_comments` — checklist
     items and acceptance criteria sometimes live in an
     inline (anchored) comment, not the body.

     **Unless this session already has the body.** When
     `init-pr` bootstrapped this same session it already
     read the issue, so re-fetching buys a second full-body
     echo for nothing. The echo budget is **per issue, per
     session**, not per skill (see
     `docs/conventions/linear-automation.md`) — one
     measured session paid **24.5k across three calls on
     one issue** for three state transitions, with every
     skill individually compliant and nothing budgeting the
     handoff. Work from what the session holds; fetch only
     when this skill was invoked cold.

   - Plan to move the issue to **In Progress** to reflect
     that review work is underway — invoking `review-pr`
     always moves it there, **including reclaiming it from
     In Review** if a prior `review-pr` run advanced it. In
     Review now belongs to the merge-queue handoff (the
     final steps), so a re-run should pull the issue back
     to In Progress rather than leave it sitting In Review
     while the review is actively redone. The one thing
     not to regress is a **Done** / **Canceled** issue —
     leave those as-is. **Do not issue this state change
     as its own `save_issue`**: fold it into the single
     box-tick write below (see "Minimize Linear echoes"),
     so the In-Progress move and the ticked checklist land
     in **one** call rather than two full-body echoes. (In
     Review can't be folded the same way — it's gated on
     CI-green at the merge-queue handoff, a different point
     in the flow, so it stays its own write at step 18.)

   - Extract every actionable requirement: markdown
     checkboxes (`- [ ]` open, `- [x]` already done),
     plus any acceptance-criteria or scope bullets
     phrased as requirements even if not checkbox
     syntax. Treat an already-`[x]`-checked box as a
     claim to verify, not a given — confirm the diff
     actually contains it.

   - For each requirement, decide from the diff
     (`git diff origin/<base>..HEAD`) and the branch's commits
     whether it is **addressed**, **partial**, or
     **missing**. A requirement that is out of scope
     for this PR by design (e.g. explicitly deferred,
     or split into a follow-up issue) counts as
     addressed *only if* the deferral is recorded —
     a commit, a PR-body note, or a linked follow-up
     filed via `/linear-task`. Silent omission is
     **missing**.

   - **Tick the addressed items in the same write that
     moves the issue to In Progress.** For every
     requirement the diff genuinely delivers, check its
     box (`- [ ]` → `- [x]`), then write the ticks **and**
     `state: "In Progress"` back in a **single**
     `save_issue`. Express the ticks as a **`patch`** — one
     `replace` op per box, flipping just that line — rather
     than re-sending the whole edited `description` (per
     `docs/conventions/linear-automation.md` → "Partial edits
     — the `patch` argument"). Leave **partial** and
     **missing** boxes unchecked, and don't invent boxes for
     non-checkbox requirements.

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

     Each `old_string` must match the body you fetched
     **exactly once**, which is also what keeps this from
     clobbering a box the author already ticked or an edit
     made since — a line that moved or changed fails the save
     loudly instead. Three cautions:

     - A box whose text is not unique needs more of its
       surrounding line to disambiguate.
     - A box whose text contains an `ENG-###` **won't match at
       all** (Linear stores that as a mention node). `patch`
       and `description` are mutually exclusive in one call,
       so this isn't a per-box fallback: if **any** box you
       need to tick is tag-bearing, express the **whole** call
       as a full-body `description` + `state` write instead.
       The fetched body is already in context, so that costs
       nothing extra.
     - Because the save is **atomic**, one stale anchor aborts
       the **whole** call — the In-Progress move included —
       and the error names only the *first* failing op. Don't
       move on as though the state landed. But do **not**
       recover by rebuilding the body from the snapshot you
       fetched: the abort means the live body has **diverged**
       from that snapshot, so a full-body write from it would
       silently overwrite whatever changed — turning a loud,
       safe failure into data loss. Instead re-`get_issue`
       **once** (the one licensed exception to "fetch the issue
       once"), rebuild the ops against the body it returns, and
       write once more. If that write aborts too, report the
       discrepancy and leave the issue alone rather than
       writing a third time.

     If there are **no** boxes to tick (no checklist, or
     none newly delivered), **and** the `get_issue` above
     already shows the issue **In Progress** (which `init-pr`
     set at bootstrap), there is **nothing to write** —
     skip the `save_issue` entirely. Only when the state
     actually needs to change (it's not yet In Progress, or
     it's being reclaimed from In Review) does a
     description-less `state: "In Progress"` write fire.
     Either way it is **at most one** write.

     **Minimize Linear echoes** (per `CLAUDE.md` →
     "Context economy"): each `save_issue` / `get_issue`
     **echoes the full issue body** back into context, and
     that echo is then replayed every later turn — worst on
     a large consolidated-spec body. The echo is a **fixed
     cost per call** — it comes back in full even on a
     state-only write that sends no body at all, and a
     `patch` does **not** shrink it — so the only lever on it
     is **fewer calls**. So fetch the issue **once** (the
     `get_issue` above), don't re-`get_issue` it — bar the
     single aborted-patch recovery licensed above — and collapse
     the In-Progress move and **all** the box-ticks into the
     **one** `save_issue` above — never a separate state
     write, and never one write per box. (`patch` is the lever
     on the *write payload*, not the echo: it keeps the ticks
     from re-sending the body as input. Both levers apply to
     that one call.) On a **re-run / rework**, don't re-flip
     the state unless it genuinely changed (the fetched state
     tells you), and if a state-change echo comes back not
     reflecting the change, **verify once and report the
     discrepancy** — do not retry, since each retry re-echoes
     the whole body.

     **A tick legitimately deferred past the fan-out RIDES the
     In-Review write — it never gets its own save.** Some
     boxes cannot honestly be ticked here, because the
     adversarial pass has not run and may yet invalidate the
     work. Deferring those is correct. What is not correct is
     what then happened: they fired their own `save_issue`
     **beside** the In-Review transition, two full-body echoes
     (~1.2k per review) where one write served both. The
     In-Review transition at the merge-queue handoff is a write
     you are making anyway — carry the deferred ticks in its
     `patch` array.

     **And the budget spans skills.** Everything above is
     within-skill discipline, which one measured session
     followed exactly and still paid 24.5k on one issue —
     because `init-pr` had already spent a `get_issue` plus
     the In-Progress write before this skill ever ran. Count
     the budget **per issue, per session**
     (`docs/conventions/linear-automation.md`): don't
     re-fetch a body the session already holds, and skip a
     write whose state is already correct. The bigger the
     consolidated body a `/merge-tasks` produced, the more
     each avoided call is worth.

   - Catalogue every **partial** or **missing**
     requirement as a **blocking** issue (step 7),
     quoting the checklist text and the `file:line`
     (or absence) that decides it.

1. **Run lint.** `make lint` runs the full pre-commit hook
   set, and on a failure its cspell hook alone dumps a
   ~450-line per-file cascade — pure noise that, once it's
   in context, is replayed every later turn. So run it
   **through the quiet runner** (per `CLAUDE.md` → "Context
   economy") — it captures the hook output to a temp log and
   prints only a one-line summary on success, or the failing
   tail + log path on failure:

   ```sh
   python3 .claude/tools/run_quiet.py -- make lint
   ```

   **When you go into the captured log, filter it — never
   `Read` it whole.** A whole-file read of a captured lint log
   is how a 500-line per-file cspell dump became the single
   largest result of a run (PR #207). Two ways to filter, and
   both keep the log out of the result:

   ```sh
   python3 .claude/tools/run_quiet.py inspect <log> --grep 'Failed|error\['
   ```

   The runner's own `inspect` subcommand does the filtering in
   its own process and reduces to the same
   `Bash(python3 .claude/tools/:*)` allow-rule every other tool
   here uses. With no flags it reprints the failure summary —
   the failed-hook index, the spelling index and the tail —
   which is what you want when the original summary has
   scrolled away. The **Grep tool** is equally good and is the
   right reach when it is offered; `inspect` is what makes this
   step work in a session where it is not, which is the case
   that once cost 58 shell `grep` calls (≈8.7k) reading these
   very logs.

   **Never a shell `grep`, `tail`, or `head` on the log — and
   the reason is context cost, not permissions.** A shell filter
   prints its matched region *into the tool result*, where
   `inspect` and the Grep tool return only the answer. That
   difference is the whole argument. To read a region rather
   than search for one, use the **Read tool's `offset` /
   `limit`** on the log path: it reads an out-of-workspace
   absolute path directly, and a line range is exactly what
   `offset`/`limit` express.

   **Do not argue this from "unfirmable prompt churn" — that
   claim is false and has been checked.**
   `firm_core.NO_BARE_WILDCARD` is a deny-list of *hazardous
   programs*; `grep`, `tail` and `head` were never members. So
   `is_bareverb_wildcard("Bash(grep:*)")` is `False`, the fast
   firm would happily write that rule, and `Bash(grep:*)` is in
   fact **already granted** on this machine and prompts for
   nothing. Both halves of the permission floor agree; there is
   no permission-churn objection to lean on.

   (This paragraph twice carried the opposite claim. The first
   version argued it for `grep` and then, two sentences later,
   said "or read only its tail" — inviting the identical shape
   it had just ruled out, which a perms sweep then harvested.
   The second version fixed the tail half but kept the false
   rationale. Stating it once, correctly, is the fix.)

   **Run the formatting-class hooks scoped in ONE invocation
   after edits — don't discover them serially.** Each
   `pre-commit run --files …` surfaces one violation class at
   a time, so a fix→lint→fix→lint loop walks cspell, then
   line-length, then cspell again: one run spent **6
   invocations** on what was a single class of problem. Run
   the formatting hooks together over the changed paths, take
   the whole list, and fix it in one pass.

   **A markdown edit costs two runs by construction.** The
   `markdownlint-fix` and `mdformat` hooks *modify files and
   then fail*, so any `.md` change needs a second full run to
   go green. That is the design, not a failure — one session
   read it as one and spent 12 invocations (≈4.4k) on a spec
   edit. Either run the two markdown hooks scoped to the
   changed `.md` files first and let the full run come back
   clean, or simply expect the second run.

   **Answer "which hooks cover this file type?" with a grep,
   not a read.** `cfg/pre-commit-lint.yml` is ~190 lines and
   was once read whole (≈1.7k) to learn which hooks cover
   `.sh` and `.github/**`. Grep it for `id:` / `files:` /
   `types_or:` instead.

   **That rule is about a CLASS of file, not that one
   filename — a CI workflow is a config index too.** It kept
   being read as advice about `pre-commit-lint.yml`
   specifically, so the workflow files stayed exempt: one
   session read the lint workflow whole (~770 tokens, 40% of
   its entire Read cost) to learn **one line**, which then sent
   it to grep the hook config anyway. Answer
   "how does CI run X?" by grepping the workflow **and** the
   hook config for the invocation — never by reading either
   whole. Neither is prose; both are indexes.

   **After a fix, the scoped re-run is the default — a full
   `make lint` is the exception.** This is already prescribed
   above and is still the most-missed instruction in the step:
   one run paid ten full sweeps across ~5 fix-and-retry cycles
   (≈5.3k) where a scoped per-hook re-run was the stated rule,
   and a later one paid **thirteen (≈5.8k)** while editing this
   very rule.

   **The scoped form is one bare command — use it.** Restating
   the rule is not the lever; thirteen sweeps happened *while
   the rule was being read*. What made `make lint` the reflex
   is that it needs no arguments, while the scoped form needed
   a changed-file list assembled by hand. It doesn't any more:

   ```sh
   python3 .claude/tools/run_quiet.py -- \
     python3 .claude/tools/lint_paths.py --changed
   ```

   That lints exactly this branch's files — everything
   differing from the merge-base with `origin/main`, plus
   untracked-not-ignored paths. Append `-- <hook-id>` to
   narrow to the hook that failed. There is no longer a
   convenience argument for the full sweep, so treat one
   outside the two checkpoints as a mistake.

   The by-hand form still works and is what to reach for when
   the paths you want are narrower than the branch:

   ```sh
   python3 .claude/tools/run_quiet.py -- pre-commit run <hook> --files <paths>
   ```

   Note the cost here is **context in failure tails**, not
   verbosity — every one of those sweeps was already wrapped
   in `run_quiet`, so wrapping harder buys nothing. The only
   lever is *fewer full sweeps*.

   **Scoping one HOOK is not enough when the violations span
   hooks — scope the whole SET instead.** This is the failure
   the per-hook advice above leaves open, and it is the
   expensive one: 15 full `make lint` sweeps, each surfacing a
   single violation class, three cspell words discovered on
   three separate sweeps. The working form runs **every** hook
   over **only** the changed files — a hook id is what you
   append to narrow further, not what makes it scoped:

   ```sh
   python3 .claude/tools/run_quiet.py -- \
     pre-commit run --config cfg/pre-commit-lint.yml --files <paths>
   ```

   `lint_paths.py --changed` above is this same shape with the
   file list resolved for you, so prefer it; reach for the
   explicit form only when the paths are narrower than the
   branch.

   **The dictionary sorter fails-then-fixes by design.** Like
   the markdown hooks, `cfg/dictionary.txt`'s `--unique` sorter
   rewrites the file and reports failure, so adding a word
   costs a second run. Expect it; it is not a defect, and a
   rebase that leaves the merge-union dictionary unsorted
   produces exactly the same benign one-round fail-then-fix.

   **Run cspell over the changed files FIRST when the change
   authors prose.** New technical terms are known to the author
   in advance, so discovering them one per round is pure
   waste — measured at three serial rounds for three words, and
   at **26 `make lint` runs (~12.8k of failure tails)** on a
   prose-heavy change where several cycles each bought a single
   unknown word. Run the spelling hook alone over the changed
   paths as a pre-flight, take the whole list of unknown words
   in one go, and only then run the full set:

   ```sh
   python3 .claude/tools/run_quiet.py -- \
     pre-commit run cspell --config cfg/pre-commit-lint.yml --files <paths>
   ```

   Then place each word by the rule in
   `docs/conventions/docs-and-style.md`: a word used in **two
   or more** files goes in `cfg/dictionary.txt`; a word used in
   exactly one gets a top-of-file `cspell:word` escape in that
   file, one word per directive. Worked example: a term you
   introduce in a tool *and* its test is two files, so it goes
   in the dictionary; a term appearing only in one module's
   docstring gets the inline escape. And note the dictionary is
   **US spelling**, so a British variant — an `-our` ending, an
   `-ise` verb — fails by construction. Write the US form
   rather than adding the variant to the dictionary; three such
   words tripped this hook while this very rule was being
   written.

   **The markdown two-step applies to the SCOPED invocation,
   not to the full sweep.** "Two runs by construction" is true
   and is stated above; what it must not license is two *full*
   sweeps per fix. One markdown-only PR ran `make lint` **eight
   times** on exactly that reading. Take the full sweep once at
   the start and once at the end, and let the fix-verify loop
   in between be the scoped run.

   **Don't hand-verify a formatting fact the gate already
   owns — and never with a byte-length tool.** Five hand-rolled
   `awk 'length($0)>80'` width checks in one session were both
   redundant with the lint run happening anyway *and wrong*:
   `awk` counts bytes, an em-dash is three, so compliant lines
   in this repo's em-dash-heavy prose flagged LONG and the
   false signal then had to be refuted. The MD013 hook reports
   every real violation with its line number.

   **A fast suite runs whole, through the wrapper — not per
   module.** The same discipline on the other axis. For an
   edit under `.claude/tools/`, the post-edit check is
   `python3 .claude/tools/run_quiet.py -- make tools-tests`,
   never a `-p test_X.py` discover run: measured, the narrow
   form cost **32 calls / ≈7.1k** against **15 calls / 516
   tokens** for the whole suite, and it missed two sibling
   tests the edits had just broken. See
   `docs/conventions/context-economy.md` → "When a suite is
   fast enough to run whole".

   Take the full sweep once at the start, and once at the end
   to confirm green. Two corollaries:

   - **Assert, don't re-run, on an unchanged tree.** If no
     file changed since the last lint, the result cannot have
     changed either — one run re-ran `make lint` on a
     byte-identical tree. Say it's unchanged and move on.
   - **`tsc --noEmit` is the exception that proves it.** It is
     whole-project by nature, so scoping the *file list* buys
     nothing — which means the lever there is **frequency**,
     not scope. One session fired `tsc --noEmit` four times
     and `biome check` five, several after single-file edits
     that could not change a type. Run it once after the
     TypeScript edits are done, not after each one.

   If lint fails, first separate **environmental**
   failures and **autofixes** from **real violations** —
   they are three different problems:

   - **A formatter's "files were modified by this hook" is
     an autofix, not a finding.** `mdformat`,
     `markdownlint-fix`, `ruff format`, and `biome` rewrite
     the file and *then* report failure; the correct
     response is to stage the reformat and **re-run once**,
     treating only a **second** failure as a real
     violation. Don't read it as a violation to diagnose,
     and don't re-run more than once — if the same hook
     fails twice, the second failure is the real one (a
     line the formatter can't fix, e.g. an MD013 overlong
     line that `mdformat` created by collapsing a wrap
     inside an inline code span). In one run both the full
     `make lint` and the scoped re-run each failed once
     purely because `biome` reformatted, and those
     immediate identical re-runs were a meaningful share of
     that session's 3.6k / 10 scoped-lint and 900 / 5
     `make lint` totals.

     **To SEE what a formatter changed, read the slice
     files — never `git diff` the file.** A formatter that
     rewrote a file has already had its output captured in
     `review-diff-docs.txt` / `-source.txt`. The general
     "don't re-derive a diff" rule is stated twice already —
     in `commit-changes` and in step 5's prose — but both sit
     where the diff is *produced*, thousands of lines before
     the point where you actually want to look at a reflow,
     and by then the operative question feels like "what did
     `mdformat` change?", which does not read as re-deriving
     anything. Measured: `git diff` appears in one session's
     hardening table at ≈1.0k over 2 calls, flagged
     `cost: context` and the only `context`-labelled shape in
     it — both calls made **after** the slices existed. One
     was `git diff --stat` to size the change, which the
     tool's verdict already reported as a per-file `changes`
     count; the other was `git diff docs/market-making.md` to
     see a reflow that `review-diff-docs.txt` already
     contained in 67 lines. Small in absolute terms, and worth
     naming because it is a documented anti-pattern being
     reproduced by the skill that owns the tool.

   - A hook that fails because its binary isn't
     installed is **not** a diff problem. The
     frontend hooks — `biome`, `tsc` — report
     "Command … not found" whenever this worktree has
     no frontend `node_modules` (each worktree is a
     fresh checkout, so deps aren't installed until you
     ask). Install them once and re-run, so the hooks
     actually evaluate the diff:

     ```sh
     pnpm --dir frontend install
     ```

     If the deps still can't be installed, treat those
     hooks as **unverifiable locally** — exactly like an
     absent Solana toolchain (steps 9–10), not as a
     blocking failure. When the diff touches none of the
     files such a stalled hook covers (e.g. a docs-only
     change vs. `biome` / `tsc`, which only target
     JS / TS / CSS), note in the report that they'll
     pass in CI and move on. **Never gate the PR on a
     hook that couldn't run.**

   - For genuine violations, parse the failing tail (and
     the log by slice) and fix
     every issue that can be fixed mechanically
     (formatting, import order, trailing whitespace,
     spelling, etc.).

   - Stage the fixes by explicit path and commit as a
     single signed commit:

     ```sh
     git add <fixed files...>
     git commit -S -m "Fix lint violations"
     ```

   - **Re-run one hook, scoped to the diff's changed files** —
     not the whole `make lint` / `--all-files` cascade. This
     applies to **both** the failure case (confirming a fix)
     **and** the ordinary post-edit confirmation later in the
     review: one run made six full `--all-files` sweeps after
     edits confined to a single crate, because only the
     failure case read as in-scope here. The full run
     re-checks every file in the repo (the cspell hook's
     ~450-line cascade is the worst of it); your edit touched
     the diff's files, so confirm against just those:

     ```sh
     python3 .claude/tools/run_quiet.py -- \
       pre-commit run <hook-id> --config cfg/pre-commit-lint.yml \
       --files <changed files...>
     ```

     **Wrap the scoped run too**, exactly as the full
     `make lint` above is wrapped. A scoped `pre-commit run`
     still prints **all 24 hook lines** — about 20 of them
     "Skipped" / "no files to check" — roughly 675 tokens
     for a one-bit answer; four such runs in one review is
     ~2k bought for nothing. The wrapper loses no signal:
     a failure still prints the failing tail and the log
     path.

     **`--config` is mandatory, not decorative.** This repo
     keeps its hook config at `cfg/pre-commit-lint.yml`, not
     the default `.pre-commit-config.yaml`, so omitting the
     flag fails outright with
     `InvalidConfigError: .pre-commit-config.yaml is not a file`.
     One run hit exactly that, read it as "the scoped path
     doesn't work here", and fell back to **seven** full
     `make lint` runs.

     Take `<changed files...>` from
     `git diff --name-only origin/<base>..HEAD`. Only fall back to a
     full `python3 .claude/tools/run_quiet.py -- make lint`
     when a hook is repo-global (it has no per-file scope) or
     when you've changed enough that a scoped re-run wouldn't
     be representative.

   - **Batch verification to checkpoints, and match the
     check to the edit.** Verify once per logical
     checkpoint — not once per edit. Two rules fall out,
     both measured on visual PRs where they were violated
     while the docs already said otherwise:

     - **A copy-only or comment-only change needs lint,
       not a build or a typecheck.** A string literal in a
       `.tsx`, or a reworded comment, cannot change a type
       or an artifact — so the optimizing build proves
       nothing the running dev server didn't already show.
       One run fired `make decks-build` **×11** as an
       inner-loop check (`decks/README.md` explicitly
       calls it a *pre-commit* check), most of them after
       copy-only edits. Another ran `pnpm -C decks check`
       **×19** across ~15 rounds, including after
       comment-only changes.
     - **Per-call cost is small; the repetition is the
       cost.** After grep, this is the top repeated shape
       in both of those sessions. Batching it to
       checkpoints costs nothing in signal, because the
       checkpoint is where a real regression would be
       caught anyway.

   - If real violations still fail after the fix
     attempt, catalogue the remaining failures as
     **blocking** issues (step 7) and do **not** mark
     the PR ready.

1. **Adversarial diff review.** Collect the diff and log —
   but write the **diff to a single file** rather than into
   context, so the fan-out below hands each agent a path
   instead of inlining N resident copies (per `CLAUDE.md` →
   "Context economy"; the file-handoff pattern). The tool
   below streams the diff straight to that file — no shell
   redirect, and the bulky diff never enters the main
   transcript.

   **Diff against `origin/<base>`, not a local branch, and
   exclude the generated families.** `review_diff.py` below
   does both — the reasoning, so the tool's behavior isn't a
   black box:

   - **Ref: `origin/<base>..HEAD`.** In a worktree, the local
     base branch is stale behind origin by already-merged
     PRs, so `git diff <base>..HEAD` pulls unrelated
     merged-PR files into `review-diff.txt` (the stale-diff
     hazard, reached via the wrong ref). The tool fetches
     `origin/<base>` itself and diffs against it.
   - **Exclude generated files with pathspec `:(exclude)`
     globs.** A review lens reads source, not regenerated
     output — yet a lockfile or a regenerated SDK/IDL tree
     can be the *bulk* of a diff (a 6001-line diff that was
     ~3607 lines of `pnpm-lock.yaml`, read by all 7 agents),
     replayed per agent and per turn. The tool's
     `DIFF_EXCLUDES` holds the known generated families so
     each lens reads source-only: lock files
     (`pnpm-lock.yaml`, `Cargo.lock`), the generated SDK
     clients (`sdk/ts/src/generated`, `sdk/rs/src/generated`),
     and the generated IDL (`sdk/idl/dropset.json`). (These
     are regenerated and gate-checked in step 9 — reviewing
     their diff by eye adds nothing.) Note the verdict's
     `files` list is **unfiltered**, so an excluded family
     still shows up there as a changed path — which is what
     lets step 9 see that it needs to run.

   **Write it to the session scratchpad, not `/tmp`.** The
   environment designates a per-session scratchpad directory
   (the harness prints its path at session start) that **is
   shared with the sub-agents** you spawn. `/tmp` is **not**
   safe here: it's shared across sessions, so a sibling
   session's stale `review-diff.txt` can sit at the same path
   and the fan-out then reviews the **wrong diff** — a real
   bug that has cost an entire 6-agent pass. Write to
   `<scratchpad>/review-diff.txt` (substitute the actual
   scratchpad path), and **verify the file is this branch's
   diff before fanning out** — which is what the gate below
   does, so a zero-length or stale file is caught now, not
   after N agents have read it.

   This whole preamble — the fetch, the two `git log` range
   checks, the excluded `git diff` into the file, the per-file
   stat, and the line count — is fixed string/path logic
   resolving to a mechanical verdict, so it lives in the
   skill-tool `.claude/tools/review_diff.py` (per `CLAUDE.md`
   → "Skill tooling"). **One** call replaces the six commands
   and returns **one** compact JSON object instead of six tool
   results:

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

   **Pass `--split`, and hand each lens its slice, not the
   whole diff.** Prompt tightening has visibly *saturated*:
   one fan-out cost ≈2.68M across five lenses on a ~1.5k-line
   diff even though every brief already inlined excerpts,
   named its comparison files, stated a turn cap, and handed
   the diff by path. The residual is **structural** — all five
   agents `Read` the *same whole* file, and that one carried
   212 lines of `docs/architecture.md` plus large comment
   reflows only one lens needed. So route by slice:

   - **source** — correctness, security, style, completeness.
   - **tests** — completeness (and correctness, when the diff
     changes behavior tests pin).
   - **docs** — the doc-freshness lens.

   **Each brief must NAME the slice it is handed**, and a
   lens scoped to one tree gets that tree's `--only` diff
   rather than the branch diff: a tools-correctness lens
   receives `--only '.claude/tools/**'`, a
   conventions-freshness lens receives `docs/conventions/**`
   plus `CLAUDE.md`. The step already computes the split and
   the failure is simply not routing it. Measured: seven
   sub-agents summing **≈2.28M** of per-turn input —
   cross-check 504.8k/6, test adequacy 502.8k/6, conventions
   freshness 357.7k/5, correctness-modified 304.6k/4, skill
   prose 289.4k/4, correctness-new 207.8k/3, security 110.0k/2
   — with four of the seven at 1.7–2.8× the top of the
   exemplar band. The diagnosis is **scope, not depth**: a
   lens's context is re-sent every turn, so input scales with
   (material handed in) × (turns taken), and the lenses that
   ran longest were the ones handed the broadest material.
   Some of that diff was irreducibly large (60 files,
   +8656/−532), but the two correctness lenses are precisely
   what `--only` exists to narrow and neither got a narrowed
   slice. Print each lens's handed-in byte count alongside the
   brief, so an over-broad one is visible at fan-out time
   rather than in the next session's metrics.

   **The docs lens gets a claim→excerpt table, not the source
   slices.** This step routes the docs slice to it and says
   nothing about what it needs to check a doc claim *against
   the code*, so the natural move is to hand it the source too
   — which is the whole diff by another name, defeating the
   split. Measured: a doc-accuracy lens handed the docs slice
   **plus both full source sub-slices** as "ground truth" came
   in at **504.4k over 6 turns**, the second most expensive
   agent of its session and 2.5× the cheapest substantive lens
   on the same diff (249.6k / 4 turns).

   A doc-accuracy lens adjudicates a bounded list of
   **claims**, not a codebase — and the main loop wrote those
   docs, so it already knows which code each claim rests on.
   Every finding that lens returned turned on a handful of
   lines already in the main loop's context; none needed
   either source slice in full. So hand it the docs slice plus
   a short table of *the assertion* and *the five to fifteen
   lines that settle it*, cold-reading only where no excerpt
   covers. Add the docs lens to the list whose comparison
   material must be **inlined rather than named** — the same
   excerpt-not-filenames discipline this step already
   prescribes for the style lens, measurably its cheapest,
   and simply never stated for docs.

   **When a slice is still huge, split the slice — do not
   tighten the prompt again.** This is the sharpened form of
   the point above, and it names the lever precisely. One
   measured fan-out ran at ~95% of total session cost while
   **fully compliant**: every lens got inlined excerpts, a
   named comparison-file list, a hard-stop turn cap and its own
   split slice, and the turn counts *held*. Input still ran 1.3
   to 5× the exemplars — because the `source` slice itself was
   **3,297 lines**. No prompt discipline reaches that; the
   input floor is the slice.

   So treat slice size as its own dial. When a slice runs past
   roughly a thousand lines, subdivide it — by crate, by
   directory, or by the natural seam the diff already has — and
   give each sub-slice its own lens instance, rather than
   spending another round rewording the brief. Prompt
   tightening has saturated; slice granularity has not.

   **The subdivision decision is PER LENS, not per fan-out.**
   Stated only as "subdivide an oversized slice", it reads as
   one judgement about the diff, so a session that finds the
   diff to be a single coherent seam declines to subdivide for
   *every* lens at once — which is the wrong shape, because the
   lenses do not all ask cross-cutting questions.

   The test: **does this lens's question span the seam between
   crates, or sit inside one?** Correctness and security
   usually need the cross-crate chain and are right to take the
   whole slice. Style, naming and a new-public-surface question
   usually do not.

   Measured (session a252a9d3, PR #391): a 1,888-line diff
   across five crates produced a 1,070-line source slice handed
   to **every** lens. Every lens came in at or under cap and the
   brief discipline was fully applied — 45 established facts
   including negatives, verbatim comparison excerpts, a
   three-sub-question ceiling, caps in both turns and tool
   calls. Input still ran 1.4–3.3× the efficient exemplars.
   The style lens's three questions were entirely about one
   file's new public surface and it was handed all five crates;
   `--only 'sdk/rs/**'` would have cut it to roughly a third.
   So the answer is never "always subdivide" — it is "decide per
   lens", and the per-lens byte count printed above is what
   makes a wrong decision visible before the lens runs.

   **Never tell a lens its slice holds a whole function.** A
   diff slice is *hunks*, so an unchanged remainder of a
   function it touches is simply not there. A brief that
   promises otherwise sends the lens looking for a body it
   cannot see, and the honest outcomes are both bad: it either
   reads the file itself — buying the whole file the slicing was
   meant to avoid — or reasons about the missing half from its
   name. Say what the slice is (changed hunks, with N lines of
   context) and inline any surrounding region the question
   genuinely needs, which the excerpt rule below already
   covers.

   **Use `--only` to cut it; do not hand-roll the split.**
   `--split` cuts by *category* (source / tests / docs) and
   cannot subdivide within one, which is why this paragraph
   used to prescribe a subdivision the tool could not perform:
   3,156- and 4,148-line source slices went to every lens
   whole (the costliest single agent on one such run took
   **634.7k** of input), and a second session hand-rolled the
   split as three separate `git diff` calls with literal path
   lists. The flag takes repeatable path globs:

   ```sh
   python3 .claude/tools/review_diff.py --base main \
     --out <scratchpad>/review-diff-programs.txt \
     --only 'programs/**' --split
   ```

   Run it once per sub-slice, and hand each lens its own
   `--out` path.

   **The slices are namespaced off that `--out` stem** —
   `--out review-diff-tools.txt --split` writes
   `review-diff-tools-source.txt` and siblings, while the plain
   `--out review-diff.txt` still writes the documented
   `review-diff-source.txt` set. That is what makes "its own
   `--out` path" true rather than aspirational: the slice names
   were once **fixed**, so a scoped `--only` run silently
   overwrote the unscoped run's slices — and because a
   tools-only run has no docs hunks, it left
   `review-diff-docs.txt` **empty**. The lens handed that slice
   then correctly reports nothing to review, which is a silent
   and total failure of that lens, in the same class as the
   missing-preamble case above. Caught in one review by a
   `wc -l` before spawning; do not rely on catching it.

   **Rust inline tests are already extracted for you.** Rust
   keeps unit tests inside the source file, so the category
   split used to produce a **zero-line** tests slice on a diff
   full of test changes — measured on a 4,351-line Rust diff,
   after which the test-adequacy lens read three source slices
   and became that run's costliest agent (473.6k).
   `--split` now routes `#[cfg(test)]` hunks into the tests
   slice by parsing the post-image, so the tests slice is
   trustworthy on Rust; if it comes back empty on a Rust diff
   now, that means something.

   **Once this has run, the slice files ARE the diff — for the
   main loop too.** Having written them, do not turn round and
   re-derive the same content with a bare `git diff` to read it
   yourself: that buys the same bytes twice, and it was the
   **second-largest single result** of two separate sessions
   (≈3.4k re-running a `git diff` over a range already sliced,
   ≈3.1k doing it for a self-review). Read the slice you want,
   or `diff_path` for the whole thing. This is the standing
   "never re-fetch what's already in context" rule applied to a
   payload the session itself produced, which is exactly why it
   slips — the rule reads as being about *tool results*, and a
   file you wrote does not feel like one.

   **But a per-FILE question wants a per-file scope.** The rule
   above is what makes the slice the reflex, and reaching for it
   to settle a claim about one file inside it buys every other
   file too. Measured (session 08c0ae6b, PR #383): a
   `read_result.py --grep '^[+-][^+-]'` over a 450-line source
   slice returned ≈4.7k — rank 3 of all results that session —
   printing 306 of 447 lines, when the question concerned one
   file with 31 changed lines. The other ~275 lines were files
   the main loop had itself just authored and already held. A
   path-limited `git diff` in the same turn cost a fraction and
   is what actually answered it.

   So scope it: `--only '<path>'` on `review_diff.py`, or a
   path-limited `git diff`. Grepping a whole slice for `^[+-]`
   re-buys every file in it, and this is the one case where
   "the slice IS the diff" points the wrong way — the slice is
   still the right *source*, just not the right *scope*.

   A lens that genuinely needs two categories gets two paths;
   the full `diff_path` stays available for the cross-check,
   which is the one pass that should see everything. Two
   caveats to state in the brief so a lens isn't misled: the
   split is **by file**, so Rust's inline `#[cfg(test)]` unit
   tests ride in the **source** slice, and an empty slice is
   still written (an absent file would be ambiguous between
   "nothing here" and "the split didn't run").

   **`ready` is the gate: do not fan out unless it is `true`.**
   It is defined as `not blockers`, so the two can never
   disagree, and the tool exits non-zero when it is false — the
   check can't be skipped by only reading the status.
   `blockers` names the reason, and there are four: a stale
   base, a **failed fetch** (freshness unverified rather than
   verified-fresh — pass `--no-fetch` to accept the local ref
   deliberately), nothing changed at all, or something changed
   but every path is an excluded generated family (no source to
   review, though the step 9/10 gates still apply — reported as
   its own distinct reason). `commits` is small, so pass it
   inline to the lenses; the diff itself lives only in
   `<scratchpad>/review-diff.txt`.

   The tool **owns** three path lists that used to sit as
   prose here and were re-typed by hand each run — the
   generated-family diff excludes, the mirror of `test.yml`'s
   `code` filter, and the generation inputs. Keeping them in
   one place is why `runs_rust_suites` and
   `runs_artifact_gates` can be read off the verdict in steps
   9 and 10 rather than re-derived. When one of those lists
   changes upstream, change it in `review_diff.py`.

   `--out` is **required**, so the path can't be omitted — but
   it is *not* validated against the scratchpad root, so
   substitute the real path rather than guessing: a guessed
   path gets created and written, not rejected. What the tool
   does remove is the **stale**-file hazard, since it rewrites
   `--out` on every run (owner-only, `0o600` — a review diff
   can carry a fixture key or a config token, so it gets the
   same treatment as a `run_quiet` log).

   **Why `base_fresh` is a field and not a line count.** The
   base can advance *past* the step-2 rebase while the review
   is in flight: worktrees share one `.git`, so a sibling
   session's fetch (or a merge landing on `main`) moves
   `origin/<base>` under this session with no fetch of its own.
   The review diff then reads as if this branch **deleted**
   whatever the base just added — a phantom `-` hunk. That is
   not hypothetical: on one run a newly-landed test showed up
   as a phantom deletion and **both** the correctness and
   completeness lenses independently flagged it as a blocking
   coverage regression; the whole fan-out was spent
   adjudicating a false positive, then re-run from scratch on a
   corrected base along with the full test suite. A line count
   **passed** throughout — it cannot distinguish a phantom
   deletion from real content, so it never surfaces this, which
   is why `diff_lines` alone is not the gate.

   So when `base_fresh` is `false` (equivalently, `base_ahead`
   is non-empty), the base has commits this branch doesn't:
   re-fetch, re-rebase onto `origin/<base>`, re-run
   `review_diff.py`, and only then fan out. **Nothing is
   spawned until `ready` is `true`.**

   Read the `files` list for **sizing and tiering** (which
   crates and surfaces the diff spans, feeding the fan-out
   scaling below) — not as a second freshness check: once
   `base_fresh` holds, a foreign path can't be base drift,
   because there is no drift to leak.

   **The spawn is already authorized — do not ask again here.**
   The entry gate (step 0) carried one `AskUserQuestion` that
   approved both "run `/review-pr` now" and the **tier**, and
   that explicit tier choice **is** the spawn approval. Spawn
   the approved tier and move on. A session operating under a
   standing "don't spawn agents unless asked" instruction has
   already satisfied it: the user picked a tier by name.

   Adding a second question here is what the one-gate rule
   exists to remove. One review ran under exactly that standing
   default and paid a round trip to ask, the user authorized it,
   the fan-out then found **six blocking defects** — and the
   question was ceremony, since the user had just typed the
   skill name. Never add a mid-flight fan-out prompt on the
   normal path.

   **The hard gate survives for genuine tooling
   unavailability.** If the harness does not offer the `Agent`
   tool at all, that is not a permission question and no
   approval can fix it: do not spawn, do not silently skip, and
   **do not substitute an inline pass**. Stop, say the tool is
   absent, and let the user decide — the same stop condition as
   before, now scoped to the case that is actually about
   availability rather than authorization.

   **Blocking is the intended behavior, not a degraded mode.**
   A session that cannot spawn **cannot complete a `review-pr`
   pass**. That is stated plainly here so it is implemented
   deliberately rather than discovered: an independent
   adversarial pass is the *entire point* of the step, so
   declining to run it is a stop condition.

   **Why the inline path was removed, since it reads as the
   obvious accommodation.** A lens running in the same context
   as the author cannot disagree with the author's blind
   spots — it shares them. That makes an inline pass
   structurally a **self-review**, whatever it happens to
   find. The objection is to the **assurance property**, so it
   does not soften with a smaller diff: an earlier proposal to
   keep the inline path behind a diff-size or rebase-risk
   ceiling is **superseded** for exactly that reason. A small
   diff self-reviewed is still self-reviewed.

   Two further notes, recorded so the old reasoning is not
   reconstructed:

   - **Cost is not the argument, and the naive cost reading is
     backwards.** An inline pass is cheaper in *total* tokens
     — no fan-out appears in the rollup at all — but every
     byte it spends lands in the **main loop**, where it is
     replayed on **every subsequent turn**. A fan-out spends
     more, in throwaway contexts that evaporate on completion,
     and the main loop only ever sees the findings. Total
     spend and main-loop context pressure are different costs;
     the inline path is cheaper in one and more exposed in the
     other. That exposure compounds under the post-review
     rebase churn this skill already documents as recurring.
   - **One such run did find two real defects**, and that is
     not evidence the path was sound — an unchecked review can
     be right. The failure it invites is the one nobody
     notices. Observed instance: a PR ran its fan-out inline
     under a no-agents instruction, correctly declared the
     reduced assurance in its summary, and still produced a
     review no fresh context ever checked.

   **Brief every sub-agent on the shell rules.** The standing
   sub-agent brief from `docs/conventions/sub-agent-brief.md`
   reaches **each** Agent prompt — the review agents here
   *and* the cross-check agent in step 6 — via the preamble
   file built below, so there is no need to `Read` that
   convention doc here. That brief is the canonical wording
   (read-only framing, Read/Grep/Glob over shell, one bare
   command per Bash call, each reducible to an allow-rule);
   it exists so sub-agents — which inherit neither that
   brief nor `CLAUDE.md` — don't reach for the
   `find` / `sed … | grep` / `cat`
   compounds that re-prompt on every run.

   **Write the invariant preamble to the scratchpad once,
   and hand each lens its path.** Every lens brief has two
   halves: a **standing** half that is byte-identical across
   all of them — the sub-agent brief above, the negative
   scope, the standing suppressions, the lint carve-out, the
   pre-emit gate — and a **per-lens** half (its dimension,
   its excerpts, its cap). The standing half ran ~1.5–2k
   tokens per lens in one measured review, and a sub-agent's
   prompt is re-sent on **every one of its turns**, so
   across six lenses at 6–14 turns each that is a meaningful
   slice of a 5.4M fan-out — paid to say the same thing
   forty-odd times.

   **Don't compose it by hand — emit it.** Writing it
   yourself means first `Read`ing
   `docs/conventions/sub-agent-brief.md` whole (≈1.7k,
   measured on two separate runs) purely to copy verbatim,
   unchanging boilerplate. That is deterministic string
   assembly over a file with one owner, so a tool does it —
   reading the brief in its **own** process, which is the
   point (per `CLAUDE.md` → "Skill tooling"). One bare
   command, and the skill reads nothing:

   ```sh
   python3 .claude/tools/lens_preamble.py \
     --out <scratchpad>/lens-preamble.md \
     --append .claude/skills/review-pr/lens-standing.md \
     --facts-file <scratchpad>/facts.md
   ```

   **Write the facts to a file; do not copy an example.** The `--fact` flags are
   equivalent, but a worked example in a copy-paste command is a trap here: the
   composed section instructs every lens to treat its contents as **binding and
   not to re-derive them**, so a run that pastes someone else's example injects
   *false* established facts into the entire fan-out. Compose
   `<scratchpad>/facts.md` from what **this** run actually verified — one claim
   per line, negatives included — and pass the path. Pass `--no-facts` only to
   state on the record that nothing was verified.

   **A fact stating a formula states the WIDTH it is computed
   in.** A formula reads as complete, so it never trips the
   what-am-I-omitting check — and an incomplete one sends a
   lens back to the source to answer the question the fact was
   supposed to settle. Measured: a facts block gave the
   flush-level factor formula without its width, so the
   claim-accuracy lens re-read the matching-math module cold to
   check whether a PPM multiply plus a max offset could trap.
   It computes in `u128`; one word would have closed it, and
   the lens spent a call from its turn cap on a file the facts
   already covered.

   So any fact carrying a formula, an expression, or a
   conversion also states:

   - the **type or width** it is performed in (`u64`, `u128`,
     `i32`, an `f64` intermediate);
   - the **rounding direction**, where one applies;
   - whether it **saturates or wraps** on overflow.

   **Quote an expression WHOLE, guard included.** An
   established fact that drops a null guard, a default, or a
   short-circuit hands the lens a materially different
   expression from the one in the source — and the fact block
   says to treat its contents as binding and not re-derive
   them, so the lens has no license to notice. A quoted
   expression is either the whole expression or it is a
   paraphrase that should be labelled as one.

   **Hoist the WRITER-side constants that define a key, not
   only the reader-side schema.** A facts block that gives a
   lens the schema a key is parsed by, without the constants
   the key is composed from, cannot support the question the
   lens was spawned to answer: whether a diff's key is
   *well-formed*. The lens can see how the value is consumed
   and not what a correct one looks like, so it either goes to
   the source for the writer or adjudicates the half it has.
   When a fact concerns a key, an id, or a wire format, hoist
   both ends.

   It composes two committed halves, each with a single
   owner: the canonical shell rules from the convention doc,
   and this skill's own standing scaffolding from
   `lens-standing.md` beside this file — the negative scope,
   the budget, the pre-emit gate, the standing suppressions.
   **That template is the agent-facing wording**; the prose in
   this step is the rationale for it. When you change one,
   check the other still describes it.

   **`--fact` is required, and it is the highest-leverage
   thing in this step.** Every brief must carry the facts
   already verified before the run — including the
   **negatives**: "there is no test harness here", "this
   export has zero call sites", "there is no central clock
   provider". The tool **refuses** a run with no facts and no
   explicit `--no-facts`, because omitting the section silently
   is what used to happen.

   This is measured twice, not a hunch. One run brought all
   five lenses in at or under their turn caps — 2, 2, 3, 4, 4
   against 5/5/4/5/8, zero overruns — and credited not the
   hard-stop wording (already standard) but an ad-hoc block of
   exactly this shape carrying three pre-run grep results, the
   lint gate's coverage, and explicit negatives; **two lenses
   said outright they needed no further reads**. Another
   reproduced it: a security lens at 90.4k over 2 turns with
   **zero cold reads**, cheaper than every exemplar named
   below, producing that review's sharpest findings.

   The excerpt rule covers what you have already read; this
   covers **what you already know isn't there** — and a lens
   cannot distinguish "nobody told me" from "I had better go
   check". Gather the facts as you prepare the review (the
   pre-run greps you were going to run anyway) and pass them.

   **Emit the preamble at the spawn gate, and treat its
   absence as a blocker.** Run `lens_preamble.py` immediately
   before spawning — in the same gate that checks
   `review_diff.py`'s `ready` — and confirm the file exists,
   rather than emitting it once earlier in the run and trusting
   it to still be there.

   This is not defensive padding; it is a measured silent
   degradation of the one step whose entire value is
   independent assurance. In one review **all four lenses
   independently reported `lens-preamble.md` missing** and said
   so in their output, and nothing in the skill noticed. The
   cause: the preamble had been emitted before a session
   restart and its scratchpad was cleaned, while
   `review_diff.py`'s slices survived because they happened to
   be regenerated after a mid-review rebase. So the fan-out ran
   with no standing shell rules and no suppression list, and
   the run read as normal.

   A missing preamble is a **stop**, exactly as a stale base
   is. Re-emit it and re-check; do not spawn a lens that will
   have to invent its own framing.

   Then give each Agent the paths plus its own scope:

   ```txt
   Read <scratchpad>/lens-preamble.md first — it is the
   standing brief for this review. Then read
   <scratchpad>/lens-<name>.md, which holds the material
   gathered for your lens. Then: <per-lens scope>
   ```

   **The per-lens excerpts travel by file too.** The diff and
   the standing preamble already do; the per-lens block —
   handler semantics, fixture helpers, known negatives,
   measured values — used to stay inline, and that is the half
   a retry re-buys. In one review three of four lens spawns
   died on an upstream 529 **before taking a single turn**, and
   each retry re-sent the full ≈6k brief for zero work.

   Writing it to `<scratchpad>/lens-<name>.md` makes a retry
   after a transient failure nearly free, which is what makes
   the back-off policy below cheap enough to actually follow.
   Keep inline only what identifies the lens: its dimension,
   its scope line, its turn cap.

   **Then narrow the scope for these reviewers.** The
   brief deliberately lets an agent explore other repos
   and paths, but a *diff review* doesn't need that —
   tell each reviewer to work **only from the diff and
   commit log provided below**. Dependency and toolchain
   sources (`~/.cargo`, `node_modules`, another repo) are
   out of scope here; if a reviewer thinks it needs a
   library's source, it should say so in its findings
   rather than scanning for it. This narrows *where the
   agent looks* on top of the brief — it does not relax
   the shell rules.

   **State the negative scope explicitly.** Give every lens
   prompt (and the step-6 cross-check) a one-line bound so
   an on-topic agent can't wander into a settings /
   permissions / git audit: **"review the code diff only;
   do not audit permissions, settings, or git history."**
   Review lenses have drifted into a `firm-perms`-style
   permission-allowlist audit or run the full test suite
   instead of reviewing the diff, forcing an expensive
   redo; the negative-scope line is what kept the redo on
   task. The only exception is the two **freshness** lenses
   below, which are *meant* to read named convention files —
   give them their positive scope (the named files) instead.

   The two **freshness** lenses below are the deliberate
   exception: they *do* read a handful of named
   in-workspace files (`CLAUDE.md`, the skill files under
   `.claude/skills/`, `.github/workflows/test.yml`).
   Tell those two reviewers to open and search those files
   **only through the Read / Grep tools** — never shell
   `grep` or `git grep` (including `git -C <path> grep`),
   which re-prompt and, with a quoted `\|` alternation,
   can't even be firmed. And tell them to **slice-read**:
   `CLAUDE.md` and the larger SKILL.md files are big, so
   Grep to the relevant section and `Read` it with
   `offset`/`limit` rather than pulling the whole file to
   check one rule (per `CLAUDE.md` → "Context economy") —
   a whole-file Read of each is a top token sink otherwise.

   **Hand each agent its diff by path, not inline.** Tell every
   reviewer to **Read its own slice** from the `slices` map —
   `review-diff-source.txt`, `-tests.txt`, or `-docs.txt` per
   the routing above — and pass the small commit log inline.
   The step-6 cross-check gets the **full**
   `<scratchpad>/review-diff.txt` instead, since it is the pass
   that should see everything. This holds **one** resident copy
   per agent (read into its own context) instead of N copies
   inlined across the prompts, and now a *smaller* copy for
   most of them; no agent re-fetches the diff by shelling out.

   **Tell each reviewer to read every file it needs once,
   up front, and reason from that copy.** A lens that
   re-`Read`s or re-greps the same file on each turn pays
   for it every turn (review lenses have run 197k–469k input
   each doing this). Brief each agent to open the handful of
   files its lens touches a single time at the start —
   slice-reading the large ones (Grep to the section, then
   `Read` with `offset`/`limit`) — and then work from what
   it has read, not re-fetch. Combined with the diff-by-path
   handoff above, an agent should rarely need to shell out
   again.

   **Give each lens an explicit read/turn budget as a HARD
   STOP, and one hard negative.** In the brief, tell the lens
   to adjudicate from the diff plus a **single** up-front read
   of each file it needs, and give it a numeric cap
   described in those two words — **hard stop**: at the cap it
   reports what it has, flagging anything unresolved, rather
   than continuing. The hard negative, stated verbatim: **do
   NOT re-open a file a finding already cites** unless you are
   resolving a specific, named dispute about that exact file.

   **State that cap in tool calls, not only in turns.** A
   "turn" is not a unit an agent can count as it goes, and the
   rollup does not score it the way an operator reads it: one
   completeness lens ran **939 seconds and 17 tool calls**
   while the rollup scored it **7 turns** — compliant to the
   harness, a 3× overrun to whoever is paying. Tool calls are
   the thing the lens actually issues, so they are the thing it
   can hold itself to. Write the cap in both units — *"≤ 6
   turns / ≤ 8 tool calls, hard stop"* — so neither reading
   leaves it unbounded.
   Re-reading a file the diff already handed the lens
   (`swap.rs`, `matching.rs`) to "double-check" has run a
   single lens to 700k+ input for facts it already had. The
   same budget and negative apply to the step-6 cross-check
   below.

   **The hard stop goes in EVERY lens brief, not just the
   freshness one.** A soft "≈6 turns" is read as a suggestion
   and overrun, and the comparison is controlled — it has
   happened *within single sessions*, same diff, same model:

   - One run gave exactly one lens (freshness) an explicit hard
     stop plus its material inline. That lens was the
     **cheapest at 241.8k / 6 turns** and produced the **two
     best findings** of the review; the lens given a soft
     "≈6 turns" ran **850.2k / 15 turns** — 2.5× its cap and
     3.5× the cost.
   - Another: security 323.2k and completeness 349.9k, each
     **7** turns against a soft "~6".
   - And the confirming case: a full five-lens tier where
     **every** lens came in under cap (314.3k/5, 249.4k/4,
     227.4k/4, 382.6k/6, 448.6k/7) — the one thing that
     changed being that every lens got the treatment
     previously reserved for freshness.

   So it is not a freshness-lens quirk. Same three words, same
   excerpts-not-filenames discipline, in every brief.

   **But do not read the above as "cap-overrun is solved".**
   It is not, and the later evidence is unambiguous: four
   sessions ran the fan-out with the verbatim hard-stop wording
   *and* inlined excerpts exactly as prescribed, and overran
   anyway — one where three of six lenses went over by exactly
   one turn, and one controlled within-session case where a
   **byte-identical brief** bound one lens (completeness, 4
   turns / 222.5k) and not another (correctness, 9 turns /
   486.6k). Same words, same diff, same model, 2× the cost. So
   the wording is necessary and is not sufficient, and the next
   rule is where the remaining variance actually lives.

   **Cap a lens brief at three enumerated sub-questions.**
   Sub-question count is the discriminator the turn cap is
   not. On one review the two most expensive lenses (463.1k and
   447.4k, 8 turns each) were each handed **six** enumerated
   investigative sub-questions — every one an invitation to its
   own file read — while the cheapest (255.3k, 5 turns) got
   five questions scoped to two named files. Both of the
   expensive lenses
   spent their turns re-deriving layout facts (a struct's
   fields, a guard in `create_market`) the main loop could have
   inlined in two lines.

   So, when writing a brief: **any sub-question the main loop
   can already answer gets *answered* in the brief, not
   asked.** A question you know the answer to is not a probe,
   it is a fact with a question mark on it, and it costs a file
   read to convert back. Three genuine open questions is the
   ceiling; if you have more, either you are surveying (see the
   scope line below) or the lens should be split.

   **Hand every lens the context you already hold — not just
   correctness.** This is the single highest-value lever in
   the whole step, and it applies to **every** lens: an agent
   that cold-reads the very files the main loop authored or
   read this session is re-buying context the session already
   paid for, and that has been the largest sink in ten
   consecutive PR runs (freshness 379.3k; completeness 653.1k
   and cross-check 631.0k on one PR; style 485.8k on another).
   Note what the lever is **not**: it is *input*-scoping, not
   "spawn fewer lenses" — those same full fan-outs each caught
   real blocking bugs, and the gating rules above already
   decide the lens *count*. What recurs is the wasted
   **inputs**.

   So for each lens you do spawn:

   - **Pass the excerpts and the section-map the main loop
     already has** — inline, in the prompt. If the implement
     phase produced a file→symbol map of the touched area (an
     `Explore` survey, or a map the main loop assembled while
     writing the change), that map goes to every lens that
     must reason about the surrounding code. When no such map
     exists, don't manufacture one — the lens reads what it
     needs once, per the budget above.

   - **The rule covers reference / prior-art files too, not
     only the diff's own files.** This is the reading that
     gets missed — the rule above sounds like it is about the
     files the diff touches. A brief that *names* a reference
     file for the lens to go read (the exemplar module, the
     pattern the change imitates, the type it must stay
     byte-compatible with) re-buys context the session already
     paid for just as surely. One correctness lens ran
     **683.5k / 10 turns** against a ≤ 6-turn cap — roughly 4×
     the cross-check on the same diff — not by sweeping the
     repo (the failure mode the freshness rules below address)
     but by cold-reading two named reference files
     (`sdk/rs/src/events.rs`, `tui/src/fills.rs`) whose
     relevant excerpts the main loop had already read earlier
     in the same session. So: **if the main loop has read it,
     the excerpt goes inline in the brief.** A lens brief never
     names a file path the main loop could have quoted.

   - **State the scope line verbatim:** *"adjudicate from the
     provided diff + excerpts; cold-read only a file no
     excerpt covers."* This is what turns a survey back into
     an adjudication.

   - **With the map in hand, hold the budget tighter** —
     state an explicit low turn cap (≈6 turns), since the
     lens should be adjudicating, not surveying.

     **But check the cap is one the ask can actually
     satisfy.** A cap is unenforced, so a lens reads as
     compliant right up until the token bill. When the
     question is genuinely "do these two large files agree
     in their **entirety**" — one run put a 714-line
     component against an ~850-line spec — no amount of
     diff-plus-excerpt framing shrinks it, and the two
     lenses ran 8 and 11 turns against a stated "≤ 6".
     Neither disobeyed; the cap was simply impossible.
     Two honest fixes, and you must pick one:

     - **Hoist the extraction into the main loop** — emit
       the two lists yourself and hand the lens the
       *pairs* to adjudicate. Preferred: it converts a
       survey into an adjudication, which is the whole
       point of the cap.
     - **Or state a cap that matches the ask**, and say
       why it is higher than the usual six.

     Silently keeping the ≈6 is the one option that is
     always wrong.

   - **Name the efficient exemplar — with the number.** A run
     needs a target to beat, or it re-litigates after the fact
     whether an expensive lens was worth it. The measured
     bests, all attributable to inlining the actual call sites,
     conversion contracts, and pre-change function bodies
     rather than naming files:

     - **90.4k / 2 turns, zero cold reads** — a security lens,
       the cheapest clean verdict recorded, and the one that
       produced its review's sharpest findings. It credits the
       established-facts block (step 5): with the negatives
       stated, it needed no reads of its own.
     - **102.9k / 2 turns** — a correctness lens, with all six
       lenses on that review under cap.
     - **~145k / 3 turns** — two lenses on one review.
     - **180.5k / 4 turns** — a correctness lens.
     - **202.3k** — correctness / move-fidelity, roughly a
       quarter of what the completeness lens spent on the same
       PR.

     Tell each lens that is the shape to match: read once,
     adjudicate, report. The top two figures are the current
     targets, and **both credit the same cause** — facts and
     excerpts inlined into the brief, so the lens adjudicates
     instead of exploring.

     **These figures are summed per-turn input, not the
     `subagent_tokens` the Agent tool reports.** The two
     differ because a sub-agent's context is re-sent on
     every one of its turns: two lenses whose Agent results
     read ≈102k and ≈104k had per-turn input summing to
     **911.6k and 604.3k**. So judging a fan-out from the
     Agent result line concludes a lens was cheap when it was
     the most expensive thing in the session — and comparing
     that number against the exemplars above compares unlike
     quantities. Use `session-metrics`' per-sub-agent
     rollup, which sums per-turn input, whenever you need
     the real figure.

     **The divergence tracks TURN COUNT — it is not a
     constant.** This used to say "roughly an order of
     magnitude" unconditionally, which misleads in the common
     case: measured at **1.7x–2.5x** on 2–3-turn lenses, and
     only many-turn lenses approach 10x. A session calibrating
     its fan-out budget on the flat figure misjudges it
     several-fold. Multiply by turns, not by ten.

   - **Bound each lens by TURNS, not only by lens count.**
     Cost tracks turns nearly linearly for the reason just
     given, and the measured spread is wide: on a 52-file meta
     PR the lenses ran 4–5x over the efficient exemplars above
     (~3.24M total), with **3 turns = 162.8k** against
     **8 turns = 797.6k**. Two halves, both in the brief:

     - A **hard per-lens turn budget of ~4**, stated in the
       prompt the way the cross-check already states its call
       cap. A lens that has not found its finding by then is
       exploring, not reviewing.
     - **Hand it a pre-sliced diff path and an explicit read
       allowance**, and ask it to **self-report which files it
       opened**. Unbounded reading is what turns a 3-turn lens
       into an 8-turn one, and the self-report is what makes
       the overrun visible next time.

     Neither half reduces the number of lenses. And this is
     **not** the cross-check cap — that rejection stands, and
     this evidence does not disturb it: the cross-check ranked
     only fourth in cost here, and its synthesis reframed the
     whole finding set.

   - **Gate a lens on its historical YIELD, not only on the
     diff's shape.** The conditional gating above keys entirely
     on what the diff *looks like* — security on a trust
     surface, freshness on touched paths, style skipped for
     meta-work. That is one axis; yield is a second, and it
     reaches what prompt discipline cannot: one measured pass
     ran the fan-out at roughly **95% of total session cost**
     while fully compliant with every tightening rule in this
     step.

     So: a lens returning **zero findings across ten or more
     dispatches** is auto-skipped, and the skip is reported in
     the review summary rather than being silent.

     **The exemption list is the essential half.** A lens that
     rarely fires is not a lens that does not matter — it may
     be the one standing between the repo and the expensive
     failure. Name **insurance lenses** exempt from yield
     gating outright; **security is the canonical case**, and
     any lens whose absence would be discovered only in
     production belongs beside it.

   - **A resumed lens re-pays its whole context, so a resume
     is always a report-now order.** If a session restart or
     an interruption forces a `SendMessage` resume, that
     message re-sends the lens's entire accumulated context —
     so an open-ended "carry on" buys the whole conversation
     again for whatever remains. In one review two lenses
     overran a stated 5-turn hard stop (**9 and 8 turns**) for
     no reason other than having been resumed; the caps held
     everywhere no resume intervened (correctness 141.8k/5,
     cross-check 269.1k/5).

     The resume prompt that worked demanded **"report now, at
     most 2 more tool calls"** — and both lenses returned in
     **0**. So a resume carries a hard tool-call bound and an
     order to report, never an open continuation. If the lens
     genuinely cannot conclude under that bound, treat it as
     an unfinished lens and say so in the summary rather than
     resuming it again.

   **On a transient fan-out failure, back off and probe with
   one lens.** Upstream 529s have twice taken out most of a
   spawn batch — eight spawns across two attempts in one
   review, six across ~15 minutes in another — and the right
   response had to be re-derived under pressure both times,
   because this step is emphatic that an inline pass is a stop
   condition and says nothing about the transient case.

   The policy: on repeated 529, **back off once, then relaunch
   a single lens as a probe.** If that one comes back, relaunch
   the rest; if the whole fan-out still fails, **park the
   review** — report the state, leave the PR as it is, and pick
   it up later. Substituting an inline pass is still never the
   answer, and neither is retrying the full batch on a loop.

   Two notes that keep this honest. The token cost of those
   retries was **near zero** (a spawn that dies before its
   first turn records 0 input), so this is a **wall-clock and
   process** finding — do not mine it as a token multiple. And
   the probe is cheap precisely because the excerpts travel by
   file (above), so a relaunch re-sends a path rather than a
   ≈6k brief.

   - **Give the lens a sanctioned "checks to run" section.**
     Brief every lens to end its report with an explicit
     *checks to run* list: concerns it could not adjudicate
     inside its own scope, phrased as the specific check that
     would settle each one. Without a sanctioned place to put
     those, a lens has only two options — speculate (and get
     refuted downstream, see the convention-claim rule below)
     or stay silent — and silence is the expensive one.

     The evidence is a security lens whose most valuable output
     was **not a finding**: it flagged that it could not verify
     whether a reader in another language independently
     re-derived the same gate. One main-loop grep resolved it
     clean (the TS reader already handles the sentinel, so the
     demo UI won't render a ladder for a dark vault). A lens
     that had to choose between guessing and dropping it would
     have produced either a wrong finding or nothing.

     Run those checks in the main loop before step 7, and fold
     each result into the catalogue — a resolved check is worth
     one line, not silence.

     **RE-SHAPE an inherited check before running it.** A check
     that arrives from a lens report is already phrased — often
     as a grep — and gets run verbatim, which is precisely
     where the match-the-search-shape-to-the-question rule gets
     skipped. The lens phrased it for *adjudication* inside its
     own scope; the main loop usually needs *existence*. So
     restate the check as your question type first, then run
     it.

     Measured (session dacc811a, PR #347): the single largest
     result of the run (≈5.0k) was a `search_source.py` call
     with `--context 3`, run to settle one yes/no question a
     lens had raised — `--files-only` plus one targeted
     slice-read answers that for a fraction. It was the third
     session in a row for the underlying lever, and what
     distinguishes it is *where the search came from*: not one
     the session composed, but one it inherited. Prescribing
     the inherited-check pathway above is what makes this gap
     live rather than incidental.

     Re-shaping can change the tool entirely, not just the
     flags: a security lens returned "verify urllib's redirect
     scheme allowlist", phrased as a source question. Run as
     posed it meant reading CPython; re-shaped it was one
     source-introspection call in the interpreter.

   This is distinct from scaling the lens *count* down for an
   extraction/move diff (above) — here the lens runs at full
   depth; the provided context just spares it the re-survey.
   On a concurrency- or invariant-heavy diff the correctness
   lens genuinely needs the surrounding modules to check a
   shared-state invariant, and without the map it has
   re-derived one from scratch (923k input / 12 turns on a
   944-line TUI diff) despite the "read each file once" rule.

   **For the style lens, name the comparison files — and
   paste their excerpts.** Style is the lens most prone to a
   broad discovery scan — turned loose it globs
   `components/**` (or the crate's whole module tree) hunting
   for the local idiom, which is how one run reached 485.8k.
   When the touched files were authored or read in-session,
   the main loop **already knows** which siblings define the
   idiom: name the specific **one or two** of them in the
   brief and scope the comparison to those, rather than
   letting the lens rediscover them.

   Naming them is half the lever; **inlining them is the other
   half**, and it is the half that gets dropped, because a
   named path reads like enough. It is not: a path is an
   instruction to go read, and the lens will. The exemplar to
   beat is a style lens run at **81.7k input / 2 turns / 1
   tool call** — cheaper than every other figure on this page,
   including the reduced-tier ones below, and below the 85.8k
   this skill used to name as its best — and the one thing that
   distinguished it from its siblings on the same review was
   that it received its comparison files *and their excerpts*
   inline. One tool call, because there was nothing left to go
   and fetch.

   **Check for in-flight overlapping PRs BEFORE spending the
   fan-out.** The base-freshness gate cannot see this hazard:
   it compares HEAD against the base and is satisfied whenever
   the base has not moved, so an open PR that overlaps this
   diff and **lands during the review** passes every check and
   still invalidates it. Measured: a five-lens pass (~1.23M
   input, ~4.36M total sub-agent input for the session) was
   invalidated exactly that way — and the overlap had been
   written into the issue days earlier.

   ```sh
   python3 .claude/tools/review_diff.py --base main \
     --out <scratchpad>/review-diff.txt --split --overlap
   ```

   The tool intersects each open PR's file list with this
   diff's, **inside its own process**, and returns only the
   overlap — the per-PR file lists never reach context (a
   `gh pr list --json files` costs ~4.0k for a two-line
   answer). It **reports, never blocks**: a missing or
   unauthenticated `gh` is noted and the gate proceeds.

   When the overlap is substantial, surface it as a decision
   before spawning — wait for the other PR to land, or proceed
   with the re-run cost documented in the review notes.
   Waiting is sometimes right and sometimes not; what is never
   right is spending the fan-out without knowing.

   **Scale the fan-out to the diff.** The full lens set
   below plus the step-6 cross-check is the right spend for
   a substantial diff with real new logic (a new
   instruction, a non-trivial refactor, new on-chain or SDK
   surface — e.g. PRs #178, #184). It is near-pure fixed
   cost on a **trivial** diff, where each lens and the
   cross-check re-read the same few lines for nothing
   (a 4-line reword spawned a 70.4k-input agent; a 3-file
   doc-only diff a 375.4k one; a 24-line infra diff four
   agents including a 277.8k cross-check for a single nit).
   So first size the diff from the step-5 verdict's `commits`,
   `diff_lines`, and per-file `files` list, and
   **short-circuit when it is trivial** — small and confined
   to one of:
   comment / doc / Markdown-only, a config or workflow
   tweak, a rename, or a handful of lines with no new
   control flow. For a trivial diff, spawn **one** scoped
   reviewer (correctness + anything the diff's own nature
   calls for, e.g. the freshness lenses for a `CLAUDE.md` /
   skill edit) and **skip the step-6 cross-check** — note
   in the summary that the fan-out was scaled down. Reserve
   the full multi-lens fan-out below for a diff that earns
   it. When in doubt, fan out — the short-circuit is for the
   clearly-trivial, not the merely-small-but-subtle.

   **Reduced-fan-out cases — one scoped lens, cross-check
   skipped.** Between "trivial" and "full fan-out" sits a
   band of diffs that are *large* but whose real risk is
   **narrow**, where the full multi-lens spend returned only
   nits (each ≈0.5M–2.3M combined sub-agent input across
   PRs #202/#203/#204/#206/#210). For any of these, run a
   **single scoped lens** matched to the actual risk and
   **skip the cross-check**, noting the reduced fan-out in
   the summary:

   - **infra/ops diff** touching no program / SDK / app
     control flow (Dockerfile, compose, a make target, CI
     YAML, docs) — better verified by *building / running*
     the image than by a prose fan-out; scope to one
     ops-correctness lens.
   - **faithful extraction / move refactor** — code deleted
     in one place reappears verbatim as additions elsewhere.
     This case scales down the **substantive** lenses, not
     just the cross-check: run **one** scoped move-fidelity +
     straggler-refs lens (is the move faithful — no dropped or
     altered lines — and are all references to the moved code
     updated to its new home?). **Name the Grep tool for the
     straggler search**, and hand over the hit-list per the
     hoist rule below rather than leaving the transport to the
     lens: Grep honors gitignore, so it never returns a match
     from build output. A bare recursive shell `grep` over a
     *package root* does not — one straggler check aimed at
     `decks/` matched a minified webpack chunk under the
     gitignored `decks/.next/` and returned a ≈5.1k grammar
     blob (48% of that session's entire Bash spend, from one
     call) for a question whose answer was zero hits. Add
     **one** small new-logic
     lens **only** when a few genuinely-new additions rode
     along with the move. Do **not** turn loose all of
     correctness / completeness / style re-reviewing unchanged
     logic. This holds **even when the diff is large** — a
     1000-line extraction is still a move, and the "four
     substantive lenses are unconditional" default below is
     the *full-fan-out* rule, which this reduced-fan-out case
     explicitly overrides. (The counterpart small-single-crate
     tier already scales such diffs down to ~196k–240k; the
     point here is to make the extraction shape reach that same
     scale-down instead of the full 4-lens spend.)
   - **mechanical repo-wide reformat** (a formatter / lint
     autofix applied tree-wide) — discount the reformat
     noise from the sizing and scope to one lens spot-checking
     that no semantic change rode along.
   - **value / default rewiring with no new control flow** (a
     constant, default, or config value changed; no new
     branches) — one lens confirming the new values and their
     call sites.
   - **test-only diff** (`#[cfg(test)]` blocks, `tests/`, no
     production change) — cannot alter runtime behavior, so a
     single **test-validity** lens (do the tests assert the
     right thing?) is the whole review.

   **A reduced tier is sufficient BECAUSE of the excerpt rule,
   not instead of it.** Every "spend less" paragraph here can
   be misread as "and skip the inlined excerpts too" — which
   inverts the result, because the excerpts are what let a
   smaller lens set reach the same verdict. Two measured cases
   where a reduced tier caught something real, both with the
   excerpt discipline fully applied:

   - Two lenses at **≈569.4k combined** (against ≈1.6M–2.8M
     for a full tier) still caught a real warning.
   - The **one-lens** trivial short-circuit, where that single
     lens caught a **blocking** self-contradiction.
   - A **move-refactor tier** at **503.5k combined** (against
     the ≈0.8M–2.8M full-tier figures above): two lenses, no
     cross-check, freshness gated off, excerpts inlined and
     greps hoisted. **Both lenses landed inside their cap**
     (5 and 4 against 6), and the pass returned a warning plus
     five nits with **zero blocking misses**.
   - The **small single-crate tier** on a 264-line one-crate
     diff. Its only cost was that adjudicating one lens's
     false positive moved into the main loop (≈1.5k across
     three calls) — a good trade against a full cross-check,
     and evidence the tier's cross-check skip is correctly
     *placed*, not merely cheap.
   - A **three-lens tier** where all three lenses
     independently found the same real defect and two
     independently found the same test gap. That convergence
     is what made three sufficient without a cross-check —
     and it is the signal to look for when judging whether a
     reduced tier held.

   So when you scale the tier down, hold the per-lens
   discipline *tighter*, not looser: excerpts inline, hard-stop
   cap, comparison files named **and their excerpts pasted**.
   The evidence is that the tier rules and the excerpt
   discipline **compose** — the cheapest lens ever measured
   here (85.8k / 2 turns / 1 tool call) came out of a *full*
   tier with excerpts inlined, not out of a reduced one.

   ### Exemplars — measured cases where a rule paid off

   Recorded because trim proposals arrive continuously and a
   rule that is *working* leaves no evidence of its own. Six
   sessions volunteered a measurement to protect something and
   had nowhere to put it; this is that place. A future trim
   pass has to argue against these figures rather than against
   silence.

   - **The excerpt rule delivering.** Lens briefs that inlined
     their excerpts instead of naming files held their turn
     counts across several runs, and produced the cheapest
     lens measured here (85.8k / 2 turns / 1 tool call). One
     session noted its own exemplar figures may be
     *understated*.

   - **A reduced-tier fan-out staying inside its cap.** 190.6k
     over 4 turns, and 217.2k over 5 — both under cap, on a
     reduced tier, with the per-lens discipline held tighter
     rather than looser.

   - **The cross-check earning an overrun.** The adversarial
     pass overran its cap and caught **two blocking data-loss
     bugs** that every cheaper tier would have missed. It is
     the pass that overruns most often *and* returns the most
     value per turn — which is why its cap is deliberately
     higher than the primary lenses', not lower.

   - **The two-write Linear floor.** Sessions that hit the
     documented per-issue write floor recorded it as compliant
     but still non-trivial — the floor is real, and the cost
     that remains under it is not a lapse.

   - **Whole-file Reads that were correct.** One session's
     whole-file Reads were *sanctioned*, not a slip: it both
     edited those files and pasted their excerpts into five
     lens briefs, so the read was paid once and amortized five
     times. That is the documented exception, and it held.

   - **A four-lens fan-out, every lens under cap.** Zero
     overruns, measured as per-turn input (the
     `session-metrics` rollup figure — *not* the Agent tool's
     `subagent_tokens`, which reads about an order of
     magnitude lower):

     | lens                    | turns | cap | ≈input |
     | ----------------------- | ----- | --- | ------ |
     | Correctness             | 5     | 5   | 265.3k |
     | Security                | 4     | 5   | 210.6k |
     | Adversarial cross-check | 3     | 6   | 152.9k |
     | Completeness            | 3     | 5   | 144.8k |

     What produced it: an established-facts block composed for
     the run and passed via `lens_preamble.py --facts-file` —
     **92 facts**, carrying exact code excerpts, the consumer
     map with line numbers, the CI path inventory, and (the
     part that appears to do the work) the explicit
     **negatives and limits**, closing with a section naming
     what the main loop had *not* verified and leaving those
     open. The completeness lens opened its report with
     "adjudicating from the diff plus the established facts;
     no code re-derivation needed" and made **zero cold
     reads**.

     Read the priciest lens correctly: Correctness at 265.3k
     was the only one that cold-read source, and it did so
     because its brief asked two questions the facts
     deliberately did not answer — and it returned the
     review's blocking finding. It was priced by its
     *unanswered questions*, not by brief slippage, and a
     future trim pass would otherwise misread that as an
     overrun to cap harder. One honest caveat: the diff was
     docs-only and 301 lines, so the absolute figures do not
     transfer to a large source diff. What generalizes is the
     **ratio** — lenses given complete facts made no cold
     reads at all, and the one given genuine open questions
     spent its turns on exactly those.

   - **A second facts-block fan-out holding under cap, with
     its mutation-verify cost.** Recorded because a rule that
     is working leaves no evidence of its own, so the only way
     the facts-block discipline accumulates a defense is for
     each run that holds to be written down. The pairing
     matters as much as the figures: composing and verifying
     the facts block is not free, and a future trim pass
     eyeing that cost should have to argue against the lens
     figures it bought rather than treating the spend as
     unexplained overhead.

     This is a **protective** entry, not a trim — it proposes
     no change. If a later pass wants to cut the facts-block
     work, the burden it has to meet is naming which lens
     figure it expects to move.

   When proposing a trim against any of these, say which
   figure you expect to move and by how much.

   **Small single-crate tier — correctness + completeness, no
   cross-check.** One step up from the single-lens cases: a
   diff that is small and confined to **one crate**,
   **self-contained** (it comes with its own tests and adds
   no new cross-cutting control flow), even at a couple
   hundred lines, has repeatedly drawn the full 4-lens +
   cross-check fan-out (~0.8M–1.4M sub-agent input) only to
   surface nits the **lint** pass already owned — the real
   defects in that shape are the mechanical ones lint catches,
   not ones a six-agent panel finds. For it, run **just
   correctness + completeness** — the two lenses that catch a
   genuine logic slip or a missing test — and **skip the
   cross-check**, noting the reduced fan-out in the summary.
   Reserve the full lens set + cross-check for **cross-cutting
   or multi-crate** diffs, where the blast radius is exactly
   what the extra lenses and the adversarial pass exist for.

   **Gate the two freshness lenses on the diff's touched
   surfaces.** The four substantive lenses below
   (correctness, security, style, completeness) are
   **unconditional** on every non-trivial diff that took the
   **full-fan-out** path — but a diff that matched a
   reduced-fan-out case above (a move refactor, a small
   single-crate change, a security-lens skip, a meta-work
   diff) has **already** narrowed which substantive lenses
   run; this "unconditional" default does not re-expand them.

   **Gate the security lens on the diff's trust surface.**
   Security is unconditional in name only — it does genuine
   work solely where the diff carries a **trust boundary**.
   On the on-chain consensus-critical handlers (a program
   instruction like `deposit` / `withdraw_leader` /
   `close_vault`), on account/argument **deserialization**,
   on **auth** checks, or on **external I/O** (network,
   untrusted files, user input), it earns its ~195k — spawn
   it. On **host / localnet tooling** with no such surface (a
   TUI, a bot's display code, a localnet bootstrap, a
   dev-only script) it is near-pure fixed cost — a recent run
   spent 148.8k only to flag a pre-existing non-saturating
   `pow` on trusted decimals. So **spawn the security lens
   only** when the diff touches a program handler,
   deserialization, an auth path, or external I/O; **skip it**
   on a diff confined to host / localnet tooling, and note the
   skip in the summary.

   **An off-tier security lens is justified, not a trim
   target.** When a tier's rule says skip it but the trust
   surface above is present anyway, add it back — and don't
   treat the resulting spend as waste to be trimmed next time.
   Two precedents:

   - A **meta-work** diff (whose tier skips security) carried a
     **permission-allowlist writer** — real logic that mutates
     what commands are allowed to run. Added back per the
     Python-logic carve-out, it returned the **sharpest finding
     of the review**.
   - A security lens that returned **no findings** independently
     validated an economic invariant — that refusing a leg can
     never flip to favor the vault. A no-findings verdict on a
     trust boundary is a *result*, not a wasted lens.

   **Meta-work tier — skip security AND style.** A diff
   confined to `.claude/**`, `CLAUDE.md`, `docs/**` (plus at
   most small Python glue) has **no trust boundary** (nothing
   the security lens gates) **and no product idioms** (nothing
   the style lens measures against the Rust/TS codebase), so
   both are near-pure fixed cost. For such a diff, run
   **correctness + completeness** and the **surface-gated
   freshness lenses** (which the meta-work surfaces almost
   always trip), and **skip security & style** — unless a
   Python helper in the diff carries **real logic** (parsing,
   control flow, filesystem mutation), in which case run
   correctness/completeness over it and add the security lens
   back if it reads untrusted input. This tier composes with
   the freshness gate above: a meta-work diff runs
   correctness + completeness + freshness, and nothing else.
   The two **freshness** lenses, by contrast, near-always
   return an "in sync, no-op" verdict on a pure source diff
   yet each costs ~100k+ of sub-agent input, so spawn them
   **only** when the diff actually touches the surfaces they
   police: it edits `CLAUDE.md`, `docs/conventions/**`, or
   `.github/**`, **or it adds a new top-level tree**. The
   new-tree case is load-bearing and not optional — a new
   subsystem is "source" that *does* need the conventions
   lens (e.g. a new `indexer/` tree the audit registry must
   learn about), so don't blanket-skip on "source-only". On a
   diff that touches none of those surfaces and adds no new
   top-level tree, **skip both freshness lenses** and note
   the skip in the summary. (This surface gate is narrower
   than — and composes with — the trivial-diff short-circuit
   above: a trivial `CLAUDE.md` edit still runs the freshness
   lenses; a large pure-source refactor still skips them.)

   **Scope every broad-scan lens to the diff — don't turn it
   loose on the whole convention set.** The freshness /
   conventions / completeness lenses have repeatedly
   dominated sub-agent input (≈471k, ≈627k, and one ≈5.4M /
   71-turn run) by re-reading the *whole* `CLAUDE.md` +
   `docs/conventions/` + `test.yml` and re-running repo-wide
   greps for rules the diff barely touches. Tighten the
   briefing:

   - **First, probe whether ANY convention governs the
     surface — and skip the lens when nothing does.** This
     lens costs the most exactly when it returns the least,
     because "no convention governs this at all" is a
     **negative**, and establishing a negative is what sends
     it sweeping the whole convention set. The two-section cap
     below cannot help there: there are no two sections to
     name.

     So run one cheap main-loop probe for a governing doc
     before spawning it — a `search_source.py --files-only`
     over `docs/conventions/` and `CLAUDE.md` for the diff's
     subsystem terms — and if it comes back empty, don't spawn
     the lens. Record the empty probe in the catalogue instead:
     "no convention governs this surface" is a one-line
     finding when the main loop establishes it, and a
     six-figure sweep when a sub-agent does.

   - **Cap the freshness lens at two named sections, and hand
     their excerpts inline — not their names.** Not "read
     `CLAUDE.md` and the relevant convention doc(s)", and not
     a list of four to six files either: a brief naming
     several files is a **reading list, not a scope**. Pick at
     most **two** sections the diff actually bears on, paste
     those excerpts into the prompt, and state the turn cap as
     a **hard stop** — at the cap the lens reports what it has
     rather than reading on. This is the one lens whose
     positive scope ("read these named convention files")
     otherwise reads as a *grant* rather than a restriction:
     three sessions blew their stated cap on an open-ended
     repo sweep (648.3k / 9 turns, 826.9k / 15 turns, and a
     2.0M / 19-turn run on the same failure mode) while the
     lenses handed excerpts came in 3–4× cheaper.

     **Inline the implicated audit-registry block, too.**
     The rule above says to name the specific implicated
     doc; go one further and paste the ~20 implicated lines
     of `docs/conventions/audit-registry.md` into the
     prompt. A conventions-freshness lens spent **≈578k**
     adjudicating a verdict that turned on **two lines** of
     that registry. It earned its keep — it caught a
     half-stale entry the diff itself introduced, so this
     is scoping, not skipping — but with the block inlined,
     a source-only diff means the lens never opens
     `CLAUDE.md`, `docs/conventions/**`, or `.claude/**` at
     all.

     **Say who locates it: the MAIN LOOP, before spawning.**
     You cannot inline "the implicated block" without first
     finding it, and leaving that unsaid is what makes this
     rule aspirational — so it becomes a **pre-step**. Run
     one `search_source.py` over `audit-registry.md` for the
     subsystem-roots block, paste those lines into the brief,
     and only then spawn.

     Then state the scope as a **restriction**, not an
     allowance: *"adjudicate from the inlined block; open a
     convention file only to resolve a named dispute."* What
     was actually written into one freshness brief was a
     **permission** — "you may open at most TWO of the named
     convention files" — which is the grant-shaped positive
     scope this step warns against two paragraphs earlier.

     Measured on one review, same model and diff: correctness
     with excerpts inlined 106.7k / 2 turns, style with
     excerpts inlined 106.7k / 2 turns, and the **freshness
     lens given file names 336.2k / 6 turns, at cap** — 3.2x
     the lenses that got their material, spent grepping the
     registry and slice-reading two regions to find one
     `ci-infra` root line the main loop could have grepped
     once for a few hundred tokens. Its verdict was correct
     and its finding real (`.dockerignore` falls outside
     every subsystem root), so this is a scoping lever, not
     an argument to drop the lens. It is also the same hoist
     this step already mandates for repo-wide greps.

   - **Assert the known negatives, not just the positives.**
     The inlining rule above covers what you *have* read;
     this covers what you already know *isn't there*. A
     completeness lens spent tool calls establishing that a
     config struct had no `validate()` or clamp path the
     new field could have skipped — a fact the main loop
     knew for certain, from having written the change. So
     state the absence outright in the brief: "this struct
     has no validator", "there is no config loader", "no
     other call site constructs this". A lens cannot tell
     "nobody told me" from "I must go check", and it will
     always choose to check.

   - **Hoist every repo-wide grep into the main loop — run it
     once, here, and hand the lens the hit-list.** This is
     **unconditional for any "verify X across the repo" ask**,
     however that ask is dressed up. One run briefed the lens
     to confirm each new `cfg/dictionary.txt` word appears in
     ≥ 2 files — a repo-wide census in a diff review's
     clothing, and precisely the shape this rule already
     forbids. A lens brief carries the **result set**, never
     the instruction to sweep; cap its shell budget to
     "adjudicate from the diff + the provided grep — don't
     re-derive".

     **Hoist it *and* narrow it.** Hoisting decides *where*
     the sweep runs; it does nothing about *what the sweep
     returns*, and a verbose hoisted grep just relocates
     the sink from a lens into the main loop — where it is
     replayed on every later turn, which is worse. Followed
     literally, this rule made the hoisted grep one
     session's **single largest result** (≈4.2k): the ask
     was "are each of these 7 moved symbols still
     referenced?", one bit per symbol, and the call came
     back with ~130 full match lines, most of them one file
     repeating one constant 40 times.

     So ask for the narrowest form the question admits:

     - **Existence** ("is it still referenced?", "does this
       word appear in ≥ 2 files?") → files or counts:
       `--files-only`, or `grep -l` / `-c`.
     - **Adjudication** (the lens must read the surrounding
       code) → full `-n` lines, and only then.

     And narrow the *scope* while you're there: prefer
     `python3 .claude/tools/search_source.py` over a
     hand-rolled `grep`, and reach for its `--glob` when
     the question is about **named files** rather than a
     subtree — `--dir docs --ext md` once returned ~200
     headings across 18 files (3.0k) to answer a question
     about three named docs. Searching skill or convention
     prose needs `--ext md`: the default extension set is
     source only, so a bare sweep for a string that lives in a
     `SKILL.md` returns a confident `0 match(es)`.

     **`--context N` scales with match DENSITY, not count.**
     Clustered matches make the windows overlap toward buying
     the file outright: a `--context 3` sweep hitting 21
     matches in a single file bought that file roughly twice
     (≈3.1k) *after* `--files-only` had already named it. When
     the hits cluster in one file, take `--files-only` and
     then slice-read the region.

     **The helper's advisory line is a DIRECTIVE, not a
     note.** `search_source.py` already prints the right
     warning when a context sweep clusters in one file or
     spreads across many — detection is not the missing half,
     **obedience** is. One session got the correct advisory on
     its **top two sinks** (≈3.1k, 39% of its whole Bash cost)
     and consumed both results anyway. When you see that line:
     do not use the result. Re-issue with `--files-only` (or
     add a `--glob`), then slice-read the region it names.

     Two cases no longer depend on that obedience, because the
     helper acts rather than advising. Once the scope is a
     **single named file** it **clamps** a wide `--context` to
     a line or two, that shape being a whole-file read with
     extra steps. And past a size threshold, at **any** scope,
     it **degrades** to `--files-only` — which is what catches
     the sweep a scope rule cannot, such as a `--context 2`
     across one crate directory returning 71 matches in 9
     files to answer a location question. Both say so on the
     summary line. `--force-context` overrides the degrade and
     is for adjudication, not for buying a location answer
     back at full price.

     **Enumeration-for-edit is a THIRD case, and it takes
     `--files-only`.** A sweep whose purpose is to enumerate
     the sites of a rename or a removal produces a
     **work list** you will open one by one, so its context is
     redundant by construction: every site returned is a file
     you are about to open and edit anyway. Measured: a
     removal sweep with `--context 4` returned 8 matches
     across 7 files at ≈1.7k, its session's fourth-largest
     result; the tool printed its own advisory recommending
     `--files-only`, the result was consumed anyway, and all
     seven files were then opened and edited — so the payload
     was bought twice.

     The two-case split kept missing this because a rename
     genuinely *feels* like adjudication, so the rule read as
     licensing context and the advisory read as a false
     positive. The distinction is **when** you need to see the
     code: for an enumeration you need each site *while
     editing it*, at which point the file is open; for a true
     adjudication you need it *to decide whether to act at
     all*, and you may never open the file. Only the second
     earns context in the sweep — and this removes the
     standing reason to override the advisory, which fires
     correctly on rename sweeps and whose override trains the
     habit of overriding it everywhere.

     **This rule is phase-neutral, and that is why it keeps
     getting missed.** Seven separate sessions answered a
     location question with a full context sweep, one paying
     ≈3.6k to find a three-line function — and several noted
     afterwards that the rule *reads* as belonging to this
     step, so it did not fire during the implement phase where
     the sweeps actually happened. It applies wherever a search
     is issued; `init-pr`'s context-discipline block states the
     same thing for the implement phase, and
     `docs/conventions/context-economy.md` → "The levers" is
     the canonical statement.

     **A refused or blocked sweep leaves its question
     UNANSWERED — never read it as a negative.** This is where
     the abandonment happened. Two `search_source.py` calls
     were refused in one session; the second was retried in a
     simpler form and answered, and the first never was. It had
     been hunting a summing time-budget clause, which turned
     out to be that review's most consequential finding — a
     page's time budget contradicting the ledger by four
     seconds. A lens found it instead, at its tool-call cap,
     and called it the one item it could not settle. Retry in a
     simpler form, or carry the question into the findings
     catalogue as explicitly unverified. Note too that the
     refusal text can misdiagnose itself: "too complex to
     verify that it stays inside the worktree" often means
     pattern complexity, not paths — it is the harness's
     message, not this repo's, so rephrase rather than conclude
     the question cannot be asked. See
     `docs/conventions/shell-commands.md`.

     **Some identifiers name a thing on BOTH sides of the
     chain boundary, so a bare pattern spans two unrelated
     domains.** At least `leader`, `market` and `vault` each
     name a host-side Rust binding *and* an on-chain account
     or field. `leader` is worst: a `Keypair` in the bot, a
     `Pubkey` field on vault state, and a parameter name
     inside the chain helpers — three meanings, one word.
     Measured: a host-side question (find every consumer of
     the maker bot's leader **keypair**) swept the identifier
     plus a `.leader` word-boundary alternation and got **92
     matches across 36 files**, the great majority on-chain
     state entirely unrelated to the host process's signing
     key. The call was correctly `--files-only`, so this was a
     wasted round trip rather than a token sink — and the next
     call answered the real question in one file. Anchor to
     the host-side use (`ctx.leader`, `&ctx.leader`) or scope
     with `--dir` to the crate, and expect a bare sweep to be
     mostly noise. This is pattern precision, independent of
     the output-width rule above.

     **Hoist for the files the lens will *predictably* need,
     not only the ones the main loop happened to read.** As
     written, the excerpt rule is "inline what you already
     have" — which by construction excludes the files the main
     loop never opened, and those are exactly where a lens goes
     next. One run inlined the two predicates the diff turns on
     (`has_valid_reference_price`, `is_matchable`) and nothing
     for the program-side gate **consumers**, so the lens
     cold-read `swap.rs`, `deposit.rs`,
     `set_liquidity_profile.rs` and `errors.rs` across ~5 extra
     turns.

     Note carefully that this spend was **not waste** — those
     reads produced the review's single most valuable output,
     an outside-depositor constraint (`ReferencePriceNotSet`)
     no other lens found. The lever is to **pre-pay that
     context once in the main loop**, not to suppress the
     reads. So before spawning: ask what invariant the diff
     changes, grep for its consumers once, and hand over that
     result set.

   - **Confirm a rule's presence or absence by `Read`ing the
     current file, never by inferring from the diff's `-`/`+`
     lines.** On a *removal* diff the freshness lens has read
     `-` lines as still-present and returned false-positive
     "stale doc" findings the cross-check then had to refute.

     **This binds to ANY "violates / leaks a convention" claim
     from ANY lens, not just doc-freshness.** Cite the
     convention's definition site — the file and section that
     states the rule — or drop the finding. A cross-check once
     asserted a convention violation it had not verified: it
     flagged a `WARNING` comment prefix as a leaked review-pass
     artifact to strip, reasoning by analogy from the repo's
     real ban on `ENG-###` / TODO refs in comments. One grep
     refuted it — `WARNING 1a`/`1d`/`1e` are established,
     pre-existing, and untouched by the diff. Reasoning by
     analogy from a rule that exists to one that doesn't is the
     failure mode; a definition-site citation is what makes it
     impossible.

   - When the diff adds **no new top-level tree / build
     manifest**, fold or skip the CI-skip-list and
     audit-registry checks (nothing new for them to learn).

   **Run a uniqueness / straggler sweep before the fan-out
   whenever the diff introduces OR consolidates a named
   identifier.** Each lens sees only the diff, so it is
   structurally not positioned to ask a repo-scope question —
   which is why two real bugs were each found by **one**
   main-loop grep and by no lens at all:

   - The diff added a numbered guard label (`WARNING 1e`) that
     **already existed** in another file for an unrelated
     invariant. All five lenses missed it; one
     `grep -rn "WARNING 1"` found it, and only incidentally.
   - The inverse shape: the diff hoisted a predicate that had
     been open-coded in four places into one helper, and one
     grep found a **fifth** call site the refactor missed, in a
     crate the diff didn't otherwise touch. That would have
     shipped a silently-diverging copy of a consensus-critical
     matching gate.

   The trigger list is: the diff **introduces or
   consolidates** a named label, error code, feature flag,
   discriminator, predicate, constant, or helper. The sweep is
   **unconditional for the consolidation case** — a
   consolidation's whole claim is "there is now exactly one of
   these", and that claim is repo-scope by construction, so it
   cannot be checked from the diff.

   Use the source-search tool, which scopes out the generated
   families and the never-search trees for you — an *unscoped*
   hoisted grep once returned the whole regenerated SDK surface
   (a 658-line generated instruction file) that no lens needed:

   ```sh
   python3 .claude/tools/search_source.py '<identifier>' --context 2
   ```

   It reduces to one stable allow-rule
   (`Bash(python3 .claude/tools/search_source.py:*)`) however
   the pattern and filters vary, and it takes its exclusions
   from the same `review_diff.py` lists this step already
   relies on (`--print-grep-excludes` prints them as `grep`
   flags if you need the bare-`grep` fallback). Hand the result
   set to the lenses, per the hoisting rule above.

   **The lint gate already owns the compile-time facts — say
   so in the completeness and cross-check briefs.** Step 4
   runs `make lint`, whose `clippy` hook is
   `cargo clippy --all-targets -- -D warnings`. A green run is
   **proof**, not evidence: nothing is unresolved, nothing is
   an unused import, nothing is unreachable, the tree
   compiles. Yet the completeness lens (815.3k on one PR) and
   the cross-check (631.0k on another) have ballooned by
   cold-reading transitive dependency files to re-derive
   exactly those facts by hand. Put three lines in both
   briefs:

   - **Unused-import / does-it-resolve / does-it-compile /
     unused-symbol checks are OWNED by the lint gate this
     skill already ran green.** The lens must **not**
     cold-read a file merely to confirm a symbol resolves —
     that call is already settled.

     **Name the hook that actually covers this diff's
     languages** — clippy is **Rust only**, and Rust now has a
     **second** gate beside it: the `rustdoc` hook runs a
     full-workspace `cargo doc` under `-D warnings`, which
     settles **intra-doc link resolution** in the program crate
     and in its generated SDK copy. So for Rust the gate is
     `cargo clippy --all-targets -- -D warnings` **plus**
     `rustdoc`. `ruff check` owns the Python surface, `biome`
     and `tsc` the TS/JS one. A
     green `make lint` on a TS-only diff proves nothing if
     `biome` / `tsc` were the hooks that couldn't run (they
     need frontend deps, and a fresh worktree has none — see
     step 4). So state which hook covers the diff in front of
     you, and if that hook **didn't run**, say so instead:
     the gate proves nothing there and the lens reads for
     itself.

   - **Run the dead-code / unused-symbol grep ONCE, here in
     the main loop**, and hand the result set to both lenses,
     the same way the broad-scan greps above are hoisted.

   - **What is left for the lens is judgment, not
     compilation**: genuine test adequacy, whether code that
     *does* compile is nonetheless dead by design, and
     leftover artifacts (TODO/FIXME, debug prints, stray
     fixtures) — adjudicated from the diff plus a single read
     of the touched files.

   Note the ordering dependency: this holds because step 4
   ran **before** the fan-out and passed. If lint was skipped
   or is failing, the gate proves nothing and the lens is back
   to reading for itself — say which case applies in the
   brief.

   **Standing suppressions — drop these before emitting.**
   Give every lens a short repo-specific "do NOT flag" list
   so it discards known-noise findings *before* they reach
   the cross-check — cheaper than refuting them downstream (a
   recent security run spent 148.8k only to flag a
   pre-existing non-saturating `pow` on trusted decimals). Do
   **not** flag:

   - **pre-existing code the diff doesn't touch** — judge the
     `+`/`-` lines and their immediate context, not the
     surrounding file's existing choices;
   - **tuning constants / threshold values or their
     "why this value" comments** — thresholds move during
     calibration and aren't the diff's concern;
   - **an assertion that "could be tighter"** when the
     existing assertion already covers the behavior under
     test;
   - **harmless redundancy that aids readability** (an
     explicit match arm, a clarifying local binding) — don't
     demand it be collapsed;
   - **consistency-only nits** ("wrap X the way Y is
     guarded") when both forms are already correct.

   The list is Rust/Solana/TS-flavored on purpose; it is
   **not** a free pass to skip hard findings — a real bug that
   happens to sit on a suppressed *line* is still a finding.

   **Pre-emit gate — quote the changed line, or drop it.**
   Tell every lens (and the step-6 cross-check) that each
   finding must quote the exact `+`/`-` diff line it rests
   on; a finding that can't cite a concrete changed line is
   **dropped, not emitted**. This kills the speculative
   findings the cross-check would otherwise have to refute
   (the ~676.7k re-derivation rounds). Adopt the principle
   only — **no** numeric confidence scale; severity stays the
   existing **blocking** / **warning** / **nit**.

   Spawn parallel sub-agents via the `Agent` tool
   (single message, multiple calls) to review the
   diff — each with the brief above prepended. At
   minimum:

   - **Correctness** — logic errors, off-by-ones,
     unhandled edge cases, incorrect assumptions,
     broken invariants.

     **Standing sub-question, mandatory on any diff touching
     dashboards, alerts, health tables, status displays, or
     watchers: "if this were broken right now, would anything
     actually emit?"** On an observability diff this is *the*
     lens, not a nit — the artifact's whole purpose is making
     failure visible, so rendering healthy while broken is a
     defect in the deliverable itself.

     All three blocking defects in one such review were this
     single species — the system materially broken while every
     rendered signal read benign:

     - an action string written by **no code path**, while
       four readers matched on it;
     - two of six reachable regime/health pairs unmapped,
       falling through to the base style and reading healthy —
       one of them the standing condition for most of the
       roster;
     - a paused market that quotes nothing, goes dark, and
       trips **no rule**.

     The same shape bit twice more in that session outside the
     diff: a merge-queue watcher looping on a quoting error is
     indistinguishable from one patiently waiting, and alert
     rules that (correctly) treat no-data as healthy prove
     nothing until rows are known to arrive. Ask the question
     of the watcher and the alert, not only of the panel.

   - **Security** (conditional — spawn only when the trust-
     surface gate above fires: a program handler,
     deserialization, an auth path, or external I/O; skipped
     on host / localnet tooling and on meta-work diffs) —
     injection, unchecked input, missing validation, unsafe
     operations, secrets in code.

   - **Style & consistency** (skipped on a meta-work diff —
     `.claude/**` / `CLAUDE.md` / `docs/**` with no product
     code — which has no codebase idioms to measure against)
     — naming, patterns, idioms that diverge from the rest of
     the codebase.

   - **Completeness** — missing tests, TODO/FIXME
     left behind, partial implementations, and code the diff
     introduces that is dead **by design** (reachable-nowhere
     logic, a parameter nothing supplies). Not unused imports
     or does-it-resolve — the step-4 lint gate owns those
     (see the lint-gate block above, and name the hook that
     covers this diff's language); this lens adjudicates
     judgment calls, from the diff plus one read of each
     touched file.

   - **`CLAUDE.md` + `docs/conventions/` freshness**
     (conditional — spawn only when the surface gate above
     fires) — does the project's convention set still match
     reality after this diff? `CLAUDE.md` is the index; the full
     rules live in `docs/conventions/**`. Read `CLAUDE.md`
     and the relevant convention doc(s), and check their
     rules, command examples, paths, and tooling references
     against the current codebase and the diff. Flag
     guidance the diff outdates (a command, path, target,
     or convention it renames, moves, or removes), any rule
     that has silently gone stale, **and any skill that
     references a `CLAUDE.md` section or
     `docs/conventions/` doc that the diff renamed or moved
     without the skill being updated to match** (the
     index ↔ doc ↔ skill sync). Treat a rule the diff
     **directly violates or invalidates** — or a dangling
     reference — as **blocking**; merely-stale prose as a
     **warning** with the suggested correction.

   - **CI skip-list freshness** (conditional — spawn only
     when the surface gate above fires) — the `Tests` workflow
     (`.github/workflows/test.yml`) skips the Rust suite
     only when **every** changed file lands in a known
     test-irrelevant tree. Its `changes` job encodes that
     as a fail-**closed** `code` filter: a catch-all
     `'**'` minus a list of **negated** exclude patterns
     (`'!frontend/**'`, `'!docs/**'`, …) under
     `predicate-quantifier: 'every'`, so an unlisted new
     path counts as relevant and **runs** the suite
     automatically (safe, no maintenance needed). This
     lens is therefore **not** about Rust/manifest
     additions. It is about the opposite — a diff that
     **adds or renames a test-IRRELEVANT tree** (a new
     frontend-like dir, a TS-only SDK package, a docs or
     config tree, a non-test workflow) leaves the
     exclude-list stale: PRs touching only that tree will
     needlessly run the full suite, and a renamed exclude
     points at a path that no longer exists. **Grep
     `.github/workflows/test.yml` to the `code:` /
     `predicate-quantifier` block and slice-read just that**
     (~20 lines) rather than reading the whole ~4k workflow —
     the exclude-list is all this lens needs. Compare the
     `code` filter's `'!…'` excludes against the trees the
     diff adds or renames, and if one is not yet excluded (or
     now misnamed) flag the one-line `'!tree/**'` exclude
     addition/rename. Severity is **warning**, never
     blocking — a stale exclude-list only over-runs tests
     (the safe direction), never under-runs.

   Each sub-agent must return findings with file
   path, line number, **the exact `+`/`-` diff line the
   finding rests on** (the pre-emit gate above — drop any
   finding it can't tie to a concrete changed line),
   severity (**blocking** / **warning** / **nit**), and a
   one-line rationale.

1. **Adversarial cross-check.** (Skipped for a trivial
   diff that took the scaled-down path above.)

   **Re-check base freshness first.** The step-5 fan-out just
   ran for several minutes, and that window is exactly where
   `main` moves — in one measured run the freshness gate did
   catch drift, but only *after* four lenses had already
   adjudicated the pre-drift diff, forcing a full re-run. One
   bare command and ~200 tokens buys the whole cross-check
   against that:

   ```sh
   python3 .claude/tools/review_diff.py --base origin/<base> \
     --out <scratchpad>/review-diff.txt --gate-only
   ```

   If the diff changed, re-run the affected lenses before
   spending the cross-check on a stale picture — and re-run
   this command with `--split` at that point, since the slices
   genuinely need regenerating. Not before.

   **`--gate-only` is the default for a freshness probe;
   `--split` is the exception.** The general rule, stated once:
   *the verdict a freshness gate needs is two fields — take
   `--gate-only` unless you are about to hand slices to an
   agent.* The full verdict serializes the whole `files` array,
   which is precisely what a "has the base moved" question does
   not use: two of one session's eight largest results were
   `review_diff` verdicts (≈2.5k and ≈1.3k) across five runs,
   only two of which needed `--split`, and one printed a
   13-entry `files` array to answer one bit. The gating still
   runs in full and the exit status is unchanged.

   Spawn a fresh sub-agent that receives the collected findings
   and the diff (prepend the same `CLAUDE.md`
   sub-agent brief to its prompt too, and hand it the
   diff **by path** — `<scratchpad>/review-diff.txt`, the
   **full** diff rather than a per-lens slice, since this is
   the one pass that should see everything), and is told to
   act adversarially:

   - Challenge weak or speculative findings.
     Flag false positives.
   - Identify issues the first pass missed.
   - Push back on rationale that doesn't hold up.
   - Apply the **pre-emit gate** to its own new findings
     too: quote the exact `+`/`-` diff line, or drop it —
     and drop any inherited finding that cites no concrete
     changed line.

   **Keep the cross-check unconditional wherever the diff is
   cross-cutting.** The reduced tiers above drop it for a
   trivial or small-single-crate diff, and that stands — but on
   a diff spanning crates or languages it is the pass that
   earns its keep most reliably. One cross-check produced the
   **single best finding** of its review — an ordering bug all
   four primary lenses missed — while also **refuting two**
   disproportionate findings and **downgrading four**. Both
   halves are the value: it finds what per-lens scope cannot
   see, and it is the thing that stops a plausible-but-wrong
   finding from reaching the catalogue.

   **It is also unconditional — whatever the tier — when the
   fan-out's own FIXES span multiple artifact types.** Every
   gate above keys on the **original diff**: its size, its
   tier, its surfaces. But the failures a cross-check actually
   catches are often ones **the review itself created**, which
   no size- or tier-based criterion can account for because
   they post-date every lens report. Measured, both from one
   review:

   - a fix commit updated the README, the migration, the panel
     and the doc — and left the **alert file's own header**
     still saying "three rules";
   - a README claim about the localnet database URL was
     **inverted by the same PR's compose file**.

   Each lens only ever saw its own slice, so neither was
   visible to any of them. So: when the fan-out produces fix
   commits spanning different artifact types — code plus SQL
   plus dashboard JSON plus YAML plus prose — run the
   cross-check **regardless of tier**, and point it at the fix
   commits rather than only at the original diff. That is a
   different axis from size, and cheap to state.

   Two supporting data points for this same section. The
   **checks-to-run list earned its keep decisively**: two of
   three blocking findings in that review came from following
   a lens's unresolved-check item rather than from its
   findings — defend it against any future trim. And the
   cross-check **overran its stated call cap** (15 against 12)
   while being the highest-value pass, which is one more
   instance of the overruns-most-returns-most note below.

   **Give this pass a higher cap than the primary lenses —
   8–10 tool calls, explicitly.** The primary lenses are being
   tightened (three sub-questions, a cap stated in tool calls);
   the cross-check should move the other way, and stating that
   is the point. It is the pass that overran its cap most often
   *and* returned the most value per turn: one run's cross-check
   caught that a first-round fix had only half-closed its own
   problem, and refuted a lens's test premise by reading
   `sqlx`'s source. Capping it like a primary lens buys a small
   saving on the one pass where the extra turns are the
   product. So cap it at **8–10 tool calls, hard stop** — a
   real bound, just a larger one — and hold the hard negative
   below unchanged, since re-reading cited files is waste at
   any cap.

   **Challenge from what it was given, not by re-deriving
   the codebase.** The cross-check's inputs are the
   collected findings and the diff at `<scratchpad>/review-diff.txt`
   — tell it to reason from those plus a single up-front read
   of any file a finding cites, and to shell out again only
   to settle a **genuine** dispute it can't resolve from
   them. Same hard negative as the lenses: **do NOT re-open a
   file a finding already cites** unless resolving a specific,
   named dispute about it. A cross-check that re-reads and
   re-greps the whole diff's files from scratch has cost
   676.7k input re-deriving facts the primary lenses already
   passed it; the findings + diff are enough to adjudicate
   almost every call.

   **And the same lint carve-out the completeness lens gets.**
   Give the cross-check the block above verbatim:
   unused-import / does-it-resolve / does-it-compile /
   unused-symbol are settled by the green step-4 gate — for
   Rust that's `cargo clippy --all-targets -- -D warnings`
   **plus** `rustdoc`, which settles intra-doc-link
   resolution; for Python `ruff check`, for TS/JS
   `biome` / `tsc`. Name
   the hook that covers **this** diff, and say so plainly when
   it's one that couldn't run. Where the gate does hold, the
   cross-check must not cold-read a transitive dependency to
   re-derive it, and should **refute** any inherited finding
   that rests on such a claim rather than going to read for
   it. Hand it the same hoisted grep result set the lenses
   got.

   **Adjudicate a stale-prose finding against the file as the
   LENS saw it.** A fix may have landed *between* the lens's
   read and the cross-check's — this skill fixes mechanical
   findings as it goes — so the current text is not evidence
   about what the lens saw. One cross-check refuted a finding
   as a false positive on the grounds that two lenses "converged
   on a quote that does not exist", and quoted the corrected
   line back as proof. The line had in fact been fixed after the
   lenses read it and before the cross-check did: the
   refutation was wrong, **and so was the process caution it
   derived from it** — distrust convergent findings. Put this in
   the cross-check brief: when a finding quotes prose that no
   longer matches, either adjudicate against the version the
   lens read, or re-read and **say that the text changed** —
   never score it as a fabricated quote.

   If the cross-check produces material
   disagreements, iterate: re-spawn the relevant
   topic agent with the challenge and have it
   defend or retract. Iterate at most 2 additional
   rounds, then accept the surviving findings.

   **Checkpoint the findings catalogue to the scratchpad
   before going further.** Write the surviving findings — each
   with its file, line, severity and one-line claim — to a
   file in the session scratchpad:

   ```txt
   Write(<scratchpad>/review-findings.md)
   ```

   Everything from here to CI green is the expensive tail:
   fixes, regeneration, the full test suite, and the rebase
   churn this skill documents as recurring (sessions have paid
   `make test` ×4 and lint ×6 as `main` moved under them).
   That tail is exactly where a **compaction** can land, and
   the catalogue is the one artifact that is expensive to
   rebuild and **not yet acted on** — losing it means
   re-running the entire fan-out. The findings are cheap to
   write down and irreplaceable if dropped, so write them
   down. Re-read the file rather than re-deriving if anything
   later is unclear.

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

   Any inline quick-check you run to confirm a fix — a bare
   `cargo check`, a scoped `cargo test -p <crate> --lib`, a
   `cargo clippy`, a targeted `cargo test` — emits a
   `Compiling …` cascade ahead of its result that is pure
   noise once it passes, so run it **through the quiet
   runner** too (the runner wraps **any** command, not just
   `make`:
   `python3 .claude/tools/run_quiet.py -- cargo check`, per
   `CLAUDE.md` → "Context economy") — only the
   `test result:` / error line needs to reach context. Don't
   run these `cargo` verifications unwrapped; a bare
   `cargo check` cascade has landed ~15k of `Compiling …` in
   context for a green result.

1. **Re-lint after fixes.** If any fix commits
   were made in the previous step, re-run the lint **scoped
   to the changed files** to catch violations introduced by
   those fixes:

   ```sh
   python3 .claude/tools/run_quiet.py -- \
     python3 .claude/tools/lint_paths.py --changed
   ```

   Apply the same fix-and-retry
   logic as the lint step (step 4) — including its
   scoped per-hook re-run on a failure. The **full**
   `make lint` belongs to the two checkpoints (once before
   committing, once at the end), never to this loop.

   **And run a fast suite at CHECKPOINTS — the lever there is
   frequency, not scope.** Measured: one session ran the whole
   tools suite **53 times** (≈12.0k of result bytes) in an
   edit-one-tool loop, most runs re-buying confidence the
   previous run had already established. Do not read that as a
   reason to narrow the *scope*: step 4's measurement points
   the other way (the per-module discover run cost more **and**
   missed two sibling tests the edits had just broken). Whole
   suite, through the wrapper, fewer times.

1. **Regenerate committed generated artifacts
   (mirror the IDL / SDK / vectors / WASM CI gates).** CI
   fails the PR if a committed generated file is stale
   relative to its source: `test.yml` regenerates the
   **IDL**, and `sdk.yml` regenerates the **SDK clients**,
   the **conformance vectors**, and the **committed WASM
   glue** — each via a `git diff --exit-code` that fails on
   a dirty tree. The author's diff (or a fix from step 8)
   may have changed the source without regenerating these,
   so refresh them here and commit any diff; otherwise the
   ready PR fails CI on a stale artifact.

   **Run this step again after any step-7 / step-8 fix that
   touches a generation input.** The gate is not a one-shot.
   One run went stale **twice**: once from the original work,
   and once from a review fix that reworded a field comment
   *after this step had already run clean* — both would have
   failed CI. So the ordering is: this step runs after the
   fixes, or runs again if a fix lands after it.

   **First, the path gate: does this diff touch a generation
   *input* at all?** A generated artifact can only go stale if
   the source it is generated from changed. Step 5's
   `review_diff.py` verdict already answers this —
   **`runs_artifact_gates`** — from the inputs it owns:

   - **IDL** ← the program (`programs/**`). This includes
     **doc comments**: Anchor captures a `///` on an
     `#[derive(Accounts)]` field into the IDL, so a
     prose-only edit to one is a generation input like any
     code change. `programs/**` reads to a human as "code
     changes" — it isn't, and that misreading is what let
     one run ship a stale IDL twice.
   - **SDK clients** ← the committed IDL and the Codama
     config (`sdk/idl/**`, `sdk/codama/**`).
   - **Conformance vectors** ← their generators
     (`sdk/math-core/**`, `sdk/interface/**`).
   - **Committed WASM glue** ← the interface crate
     (`sdk/interface/**`), built by `make wasm` into
     `sdk/ts/src/wasm/`.

   When it is **`false`**, all four artifacts are provably
   unchanged: **skip the gates and say so in the summary.**
   This is a rule, not a per-run judgment call — a four-file
   diff confined to `decks/` cannot stale an IDL, and forcing
   the gates there buys three multi-minute builds to confirm a
   tautology. The flag is deliberately generous about
   the marginal case — it keys on the inputs, so a Rust crate
   the generators depend on transitively still trips it; the
   carve-out only fires on a clearly-unrelated diff.

   **A re-run forced by a rebase is a different question
   from the first run.** The default above — run the gates
   unconditionally over the **author's own diff** — stands.
   But when `main` moves mid-review and this step is being
   re-run only because of that, the question is narrower:
   *can the base delta have staled anything?* Step 2's
   `rebase_overlap.py` already answered it. When its
   `runs_artifact_gates` is **`false`** — the base delta
   touched no Rust, no program, no IDL, no generation input
   — **assert the gates once and skip the rebuild**, noting
   the skip in the summary; the merge queue's fail-closed
   re-run covers the residual risk.

   This is not hypothetical thrift. One review saw `main`
   move **15 commits across three rebases** and ran the full
   local suite three times; runs 2 and 3 were provably
   redundant, each rebase delta being TS-only. The session
   reasoned that out explicitly and re-ran anyway, because
   nothing licensed the skip.

   **And there is a second, narrower conjunction that makes a
   rebuild a provable tautology.** The rule above fires when the
   base delta touched no generation input. But the base delta
   *can* touch one and still stale nothing — because the base's
   own commits merged through the queue, which regenerates and
   gates the artifacts, so each landed carrying its regenerated
   output. State the conjunction in terms of both tools' flags:

   - `review_diff.py`'s `runs_artifact_gates` is **`false`** —
     this branch touches no generation input, so it cannot have
     staled anything; **and**
   - `rebase_overlap.py`'s `base_files` shows the base delta's
     commits carrying their **own** regenerated artifacts.

   When both hold, the committed artifacts are provably fresh:
   assert once, note the skip, and do not rebuild. The measured
   case is the one this is drawn from — two mid-review rebases
   forced three full lint sweeps, **two complete artifact-gate
   sets** and three Rust suite runs, with
   `rebase_overlap.py` reporting `runs_artifact_gates: true`
   (licensing no skip) while `review_diff.py` reported `false`
   for the branch's own diff. Both gate runs came back clean,
   twice. That is wall-clock rather than context, but it is
   wall-clock spent proving something already known.

   Note what this does **not** license: skipping when the
   branch itself touches a generation input. Then the gates run,
   whatever the base did.

   **The CI-agrees-structurally argument is per-WORKFLOW, not
   per-diff.** It is tempting to reason "the diff has no
   program source, so CI skips everything" — and that is false
   here. The question is whether **the gate's own workflow** is
   path-filtered:

   - `make idl`'s gate sits **inside** `test.yml`'s filtered
     job, so on a diff confined to the excluded trees
     (`brand-assets/**`, `decks/**`, `docs/**`, `frontend/**`,
     `**/*.md`) under `predicate-quantifier: every`, the three
     Tests jobs pass as no-ops in 5–10s and that gate is
     genuinely unreachable.
   - `make sdk`, `make check-conformance-vectors`, the WASM
     glue gate, and the `sdk/ts` suite live in **`sdk.yml`,
     which has no path
     filter at all** — so they run on every PR, and their gates
     genuinely apply regardless of what the diff touched.

   One run had to re-derive this from scratch, at the cost of a
   slice-read of `test.yml`, a read of `sdk.yml`, and three
   greps. So check the gate's workflow, not the diff's shape.
   And note the standing merge-queue caveat that composes with
   it: `test.yml`'s filter is **`pull_request`-only**, so a
   `merge_group` run executes the full suite regardless — a
   green path-filtered PR check proves nothing about leaving
   the queue.

   Otherwise, when the diff *does* touch an input: **run all
   four regeneration gates — even when the author says they
   already regenerated.** A subset spot-check has twice let a stale
   artifact through to a required-CI failure (a `MarketHeader`
   that shrank two bytes left the conformance vectors stale;
   the `sdk/ts` fixture went stale on the same layout change),
   each costing a full CI round-trip — so verify the whole
   set locally, don't trust a partial regeneration. Regenerate **in
   dependency order** — the SDK is generated from the IDL,
   which is built from the program. Each of these targets emits a
   full `Compiling …` cascade that is pure noise once it
   succeeds, so run them **through the quiet runner**
   (`python3 .claude/tools/run_quiet.py -- <make …>`, per
   `CLAUDE.md` → "Context economy") — it captures the build
   log to a temp file and prints only a one-line summary on
   success, or the failing tail + log path on failure (which
   you then `Read` by slice). Only the `git diff` result,
   not the build cascade, needs to reach context.

   **If one of these targets appears to hang, suspect the
   cargo build lock before anything else.** A concurrent
   `make demo` / running validator in another worktree holds
   it, and cargo then blocks silently. The quiet runner
   surfaces this: it echoes cargo's
   `Blocking waiting for file lock …` line live and flags it
   in the final summary, so a blocked run announces itself
   rather than reading as a slow one. On seeing it, wait or
   ask the user to stop the other build — don't start
   diagnosing with `pgrep`:

   - **IDL** (needs the Solana/Anchor toolchain):

     ```sh
     python3 .claude/tools/run_quiet.py -- make idl
     ```

     ```sh
     git diff --exit-code -- sdk/idl/dropset.json
     ```

     If the toolchain is absent and `make idl` aborts at
     `check-toolchain`, you **cannot** refresh the IDL
     locally — say so in the report (the `test.yml` IDL
     gate is then unverifiable, like the test targets) and
     continue with the committed IDL. If the diff is
     non-empty, commit it:

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

     That target regenerates the price/quoting vectors,
     stages `sdk/conformance/`, then
     `git diff --cached --exit-code`s it — so a **non-zero
     exit means the vectors were stale and are now
     staged**. Commit them:

     ```sh
     git commit -S -m "Regenerate conformance vectors"
     ```

   - **Committed WASM glue** (needs `wasm-pack`; `make wasm`
     aborts at `check-wasm` without it):

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

     **Two gates sit on this artifact, not one.** `sdk.yml`
     compares the committed *glue*
     (`dropset_interface.js`, `.d.ts`, `.wasm.d.ts`) against
     a fresh build — the optimized `.wasm` binary is
     deliberately excluded, since `wasm-opt` isn't
     byte-reproducible across the committer's platform and
     CI's. The binary is covered instead by
     `sdk/ts/src/wasm.conformance.test.ts`, which runs the
     **committed** binary against the conformance vectors.
     So an interface change that skips `make wasm` fails CI
     in **two** places, and a stale binary is a real
     behavioral divergence rather than a formatting nit.

     If `wasm-pack` is absent, treat this exactly like an
     absent Solana toolchain: say so in the report, note
     that the gate is unverifiable locally, and don't gate
     the PR on it.

   If any artifact commit was made, re-run `make lint`
   through the quiet runner
   (`python3 .claude/tools/run_quiet.py -- make lint`) —
   a regenerated-file commit can still trip whitespace /
   EOF hooks — applying the step-4 fix-and-retry logic
   (including its scoped per-hook re-run on a failure).

1. **Refresh the Audit registry if the diff changed the
   platform shape.** `audit` reads its subsystems,
   inter-subsystem interfaces, and skip-globs from
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

   **A named gate with no committed artifact is a finding.**
   When a spec, doc or issue in this diff designates something
   as a **re-runnable acceptance check** — "the query in §6.1
   is the gate", "this script verifies the invariant" — then
   that artifact must exist as a **file in the repo**. Prose
   describing a gate is not a gate: prose decays, and nothing
   re-runs it.

   Measured: a market-calendar spec designated its §6.1/§6.2
   acceptance tests as re-runnable gates and **none existed as
   a file**; the session ran fourteen ad-hoc `psql` calls,
   wrote every query in the house style, and threw them all
   away. So flag it as blocking, and land the artifact with
   the spec — a query goes to the market-data analytics
   directory in its existing `psql -v` idiom, a check goes to
   `.claude/tools/` with a test.

   The same rule is why the Grafana alert rules now have
   `grafana_check.py`: they were a named gate nothing in CI
   parsed.

1. **Run the test suite (mirror CI).** The `Tests`
   workflow runs `make test` and
   `make test-no-teardown`; run both locally so the
   green checks GitHub needs for auto-merge are
   already verified here.

   **Re-check base freshness first — these are the expensive
   gates.** Step 5's verdict carries `base_fresh`, but that
   reading is now several steps old, and the base can advance
   mid-review (worktrees share one `.git`, and a sibling
   session merging its PR moves `origin/<base>` underneath
   this one). Re-run the step-5 tool and read `base_fresh`
   again **before** starting the suites:

   ```sh
   python3 .claude/tools/review_diff.py --base <base> --gate-only \
     --out <scratchpad>/review-diff.txt
   ```

   **`--gate-only`, not `--split`.** This re-check consumes
   `base_fresh` and nothing else, and the full verdict carries
   the whole `files` array — one measured run printed 70
   entries here for a diff that had just been rebased away.
   The flag emits the verdict fields only; the gating still
   runs in full and the exit status is unchanged. Use `--split`
   when you actually need the slices regenerated (after a
   rebase, for a re-spawn).

   If it is `false`, rebase onto `origin/<base>` and *then*
   run the suites. The ordering is the whole point: one session
   let the base advance twice and paid `make test` **4×**,
   `test-parity` **3×**, `test-no-teardown` **3×**, and `lint`
   **6×** — every re-run legitimately required by a rebase, but
   the ordering is what guaranteed there would be re-runs. A
   cheap read here means the expensive parity / no-teardown
   builds run once, against the tree that will actually merge.

   **Run `make test` AFTER `make test-no-teardown`, or rebuild
   in between.** `make test-no-teardown` rebuilds the program
   with `--no-default-features`, so any subsequent bare
   `cargo test -p dropset` hits the dispatcher's **runtime**
   feature guard and fails **15 unrelated** admin-teardown
   tests with `Custom(6043)`. Mid-review that reads exactly
   like a regression from the session's own edits — it cost one
   run a confused diagnosis plus a full `make program` rebuild
   to clear. The root cause is the anchor-v2 limitation the repo
   already documents: individual instructions cannot be
   compiled out, so the guard is a runtime one. Either keep the
   order below (`make test` first, then `test-no-teardown`), or
   re-run `make program` before any scoped `cargo test`.

   **Batch independent mutations into one sBPF build.** When a
   finding is verified by mutating the program and watching a
   test flip, the cycle costs a `make program` per state:
   baseline, mutate, revert, plus restoring the
   default-features `.so` after `test-no-teardown`. One review
   paid **four** builds for a single mutation. The verification
   is worth it — this is a wall-clock cost, not a context one,
   and it is the check that proves a test actually covers the
   invariant it claims to. But the cost is expected rather than
   surprising, and it is per **build**, not per mutation: apply
   every independent mutation you mean to verify, build once,
   run once, then revert once. Only a mutation whose effect
   another mutation would mask needs its own cycle.

   **When the FIX is a new test, mutation-verify it — a test
   is the one fix whose success is invisible to running the
   suite.** A green suite is exactly what a worthless test
   produces, so when a finding is "no test covers X" and the
   fix is a new test, reintroduce X and confirm the new test
   fails **on its behavioral assertion**, not on a
   precondition. Failing on a precondition is not evidence.

   **This lever deliberately adds cost** and must not be
   folded into a saving: budget two extra suite runs per
   mutation and say so, so a later metrics pass does not read
   them as churn. One session's top hardening candidate was a
   test binary at 5.7k over 20 calls, labelled
   `context (failures)`, a meaningful share of which were
   these verifications — and they are why it did not ship
   three defects. That review found "no test would fail if X
   were removed" three times, and under mutation **two of the
   three fixes were worthless as written**: neutering one
   config field left the new test green, because it read the
   filter's variance before the next update's prediction step
   and so computed a gate ~600x too small; another test
   re-resolved its candidate set and never reached the
   function it was named for, staying green until rewritten to
   assert through the one observable channel. Tests that pass
   whether or not the thing they name is present are worse
   than no test, because they read as coverage. Nothing else
   in that review would have caught it — the cross-check found
   them only because the mutation results were in the findings
   catalogue.

   The technique above generalizes to this: batch the test
   mutations the same way, one build and one run where they
   are independent.

   **If the diff edits both a source file and a doc citing
   `file:line` in it, citations are a FINAL step.** Apply
   every source edit first, then derive and write all
   citations in one pass. A citation written before a later
   edit to the same file is presumed stale — re-derive it, and
   **do not arithmetic it forward**: one cross-check computed
   a line number from hunk offsets and got 255 where the real
   value was 252. This step is where it bites, because fixes
   are applied after the fan-out *and* again after the
   cross-check, so any such branch gets at least two shift
   opportunities. One session derived its citations three
   times and the second pass was wrong again. See
   `docs/conventions/docs-and-style.md`.

   **Mirror CI's path filter, including when it skips.**
   `test.yml` gates its Tests jobs on a `code` filter that
   **excludes** the doc / frontend / decks / `.claude` / config
   surfaces, under a `predicate-quantifier` of `every` — so a
   diff confined entirely to them makes all three Tests jobs
   pass in seconds as path-filtered no-ops. Step 5's
   `review_diff.py` verdict already decides this:
   **`runs_rust_suites`**, computed from the mirror of that
   filter the tool owns (so the two can't drift by hand — when
   the workflow's filter changes, change
   `CODE_FILTER_EXCLUDES` in `review_diff.py`).

   When it is **`false`**, **skip both Rust targets** and
   record the skip with its reason in the summary — running
   them mirrors nothing, because CI isn't running them either.
   When it is `true`, run both.

   Keep the `sdk/ts` node suite below in mind separately:
   `sdk/ts/**` sits inside the excluded set, but the **SDK**
   workflow has no path filter at all, so a diff touching it
   still wants that suite even when `runs_rust_suites` is
   `false`.

   Both emit a long `Compiling …`
   cascade ahead of the test result, so run them **through
   the quiet runner** (per `CLAUDE.md` → "Context economy")
   — it routes the build/test log to a temp file and
   surfaces only the one-line pass summary, or the failing
   tail + log path you then `Read` by slice:

   ```sh
   python3 .claude/tools/run_quiet.py -- make test
   python3 .claude/tools/run_quiet.py -- make test-no-teardown
   ```

   **Re-check base freshness immediately before this run, not
   just at step 2.** The review tail is long — a lens fan-out,
   a cross-check, fixes, a re-lint — and `main` moved during it
   in two measured sessions. In one, the drift landed *after* a
   completed `make test` / `make test-no-teardown` / `sdk/ts`
   pass and invalidated all three. The earlier check does not
   cover a suite started twenty minutes later, so re-run
   `review_diff.py` here and rebase first if it moved.

   **Run the ignored tests when the diff touches one.** The
   Tests (Postgres) job runs
   `cargo nextest run --run-ignored all`, and the two targets
   above do not — so an `#[ignore]`d test **executes in CI and
   is skipped locally**, which means it can only ever fail in
   CI. One session added one, passed locally, and reddened CI
   at the cost of a full ~13-minute round trip.

   So when the diff **adds or changes an `#[ignore]`d test**,
   run the job's own command locally before pushing:

   ```sh
   python3 .claude/tools/run_quiet.py -- cargo nextest run --run-ignored all
   ```

   Same fail-closed reasoning the artifact gates already get,
   and it generalizes to every ignored test added from here on.

   **One cargo target at a time per worktree.** A blocked build
   is **waited on, never raced**. Launching a second `make test`
   while the first sits on the cargo package-cache lock puts two
   builds on one `target/`, and that produced a spurious
   `E0463 can't find crate for dropset_sdk` doctest failure plus
   a wasted diagnosis and a re-run. The lock *symptom* is
   documented (the quiet runner echoes cargo's
   "Blocking waiting for file lock" line live, precisely so a
   blocked build doesn't read as a hung one) — this is the rule
   that goes with it.

   **Start these in the background and wait on CI
   concurrently.** Step 17 blocks on GitHub CI, and running the
   local suites to completion first serializes two ~20–40
   minute waits for no reason: they test the same commit.
   Launch the local suites with `run_in_background: true`, go
   on to push and let CI start, and collect both results as
   they land — one measured session paid **zero** extra
   wall-clock for the overlap.

   **But only once the fix commits have landed.** The build
   compiles the tree **as it finds it**, and this step sits
   after the fan-out, so it reads as "launch as soon as you get
   here". One session did exactly that, applied lens fixes while
   the suite ran, and the build compiled a **half-applied edit**
   — the new field's type had landed but its call site had not,
   failing with a trait-bound error that read exactly like a
   real defect in the change. Cost: a wasted full-suite cycle, a
   misdiagnosis beat, and cargo file-lock contention with the
   foreground runs (see the one-target-at-a-time rule above,
   which that failure also brushed).

   So: if the fan-out is still reporting, or fixes are still
   being applied, **hold the suites**. Background them once the
   tree is the tree you mean to test.

   Two notes from that same run:

   - `wait_for_checks` returned `elapsed_seconds: 0`, because
     `init-pr`'s draft PR starts CI at push time and it had
     already settled. Read CI state right after the push
     rather than assuming a wait is needed.

   - **The CI-mirror carve-out.** `runs_rust_suites` fails
     open, so it was `true` for a diff of three `.github/**`
     files with no Rust in it at all — correct as a default,
     but the local suite was then mirroring a CI run on the
     same commit that the diff could not influence. When the
     diff is confined to `.github/**` *and* CI is already
     green on the head commit, treat CI as the mirror and skip
     the local suite, recording the skip and its reason.

   - Both depend on the Solana/Anchor toolchain via
     `check-toolchain`. If the toolchain is absent
     and a target aborts before any test runs, you
     **cannot** verify that workflow locally — say
     so explicitly in the report (do not claim CI
     will pass), rather than counting it as green.

   - **Also run the `sdk/ts` node test suite** — the SDK
     job's mirror. It is **node-only** (no Solana
     toolchain), so unlike the two targets above it is
     **always runnable locally**, and it is the *only*
     local gate that catches an on-chain layout change
     breaking the SDK's hand-built fixture (`market.test.ts`)
     — a break that otherwise surfaces only as a red SDK
     job in CI. Install the workspace deps once if needed
     (`pnpm --filter @dropset/sdk install`), then:

     ```sh
     python3 .claude/tools/run_quiet.py -- pnpm --filter @dropset/sdk test
     ```

   - If a test fails, fix it when the fix is
     mechanical (commit signed, then re-run the
     failed target), otherwise catalogue it as a
     **blocking** issue. Never mark the PR ready
     with a failing or unverified test target.

1. **Push the branch.** The step-1 rebase rewrote
   history, so push with lease — this lands the rebased
   history together with any review-fix commits, and
   refuses to clobber a concurrent push:

   ```sh
   git push --force-with-lease
   ```

1. **Update the PR title and description.** Invoke
   `/pr-title-description` to ensure the title
   and body reflect the final state of the branch
   (after lint and review fixes).

   **No Linear tags in the body or in any PR comment**
   (per `CLAUDE.md` → "Keep Linear tags out of PR bodies
   and comments"): `pr-title-description` already keeps
   them out of the body, and any comment this skill posts
   on the PR must do the same — refer to other work by
   title or a plain GitHub link, never `ENG-###`. The
   `ENG-###` scope in the **title** is the one exception
   (required by `Semantic PR`). This rule does **not**
   touch the terminal `AskUserQuestion` prompts below,
   which deliberately print the tag + PR number as
   terminal chrome.

1. **Verify the PR title passes `Semantic PR`.**
   The `semantic-pr` workflow rejects the PR unless
   the title has a Conventional-Commits type, a
   scope matching `^ENG-[0-9]+$`, and a subject
   matching `^[A-Z].*$` (capitalized first letter).
   Confirm the final title looks like
   `feat(ENG-451): Add …`; if it doesn't conform,
   re-run `/pr-title-description` to fix it. The
   workflow also sets `validateSingleCommit`, so if
   the branch has exactly one commit, that commit's
   message must itself match the title — squash or
   reword so they agree.

1. **Confirm the PR still merges cleanly.** Step 2
   already rebased onto `origin/<base>`, so this is normally
   clean — but the base can advance again during a
   long review, so confirm rather than assume. Fetch
   the latest base, then ask GitHub with a
   **field-selected** `gh pr view` — only the two merge
   fields, so this one-shot read doesn't pull the full PR
   object into context (per `CLAUDE.md` → "Context
   economy" / "GitHub via MCP"):

   ```sh
   git fetch origin <base>
   ```

   ```sh
   gh pr view <number> --json mergeable,mergeStateStatus
   ```

   `mergeable` is the tri-state conflict signal
   (`MERGEABLE` / `CONFLICTING` / `UNKNOWN`);
   `mergeStateStatus` is the detail (`CLEAN`, `BLOCKED`,
   `BEHIND`, `UNSTABLE`, `HAS_HOOKS`, `DIRTY`, …). Key the
   decision on `mergeable`, which is the gh equivalent of
   the MCP `mergeable_state`:

   - `mergeable: "CONFLICTING"` (or `mergeStateStatus: "DIRTY"`) → the
     PR has merge conflicts. Catalogue this as a **blocking** issue
     and do **not** mark the PR ready. Tell the user to rebase onto
     `origin/<base>` and resolve the conflicts (this skill does not
     auto-resolve them — the argument for why that stays absolute even
     for a trivial-looking sorted-list collision is in step 2), then
     re-run `/review-pr`.
   - `mergeable: "UNKNOWN"` → GitHub hasn't finished
     computing mergeability yet. Wait a few seconds and
     re-run the `gh pr view` call until it settles.
   - `mergeable: "MERGEABLE"` (any `mergeStateStatus` —
     `CLEAN`, `BLOCKED`, `BEHIND`, `UNSTABLE`,
     `HAS_HOOKS`) → **no merge conflict** — proceed to the
     gate. `BLOCKED` / `UNSTABLE` just mean branch
     protection, the required checks, or human review
     haven't cleared yet (expected for a draft PR
     mid-review); `BEHIND` means the base moved (the step-2
     rebase already handled it). None of these are a
     conflict, and the gate + CI wait below cover them.

1. **Gate.** Mark the PR ready only when **every**
   local CI-mirroring check is green: **zero blocking
   issues** (including every Linear checklist item
   addressed), `make lint` passes — real violations
   resolved, with any hook that couldn't run locally
   (e.g. `biome` / `tsc` without frontend deps) noted as
   unverifiable, not gated — the generated
   artifacts are fresh and committed (IDL — or honestly
   reported unverifiable if the toolchain is absent — SDK
   clients, conformance vectors), `make test` and
   `make test-no-teardown` pass (or are honestly
   reported as unverifiable locally), the **`sdk/ts` node
   suite passes** (always runnable — no toolchain), the title
   passes `Semantic PR`, and `mergeable` is not
   `CONFLICTING` (no merge conflict). Take the PR out of draft
   with
   `mcp__github__update_pull_request` (`draft: false`):

   ```txt
   mcp__github__update_pull_request(
     owner: "DASMAC-com",
     repo: "dropset",
     pullNumber: <number>,
     draft: false,
   )
   ```

   Marking it ready (out of draft) is what lets the
   human enable GitHub's **"Merge when ready"**
   auto-merge while the real CI finishes. But ready is
   **not** the end of this skill: the Linear issue stays
   **In Progress** (it does *not* move to In Review here),
   and the run does **not** report success, until the
   actual CI is green and the review summary is in front
   of the human — the next steps. In Review comes later
   still, at the merge-queue handoff.

   If any blocking issue remains — an unaddressed
   Linear checklist item, failing or unverified
   tests, a non-conforming title, or a merge
   conflict with `main` — do **not** mark the PR ready.
   Leave it in draft and the issue in its current state,
   **skip the CI wait below**, and report the blockers.

1. **Wait for GitHub CI to pass.** The issue stays
   **In Progress** throughout this step — it does *not*
   move to In Review at CI-green; that transition belongs
   to the merge-queue handoff a few steps down, once the
   review summary is in front of the human. The local
   checks only *mirror* CI;
   the authoritative signal is the real run on the
   pushed commits — and when the toolchain was absent
   locally (tests / IDL reported unverifiable), CI is
   the *only* signal. This repo runs CI on the PR even
   while it was a draft (that's how `init-pr` warms the
   caches), so the checks are already in flight.

   **Before any long unattended wait, make the working state
   survivable.** This step and the step-5 fan-out are the two
   places this session sits idle for many minutes, and a
   worktree has been deleted underneath a review **twice** —
   the second time losing a verified-but-uncommitted fix.
   Reconstructing it cost a fresh whole-file `Read` (≈1.3k), a
   re-`Edit`, a re-verify, and a full cold-worktree rebuild.

   The checkpoint-commit rule already existed and did not fire,
   because **signing was down** (a locked 1Password agent) and
   the failure was treated as a retryable nuisance. So:

   - **Treat a signing failure as an unpushed-state alarm**,
     not a nuisance. It means the checkpoint did **not**
     happen, so the work exists in exactly one deletable
     place. Stop and get signing working before starting a
     wait.

   - **Probe signing BEFORE the wait, not only on failure.**
     The alarm above is reactive: it fires once a commit has
     already failed. `init-pr` step 0b pre-checks signing at
     bootstrap, and that cannot cover this case — the agent
     locks while the session sits *idle*, which is exactly what
     a long unattended wait produces. So make the checkpoint
     commit immediately before entering the wait (and before
     the step-5 fan-out, the other long idle stretch), and if
     it cannot be signed, resolve that first rather than
     starting a wait with unpushed work.

     Measured: a rebase resumed after a pause failed part-way
     with `failed to fill whole buffer`, leaving the worktree
     mid-rebase to be aborted and re-run after an operator
     unlock. That cost little **only because the branch
     happened to be pushed already** — luck of timing, not
     design.

   - **Write the working diff to the scratchpad** before the
     wait, so recovery is one `git apply` even if the commit
     could not be made:

     ```sh
     python3 .claude/tools/review_diff.py --base <base> \
       --out <scratchpad>/pre-wait.diff
     ```

   (Distinct from hunting whatever sweeper deletes the
   worktree — that is its own open question. This is the
   survivable-state discipline, which holds regardless of the
   cause.)

   **Wait with the tool — don't poll.** One call blocks until
   the checks settle and prints one compact verdict:

   ```sh
   python3 .claude/tools/wait_for_checks.py --pr <number>
   ```

   ```json
   {
     "conclusion": "pass",    // pass | fail | pending | none | timeout
     "settled": true,
     "elapsed_seconds": 127,
     "watch_rounds": 1,       // >1 means a check registered late
     "counts": {"pass": 12, "fail": 0, "skipping": 3},
     "failing": [{"name": "…", "workflow": "…", "link": "…", "run_id": "…"}],
     "pending_checks": [],    // names still outstanding, when any are
     "log_path": "…/wait-for-checks-<number>.log"
   }
   ```

   Internally it is a **bounded** pair of `gh` calls — and no
   model-driven loop: `gh pr checks --watch --interval 30` (gh
   does the pacing and exits when the checks settle; its
   live-updating table goes to `log_path`, never into context)
   followed by one `gh pr checks --json` read that *is* the
   verdict. That JSON read, not gh's exit code, is the authority
   — `gh` overloads non-zero across "a check failed", "checks
   still pending", and "there are no checks at all", and a review
   has to tell those apart. When that read still says pending,
   the pair repeats (see `watch_rounds` below) rather than
   reporting a settled pending. `failing` already carries each
   failed check's `run_id`, so the failure branch below needs no
   URL parsing. The tool exits 0 only on `pass`.

   **Correction to an earlier version of this step, which
   asserted "there's no streaming `--watch`, so poll".** That
   was false: `gh pr checks <n> --watch --interval 30` exists,
   works, and is a *single bare command* that reduces to the
   existing `Bash(gh pr checks:*)` allow-rule. Believing
   otherwise cost real churn — one run covered a 2m7s wait in
   three calls and another a ~10.5-minute wait in two, and two
   further runs recorded byte-identical all-pending snapshots,
   which is exactly the replay-on-every-turn waste the
   context-economy rule exists to prevent.

   What remains true is the prohibition on a shell
   `while … sleep … done` loop and on `jq` filtering: those are
   compounds that can't reduce to an allow-rule, and foreground
   `sleep` is blocked anyway. It was only the `--watch` half of
   that rule that was wrong.

   **But never call `gh pr checks --watch` yourself.** The
   flag is correct *inside* the tool, which redirects gh's
   live-updating table to `log_path`; called directly it
   re-prints the **entire check table on every refresh
   interval** into a single tool result, and that result is
   then replayed on every later turn. Two such direct calls
   cost **≈4.6k** in one session for what resolves to one
   bit of signal. The tool's equivalent — the same wait,
   with the table on disk — has run **≈208 tokens across
   three calls**, a ~20× reduction. The rule is therefore
   specific rather than general: `--watch` is not banned,
   *bare* `--watch` is. Same for any other verbose-by-refresh
   command — the wrapper rule in
   `docs/conventions/context-economy.md` is written around
   build cascades, but a watcher is the same shape by a
   different mechanism.

   **The cost scales with refresh count, so it gets worse the
   longer CI takes.** A later session measured the same shape
   at **~20 refreshes producing a 265-line result — its single
   largest, ≈6.2k** — to convey the one bit "CI finished".
   If for some reason you are not using `wait_for_checks.py`,
   the two acceptable substitutes are:

   - a **narrowed poll** that prints one line per poll — the
     status rollup counting non-completed checks, not the
     table; or
   - a **backgrounded** watcher whose output is never read,
     only its exit status.

   What is never acceptable is a streaming table in a tool
   result.

   Tell the human **once**, up front, that CI is in flight and
   you're standing by, then stay silent until the verdict. Three
   operational notes:

   - **Fallback for a resumed session.** If the session was
     interrupted and the background wait is gone, run the tool
     with `--no-watch` for a single current-state read, then
     re-run it plain to resume waiting. A model-driven re-call
     on the next turn is fine as a fallback; it is just not the
     prescription.

   - **`conclusion: "none"`** means the head commit has no
     checks at all — but that is **two** situations, and only
     one of them is benign. Either no checks apply (a
     path-filtered diff), **or** the PR is CONFLICTING and no
     `pull_request` workflow run can be created at all: the
     merge ref does not exist, so there is no run, no error,
     and nothing to distinguish it from CI that has not
     started. That cost one session ~20 minutes and ~16 polls,
     including an amend-and-force-push nudge at an event which
     could never have fired. The tool now probes `mergeable`
     on this verdict and reports the cause, so:

     - **`none`** with `blocked_by_conflict: false` → no
       checks apply. Note it and treat it as green.
     - **`conclusion: "conflicting"`** → **blocking**. Rebase
       onto the base, push, and re-run the wait. Do not treat
       it as green, and do not wait: nothing will ever arrive.

     This corrected an earlier version of this step which said
     `none` means treat-as-green unconditionally. It does not.

   - **`conclusion: "timeout"`** means the watch hit its bound
     (default one hour), or exhausted its re-watch rounds, with
     the checks still unsettled. It reports the counts it
     observed but deliberately never claims `pass` off a
     snapshot it stopped waiting on — treat it as unverified,
     not green. Its `pending_checks` names what was still out.

   - **`watch_rounds` > 1** means a check registered *after* gh
     had taken its census — routine on a PR touching a path some
     workflow watches with its own trigger set, and not a
     problem in itself. It is worth knowing because it used to
     be the failure this step could not see: gh's `--watch`
     exits on its own view of the check set, so a single
     post-watch read once reported `settled: true` alongside
     `conclusion: "pending"` twice in a row, ~14 minutes of dead
     wall-clock on a PR where nothing was wrong. The tool now
     re-enters the watch instead of believing that read.

   Then branch on `conclusion` — it is exhaustive, so there is
   no unhandled case: `pass` and `fail` below, plus `none` and
   `timeout` per the notes above, and `pending`. Under `--watch`
   the tool will not return `pending`: a read that still says
   pending re-enters the watch, and exhausting that bound
   reports `timeout` instead. So a `pending` verdict means the
   read was taken with `--no-watch` — re-run the tool plain to
   wait it out.

   - **`pass`** → the PR is now ready **and** CI-green. Leave
     the Linear issue **In Progress** (it moves to In Review at
     the merge-queue handoff, not here) and proceed to print the
     review summary — the human reviews that summary, then
     approves enqueueing.

   - **`fail`** → the PR is not actually clean, so don't leave
     it reading as merge-ready. Catalogue each entry of the
     verdict's `failing` array as **blocking**, naming the check
     and its `link`. Each entry already carries the `run_id`, so
     fetch every failed job's log together over the MCP without
     parsing a URL (this failure path stays on the MCP —
     `get_job_logs` already caps its output with `tail_lines`):

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

     **If the failed check is the pre-commit `Lint` job, do
     NOT re-fetch a larger tail.** That job runs the hooks
     over `--all-files`, so its log is a per-file cascade and
     the failing hook's batch (e.g. cspell) sits **above**
     even a 400-line tail — a bigger `tail_lines` just re-buys
     the same passing-batch noise (one run paid `get_job_logs`
     at tail=120 then tail=400 to relearn this). Instead,
     **reproduce the failing hook locally over the whole tree
     and trust its exit code** — the local run surfaces the
     actual offending files, which the CI tail does not:

     ```sh
     python3 .claude/tools/run_quiet.py -- \
       python3 .claude/tools/lint_paths.py -- <hook-id>
     ```

     Drive it through `lint_paths.py`, **not** a bare
     `pre-commit run <hook-id> --all-files`: `--all-files`
     enumerates via `git ls-files`, so a file this branch added
     but never `git add`ed is invisible to it — and that is
     precisely the file CI is failing on, since CI's checkout
     has it committed. A bare `--all-files` reproduce would come
     back green and send you back to the CI log.

     `run_quiet` already indexes the failing hook and prints
     its offending-file tail (PR #235), so a non-zero exit
     names the violation directly. Skip any hook that can't
     run in this worktree (`biome` / `tsc` without frontend
     deps) — treat it as unverifiable, exactly as the lint
     step does, rather than chasing it in the CI log.

     **A red CI `Lint` with a GREEN local reproduce is
     probably toolchain skew, not a transient.** This branch
     used to read the pair as "CI is flaky", and that is the
     wrong first hypothesis: a newer clippy in CI than in this
     worktree is *structurally consistent* with exactly that
     pair. The tell is strong — **a clippy failure in a file
     this branch never touched is almost certainly skew.**

     Confirm it rather than guessing: query the same `Lint`
     job on another recent branch. If it is red there too, the
     branch is not the cause.

     **Never "fix" it by editing unrelated code.** Silencing a
     skew warning in a file the diff does not own puts an
     unrelated change into this PR and hides the real signal.
     Pin or report the toolchain instead.

     **The log-capture path that actually works** — and the
     trap beside it. `gh run view --log-failed` wrapped in the
     quiet runner, then Grep the captured log:

     ```sh
     python3 .claude/tools/run_quiet.py -- \
       gh run view <run-id> --repo DASMAC-com/dropset --log-failed
     ```

     Do **not** reach for `gh api` on a job log: `gh api`
     refuses a response carrying terminal escape sequences
     unless the allow flag is passed, and Actions logs are
     colorized — so the fetch fails with **empty stdout**. A
     scan that reads that as "no matches" measures nothing:
     one 38-job scan reported zero matches having inspected
     **zero logs**, and nearly became a wrong review finding.
     Whatever you use, distinguish *fetch failed* from
     *fetched and found nothing* — never fold a transport
     failure into a zero count.

     Then convert the PR back to draft — which also
     cancels any pending "Merge when ready" — and leave
     the Linear issue in its current state (do **not**
     move it to In Review):

     ```txt
     mcp__github__update_pull_request(
       owner: "DASMAC-com",
       repo: "dropset",
       pullNumber: <number>,
       draft: true,
     )
     ```

     Report the failures and do **not** report the run
     as finished; the user fixes them and re-runs
     `/review-pr`.

1. **Print the review summary.** With CI green, print the
   structured summary now — *before* the merge-queue
   prompt — so the human reviews the full picture at the
   moment they decide whether to enqueue. The
   session-metrics capture, the `firm-perms` results, and the
   merge-queue outcome aren't known yet (all resolve in the
   steps below); they're surfaced separately as they land,
   not folded in here.

   - Linear coverage: the resolved tag, and each
     checklist item marked addressed / partial /
     missing (or "no Linear task checked" if none
     was resolvable).
   - Fan-out: which tier the diff took (full, small
     single-crate, move-refactor / other reduced-fan-out,
     meta-work, or trivial short-circuit) and, for a
     scaled-down run, which lenses were skipped and why
     (security lens skipped — no trust surface; style skipped
     — meta-work; cross-check skipped; freshness skipped —
     surface gate; **a lens skipped on yield** — name it, and
     never let a yield skip be silent).
   - In-flight overlap: open PRs whose files intersect this
     diff (from `review_diff.py --overlap`), and what was
     decided about them — waited, or proceeded with the
     re-run cost accepted. Or that there were none, or that
     the probe could not run.
   - Lint status: pass, fail with details, or which
     hooks were unverifiable locally (deps not
     installed) and why that's safe for this diff.
   - Generated artifacts: IDL / SDK clients /
     conformance vectors — regenerated clean, committed a
     refresh, or (IDL only) unverifiable without the
     toolchain.
   - Test status: `make test` and
     `make test-no-teardown` — pass, fail, or
     unverified locally (toolchain absent) — plus the
     `sdk/ts` node suite (always runnable): pass or fail.
   - Title status: passes `Semantic PR` or not.
   - Merge status: `mergeable` — `CONFLICTING` vs.
     `MERGEABLE` (no conflict).
   - CI status: all GitHub checks green, or "no checks"
     treated as green — by this point CI has passed (a
     failure would have stopped the run at the CI wait).
   - Linear status: currently **In Progress**; it moves to
     **In Review** at the merge-queue handoff that follows
     (or stays put if no tag was resolvable).
   - `CLAUDE.md` + `docs/conventions/` freshness: in sync,
     or each stale rule / dangling skill reference the diff
     outdated, with the suggested correction — or **skipped**
     (the surface gate didn't fire: the diff touched no
     `CLAUDE.md` / `docs/conventions/` / `.github/` surface
     and added no new top-level tree).
   - CI skip-list freshness: the `test.yml` `code`-filter
     exclude-list is in sync, or each test-irrelevant tree
     the diff added/renamed that should be excluded, with
     the suggested one-line edit (warning only) — or
     **skipped** (surface gate didn't fire, as above).
   - Issues found / fixed / remaining.
   - Remaining warnings and nits for human review,
     each with `file:line` and rationale.
   - Whether the PR was marked ready.

1. **Move the issue to In Review and offer to add the PR
   to the merge queue.** Run this step **only** when the
   CI wait took the **all-checks-green** path and the
   review summary above has been printed — the PR is
   ready, CI is green, and the human has the full picture
   in front of them. (If CI failed, no checks ran, or the
   gate was never reached, skip this entirely.)

   **This handoff goes through `AskUserQuestion` —
   unconditionally, never silently.** It is a skill-to-skill
   handoff (per `CLAUDE.md` → "The PR workflow and skill
   handoffs"), so **both** actions it performs are gated on
   the TUI selector, not a free-text question and not an
   unprompted state change: (a) the **In Review** move and
   (b) the **enqueue** offer are one gated handoff — the
   `AskUserQuestion` below is what authorizes advancing the
   issue and enqueueing. Do **not** advance the issue to In
   Review and then enqueue without surfacing the selector,
   and do **not** substitute a plain-text "shall I enqueue?"
   for it. The recommended default ("yes, add it to the merge
   queue") goes **first** in the options. The only paths that
   skip the selector are the non-happy ones spelled out below
   (a `CONFLICTING`/`UNKNOWN` mergeability re-check, or no
   resolvable tag).

   **First, re-check mergeability — a PR that was
   `MERGEABLE` at the ready gate can turn `CONFLICTING`
   while CI ran, if the base advanced.** So before moving the
   issue and before the prompt, re-read the conflict signal
   (the same read the ready gate used):

   ```sh
   gh pr view <number> --json mergeable,mergeStateStatus
   ```

   - `mergeable: "CONFLICTING"`
     (or `mergeStateStatus: "DIRTY"`) → **do not** offer to
     enqueue and **do not** advance the issue. Report the
     conflict, tell the human to rebase onto `origin/<base>`
     to resolve it (this skill does not auto-resolve — see
     step 2 for why that holds even here), and leave
     the issue **In Progress**. Stop here — the enqueue offer
     is off the table until the rebase clears the conflict.
   - `mergeable: "UNKNOWN"` → GitHub hasn't finished
     computing mergeability; re-poll a few times (a short
     wait between reads) before deciding rather than
     offering blindly. If it stays `UNKNOWN`, say so and
     hold rather than enqueue.
   - `mergeable` not conflicting (`MERGEABLE`) → proceed to
     the In Review move and the enqueue prompt below.

   **If this branch adds a migration, check for a cross-branch
   number collision before enqueueing.** The in-tree ascend
   guard and the schema-fence tests only ever see **one** tree,
   and they start from an **empty** database — so a duplicate
   number is invisible until both files coexist, which first
   happens on the merge queue's merge-group branch. Until then
   both PRs ride green.

   The dequeue is the cheap half. The shared dev Postgres may
   already have one branch's number applied, and an applied
   migration is **immutable** (sqlx records a checksum), so
   renumbering the **wrong** branch wedges the shared database
   — and the only fixes are manual surgery or a wipe that
   destroys collected market data outside the venues' backfill
   windows. Two in-flight PRs really did each add an `0003`.

   One call — the tool does the open-PR read itself:

   ```sh
   python3 .claude/tools/migration_collisions.py --others-from-gh
   ```

   It compares **numbers, not filenames** — `0003_telemetry.sql`
   and `0003_roster.sql` collide — and exits non-zero on a
   collision, so a caller checking only the status cannot
   enqueue through one.

   **This step used to prescribe two commands with no way to
   connect them**, and every sanctioned route across the gap is
   closed: a `>` redirect is a compound the shell guard blocks,
   a pipe likewise, and capturing the listing to re-emit it
   with `Write` routes every open PR's file list through
   context — the ~4.0k-for-two-lines cost the tool exists to
   avoid. So the read moved inside the tool, which is how
   `review_diff.py --overlap` already solves the identical
   problem; the two steps should not disagree about it.
   `--others <file>.json` remains for a caller that already
   holds the inventory, and keeps the compare network-free for
   tests.

   **On a collision, stop and coordinate the renumber
   deliberately.** The tiebreak, and its direction is the whole
   point: **the branch whose number is already applied to the
   shared dev DB keeps it; the other renumbers.** Renumbering
   the applied one is the move that wedges the database.

   Skip this entirely when the branch adds no migration — the
   tool says so and exits zero.

   **A rebase-forced re-run may enqueue BEFORE re-verifying.**
   The normal gate is "everything green locally, then
   enqueue". That gate cannot win a race it is structurally
   losing: `main` lands roughly **every 20 minutes** and a full
   local re-verify costs about the same, so a base that moves
   during the review invalidates the pass you just finished,
   and the re-run invites the next move. One review lost **two
   full cycles** to exactly this — `main` landed 4 commits
   during the lens fan-out and 3 more during a 1289-second CI
   wait, and the second batch invalidated a completely green
   local suite *plus* that CI run.

   So when the **only** reason for a re-run is a rebase, and
   the base delta is already covered by the queue's own suite:
   **push, enqueue, and let the queue verify.** The merge queue
   re-runs the whole Rust suite on its own queue branch against
   the merged result — that is the actual gate, and it is
   fail-closed. Re-running it locally first proves nothing the
   queue will not prove, on a tree that is already stale.

   Three conditions, all required, so this cannot become a
   general license to skip verification:

   - the branch's own diff is **unchanged** since the last
     green local pass (no new fix commits — only the rebase);
   - `rebase_overlap.py` reports **no overlap** between the
     base delta and this branch's files; and
   - the queue's suite covers the base delta —
     `runs_rust_suites` on that same report.

   If any of those fails, verify locally first as usual. And
   say in the summary that the local re-verify was skipped and
   why, so the queue result is read as the gate it is.

   (This is the standing "enqueue early, verify in parallel"
   principle, which was recorded as a lesson but never wired
   into this gate.)

   This prompt is the handoff: per `CLAUDE.md`, **In
   Review** means "okay for the human to look at the PR
   and approve enqueueing it." So move the Linear issue
   (the tag resolved in step 3) to **In Review** here —
   with, or just before, the prompt — not earlier. Skip
   only if no tag was resolvable:

   This is a **state-only** write, so it goes through the
   zero-echo path rather than the MCP:

   ```sh
   python3 .claude/tools/board_batch.py state \
     --id <ENG-###> --state "In Review"
   ```

   It prints one line. The MCP equivalent echoes the entire
   stored body back to transmit one enum — measured at ≈3.6k
   on a consolidated issue, and this is the write that pays it
   at its worst, since the body is at its largest by the
   handoff. Unlike `init-pr`'s bootstrap write, nothing here
   needs the body: the session has held it since step 3, so
   the echo buys nothing at all.

   **One write, no retry loop.** If the response echo comes
   back still showing In Progress, do **not** re-issue the
   write chasing it (that loop cost five body-echoing
   `save_issue`/`get_issue` round-trips on PR #207) — verify
   once and, if it still disagrees, report the discrepancy
   and move on. The transition is idempotent; a silent echo
   is not worth another full-body round-trip.

   Then ask with `AskUserQuestion` — always this tool, so
   the human gets the little TUI pop-up selector and picks
   "yes, add it to the merge queue" (or "skip, I'll merge
   by hand") right in the terminal instead of typing a
   reply. In the question text, **clearly print both
   identifiers the human needs to pull up the PR**: the
   Linear tag (e.g. `ENG-536`) and the GitHub PR number
   (e.g. `#138`) — so it's unambiguous which PR they're
   approving.

   - **If the user approves**, add it to the merge queue.
     Enqueueing is the one `gh` **write** the skill makes
     (the dequeue probe in the final step is the one `gh`
     **read**): the MCP server exposes no auto-merge /
     merge-queue tool (`merge_pull_request` does an
     *immediate* merge, which bypasses the queue), so use
     `gh pr merge` with `--auto`, which enables "Merge
     when ready" / enqueues behind the required checks:

     ```sh
     gh pr merge <number> --auto
     ```

     **Pass no merge-strategy flag** (no `--squash` /
     `--merge` / `--rebase`). This repo is governed by a
     GitHub **merge queue**, which sets the strategy itself;
     an explicit `--squash` conflicts with it and `gh` warns
     that the merge strategy for `main` is set by the merge
     queue. The enqueue still takes (exit 0), but the flag is
     pure noise — omit it and let the queue decide.

     **Confirm the enqueue from the `gh` exit, not from a
     polled field.** A zero exit means "Merge when ready"
     is now enabled — the enqueue took. (The hosted GitHub
     MCP's `pull_request_read` `get` response does **not**
     carry `auto_merge`, so there is no MCP field to poll
     for the enqueue; the `gh` exit is the signal.) Report
     the enqueue and move on — the queue *outcome* (landed
     vs. taken out) lands asynchronously and is surfaced by
     the final step, after `firm-perms`; do **not** block
     here waiting for the merge.

     **And expect no output at all.** Under this harness
     `gh pr merge --auto` prints **nothing** — stdout is not a
     TTY, so the "Merge when ready" confirmation `gh` shows
     interactively never appears. The exit status is genuinely
     the *whole* signal: there is no reply text to read back,
     and one run read the silence as a failed enqueue. If you
     want positive confirmation beyond exit 0, take it from a
     `mergeQueueEntry` probe after the settle delay — not from
     anything the command said.

   - **If the user declines**, leave the PR ready and the
     issue In Review, and note that they can merge it (or
     enable "Merge when ready") themselves.

1. **Capture session metrics** (while the PR sits in the
   queue). A review run is long and tool-heavy, so it is the
   natural moment to account for where its tokens went and
   bank trim recommendations for the skill suite. Run it
   **after** the enqueue, **not** gated on the merge landing
   — the merge resolves asynchronously in the queue, so this
   is productive work to do while it does — and regardless of
   the merge outcome (it analyzes the session, not the PR).
   It runs **before** `firm-perms` by design: `/session-metrics`
   itself triggers command approvals, and the `firm-perms`
   **sweep** that follows should harvest them, so metrics comes
   first and firm-perms is the **last interactive step**.

   **Ask first, via `AskUserQuestion` — and ask for BOTH
   closing gates in that one call.** This is a skill-to-skill
   handoff, so gate it on the same TUI selector the
   merge-queue prompt uses (per `CLAUDE.md` → "The PR workflow
   and skill handoffs"): one call carrying two questions —
   whether to capture session metrics now, offering "yes, run
   /session-metrics" (**first**, the recommended default) and
   "skip"; and the `firm-perms` question the next step
   describes, with its own options.

   This is the same argument the entry gate already makes for
   asking tier and spawn together, and it applies more
   strongly here: both gates are unconditional, both are pure
   run-this-closing-step authorizations, neither answer
   changes what the other does, and by this point the user has
   already been prompted twice.

   **The ordering objection does not apply.** The next step
   requires `firm-perms` to run *after* `/session-metrics` so
   its sweep harvests that run's approvals — but that
   constrains **execution** order, not **question** order.
   Asking both up front and then running them in the
   prescribed sequence preserves the property exactly, which
   one session confirmed in practice: one interaction, both
   approved, the sweep still ran last and still saw the
   metrics run's approvals, for ≈60 tokens.

   The real saving is one fewer blocking round trip at the
   point the user is most likely to have walked away — the
   merge resolves asynchronously and actively invites other
   work, and a prompt firing into an absent user is the
   failure the next step warns about. Halving the tail prompts
   halves that exposure. Keep both decline paths and both
   reporting requirements exactly as they are; only the prompt
   is merged.

   - On **decline**, skip this step and note in the report
     that session metrics were **not** captured this run.
   - On **approve**, invoke `/session-metrics`. It derives
     this session's id from the scratchpad path, runs the
     `session_metrics.py` tool to rank the run's token sinks
     and hardening candidates (the transcript is read in the
     tool's own process, so it never enters context), then
     files each trim lever it identifies as its own **parked
     issue** under the `Trim levers` milestone, through the
     zero-echo `trim_levers.py` writer — appending this
     session's evidence to a lever that already exists rather
     than duplicating it. `trim-context` folds those later. It
     authors no source edit, so it's safe to run regardless of
     the gate or CI outcome. If the `Trim levers` milestone
     does not exist, the writer says so by name — note that in
     the report.

   **Ground the recommendations in this run.** As the review
   progressed you may have noticed wasteful payloads (a
   whole-file Read, a verbose build log, a repeated full PR
   read, an inlined-diff fan-out). Per
   `docs/conventions/context-economy.md` → "Track consumption
   ideas as you go", carry those observations into
   `/session-metrics` so its prose names concrete levers, not
   just the tool's raw sink ranking.

1. **Firm up the permission allowlist** — the **last
   interactive step**, so it sees the whole run's approvals.
   A review run approves a lot of one-off commands (the
   diff-review and cross-check agents in steps 5–6, the
   enqueue, and the `session-metrics` step just above all
   pile them up), so the natural moment to generalize them
   is *after* all of them, while the user is still present to
   confirm. Run this **after** `session-metrics` and
   **before** the async merge-queue outcome watch below —
   **not** gated on the merge landing (it resolves
   asynchronously), and crucially **not** deferred past it:
   the merge can land minutes later via a scheduled wakeup,
   and a propose-then-confirm gate firing then would prompt
   an absent user.

   **This step's question was already asked** — it rides the
   previous step's batched `AskUserQuestion` as its second
   question ("yes, run /firm-perms sweep", **first** and
   recommended, versus "skip"), because both closing gates are
   unconditional and independent and the user has been
   prompted twice already. Do **not** prompt again here; act
   on the answer already given. Only the prompt was merged —
   the execution order is unchanged, and this step still runs
   **after** `/session-metrics` so the sweep harvests that
   run's approvals.

   - On **decline**, skip this step and note in the
     report that permissions were **not** firmed this run.

   - On **approve**, run the **sweep** — `/firm-perms sweep`.
     Because this step now sits at the tail of the
     interactive sequence, the whole-session harvest is the
     right default — **not** the single-approval `firm_last`
     fast-firm, which made sense only when this step ran
     mid-run. The sweep collects every approval this run made
     (sub-agent and `session-metrics` commands included),
     generalizes and dedupes them, and writes the result to
     the one shared `settings.local.json` at the main
     checkout behind its propose-then-confirm gate — live in
     every worktree at once. Relay what it firmed.

     Watch for what it reports as **unfirmable**: a
     `find / … | head`, a `sed … | grep`, or a heredoc is
     malformed, not missing a glob, and when an agent emitted
     it that's a signal the **step-5 reviewer brief leaked** —
     tighten the brief so the pattern stops recurring, rather
     than allow-listing it.

   **A source edit this step produces cannot land on this
   branch — route it to the batch issue.** `firm-perms` may
   conclude that a pattern traces to a committed skill,
   script, or Makefile target and belongs fixed at the source
   rather than allow-listed. By the time this step runs the
   branch is pushed and usually enqueued, so such an edit has
   nowhere to go: committing it means a second PR, and
   leaving it in the worktree loses it when the worktree is
   pruned. Not hypothetical — a nine-line fix was stranded
   exactly this way and survived only because someone ran
   `git status` before deleting the worktree.

   So **write the edit verbatim into the open `Claude:` batch
   issue** — the standing accumulator for agent-infra work,
   found as the open Backlog issue whose title carries the
   `Claude:` prefix — never into the merged branch, and never
   left dirty in the worktree. Then **say so out loud in the
   report**, naming the issue, so the handoff is visible
   rather than assumed. The step ordering stays as it is:
   moving the source-edit half before the ready gate would
   miss exactly the approvals granted during the CI wait,
   which are most of them.

   **The one residual gap.** The `gh api graphql` merge-queue
   probe in the outcome-watch step below runs *after* this
   sweep — unattended, during the queue wait — so its
   approval falls outside the harvest. Rather than chase it
   every run, **pre-firm that one fixed probe shape** once:
   confirm the `Bash(gh api graphql:*)` allow-rule the probe
   reduces to is present (the sweep will propose it if this
   run approved it), so the probe never prompts while the
   user is away.

1. **Surface the merge-queue outcome** (separately). Run
   this **only** if the user approved the enqueue (skip it
   if they declined — there's nothing queued to watch). The
   merge lands asynchronously, so this is its own note,
   printed after `firm-perms` and after the review summary
   above — the summary couldn't know this outcome yet.

   **The Linear status is settled at enqueue.** After enqueue
   the PR sits in the queue asynchronously
   (`mergeQueueEntry.state: AWAITING_CHECKS`), and the
   Linear/GitHub integration makes **no state transition on
   merge** — that is a team setting, not a skill behavior. So
   the **In Review** state written at the enqueue handoff is
   the state the issue holds through the merge and after it,
   and it is accurate to read or report at any point.

   **There is deliberately no post-merge write here.** An
   earlier revision of this step observed the integration's
   auto-**Done** and wrote the issue *back* to In Review,
   because merge and completion are different events and an
   auto-Done hides exactly the sessions that still owe
   follow-up — session metrics, perms firming, post-merge
   tidy, and feedback that has to reach a planning session.
   The team setting now states that convention natively, so
   the write-back was retired: it spent a full-body Linear
   echo in every implementation session to restore a state
   that nothing had moved.

   Two consequences follow, and both remove machinery rather
   than add it. Nothing races this step, so no ordering
   against an auto-transition is needed. And the residual gap
   that revision accepted — a session dying before the
   write-back ran, leaving the issue auto-Done with nobody to
   re-mark it — is gone, because nothing sets Done at all now.

   Then run the existing tail — post-merge tidy, session
   metrics, `firm-perms` — and close with an explicit
   **follow-up ledger**: every operator-facing item either
   addressed, handed to a planning session, or filed as a task.
   Name them.

   **Only an explicit approval moves it to Done.** The terminal
   step is an `AskUserQuestion` — *"all follow-up complete:
   metrics filed, perms firmed, feedback forwarded — mark
   Done?"* — and nothing else sets that state.

   Watch whether the PR lands or gets kicked back out with a
   **single** probe per check: the `gh api graphql` dequeue
   probe below already selects `state` and `merged` **and**
   the merge-queue fields, so it answers "landed?", "still
   queued?", and "dequeued?" in one read — the old
   `pull_request_read` `get` poll that used to run first was
   redundant (it carried the full PR object every poll just
   to read `state`/`merged`) and is dropped (per `CLAUDE.md`
   → "Context economy").

   **Prefer blocking on the queue's own run over re-probing.**
   Once `mergeQueueEntry` names the `gh-readonly-queue/…`
   branch's check run, one command waits it out — the same
   tool as the CI wait, in its run mode:

   ```sh
   python3 .claude/tools/wait_for_checks.py --run <run-id>
   ```

   ```json
   {
     "run_id": "1234567890",
     "conclusion": "pass",     // pass | fail | timeout
     "settled": true,
     "elapsed_seconds": 214,
     "exit_code": 0,
     "log_path": "…/wait-for-run-1234567890.log"
   }
   ```

   gh paces it and exits when the run settles, so nothing is
   polled, and its `--exit-status` makes a failed queue run
   non-zero — a dequeue can't read as a merge. Then issue the
   graphql probe **once** to read the terminal state.

   **Don't shell `gh run watch` directly.** Like
   `gh pr checks --watch`, it is verbose-by-refresh: it
   re-prints the **whole job tree** on every refresh into one
   tool result. One bare call emitted **64.6KB**, overflowed
   the tool-result cap, was persisted to disk, and the
   terminal state still had to be re-probed by graphql
   afterwards — that session's largest single result,
   effectively fetched twice. The tool routes the tree to
   `log_path` and hands back the verdict; `Read` the log by
   slice only if the run actually failed.

   Fall back to re-issuing the graphql probe as a fresh tool
   call across turns when there is no run id to watch — the
   registration window below, or a queue entry that never
   produced a run. Either way: **never** a shell
   `while … sleep … done` loop or a `jq` filter. Say once,
   up front, that the PR is queued and you're standing by;
   then stay silent until a **terminal** outcome (merged,
   or taken out of the queue), pinging the human only then.
   Every probe is resumable — a fresh call returns the current
   snapshot.

   **Give the queue entry a settle window: the first probe
   waits.** GitHub does not register the merge-queue entry
   synchronously with the enqueue, so a probe fired the
   instant a **successful** `gh pr merge --auto` returns
   (exit 0) reports:

   ```json
   {
     "state": "OPEN",
     "merged": false,
     "mergeQueueEntry": null,
     "autoMergeRequest": null
   }
   ```

   — all-null on a PR that isn't registered **yet**; a
   re-probe seconds later returns
   `mergeQueueEntry: {"state": "QUEUED"}`. So don't fire the
   *first* probe immediately — there is no run id to watch yet,
   so this is the fallback case: let a turn pass before probing.
   Exit 0 from `gh pr merge --auto` confirms only that
   auto-merge was **enabled**, not that the queue entry exists.

   This is the **timing twin** of the `autoMergeRequest`
   false positive described next. That one was fixed by
   keying on `mergeQueueEntry` instead — but inside the
   registration window *both* fields are null, so the
   "both null" test does **not** save you here. Never treat
   the first post-enqueue probe as authoritative.

   This is the one `gh` **read** the skill makes (mirror of
   the enqueue write). The signal that distinguishes "still
   queued" from "silently removed" is the PR's
   **`mergeQueueEntry`**: it is non-null exactly while the PR
   sits in the merge queue and flips to `null` the moment it
   leaves. **Do not** key on `autoMergeRequest` for that — on
   a merge-queue repo a genuinely-queued PR reports
   `autoMergeRequest: null` (and a `CLEAN` `mergeStateStatus`,
   not `QUEUED`), so the old `autoMergeRequest`-null test was
   a **false positive** that announced "taken out of the
   queue" on every run. `mergeQueueEntry` isn't exposed by
   the MCP `get` (nor by `gh pr view --json`), so query it
   over GraphQL, where the same query also returns `state`,
   `merged`, and `autoMergeRequest` (the last keeps the
   **classic-auto-merge** path — repos with no merge queue —
   working).

   Keep the command globbable: the query body has braces and
   quotes that trip the brace-with-quote guard, so write it
   to a file with the **Write** tool rather than inlining it,
   then pass the PR number as a typed variable and the file
   on a stable command line (per the file-handoff rule in
   `CLAUDE.md`). The query (write it to e.g.
   `/tmp/mq-probe.graphql`):

   ```graphql
   query($number: Int!) {
     repository(owner: "DASMAC-com", name: "dropset") {
       pullRequest(number: $number) {
         state
         merged
         mergeQueueEntry { state }
         autoMergeRequest { enabledAt }
       }
     }
   }
   ```

   ```sh
   gh api graphql -F number=<number> -F query=@/tmp/mq-probe.graphql
   ```

   This reduces to a `Bash(gh api graphql:*)` allow-rule —
   only `<number>` varies; the brace-heavy query rides in the
   file, not the command line. Branch on the single result:

   - `merged: true` (or `state: "MERGED"` / `"CLOSED"`) → it
     landed; report the merge (the Linear/GitHub integration
     will have moved the issue to **Done** on this signal —
     report that it did, don't hand-move it). Key on `merged`
     / `state`.
     Then **mark this PR's own GitHub notification done** so
     it doesn't linger (the immediate companion to
     `housekeeping`'s merged-PR notification sweep): find the
     thread whose `subject.url` ends in this PR's number and
     dismiss it — never `mark_all_notifications_read`.

     **Read the thread id with a field-selected `gh`, not the
     full-object MCP call.** This lookup needs exactly one
     value, and `mcp__github__list_notifications` returned
     **3.7k tokens in a single call** for it — the 6th-largest
     result of one session — because it embeds the complete
     repository object (every `*_url` template) once per
     notification, repeated for each of three. That is the
     same reasoning this skill already applies to the
     field-selected `gh pr view` and `gh pr checks` reads,
     and the notification lookup
     was simply missed when that carve-out was written, so it
     is a **documented `gh` exception** (per
     `docs/conventions/github-mcp.md`) on the same grounds:

     ```sh
     gh api /notifications --jq '.[] | {id, url: .subject.url}'
     ```

     Then dismiss over the MCP, which is a small write:

     ```txt
     mcp__github__dismiss_notification(
       threadID: "<this PR's notification id>",
       state: "done",
     )
     ```

     Whichever transport you use, the payload is a
     **fixed cost, read once**: never re-fetch the list to
     re-find the id.

     `state: "done"`, **not** `"read"` — `"read"` only clears
     the unread marker and the thread stays in the GitHub
     inbox, which is exactly the lingering-notification
     complaint this step exists to answer. A merged PR has
     nothing left to come back to, so `"done"` is the right
     terminal state. Doing it here rather than deferring to
     the next `housekeeping` pass is the point: otherwise the
     flow finishes and the notification sits until morning.

     If no notification matches (already cleared), skip it.

     Finally, **reclaim this worktree's disk**. Now the PR
     has landed, its worktree is dead weight until
     `housekeeping` prunes it — and if that prune is delayed
     (or the tree is left dirty and skipped), its multi-GB
     Rust `target/` and pnpm `node_modules` sit around. Nuke
     them with the `clean` target, which reduces to a
     `Bash(make clean:*)` allow-rule:

     ```sh
     make clean
     ```

     Run this **only** on a confirmed merge — a still-queued
     or dequeued PR keeps its build tree for a possible
     re-run.

   - `state: "OPEN"` with `mergeQueueEntry` non-null (or, on
     a classic-auto-merge repo, `autoMergeRequest` non-null)
     → still queued; keep polling.

   - `state: "OPEN"` with both `mergeQueueEntry` **and**
     `autoMergeRequest` null → **not conclusive on its own.**
     Per the settle window above, this is also what the
     registration gap looks like: the PR that was never
     registered and the PR that was registered and kicked out
     read *identically* on this probe.

     **First rule out that it already MERGED, because this
     probe cannot be trusted to notice.** GraphQL lags the
     merge. A measured run got an open, unmerged, all-null
     answer from GraphQL while a `gh pr view` state read *at
     the same moment* returned `MERGED`.

     Every all-null test below — the `gh run list`
     evidence, the earlier-non-null rule, the two-consecutive
     rule — reads that as a removal, so following them
     literally concludes "kicked out of the queue" on a PR
     that merged, and sends the next session diagnosing a CI
     failure that does not exist.

     One read settles it, and it is already inside the `gh`
     carve-out for polled PR-state reads
     (`docs/conventions/github-mcp.md`):

     ```sh
     gh pr view <number> --json state,mergedAt
     ```

     `state: "MERGED"` (or a non-null `mergedAt`) → it
     merged; take the merged path and stop. Only once this
     says otherwise do the removal tests below mean anything.

     **Resolve the remaining ambiguity with one `gh run list`
     — before concluding anything.** The queue branch is the evidence:
     if a run set for `pr-<number>-` exists, the entry
     demonstrably existed, so an all-null probe now means it
     **left**. If no such run set exists, the PR was never
     registered and this is the window. One read settles it,
     where waiting for a second probe only settles it slowly:

     ```sh
     gh run list --limit 20 --json headBranch,name,status,conclusion,url
     ```

     Pick the entry whose `headBranch` carries the
     `pr-<number>-` prefix. A bare `gh run list` reduces to a
     `Bash(gh run list:*)` allow-rule, so this costs one
     allow-listed call.

     This call used to sit **after** the branch below, as part
     of diagnosing a removal already concluded — which meant a
     run that read all-null as "not registered yet" never
     reached it. On PR #311 that first probe *was* a terminal
     dequeue: the PR had enqueued normally, the queue branch's
     full run set had already completed, and `SDK` failed
     transiently and kicked it out — while all 8 checks on the
     PR itself stayed green, exactly as the note below warns.
     The cost of concluding without this read was a blind
     re-enqueue on a wrong diagnosis. It happened to be the
     right action, because the failure was transient
     infrastructure — but that was luck, not reasoning.

     Failing that evidence, fall back to the slower tests:
     treat it as terminal only when an **earlier** probe in
     this watch already returned a non-null `mergeQueueEntry`,
     or when **two consecutive** all-null probes, spaced by the
     pacing delay, have returned. Until then it is the
     registration window: keep polling, and stay silent.

     **Diagnose the removal from the queue branch's CI, not
     from the PR's checks.** This is why the run list above is
     the right evidence in the first place. The queue does not
     re-run the PR branch's checks — it builds a temporary
     branch (`gh-readonly-queue/<base>/pr-<number>-<sha>`,
     i.e. `main` with this PR merged into it) and runs CI
     *there*. A failure on that branch dequeues the PR while
     leaving **every** check on the PR itself green, so
     `gh pr checks <number>` reports "nothing failed" on a PR
     that was demonstrably kicked out.

     Key on **job** conclusions, not the parent run's
     `status`: a run can still read `in_progress` while one of
     its jobs has already failed and triggered the dequeue.
     Once the failing job is identified, pull it with the
     same `get_job_logs` call as the CI-wait failure path
     (`failed_only: true`, `tail_lines: 100`).

     Then split the response by **what** failed — the two
     causes want opposite handling:

     - **Transient infrastructure** — a toolchain, network,
       or cache failure **before any test executed** (e.g.
       `Cache not found for input keys: toolchain-solana-…`
       followed by `Failed to install platform-tools`).
       Nothing is wrong with the code. Re-enqueue, and say
       that's what you're doing and why.
     - **A real `main`-integration conflict** — a test
       assertion failure, compile error, or lint violation on
       the queue branch. This is exactly what the queue
       exists to catch, and it means the PR conflicts
       semantically with code that landed after its last
       rebase. Do **not** re-enqueue. Catalogue it as
       blocking, re-draft the PR, and tell the user to rebase
       on `main` and re-run `/review-pr`.

     Report the removal either way, naming the queue-branch
     job that caused it.

   **Once this step resolves, the run is not over.** Each
   remaining closing step is already gated on its own
   `AskUserQuestion` — the prompts are not missing. What is
   missing is anything ensuring the tail runs **at all** when
   the flow diverts after the enqueue, which is common: the
   merge resolves asynchronously, so the wait actively
   invites other work. One session enqueued, then spent many
   turns in a three-party negotiation about Linear blocking
   edges (the user plus a peer planning session) and never
   came back; the user had to ask for the closing steps
   explicitly. Every individual step behaved correctly. The
   sequence just ended early, and silently.

   So restate the remainder as a checklist and work it:

   - [ ] **Session metrics** captured (the step above).
   - [ ] **`firm-perms`** run — the last interactive step.
   - [ ] **Post-merge tidy**, on a merge: the notification
     dismissed `state: "done"`, and `make clean` run.
   - [ ] **Working tree clean** — `git status --short` is
     empty. Anything still modified is work the merge did not
     carry, and the worktree is about to be pruned. Surface
     it and decide where it goes; never report the run
     complete with a dirty tree.

   **That last item is detection, and it goes last on
   purpose.** The step above names one *cause* of a late edit
   (a `firm-perms` source edit produced after the push), but
   a stray change can come from anywhere — a fix made during
   the session-metrics step, a partially applied edit, a
   scratch file written into the repo instead of the
   scratchpad. Nothing else at the end of a review looks at
   tree state: `git status` is read at step 2 and never
   again, so the blast radius is total and the evidence is
   deleted with the worktree. It runs after the post-merge
   tidy because both `make clean` and `firm-perms` can
   themselves touch the tree.

   **A diversion does not discharge them.** Another skill, a
   message from a peer session, a fresh user request, or a
   long queue wait — none of those close the review. Come
   back and finish the checklist, or say explicitly which
   item you are skipping and why. An unanswered
   `AskUserQuestion` is a deferral the user chose; an
   unasked one is a step that was dropped.
