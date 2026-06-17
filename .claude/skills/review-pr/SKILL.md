---
name: review-pr
description: Adversarial pre-review — verify the Linear task's checklist is fully addressed, lint, catalogue issues, fix what's mechanical, ready the PR, and wait for GitHub CI to pass before marking the Linear issue In Review so the human can merge with nothing left to check.
user-invocable: true
---

<!-- cspell:word oneline -->

<!-- cspell:word unstarted -->

# `review-pr`

Act as an adversarial reviewer before the human
looks at the PR. Run lint, audit the diff,
catalogue every issue, fix what can be fixed
mechanically, and mark the PR ready only when
it's clean. Then wait for the real GitHub CI to go
green before moving the Linear issue to In Review and
reporting done — so when this skill finishes, the
human can merge (or let "Merge when ready" land it)
with nothing left to check.

Run this after autonomous work is complete and
all changes are committed and pushed.

## Steps

1. **Locate the PR.** Identify the current branch,
   then look up its PR — run the branch listing on
   its own and pass the name to `gh pr view`
   literally (no command substitution, so the call
   reduces to a stable allow-rule):

   ```sh
   git branch --show-current
   ```

   ```sh
   gh pr view <branch> --json number,title,state,isDraft
   ```

   If no PR exists, stop and tell the user to
   run `/init-pr` first.

1. **Clean tree, then rebase onto `main`.** First run
   `git status` — if there are uncommitted changes,
   stop and tell the user to commit first (or run
   `/commit-changes`). Then rebase onto the latest
   `main` so the review runs on the state the branch
   will actually merge as, instead of a base that has
   drifted while the work was in flight — this is what
   minimizes file conflicts at merge time:

   ```sh
   git fetch origin main
   git rebase origin/main
   ```

   - If the rebase **conflicts**, abort it
     (`git rebase --abort`), catalogue the conflict as
     a **blocking** issue (step 7), and tell the user
     to rebase and resolve manually, then re-run — this
     skill does not auto-resolve conflicts.
   - If it **succeeds but integrated new commits from
     `main`**, the diff now reflects that integration.
     A clean *textual* rebase can still leave a
     *semantic* conflict (main renamed or changed
     something this branch still calls), so flag those
     for the adversarial review (step 5) and the test
     run (step 10) to catch. The rebase rewrote history,
     so the branch must be force-pushed — step 11 does
     this with `--force-with-lease`.

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

   - If the issue is still **unstarted** (Todo /
     Backlog), move it to **In Progress** so the board
     reflects that work is underway. Don't regress an
     issue already In Review / Done:

     ```txt
     mcp__claude_ai_Linear__save_issue(
       id: "<ENG-###>",
       state: "In Progress"
     )
     ```

   - Extract every actionable requirement: markdown
     checkboxes (`- [ ]` open, `- [x]` already done),
     plus any acceptance-criteria or scope bullets
     phrased as requirements even if not checkbox
     syntax. Treat an already-`[x]`-checked box as a
     claim to verify, not a given — confirm the diff
     actually contains it.

   - For each requirement, decide from the diff
     (`git diff main..HEAD`) and the branch's commits
     whether it is **addressed**, **partial**, or
     **missing**. A requirement that is out of scope
     for this PR by design (e.g. explicitly deferred,
     or split into a follow-up issue) counts as
     addressed *only if* the deferral is recorded —
     a commit, a PR-body note, or a linked follow-up
     filed via `/linear-task`. Silent omission is
     **missing**.

   - **Tick the addressed items.** For every
     requirement the diff genuinely delivers, check its
     box (`- [ ]` → `- [x]`) and write the updated
     description back with `save_issue` (id + the full
     edited `description`). Leave **partial** and
     **missing** boxes unchecked, and don't invent
     boxes for non-checkbox requirements. Diff against
     the live body you just fetched so you never clobber
     a box the author already ticked or other edits made
     since.

   - Catalogue every **partial** or **missing**
     requirement as a **blocking** issue (step 7),
     quoting the checklist text and the `file:line`
     (or absence) that decides it.

1. **Run lint:**

   ```sh
   make lint
   ```

   If lint fails, first separate **environmental**
   failures from **real violations** — they're not the
   same problem:

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

   - For genuine violations, parse the output and fix
     every issue that can be fixed mechanically
     (formatting, import order, trailing whitespace,
     spelling, etc.).

   - Stage the fixes by explicit path and commit as a
     single signed commit:

     ```sh
     git add <fixed files...>
     git commit -S -m "Fix lint violations"
     ```

   - Re-run `make lint` to confirm it passes.

   - If real violations still fail after the fix
     attempt, catalogue the remaining failures as
     **blocking** issues (step 7) and do **not** mark
     the PR ready.

1. **Adversarial diff review.** Get the full diff:

   ```sh
   git diff main..HEAD
   git log main..HEAD --oneline
   ```

   **Brief every sub-agent on the shell rules.**
   Sub-agents don't inherit this project's `CLAUDE.md`
   shell conventions, so left to themselves they reach
   for `find / …`, `sed -n '…p' … | grep`, `cat`, and
   other compounds that can't reduce to an allow-rule
   and re-prompt on **every** run — the exact churn
   this skill exists to avoid. Prepend this standing
   brief to **each** Agent prompt (the review agents
   here *and* the cross-check agent in step 6):

   > - You are a **read-only reviewer**. The full diff
   >   and commit log are included below — review from
   >   them; you rarely need a shell at all.
   > - To inspect repo files, use the **Read / Grep /
   >   Glob** tools — never `cat` / `head` / `tail` /
   >   `sed` / `awk` / `find` / `grep` in Bash.
   > - Stay **inside the repo**. Never search the
   >   filesystem (`find / …`, `~/.cargo`,
   >   `~/.claude/projects/…`) or slice transcript /
   >   tool-result files; toolchain and dependency
   >   sources are out of scope for a diff review. If
   >   you genuinely need a library's source, say so in
   >   your findings — don't scan for it.
   > - **One bare command per Bash call** — no pipes,
   >   `&&`, `;`, command substitution `$(…)`,
   >   redirects, or heredocs. Each call must reduce to
   >   a `prefix:*` allow-rule.

   Pass the `git diff main..HEAD` and `git log` output
   you already collected **inline** in each prompt, so
   no agent re-fetches them by shelling out.

   Spawn parallel sub-agents via the `Agent` tool
   (single message, multiple calls) to review the
   diff — each with the brief above prepended. At
   minimum:

   - **Correctness** — logic errors, off-by-ones,
     unhandled edge cases, incorrect assumptions,
     broken invariants.
   - **Security** — injection, unchecked input,
     missing validation, unsafe operations,
     secrets in code.
   - **Style & consistency** — naming, patterns,
     idioms that diverge from the rest of the
     codebase.
   - **Completeness** — missing tests, TODO/FIXME
     left behind, partial implementations,
     unused imports or dead code introduced by
     the diff.
   - **`CLAUDE.md` freshness** — does anything in
     the project's `CLAUDE.md` still match reality
     after this diff? Read `CLAUDE.md` and check its
     rules, command examples, paths, and tooling
     references against the current codebase and the
     diff. Flag guidance the diff outdates (a
     command, path, target, or convention it renames,
     moves, or removes) and any rule that has silently
     gone stale. Treat a rule the diff **directly
     violates or invalidates** as **blocking**;
     merely-stale prose as a **warning** with the
     suggested correction.

   Each sub-agent must return findings with file
   path, line number, severity (**blocking** /
   **warning** / **nit**), and a one-line
   rationale.

1. **Adversarial cross-check.** Spawn a fresh
   sub-agent that receives the collected findings
   and the diff (prepend the step-5 reviewer brief to
   its prompt too, and pass the diff inline), and is
   told to act adversarially:

   - Challenge weak or speculative findings.
     Flag false positives.
   - Identify issues the first pass missed.
   - Push back on rationale that doesn't hold up.

   If the cross-check produces material
   disagreements, iterate: re-spawn the relevant
   topic agent with the challenge and have it
   defend or retract. Iterate at most 2 additional
   rounds, then accept the surviving findings.

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

1. **Re-lint after fixes.** If any fix commits
   were made in the previous step, re-run
   `make lint` to catch violations introduced by
   those fixes. Apply the same fix-and-retry
   logic as the lint step (step 4).

1. **Regenerate committed generated artifacts
   (mirror the IDL / SDK / vectors CI gates).** Three
   workflows fail the PR if a committed generated file is
   stale relative to its source: `test.yml` regenerates
   the **IDL**, and `sdk.yml` regenerates the **SDK
   clients** and the **conformance vectors** — each via a
   `git diff --exit-code` that fails on a dirty tree. The
   author's diff (or a fix from step 8) may have changed
   the program without regenerating these, so refresh them
   here and commit any diff; otherwise the ready PR fails
   CI on a stale artifact. Regenerate **in dependency
   order** — the SDK is generated from the IDL, which is
   built from the program:

   - **IDL** (needs the Solana/Anchor toolchain):

     ```sh
     make idl
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
     make sdk
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
     make check-conformance-vectors
     ```

     That target regenerates the price/quoting vectors,
     stages `sdk/conformance/`, then
     `git diff --cached --exit-code`s it — so a **non-zero
     exit means the vectors were stale and are now
     staged**. Commit them:

     ```sh
     git commit -S -m "Regenerate conformance vectors"
     ```

   If any artifact commit was made, re-run `make lint`
   (a regenerated-file commit can still trip whitespace /
   EOF hooks), applying the step-4 fix-and-retry logic.

1. **Run the test suite (mirror CI).** The `Tests`
   workflow runs `make test` and
   `make test-no-teardown`; run both locally so the
   green checks GitHub needs for auto-merge are
   already verified here:

   ```sh
   make test
   make test-no-teardown
   ```

   - Both depend on the Solana/Anchor toolchain via
     `check-toolchain`. If the toolchain is absent
     and a target aborts before any test runs, you
     **cannot** verify that workflow locally — say
     so explicitly in the report (do not claim CI
     will pass), rather than counting it as green.
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

1. **Confirm the PR still merges cleanly.** Step 1
   already rebased onto `main`, so this is normally
   `MERGEABLE` — but `main` can advance again during a
   long review, so confirm rather than assume. Fetch
   the latest base and ask GitHub:

   ```sh
   git fetch origin main
   gh pr view <number> --json mergeable,mergeStateStatus
   ```

   - `mergeable: "MERGEABLE"` → no conflicts;
     proceed to the gate.
   - `mergeable: "CONFLICTING"` → the PR has merge
     conflicts. Catalogue this as a **blocking**
     issue and do **not** mark the PR ready. Tell
     the user to rebase onto `main` and resolve the
     conflicts (this skill does not auto-resolve
     them), then re-run `/review-pr`.
   - `mergeable: "UNKNOWN"` → GitHub has not finished
     computing mergeability yet. Wait a few seconds
     and re-run the `gh pr view` command until it
     resolves to `MERGEABLE` or `CONFLICTING`.

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
   reported as unverifiable locally), the title
   passes `Semantic PR`, and `mergeable` is
   `MERGEABLE`:

   ```sh
   gh pr ready <number>
   ```

   Marking it ready (out of draft) is what lets the
   human enable GitHub's **"Merge when ready"**
   auto-merge while the real CI finishes. But ready is
   **not** the end of this skill: the Linear issue does
   **not** move to In Review, and the run does **not**
   report success, until the actual CI is green — that's
   the next step.

   If any blocking issue remains — an unaddressed
   Linear checklist item, failing or unverified
   tests, a non-conforming title, or a merge
   conflict with `main` — do **not** mark the PR ready.
   Leave it in draft and the issue in its current state,
   **skip the CI wait below**, and report the blockers.

1. **Wait for GitHub CI to pass, then move the issue
   to In Review.** The local checks only *mirror* CI;
   the authoritative signal is the real run on the
   pushed commits — and when the toolchain was absent
   locally (tests / IDL reported unverifiable), CI is
   the *only* signal. This repo runs CI on the PR even
   while it was a draft (that's how `init-pr` warms the
   caches), so the checks are already in flight. Watch
   them to completion, polling on an interval so the
   output stays quiet:

   ```sh
   gh pr checks <number> --watch --interval 30
   ```

   `--watch` blocks until every check finishes and exits
   **non-zero if any failed**. Two operational notes:

   - CI can outlast a single Bash timeout. If the call
     is killed before the checks settle, just run it
     again — watching is resumable and idempotent, so
     re-invoke until it returns a final result.
   - If `gh` reports **no checks** on the branch, there
     is nothing to wait on — note that in the report and
     treat it as green rather than blocking forever.

   Then branch on the outcome:

   - **All checks green** → move the Linear issue (the
     tag resolved in step 3) to **In Review** so the
     board reflects it's awaiting human review — the
     final transition after step 3 set it In Progress
     and ticked the delivered items. Skip only if no tag
     was resolvable:

     ```txt
     mcp__claude_ai_Linear__save_issue(
       id: "<ENG-###>",
       state: "In Review"
     )
     ```

     The PR is now ready **and** CI-green: the human can
     merge it (or let "Merge when ready" land it) without
     waiting on anything else.

   - **Any check failed** → the PR is not actually clean,
     so don't leave it reading as merge-ready. Catalogue
     each failing check as **blocking** (name it and its
     log URL from the `gh pr checks` output), convert the
     PR back to draft — which also cancels any pending
     "Merge when ready" — and leave the Linear issue in
     its current state (do **not** move it to In Review):

     ```sh
     gh pr ready <number> --undo
     ```

     Report the failures and do **not** report the run
     as finished; the user fixes them and re-runs
     `/review-pr`.

1. **Firm up the permission allowlist.** A review
   run approves a lot of one-off commands, so it is
   the natural moment to generalize them. Invoke
   `/firm-perms` to collapse the per-worktree and
   per-arg `permissions.allow` entries into reusable
   globs and propagate them to the base repo so
   future worktrees inherit them. This is
   housekeeping on the gitignored
   `.claude/settings.local.json` — it does **not**
   affect the PR diff or its ready state, so run it
   regardless of the gate or CI outcome.

   **Account for what the review agents requested.**
   The diff-review and cross-check agents (steps 5–6)
   run in this session, so every command they made you
   approve is part of this run's churn. Tell
   `/firm-perms` to fold those in too — its harvest
   covers sub-agent approvals, not just commands you
   typed. Two outcomes, and report both:

   - A request that **can** be firmed (a bare command
     that just needed a `:*` glob) gets generalized
     and propagated like any other.
   - A request that **can't** — a `find / … | head`, a
     `sed … | grep`, a heredoc — is malformed, not
     missing a glob; `/firm-perms` sets it aside. When
     it does, that's a signal the **step-5 reviewer
     brief leaked**: an agent emitted shell the brief
     forbids. Tighten the brief (or the prompt) so the
     pattern stops recurring, rather than trying to
     allow-list it.

1. **Report.** Print a structured summary:

   - Linear coverage: the resolved tag, and each
     checklist item marked addressed / partial /
     missing (or "no Linear task checked" if none
     was resolvable).
   - Lint status: pass, fail with details, or which
     hooks were unverifiable locally (deps not
     installed) and why that's safe for this diff.
   - Generated artifacts: IDL / SDK clients /
     conformance vectors — regenerated clean, committed a
     refresh, or (IDL only) unverifiable without the
     toolchain.
   - Test status: `make test` and
     `make test-no-teardown` — pass, fail, or
     unverified locally (toolchain absent).
   - Title status: passes `Semantic PR` or not.
   - Merge status: `MERGEABLE` or `CONFLICTING`.
   - CI status: all GitHub checks green, or each failed
     check with its log URL, or "no checks" / still
     pending — the run is **not** finished until CI is
     green.
   - Linear status: moved to **In Review** (PR ready and
     CI green), or left unchanged (blockers, CI failing
     or pending, or no tag resolvable).
   - `CLAUDE.md` freshness: in sync, or each stale
     rule / reference the diff outdated, with the
     suggested correction.
   - Issues found / fixed / remaining.
   - Permissions: rules `/firm-perms` generalized this
     run, and any malformed request it set aside —
     naming the review agent that emitted it (step 5
     brief leak) so the prompt can be tightened.
   - Remaining warnings and nits for human review,
     each with `file:line` and rationale.
   - Whether the PR was marked ready.
