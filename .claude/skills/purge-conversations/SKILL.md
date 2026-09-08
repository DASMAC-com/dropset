---
name: purge-conversations
description: Reclaim disk from Claude Code's local state — old session transcripts (`~/.claude/projects`), the file-history store (`~/.claude/file-history`), and the CLI cache (`~/Library/Caches/claude-cli-nodejs`) — under an age rule with open-PR protection. The deterministic filesystem logic lives in the committed `.claude/tools/prune_conversations.py` (dry-run by default, hard-delete only on `--apply`); this skill drives the GitHub PR lookups over the MCP, hands the tool the worktree branches with an open PR, shows the grouped dry-run manifest, gets one approval via AskUserQuestion, then applies and reports bytes freed. Offered by `housekeeping`; nothing is deleted without an explicit yes.
disable-model-invocation: false
user-invocable: true
---

# `purge-conversations`

Reclaim disk from Claude Code's **local state** — the
session transcripts and adjacent caches that pile up as
you work — with an **age rule** guarded by **open-PR
protection**, so an active line of work is never dropped.
The deterministic filesystem logic lives in the committed
`.claude/tools/prune_conversations.py`; this skill drives
the GitHub reads, gets one approval, and reports what was
freed.

## Motivation (measured 2026-07-01)

`~/.claude` was **653M**, of which the transcripts under
`~/.claude/projects` were **598M** (92%). Adjacent sinks:
`~/.claude/file-history` **40M**, the CLI cache
`~/Library/Caches/claude-cli-nodejs` **41M**. Realistic
reclaim: ~600M.

The non-obvious finding: **almost all of the 598M is
dropset itself** (~596M) — the other project dirs are
near-empty shells. Within dropset, the **base-repo project
dir alone was 151M** (main / housekeeping sessions, never
PR-linked). So the real lever is the age rule on dropset,
not a non-dropset sweep.

**That 151M is no longer reclaimable here, deliberately.**
The base checkout is a live worktree, so it is now kept
unconditionally. In practice we expect this to change
nothing: that one directory holds *every* session ever
started from the base repo, so its mtime should stay fresh
and the age rule is unlikely ever to have fired on it — the
outcome the same, stated as "live worktree" rather than
"within age". **That is reasoning about mtime, not a
measurement over months** — unlike the verified claims
elsewhere in this file, and worth reading as the weaker
thing it is. If the directory does grow without bound, the
fix is a per-session rule rather than a whole-slug age rule;
do not reach for it by dropping the live-worktree
protection.

## Two mechanisms, three roots

1. **Slug-partitioned** — `~/.claude/projects` **and** the
   CLI cache `~/Library/Caches/claude-cli-nodejs` both name
   a subdirectory per working directory with the same
   `slugify()` scheme (every `/` and `.` → `-`, shared with
   `session_metrics.py`). Five keep-rules are tried in order —
   current session, open PR, **live worktree**, **recent prompt
   activity**, then the age grace period — and only a slug that
   **fails** all of them is deleted. A slug whose **worktree still
   exists** is therefore kept unconditionally, whatever its age
   and whatever its PR says. The CLI cache also carries stale
   slugs for dead repos — good reclaim.

   Two riders on that summary, because "worktree gone ⇒ age
   rule" is *necessary* but not *sufficient*: recent prompt
   activity keeps a gone-worktree slug anyway, and a slug the
   caller marks **completed** is deleted outright, skipping the
   grace period. The activity rule applies to **every** slug, so
   a non-dropset slug is not age-only either.

1. **Session-UUID** — `~/.claude/file-history` is one flat
   subdirectory per session UUID, mixing every repo, so it
   can't be cheaply repo-scoped by name. It is age-ruled, but
   **joined back** to the projects tree first: each session is
   stored there as `<slug>/<uuid>.jsonl`, so a session
   belonging to a slug we are keeping is kept here too.

The **current session is always kept** in every root — by
the current working directory's slug, and (belt-and-braces)
by session id when known. Its dirs are freshly written
anyway, so the age rule keeps them regardless.

## The dropset ↔ open-PR join

Derive the dropset set **forward, never by inverting
slugs** — a string prefix would wrongly catch a sibling
repo like `dropset-beta`, whose slug starts with the base
repo's. The tool runs `git worktree list --porcelain` from
`--dropset-repo` and takes the slug of each real worktree
path → the dropset slug set.

**An existing worktree is protection in itself**, whatever
the PR says and whatever the age: a worktree on disk means a
session someone intends to resume. The PR lookup this skill
performs is now **belt-and-braces on top of that**, not the
load-bearing protection it used to be. It still earns its
place — "open PR" is the more useful reason to show a human —
but a failure of this lookup can no longer reach a live
session.

Be precise about how much it adds, because the obvious
stronger claim is false: the tool builds the protected set
and the live set from the **same** worktree list, so the
protected set is a subset of it. The open-PR rule therefore
changes no keep/delete outcome today — only the reason
string. In particular it does **not** rescue a branch whose
worktree is already gone: such a branch contributes no
worktree entry, so it cannot be protected either. The tool
warns when a `--protected-branch` matches no live worktree,
and that warning is the only trace it leaves.

**`state: "open"` already includes drafts.** Worth stating
because the first theory of the 2026-09-07 loss was that
drafts were being excluded, and that theory is wrong: the
REST `state` filter is open/closed only, with `draft` a
separate field on the result. Verified directly — a draft PR
queried with `state: "open"` comes back with
`"state": "open", "draft": true`. Do not "fix" this.

Only a slug whose **worktree is gone** reaches the age rule.
Such a slug is now reported as its own category rather than
folded in with `non-dropset`, and the manifest **names its
issue tag** — see the incident note under "Safety
invariant".

## Decisions locked

- **Hard `rm`, no trash retention.** Move-to-trash was
  rejected (it doesn't free space until emptied). Instead:
  print a **dry-run manifest** grouped by category
  (completed / dropset-old / non-dropset / file-history /
  CLI-cache) with per-group and total MB, **each dropset slug
  named by its issue tag**, plus the kept count
  **broken out by reason**, get
  **one** approval via `AskUserQuestion`, then hard-delete
  and **report bytes freed**.
- **Age threshold default 2 days** (the tool's
  `--age-days`), applied to all three roots.
- **No new env var.** The projects root comes from
  `$CLAUDE_CONFIG_DIR` / `$HOME` (the tool reuses
  `claude_home()`); the dropset root is discovered at
  runtime from `git worktree list`; the slug is `slugify()`.
- **`housekeeping` offers it** via `AskUserQuestion` (per
  its step-9 `/session-metrics` offer pattern) — never an
  unattended auto-step; nothing deletes without a yes.

## Safety invariant

The tool only ever deletes a directory that resolves
**under** one of the three known roots, **never follows a
symlink**, refuses any entry that escapes its root, and
never touches the current session. **Dry-run is the
default**; deletion requires the explicit `--apply` this
skill passes only after approval.

### The 2026-09-07 loss, and why the invocation is now fixed

A `housekeeping` pass invoked the tool as
`--protected-branch eng-1192 --current-session … --apply`
with **no `--dropset-repo`**. That argument was optional, and
omitting it made the worktree lookup return an empty list —
so no slug was recognized as dropset, the protected branch
could not be mapped to any slug, and **every** worktree slug
older than two days was deleted as "non-dropset transcripts".
One live session's transcript went with it. The approval
prompt read `8.3 MB non-dropset transcripts`, so what the
operator approved bore no resemblance to what was lost.

Two failures, both now closed in the tool:

- **The invocation drifted.** Earlier runs passed the repo;
  that one did not. The argument is now **defaulted from the
  tool's own committed location** — deliberately *not* from
  the working directory, which would only move the hole: run
  from inside any unrelated checkout and the cwd resolves
  fine, to the wrong repo. The run **aborts** when no repo
  resolves, and an explicit path naming a *different*
  repository is refused too, so there is no longer an
  invocation that silently protects nothing.
- **The tool degraded silently.** An empty worktree list now
  aborts rather than sweeping.

Two further changes make a bad manifest visible rather than
merely unlikely: every dropset slug proposed for deletion is
**named by its issue tag** on its own line, and the prompt
history is cross-checked so a slug with recent activity is
kept even if the PR lookup returns nothing at all. Both of a
session's directories — the transcript and its `file-history`
— are now protected together, since the loss took both.

**Still pass `--dropset-repo` explicitly** in the commands
below. The default is a backstop against the next drift, not
a license to stop being explicit.

## Steps

**1. Resolve the dropset repo and its worktree branches.**
List the worktrees from the base repo and read the paths /
branches out of the porcelain output yourself (no command
substitution):

```sh
git worktree list --porcelain
```

The worktree whose `branch` is `refs/heads/main` is the
base repo (`<dropset-repo>`). Collect the other worktrees'
branch names.

**2. Find which branches have an open PR.** For each
worktree branch, read its PR through the GitHub MCP (this
repo is `DASMAC-com/dropset`; the `head` filter is
`owner:branch`) and note the ones whose PR is **open**:

```txt
mcp__github__list_pull_requests(
  owner: "DASMAC-com",
  repo: "dropset",
  head: "DASMAC-com:<branch>",
  state: "open",
)
```

A branch with a returned open PR is **protected**; a branch
with none (merged, closed, or never opened) is not. Drafts
count as open — see "The dropset ↔ open-PR join" above; that
is a property of the API's `state` filter, verified, not an
assumption.

**If this lookup fails, say so and carry on** — do not fall
back to running the tool without `--protected-branch`. An
existing worktree protects itself now, so a failed lookup
costs a *reason string* in the manifest rather than a
session; what it must never do is become a reason to invoke
the tool differently.

**3. Dry-run the prune.** Run the tool with the dropset repo,
one `--protected-branch` per open-PR branch, and the current
session id if known — a single bare command reducing to the
`Bash(python3 .claude/tools/*)` allow-rule:

```sh
python3 .claude/tools/prune_conversations.py \
  --dropset-repo <path> \
  --protected-branch <b1> --protected-branch <b2> \
  --completed-slug <s1> \
  --current-session <uuid>
```

It prints the grouped manifest — per-group and total MB, and
the kept count **broken out by reason** — and deletes
**nothing**. (Omit `--current-session` if the id isn't to
hand; the active session's dirs are protected by their slug
and their fresh mtime regardless.)

**Pass `--completed-slug` for finished work.** A slug whose
worktree no longer exists **and** whose branch's PR is merged
or closed is done, so the age grace period protects nothing —
this flag deletes it without waiting. Step 2 already computes
exactly that set one step earlier; it simply was not being
handed to the tool. An open PR still wins over a completed
marking, so a mistake in the set arithmetic costs disk rather
than data.

**Read the breakout, not the total.** One dry run reported
"41 protected", which read as open-PR protection — but only
**four** records were actually open-PR-protected and the rest
were the blunt two-day age rule across three roots. Those are
different facts: one is work in flight, the other is a grace
period, and only the first is a reason not to reclaim the
space.

The breakout now names more than those two, and they carry
very different weight. Five reasons mean work someone can
still come back to:

- `current session`
- `open PR`
- `live worktree`
- `recent session activity`
- `session of a kept project`

Only the age rule is the blunt grace period, and it prints as
more than one string:

- `within age`
- `worktree gone, within age`
- `non-dropset, within age`

A run whose kept count is mostly age-rule reasons is
reclaimable; one dominated by the other five is correctly
protected.

**Read the named tags before approving anything.** Each
dropset slug proposed for deletion prints on its own line as
`- eng-1192 (2.1 MB)`. That listing is the check that would
have caught the 2026-09-07 loss, and it only works if it is
actually read: **check every named tag against the worktrees
and issues you know are still live**, and if any one of them
is work you recognize as unfinished, answer **no** and
investigate before re-running.

**Every** directory is named, not only the tagged ones — an
entry with no tag prints its directory name instead, because
a wrongly resolved repo would otherwise strand real sessions
in an anonymous bulk line. Tags are never truncated; only a
long tail of *untagged* entries is ever summarized away, so a
remainder line never conceals a dropset session.

**4. Approve, then apply.** Show the manifest and ask via
**`AskUserQuestion`** whether to hard-delete it (recommended
option **first**, e.g. "yes, free ~X MB"; the other "no,
keep everything"). Only on an explicit yes, re-run with
`--apply` added:

```sh
python3 .claude/tools/prune_conversations.py \
  --dropset-repo <path> \
  --protected-branch <b1> \
  --current-session <uuid> --apply
```

**5. Report.** Print the tool's final line — dirs deleted
and bytes freed — or, on "no", that nothing was deleted.

## Notes

- **Distinct from the memory-freshness step.** `housekeeping`
  step 8 curates the auto-memory *knowledge store* for
  staleness; this skill reclaims *disk* from transcripts and
  caches. Different targets, different rules.
- Shell discipline (per `CLAUDE.md`): every command is a
  single bare call that reduces to an allow-glob — no `&&`,
  pipes, `$(...)`, or redirects.
