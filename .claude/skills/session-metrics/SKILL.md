---
name: session-metrics
description: The shared closing tail every session runs immediately before its close question or final state write — review-pr's post-merge tail, plan's close-out, wrap, and the architect, explore (audit-scope) and chore handoffs. Derives the session id, prints the `session_metrics.py --brief` cost estimate verbatim, and writes the session's own `Session account` block (three to six lines on what was waste and what it kept having to do). Judges no lever, files nothing, asks nothing; the daily digest lifts the block and `trim-context`, the fold analyst, files across every session.
disable-model-invocation: false
user-invocable: true
---

# `session-metrics`

The tail every closing path shares. **The session makes no judgment
about its own metrics**: the tool prices the session, the session
says in its own words what it kept running into, and the fold
analyst (`trim-context`) judges across the whole day's digest. The
cost line on the issue is the close tool's write, not this skill's.

## Steps

**1. Derive the session id** from your scratchpad path: the
directory name immediately above `scratchpad`, a bare UUID. An
exported `CLAUDE_SESSION_ID` works too. A planning session computes
it instead, never listing the projects directory:

```sh
python3 .claude/tools/resolve_session.py --daily-id plan
```

**2. Run the brief and print its paragraph verbatim** — never
restated, rounded or summarized:

```sh
python3 .claude/tools/session_metrics.py --brief --session-id <uuid>
```

It reads the transcripts in its own process, so nothing but the
paragraph enters context. It prices every substrate at Bedrock rates
and labels a seat session's figure "at Bedrock rates, not billed".

**3. Write the account**, ending the message with:

```text
## Session account

- <the thing>
```

Three to six lines, in the session's own words, on **what was waste
and what it kept having to do** — name the thing, not the fix
("re-read the review-pr step 22 tail three times", not "split
step 22"). These are ideas, not filings: nothing goes to Linear, no
lever is judged or ranked, no question is asked. The heading is
fixed because the digest finds it deterministically, from the
heading through the next one, at most 40 lines. Name a payload
by tool and target, never a raw command or URL, since a label can
carry a credential into the digest.

That is the whole skill.

## Where the cost line goes

- **A close through the tool** passes the same id, and the line lands
  after the state write, zero-echo:
  `python3 .claude/tools/close_session.py --state <state> --session-id <uuid>`.
- **A chore** has no close tool: print
  `session_metrics.py --cost-line --session-id <uuid>`, `Write` the
  line into the handoff note, and append that with
  `linear_issue.py append`.
- **A planning session** has no issue: its close-out rewrite carries
  the line under `Session-metrics state`.

The line is `**Cost actual**: $<usd> estimate, …` on Bedrock and a
`**Token profile**` on the seat (`docs/conventions/context-economy.md`
→ "Cost bands"). An id that resolves to no transcript skips the line
and says so.

## Notes

- **An estimate, tuned daily.** Tomorrow's digest is the accurate
  figure and captures the turns after this one. The brief and the
  launcher's exit epilogue price through the same `RATES_BY_MODEL`
  table, which the digest calibrates each morning against the
  previous day's Bedrock bill; filing the rate update when the
  calibration flags is the analyst's standing duty.
- **The full report** — `make session-metrics SESSION=<uuid>`
  (`ARGS=--json`) — is for mining a session by hand and is not part
  of the tail; the tool's docstring documents each line. `/harden`
  reads its candidates from the digest.
