---
name: harden
description: Turn a proven ad-hoc command shape into a committed, tested Python tool — the consumer for the hardening candidates session-metrics already ranks. Takes a candidate (a repeated command shape, or a named workflow), demands provenance that it actually recurred, writes the tool under .claude/tools/ with unittest coverage and a real assertion over an embedded fixture, wires the skill reference that will drive it, and verifies it runs before committing. Refuses to harden a shape with no measured recurrence.
user-invocable: true
---

# `harden`

**Codify a proven ad-hoc flow into a permanent, tested
artifact.**

The repo already has both halves of this loop except the
middle. The tooling convention says a workflow that is
established and repeated should become a Python tool. And
`session-metrics` already **produces the signal** — it ranks
repeated command shapes worth hardening, each labelled by cost
class (context / context (failures) / wall-clock /
prompt-churn).

Nothing consumed that signal. It sat in a report and a human
had to decide to act on it, which meant the recurring manual
step in the meta loop stayed manual. This verb closes the gap.

## Input

A candidate, in any of these forms:

- a **command shape** from a `session-metrics` hardening list
  ("14 invocations of `psql -v … -f …`");
- a **named workflow** the operator describes ("verifying a
  provisioned Grafana instance");
- nothing — in which case run `/session-metrics` for this
  session first and take its top candidate.

## Step 1: demand provenance

**Refuse to harden a shape with no measured recurrence.**
This is the guard that keeps the tool directory from filling
with speculative helpers, and it is not a formality — the
skill-tooling convention is explicit that MCP and ad-hoc shell
are the right answer *until* a workflow is established and
repeated, and hardening early is how a directory accumulates
tools nothing drives.

State, before writing anything:

- **how many times** the shape ran, and in how many distinct
  sessions;
- **the cost class** it was flagged under;
- **why the ad-hoc form cannot reduce to an allow-rule**, if
  the class is prompt-churn.

Two or more sessions, or a decisive single-session count, is
the bar. One occurrence is a candidate to record, not to
build — file it as a trim lever instead.

**The exception, stated so it is not argued each time:** a
shape that can only be expressed in **forbidden** forms —
inline interpreter one-liners, a stopgap grant, a compound —
is worth hardening on the first occurrence, because every
repetition of it is a permission prompt that can never be
firmed. `grafana_check.py` came from exactly that case.

## Step 2: design the narrow tool

Per `docs/conventions/skill-tooling.md`:

- **stdlib Python only**, under `.claude/tools/`;
- **never a Cargo workspace member** — it must not compile
  with the on-chain project;
- one clear subcommand set, and a **required mode** wherever a
  default would be "print everything";
- **summarize by default, gate detail behind a flag** — the
  tool is a tool-result generator, so it inherits that rule
  from `docs/conventions/context-economy.md`;
- a **non-zero exit** when the answer is "something is wrong",
  so the tool can be used as a gate rather than only read as a
  report.

Reuse before inventing: if the output shape is
"slice this text", delegate to `read_result.py`'s renderers
rather than writing a second `--section` that behaves subtly
differently.

## Step 3: write the test alongside, with a real assertion

**Embed a fixture and assert something that would actually
break.** A test that only checks the tool runs without raising
is worse than no test, because it reports confidence it has
not earned.

The bar for each new tool:

- a fixture in the test file itself — no network, no live
  service, no dependency on this checkout's state;
- at least one assertion that **fails if the core logic is
  wrong**, not merely if it crashes;
- a case for the **failure path** — the refusal, the non-zero
  exit, the malformed input;
- where the tool guards a credential or a destructive action,
  a test that the guard **cannot be bypassed** by the obvious
  route.

Then run the whole suite, wrapped:

```sh
python3 .claude/tools/run_quiet.py -- make tools-tests
```

**Expect the first run to find something.** On the two tools
most recently hardened, the self-test and the suite each
caught a real defect before commit — a guard whose escape
marker defeated its own deny tier, and a parser that reported
zero rules on a valid file. That is the test doing its job at
the only moment it is cheap.

## Step 4: wire the reference that drives it

A committed tool nothing points at is **inert**, which is the
same failure mode the guard hooks have and the reason
`make hook-wiring` exists. So the tool is not done until:

- the **skill step** that will run it names it, with the exact
  invocation;
- the **convention doc** that owns the rule points at it, if
  the tool replaces prescribed prose;
- any **superseded guidance is retracted, not left standing**
  — if the tool exists because a convention claimed adequate
  workarounds, say plainly that the claim is withdrawn.

## Step 5: verify, then commit

Run the tool **against real repo state** once, not only
against its fixture — the fixture proves the logic, this
proves the wiring. Then lint the changed set and commit:

```sh
python3 .claude/tools/run_quiet.py -- \
  python3 .claude/tools/lint_paths.py --changed
```

Report: the tool's path, the candidate it discharges (with its
fingerprint if it came from a parked lever), the test count,
and what the real-state run returned.

**If the candidate came from a parked trim lever, say so** —
that lever is now dischargeable, and `trim-context` should
close it rather than folding it into a task that proposes
building what already exists.
