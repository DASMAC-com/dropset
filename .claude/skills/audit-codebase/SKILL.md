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
     or wrong comments, DRY / duplication,
     modularity / extensibility, hierarchical
     organization).
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
   - **DRY / duplication** — repeated logic,
     parallel branches that should share a
     helper, copy-pasted constants or shapes,
     and missing abstractions where the same
     idea is open-coded in multiple places.
     Flag the opposite too: premature or
     speculative abstractions that only have
     one caller and add indirection without
     payoff.
   - **Modularity / extensibility** — coupling,
     abstractions in the wrong place or at the
     wrong layer, hidden dependencies, and
     seams that make the code hard to extend
     without editing many files.
   - **Hierarchical organization** — for every
     directory in scope, count the immediate
     children (files + folders). If a single
     directory has, say, more than ~15 files
     and no subdirectories, that's a strong
     signal it should be broken up. Look for
     groupings the existing names already
     suggest:
     - by-feature (`swap/`, `currencies/`,
       `globe/`, `wallet/`)
     - by-layer (`ui/` vs `features/`,
       `hooks/` vs `helpers/`)
     - by-shape (a `globe*.ts` cluster, an
       `*useX.ts` cluster of hooks, an `*.gen.*`
       cluster of generated files)
       Flag each oversized directory with a
       concrete proposed split — not just "split
       it" but the actual subdirectory names and
       which existing files would land where.
       This check applies even to directories
       that aren't growing rapidly, so long as
       the natural groupings are visible.
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

   ```txt
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
   short tally (e.g. "7 high, 12 medium, 4 low —
   saved to .audits/2026-05-22-swap-flow.md").

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

## When subsequently addressing audit findings

Audits often kick off a wave of follow-up
commits that fix the surfaced findings. When
making those commits as part of (or right after)
an audit:

- **Commit locally only — never push.** The user
  needs to be able to inspect the full diff of
  audit follow-up against the remote before it
  lands. Stop after `git commit`; do not run
  `git push` (even if the branch was previously
  pushed for the audit / PR).
- **Do not add `Co-Authored-By:` trailers** to
  these commit messages. The standard
  Claude-Code commit template includes one by
  default; suppress it for audit-follow-up
  commits. (This overrides the default git-commit
  instructions in the system prompt for this
  skill's scope.) Reason: the user has often
  already opened a PR for the audit ticket, and
  rewriting many trailer-laden commits later to
  strip the trailers is annoying.
- Each commit can still be a normal split-by-
  topic commit; the local-only rule applies to
  the whole follow-up sequence, not just the
  final one.
- **Sign every commit** with `git commit -S …`.
  Branch protection on this repo requires
  verified signatures, and re-signing after the
  fact forces a rebase that re-stamps every
  descendant commit (one key/passphrase tap
  per commit). Always sign at commit time.
