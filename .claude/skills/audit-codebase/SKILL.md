---
name: audit-codebase
description: Audit a defined scope of the codebase for magic numbers, stale comments, and modularity, with adversarial sub-agent cross-checking, producing an incremental checklist.
disable-model-invocation: true
user-invocable: true
---

# `audit-codebase`

Run a structured audit over a defined scope of
the codebase and produce a checklist of findings
that can be addressed incrementally. Use when a
major milestone lands, a new feature ships, or
before declaring a subsystem "stable".

## Input

Required:

- **Scope** — the paths, feature, or module to
  audit (e.g. "the swap flow", `src/picker/`,
  "the whole codebase"). If missing, stop and
  ask the user.

Optional (always ask if not provided):

- **Extra focus areas** — anything the user
  wants weighted heavily (e.g. "look hard at
  error handling in the RPC path", "I'm worried
  about race conditions in the balance fetch").

## Steps

1. Collect scope and ask for any extra focus
   areas. Do not start the audit without an
   explicit scope.

1. Draft an **audit plan** that lists:

   - The exact paths/modules in scope.
   - The default checks (magic numbers, stale
     or wrong comments, modularity /
     extensibility).
   - Any user-specified extra focus areas.
   - The sub-agents that will run in parallel
     and what each is responsible for.

1. Show the plan to the user and have them
   confirm or revise it. **Do not begin the
   search phase until the user approves.**

1. Ensure the findings directory exists and is
   gitignored:

   ```sh
   mkdir -p .audits
   grep -qxF '.audits/' .gitignore || \
     echo '.audits/' >> .gitignore
   ```

1. Run the audit by spawning parallel sub-agents
   via the `Agent` tool (single message, multiple
   calls). At minimum:

   - **Magic numbers / hardcoded constants** —
     unnamed values that should be named or
     configured.
   - **Comments** — comments that lie about,
     contradict, or no longer match the code.
   - **Modularity / extensibility** — coupling,
     duplication, abstractions in the wrong
     place, hidden dependencies.
   - **One sub-agent per user-specified extra
     focus area.**

   Each sub-agent must return findings with file
   path, line number, severity (high/medium/low),
   and a one-line rationale.

1. **Adversarial cross-check.** Spawn a fresh
   sub-agent that receives the collected
   findings and the scope, and is told to act
   adversarially:

   - Challenge weak or speculative findings.
     Flag false positives.
   - Identify issues the first pass missed.
   - Push back on rationale that doesn't hold up.

   If the cross-check produces material
   disagreements, iterate: re-spawn the relevant
   topic agent with the challenge and have it
   defend or retract. Continue until findings
   stabilize (consensus, not just one round).

1. Write the consolidated findings to:

   ```
   .audits/<YYYY-MM-DD>-<scope-slug>.md
   ```

   Format as a markdown checklist grouped by
   category. Each item:

   - A `[ ]` checkbox so it can be ticked off
     incrementally.
   - A clickable file:line reference.
   - Severity tag.
   - One-line rationale.

   Document header should record the scope,
   extra focus areas, and date.

1. Print the path to the audit document and a
   short tally (e.g. `7 high, 12 medium, 4 low
   — saved to .audits/2026-05-22-swap-flow.md`).

## Notes

- The audit document is intentionally
  gitignored. Findings get addressed in normal
  PRs; the doc is a working scratchpad, not a
  deliverable.
- If a prior audit doc exists for the same
  scope, read it first and skip items already
  closed — surface only new or still-open
  findings.
- Audits are read-only. This skill never edits
  source files; it only writes the findings
  document.
