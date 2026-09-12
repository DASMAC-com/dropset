---
name: merge-tasks
description: Consolidate several Linear issues into one, given their numbers. Folds each non-survivor's body into the lowest-numbered survivor as a labeled # Part section (preserving every Fingerprint), carries a legacy Touches line forward as one consolidated union when the folded bodies have one (the field is retired, so nothing invents one), carries relatedTo append-only while surfacing every inherited blockedBy/blocks as a proposal for the user to approve (blocking is human-curated), applies the Claude: prefix when every issue is meta-work, and cancels the folded issues through the zero-echo field batch. Both writes stay off the body-echoing MCP path: the survivor's body goes through linear_patch.py and the cancels through board_batch.py. Files no collision links — the automated file-overlap machinery is retired. Confirms the plan via AskUserQuestion before any write. The deterministic parsing/assembly lives in the merge_tasks.py tool.
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

**The main caller is the planning bootstrap's meta-batch
assembly.** Once a day it sweeps the `Claude meta` parking
milestone and folds the parked strays into **small themed
batches** through this skill — so the `Claude:`-prefix decision
and the size bound below are on the hot path rather than being
edge cases. Two things that assembly expects of each survivor,
and that a caller should pass rather than fix up afterwards:

- It is filed **parked** — `Todo` plus `Claude meta`, like the
  strays it was folded from — and only a batch the planning
  session **promotes** is moved to **Backlog, Urgent**, which
  means **clearing the milestone as well as** changing state.
  Assembling and promoting are different acts now that a
  bootstrap produces several batches; filing every survivor
  straight to Backlog/Urgent would flood the operator's Next
  view with Urgent meta work.
- It carries **no blocking edge** — nothing serializes the
  batches at all now, neither a relation nor the retired
  assembly precondition.

See the `plan` skill, step 1.

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

- **Carries a legacy `**Touches**:` line without inventing
  one.** The declared-scope glob field is **retired** — no
  filing emits it any more (`CLAUDE.md` → "Structured filing
  fields"). Issues filed before that still have one, so when
  any folded body carries globs the merged issue keeps a
  single consolidated line holding their union, and when none
  does it gets no line at all. Never add globs by hand to
  give it one.

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

  **The `Claude:` meta class is bound too — the old exemption
  is RETIRED.** Operator rule, 2026-09-11, superseding the
  2026-08-24 no-size-bound exemption: meta work folds into
  **small themed batches of roughly 4–5 parts**, so this
  warning fires on a `Claude:`-prefixed survivor exactly as it
  does on any other.

  The exemption's own reasoning is what fell. It rested on a
  scheduling claim — that at most one meta task ever runs in
  flight, since they all contend on the same skill files, so a
  second issue buys no parallelism and costs a merge conflict.
  The contention is real; what changed is its price relative to
  the alternative. Session cost is roughly **quadratic in
  session length**
  (`docs/conventions/context-economy.md` → "Session length is
  itself a cost lever"), so serializing meta work into one
  unbounded batch buys that quadratic in exchange for avoiding
  a **rebase** — which inside a short session costs cents. The
  measured counterexample is the ENG-1194 batch: the whole
  parked pool in one issue, worked in a 19-hour, \$409 run.
  Churn speed through the self-improvement loop still matters
  most, and small batches are now how it is bought.

  The readability argument above therefore applies to meta work
  as well, rather than being outweighed by it. The **coherence
  floor** is untouched and still binds absolutely: meta work
  never folds together with product code, and a set that must
  land as one PR is never scattered — it splits into
  *sequential* PRs instead of growing past a short session.

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

**2. Fetch the bodies with the tool, not with `get_issue`.**
One bare command reads every body over GraphQL **in its own
process** and writes the file step 3 consumes:

```sh
python3 .claude/tools/merge_tasks.py fetch --survivor 615 \
  --out /tmp/merge-tasks.json 615 622 623 624
```

It prints identifiers and a byte count — never a body. **No
body transits context at any point in this step.**

That is the whole reason it exists. Folded bodies used to
transit context roughly **three** times per fold: once as
each `get_issue` echo, once as the hand-written `Write`
composing that same JSON, and once as the `Read` of the
generated ops. About **50k of one planning session's ~135k
output** was body re-emission. This removes the first two —
measured on a real pair, 68KB of body handled at zero
context cost.

The third is **structural and stays**: the ops go through
the MCP `patch` path, whose anchor matching with atomic
abort is load-bearing safety, and handing ops to an MCP call
means reading them. It shrank anyway — with `**Touches**:`
retired, a fold of post-retirement issues is appends only,
so there is no `replace` and no anchor.

**Relations still need `get_issue`.** The tool fetches
bodies, not relations. When the fold has to carry
`blockedBy` / `blocks` / `relatedTo` (step 3's union below),
call `mcp__claude_ai_Linear__get_issue` with
`includeRelations: true` for that — once per issue, no
reloads. Skip it entirely when no issue in the set has
relations worth carrying.

**3. Assemble the merged issue.** Run the tool over the file
step 2 wrote, passing **both** `--out` and
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
  section, plus — **only when a folded body carries legacy
  globs** — one `replace` swapping the survivor's
  `**Touches**:` line for the union. **Prefer this.** The
  ops carry only the *folded* bodies, so the survivor's own
  text — 28KB is unremarkable — is never re-sent at all.
  `null` when no safe anchor exists, with
  `patch_fallback_reason` naming the rule it tripped (two
  `**Touches**:` lines, an `ENG-###` in the anchor, or over
  Linear's 50-op cap). Since the field is retired, a fold of
  issues filed after that is **appends only** — no anchor, so
  none of those fallback cases can arise.
- **`description_path`** — the whole merged body, the
  wholesale fallback for exactly that case.

The merged body is the survivor body + each non-survivor
folded as a `# Part N — <title>` section (every fingerprint
preserved, and a consolidated `**Touches**:` line only if
some folded body had one); the `title`
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
  edge** onto an issue the human never placed it on — one
  with a wider scope than the edge was ever judged
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

- the consolidated `**Touches**:` union — **only when a folded
  body carries a legacy one**. The field is retired, so a fold
  of post-retirement issues has no union to show and this line
  is simply absent,

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

  **This warning applies to a `Claude:`-prefixed survivor as
  well.** The old skip-for-meta exemption is retired (operator
  rule, 2026-09-11): meta work is now size-bound like everything
  else, at roughly 4–5 parts per batch, so an oversized meta
  survivor is exactly the case worth flagging. See "What it does
  — and does not" above for why the one-in-flight reasoning that
  justified the exemption no longer holds.

Offer "yes, merge" (**first**, the recommended default) and
"cancel". Proceed only on an explicit yes.

**5. Write the survivor, then cancel the rest.** On
approval:

- Update the survivor with `mcp__claude_ai_Linear__save_issue`
  (id = survivor) — the new `title`, the body, the
  `relatedTo` union, and **only the blocking edges the user
  approved** in step 4 (these args are append-only, so
  passing them is safe).

  **Apply the body through the ZERO-ECHO patch tool, not the
  MCP.** When step 3 reported a `patch_ops_path`, hand that
  path straight to the committed writer — never `Read` the ops
  file to pass its array through the MCP:

  ```sh
  python3 .claude/tools/linear_patch.py patch \
    --ops <patch_ops_path> <survivor>
  ```

  Note the shape: `patch` is a **subcommand**, the identifier is
  **positional**, and `--ops` takes the ops **file path** (the
  tool reads it in its own process — that is what makes this
  zero-echo).

  It applies the same ops and prints only a size. Two
  body-sized transits disappear: reading the ops file into
  context, and the MCP's echo of the survivor's whole stored
  body. Measured on one fold — the ops file was **≈9.5k**, the
  session's second-largest result, and the `save_issue` echo
  **≈10.4k**, its largest; 24 `save_issue` calls totalled 35.1k
  and were the costliest tool of that session. A later fold's
  ops file reached **≈53k** (148 KB of folded bodies), where
  reading it to pass it through was simply infeasible.

  The MCP `patch`'s safety argument is preserved, not traded
  away: the tool does the same anchor matching with the same
  atomic abort, refusing the whole sequence if any single op
  cannot be applied.

  **The op vocabulary is settled** — `linear_patch.py` now
  accepts the MCP spellings (`old_string` / `new_string`,
  `from` / `to`) as aliases, so an assembled ops file applies
  either way. It did not always: a fold was once rejected on
  its first op with `'replace' needs a string 'text'`.

  **A post-retirement fold is appends only.** The single
  `replace` a fold can emit is the legacy `**Touches**:`
  consolidation, and `merge_tasks.py` emits it only when the
  survivor actually carries such a line — so a fold of issues
  filed since the field was retired produces no `replace` at
  all. That is why the alias above mattered: the one op whose
  vocabulary differed was also the only one a legacy fold
  needed.

  **Fall back to wholesale** when `patch_ops_path` is `null`:
  `Read` `description_path` and pass its contents as
  `description` through `save_issue`. `patch_fallback_reason`
  says why the anchor couldn't be made safe; relay it in the
  step-6 report so a recurring cause is visible rather than
  silent. Note the `title` goes through `board_batch.py fields`
  or the same `save_issue` — the patch tool governs the body
  only.

- **Cancel the non-survivors through the zero-echo field
  batch, in one call** — never a `save_issue` per issue:

  ```sh
  python3 .claude/tools/board_batch.py fields \
    --updates <scratchpad>/cancel.json
  ```

  with `{"<number>": {"state": "Canceled"}}` per non-survivor.
  A `save_issue` cancel re-echoes the whole body to change one
  enum: measured at ≈6.0k for one cancel plus ≈6.1k for the
  `get_issue` before it, and a later fold would have paid
  roughly **33k** to cancel a survivor whose body had reached
  130 KB.

  **The `duplicateOf` marker is dropped deliberately.** It is
  not an issue field, so it cannot ride `fields`, and the
  survivor's `# Part` headings already name every folded issue
  — which is the same information, in the artifact a reader
  actually opens. Do not reintroduce a body-echoing write to
  restore a marker the body already carries.

**6. Report.** One line: the survivor (with its final
title), the issues folded in and canceled, and which
inherited blocking edges were carried (and which were left
as prose).

**No collision step.** Nothing records overlap — the
automated file-collision machinery is retired, and so is the
declared-scope field it read. A survivor whose scope has
obviously widened is worth a line in the report as prose,
since it is a signal for the next consolidation pass, but it
produces no relation write. Reconciling overlap is
planning-session work.

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
