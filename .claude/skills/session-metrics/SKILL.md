---
name: session-metrics
description: Capture where a session spent its tokens and recommend concrete trims. The deterministic core — resolve the session's on-disk transcript, read it (and its sub-agent transcripts) in its own process so the huge file never enters context, and rank the costliest tools / largest single results / per-sub-agent rollup plus the repeated command shapes worth hardening into a tool (ranked by result size, not call count, each labeled context / context (failures) / wall-clock / prompt-churn) — runs as the committed `session_metrics.py` tool under `.claude/tools/` (`make session-metrics SESSION=<uuid>`). The skill drives that tool, then files each trim lever it identifies as its own parked Linear issue under the `Trim levers` milestone, fingerprinted and written through the zero-echo `trim_levers.py` writer (appending this session's evidence to a lever that already exists rather than duplicating it), which `trim-context` later folds into propose-only skill-improvement tasks. Runs at the end of a `review-pr` session (its handoff offers it) or standalone for any session id.
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

Each recommendation lands as **its own parked Linear issue** —
one lever, one issue, stamped with the `Trim levers` milestone
and keyed by a `**Fingerprint**:` — which `trim-context` later
folds into propose-only skill-improvement Backlog tasks, on its
own or as `housekeeping`'s Session Metrics step. This is the
feedback loop that systematizes, every session, the by-hand
analysis that motivated this work.

There used to be a single Linear inbox **document** collecting
one entry per session. It is retired: with roughly ten parallel
sessions a day it outgrew the harness's tool-result cap between
mining passes (67.0k characters at the last one), so the
consumer had to spill it to disk and mine it with a throwaway
script. Per-lever issues remove the growth entirely, and make a
lever's **recurrence** an accumulating fact on one issue instead
of a pattern a later pass has to re-detect in prose.

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

- **Totals** — input / output / cache-write / cache-read
  tokens and turn count, summed across every assistant turn.

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

**3. There is no inbox document to resolve.** Levers are filed
as **parked issues**, and the writer
(`.claude/tools/trim_levers.py`) reads `LINEAR_API_KEY`,
`LINEAR_PROJECT_ID`, `LINEAR_TEAM_ID` and
`LINEAR_ASSIGNEE_ID` from the environment itself, erroring
clearly by name if one is unset — so there is nothing to
`printenv` here and no doc-id no-op path.

The one prerequisite that is **not** self-healing: the
`Trim levers` project milestone must exist. The writer
refuses with a message naming the milestones that do exist
rather than filing into the wrong place, so if that error
appears, print the tool's summary anyway (the run is not
wasted) and report the missing milestone as the blocker.

**4. Compose the recommendations.** This is the skill's
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
  the right default — a single occurrence is a lever to file,
  not a tool to write. The one exception is a shape
  expressible only in **forbidden** forms (an inline
  interpreter one-liner, a stopgap grant, a compound): every
  repetition of those is a prompt that can never be firmed, so
  one occurrence is enough.

- **The observations you kept during the session** — per
  `CLAUDE.md`'s "track consumption ideas as you go" habit,
  the running notes on what felt wasteful. The sinks say
  *where*; your notes say *why* and *what to change*.

Write each recommendation as grounded prose: name the
sink (or candidate shape), state the lever
(transport-agnostic where it applies — "request less",
narrowest method, field-select, read by slice, scope the
sub-agent, harden into a tool), and where you can, name the
**concrete** skill step or convention-doc rule to edit.
Keep it tight; this is a recommendation, not a patch.

**5. File each lever as its own parked issue.** One lever, one
issue, keyed by a `**Fingerprint**:` — never one entry
bundling a whole session. Every write goes through the
committed writer, which prints **one line** per write
(identifier and url) instead of echoing a body back:

```sh
python3 .claude/tools/trim_levers.py probe --fingerprint <domain>:<slug>
```

- **`NONE`** (exit 1) — this lever is new. Write its body to a
  scratchpad file and file it:

  ```sh
  python3 .claude/tools/trim_levers.py file \
      --title '<the lever, as an imperative>' \
      --fingerprint <domain>:<slug> \
      --body-file <scratchpad>/lever.md
  ```

  **No `--touches`.** The declared-scope glob field is
  retired (`CLAUDE.md` → "Structured filing fields") — name
  the skill or tool the lever would edit in the body prose,
  which is what `trim-context` reads when it folds.

- **`MATCH ENG-###`** — this lever has been seen before.
  **Append this session's evidence** to the issue that already
  exists rather than filing a duplicate:

  ```sh
  python3 .claude/tools/trim_levers.py append-evidence \
      --fingerprint <domain>:<slug> \
      --evidence-file <scratchpad>/evidence.md
  ```

  A `MATCH` is not one situation. The writer distinguishes
  three dispositions and each takes a **different** action, so
  read the state in the probe's output:

  - **parked** (open, milestone set) — accumulates. Run
    `append-evidence`; it grows that lever.
  - **folded** (closed, its content copied into an aggregated
    task) — the fix is queued but the lever record is
    discharged. `append-evidence` here prints
    `NOTED … no longer parked` and **exits 0** — it does not
    fail, and it records nothing. So if this session saw the
    lever again, **run `file`**: it proceeds on a folded match
    and names what it supersedes. Folding is not permanent;
    only rejection is.
  - **rejected** (closed as `Canceled` with a reason) — settled.
    `file` hard-refuses here, which is the intent. Read the
    closing reason and **do not refile**. If this session's
    evidence genuinely defeats it, say so in the report and
    leave it to a human — reopening someone's reasoned
    rejection is not an unattended act.

  The middle case is the one that is easy to get wrong, and it
  is routine rather than exotic: `housekeeping` folds every
  pass, so any lever spends the window between its fold and its
  PR landing in exactly that state. Treating the `NOTED` line
  as a refusal would silently drop every recurrence in that
  window — which is the failure the per-lever design exists to
  prevent.

  Why a fold produces this at all: it copies each
  `**Fingerprint**:` line into the aggregated task, so from then
  on the probe legitimately matches two issues (the closed
  original and the open aggregate) and neither is parked.

The fingerprint is `<domain-token>:<slug>`, and its first token
must be **dotless** — Linear linkifies a hostname-valid
basename and corrupts the stored key, which would break the
probe above. The writer refuses a dotted domain outright, so a
rejection there means rename the token (`feeds-http`, never
`http.rs`), per `docs/conventions/linear-automation.md` →
"Structured filing fields".

**Why a tool and not `save_issue`.** The MCP write echoes the
entire stored body back on every call, even one that sends no
body — a fixed cost `patch` does not reduce, which *compounds*
on an accumulator: five touches on one issue measured ≈53k with
per-touch cost rising monotonically. `append-evidence` does its
read-modify-write **inside the tool process**, so a growing
lever body never enters a transcript at all. The convention doc
states this carve-out explicitly (same section → "Carve-out: a
high-volume automated pipeline may bypass the MCP").

**Keep each lever body compact.** It is a proposal, not a
report: the lever, the sessions and figures that motivate it,
and the concrete skill or convention-doc edit it implies.
Compactness is a discipline from day one here — the document
this replaced grew past the tool-result cap precisely because
nothing bounded per-entry length.

**No relations, no blocking edges.** Parked levers are exempt
from the meta batch and its edge until `trim-context` folds
them; the writer has no relation mutation at all.

**A re-run is safe.** The probe makes this idempotent: a second
run for the same session finds each lever already filed and
appends evidence rather than duplicating. There is no
duplicate-header hazard of the kind the old document-append had,
and no size gate anywhere — metrics feed the improvement loop,
so the producer records unconditionally.

**6. Report** in one line — the session, the top sink, how many
levers were filed new versus appended-as-evidence, and any lever
skipped because it matched a closed (rejected) one.

## Notes

- **No source edits.** This skill writes only to Linear (the
  parked lever issues) and never authors a code or skill diff,
  never commits, never pushes. The skill-improvement edits
  its recommendations imply are folded — propose-only — by
  `trim-context` (run on its own or by `housekeeping`) and
  applied later by a human.
- **Runs standalone or at handoff.** `review-pr` offers it
  after its `firm-perms` gate (recommended, via
  `AskUserQuestion`), but it runs just as well invoked by
  hand for any session id whose transcript is still on disk.
- **Approximate by design.** Result token counts are
  bytes ÷ 4 — adequate for *ranking* sinks, which is the
  decision the recommendations turn on. Treat the numbers as
  relative, not billing-exact (no dollar figures are
  reported).
- **Sink labels can carry input fragments — scrub before
  writing to Linear.** The "Largest single results" labels
  are short heads of the call's input (a Bash command, a
  URL, a query). If a command or URL embedded a secret, that
  fragment could ride into a filed lever body. When you cite a
  sink in a lever, summarize it by its **tool and target**
  (file, package, MCP method) rather than pasting a raw
  command/URL verbatim, and drop anything that looks like a
  credential. Keep secrets in env vars, not inline, so they
  never reach a label in the first place.
- Shell discipline (per `CLAUDE.md`): every command is a
  single bare call that reduces to an allow-glob — no `&&`,
  pipes, `$(…)`, or redirects. The writer takes its ids from
  the environment itself, so there is nothing to `printenv`.
