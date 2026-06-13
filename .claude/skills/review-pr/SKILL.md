---
name: review-pr
description: Adversarial pre-review — verify the Linear task's checklist is fully addressed, lint, catalogue issues, fix what's mechanical, and ready the PR for human review.
user-invocable: true
---

<!-- cspell:word oneline -->

# `review-pr`

Act as an adversarial reviewer before the human
looks at the PR. Run lint, audit the diff,
catalogue every issue, fix what can be fixed
mechanically, and mark the PR ready only when
it's clean.

Run this after autonomous work is complete and
all changes are committed and pushed.

## Steps

1. **Locate the PR.** Identify the current branch
   and its PR:

   ```sh
   branch=$(git branch --show-current)
   gh pr view "$branch" \
     --json number,title,state,isDraft
   ```

   If no PR exists, stop and tell the user to
   run `/init-pr` first.

1. **Check for uncommitted work.** Run
   `git status` — if there are uncommitted
   changes, stop and tell the user to commit
   first (or run `/commit-changes`).

1. **Check the Linear task is fully addressed.**
   The PR exists to satisfy a Linear issue, and
   autonomous runs have a habit of shipping a diff
   that covers only *some* of the task's checklist.
   Establish what the task asked for and confirm the
   diff delivers all of it before reviewing anything
   else.

   - Resolve the tag from the PR title scope (the
     `ENG-###` inside `type(ENG-###): …`), falling
     back to the branch name. If neither yields an
     `ENG-###`, skip this step and note in the report
     that no Linear task was checked.

   - Fetch the issue with the `mcp__claude_ai_Linear__get_issue`
     tool, passing the tag as `id` (e.g. `"ENG-490"`).
     Read the description, and also pull
     `mcp__claude_ai_Linear__list_comments` for the
     issue — checklist items and acceptance criteria
     sometimes live in an inline (anchored) comment,
     not the body.

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

   - Catalogue every **partial** or **missing**
     requirement as a **blocking** issue (step 7),
     quoting the checklist text and the `file:line`
     (or absence) that decides it. Do **not** tick
     the boxes in Linear or otherwise mutate the
     issue — this step only verifies and reports.

1. **Run lint:**

   ```sh
   make lint
   ```

   If lint fails:

   - Parse the output and fix every issue that
     can be fixed mechanically (formatting,
     import order, trailing whitespace, spelling,
     etc.).

   - Stage the fixes by explicit path and commit
     as a single signed commit:

     ```sh
     git add <fixed files...>
     git commit -S -m "Fix lint violations"
     ```

   - Re-run `make lint` to confirm it passes.

   - If lint still fails after the fix attempt,
     catalogue the remaining failures as
     **blocking** issues (step 7) and do **not**
     mark the PR ready.

1. **Adversarial diff review.** Get the full diff:

   ```sh
   git diff main..HEAD
   git log main..HEAD --oneline
   ```

   Spawn parallel sub-agents via the `Agent` tool
   (single message, multiple calls) to review the
   diff. At minimum:

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

   Each sub-agent must return findings with file
   path, line number, severity (**blocking** /
   **warning** / **nit**), and a one-line
   rationale.

1. **Adversarial cross-check.** Spawn a fresh
   sub-agent that receives the collected findings
   and the diff, and is told to act
   adversarially:

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

1. **Push all fix commits:**

   ```sh
   git push
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

1. **Check for merge conflicts with `main`.** A
   branch can be lint-clean and review-clean yet
   still conflict with `main` if `main` advanced
   while the work was in flight. Fetch the latest
   base and ask GitHub whether the PR still merges
   cleanly:

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
   CI-mirroring check is green so the human can
   safely approve auto-merge: **zero blocking
   issues** (including every Linear checklist item
   addressed), `make lint` passes, `make test` and
   `make test-no-teardown` pass (or are honestly
   reported as unverifiable locally), the title
   passes `Semantic PR`, and `mergeable` is
   `MERGEABLE`:

   ```sh
   gh pr ready <number>
   ```

   If any blocking issue remains — an unaddressed
   Linear checklist item, failing or unverified
   tests, a non-conforming title, or a merge
   conflict with `main` — leave it in draft.

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
   regardless of the gate outcome.

1. **Report.** Print a structured summary:

   - Linear coverage: the resolved tag, and each
     checklist item marked addressed / partial /
     missing (or "no Linear task checked" if none
     was resolvable).
   - Lint status: pass or fail with details.
   - Test status: `make test` and
     `make test-no-teardown` — pass, fail, or
     unverified locally (toolchain absent).
   - Title status: passes `Semantic PR` or not.
   - Merge status: `MERGEABLE` or `CONFLICTING`.
   - Issues found / fixed / remaining.
   - Remaining warnings and nits for human review,
     each with `file:line` and rationale.
   - Whether the PR was marked ready.
