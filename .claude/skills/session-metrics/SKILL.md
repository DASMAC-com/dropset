---
name: session-metrics
description: Capture where a session spent its tokens and recommend concrete trims. The deterministic core — resolve the session's on-disk transcript, read it (and its sub-agent transcripts) in its own process so the huge file never enters context, and rank the costliest tools / largest single results / per-sub-agent rollup plus the repeated command shapes worth hardening into a tool — runs as the committed `session_metrics.py` tool under `.claude/tools/` (`make session-metrics SESSION=<uuid>`). The skill drives that tool, then writes narrative trim recommendations — grounded in the ranked sinks and hardening candidates plus the observations the model kept during the session — into the Linear "Session Metrics" inbox document, which `housekeeping` later mines into propose-only skill-improvement tasks. Runs at the end of a `review-pr` session (its handoff offers it) or standalone for any session id.
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

The two land together as one entry in a Linear inbox
document, which `housekeeping` drains into propose-only
skill-improvement Backlog tasks. This is the feedback loop
that systematizes, every session, the by-hand analysis that
motivated this work.

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
- **Hardening candidates** — the repeated `Bash` command
  shapes (grouped by normalized prefix), flagged
  `deterministic` when they're string/path/env logic worth
  porting into a tool (per `CLAUDE.md` → "Skill tooling").
  This is what nominates a settled, repeated workflow —
  the `git worktree list` / branch-validation kind of
  sequence — for extraction.

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

**3. Resolve the inbox doc id** from the environment, on the
same bare-`printenv` rule as the other Linear ids (one
variable per call — never a combined `printenv A B`):

```sh
printenv LINEAR_SESSION_METRICS_DOC_ID
```

If it's empty, say so and stop — the prerequisite isn't set
up, and the skill no-ops with a clear message rather than
guessing a doc id. (Still print the tool's summary so the
run isn't wasted.)

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
  step that emits it.
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

**5. Append a dated entry to the inbox** — never clobber
existing content. Read the doc **live** with
`mcp__claude_ai_Linear__get_document` (id = the resolved
value); never reuse a stale snapshot. Build the new body
from the body you just fetched, **adding** one entry and
changing nothing else, then save with
`mcp__claude_ai_Linear__save_document` (id = the resolved
value, literal newlines). If the doc `updatedAt` is newer
than when you fetched it, re-fetch and rebuild rather than
overwriting a concurrent edit.

The entry is **unchecked** (`- [ ]`) so `housekeeping` sees
it as unprocessed work; housekeeping ticks it once consumed.
Use this shape (one entry per session):

```md
- [ ] <date> · <branch or PR> · session <short-uuid>
  - Measured: in/out/cache tokens, cache-hit %, top sinks
    (tool → approx size), hardening candidates
  - Recommends: <tailored trim guidance + tool-extraction
    candidates + the concrete skill / convention-doc edits
    it implies>
```

Use today's date (from your environment's current date) and
the branch or PR number this session worked. Keep the
`Measured:` line a faithful digest of the tool's summary;
put the prose in `Recommends:`.

**6. Report** in one line — the session, the top sink, and
that the entry was appended (or that the skill no-op'd
because the doc id was unset).

## Notes

- **No source edits.** This skill writes only to Linear (the
  inbox document) and never authors a code or skill diff,
  never commits, never pushes. The skill-improvement edits
  its recommendations imply are filed — propose-only — by
  `housekeeping` and applied later by a human.
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
  fragment could ride into the shared inbox doc. When you
  write the `Measured:` line, summarize a sink by its **tool
  and target** (file, package, MCP method) rather than
  pasting a raw command/URL verbatim, and drop anything that
  looks like a credential. Keep secrets in env vars, not
  inline, so they never reach a label in the first place.
- Shell discipline (per `CLAUDE.md`): every command is a
  single bare call that reduces to an allow-glob — no `&&`,
  pipes, `$(…)`, or redirects; resolve the doc id with a
  bare `printenv`, one variable per call.
