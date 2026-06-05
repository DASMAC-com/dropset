---
name: review-pr
description: Adversarial pre-review — lint, catalogue issues, fix what's mechanical, and ready the PR for human review.
disable-model-invocation: true
user-invocable: true
---

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
     **blocking** issues (step 6) and do **not**
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
   logic as step 3.

1. **Push all fix commits:**

   ```sh
   git push
   ```

1. **Update the PR title and description.** Invoke
   `/pr-title-description` to ensure the title
   and body reflect the final state of the branch
   (after lint and review fixes).

1. **Gate.** If there are **zero blocking issues**
   and lint passes, mark the PR ready:

   ```sh
   gh pr ready <number>
   ```

   If blocking issues remain, leave it in draft.

1. **Report.** Print a structured summary:

   - Lint status: pass or fail with details.
   - Issues found / fixed / remaining.
   - Remaining warnings and nits for human review,
     each with `file:line` and rationale.
   - Whether the PR was marked ready.
