---
name: trim-context
description: Fold parked trim levers into one propose-only skill-improvement task — the consumer half of the `session-metrics` producer. Sweeps the `Trim levers` project milestone (never a document), folds the parked levers into a single aggregated `Claude:` task — always ONE task, whatever the lever count or surface spread, with one `# Part N` section per lever, each keeping its own `**Fingerprint**:` line — filed PARKED itself (Todo plus the `Claude meta` milestone, so the planning bootstrap's batch assembly consumes it rather than it sitting unblocked in Next), then closes the parked originals so the milestone lifecycle is the state machine and nothing needs draining. A lever judged not worth acting on is closed with its reason instead, which suppresses refiling permanently. Never edits a skill or convention doc — filing a task is the proposal. Runs standalone or as `housekeeping`'s Session Metrics step.
disable-model-invocation: false
user-invocable: true
---

# `trim-context`

The **consumer** half of the context-economy feedback loop.
`session-metrics` is the producer: at the end of a session it files each
trim lever as its own small **parked issue**, stamped with the
`Trim levers` project milestone and keyed by a `**Fingerprint**:`.
`trim-context` is the periodic **fold**: it sweeps that milestone, folds
the parked levers into a single aggregated propose-only **parked** task
(state `Todo` plus the `Claude meta` milestone, so the next planning
bootstrap's batch assembly consumes it), and closes the originals.

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

This skill sweeps a milestone and **files** one parked task — `Todo`
plus the `Claude meta` milestone, not Backlog — so it needs
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

That stop condition is **reachable**, which it once was not. `list`
filters to **open** levers, not merely milestoned ones, so a lever closed
as a recorded rejection drops out of the pool. Before that filter the
pool always looked non-empty once any rejection existed — one real run
returned 12 rows of which **9 were canceled rejections**, and only 3 were
foldable — so this step could never fire and step 2 was invited to fold
settled work.

**2. Read the bodies you are going to fold — in ONE call.** The listing
gives you each lever's identifier, title and state. Decide from the
titles which levers this pass folds, then fetch those bodies through the
tool rather than one MCP `get_issue` per lever:

```sh
python3 .claude/tools/trim_levers.py list --fingerprints
python3 .claude/tools/trim_levers.py list --bodies-out <scratchpad>/levers.md
```

`--fingerprints` adds each lever's dedup key to the listing — the thing a
sibling lookup actually needs, and the one field the plain listing omits.
`--bodies-out` fetches **every** parked body in a single query and writes
them to a file with one `## <identifier>` heading each, printing only
sizes; slice it with
`python3 .claude/tools/read_result.py --section '<identifier>' <file>`.

**The fold's reads had become its larger cost**, which is why this exists:
the plain listing prints titles only, so a fold ran **one fetch per
lever** — 21 of them on one pass — and the nearest sweep that did carry
bodies (a project-wide Todo read) cost **10.6k for 65 issues with every
description truncated anyway**, so five per-issue follow-ups ran
regardless. That read cost is what sized one fold down to five levers.

A parked pool that has grown past what one coherent PR should carry is
still a reason to fold a subset now and leave the rest parked — the
cheaper read changes what a fold costs, not what a fold should contain.

**3. Fold into ONE task — always.** Operator ruling, 2026-08-25: a
trim-lever fold produces a **single** task, regardless of how many levers
it carries or how many surfaces they touch. This step used to say "the
fewest coherent PRs, one task per coherent group"; it no longer does, and
**no size bound applies** either.

The coherence floor is not being abandoned — it stays exactly where it
belongs, on **audit findings and product filings**, which different
sessions pull and which genuinely must not cross an app or language
boundary. The meta-work fold is the named exemption, on the
one-task-in-flight reasoning: these levers all edit `.claude/**`,
`docs/conventions/**` or `CLAUDE.md`, so they land as one PR by
construction, and splitting them into groups only produces several issues
that serialize behind one another for no gain.

The **per-lever fingerprint requirement is unchanged** — see step 4. That
is what keeps later dedup matching each lever individually, and it is the
reason one task loses nothing.

**4. File the aggregated task, propose-only.** The fold's output is one
**parked** `Claude:` task. A trim lever always edits a
skill or convention doc, so the task is meta-work — prepend the
**`Claude:`** prefix to its title, per `CLAUDE.md` → "Claude: meta-work
prefix".

**It parks rather than going to Backlog:** state **`Todo` plus the
`Claude meta` milestone**, in the creating call. The next planning
bootstrap sweeps that milestone and consumes this task as a part of its
batch, so it reaches a session that way rather than by sitting unblocked
in the operator's Next view. The `Trim levers` milestone, this skill's
own writer, and the per-lever fold below are all unchanged — only where
the *output* lands moved.

**Compose the body with the tool, not by hand:**

```sh
python3 .claude/tools/trim_levers.py compose \
  --bodies-file <scratchpad>/levers.md --out <scratchpad>/folded.md
```

It reads the `--bodies-out` dump from step 2 and emits the aggregated
body, printing only a summary — the body stays on disk, which is the same
zero-echo trade as the fetch half. `--exclude ENG-1,ENG-2` drops levers
already folded; `--start N` continues the numbering when a batch is
composed in halves (the summary prints the next part number).

**Why this is a tool.** Under the whole-pool ruling a fold carries the
entire parked pool — one pass folded **41 levers, 101,798 chars** — so
re-authoring by hand stopped being sensible and that pass wrote a
throwaway script instead. Two of the rules it encodes are easy to get
wrong by hand and silently damaging when missed, which is exactly the
kind of thing that belongs in committed code rather than in prose:

- **Heading demotion.** Lever bodies carry their own `#`-level headings
  (`# Lever`, `# Evidence`, `# Proposed edit`). Pasted unmodified under a
  `# Part N` heading they collide at the same level and the task loses
  its structure — every word present, reading as one flat document. The
  tool demotes them, leaving fenced examples alone.
- **Fingerprint preservation.** The tool **fails loudly** if any emitted
  part carries no `**Fingerprint**:` line, rather than emitting a body
  that looks right. A hand fold that summarizes instead of carrying the
  body drops them, and the loss is invisible until a later pass refiles
  a lever that was already folded.

Read the composed file before filing — the tool guarantees structure and
fingerprints, not that the umbrella title you write actually describes
the pool.

Its body is **one `# Part N — <title>` section per lever**, and
carries:

- one **`**Fingerprint**: <domain>:<lever-slug>`** line **per lever**,
  copied from the parked original so later dedup still matches
  individually.

That is the whole field set. There is **no `**Touches**:` line** — the
declared-scope glob field is retired (per
`docs/conventions/linear-automation.md` → "Retired: `**Touches**:`"), so
there is no union to compose. Each part's prose names the skill or
convention doc it edits, which is what an implementer reads anyway.

Set `state`, `priority` and any relations in the **creating** call — a
follow-up write buys a second full body echo for nothing (same convention
doc → "Relations and state belong in the CREATING call").

```txt
mcp__claude_ai_Linear__save_issue(
  team: "<$LINEAR_TEAM_ID>",
  project: "<$LINEAR_PROJECT_ID>",
  assignee: "<$LINEAR_ASSIGNEE_ID>",
  state: "Todo",
  milestone: "Claude meta",
  title: "Claude: <umbrella summary of this fold's trim levers>",
  description: "<one `# Part N — <title>` section per lever — each the
    lever, the sessions that motivate it, the concrete skill /
    convention-doc edit it implies, and its own **Fingerprint**: line>",
  priority: 3,
)
```

**Autonomy bound:** filing a task *proposes* a fix — this skill **never**
edits a skill, a convention doc, or `CLAUDE.md`; that lands later through
a normal PR.

**5. Close the parked originals — this is load-bearing, not tidiness.**
A folded lever's content now lives in the aggregated task, so the parked
issue is discharged. Closing it is what keeps the **producer** working:
the fold copies each `**Fingerprint**:` line into the aggregated task, so
from this moment a fingerprint probe legitimately matches **two** issues.
`session-metrics` resolves that by appending only to a lever that is
still *open and milestoned*, so leaving the original parked would give it
two live candidates and it would refuse rather than accumulate — breaking
the recurrence-accumulation this pipeline exists for.

Close it and clear its milestone in **one** field-only write per issue,
through the board tool rather than the MCP (per
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
pass re-proposes it, and `trim_levers.py file` hard-refuses a canceled
match.

**The reason has to reach the body, and that takes two writes in this
order.** `board_batch.py fields` is field-only by construction, so it
cannot carry prose. While the lever is **still parked**, append the
reason with the zero-echo writer, then close it:

```sh
python3 .claude/tools/trim_levers.py append-evidence \
    --fingerprint <domain>:<slug> --evidence-file <scratchpad>/reason.md
```

```sh
python3 .claude/tools/board_batch.py fields --updates <scratchpad>/reject.json
```

Order matters: once the state is `Canceled` the lever is no longer
parked, and `append-evidence` will report `NOTED … no longer parked`
rather than writing. The reason is the whole point of the rejection —
without it a later pass has a closed issue and no argument. This is the
mechanism that replaces the old "not-a-trim register" idea — the register
falls out of the lifecycle rather than being a separate artifact anyone
has to maintain.

**Clear the milestone here too, exactly as step 5 does** — so the
`reject.json` above carries `{"state": "Canceled", "milestone": null}`,
not the state alone. The milestone means **"awaiting a fold"**, and a
rejected lever is not awaiting one; leaving it stamped made the two
steps disagree about what the milestone was for. Permanence does not
depend on it either way: the fingerprint probe searches resolved and
archived issues, so the rejection sticks whether or not the milestone
is still attached. (`trim_levers.py list` now also filters to open
levers, so a still-stamped rejection no longer pollutes the pool — but
that is the backstop, not the reason. Both halves should agree.)

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
  parked task and the parked levers' closures — and never authors a code
  or skill diff, never commits, never pushes. The improvements it
  proposes are applied later by a human through a normal PR.
- **No relations, ever.** Folding places no blocking edge. A parked
  lever sits outside the meta batch until folded, and the aggregated
  task the fold produces parks under `Claude meta` to be consumed by the
  next batch assembly — which carries **no edge at all**, its assembly
  precondition having replaced the one it used to need. Blocking
  edges are human-curated in a planning session (`CLAUDE.md` →
  "Blocking relations").
- **Runs standalone or as housekeeping's step.** `housekeeping` delegates
  its Session Metrics step to this skill; it runs just as well by hand any
  time the milestone has parked levers. Either way the behavior is
  identical — there is no attended / propose-only mode.
- **Shell discipline** (per `docs/conventions/shell-commands.md`): every
  command is a single bare call that reduces to an allow-glob — no `&&`,
  pipes, `$(…)`, or redirects; resolve each id with a bare `printenv`,
  one variable per call.
