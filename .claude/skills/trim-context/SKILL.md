---
name: trim-context
description: The fold analyst — `plan`'s bootstrap fold sub-agent, or run by hand. Reads the day's session digest (every session's numbers beside its own Session account, plus flagged CI lines), judges across all of them at once which trim levers clear the bar, dedups each candidate by fingerprint and by Backlog title, folds a near-match into the existing issue as a new part, and files the rest as small themed `Claude:` batches of about 4–5 parts, one `**Fingerprint**:` per part, straight into Backlog with no milestone. Never edits a skill or convention doc — filing a batch is the proposal.
disable-model-invocation: false
user-invocable: true
---

# `trim-context`

The **analyst** half of the context-economy loop. Sessions report;
the analyst judges. Every session's numbers and its own account of
where it could have gone better land in one deterministic **digest**
the daily upkeep pass writes
(`docs/conventions/context-economy.md` → "The daily digest"); this
skill reads that digest, decides what is worth filing, and files it.
A session files nothing that is only a trim, so no session spends
context checking what other sessions filed — the analyst sees the
whole day at once, which is where a shared sink is visible.

It runs identically by hand or as the `plan` bootstrap's background
sub-agent, and **asks nothing**. Filing a batch *is* the proposal: it
never edits a skill, a convention doc or `CLAUDE.md`.

## Input

The brief names the digest path — the upkeep report's `digest` line,
`<base>/.claude/session-metrics/digest-<YYYY-MM-DD>.md`. By hand, take
the newest `digest-*.md` there. No digest, or one with no session
blocks: report that and stop.

Resolve the filing destination, each with its **own** bare `printenv`
(a combined one prints only the first value on macOS / BSD); if any is
empty, say so and stop:

```sh
printenv LINEAR_TEAM_ID
printenv LINEAR_PROJECT_ID
printenv LINEAR_ASSIGNEE_ID
```

## Steps

**1. Read the digest once, whole.** It is the fold's entire input and
is bounded by construction (three rows per ranked list per session),
so this is the planned multi-region read the context-economy
convention licenses. Read the day header (flagged figures, flagged CI
lines), then every session block: its top sinks, largest results,
resident-prose share, hardening candidates with their cost kind, and
its **Session account** — the session's own statement of what it
would change, which says *why* where the numbers say *where*. A block
reading "no account" still has its numbers.

**2. Judge across the whole day.** A candidate is a lever: a concrete
edit to a named skill, tool or convention rule. The bar is unchanged —
a lever is **turn-eliminating**, **prose-subtracting** or
**hardening**, worth roughly **3k per occurrence**; a lever derived
from a planning session must be **quality-preserving** (a narrower
echo or a removed re-read qualifies; a model downgrade or a thinned
bootstrap read does not). Weigh in this order:

- **Recurrence across sessions.** The same sink in three blocks is one
  lever with three occurrences, not three levers — this is what the
  per-session filing never saw.
- **Turns before payload.** A lever that collapses a judgment-free
  multi-turn flow into one tool run outranks one that shrinks a result
  (same doc → "Session length is itself a cost lever"). Rank by
  estimated dollars, not tokens.
- **A hardening candidate** with measured recurrence names `/harden`
  as its edit; state its cost kind, since a `wall-clock` or
  `prompt-churn` candidate is not a context saving.
- **A flagged CI line** is a candidate like any other: name the
  workflow or job it would change.

An account's suggestion is evidence, not a verdict — reject one that
the numbers do not support. Drop below-bar noise silently.

**3. Verify each candidate against HEAD.** The digest is yesterday's;
a sibling PR may have landed the fix. Ask whether the **capability**
exists, never whether a path does — a rename is the usual reason a
landed fix reads as missing. A few cheap probes (a `--help`, an `ls`,
one scoped search); a landed candidate is dropped.

**4. Dedup every candidate twice.** Give each a fingerprint
`<domain-token>:<slug>` (dotless domain; a resident-prose lever keys on
the line it names, `claude-skills:resident-<slug>`). Then:

```sh
python3 .claude/tools/trim_levers.py probe --fingerprint <domain>:<slug>
```

- `NONE` (exit 1) — new; go on to the title check.
- A **Canceled** match — a recorded rejection. Drop the candidate
  unless today's evidence plainly defeats the recorded reason; then
  say so in the report and leave it to a human.
- An **open** match — the lever is already a part of a live batch;
  add nothing.
- A **Done** match — the earlier fix shipped and the sink recurred;
  treat it as new, naming the shipped issue in the part.

Then check the **open Backlog by title**, since a near-duplicate under
a different fingerprint is the likelier miss:

```sh
python3 .claude/tools/linear_issue.py find \
  --query '<two or three key words>' --state Backlog
```

A batch or part that is plainly the same work as an open issue is
**folded into it as a new part**, never filed as a sibling: compose
that one part continuing the issue's numbering, then append it
zero-echo —

```sh
python3 .claude/tools/trim_levers.py compose \
  --parts-file <scratchpad>/fold-<n>.md \
  --out <scratchpad>/fold-<n>-body.md --start <next part number>
```

```sh
python3 .claude/tools/linear_issue.py append --id <ENG-###> \
  --file <scratchpad>/fold-<n>-body.md
```

**5. Compose the batches — one call.** Write every remaining part to
one parts file, a `## <fingerprint> | <imperative title>` section each:
the lever in a sentence, the evidence (sessions, figures, the account
lines it rests on — summarize a sink by tool and target, never a raw
command or URL, which can carry a secret), and the concrete edit. Group
them into themes of **about 4–5 parts** by the surface they change, in
a groups file:

```json
[{"title": "Claude: <theme>", "parts": ["<domain>:<slug>", "..."]}]
```

```sh
python3 .claude/tools/trim_levers.py compose \
  --parts-file <scratchpad>/parts.md \
  --groups-file <scratchpad>/groups.json --out-dir <scratchpad>/tasks
```

It numbers each batch's parts from 1, demotes their headings, writes
each part's `**Fingerprint**:` exactly once, and refuses a part in no
batch or in two. A batch over five parts is an advisory, not a
refusal. Order parts, and batches, turns-eliminated first.

**6. File each batch** — exactly the paths the compose summary named,
never a glob of the directory (it flags a stale file left there):

```sh
python3 .claude/tools/linear_issue.py create --title 'Claude: <theme>' \
  --body-file <scratchpad>/tasks/01-<slug>.md --state Backlog --priority 1
```

**Backlog with no milestone**: the analyst is the planning session's
own sub-agent and its output is already folded, so the park-then-
promote hop executor filings take does not apply. The `--title` is the
whole title, prefix included. No relations, ever.

**7. Record a rejection that should stick.** A candidate that clears
the bar on its face but is rejected on judgment — a standing decision,
a quality cost — is filed **Canceled** with its fingerprint and the
reason, so the next probe drops it instead of re-litigating:

```sh
python3 .claude/tools/linear_issue.py create --title 'Claude: <lever>' \
  --body-file <scratchpad>/reject-<slug>.md --state Canceled
```

Below-bar noise gets no record; it is simply not filed.

**8. Report as your final message** (a sub-agent's last message is its
whole return), at most thirty lines: the batches filed with their
ENG-###s and part counts, the parts folded into existing issues, the
rejections recorded and why, and the sessions and dollars the digest
covered. The planning session names the new batches in its review ask.

## Notes

- **No source edits.** This skill writes only to Linear and never
  commits or pushes; the edits it proposes land later through a
  normal PR.
- **No blocking edges** (`CLAUDE.md` → "Blocking relations").
- **Shell discipline** (`docs/conventions/shell-commands.md`): every
  command is one bare call — no `&&`, pipes, `$(…)` or redirects.
