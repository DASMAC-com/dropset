---
name: housekeeping
description: One pass of day-to-day repo upkeep, run from the base repo root — fast-forward main so the run uses the latest skills, prune the worktrees of already-merged PRs, drain the Linear Permissions inbox doc via firm-perms (propose-only), and restage the backlog. The cspell dictionary check is opt-in (pass `cspell`) and off by default. Drive it with `/loop 30m housekeeping` while developing, or run it once at the start of the day.
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
1. **Drain the Permissions inbox** — invoke
   `firm-perms` in **propose-only** mode against the
   Linear Permissions doc, so each captured prompt gets
   a recommended disposition (and malformed ones a
   source-fix task) without writing settings unattended.
1. **Restage the backlog** — hand off to
   `stage-backlog` so the Task Staging document
   reflects everything currently open.

**Opt-in: spelling-escape hygiene.** The `cspell-audit`
check is **not** part of the default pass — it runs only
when you invoke `housekeeping cspell` (see "Input").
Escape drift is slow and just as easy to check by hand
(`/cspell-audit`), so it's kept out of the 30-minute loop
unless you ask for it. When the flag is set, the pass
adds a step: run `cspell-audit` read-only and **file** any
drift — a `cfg/dictionary.txt` entry to move, or a file
whose inline escapes need regrouping into a top block — as
a Backlog task to fix later.

## Input

Optional. The only argument is the **`cspell`** flag:
when the invocation includes `cspell` (e.g.
`housekeeping cspell`, or `/loop 30m housekeeping cspell`),
the pass runs the opt-in spelling-escape check (step 3);
with no argument it skips that step entirely. Any other
argument is ignored.

## Run it from the base repo root

This skill operates **across** worktrees — it
removes them — so it must run from the **base
repository**, never from inside a worktree (you
can't remove the worktree you're standing in). The
first step verifies this and stops if you're in the
wrong place.

It is safe to run repeatedly and makes **no source
edits** of its own: its only writes are removing
merged worktrees, filing / staging Linear issues, and
annotating the Linear Permissions doc with recommended
dispositions (it never writes `settings.local.json`
unattended).
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

Steps 3–5 file and stage Backlog issues and drain the
Permissions doc, so they use the same env-resolved
Linear destination as `linear-task` / `stage-backlog`.
Resolve each variable with its **own** bare `printenv`
(one `Bash(printenv:*)` allow-rule covers them all) —
never a combined `printenv A B C`, which on macOS /
BSD prints only the first value:

```sh
printenv LINEAR_TEAM_ID
printenv LINEAR_PROJECT_ID
printenv LINEAR_ASSIGNEE_ID
printenv LINEAR_TASK_STAGING_DOC_ID
printenv LINEAR_PERMISSIONS_DOC_ID
```

If any is empty, skip the step that needs it and say
so; don't guess an id. (`firm-perms` resolves
`LINEAR_PERMISSIONS_DOC_ID` itself in step 4; it's
listed here only so the whole set lives in one place.)

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
output and check whether its PR has merged through the
GitHub MCP. This repo is `DASMAC-com/dropset`, and the
`head` filter is `owner:branch`; query **all** states so
a closed-and-merged PR is visible:

```txt
mcp__github__list_pull_requests(
  owner: "DASMAC-com",
  repo: "dropset",
  head: "DASMAC-com:<branch>",
  state: "all",
)
```

Read the matching PR's `merged_at`: a **non-null**
`merged_at` means it merged (GitHub reports a merged PR as
`state: "closed"` with `merged_at` set, so key on
`merged_at`, not `state`).

- `merged_at` is set → the branch is done. Remove
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

- `merged_at` is null (PR still open, or closed
  without merging), or no PR exists, or the removal
  refused on a dirty worktree → **leave it**.
  Closed-without-merge and dirty worktrees are not safe
  to drop automatically; list them in the report so the
  user can decide.

After processing them all, tidy any stale worktree
admin entries:

```sh
git worktree prune
```

**3. Spelling-escape hygiene — run cspell, file the
drift as one aggregated issue.** **Opt-in — run this step
only when the invocation passed the `cspell` flag (see
"Input"); otherwise skip straight to step 4.** When it
runs: invoke the `cspell-audit`
skill in **delegated** (read-only) mode via the Skill
tool — it returns two kinds of violation and **edits
nothing**: a `cfg/dictionary.txt` word used in fewer
than two files (with its sole file and recommended
action), and a file whose inline escapes aren't in one
contiguous block at the top (with its path). This skill
is the only place the scheduled check lives — opt-in here,
via the `cspell` flag; `audit-loop` no longer runs it.

cspell fixes are all trivial and file-disjoint, so they
belong in **one PR** — file the run's drift as a **single
aggregated** Backlog issue, **not** one issue per finding.
(The old per-finding behavior scattered them into separate
parallel sessions that `stage-backlog` then had to
hand-consolidate.) Each finding is a **bullet carrying its
own `**Fingerprint**:` line**, so one issue = one PR while
later passes still dedup each finding individually. The
fingerprint `<key>` is keyed by kind: `dictionary:<word>`
for a dictionary entry, or `cspell-placement:<path>` for a
mis-placed file.

Dedup and refile so a 30-minute loop never duplicates work:

- Before filing, list the open Backlog
  (`mcp__claude_ai_Linear__list_issues`, same destination)
  and collect every `**Fingerprint**:` line already present
  across the open cspell issues. Only **new** findings —
  fingerprints not already open — are filed; drop the rest.

- If an **open aggregated cspell issue already exists** (an
  open Backlog issue carrying any `dictionary:` /
  `cspell-placement:` fingerprint — going forward there is
  at most one), **append** the new findings to its
  description and re-save it (`save_issue` with that issue's
  `id` and the full edited `description`), rather than
  opening a second aggregated issue. Diff against the live
  body you just read so existing bullets aren't clobbered. If
  more than one such issue somehow exists (e.g. a legacy
  per-finding issue alongside an aggregated one), append to
  the **lowest-ENG** one and note the others in the report so
  they can be hand-consolidated.

- Otherwise **create** one aggregated issue, one bullet per
  new finding:

  ```txt
  mcp__claude_ai_Linear__save_issue(
    team: "<$LINEAR_TEAM_ID>",
    project: "<$LINEAR_PROJECT_ID>",
    assignee: "<$LINEAR_ASSIGNEE_ID>",
    state: "Backlog",
    title: "cspell hygiene: move words inline / regroup escape blocks",
    description: "<one bullet per finding, each w/ a **Fingerprint**: line>",
    priority: 3,
  )
  ```

- If every finding is already open (nothing new), file
  **nothing** — neither create nor append.

Flagging the drift as a task — not fixing it here —
keeps this pass non-editing and lets the fix land in
a normal PR. (To fix it directly instead, run
`cspell-audit` on its own; that's its default mode.)

**4. Drain the Permissions inbox.** Invoke the
`firm-perms` skill (via the Skill tool) in
**propose-only** mode against the Linear Permissions
doc — pass it `doc propose-only`. It resolves
`LINEAR_PERMISSIONS_DOC_ID` itself, reads the doc
live, adjudicates each unchecked entry, annotates it
with the rule it *recommends* firming (or files a
source-fix task for a malformed one), and writes those
notes back into the doc. Because this pass is
unattended, `firm-perms` in this mode **never writes
`settings.local.json` and never ticks a checkbox** —
it only proposes. The actual firming is left for an
attended `/firm-perms doc` run. If
`LINEAR_PERMISSIONS_DOC_ID` is unset, `firm-perms`
says so and this step is a no-op.

**5. Restage the backlog.** Invoke the
`stage-backlog` skill (via the Skill tool) to rewrite
the Task Staging document from the current open
Backlog — including anything steps 3–4 just filed. All
the grouping / merge logic lives there; this skill
just triggers it.

**6. Report.** Print a short summary:

- `main`: fast-forwarded to the latest, or left at its
  current commit (with the reason) if the pull couldn't
  fast-forward — so a pass that ran on a stale checkout
  is never silent.
- Worktrees pruned (path + branch), and any left in
  place with the reason (PR open/closed-unmerged, no
  PR, or dirty tree).
- Spelling-escape drift (only if the `cspell` flag was
  passed; otherwise note the step was skipped): the
  aggregated cspell issue —
  whether new findings were filed into a fresh one or
  appended to the open one (with its ENG-###), how many
  (dictionary words to move and files whose escapes need
  regrouping), and any skipped as already-open duplicates;
  or that no drift was found.
- Permissions inbox: entries annotated with a
  recommended firm, source-fix tasks filed (with their
  ENG-###), and any skipped as already-handled — or
  why the step was skipped (e.g. a missing env var).
- Backlog staging: that `stage-backlog` ran, or why
  it was skipped (e.g. a missing env var).
