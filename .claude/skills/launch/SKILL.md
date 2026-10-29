---
name: launch
description: Run the launcher — the executor-tier session that starts the day, opened by the fresh-day `plan` pre-launch in a tab just left of the scheduler's (or typed as `launch`), in the base repo with no worktree, branch or PR. Runs the trim-context lever fold over the day's digest in its own visible tab, filing the Claude: batches straight into Backlog and writing the fold stamp the scheduler reads; gathers the kill and merge candidates it cannot decide alone, filing and moving nothing on them. Waits, event-driven, for the scheduler's `state of play written` message, then dispatches the curator with the hand-off as its first message. Then asks the morning's one destructive question — the finished worktrees and stray branches with their commits ahead of main, the transcript purge with its bytes, and the upkeep report's other destructive items — and applies only what the operator ticks. Ends with session metrics and its cost; the tab stays open and a late message is acted on.
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

Keep the section in context: step 3's ask reads its destructive
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
go on to step 3 — so a day with no scheduler still gets its prune
ask.

## 3. The prune ask

The morning's one destructive question, asked once, here (ruling
16). A past rename of sessions and worktrees deleted live work on a
rule that looked deterministic, so no pass applies these unasked.
Gather, each with one call:

- **Finished worktrees and stray branches** — the prune step's
  would-remove names, each with its `ahead` line (commits beyond
  `origin/main`, and the tip):

  ```sh
  python3 .claude/tools/upkeep.py prune --json
  ```

  It is a dry run. Its names' issues are completed or canceled, so
  the apply lifts the unpushed and dirty-tree holds — only a live
  session's lock still holds a tree. So a name with commits ahead is
  worth a look before a yes, and its tip is the only restore handle
  (`git branch <name> <tip>`) once deleted; say it beside the name. A
  held-back name (`skipped`) is reported, never asked.

- **The transcript purge** — `purge-conversations` steps 1–3 (the
  open-PR join, then its dry-run manifest), reading every named tag
  as that skill says.

- **The upkeep report's other destructive items**, from the
  section in context: allowlist cruft, permission-refresh
  candidates and stale memories, when listed.

Then **one `AskUserQuestion` call**, one question per category with
something in it (none for an empty one): the prune names, one per
line with their counts; the purge's total and its named tags; and a
`multiSelect` over the report's items. The recommended answer first.
Apply only what is ticked:

- **Prune**: `python3 .claude/tools/upkeep.py prune --apply` —
  same rule, same gates, recomputed at apply time, so it is the whole
  set or nothing. Say what it removed (with each tip) and what it
  held back, and name loudly any removed name the ask did not show —
  its tip in the output is then its only record.
- **Purge**: `purge-conversations` step 4's apply, this yes its
  approval, then its step 5 report.
- **Allowlist cruft**: remove the approved entries from
  `<base>/.claude/settings.local.json` with the Edit tool, then an
  exit-code-only parse check.
- **Permission-refresh candidates**: `allowlist.py add` per approved
  rule.
- **Stale memories**: delete each approved memory file **and** its
  `MEMORY.md` pointer.

Nothing to ask in any category: say so in one line and skip the ask.

## 4. The tail

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

- **Writes only to Linear**: the fold's batches, folds and
  rejections. No state write on any other issue, no edges, no
  priorities, no Planning-document write — the document is the
  scheduler's.
- **Shell discipline** (`docs/conventions/shell-commands.md`): every
  command is one bare call.
