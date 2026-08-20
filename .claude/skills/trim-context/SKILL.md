---
name: trim-context
description: Fold parked trim levers into one propose-only skill-improvement task — the consumer half of the `session-metrics` producer. Sweeps the `Trim levers` project milestone (never a document), folds the parked levers into a single aggregated `Claude:` Backlog task under the fewest-coherent-PRs rule — one bullet per lever, each keeping its own `**Fingerprint**:` line under a combined `**Touches**:` — then closes the parked originals so the milestone lifecycle is the state machine and nothing needs draining. A lever judged not worth acting on is closed with its reason instead, which suppresses refiling permanently. Never edits a skill or convention doc — filing a task is the proposal. Runs standalone or as `housekeeping`'s Session Metrics step.
disable-model-invocation: false
user-invocable: true
---

# `trim-context`

The **consumer** half of the context-economy feedback loop.
`session-metrics` is the producer: at the end of a session it files each
trim lever as its own small **parked issue**, stamped with the
`Trim levers` project milestone and keyed by a `**Fingerprint**:`.
`trim-context` is the periodic **fold**: it sweeps that milestone, folds
the parked levers into a single aggregated propose-only Backlog task, and
closes the originals.

It runs identically whether invoked standalone or by `housekeeping` —
there is **no** propose-only vs. apply split, because filing a task *is*
the proposal: this skill never edits a skill or convention doc, so an
unattended pass and a hand run do exactly the same thing.

## This skill reads no document

It used to mine a Linear "Session Metrics" inbox **document**, and that
document is retired. The reason is structural rather than tidiness: with
roughly ten parallel sessions a day and a few thousand characters per
entry, the body crossed the harness's tool-result cap between any two
drains — 67.0k characters at the last one — so each pass spilled it to
disk and picked it apart with a hand-written scratchpad script, twice.
The producer appended with a cheap `patch` and paid only its entry's
length, while **mining** re-read the whole body every pass; the cost sat
entirely on this side.

Per-lever parked issues fix that at the root, and buy two things the
document never could:

- **Recurrence becomes a fact, not a pattern to re-detect.** A lever seen
  again gets this session's evidence appended to the issue that already
  exists, so "this recurred in four sessions" is recorded rather than
  something a miner has to notice in prose.
- **A rejection sticks.** A lever judged not worth acting on is **closed
  with its reason**, and dedup-against-resolved then suppresses refiling
  permanently. Nine of thirteen entries in the final document-era pass
  carried an explicit "do not mine this as waste" note, several written
  only because an earlier pass had re-proposed the very thing on
  intuition. That whole class of re-litigation disappears.

No drain bookkeeping survives: the milestone lifecycle **is** the state
machine, on the pattern the `Audit findings` milestone already proved.

## Linear destination

This skill sweeps a milestone and **files** one Backlog task, so it needs
the env-resolved filing destination (the same one `linear-task` /
`housekeeping` use). Resolve each variable with its **own** bare
`printenv` (one `Bash(printenv:*)` allow-rule covers them all) — never a
combined `printenv A B C`, which on macOS / BSD prints only the first
value:

```sh
printenv LINEAR_TEAM_ID
printenv LINEAR_PROJECT_ID
printenv LINEAR_ASSIGNEE_ID
```

If any is empty, say so and stop before filing. There is no longer a
document id to resolve, so there is no inbox-not-set-up no-op path.

## Steps

**1. Sweep the parked levers.** One call, titles and urls only — no
bodies:

```sh
python3 .claude/tools/trim_levers.py list
```

If nothing is parked, report that and stop; there is no fold to do.

**2. Read only the bodies you are going to fold.** The listing gives you
each lever's identifier, title and state. Decide from the titles which
levers this pass folds, then read *those* bodies and no others. A parked
pool that has grown past what one coherent PR should carry is a reason to
fold a subset now and leave the rest parked — not a reason to read
everything.

**3. Group into the fewest coherent PRs.** Per `CLAUDE.md` → "Structured
filing fields", fold every set of levers that would land as **one PR**
(same subsystem / crate / language-domain) into a single issue, and never
fold across separate apps / languages / deploy units — that is the
coherence floor. In practice most trim levers touch `.claude/**` and
`docs/conventions/**` and belong together; a lever that touches product
code does not join them.

**4. File the aggregated task, propose-only.** The fold's output is one
`Claude:` Backlog task per coherent group. A trim lever always edits a
skill or convention doc, so the task is meta-work — prepend the
**`Claude:`** prefix to its title, per `CLAUDE.md` → "Claude: meta-work
prefix". Its body is **one `# Part N — <title>` section per lever**, and
carries:

- one **`**Fingerprint**: <domain>:<lever-slug>`** line **per lever**,
  copied from the parked original so later dedup still matches
  individually, and
- a single **`**Touches**:`** line that **unions** every folded lever's
  globs (per `docs/conventions/linear-automation.md` → "Structured filing
  fields").

Set `state`, `priority` and any relations in the **creating** call — a
follow-up write buys a second full body echo for nothing (same convention
doc → "Relations and state belong in the CREATING call").

```txt
mcp__claude_ai_Linear__save_issue(
  team: "<$LINEAR_TEAM_ID>",
  project: "<$LINEAR_PROJECT_ID>",
  assignee: "<$LINEAR_ASSIGNEE_ID>",
  state: "Backlog",
  title: "Claude: <umbrella summary of this fold's trim levers>",
  description: "<one `# Part N — <title>` section per lever — each the
    lever, the sessions that motivate it, the concrete skill /
    convention-doc edit it implies, and its own **Fingerprint**: line>
    \n\n**Touches**: <combined globs>",
  priority: 3,
)
```

**Autonomy bound:** filing a task *proposes* a fix — this skill **never**
edits a skill, a convention doc, or `CLAUDE.md`; that lands later through
a normal PR.

**5. Close the parked originals.** A folded lever's content now lives in
the aggregated task, so the parked issue is discharged. Close it and
clear its milestone in **one** field-only write per issue, through the
board tool rather than the MCP (per
`docs/conventions/linear-automation.md` → "Field-only writes go through
`board_batch.py`, not the MCP") — an MCP `save_issue` would echo each
body back for a one-field change:

```sh
python3 .claude/tools/board_batch.py fields --updates <scratchpad>/close.json
```

with an updates file naming each folded lever:

```json
{
  "912": { "state": "Done", "milestone": null },
  "913": { "state": "Done", "milestone": null }
}
```

Reference the aggregated task from each closed lever only if you are
already making a body write for another reason; the fold's own body cites
the levers, which is the durable record.

**6. Close a rejected lever with its reason instead.** A lever you judge
not worth acting on is **not** folded and **not** left parked — it is
closed as `Canceled` with a one-paragraph reason appended to its body,
naming the evidence. That is what makes the rejection permanent: the
fingerprint probe searches resolved and archived issues too, so no later
pass re-proposes it. This is the mechanism that replaces the old
"not-a-trim register" idea — the register falls out of the lifecycle
rather than being a separate artifact anyone has to maintain.

Reject on evidence, not on taste. Recorded rejections worth knowing
about, each measured: narrowing a planning board read (2–3k against 87.6k
of writes — worthwhile for judgment, not tokens); abridging a bootstrap
document read (it is the entire handoff from the previous session, and
the only carrier of *why*); a cheaper model for planning sessions
(standing decision); the cross-check's higher turn cap (it ran multiples
of the cheapest lens and earned it twice over, once catching a spec that
would have let a later rewrite restore a fund-confiscation bug);
correctly-amortized whole-file Reads; checkpoint-per-logical-change
commit counts (14 calls at ≈700 total is the convention working);
post-rebase re-verify churn driven by `main` moving mid-review.

**7. Report** in one line: the aggregated task(s) filed with their
ENG-###s and how many levers each folded, how many parked levers were
closed, how many were rejected and why, and how many remain parked for a
later fold.

## Appendix: the one-time legacy drain

The retired inbox document may still exist with unfolded entries in it.
Draining it is a **one-time** act, not part of this skill's normal pass,
and it is best done in a planning session where the board is already
open: read the document once, fold its remaining levers through steps
3–4 above, then delete the document. Do not rebuild the document-mining
path to do it — read it with the slice-reader if it overflows:

```sh
python3 .claude/tools/read_result.py <spilled-file> --field content --headings
```

Once it is gone, `LINEAR_SESSION_METRICS_DOC_ID` is dead configuration
and can come out of the environment.

## Notes

- **No source edits.** This skill writes only to Linear — the filed
  Backlog task and the parked levers' closures — and never authors a code
  or skill diff, never commits, never pushes. The improvements it
  proposes are applied later by a human through a normal PR.
- **No relations, ever.** Folding places no blocking edge; parked levers
  are exempt from the serial meta chain until folded, and the aggregated
  task the fold produces is what the chain governs. Blocking edges are
  human-curated in a planning session (`CLAUDE.md` → "Blocking
  relations").
- **Runs standalone or as housekeeping's step.** `housekeeping` delegates
  its Session Metrics step to this skill; it runs just as well by hand any
  time the milestone has parked levers. Either way the behavior is
  identical — there is no attended / propose-only mode.
- **Shell discipline** (per `docs/conventions/shell-commands.md`): every
  command is a single bare call that reduces to an allow-glob — no `&&`,
  pipes, `$(…)`, or redirects; resolve each id with a bare `printenv`,
  one variable per call.
