---
name: housekeeping
description: The thing to fire up when you arrive — one pass of day-to-day repo upkeep, run from the base repo root: fast-forward main so the run uses the latest skills, upgrade the Claude Code CLI (best-effort brew cask), prune the worktrees of already-merged PRs and dismiss their stale GitHub notifications, mine the Session Metrics inbox via trim-context (one aggregated propose-only task), propose merges among `Claude:`-prefixed meta-work issues only, and run one finite `/audit` rotation inline — the audit runs by DEFAULT, filing its findings parked under the Audit findings milestone, and is skipped only when given the `no-audit` flag. It does NOT analyze the board: collision sweeps, Backlog-wide merge groups, and scheduling smells belong to the `plan` skill. The cspell dictionary check is opt-in (pass `cspell`) and off by default. By default it runs one-shot — start to finish with no prompts interrupting the upkeep, flagging deferred items in its report (pass `interactive` to restore the per-step AskUserQuestion gates); either way it closes with one batched AskUserQuestion offering to act on everything it deferred, which an unattended run can simply leave unanswered. Run it once at the start of the day, or drive ad-hoc upkeep with `/loop 30m housekeeping`. One pass per invocation, safe to repeat.
disable-model-invocation: false
user-invocable: true
---

# `housekeeping`

The **one thing to fire up when you arrive**: it
does the morning upkeep — the chores that pile up while
you develop but don't belong to any one PR — then runs one
finite `/audit` rotation inline, **by default**, so the repo
stays continuously audited. Pass `no-audit` to skip it for a
deliberately quick pass. It first
fast-forwards `main` so the pass runs on the latest
committed skills and upgrades the Claude Code CLI
(best-effort), then:

1. **Prune merged worktrees** — remove the local
   worktree (and branch) of every PR that has
   already merged.
1. **Mine the Session Metrics inbox** — delegate to
   `trim-context`, which files a **single aggregated
   propose-only** skill-improvement task (one bullet per
   recurring trim lever) for the trim patterns that
   recur across sessions, never editing a skill itself.
1. **Check convention references** — flag any skill that
   points at a `CLAUDE.md` section or `docs/conventions/`
   doc that no longer exists, filing the drift
   **propose-only**.
1. **Propose merges among meta-work issues** — fold
   near-duplicate `Claude:`-prefixed issues, which are this
   skill's own filing output. Nothing else about the board
   is touched.
1. **Run one audit rotation** — invoke `/audit` once (a
   single finite rotation) inline, then **exit**. Its
   findings land **parked** under the Audit findings
   milestone, so a rotation costs the pull queue nothing.
   Pass `no-audit` to skip.

**It does not analyze the board.** No collision sweep, no
Backlog-wide merge-group scan, no scheduling-smell report:
those belong to the `plan` skill, which is the session that
actually decides sequencing. See "Why the board belongs to
`plan`" below.

The morning entry point is a **single one-shot run**:
upkeep → one `/audit` rotation → closing gate → exit. By default the upkeep
goes **start-to-finish with no `AskUserQuestion`
interrupting it** — it files or flags its deferred items and
reports rather than stopping to ask (see "One-shot vs.
interactive mode"); pass `interactive` to restore the
per-step prompts. It then closes with **one batched
question** offering to act on everything it deferred, which
fires in both modes and which an unattended run can leave
unanswered without losing any work (see "The closing gate").
It does *not* stay on
a timer; the `/loop 30m housekeeping` cadence is there for
ad-hoc upkeep while you work, but the morning driver is the
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

- **The `no-audit` flag** — the audit runs **by default**.
  A bare `/housekeeping` does upkeep *and* one finite
  `/audit` rotation (step 11); passing `no-audit` (e.g.
  `housekeeping no-audit`) skips the rotation for a
  deliberately quick pass.

  **This reverses the old opt-in `audit` flag**, and the
  reason is Part 10's parking change rather than a change of
  heart about audits. A rotation now files its findings
  **parked** under the Audit findings milestone, so it no
  longer injects anything into the pull queue — which was
  the entire cost of running it often. With that cost gone,
  the default that keeps the repo continuously audited is
  the right one. (`/audit` is itself finite — one rotation,
  no cap — so the flag only decides *whether* to run it, not
  how much.)

- **The `cspell` flag** — when the invocation includes
  `cspell` (e.g. `housekeeping cspell` or
  `housekeeping audit cspell`, and likewise under
  `/loop 30m housekeeping cspell`), the pass runs the
  opt-in spelling-escape check (step 3); without it that
  step is skipped.

- **The `interactive` flag** — by default the pass runs
  **one-shot**: start to finish with **no**
  `AskUserQuestion` gate, every interactive step taking its
  non-prompting branch so the morning driver never stalls
  waiting on an answer. Passing `interactive` (e.g.
  `housekeeping interactive`) restores the prompts — the
  meta-work merge gate (step 6), the
  perms-cruft removal (step 7), the stale-memory purge
  (step 8), and the session-metrics (step 9) and
  purge-conversations (step 10) offers. See "One-shot vs.
  interactive mode" for the full mapping.

Any other argument is ignored.

## One-shot vs. interactive mode

The morning driver has to **run to completion** — a direct
`/housekeeping` that stops at half a dozen
`AskUserQuestion` gates isn't a one-shot. So the pass has
two modes, and the default is the non-interrupting one:

- **One-shot (the default).** A bare `/housekeeping` (with
  or without `no-audit` / `cspell`) and **every** `/loop`
  cadence run. It fires **no** *intermediate*
  `AskUserQuestion`: each interactive step takes its
  **non-prompting branch** — the same branch the steps below
  label the *unattended* pass.
  Concretely: step 6 **lists** its meta-work merge
  candidates and merges nothing; steps 7
  and 8 **propose / list** the perms cruft and the stale
  memories and delete nothing; and steps 9 and 10 (the
  session-metrics and purge-conversations offers) are
  **skipped**. The deferred items are filed or flagged in
  the report for a later attended pass — nothing is ever
  deleted unattended. The **only** thing that may interrupt
  a one-shot is a genuine high-severity `/audit` finding on
  the `PushNotification` path (step 11); that stays.
- **Interactive** (`/housekeeping interactive`). Restores
  every **per-step** `AskUserQuestion` gate listed above, so
  you can act on the candidates as each step reaches them.
  Use it for an attended cleanup.

**The closing gate fires in both modes.** It is *not* what
the `interactive` flag controls. See "The closing gate"
below: the non-interrupting rule above governs the **middle**
of a pass, where an unanswered question would stall the
upkeep. Once the last upkeep step is done there is nothing
left to interrupt, and the run is holding a pile of proposals
that otherwise die in the report.

**Mapping.** Below, wherever a step distinguishes an
**attended** pass from an **unattended** one, read
*attended* = interactive and *unattended* = one-shot. The
per-step wording already spells out both branches; this
gate just fixes which branch the default takes.

## The closing gate

After the last upkeep step, fire **one** `AskUserQuestion`
batching every decision the pass deferred. One pass surfaced
several merge groups, two allowlist
defects and three stale memories — every one of them needing
a human decision the pass had deliberately declined to ask
for. Today they are printed and forgotten, and the next pass
re-derives the identical list from scratch.

Batch these, each **only when non-empty**:

- the **meta-work merge proposals** from step 6 ("merge
  these?") — `Claude:`-prefixed issues only;
- the **allowlist cruft** approved for removal in step 7;
- the **stale memories** approved for purge in step 8;
- the **inbox size** only when `trim-context` reports the
  body still growing *despite* draining on file — which
  should be impossible, so it means something is wrong with
  the drain rather than that a clear is owed. (Do **not**
  batch a "clear the inbox?" question: `trim-context` now
  drains every consumed entry unconditionally, so there is
  no such decision left for a human to authorize.)

Three constraints on it:

1. **One call, not five.** `AskUserQuestion` takes up to four
   questions with multi-select, which covers the batch. If a
   pass somehow accumulates more categories than fit, drop
   the lowest-value category to a report line rather than
   firing a second call.
1. **It stays skippable.** The gate is the *last* thing the
   pass does, so an unattended run (cron, `/loop`) that never
   gets an answer has already completed all its work — the
   questions simply go unanswered and the report stands
   exactly as it does today. **Nothing may depend on the
   answer**; anything that would is upkeep, and belongs
   earlier.
1. **It fires in both modes.** `interactive` restores the
   per-step prompts, which is a different thing.

## Why the board belongs to `plan`

This skill used to analyze the board: a full collision
sweep, a Backlog-wide scan for merge groups, and a
scheduling-smell report. **All three moved to the `plan`
skill** (its steps 1–3).

The reason is not that the work stopped mattering — it is
that planning sessions now exist as a distinct session kind,
and the work is theirs. Sequencing the board is a judgment
call that needs the direction a planning session holds and
this one does not. A housekeeping pass running between
planning sessions was reaching board conclusions — updating
edges, proposing merges — with none of that context, which
put the two skills in conflict over the same artifact.

What moved, and where it landed:

- the **full collision sweep** → `plan` step 1's bootstrap.
  It files only `related` links and no blocking edge, so it
  is bookkeeping rather than judgment; it moved anyway,
  because the operator's direction is that housekeeping
  stops touching the board at all. The **file-time**
  `sync_blockers.py --for` calls the filing skills make are
  unaffected — those are part of filing, not board analysis.
- **merge-group proposals** → `plan` step 2, alongside
  Queue honesty, which is the decision they serve.
- the **scheduling-smell scan** → `plan` step 1. It had no
  stated home there before, yet a planning session ran it
  and immediately found two dead edges holding an Urgent
  issue out of the available set — so it earns its place.

**The one carve-out that stays here** is step 6's meta-work
merge proposal, scoped to the `Claude:` title prefix. That
is not board sequencing: this skill *files* those issues
itself via the `trim-context` mining pass, so folding its
own near-duplicate output is upkeep on its own artifacts.

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
annotating the Linear Session Metrics doc with
recommended dispositions (it never edits a skill
unattended). Its last step runs one `/audit` rotation
(step 11), but housekeeping itself makes no source edit —
the rotation only files Linear issues, and files them
parked.

Run it **once when you arrive** for the full morning-driver
flow (upkeep, then one audit rotation, then exit), pass
`no-audit` for upkeep only, or drive ad-hoc upkeep on a
timer:

```sh
/loop 30m housekeeping
```

Invoked through `/loop 30m`, the harness re-runs this
skill every 30 minutes; each invocation does exactly
one pass and exits. Run it once by hand any time to
clean up on demand.

## Linear destination

Steps 3–6 file and reconcile Backlog issues and mine the
Session Metrics doc, so they use the
same env-resolved Linear destination as `linear-task` /
`sync-blockers`. Resolve each variable with its **own**
bare `printenv` (one `Bash(printenv:*)` allow-rule
covers them all) — never a combined `printenv A B C`,
which on macOS / BSD prints only the first value:

```sh
printenv LINEAR_TEAM_ID
printenv LINEAR_PROJECT_ID
printenv LINEAR_ASSIGNEE_ID
printenv LINEAR_SESSION_METRICS_DOC_ID
```

If any is empty, skip the step that needs it and say
so; don't guess an id. (`trim-context` resolves
`LINEAR_SESSION_METRICS_DOC_ID` itself in step 4; it's
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
(`cspell-audit`, `sync-blockers`), rather than whatever
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

**Then upgrade the Claude Code CLI** — same arrival-refresh
spirit as the `main` fast-forward, keeping the tool itself
current, not just the checkout. On this machine Claude Code
is a Homebrew **cask** named `claude-code@latest`
(`/opt/homebrew/bin/claude` →
`Caskroom/claude-code@latest/…`), so the upgrade is a single
bare command reducing to the `Bash(brew upgrade:*)`
allow-rule:

```sh
brew upgrade --cask claude-code@latest
```

Two caveats: **(1)** it's a **cask**, not a plain formula —
`--cask claude-code@latest` is the verified name; a bare
`brew upgrade claude-code` would be a silent no-op. **(2)**
like the skill-refresh caveat above, the running session
keeps the binary it launched with; the upgrade takes effect
on the **next** launch. Make it **best-effort** — a brew
hiccup or an offline machine must never block the upkeep
pass, so on any error note it in the report and continue.
(This runs on every pass, including the `/loop 30m` cadence
— a cheap no-op when already current; gate it behind a flag
only if the loop churn proves noisy. Upgrading the CLI is
not a repo source edit, so it doesn't break the skill's
"makes no source edits" guarantee.)

**2. Prune merged worktrees.** Read the set of merged PRs
**once**, field-selected, instead of one full-body MCP
`list_pull_requests` per worktree branch (each of those
returns the whole PR object, replayed every later turn —
see `docs/conventions/context-economy.md`). `gh pr list`
has a `merged` state filter the MCP lacks and `--json`
selects just the three fields the decision needs; it's a
`--json` **flag**, not a pipe, so it reduces to the
already-pre-approved `Bash(gh pr list:*)` read-rule (see
`docs/conventions/github-mcp.md`):

```sh
gh pr list --state merged --json number,headRefName,mergedAt --limit 100
```

The returned `headRefName`s are the branches whose PR
**merged**. Hand that set to the prune helper, which does the
deterministic git work — for every worktree **other than**
the `refs/heads/main` base, remove the ones whose branch
merged (bare `git worktree remove`, no `--force`, so a dirty
or locked tree refuses and is recorded as skipped, not
dropped), force-delete each removed branch (a squash/rebase
tip isn't an ancestor of `main`, so `-d` would wrongly
refuse; the PR is confirmed merged), then
`git worktree prune` the stale admin entries. Pass the
merged branches (run from the base repo root):

```sh
python3 .claude/tools/prune_worktrees.py --merged <branch> <branch> ...
```

It prints `{removed, skipped, left, pruned, dry_run}` — the
tally to report: `removed` (worktree + branch dropped),
`skipped` (a dirty/locked merged tree that refused — the safe
outcome), and `left` (branches not in the merged set: PR
still open, closed-without-merge, or no PR). Preview with
`--dry-run` first if you want to see the candidates without
touching anything. Closed-without-merge and dirty worktrees
are not dropped automatically — they land in `skipped` /
`left` so the user can decide.

**Then mark notifications for merged PRs done.** Merged PRs
leave GitHub notifications that otherwise pile up with no
easy bulk clear. List the unread notifications through the
GitHub MCP and dismiss only the ones whose PR has **merged**
— a robust catch-all that also covers auto-merged PRs and
others' PRs the worktree sweep above never touches:

```txt
mcp__github__list_notifications(
  owner: "DASMAC-com",
  repo: "dropset",
)
```

For each notification whose `subject.type` is
`PullRequest`, read that PR (its number is the tail of
`subject.url`) and key on `merged_at` exactly as above:

```txt
mcp__github__pull_request_read(
  owner: "DASMAC-com",
  repo: "dropset",
  pullNumber: <number>,
  method: "get",
)
```

- `merged_at` is **non-null** → mark that one notification
  **done**:

  ```txt
  mcp__github__dismiss_notification(
    threadID: "<notification id>",
    state: "done",
  )
  ```

- `merged_at` is null (open or closed-unmerged), or the
  subject isn't a PR → **leave it**.

`state: "done"` is load-bearing, and it is the one argument
this step gets asked about. `"read"` only clears the unread
marker — the thread **stays in the GitHub inbox**, so a
sweep that used it looked like a no-op to the human reading
their notifications. `"done"` removes the thread, which is
the correct terminal state here precisely because the step
already gated on `merged_at`: a merged PR has nothing left
to come back to. Say "done" (not "read") in the report line
too, so the report matches what the inbox shows.

**Never** call `mark_all_notifications_read` — that would
clear unread mentions, review requests, and other non-merge
notifications too. Only a confirmed-merged PR's
notification is marked done.

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
via the `cspell` flag; `audit` no longer runs it.

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
  at most one), add the new findings to it rather than
  opening a second aggregated issue — with a **`patch`** on
  that issue's `id` (one `append` op carrying the new
  bullets, plus a `replace` on its `**Touches**:` line),
  not a re-sent `description`, per
  `docs/conventions/linear-automation.md` → "Partial edits —
  the `patch` argument". The `append` can't clobber an
  existing bullet, so no diffing against the live body is
  needed for the new findings — but copy the `**Touches**:`
  line the `replace` targets **verbatim** from the issue you
  just read, since that op has to match it exactly once. If
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

**4. Mine the Session Metrics inbox.** Invoke the
`trim-context` skill (via the Skill tool) — the consumer
half of the `session-metrics` producer. It resolves
`LINEAR_SESSION_METRICS_DOC_ID` itself, reads the doc
live, synthesizes the trim levers that **recur** across
the unprocessed entries, files a **single aggregated
propose-only** skill-improvement Backlog task — one
bullet per distinct lever, each with its own
`**Fingerprint**:` line under a combined `**Touches**:`,
deduped against the open Backlog and **appended** to the
open aggregated task rather than opening a second — and then
**drains** each consumed entry out of the doc, recording its
disposition in the drain history. `trim-context` has
**no** attended / propose-only split — filing a task *is*
the proposal, so it never edits a skill or convention
doc.

**Pass it nothing about clearing.** There is no clear
decision to inherit any more: filing the task discharges the
entry, so `trim-context` drains unconditionally in both
modes. This step used to hand it a **leave** decision on the
one-shot path — which, since one-shot is the default and the
only path the morning driver takes, meant the clear
effectively never fired and the inbox filled with
already-filed entries. That hook is retired; don't
re-introduce it. If `LINEAR_SESSION_METRICS_DOC_ID` is
unset, `trim-context` says so and this step is a no-op.

**5. Check the convention ↔ skill reference sync.**
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
  destination as step 4 (`save_issue`,
  `state: "Backlog"`, priority 3), one aggregated task per
  pass listing each dangling reference and its fix, with a
  `**Fingerprint**: convention-ref:<skill>:<target>` line
  per finding so later passes dedup; drop any fingerprint
  already open. The task only edits skills / `CLAUDE.md` /
  `docs/conventions/**`, so it's meta-work — prepend the
  **`Claude:`** prefix to its title (per `CLAUDE.md` →
  "Claude: meta-work prefix"). **Autonomy bound:** filing *proposes* the
  fix — it never edits a skill, `CLAUDE.md`, or a doc; that
  lands later through a normal PR. If everything resolves,
  file nothing and note "in sync" in the report.

**6. Propose merges among meta-work issues only.**

**This step does not analyze the board.** No collision
sweep, no Backlog-wide merge-group scan, no scheduling-smell
report — all three moved to the `plan` skill (its steps 1–3),
which is where somebody is actually deciding sequencing.
See "Why the board belongs to `plan`" below.

What remains is a narrow carve-out, and it exists because
this skill is itself a **producer** of issues: step 4 runs
the `trim-context` mining pass, and repeated passes can file
near-duplicate aggregated tasks. Folding those is upkeep on
this skill's **own output**, not board sequencing.

So: scan open issues whose titles carry the **`Claude:`**
meta-work prefix, and propose folding any that would land as
one PR. Scope it by that title token — an issue without the
prefix is out of scope for this step, whatever it touches.
Propose-only, as everywhere else here: in an attended pass
surface the groups via `AskUserQuestion` and run
`/merge-tasks <ids>` on the approved ones; in a one-shot
pass just list the suggestions and merge nothing. The
**coherence floor** still binds (`merge-tasks`' own
`cross_area` warning is the backstop).

If no meta-work duplicates are open, say "no meta-work
merges proposed" and move on.

**7. Audit the base-repo permission allowlist for cruft.**
`firm-perms` only ever **adds** to
`<base>/.claude/settings.local.json` (unions, generalizes),
never prunes — so dead weight accumulates. Get the suspicious
shortlist from the helper (`<base>` was resolved in step 1)
rather than whole-reading the ~250-entry array into context
(per `CLAUDE.md` → "Context economy" / "Skill tooling"):

```sh
python3 .claude/tools/allowlist.py \
  --settings <base>/.claude/settings.local.json cruft
```

It prints `{count, flagged: [{index, rule, category, reason}]}`
— only the entries that look wrong, keyed by category:

- **over-broad grants** (`category: over-broad`) — a bare
  `Bash(:*)`, an unscoped `Read(…)` / `Edit(…)` root, or a
  bare-verb wildcard that subsumes many narrower rules;
- **dangerous one-offs** (`category: dangerous`) — `rm -rf`,
  `curl … | sh`, `git push --force`;
- **machine paths** (`category: machine-path`) — a malformed
  path (a doubled slash, so the rule can never match), or an
  absolute home path in a settings file where one does not
  belong;
- **stale machine paths** (`category: machine-path-stale`) —
  an absolute path that no longer resolves on disk, which is
  what worktree rules decay into as worktrees are pruned;
- **stale single-use commands** (`category: subsumed`) — a
  narrower rule an earlier one already covers (the dead weight
  `firm-perms` never removes).

**Don't expect `machine-path` on this repo's own allowlist.**
The audited file is `settings.local.json` — git-ignored and
machine-local by design — so an absolute `/Users/<name>/…` is
the *correct* form there, and the check knows it (the response
carries `machine_local_settings: true`). It used to flag them
regardless, which returned **40 entries of which 39 were false
positives**, nearly all load-bearing: the
`git -C <base>/.claude/worktrees/*` rules the worktree flow
needs, the `~/.zshrc` reads `local-integrations.md` prescribes,
and the `python3 <base>/.claude/tools/*` entry point. If you
see a wall of `machine-path` here, suspect the check before the
allowlist.

The helper is deterministic and pattern-based, so also skim
its `flagged` list for a **secret** that leaked into a rule
(it can't classify those) before proposing removals.

**Autonomy bound: propose, never auto-delete.** Dropping a
permission is low-blast-radius, but silently editing the
allowlist unattended is surprising. In an **attended** pass,
surface the shortlist via **`AskUserQuestion`** and remove
(with the Edit tool, per the JSON-editing convention) only
the entries the human approves — and confirm the edited file
still parses with an **exit-code-only** check,
`python3 -m json.tool <file> >/dev/null` or the same routed
through `run_quiet`, never a full pretty-print echo that
re-dumps the array into context; in an **unattended** pass,
file the candidates **propose-only** (or just list them) and
delete nothing. This is the pruning half; `firm-perms` is
the add-only half, and the allowlist is `settings.local.json`
(git-ignored per the settings.json decision). The
`allowlist.py cruft` helper above is what keeps the full file
out of context — it returns only the shortlist, so this step
never reads the whole ~250-entry array.

**8. Review saved auto-memory for staleness.** The saved
auto-memory (`~/.claude/projects/<slug>/memory/*.md` plus the
`MEMORY.md` index) accretes; curate it for freshness.

**Gate the scan on a cadence first.** Reading the whole
store and repo-verifying every reference is the pass's
**dominant compute**, and the store changes slowly, so
don't re-run it on every 30-minute `/loop` iteration. Ask
the gate whether a fresh scan is warranted — the memory dir
is `~/.claude/projects/<slug>/memory` (`<slug>` the cwd
slug, the same path used below):

```sh
python3 .claude/tools/memory_scan_gate.py check <memory_dir>
```

It prints `{scan, reason, …}` and returns `scan: true` when
the store changed since the last scan **or** the daily floor
(~20h) has elapsed — so the morning one-shot still re-scans,
but a tight `/loop` within that window skips. **If `scan`
is `false`, skip the rest of this step** and note "memory
scan skipped (`<reason>`)" in the report. Only when `scan`
is `true`, do the review below.

Read the memory bodies **in this step's own pass** and flag
a memory as stale when it:

- names a **file / function / flag / `ENG-###`** that no
  longer exists (a dangling reference — the same check the
  memory-recall caveat demands before acting on a memory);
- is **superseded or contradicted** by a newer memory or by
  current code / conventions;
- describes work that has since **shipped and is now
  derivable from the repo**, so it no longer earns its
  context slot.

For each stale candidate, **purge** = delete the memory
`.md` file **and** remove its one-line `MEMORY.md` pointer
(keep the index and the files in sync). **Autonomy bound:**
losing a still-good memory is worse than keeping a stale
one, so in an **attended** pass confirm the candidates via
**`AskUserQuestion`** before deleting; in an **unattended**
pass, list them and delete nothing. **Read-mostly wrt
context:** report only slugs + one-line reasons, never
replay full memory bodies into the main loop. (Distinct from
the `purge-conversations` skill, which reclaims *disk* from
transcripts/caches; this curates the knowledge store for
freshness.)

**Record the scan** once the review has run (in either mode
— a one-shot that only *listed* stale candidates still
performed the scan) so the next `check` measures against
it:

```sh
python3 .claude/tools/memory_scan_gate.py record <memory_dir>
```

**9. Offer a session-metrics run.** The morning pass both
*mines* the Session Metrics inbox (step 4) and can
*contribute* to it: offer, via **`AskUserQuestion`** with
the recommended default **first**, to run `/session-metrics`
for the **current** session so this pass also appends a
fresh measured entry (the producer side of the loop).
Run it only on an explicit yes. It comes *after* step 4 so
the entry it appends is next pass's work rather than this
one's — but nothing depends on that ordering any more: step
4 drains exactly the entries it read, and `patch`'s
exactly-once anchors make a concurrent append safe. (In
an unattended pass with no one to answer, skip the offer.)

**10. Offer a purge-conversations run.** Local transcripts
and caches (`~/.claude/projects`, `~/.claude/file-history`,
the CLI cache) accumulate — the base-repo project dir alone
measured 151M. Offer, via **`AskUserQuestion`** with the
recommended default **first** (mirroring the step-9
`/session-metrics` offer), to run `/purge-conversations` for
this machine. Run it only on an explicit yes — it prints a
dry-run manifest and takes its **own** approval before
deleting anything, so this is a two-gate handoff, never an
unattended delete. (In an unattended pass with no one to
answer, skip the offer — nothing is purged.)

**11. Run one audit rotation (unless `no-audit` was
passed).** The morning's last act, and it runs **by
default** — there is no prompt, because a rotation no longer
costs the pull queue anything.

- **Default** → invoke the `audit` skill (via the Skill
  tool) **once**. `/audit` is finite — a single seven-unit
  rotation that files its findings (recording each one's
  file collisions via `sync-blockers --for` as it goes),
  fires a high-severity `PushNotification` only when
  something warrants interrupting you, and stops on its own
  with a `DONE` line. It runs **inline** (it's bounded, so
  there's no background campaign to wait on), then **this
  housekeeping pass exits**.
- **`no-audit` was passed** → skip the rotation and end
  the pass after the upkeep.

**Findings land parked, and say so.** Every finding the
rotation files carries the **Audit findings** project
milestone, which means *parked* — a first-class open issue
for dedup, collision detection and search, but **not** in
the pull queue and invisible to a planning bootstrap until
somebody asks for it. So when reporting the rotation, give
**counts and titles only**, and state plainly that the
findings are parked and awaiting sequencing. A
`/housekeeping` run must not read as having queued work;
promoting a finding is the `plan` skill's call (its step 8).

**The kickoff is a one-shot, not a loop.** `/audit` is a
single bounded rotation, not a continuous campaign — it
files what its seven units surface and stops. To audit
again, run `/housekeeping` (or `/audit`) again.

**12. Report.** Print a short summary:

- Mode: **one-shot** (the default — ran to completion with
  no prompts, deferred items flagged) or **interactive**.
- `main`: fast-forwarded to the latest, or left at its
  current commit (with the reason) if the pull couldn't
  fast-forward — so a pass that ran on a stale checkout
  is never silent.
- Claude Code CLI: upgraded (with the new version), already
  current, or the brew error if the upgrade couldn't run
  (never fatal).
- Worktrees pruned (path + branch), and any left in
  place with the reason (PR open/closed-unmerged, no
  PR, or dirty tree); and how many merged-PR
  notifications were marked **done**.
- Spelling-escape drift (only if the `cspell` flag was
  passed; otherwise note the step was skipped): the
  aggregated cspell issue —
  whether new findings were filed into a fresh one or
  appended to the open one (with its ENG-###), how many
  (dictionary words to move and files whose escapes need
  regrouping), and any skipped as already-open duplicates;
  or that no drift was found.
- Session Metrics inbox: the aggregated skill-improvement
  task — whether new levers were filed into a fresh one or
  appended to the open one (with its ENG-###), and how many
  levers — for the recurring trim patterns, how many session
  entries were consumed **and drained**, any levers skipped
  as already-handled, and the inbox's remaining size — or
  why the step was skipped (e.g. a missing env var).
- Convention references: in sync, or the dangling
  `CLAUDE.md` / `docs/conventions/` references filed
  (with the ENG-### of the aggregated task).
- Meta-work merge proposals: the near-duplicate
  `Claude:`-prefixed clusters proposed for folding via
  `merge-tasks` and which the human approved merging
  (attended), or the suggested groups listed (unattended) —
  or that there were none. **Nothing about the wider board**
  is reported here; that is the `plan` skill's output.
- Permission allowlist: the `settings.local.json`
  entries flagged as cruft and, for an attended pass, which
  the human approved removing — or that it was clean.
- Auto-memory: the memory slugs flagged stale (with the
  one-line reason each) and, for an attended pass, which
  were purged — or that all are fresh; or that the scan was
  **skipped this pass** (with the cadence-gate reason).
- Session metrics run: whether a `/session-metrics` run
  was offered and accepted for this session, or skipped.
- Purge-conversations: whether a `/purge-conversations` run
  was offered and accepted (with the MB freed), or skipped.
- Audit: one `/audit` rotation ran inline — report its
  **count and the finding titles only**, and say that they
  are **parked** under the Audit findings milestone awaiting
  sequencing by a planning session. Or that it was skipped
  because `no-audit` was passed.

**13. Fire the closing gate.** With the report printed, batch
every deferred decision into the single `AskUserQuestion`
described in "The closing gate" above — the meta-work merge
groups, the allowlist cruft, the stale memories,
the inbox size — including only the categories that are
non-empty. This runs in **both** modes.

**The parked audit findings are not part of this gate.**
Offering to slate them in is the `plan` skill's step 8;
asking here would put a sequencing decision in the session
that deliberately does not do sequencing.

Act on whatever comes back, and if nothing does (an
unattended run with nobody watching), that's fine: the pass
is already complete and the report already stands. Nothing
here may be load-bearing.
