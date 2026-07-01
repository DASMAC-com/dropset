---
name: housekeeping
description: The thing to fire up when you arrive — one pass of day-to-day repo upkeep, run from the base repo root: fast-forward main so the run uses the latest skills, upgrade the Claude Code CLI (best-effort brew cask), prune the worktrees of already-merged PRs and dismiss their stale GitHub notifications, mine the Session Metrics inbox via trim-context (one aggregated propose-only task), reconcile Backlog blocking edges via a sync-blockers sweep, then — only when given the `audit` flag (`/housekeeping audit`) — run one finite `/audit` rotation inline and exit; with no flag the audit is skipped. The cspell dictionary check is opt-in (pass `cspell`) and off by default. Run it once at the start of the day, or drive ad-hoc upkeep with `/loop 30m housekeeping`. One pass per invocation, safe to repeat.
disable-model-invocation: false
user-invocable: true
---

# `housekeeping`

The **one thing to fire up when you arrive**: it
does the morning upkeep — the chores that pile up while
you develop but don't belong to any one PR — then, **when
you pass the `audit` flag** (`/housekeeping audit`), runs
one finite `/audit` rotation inline so a fresh batch of
findings lands on the Backlog. With **no** flag it stops
after the upkeep and skips the audit. It first
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
1. **Reconcile blocking edges** — run an optional full
   `sync-blockers` sweep to catch any file-overlap edge
   the file-time `--for` calls didn't already file.
1. **Run one audit rotation** — **only when the `audit`
   flag was passed**, invoke `/audit` once (a single
   finite rotation) inline, then **exit**. With no flag,
   skip this and exit after the upkeep.

The morning entry point is a **single one-shot run**:
upkeep → one `/audit` rotation (only when the `audit` flag
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

- **The `audit` flag** — when the invocation includes
  `audit` (e.g. `housekeeping audit`), the pass runs one
  finite `/audit` rotation inline after the upkeep
  (step 11); without it the audit is **skipped** entirely
  and the pass exits after the upkeep. So
  `/housekeeping audit` does upkeep then one audit
  rotation, while a bare `/housekeeping` does upkeep only.
  (Unlike the old finding-cap argument, `/audit` is
  itself finite — one rotation, no cap — so the flag only
  decides *whether* to run it, not how much.)
- **The `cspell` flag** — when the invocation includes
  `cspell` (e.g. `housekeeping cspell` or
  `housekeeping audit cspell`, and likewise under
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
annotating the Linear Session Metrics doc with
recommended dispositions (it never edits a skill
unattended). When given the `audit` flag, its last step
runs one `/audit` rotation (step 11), but housekeeping
itself makes no source edit — the rotation only files
Linear issues.

Run it **once when you arrive with the `audit` flag**
(e.g. `housekeeping audit`) for the full morning-driver
flow (it runs one audit rotation and exits), or with no
flag for upkeep only, or drive ad-hoc upkeep on a
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
see [context economy](context-economy.md)). `gh pr list`
has a `merged` state filter the MCP lacks and `--json`
selects just the three fields the decision needs; it's a
`--json` **flag**, not a pipe, so it reduces to the
already-pre-approved `Bash(gh pr list:*)` read-rule (see
`docs/conventions/github-mcp.md`):

```sh
gh pr list --state merged --json number,headRefName,mergedAt --limit 100
```

Build a local set of the returned `headRefName`s — those
are the branches whose PR **merged**. Then, for every
worktree in the porcelain list **other than** the
`refs/heads/main` base, take its literal path and branch
and decide from that set (no per-branch network call):

- the worktree's branch **is** in the merged set → the
  branch is done. Remove
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

- the branch is **not** in the merged set (PR still open,
  closed without merging, or no PR exists), or the removal
  refused on a dirty worktree → **leave it**.
  Closed-without-merge and dirty worktrees are not safe
  to drop automatically; list them in the report so the
  user can decide.

After processing them all, tidy any stale worktree
admin entries:

```sh
git worktree prune
```

(The prune sequence — the `remove` / `branch -D` / `prune`
trio per merged branch — is a settled,
repeated deterministic shape; if it grows, it's a candidate
to harden into a `.claude/tools/` Python helper per
`CLAUDE.md` → "Skill tooling", leaving the skill to report
the tally.)

**Then clear notifications for merged PRs.** Merged PRs
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

- `merged_at` is **non-null** → dismiss that one
  notification:

  ```txt
  mcp__github__dismiss_notification(
    threadID: "<notification id>",
    state: "read",
  )
  ```

- `merged_at` is null (open or closed-unmerged), or the
  subject isn't a PR → **leave it**.

**Never** call `mark_all_notifications_read` — that would
clear unread mentions, review requests, and other non-merge
notifications too. Only a confirmed-merged PR's
notification is dismissed.

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
open aggregated task rather than opening a second — and
writes each consumed
entry's disposition back into the doc. `trim-context` has
**no** attended / propose-only split — filing a task *is*
the proposal, so it never edits a skill or convention
doc. At the end of its run `trim-context` also offers (via
`AskUserQuestion`) to clear the now-processed inbox
entries; that prompt lives in `trim-context` itself, so
this step **inherits** it through the delegation — don't
re-implement it here. If `LINEAR_SESSION_METRICS_DOC_ID`
is unset, `trim-context` says so and this step is a no-op.

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

**6. Reconcile blocking edges.** Invoke the
`sync-blockers` skill (via the Skill tool) to run a
**full sweep** over the open Backlog, filing any
file-overlap `blocks` edge that isn't already declared.
The deterministic Python tool does all the work in its own
process; this skill just triggers it and reports the
one-line tally. This sweep is a **catch-up**, not the
primary mechanism: the filing skills (`linear-task`,
`audit`, `audit-scope`, `merge-tasks`) already file each
new issue's overlap edges at file time via
`sync_blockers.py --for`, so the sweep only picks up edges
a `**Touches**:` line backfilled onto an *older* issue
would newly imply. It needs `LINEAR_API_KEY` /
`LINEAR_PROJECT_ID`; if either is unset, skip it and say so.

**7. Audit the base-repo permission allowlist for cruft.**
`firm-perms` only ever **adds** to
`<base>/.claude/settings.local.json` (unions, generalizes),
never prunes — so dead weight accumulates. Review the base
allowlist's `allow` array (the ~244-entry set; `<base>` was
resolved in step 1) for entries that shouldn't be there:

- **over-broad grants** — a bare `Bash(:*)`, an unscoped
  `Read(…)` / `Edit(…)` root, or a wildcard that subsumes
  many narrower rules;
- **secrets or absolute machine paths** that leaked into a
  rule;
- **dangerous one-offs** — `rm -rf`, `curl … | sh`,
  `git push --force`;
- **stale single-use commands** a stable prefix already
  covers (the dead weight `firm-perms` never removes).

**Autonomy bound: propose, never auto-delete.** Dropping a
permission is low-blast-radius, but silently editing the
allowlist unattended is surprising. In an **attended** pass,
surface the shortlist via **`AskUserQuestion`** and remove
(with the Edit tool, per the JSON-editing convention) only
the entries the human approves; in an **unattended** pass,
file the candidates **propose-only** (or just list them) and
delete nothing. This is the pruning half; `firm-perms` is
the add-only half, and the allowlist is `settings.local.json`
(git-ignored per the settings.json decision). **Keep the
full file out of context:** a `.claude/tools/` helper that
parses the allowlist and returns only the suspicious
shortlist is the intended hardening (per `CLAUDE.md` → "Skill
tooling") — until it exists, read the file once in this
step's own reasoning and report only the flagged entries,
never the whole 244-entry array.

**8. Review saved auto-memory for staleness.** The saved
auto-memory (`~/.claude/projects/<slug>/memory/*.md` plus the
`MEMORY.md` index) accretes; curate it for freshness. Read
the memory bodies **in this step's own pass** and flag a
memory as stale when it:

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

**9. Offer a session-metrics run.** The morning pass both
*mines* the Session Metrics inbox (step 4) and can
*contribute* to it: offer, via **`AskUserQuestion`** with
the recommended default **first**, to run `/session-metrics`
for the **current** session so this pass also appends a
fresh measured entry (the producer side of the loop).
Run it only on an explicit yes. Because a `session-metrics`
run **appends** a new unprocessed entry, this offer comes
*after* step 4's mine-and-clear, so the clear in step 4 is
evaluated against the inbox state before this append. (In
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

**11. Run one audit rotation (only when the `audit` flag was
passed).** The morning's last act: with upkeep done, the
**`audit` flag decides whether to run a rotation** —
passing it *is* the go-ahead, so there is no separate
prompt (this is the one handoff in the suite that's
arg-gated rather than `AskUserQuestion`-gated, precisely
because the flag carries the intent).

- **The `audit` flag was passed** (`housekeeping audit`) →
  invoke the `audit` skill (via the Skill tool) **once**.
  `/audit` is finite — a single seven-unit rotation that
  files its findings (syncing each one's overlap edges via
  `sync-blockers --for` as it goes), fires a high-severity
  `PushNotification` only when something warrants
  interrupting you, and stops on its own with a `DONE`
  line. It runs **inline** (it's bounded, so there's no
  background campaign to wait on), then **this housekeeping
  pass exits**.
- **No `audit` flag** → skip the audit entirely and end
  the pass after the upkeep. (To run a rotation, re-run
  with the `audit` flag, or invoke `/audit` directly.)

**The kickoff is a one-shot, not a loop.** `/audit` is a
single bounded rotation, not a continuous campaign — it
files what its seven units surface and stops. To audit
again, run `housekeeping audit` (or `/audit`) again. The
rotation syncs each finding's overlap edges as it files
them; the next pass's step 6 is the full reconciliation
sweep.

**12. Report.** Print a short summary:

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
  notifications were dismissed.
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
  entries were consumed, and any levers skipped as
  already-handled — or why the step was skipped (e.g.
  a missing env var).
- Convention references: in sync, or the dangling
  `CLAUDE.md` / `docs/conventions/` references filed
  (with the ENG-### of the aggregated task).
- Blocking edges: the `sync-blockers` reconciliation
  sweep's one-line tally (backlog issue count + overlap
  edges filed), or why it was skipped (e.g. a missing env
  var).
- Permission allowlist: the base-repo `settings.local.json`
  entries flagged as cruft and, for an attended pass, which
  the human approved removing — or that it was clean.
- Auto-memory: the memory slugs flagged stale (with the
  one-line reason each) and, for an attended pass, which
  were purged — or that all are fresh.
- Session metrics run: whether a `/session-metrics` run
  was offered and accepted for this session, or skipped.
- Purge-conversations: whether a `/purge-conversations` run
  was offered and accepted (with the MB freed), or skipped.
- Audit: one `/audit` rotation ran inline (with its
  `DONE` tally), or was skipped because the `audit` flag
  wasn't passed.
