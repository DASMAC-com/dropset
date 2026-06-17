---
name: housekeeping
description: One pass of day-to-day repo upkeep, run from the base repo root — fast-forward main so the run uses the latest skills, prune the worktrees of already-merged PRs, run the cspell dictionary check and file any drift as a Backlog task, and restage the backlog. Drive it with `/loop 30m housekeeping` while developing, or run it once at the start of the day.
disable-model-invocation: false
user-invocable: true
---

# `housekeeping`

A single iteration of routine repo upkeep — the
chores that pile up while you develop but don't
belong to any one PR. It first fast-forwards `main`
so the pass runs on the latest committed skills, then
does three things:

1. **Prune merged worktrees** — remove the local
   worktree (and branch) of every PR that has
   already merged.
1. **Dictionary hygiene** — run the `cspell-audit`
   check read-only and **file** any
   `cfg/dictionary.txt` drift as a Backlog task to
   fix later (this is the *only* place cspell runs
   on a schedule; `audit-loop` no longer does it).
1. **Restage the backlog** — hand off to
   `stage-backlog` so the Task Staging document
   reflects everything currently open.

## Run it from the base repo root

This skill operates **across** worktrees — it
removes them — so it must run from the **base
repository**, never from inside a worktree (you
can't remove the worktree you're standing in). The
first step verifies this and stops if you're in the
wrong place.

It is safe to run repeatedly and makes **no source
edits** of its own: its only writes are removing
merged worktrees and filing / staging Linear issues.
Drive it on a timer while you work, or run it by
hand:

```sh
/loop 30m housekeeping
```

Invoked through `/loop 30m`, the harness re-runs this
skill every 30 minutes; each invocation does exactly
one pass and exits. Run it once by hand any time to
clean up on demand.

## Linear destination

Steps 2 and 3 file and stage Backlog issues, so they
use the same env-resolved Linear destination as
`linear-task` / `stage-backlog`. Resolve each
variable with its **own** bare `printenv` (one
`Bash(printenv:*)` allow-rule covers them all) —
never a combined `printenv A B C`, which on macOS /
BSD prints only the first value:

```sh
printenv LINEAR_TEAM_ID
printenv LINEAR_PROJECT_ID
printenv LINEAR_ASSIGNEE_ID
printenv LINEAR_TASK_STAGING_DOC_ID
```

If any is empty, skip the step that needs it and say
so; don't guess an id.

## Steps

**1. Confirm you're at the base repo root, then
fast-forward `main`.** List the worktrees and read the
paths out of the output yourself (no command
substitution):

```sh
git worktree list --porcelain
```

The worktree whose `branch` line is
`refs/heads/main` is the base repo. If the current
working directory is **not** that path, stop and
tell the user to run `housekeeping` from the base
repo root — do not `cd` there yourself (a `cd … &&`
compound can't reduce to an allow-rule). Keep the
parsed worktree list; step 2 reuses it.

Once confirmed, fast-forward `main` so the pass runs
on the latest committed code — the up-to-date version
of **this** skill and of the sub-skills it invokes
(`cspell-audit`, `stage-backlog`), rather than whatever
was current when the worktree was last synced. The base
repo has `main` checked out, so pull it in place (a bare
`git pull` reduces to the `Bash(git pull:*)` allow-rule):

```sh
git pull --ff-only
```

If the fast-forward fails (the base repo has diverging
local commits or a dirty tree), warn and continue with
what's checked out — never force or reset; this skill
makes no source edits. One honest caveat: the running
invocation already loaded its own instructions before
the pull, so a change to *this* skill takes effect on
the **next** iteration; the sub-skills invoked later in
this same pass (via the Skill tool) are read fresh and
do pick up the refreshed version immediately.

**2. Prune merged worktrees.** For every worktree in
the list **other than** the `refs/heads/main` base,
take its literal path and branch from the porcelain
output and check whether its PR has merged — pass the
branch inline so the call stays a `Bash(gh pr view:*)`
allow-rule:

```sh
gh pr view <branch> --json number,state,title
```

- `state` is `MERGED` → the branch is done. Remove
  the worktree (bare command, no `--force` — a
  worktree with uncommitted changes refuses, which is
  the safe outcome; leave it and note it):

  ```sh
  git worktree remove <path>
  ```

  Then delete the local branch. A squash- or
  rebase-merged branch tip is **not** an ancestor of
  `main`, so `git branch -d` would wrongly refuse;
  since `gh` already confirmed the PR merged, force
  the delete:

  ```sh
  git branch -D <branch>
  ```

- `state` is `OPEN` / `CLOSED` (unmerged), or no PR
  exists, or the removal refused on a dirty worktree →
  **leave it**. Closed-without-merge and dirty
  worktrees are not safe to drop automatically; list
  them in the report so the user can decide.

After processing them all, tidy any stale worktree
admin entries:

```sh
git worktree prune
```

**3. Dictionary hygiene — run cspell, file the
drift.** Invoke the `cspell-audit` skill in
**delegated** (read-only) mode via the Skill tool —
it returns each `cfg/dictionary.txt` violation (a
word used in fewer than two files, with its sole
file and recommended action) and **edits nothing**.
This skill is now the home of that periodic check;
`audit-loop` no longer runs it.

For each returned violation, file a Backlog task the
same way `linear-task` does — env-resolved
destination (above), `state: "Backlog"`, no parent,
priority 3 — with a fingerprint line so re-runs
dedup. Before filing, list the open Backlog
(`mcp__claude_ai_Linear__list_issues`, same
destination) and skip any issue already carrying the
same `**Fingerprint**: dictionary:<word>` line, so a
30-minute loop doesn't refile what's still open:

```txt
mcp__claude_ai_Linear__save_issue(
  team: "<$LINEAR_TEAM_ID>",
  project: "<$LINEAR_PROJECT_ID>",
  assignee: "<$LINEAR_ASSIGNEE_ID>",
  state: "Backlog",
  title: "cfg/dictionary.txt: move <word> inline / drop dead entry",
  description: "<finding + action>\n\n**Fingerprint**: dictionary:<word>",
  priority: 3,
)
```

Flagging the drift as a task — not fixing it here —
keeps this pass non-editing and lets the fix land in
a normal PR. (To fix the dictionary directly instead,
run `cspell-audit` on its own; that's its default
mode.)

**4. Restage the backlog.** Invoke the
`stage-backlog` skill (via the Skill tool) to rewrite
the Task Staging document from the current open
Backlog — including anything step 3 just filed. All
the grouping / merge logic lives there; this skill
just triggers it.

**5. Report.** Print a short summary:

- `main`: fast-forwarded to the latest, or left at its
  current commit (with the reason) if the pull couldn't
  fast-forward — so a pass that ran on a stale checkout
  is never silent.
- Worktrees pruned (path + branch), and any left in
  place with the reason (PR open/closed-unmerged, no
  PR, or dirty tree).
- Dictionary drift: violations filed (with their
  words), and any skipped as already-open duplicates.
- Backlog staging: that `stage-backlog` ran, or why
  it was skipped (e.g. a missing env var).
