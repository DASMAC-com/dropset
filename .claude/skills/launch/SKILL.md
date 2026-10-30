---
name: launch
description: Run the launcher — the executor-tier session that starts the day, opened by the fresh-day `plan` pre-launch in a tab left of the scheduler's (or typed as `launch`), in the base repo with no worktree, branch or PR. Runs the trim-context lever fold in its own visible tab, filing the Claude: batches into Backlog and writing the fold stamp; gathers kill and merge candidates, filing nothing on them. Then prunes, unasked, every worktree and branch whose issue is Done or Canceled and whose session the digest has read, reporting each tip. Waits, event-driven, for the scheduler's `state of play written`, then dispatches the curator with the hand-off. Weekly, a read-only sub-agent judges every memory stale, superseded, repo-bound or keep. Then asks the one destructive question — the transcript purge, the upkeep report's other destructive items and the judged memories — applying only what is ticked. Ends with session metrics and its cost; the tab stays open.
disable-model-invocation: false
user-invocable: true
---

# `launch`

The **launcher** starts the day. The pre-launch (a fresh-day `plan`
in the typed tab) ran the AWS gate and the upkeep pass, then opened
two tabs: the scheduler's, rightmost, and this one just left of it
(`docs/conventions/local-integrations.md` → the verb table). The
scheduler bootstraps the board while this tab folds the levers, so
both finish around the same time; the curator starts only once both
have.

Three seats, three jobs:

- the **scheduler** (`plan`) owns the board's schedule and the
  Planning document;
- the **curator** (`curate`) owns review — Backlog to Todo, kills
  and merges;
- the **launcher** files the levers, hands the curator its
  candidates, brings it up, and asks the destructive question.

## Where it runs

The **base repo**, never a worktree; no branch, no commits, no PR.
Executor tier, **Bedrock** by default (`launch anthropic` is the
escape). The `launch` helper pins the model and refuses an
unresolved tier, so there is no in-session model check. There is
**no governing issue**: no state write, no close tool. It does not
cycle — one session a day, `launch-<day>`.

**`DS_LAUNCH_SCHEDULER` first.** Read it with its own bare call:

```sh
printenv DS_LAUNCH_SCHEDULER
```

Set only by the pre-launch's last fallback (neither tab opened), it
names the scheduler verb — `plan` or `plan anthropic`. Dispatch it
before anything else:

```sh
python3 .claude/tools/session_dispatch.py plan
```

(with `anthropic` appended when the variable carries it). A `held`
line is the machine gate's refusal: say it loudly and name the verb
for the operator to type; never pass `--over-budget` on your own.

## 1. The lever fold

Run the [`trim-context`](../trim-context/SKILL.md) method inline,
in this tab — never a sub-agent, since the point of this tab is
that the fold's narration is visible. Its input is the upkeep
report's `digest` line; read the report section alone, never the
whole document:

```sh
python3 .claude/tools/planning_doc.py --section 'Upkeep report'
```

Keep the section in context: step 4's ask reads its destructive
items from it. A report that wrote no digest means no fold: say so,
write no stamp, and go on to step 2.

Follow trim-context steps 1–7 as written — batches **Backlog, no
milestone**, folds into open issues zero-echo, rejections filed
Canceled — then stamp the fold beside the upkeep stamp:

```sh
python3 .claude/tools/upkeep.py fold-stamp --batches <N> --folds <M>
```

It prints the line the scheduler's bootstrap will say. Stamp a fold
that filed nothing too (`--batches 0`): the stamp means the fold
**ran**.

**Gather the kill and merge candidates** while folding, since the
fold already reads the evidence: a Backlog issue the title dedup
(`linear_issue.py find`) shows to be the same work as another (a
**merge** candidate, both ids), or one whose premise the HEAD check
shows already landed or retired (a **kill** candidate). Each carries
one line of evidence — the commit, the PR or the sibling issue. File
nothing and move nothing on them: canceling and merging are the
curator's, on its judgment and the operator's tick.

**Then prune, unasked** — the first act after the stamp (operator
ruling, 2026-10-29: Done means prune):

```sh
python3 .claude/tools/upkeep.py prune --apply
```

Its names' issues are completed or canceled with no PR open, so
everything attached goes — the checkout, its `.git/worktrees`
registration and the local branch — dirty or unpushed work included,
since Done is the operator's own word. It runs after the pre-launch
pass's digest step, and a tree whose session the digest has not read
yet is held (`held: not yet digested`) until the next morning. Say in
one line per name what it removed, **with each tip** — the only
restore handle, via `git branch <name> <tip>` — then what it held: a
live session's lock (`skipped`) or the digest hold, reported,
never asked and never retried. Linear unreachable prunes nothing;
say so.

## 2. The hand-off and the curator

**Wait for the scheduler's message.** Its bootstrap sends exactly
`state of play written` to `launch-<day>` once the Planning document
holds the state of play. The two run side by side, so it may land
**while the fold is still running**: then go straight on to the
dispatch once the fold is done. Otherwise run `ListAgents` once to
see that a scheduler (`schedule-<day>`) is live and say so; then end
the turn. **Never poll** the board, the document or the listing.
The message arriving resumes this session.

**Dispatch the curator once per day.** If a `curate-<day>` is
already listed, or this session already dispatched or handed off,
a further `state of play written` (should one arrive again),
`launch the curator now` or `curator up` is only acknowledged —
never a second dispatch or hand-off. Otherwise, once **both** the
fold is done **and** the message has arrived, check the pool:

```sh
python3 .claude/tools/ratify_staged.py list --pool inbox
```

An **empty pool** dispatches nothing: say so; the day's first
filer messages the scheduler, which starts `curate` by hand. Else
dispatch, on Bedrock unless the operator named `anthropic` in this
tab (then append it):

```sh
python3 .claude/tools/session_dispatch.py curate
```

Read the dispatch's own lines. On an opened tab, run `ListAgents`
until `curate-<day>` is listed (a fresh tab takes a few seconds;
re-list at most three times), then send it **one** message, this
shape exactly, with each list's lines or `none`:

```txt
Hand-off from launch-<day>: lever fold done, state of play written.
Batches filed:
- ENG-### — <title> (<n> parts)
Folds made:
- <fingerprint> → ENG-###
Levers rejected:
- <fingerprint> — <one-line reason>
Kill candidates:
- ENG-### — <evidence>
Merge candidates:
- ENG-### + ENG-### — <evidence>
```

The curator's first ask shows the two candidate lists as
checkboxes. Its **fold protocol** — folding the operator's review
comments into a task body — is a different sense of fold and is
untouched; the lever fold is finished before it starts.

**A held or absent curator still gets its hand-off.** On
`held  curate: …` the machine gate refused: say so loudly and never
pass `--over-budget`. On a tab that opened but whose session is not
listed after the three re-lists, say so. Either way, print the
hand-off block in this tab and tell the operator to type `curate`
once a slot frees, then `curator up` here: that message re-lists
and sends the block, so nothing nudges the launcher unasked and
nothing is lost.

**No message by the time the fold is done** means idling at no
cost. The operator's `launch the curator now` in this tab is the
override: dispatch and hand off as above without the message, then
go on to step 3 — so a day with no scheduler still gets its
destructive ask.

## 3. The memory judge

The memory posture is a **minimal** store: a memory earns its place
only when it is about the operator or how to work and is not
derivable from the repo. The upkeep report's `memory` line carries
`memory_audit.py`'s mechanical findings and the `MEMORY.md` line
count, which should fall run over run. The judgment is weekly, on
its own gate, with the auto-memory directory this session's system
prompt names:

```sh
python3 .claude/tools/memory_scan_gate.py judge-check <memory-dir>
```

On `judge: false`, skip to step 4. Otherwise take the pre-filter
(`memory_audit.py <memory-dir> --repo-root . --json`, written to a
scratch file) and spawn **one** read-only `Explore` sub-agent: the
canonical brief by pointer (`lens_preamble.py --out <path>
--no-facts`), the memory directory and the base checkout as its
named-path allowlist,
the pre-filter's path, a budget (≤ 30 turns), and this deliverable
— one row per memory file, `<file> — <verdict> — <fact>`, after
reading every file and checking each claim against HEAD and the
board:

- **stale** — disproved; the fact is the commit, file or issue state
  that disproves it;
- **superseded** — a named sibling memory says it better;
- **repo-bound** — a durable project or convention fact that belongs
  in a committed doc; the fact names the doc;
- **keep** — about the operator or how to work, not derivable from
  the repo. `feedback` memories are always kept.

**`first: true` is the full cull**: the whole set, uncapped. Later
runs are incremental, capped at the ten strongest non-keep verdicts.
Record the run whatever the yield:

```sh
python3 .claude/tools/memory_scan_gate.py judge-record <memory-dir>
```

The verdicts join step 4's ask. **Repo-bound** items are filed first
as **one** `Claude:` docs task per run (Backlog, no milestone; one
part per memory with its target doc and the claim to land), so each
tick deletes a memory whose fact already has a home.

## 4. The destructive ask

The morning's one destructive question, asked once, here (ruling
16). Worktrees and branches are no longer in it: step 1 pruned them.
Gather, each with one call:

- **The transcript purge** — `purge-conversations` steps 1–3 (the
  open-PR join, then its dry-run manifest), reading every named tag
  as that skill says.

- **The upkeep report's other destructive items**, from the
  section in context: allowlist cruft and permission-refresh
  candidates, when listed.

- **Memories**, from step 3 on a judge day: every stale, superseded
  and repo-bound verdict with its fact beside the name (the first
  run's whole set as one checklist, uncapped).

Then **one `AskUserQuestion` call**, one question per category with
something in it (none for an empty one): the purge's total and its
named tags; a `multiSelect` over the report's items; and a
`multiSelect` over the memories. The recommended answer first.
Apply only what is ticked:

- **Purge**: `purge-conversations` step 4's apply, this yes its
  approval, then its step 5 report.
- **Allowlist cruft**: remove the approved entries from
  `<base>/.claude/settings.local.json` with the Edit tool, then an
  exit-code-only parse check.
- **Permission-refresh candidates**: `allowlist.py add` per approved
  rule.
- **Memories**: delete each ticked memory file **and** its
  `MEMORY.md` pointer, then say the index's new line count.

Nothing to ask in any category: say so in one line and skip the ask.

## 5. The tail

In order, unprompted:

1. Session metrics (`session-metrics`).
1. Its own cost, in one line.
1. One line: the scheduler and the curator are up (or which one is
   not, and why), and the operator may paste anything from the
   pre-launch log here.

The tab **stays open**. A message after the tail — a pasted log
line, a late `state of play written`, `launch the curator now`,
`curator up` — is answered like any other, under step 2's
once-per-day dispatch rule.

## Notes

- **Writes to Linear** only the fold's batches, folds and
  rejections and the memory judge's docs task. No state write on any
  other issue, no edges, no priorities, no Planning-document write —
  the document is the scheduler's. Locally it prunes finished
  worktrees unasked and deletes only the memories the operator
  ticked.
- **Shell discipline** (`docs/conventions/shell-commands.md`): every
  command is one bare call.
