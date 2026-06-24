---
name: housekeeping
description: The thing to fire up when you arrive — one pass of day-to-day repo upkeep, run from the base repo root: fast-forward main so the run uses the latest skills, prune the worktrees of already-merged PRs, drain the Linear Permissions inbox doc via firm-perms (propose-only), mine the Session Metrics inbox into propose-only skill-improvement tasks, restage the backlog, then — only when given a finding cap (e.g. `/housekeeping 15`) — automatically kick off the audit-loop as a background campaign capped at that many issues and exit; with no number the audit-loop is skipped. The cspell dictionary check is opt-in (pass `cspell`) and off by default. Run it once at the start of the day, or drive ad-hoc upkeep with `/loop 30m housekeeping`. One pass per invocation, safe to repeat.
disable-model-invocation: false
user-invocable: true
---

# `housekeeping`

The **one thing to fire up when you arrive**: it
does the morning upkeep — the chores that pile up while
you develop but don't belong to any one PR — then, **when
you hand it a finding cap** (e.g. `/housekeeping 15`),
kicks off the audit campaign in the background so that, by
the afternoon, there's a ready-to-fire task sequence. With
**no** number it stops after the upkeep and skips the
audit-loop. It first fast-forwards `main` so the pass runs
on the latest committed skills, then:

1. **Prune merged worktrees** — remove the local
   worktree (and branch) of every PR that has
   already merged.
1. **Drain the Permissions inbox** — invoke
   `firm-perms` in **propose-only** mode against the
   Linear Permissions doc, so each captured prompt gets
   a recommended disposition (and malformed ones a
   source-fix task) without writing settings unattended.
1. **Mine the Session Metrics inbox** — read the Linear
   Session Metrics doc and file **propose-only**
   skill-improvement tasks for the trim patterns that
   recur across sessions, never editing a skill itself.
1. **Check convention references** — flag any skill that
   points at a `CLAUDE.md` section or `docs/conventions/`
   doc that no longer exists, filing the drift
   **propose-only**.
1. **Restage the backlog** — hand off to
   `stage-backlog` so the Task Staging document
   reflects everything currently open.
1. **Kick off the audit-loop** — **only when a finding
   cap was passed**, automatically launch the background
   audit campaign capped at that many issues, then
   **exit**. With no number, skip this and exit after the
   upkeep.

The morning entry point is a **single one-shot run**:
upkeep → background audit-loop (only when a finding cap
was passed) → exit. It does *not* stay on a timer; the
`/loop 30m housekeeping` cadence is there for ad-hoc
upkeep while you work, but the morning driver is the
one-shot. Each invocation is one pass and safe to repeat.

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

Optional, and accepts two independent arguments in any
order:

- **A finding cap** (a bare integer, e.g.
  `housekeeping 15`) — the maximum number of issues the
  audit-loop should file before it self-terminates. Its
  **presence is also the gate**: given a number, the pass
  automatically kicks off the background audit-loop capped
  at it (step 8); with **no** number, the audit-loop is
  **skipped** entirely and the pass exits after the
  upkeep. So `/housekeeping 15` does upkeep then launches a
  15-issue campaign, while a bare `/housekeeping` does
  upkeep only.
- **The `cspell` flag** — when the invocation includes
  `cspell` (e.g. `housekeeping cspell` or
  `housekeeping 15 cspell`, and likewise under
  `/loop 30m housekeeping cspell`), the pass runs the
  opt-in spelling-escape check (step 3); without it that
  step is skipped.

Any other argument is ignored.

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
annotating the Linear Permissions and Session Metrics
docs with recommended dispositions (it never writes
`settings.local.json` and never edits a skill
unattended). When given a finding cap, its last step
*launches* the audit-loop as a background task (step 8),
but housekeeping itself makes no source edit — the
campaign only files Linear issues.

Run it **once when you arrive with a finding cap**
(e.g. `housekeeping 15`) for the full morning-driver flow
(it launches the background campaign and exits), or with
no number for upkeep only, or drive ad-hoc upkeep on a
timer:

```sh
/loop 30m housekeeping
```

Invoked through `/loop 30m`, the harness re-runs this
skill every 30 minutes; each invocation does exactly
one pass and exits. Run it once by hand any time to
clean up on demand.

## Linear destination

Steps 3–7 file and stage Backlog issues and drain the
Permissions and Session Metrics docs, so they use the
same env-resolved Linear destination as `linear-task` /
`stage-backlog`. Resolve each variable with its **own**
bare `printenv` (one `Bash(printenv:*)` allow-rule
covers them all) — never a combined `printenv A B C`,
which on macOS / BSD prints only the first value:

```sh
printenv LINEAR_TEAM_ID
printenv LINEAR_PROJECT_ID
printenv LINEAR_ASSIGNEE_ID
printenv LINEAR_TASK_STAGING_DOC_ID
printenv LINEAR_PERMISSIONS_DOC_ID
printenv LINEAR_SESSION_METRICS_DOC_ID
```

If any is empty, skip the step that needs it and say
so; don't guess an id. (`firm-perms` resolves
`LINEAR_PERMISSIONS_DOC_ID` itself in step 4, and
`trim-context` resolves `LINEAR_SESSION_METRICS_DOC_ID`
itself in step 5; both are listed here only so the whole
set lives in one place.)

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
parallel sessions / chips for no benefit.) Each finding is a
**bullet carrying its
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

**5. Mine the Session Metrics inbox.** Invoke the
`trim-context` skill (via the Skill tool) — the consumer
half of the `session-metrics` producer. It resolves
`LINEAR_SESSION_METRICS_DOC_ID` itself, reads the doc
live, synthesizes the trim levers that **recur** across
the unprocessed entries, files one **propose-only**
skill-improvement Backlog task per distinct lever (each
with a `**Touches**:` + `**Fingerprint**:` line, deduped
against the open Backlog), and writes each consumed
entry's disposition back into the doc. `trim-context` has
**no** attended / propose-only split — filing a task *is*
the proposal, so it never edits a skill or convention
doc. If `LINEAR_SESSION_METRICS_DOC_ID` is unset,
`trim-context` says so and this step is a no-op.

**6. Check the convention ↔ skill reference sync.**
`CLAUDE.md` is the **index**; the full operating
conventions live in `docs/conventions/**`, and the skills
reference both. A moved section or renamed doc can leave a
skill pointing at something that no longer exists, so this
read-only pass flags that drift the same way `review-pr`'s
freshness lens does on the PR path — here, periodically.

- **Collect the targets.** List the headings in
  `CLAUDE.md` and the files under `docs/conventions/`
  (Read / Glob; never a shell `find … | …` pipe).
- **Scan the skills.** Grep `.claude/skills/**` for
  references to `CLAUDE.md` section names and
  `docs/conventions/…` paths (the Grep tool, or a bare
  single `grep` where it's absent — never `git grep`).
- **Flag dangling references** — a skill that cites a
  `CLAUDE.md` section heading that no longer exists, or a
  `docs/conventions/<file>.md` path that isn't present.
- **File propose-only**, to the same env-resolved
  destination as steps 4–5 (`save_issue`,
  `state: "Backlog"`, priority 3), one aggregated task per
  pass listing each dangling reference and its fix, with a
  `**Fingerprint**: convention-ref:<skill>:<target>` line
  per finding so later passes dedup; drop any fingerprint
  already open. **Autonomy bound:** filing *proposes* the
  fix — it never edits a skill, `CLAUDE.md`, or a doc; that
  lands later through a normal PR. If everything resolves,
  file nothing and note "in sync" in the report.

**7. Restage the backlog.** Invoke the
`stage-backlog` skill (via the Skill tool) to rewrite
the Task Staging document from the current open
Backlog — including anything steps 3–6 just filed. The
deterministic Python tool does all the work (read →
render → write); this skill just triggers it. This
**full** re-stage is the authoritative reconcile, run
fresh from the live Backlog each morning.

**8. Kick off the audit-loop (only when a finding cap was
passed).** The morning's last act: with upkeep done, the
**finding cap argument decides whether to run a
campaign** — passing a number *is* the go-ahead, so there
is no separate prompt (this is the one handoff in the
suite that's arg-gated rather than `AskUserQuestion`-gated,
precisely because the number carries the intent).

- **A finding cap was passed** (e.g. `housekeeping 15`) →
  launch the audit-loop **in the background**
  (`run_in_background`): start a background task that
  drives `/loop audit-loop`, passing that number as its
  **finding cap** argument. The campaign files findings,
  re-stages the Task Staging document once at the end of
  its run (`audit-loop` step 11), and fires a
  high-severity `PushNotification` only when something
  warrants interrupting you. Then **this housekeeping pass
  exits** — it does not wait on the campaign.
- **No finding cap** → skip the audit-loop entirely and
  end the pass after the upkeep. (To run a campaign, re-run
  with a number.)

**The kickoff is a one-shot, not a loop.** When a cap was
given, this run does the upkeep, launches the audit-loop as
a background task, and exits — it does *not* stay on a
timer. During the morning only the audit-loop (re-staging
once at the end of its run) writes the Task Staging
document, so there's no second loop to coordinate; the
next morning's step 7 re-stages again from the live
Backlog.

**9. Report.** Print a short summary:

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
- Session Metrics inbox: skill-improvement tasks filed
  (with their ENG-###) for the recurring trim levers,
  how many session entries were consumed, and any skipped
  as already-handled — or why the step was skipped (e.g.
  a missing env var).
- Convention references: in sync, or the dangling
  `CLAUDE.md` / `docs/conventions/` references filed
  (with the ENG-### of the aggregated task).
- Backlog staging: that `stage-backlog` ran, or why
  it was skipped (e.g. a missing env var).
- Audit-loop: launched in the background with finding
  cap N (so the campaign runs while you work), or
  skipped because no finding cap was passed.
