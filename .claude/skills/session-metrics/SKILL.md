---
name: session-metrics
description: Use at the end of a session (`review-pr` runs it unprompted before its close), or standalone for any session id, to see where its tokens went and recommend concrete trims. Drives the committed `session_metrics.py` tool (`make session-metrics SESSION=<uuid>`), which ranks the costliest tools, largest results, sub-agent rollup, and repeated command shapes worth hardening without loading the transcript into context. Writes each trim lever into the session's `Session account` block and files nothing; the daily digest lifts the block and `trim-context`, the fold analyst, judges and files across every session.
disable-model-invocation: false
user-invocable: true
---

# `session-metrics`

Account for a session's token consumption and turn it into
**actionable trim recommendations**. The skill has two
complementary halves:

- **Evidence** — the numbers, produced by the
  `session_metrics.py` tool: token totals, cache-hit
  rate, the tools whose results cost the most, the largest
  single results, a per-sub-agent rollup, and the repeated
  command shapes worth hardening into a tool. The tool
  says *where* the tokens went and *what's repeated*.
- **Recommendations** — the centerpiece: narrative prose
  that reads the ranked sinks and hardening candidates and
  says *what to do about them* (a concrete skill or
  convention-doc edit, a "request less" discipline, a
  sub-agent fan-out to scope down, a workflow to harden
  into a Python tool).

Each recommendation lands as **one line of the session's
`Session account` block** (step 4). The daily digest lifts that
block beside the session's numbers, and `trim-context` — the fold
analyst, `plan`'s bootstrap sub-agent — judges across the whole
day's sessions and files **small themed** propose-only batches
(roughly 4–5 parts each) into the review inbox. A session files
nothing that is only a trim, so no session spends context
checking what others filed.

## The mechanism (why a tool)

A session transcript is multi-megabyte newline-delimited
JSON. The whole point of this work is to *reduce* context,
so the skill must never read that file into the model's
context to analyze it. The tool reads it in its **own**
process and emits only a few-hundred-token summary — that
summary is all the model ever sees. Token attribution is
mechanical (sum `usage` blocks, tie each `tool_result` back
to its `tool_use` by id, rank by serialized size), so it
belongs in the tool; the *recommendations* need a model
and stay here. It is a stdlib-only Python skill-tool under
`.claude/tools/` (per `CLAUDE.md` → "Skill tooling"), not a
Cargo workspace member.

## Deterministic core: the `session_metrics.py` tool

The tool (`.claude/tools/session_metrics.py`, run via
`make session-metrics`) resolves the transcript path itself
from the session id — the Claude home (`CLAUDE_CONFIG_DIR`,
else `~/.claude`) plus the working-directory project slug,
with a scan of every project directory as a fallback so a
worktree whose slug differs still resolves. It then reads
the main transcript and every sibling sub-agent transcript
and prints, as compact Markdown (or `--json`):

- **What the session cost** — the headline, stated first: one
  line saying this session cost about N dollars, then the
  breakdown (cache-read, cache-write, output, input), computed
  from the session's own transcript usage records with each
  message priced at **its own model's** Bedrock rate. The
  headline names every priced model and whether its rate is
  verified against a bill or only projected (Opus 5.5 is
  projected until the first reconciled fleet day). A model
  with no rate row **withholds the dollar figure and names
  the model** rather than pricing at a default; the fix is a
  row in the tool's `RATES_BY_MODEL`. Sub-agent cost is priced
  too and split out, because a fan-out session's bill is
  mostly theirs.

  **A seat session is priced too, labelled "at Bedrock rates,
  not billed".** A **seat** session (`plan` on the seat,
  `architect`) bills against the Claude subscription, which is in
  effect those rates subsidized. The figure says what the work
  would have cost, and the label stops anyone quoting it as a
  charge. Quote it with the label, every time.

  **The substrate comes from the launch marker, not from the
  model id.** Measured 2026-09-14: a Bedrock worker's
  transcript records `message.model` as the plain
  `claude-opus-5`, byte-identical to what a seat Opus session
  records, and no region-prefixed or inference-profile id
  appears in any local transcript — so the id cannot
  discriminate; nor can the model *name*, since seat verbs also
  run on Opus. The tool reads the marker
  `.claude/shell/init.zsh` writes, and **an absent marker
  reads as seat**, which is the safe direction: a missing
  figure is a visible gap, a wrong one silently corrupts the
  daily reconciliation. Pruning a worktree can take its marker
  with it, so `--substrate bedrock` is the override for mining
  an older worker session.

  **Dollars come from the transcript because AWS cannot answer
  this.** Cost Explorer has no session dimension and lags up
  to a day; true AWS-side attribution would need per-verb
  application inference profiles. So AWS is the
  **reconciliation** path — one Cost Explorer query per
  planning bootstrap, compared against the sum of the fleet's
  transcript-priced estimates — never the source.

- **Totals** — input / output / cache-write / cache-read
  tokens and turn count, summed across every assistant turn.

- **Prefix growth** — the prefix carried by the first and last
  billed request, plus its peak. This is what makes the
  quadratic legible: every turn replays the whole
  conversation, so the bill is roughly the average prefix
  times the turn count — which is why a fat early payload is
  paid many times over, and why session length is itself the
  largest cost lever (`CLAUDE.md` → "Context economy"). The
  peak is reported separately because compaction can drop the
  prefix, so the final turn is not the high-water mark.

  Because the cost is quadratic in turns, read this pair as a
  prompt to ask **which turns could have been eliminated
  entirely** — not merely which payloads could have been
  smaller.

- **Resident instruction prose** — every invoked skill's entry
  file, the instructions files (`CLAUDE.md` and the memory
  index) and the skills listing, each times the turns after its
  injection, in token-turns, as a share of all input, and (on
  Bedrock) priced at the cache-read rate. Instruction prose is
  not a tool result, so no other line can see it; a sibling
  file read early is entry content under another name, and
  shows up in the sinks instead. The bytes-per-token ratio is
  **calibrated** from the session's own skill injections when
  one dominated its turn, and the report flags a ratio outside
  the 3.5–4.5 band the skill-size gate's byte caps assume —
  that is the signal to revisit the cap. When the share clears
  the bar the report names, record it as a lever like any other
  (step 4), naming the top resident line — usually an entry file
  to split.

- **Rebases** and **day span** — the rebase count (a started
  `git rebase` or `git pull --rebase` that replayed something,
  and every `--auto` re-enqueue after one that took), each with
  the main-session spend after
  it; and the first and last stamps, the longest idle gap
  (`STALLED` past three hours) and the share spent after it.
  Both are process costs, the operator's merge-quickly rule made
  a number; `fleet_cost.py reconcile` rolls them up per day.
  Either line is a lever only under the same bar as any other:
  turn-eliminating, prose-subtracting or hardening, at about 3k
  per occurrence (`plan`'s rulings → "The lever reservoir").

- **Cache-hit rate** — cache-read ÷ all input.

- **Costliest tools** — by total result size, with an
  approximate token count (bytes ÷ 4; per-result token
  counts aren't on disk, and ranking is what matters).

- **Largest single results** — the individual results that
  cost the most, each with a short label (the file for a
  Read, the command for a Bash, the method for an MCP call).

- **Sub-agents** — a per-agent token rollup, which is what
  catches an inlined-diff fan-out (the cost lands in each
  sub-agent's input, not the main tool table).

  **This rollup sums per-turn input, and that is a different
  quantity from the Agent tool's `subagent_tokens`.** Say so
  when reporting it, because the two differ by roughly an
  order of magnitude and are easy to compare by accident: a
  sub-agent's context is re-sent on every one of its turns,
  so two lenses whose Agent results read ≈102k and ≈104k had
  per-turn input summing to **911.6k and 604.3k**. Anyone
  judging fan-out efficiency from the Agent result line
  concludes a lens was cheap when it was the most expensive
  thing in the session — and `review-pr`'s efficient-exemplar
  figures (~145k, 180.5k, 202.3k) are measured *this* way, so
  an Agent-result number benchmarked against them compares
  unlike quantities.

- **Hardening candidates** — the repeated `Bash` command
  shapes (grouped by normalized prefix), **ranked by result
  size, not call count**, and flagged `deterministic` when
  they're string/path/env logic worth porting into a tool
  (per `CLAUDE.md` → "Skill tooling"). This is what
  nominates a settled, repeated workflow — the
  `git worktree list` / branch-validation kind of sequence —
  for extraction.

  Each candidate carries a **`cost`** label, because "worth
  hardening" has **four** different reasons and conflating them
  produced wrong recommendations:

  - **`context`** — the results are large and the shape is
    unwrapped, so it is a real token sink. Hardening it saves
    tokens.
  - **`context (failures)`** — large *and* already
    `run_quiet.py`-wrapped, so the bytes are **failure tails**.
    The lever is different: the wrapping is in place, and what
    costs tokens is how often the command failed — so the fix
    is fewer round trips, not more redirection. Reported apart
    from plain `context` because a session read the combined
    label as "the wrapper isn't working" and filed a defect
    against it; the classification was right and only the name
    was ambiguous.
  - **`wall-clock`** — wrapped and quiet in practice, so its
    output never entered context. Hardening it buys *latency*,
    not tokens. Three sessions had to add this disclaimer by
    hand for `make lint` ×10, which cost ~20 tokens.
  - **`prompt-churn`** — cheap and fast, but each slightly
    different variant is a fresh permission prompt. A
    `printenv` is the type case.

  **Don't read count as cost.** `grep` topped the old
  count-ranked table in five consecutive sessions while being
  negligible by size (one session's largest grep result was
  ≈516 bytes), and hoisting a shape deliberately *converts
  many small calls into a few larger ones* — so a
  count-ranked table flags the fix as the new problem.

Nothing about the host is hard-coded: paths resolve
dynamically and the summary refers to locations generically.

## Steps

**1. Derive the session UUID.** Read it from your **scratchpad
directory** path (shown in your environment): the UUID is the
directory name immediately above `scratchpad` — a value like
`12e00466-e6f5-450e-b8de-9a037a678373`. This is the same id
that names the on-disk transcript, so no path-guessing is
needed. (If a `CLAUDE_SESSION_ID` is exported, that works
too; the scratchpad component is the reliable source.)

**2. Run the tool** with that id and capture the summary:

```sh
make session-metrics SESSION=<uuid>
```

The summary is small and safe to hold in context. Read it —
it is the evidence the recommendations are grounded in. (Add
`ARGS=--json` if you want the structured form instead.)

**Then append the measured leg to the session's own issue** —
the `ENG-###` its branch or worktree names; none for a session
with no issue. One line in a fixed form, written to a scratchpad
file and appended zero-echo:

```sh
python3 .claude/tools/linear_issue.py append \
  --id <ENG-###> --file <scratchpad>/cost-line.md
```

A Bedrock session writes the first form, a seat session the
second and **never a dollar figure**:

```text
**Cost actual**: $<usd>, <n> turns, peak prefix <k>, <YYYY-MM-DD>
**Token profile**: <cr> cache-read, <n> turns, peak prefix <k>, <YYYY-MM-DD>
```

A second session on the issue appends a second line rather
than editing the first. Format and band table:
`docs/conventions/context-economy.md` → "Cost bands".

**3. Compose the recommendations.** This is the skill's
judgment work — a tool can't do it. Ground every
recommendation in **three** sources:

- **The ranked sinks** from step 2 — the concrete tools and
  results that dominated *this* session. A repeated full-PR
  read, a whole-file Read where a slice would do, a verbose
  build-log Bash, an inlined-diff fan-out across sub-agents.

- **The hardening candidates** from step 2 — the repeated,
  `deterministic` command shapes. A shape that recurs across
  runs and is string/path/env logic is a candidate to port
  into a Python tool (per `CLAUDE.md` → "Skill tooling");
  recommend the extraction, naming the shape and the skill
  step that emits it. **State the candidate's `cost` label**
  in the recommendation — a `wall-clock` or `prompt-churn`
  candidate is still worth porting, but recommending it as a
  *context* saving is simply false, and the fold will treat it
  as one if you don't say otherwise. A
  **`context (failures)`** candidate needs the sharpest
  restatement of all: it is already wrapped, so proposing that
  it be routed through the quiet runner is a no-op — the lever
  is whatever made it fail repeatedly.

  **There is a consumer for these now: `/harden`.** This list
  was the producer half of a loop with nothing on the other
  end — it went into a report and stayed there unless a human
  chose to act. When a candidate is strong enough to build
  today, say so and name the verb, so the fold does not
  re-propose building a tool that a session could simply
  build. `/harden` demands the provenance this step already
  has (the count, the sessions, the cost label), so hand it
  over rather than restating it.

  It **refuses** a shape with no measured recurrence, which is
  the right default — a single occurrence is a lever to record,
  not a tool to write. The one exception is a shape
  expressible only in **forbidden** forms (an inline
  interpreter one-liner, a stopgap grant, a compound): every
  repetition of those is a prompt that can never be firmed, so
  one occurrence is enough.

- **The observations you kept during the session** — per
  `docs/conventions/context-economy.md` → "Track consumption
  ideas as you go", the running notes on what felt wasteful.
  The sinks say *where*; your notes say *why* and *what to
  change*.

Write each recommendation as grounded prose: name the
sink (or candidate shape), state the lever
(transport-agnostic where it applies — "request less",
narrowest method, field-select, read by slice, scope the
sub-agent, harden into a tool), and where you can, name the
**concrete** skill step or convention-doc rule to edit.
Keep it tight; this is a recommendation, not a patch.

**Ask which turns could have been eliminated entirely — every
run.** Walk the session for stretches of consecutive turns with no
judgment inside (a chain of deterministic acts, a poll-and-react
loop, a lookup-then-act pair) and name each as a lever: the flow,
the turns it spent, and the one tool run that would replace it.
**A turn-eliminating lever ranks above any lever that merely
shrinks a payload** (`docs/conventions/context-economy.md` →
"Session length is itself a cost lever"), since deleting a turn
removes every replay it would have paid. Rank within each class
as below.

**Rank levers by estimated dollars saved, not tokens.** Dollars
are what the credit spend is billed against, and the two orders
differ: a cache-read-heavy sink priced at $0.22/Mtok can rank
below a much smaller output-heavy one at $22.00/Mtok (Opus 5.5,
projected). Where a
lever's saving can only be stated in tokens, price it at the
rates the tool reports and say which tier it lands in. A
**seat** session ranks the same way, on its not-billed figure.

**4. Write the levers into a `Session account` block — file
nothing.** A session files nothing that is only a trim: the
daily digest lifts this block verbatim from the session's
message text (it matches a `## Session account` heading,
through the next heading, at most 40 lines), and the fold
analyst (`trim-context`) judges and files across every
session at once. So end your report message with:

```text
## Session account

- <target skill/tool/rule> — <measured payload or turns> — <lever>
```

One line per lever, ranked as step 3 says. Each line carries
all three fields, because the block is the only record of the
lever: the **target** the edit lands in, the **measured**
figure (tokens, dollars or turns, with the session's own
numbers), and the **lever** as an imperative. Summarize a sink
by tool and target, never a raw command or URL. This step is
interim: the later shared session close-out task retires it.

**5. Report** in one line — the session, the top sink, and how
many levers the account names.

## Notes

- **No source edits, no filing.** This skill writes only the
  cost line to the session's own issue and never authors a
  code or skill diff, never commits, never pushes. The edits
  its levers imply are filed — propose-only — by
  `trim-context` and applied later by a human.
- **Runs standalone or at handoff.** `review-pr` runs it
  unprompted, once, immediately before its close question after a
  merge — no approval asked — but it runs just as well invoked by
  hand for any session id whose transcript is still on disk.
- **Approximate by design.** Result token counts are
  bytes ÷ 4 — adequate for *ranking* sinks, which is the
  decision the recommendations turn on. Treat the numbers as
  relative, not billing-exact (sinks are sized in tokens, not
  priced).
- **Sink labels can carry input fragments — scrub before
  writing the account.** The "Largest single results" labels
  are short heads of the call's input (a Bash command, a
  URL, a query). If a command or URL embedded a secret, that
  fragment could ride through the digest into a filed batch. When you cite a
  sink in a lever, summarize it by its **tool and target**
  (file, package, MCP method) rather than pasting a raw
  command/URL verbatim, and drop anything that looks like a
  credential. Keep secrets in env vars, not inline, so they
  never reach a label in the first place.
- Shell discipline (per `CLAUDE.md`): every command is a
  single bare call that reduces to an allow-glob — no `&&`,
  pipes, `$(…)`, or redirects.
