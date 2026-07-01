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
PR-linked). So the real lever is the age + open-PR rule on
dropset, not a non-dropset sweep.

## Two mechanisms, three roots

1. **Slug-partitioned** — `~/.claude/projects` **and** the
   CLI cache `~/Library/Caches/claude-cli-nodejs` both name
   a subdirectory per working directory with the same
   `slugify()` scheme (every `/` and `.` → `-`, shared with
   `session_metrics.py`). A **dropset** slug gets the age
   rule **unless** its worktree branch has an **open PR**
   (kept regardless of age); every **non-dropset** slug is
   age-only. The CLI cache also carries stale slugs for dead
   repos — good reclaim.
1. **Session-UUID** — `~/.claude/file-history` is one flat
   subdirectory per session UUID, mixing every repo, so it
   can't be cheaply repo-scoped: **age-only** by directory
   mtime.

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
path → the dropset slug set. This skill supplies the **open-PR**
branches: for each worktree branch, read its PR via the
GitHub MCP and pass `--protected-branch <branch>` for every
one whose PR is **open** (not merged, not closed). The
base-repo dir and pruned-worktree dirs have no open PR, so
the age rule applies to them.

## Decisions locked

- **Hard `rm`, no trash retention.** Move-to-trash was
  rejected (it doesn't free space until emptied). Instead:
  print a **dry-run manifest** grouped by category
  (dropset-old / non-dropset / file-history / CLI-cache)
  with per-group and total MB plus the protected count, get
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
with none (merged, closed, or never opened) is not.

**3. Dry-run the prune.** Run the tool with the dropset repo,
one `--protected-branch` per open-PR branch, and the current
session id if known — a single bare command reducing to the
`Bash(python3 .claude/tools/*)` allow-rule:

```sh
python3 .claude/tools/prune_conversations.py \
  --dropset-repo <path> \
  --protected-branch <b1> --protected-branch <b2> \
  --current-session <uuid>
```

It prints the grouped manifest — per-group and total MB, and
the protected count — and deletes **nothing**. (Omit
`--current-session` if the id isn't to hand; the active
session's dirs are protected by their slug and their fresh
mtime regardless.)

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
