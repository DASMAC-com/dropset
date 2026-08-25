---
name: root-cause
description: Systematic debugging under two hard rules — no fix may be written before the root cause is investigated, and after three failed hypotheses the ARCHITECTURE is what gets questioned rather than the next hypothesis. Scope-locks edits to the module under investigation, keeps a numbered hypothesis ledger with the evidence that killed each one, and at the limit forces an explicit choice between continuing, escalating, and instrumenting-and-waiting rather than hard-stopping. For a bug whose cause is not yet known; a known cause just needs a fix.
user-invocable: true
---

# `root-cause`

**For a bug whose cause is not known.** If you already know
why it breaks, this verb is overhead — go and fix it.

Nothing else in this repo covers systematic debugging, and the
failure it prevents is one `review-pr` structurally cannot
catch: **stacking speculative fixes without a confirmed
cause.** By the time a review runs, the speculative fixes are
already written, they look like deliberate changes, and the
reviewer has no way to tell a fix from a guess that happened
to turn the test green.

## The two rules

Everything else here is procedure. These two are the skill.

### 1. No fix before the cause is investigated

**Do not edit product code until you can state the mechanism**
— which line, on which path, with which input, produces the
observed behavior.

"I think it's the cache" is not a mechanism. "The cache key
omits the mint decimals, so two markets with different
decimals collide on the same key" is.

Instrumentation, logging and tests are **not** fixes and are
allowed at any point. The prohibition is on changing behavior
in the hope that the symptom moves.

### 2. After three failed hypotheses, question the architecture

Keep a numbered ledger. When the **third** hypothesis is
disproved, stop generating a fourth of the same kind and ask a
different question: **is the model wrong?**

Three plausible, well-formed hypotheses failing in a row is
itself evidence — usually that some assumption shared by all
three is false. Concretely, ask:

- What am I assuming is **atomic** that isn't?
- What am I assuming happens **once** that happens twice, or
  zero times?
- What am I assuming is **ordered** that is concurrent?
- What do I believe about a **boundary** — a CPI, an await, a
  process edge, a serialization — that I have not verified?
- Is the thing I am debugging even **reached**? (Prove it
  with instrumentation before hypothesis four.)

## The hypothesis ledger

Maintain it in the conversation, and keep it short:

```txt
H1: stale .so from a previous build      → KILLED: rebuilt, symptom persists
H2: min_out rounding on the error path   → KILLED: path not reached
H3: decimals mismatch in the cache key   → KILLED: keys differ in the repro
--- three failed; questioning the architecture ---
```

Each entry carries **the evidence that killed it**, not just
the verdict. That is what makes the ledger worth keeping: it
stops a later turn re-proposing H1, and it is what you hand
over if the investigation escalates.

## Scope lock

**Edits are confined to the module under investigation** for
the duration. Widening the blast radius mid-investigation
means a symptom change can no longer be attributed — which
destroys the only signal you have.

If the evidence points outside the locked scope, that is a
finding: say so, re-state the scope explicitly, and continue.
Do not quietly start editing elsewhere.

## At the limit: choose, don't stop

When the architecture question has been asked and the cause is
still unknown, **do not hard-stop and do not keep grinding.**
Put the choice to the operator, explicitly — three options,
and say which you would take and why:

1. **Continue** — there is a specific next experiment worth
   running. Name it. "More of the same" is not this option.
1. **Escalate** — hand the ledger to a fresh perspective: an
   `architect` session if it smells structural, or a sub-agent
   fan-out briefed with the ledger and the negative results.
   The ledger is what makes this cheap rather than a restart.
1. **Instrument and wait** — the bug is real but not
   reproducible on demand. Land *observability only* (a log
   line, a counter, an assertion that will fire), say
   explicitly that no fix was made, and file the follow-up.

**Option 3 is a legitimate outcome, not a failure.** A session
that lands a well-placed assertion and no fix has done more
than one that lands a speculative fix and closes the issue.

## Finishing

When the cause **is** found, state it as a mechanism, then
hand off:

- the **fix** is ordinary work — implement it, and let
  `review-pr` review it;
- the **ledger** goes into the Linear issue as a comment
  (narrative belongs in a comment, not a body edit — see
  `docs/conventions/linear-automation.md`), because the
  disproved hypotheses are the expensive part and they are
  what a regression investigation will want;
- if the bug was invisible while the system rendered
  healthy, that is the **silence-is-not-success** shape —
  flag the observability gap as its own follow-up, since the
  fix closes the bug but not the blindness.
