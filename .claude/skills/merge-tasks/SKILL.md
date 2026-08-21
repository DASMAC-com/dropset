---
name: merge-tasks
description: Consolidate several Linear issues into one, given their numbers. Folds each non-survivor's body into the lowest-numbered survivor as a labeled # Part section (preserving every Fingerprint), unions the Touches globs, carries relatedTo append-only while surfacing every inherited blockedBy/blocks as a proposal for the user to approve (blocking is human-curated), applies the Claude: prefix when every issue is meta-work, and cancels the folded issues as duplicateOf the survivor. Files no collision links — the automated file-overlap machinery is retired. Confirms the plan via AskUserQuestion before any write. The deterministic parsing/assembly lives in the merge_tasks.py tool.
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

- **Append-only on relations, and it never carries a
  blocking edge unasked.** It unions `relatedTo` onto the
  survivor freely and never clears an existing edge. An
  inherited `blockedBy` / `blocks` is a **proposal** shown in
  the step-4 plan — carrying it would be the automation
  redirecting a human-placed edge onto an issue the human
  never judged it against (per `CLAUDE.md` → "Blocking
  relations").

- **Never drops a `**Fingerprint**:` line** — each folded
  body is preserved verbatim under its `# Part` heading, so
  the per-lever dedup keys all survive.

- **Unions, never overwrites, `**Touches**:`** — the merged
  issue carries one consolidated `**Touches**:` line that
  is the union of every folded issue's globs.

- **Confirms before any write** (see step 4). Nothing is
  mutated until the human approves the plan.

- **Won't grow a survivor past the point of being
  readable.** When the survivor's body is already large (past
  roughly **20KB**, or a dozen-plus `# Part` sections), say so
  at the step-4 confirmation and recommend **splitting**
  rather than growing: land what's there, or keep the
  aggregate's detail in a repo doc with the issue carrying
  only pointers and the `**Fingerprint**:` lines.

  The reason is **human**, not mechanical. An earlier version
  of this rule rested on cost and transcription risk — every
  fold re-emitting the survivor's entire description, since
  "`save_issue` replaces `description` wholesale and Linear
  has no append API". The second half of that was wrong:
  `save_issue` takes a `patch` array, so a fold *can* add a
  `# Part` without re-sending the body, and the server applies
  it atomically (no corruption risk) — see
  `docs/conventions/linear-automation.md` → "Partial edits —
  the `patch` argument". The advice survives anyway on the
  merit that actually matters: a 28KB issue with a dozen
  `# Part` sections is a bad artifact for a human to read,
  prioritize, or scope a PR from, however cheaply it was
  assembled. Recommend the split for that reason, and don't
  claim a cost argument that no longer holds.

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

Then run the tool over it, passing **both** `--out` and
`--ops-out` so neither large payload is echoed to stdout
(per `CLAUDE.md` → "Context economy"):

```sh
python3 .claude/tools/merge_tasks.py assemble /tmp/merge-tasks.json \
  --out /tmp/merge-body.md --ops-out /tmp/merge-ops.json
```

It returns the metadata inline — `title`, `touches`,
`all_meta`, `cross_area` — plus a path to each of the two
ways it expressed the fold:

- **`patch_ops_path`** (+ `patch_ops_count`) — the fold as
  Linear `patch` operations: one `append` per `# Part`
  section, and one `replace` swapping the survivor's
  `**Touches**:` line for the union. **Prefer this.** The
  ops carry only the *folded* bodies, so the survivor's own
  text — 28KB is unremarkable — is never re-sent at all.
  `null` when no safe anchor exists, with
  `patch_fallback_reason` naming the rule it tripped (two
  `**Touches**:` lines, an `ENG-###` in the anchor, or over
  Linear's 50-op cap).
- **`description_path`** — the whole merged body, the
  wholesale fallback for exactly that case.

The merged body is the survivor body + each non-survivor
folded as a `# Part N — <title>` section (every fingerprint
preserved, one consolidated `**Touches**:` line); the `title`
carries the **`Claude:`** prefix when `all_meta` is true (per
`CLAUDE.md` → "Claude: meta-work prefix"), and `cross_area`
is set when the merge mixes meta-work with product code.
Neither payload ever transits context as a tool result — in
step 5 you `Read` whichever file you're going to use.

Union the relations yourself (a plain set union the tool
doesn't need the network for): collect every
`blockedBy` / `blocks` / `relatedTo` id across all the
fetched issues, and **drop any that point at one of the
issues being merged** (a folded issue must not end up
blocking the survivor).

Then split what remains, because the two kinds are not
carried the same way:

- **`relatedTo`** transits freely — it gates nothing, so
  carrying it forward costs nothing.
- **`blockedBy` / `blocks` do not transit silently.**
  Blocking is **human-curated** (`CLAUDE.md` → "Blocking
  relations"), and carrying an inherited edge onto the
  survivor is the automation **redirecting a human-placed
  edge** onto an issue the human never placed it on — with a
  wider `**Touches**:` union than the edge was ever judged
  against. So treat every inherited blocking edge as a
  **proposal**: list it in the step-4 plan, naming which
  folded issue it came from, and pass it only for the ones
  the user approves. Unapproved edges are recorded as prose
  in the survivor's body (`**Suspected dependency**: …`), so
  the ordering claim survives the merge even when the edge
  doesn't.

**4. Confirm the plan — via `AskUserQuestion`.** Before any
write, show the plan and wait for the go-ahead (the same
TUI-selector pattern the other skill handoffs use):

- the chosen **survivor** and the issues folding into it,
- the union of the `**Touches**:` globs,
- the resulting title (note when the `Claude:` prefix is
  applied),
- every **inherited blocking edge** (per step 3), each
  naming the folded issue it came from, so the user can say
  which carry over to the survivor — the default for any
  edge not explicitly approved is **not carried**, and
- a **cross-area warning** when `cross_area` is true — the
  issues span unrelated surfaces (meta-work mixed with
  product / on-chain code), so the merge may not be
  intended; surface it so the user can confirm, and
- an **oversized-survivor warning** when the merged body
  exceeds roughly **20KB** — per "What it does — and does
  not" above, a survivor this large stops being a readable
  artifact for whoever has to prioritize it and scope a PR
  from it. Name the size and recommend **splitting** instead
  of growing; that makes "cancel" the honest default for this
  one case, so say which way you'd go.

Offer "yes, merge" (**first**, the recommended default) and
"cancel". Proceed only on an explicit yes.

**5. Write the survivor, then cancel the rest.** On
approval:

- Update the survivor with `mcp__claude_ai_Linear__save_issue`
  (id = survivor) — the new `title`, the body, the
  `relatedTo` union, and **only the blocking edges the user
  approved** in step 4 (these args are append-only, so
  passing them is safe).

  **Prefer the `patch` path.** When step 3 reported a
  `patch_ops_path`, `Read` that file and pass its array as
  **`patch`** — the survivor's existing body is then never
  re-sent, only the folded parts and one short anchor. Never
  pass `patch` alongside `description`; they are alternatives
  (per `docs/conventions/linear-automation.md` → "Partial
  edits"). Note the `title` still goes as an ordinary
  argument — `patch` governs the body only.

  **Fall back to wholesale** when `patch_ops_path` is `null`:
  `Read` `description_path` and pass its contents as
  `description`. `patch_fallback_reason` says why the anchor
  couldn't be made safe; relay it in the step-6 report so a
  recurring cause (e.g. survivors accumulating a second
  `**Touches**:` line) is visible rather than silent.

- For **each** non-survivor, `save_issue` (id = that issue)
  with `state: "Canceled"` and `duplicateOf: "<survivor>"`,
  so the board shows it folded into the survivor.

**6. Report.** One line: the survivor (with its final
title), the issues folded in and canceled, and which
inherited blocking edges were carried (and which were left
as prose).

**No collision step.** The survivor's `**Touches**:` is now
the union of every folded issue's, which is wider than any
one of them — but nothing records overlap: the automated
file-collision machinery is retired. A wider union is worth
mentioning in the report as prose, since it is a signal for
the next consolidation pass, but it produces no relation
write. Reconciling overlap is planning-session work.

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
  only to Linear (the survivor update and the
  cancellations). It authors no code or skill diff, and
  never commits or pushes.
- **Shell discipline** (per `docs/conventions/shell-commands.md`):
  every command is a single bare call that reduces to an
  allow-glob — the tool calls match
  `Bash(python3 .claude/tools/*)`; pass the issues JSON
  through a file, never a heredoc or pipe.
