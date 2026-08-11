---
name: plan
description: Run a planning session — the complement to a worktree implementation session. Bootstraps from the "Planning" Linear document (id in `LINEAR_PLANNING_DOC_ID`), surfaces the Todo umbrellas unprompted, then keeps the board coherent: the Queue honest, blocking edges curated, issues filed and amended to house convention. Writes decisions back into the Planning doc incrementally and behind a close-out gate, and captures the session's own token profile. Planning sessions run in the base repo (started `naps planning-<day>`, resumed `rnaps planning-<day>`), never in a worktree.
user-invocable: true
---

# `plan`

The working split is two session kinds. **Implementation**
sessions run one deterministic spec to completion in a
worktree: `/init-pr` → build → `/review-pr`. **Planning**
sessions are their complement — they stage that work, keep
the board coherent, and carry direction across days.

This skill is the planning session's method. It exists so
each new session doesn't reconstruct it from scratch.

## The Planning document is the living state

The session's memory between days is a **Linear document**
titled "Planning", whose id is exported as
`LINEAR_PLANNING_DOC_ID` in `~/.zshrc`. Resolve it with a
bare `printenv` — its own call, per
`docs/conventions/linear-automation.md`:

```sh
printenv LINEAR_PLANNING_DOC_ID
```

If it is unset, say so and stop rather than searching Linear
for a document by title: a planning session that guesses its
own state file will write decisions into the wrong place.

**The skill maintains that document; it never becomes a
second copy of it.** Nothing here restates the current phase,
the standing decisions, or the sequencing — those live in the
doc, they change weekly, and a copy in this file would be
wrong within days. This file carries only the *method*.

Two standing rules for the doc itself:

- **Work items live in issues, never here.** The document
  carries direction, sequencing, and vocabulary.
- **Prune superseded lines** rather than letting it grow.
  A close-out that only ever appends turns the bootstrap
  read into an archaeology exercise.

## Where it runs

The **base repo**, never a worktree — a planning session
touches the board, not a branch. Start one with
`naps planning-<day>` (named for the day it started:
`planning-10` = Aug 10) and resume it with
`rnaps planning-<day>`. Those helpers, and their worktree
counterparts `aps` / `raps`, are documented in
`docs/conventions/local-integrations.md`.

## Steps

**1. Bootstrap, and surface the umbrellas unprompted.**
Fetch the Planning doc with
`mcp__claude_ai_Linear__get_document` (id = the resolved
value) and read it first — it is the handoff from the last
session, and everything below assumes it.

Then read the board: the **Todo umbrellas** (the meta /
initiative issues) and the **Backlog** (titles and parents;
pull a body only when a decision turns on it — the echo
budget in `docs/conventions/linear-automation.md` applies
here as much as anywhere, and a planning session touches
many issues).

**Then say the umbrellas out loud, without being asked** —
"these meta tracks are open: …". The operator should not have
to remember to ask which tracks exist; that is exactly the
context the session was started to hold.

**2. Keep the Queue honest.** The Queue — the unblocked
Backlog — is the **safe-to-start set**, and its meaning is
load-bearing: an implementation session pulls from it and
runs to completion without re-planning. So every unblocked
issue must be genuinely startable *now*, without carrying
refactor-later burden that a later issue is going to undo.

When an issue isn't: reorder it behind what it depends on,
re-scope it, or split the first genuinely actionable unit
into its own pullable task. An issue that is startable only
"if you also do X first" is a scheduling bug, not a task.

**3. Curate blocking edges — this is the one place they are
placed.** Blocking edges mean **semantic rollout ordering**,
and they are human-curated end to end: no automated writer
files one, ever (see `docs/conventions/linear-automation.md`
→ "Blocking relations"). Planning sessions are where they are
placed, changed, and removed, because that is where somebody
is actually deciding the order.

Three things that are **not** blocking edges, each with its
own handling:

- **File overlap** — `related`-linked and reported as a
  collision cluster by `sync-blockers`. It costs a rebase,
  not an ordering.
- **Coupling that belongs in one PR** — fold the issues into
  one via `/merge-tasks`, don't relate them.
- **A suspicion** — record it as prose in the body. An edge
  you are not sure about costs more than a missing one: a
  spurious edge silently drops an issue out of the available
  set.

**4. File and amend to house convention.** A planning session
does a lot of writing, and every rule that binds a filing
skill binds it too:

- **Dedup first** — search the open Backlog before filing.
- **Amend with `patch` ops**, never by re-sending a body;
  anchor on **plain prose**, never on a line carrying an
  `ENG-###` (Linear stores it as a mention node) or heavy
  bold / code spans.
- **Fewest coherent PRs** — fold coupled findings, but never
  across separate apps, languages, or deploy units.
- **Todo / Backlog split** — initiatives and meta work in
  **Todo**; pullable work items in **Backlog**, which is what
  the operator pulls from.
- **`Claude:` prefix** on meta-work titles (anything whose
  `**Touches**:` sit entirely under `.claude/**`,
  `CLAUDE.md`, or `docs/conventions/**`).

**5. Coordinate with in-flight sessions.** A planning
decision can invalidate the ground truth an implementation
session is working from — a re-scope, a changed sequencing, a
retired approach. When that happens, **message the affected
worktree session** rather than letting it discover the change
at review time. Use `ListAgents` to find them.

**6. Write back — incrementally, then at the gate.**

**Prefer writing after each major decision** over composing
one entry at the end. Planning sessions are frequently closed
abruptly — the terminal is killed, the worktree is deleted —
and an un-written decision is simply lost. Incremental
write-back means an abrupt close costs nothing.

**Then gate the close-out; don't wait to be told.** When the
operator signals wrap-up, **or the conversation lulls after
significant decisions**, fire an `AskUserQuestion` offering
to close out — "close out now? (writes the dated entry to the
Planning doc)" — with that as the recommended first option.
The gate exists precisely because the write-back is otherwise
the thing that gets forgotten.

A close-out entry is dated, names the session
(`planning-<day>`), and records **decisions and their
reasons** — not a transcript. Prune the lines it supersedes
in the same write.

**A session that decides without writing back has dropped its
main deliverable.** The board shows *what* was decided; the
doc is the only place that carries *why*, and why is what the
next session needs.

**7. Capture the session's token profile at close-out.** Run
the committed metrics tool over the planning transcript and
file the entry into the Session Metrics inbox, the same as an
implementation session — so `trim-context` mines
planning-session shapes too, rather than only review shapes:

```sh
make session-metrics SESSION=<uuid>
```

**One hard constraint on what may be proposed from it.**
Planning sessions deliberately run the most capable models
with generous context. **Fidelity is the point, and is never
traded for tokens.** So a planning-derived lever must be
**quality-preserving**: a cheaper bootstrap fetch, a narrower
Linear echo, a repo survey hoisted into a sub-agent, a
redundant re-read removed. Never a model downgrade, never a
skipped bootstrap read, never anything that thins the
decision context.

A lever that would trade planning fidelity for tokens is
recorded as **rejected, with the reason** — not proposed.
Writing it down as rejected is what stops the next mining
pass from re-deriving it.

## Notes

- **No source edits.** Like the other board skills, this one
  writes to Linear and to the Planning document. It authors
  no diff, and commits nothing.
- **It is not a one-shot.** A planning session is a
  conversation that spans hours or days; this skill describes
  how to run one, not a pass to execute and exit.
