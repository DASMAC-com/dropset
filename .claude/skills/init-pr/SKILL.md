---
name: init-pr
description: Bootstrap a worktree — pull main, set up the branch, push a draft PR, and warm CI caches.
disable-model-invocation: true
user-invocable: true
---

# `init-pr`

Bootstrap the current worktree: pull main in the
base repo, set up the branch, push a draft PR so
CI caches start warming while work continues.

This is the first skill an agent should run after
`claude --worktree <tag>` starts.

## Input

Accepts an optional Linear tag like `eng-123`.
If not provided, infer it from the worktree
directory name (the last component of the current
working directory). If the inferred name doesn't
match `eng-###` (case-insensitive), stop and ask.

**When invoked with no other context** — just the
tag (or nothing), and no task instructions in the
session — treat the linked Linear issue as the full
specification for this worktree. After
bootstrapping, surface that issue's description and
checklist as the plan of work (final step) so the
session can proceed straight into the task without
asking what to build. Instructions the user *did*
give take precedence over the issue.

## Decision points use `AskUserQuestion`

`init-pr` brackets the whole worktree session: it
bootstraps, surfaces the task, and the session then
proceeds straight into the work. Wherever that flow
needs a decision from the user — a design choice, an
open question, a branching point — ask through the
**`AskUserQuestion`** TUI selector, not a free-text
prompt, so the human picks from the little terminal
pop-up instead of typing a reply. Offer concrete
options, and where one is the sensible default put it
**first** and label it "(Recommended)". The closing
`/review-pr` handoff (final step) is one such decision
point; the same applies to every other one the session
surfaces. This mirrors how `review-pr` already prompts
at its merge-queue handoff — the same TUI-selector
pattern, applied one stage earlier at the
init-pr → review-pr boundary.

## Steps

1. Validate that the resolved tag matches the
   pattern `eng-###` (case-insensitive). If not,
   stop and ask the user for it.

1. Pull main in the **base repository** (not this
   worktree). Don't use command substitution to
   find it — run the listing as its own command and
   read the path out of the output yourself:

   ```sh
   git worktree list --porcelain
   ```

   In that output, the worktree whose `branch` line
   is `refs/heads/main` is the base repo. Take its
   literal path and pull, passing the path inline so
   the call reduces to a stable allow-rule (no `$(…)`):

   ```sh
   git -C <main-worktree-path> pull --ff-only
   ```

   If no worktree has `main` checked out, skip the
   pull and warn the user.

1. Symlink `frontend/.env.local` from the main
   worktree so `pnpm dev` / `make frontend` pick up
   the same env without a manual copy. `.env*` is
   in `frontend/.gitignore`, so the symlink isn't
   tracked. Skip if main has no env file, or if
   this worktree already has one (don't clobber a
   real file someone placed deliberately).

   Do the existence checks with the **Glob/Read
   tools**, not a shell `test`/`if`. A
   `test … && … || …` compound never reduces to an
   allow-rule and re-prompts every run:

   - Glob `frontend/.env.local` in **this** worktree.
     If it matches, a file already exists — skip.
   - Glob (or Read) `frontend/.env.local` under the
     main worktree path. If it doesn't exist, main
     has no env file — skip and move on.

   Only when this worktree has none and main has one,
   create the link (the bare `ln` matches an existing
   allow-rule, so it won't prompt):

   ```sh
   ln -s <main-worktree-path>/frontend/.env.local frontend/.env.local
   ```

   If main isn't checked out anywhere (previous
   step), skip this one too.

1. Normalize the branch name to the bare Linear tag.
   The `aps` shell helper starts worktree sessions with
   `claude -w <tag>`, which names the worktree directory
   `eng-###` but the **branch** `worktree-eng-###` —
   there's no CLI flag to drop the `worktree-` prefix, so
   the skill strips it here rather than leaving each
   session to rename it by hand. Check the current branch:

   ```sh
   git branch --show-current
   ```

   - If it's `worktree-eng-###` (the `aps` default),
     rename it to the bare `eng-###` that matches the
     Linear issue identifier — pass both names literally
     so the call reduces to a stable allow-rule:

     ```sh
     git branch -m worktree-eng-### eng-###
     ```

   - If it's already `eng-###` (or any other non-`worktree-`
     name), this is a **no-op** — leave it alone. Only the
     `worktree-`-prefixed default is rewritten.

1. Rebase onto the freshly-pulled main so the
   worktree starts from the latest code:

   ```sh
   git rebase main
   ```

   If the rebase produces conflicts, abort it
   (`git rebase --abort`) and tell the user.
   Do not attempt to resolve conflicts
   automatically in this skill.

1. Create an empty, **signed** commit so there is
   something to push:

   ```sh
   git commit --allow-empty -S -m "<ENG-###>"
   ```

   The `-S` is mandatory: branch protection on
   this repo requires every commit to have a
   verified signature.

1. Push the branch:

   ```sh
   git push -u origin <eng-###>
   ```

1. Create a draft PR with the Linear tag as the
   title and an empty body, via the GitHub MCP. This
   repo is `DASMAC-com/dropset`, so pass
   `owner: "DASMAC-com"`, `repo: "dropset"`; the head
   is the branch you just pushed and the base is `main`:

   ```txt
   mcp__github__create_pull_request(
     owner: "DASMAC-com",
     repo: "dropset",
     title: "<ENG-###>",
     head: "<eng-###>",
     base: "main",
     body: "",
     draft: true,
   )
   ```

   The call returns the PR object, including its
   `html_url` — keep it for the final step.

1. Mark the Linear issue **In Progress** so the board
   reflects that work on this worktree has started.
   Update it by identifier (the uppercase tag) via the
   `claude.ai Linear` MCP:

   ```txt
   mcp__claude_ai_Linear__save_issue(
     id: "<ENG-###>",
     state: "In Progress"
   )
   ```

   If the issue doesn't exist or the update fails, warn
   and continue — bootstrapping shouldn't be blocked by
   Linear.

1. Print the new PR URL and confirm the Linear issue was
   moved to In Progress.

1. **Surface the task when no other context was
   given.** If the session was started with no
   instructions beyond the tag, the linked Linear
   issue *is* the spec. Fetch it with
   `mcp__claude_ai_Linear__get_issue` (id = the
   uppercase tag) and pull
   `mcp__claude_ai_Linear__list_comments` too —
   acceptance criteria sometimes live in an anchored
   comment, not the body. Present its description and
   any checklist as the plan of work so the session
   can proceed straight into the task. If the user
   provided their own instructions, those win; don't
   override them with the issue.

1. **Hand off to `/review-pr` when the work is ready.**
   This is the closing step of the bracketed session.
   Once the surfaced task's work is complete and every
   design decision and open question has been resolved
   (each asked through `AskUserQuestion`, per "Decision
   points use `AskUserQuestion`" above), announce that
   the work is ready and ask — again **via
   `AskUserQuestion`** — whether to run `/review-pr` now.
   Offer two options: "yes, run /review-pr" (**first**,
   the recommended default) and "not yet".

   - On **yes**, route straight into `/review-pr`.
   - On **not yet**, stop and leave the PR as it is.

   Do **not** surface `/pr-title-description` as its own
   step in this flow: `review-pr` already **calls** it
   for the final title and body (its steps 13–14), so
   offering it here would be redundant noise. The two
   user-facing skills are `/init-pr` then `/review-pr`;
   `pr-title-description` is a helper `review-pr` drives,
   not a freestanding stage.
