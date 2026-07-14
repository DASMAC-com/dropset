---
name: merge-tasks
description: Consolidate several Linear issues into one, given their numbers. Folds each non-survivor's body into the lowest-numbered survivor as a labeled # Part section (preserving every Fingerprint), unions the Touches globs, carries blockedBy/blocks/relatedTo relations append-only, applies the Claude: prefix when every issue is meta-work, cancels the folded issues as duplicateOf the survivor, and syncs the survivor's file-overlap blocking edges via sync-blockers `--for`. Confirms the plan via AskUserQuestion before any write. The deterministic parsing/assembly lives in the merge_tasks.py tool.
user-invocable: true
---

# `merge-tasks`

Consolidate several Linear issues into one — codifying the
manual fold done by hand (e.g. rolling a cluster of
`Claude:` agent-infra issues into a single mega-task). The
**deterministic** parts (number parsing/dedup, survivor
resolution, body-section assembly, the `**Touches**:`
union, and the `Claude:`-prefix decision) live in the
committed Python tool `.claude/tools/merge_tasks.py` (per
`CLAUDE.md` → "Skill tooling"); the skill drives the Linear
MCP reads and writes around it.

## Input

The issue numbers to merge — bare (`615`) or tagged
(`ENG-615`), in any order, **deduped**:
`/merge-tasks 615 622 623 624`. The **survivor** (the issue
the rest fold into) defaults to the **lowest-numbered**;
to override, the user names one explicitly (e.g. "merge
622 623 into 624").

## What it does — and does not

- **Append-only on relations.** It unions
  `blockedBy` / `blocks` / `relatedTo` onto the survivor;
  it never clears an existing edge.
- **Never drops a `**Fingerprint**:` line** — each folded
  body is preserved verbatim under its `# Part` heading, so
  the per-lever dedup keys all survive.
- **Unions, never overwrites, `**Touches**:`** — the merged
  issue carries one consolidated `**Touches**:` line that
  is the union of every folded issue's globs.
- **Confirms before any write** (see step 4). Nothing is
  mutated until the human approves the plan.

## Steps

**1. Resolve the survivor and the deduped set.** Pass the
user's tokens to the tool (add `--survivor N` only if the
user named one); it parses, dedups, and picks the survivor:

```sh
python3 .claude/tools/merge_tasks.py plan 615 622 623 624
```

It prints `{"survivor": "ENG-###", "ids": [...]}`. If the
user named a survivor, append `--survivor <number>`. The
tool errors if fewer than two distinct issues remain.

**2. Fetch each issue once.** For every id in `ids`, call
`mcp__claude_ai_Linear__get_issue` with
`includeRelations: true` — one fetch per issue, no reloads
(context-cheap). Keep each issue's `title`, `description`,
and its `blockedBy` / `blocks` / `relatedTo` relations.

**3. Assemble the merged issue.** Write the fetched issues
to a temp JSON file with the **Write** tool (the
file-handoff pattern from `CLAUDE.md` → "Shell commands";
no heredoc) — shape:

```json
{
  "survivor": "ENG-615",
  "issues": [
    {"id": "ENG-615", "number": 615, "title": "…", "description": "…"},
    {"id": "ENG-622", "number": 622, "title": "…", "description": "…"}
  ]
}
```

Then run the tool over it:

```sh
python3 .claude/tools/merge_tasks.py assemble /tmp/merge-tasks.json
```

It returns the keys `title`, `description`, `touches`,
`all_meta`, and `cross_area`: the merged `description`
(survivor body + each non-survivor folded as a
`# Part N — <title>` section,
every fingerprint preserved, one consolidated
`**Touches**:` line), the `title` with the **`Claude:`**
prefix applied when `all_meta` is true (per `CLAUDE.md` →
"Claude: meta-work prefix"), and `cross_area` set when the
merge mixes meta-work with product code.

Union the relations yourself (a plain set union the tool
doesn't need the network for): collect every
`blockedBy` / `blocks` / `relatedTo` id across all the
fetched issues, and **drop any that point at one of the
issues being merged** (a folded issue must not end up
blocking the survivor). The remainder is what the survivor
gets, append-only.

**4. Confirm the plan — via `AskUserQuestion`.** Before any
write, show the plan and wait for the go-ahead (the same
TUI-selector pattern the other skill handoffs use):

- the chosen **survivor** and the issues folding into it,
- the union of the `**Touches**:` globs,
- the resulting title (note when the `Claude:` prefix is
  applied), and
- a **cross-area warning** when `cross_area` is true — the
  issues span unrelated surfaces (meta-work mixed with
  product / on-chain code), so the merge may not be
  intended; surface it so the user can confirm.

Offer "yes, merge" (**first**, the recommended default) and
"cancel". Proceed only on an explicit yes.

**5. Write the survivor, then cancel the rest.** On
approval:

- Update the survivor with `mcp__claude_ai_Linear__save_issue`
  (id = survivor) — the new `title` and `description`, plus
  the union of `blockedBy` / `blocks` / `relatedTo` (these
  args are append-only, so passing the union is safe).
- For **each** non-survivor, `save_issue` (id = that issue)
  with `state: "Canceled"` and `duplicateOf: "<survivor>"`,
  so the board shows it folded into the survivor.

**6. Sync the survivor's blocking edges.** The survivor's
`**Touches**:` is now the union of every folded issue's, so
run the incremental sweep on it to file any new file-overlap
`blocks` edges against the open Backlog — one bare command
that reduces to the
`Bash(python3 .claude/tools/sync_blockers.py:*)`
allow-rule:

```sh
python3 .claude/tools/sync_blockers.py --for <survivor>
```

Best-effort: it needs `LINEAR_API_KEY` / `LINEAR_PROJECT_ID`;
if either is unset, note it and continue. The canceled
non-survivors drop out of the open Backlog on their own, so
their stale overlap edges no longer gate anything.

**7. Report.** One line: the survivor (with its final
title), the issues folded in and canceled, and that its
blocking edges were synced.

## Notes

- **This skill is how aggressive folding lands on the
  board.** The filing/audit default is to file the **fewest
  coherent PRs** (`docs/conventions/linear-automation.md` →
  "Fold coupled findings into one issue"); when coupled
  issues nonetheless landed separately, `merge-tasks` folds
  them back into one. `housekeeping` proactively proposes
  such merge groups. The **coherence floor** — never fold
  across separate apps, languages, or deploy units — is
  enforced by the `cross_area` warning in step 4: don't
  confirm a merge that mixes unrelated surfaces.
- **Read-only with respect to source.** This skill writes
  only to Linear (the survivor update, the cancellations,
  and the survivor's `sync-blockers` overlap edges). It
  authors no code or skill diff, and never commits or pushes.
- **Shell discipline** (per `docs/conventions/shell-commands.md`):
  every command is a single bare call that reduces to an
  allow-glob — the tool calls match
  `Bash(python3 .claude/tools/*)`; pass the issues JSON
  through a file, never a heredoc or pipe.
