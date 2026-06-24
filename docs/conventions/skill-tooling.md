# Skill tooling

The deterministic helpers behind skills — transcript parsers, branch
checks, doc renderers — are **glue for Claude**, not part of the
on-chain product. Two principles govern where they live and when an
MCP-driven workflow graduates into one.

## Skill tools and hooks are Python under `.claude/tools/`

A skill's deterministic helper parses a transcript, checks a branch
name, rewrites a doc. When it lives as a Cargo **workspace member** it
gets pulled into every `cargo build` / `cargo clippy` / `cargo test`
of the actual on-chain project, slowing the compiles that matter and
coupling skill tooling to the program's toolchain.

- **Every tool or hook invoked by a skill is written in Python**, not
  Rust. Precedent: the compound-shell guard hook
  `.claude/hooks/no_compound_bash.py` is Python, and the repo already
  lints Python with `ruff-check` / `ruff-format` in
  `cfg/pre-commit-lint.yml` — so no new toolchain is needed.
- **They live in `.claude/tools/`**, co-located with `.claude/hooks/`
  and `.claude/skills/` because they exist specifically for Claude, and
  explicitly **outside** the Cargo workspace — **never** a member of
  `Cargo.toml`. The `ruff` pre-commit hook has no `files` filter, so it
  already covers `.claude/tools/**` by default.
- **Stdlib only** where practical (JSON + filesystem), so a tool runs
  with a bare `python3` and needs no install step.
- **Cover them with stdlib `unittest`** in a sibling
  `test_<tool>.py`, runnable as `python3 .claude/tools/test_<tool>.py`
  (no pytest dependency). The `Bash(python3 .claude/tools/*)`
  allow-rule covers running both the tools and their tests.
- A skill drives its tool through a stable interface — usually a
  `make` target (e.g. `make session-metrics`) so the skill's
  allow-rule (`Bash(make session-metrics:*)`) is unchanged if the tool
  is later rewritten.

Today `.claude/tools/` holds `session_metrics.py` (the
`session-metrics` core), `init_pr_branch.py` (the `init-pr`
branch/worktree checks), and `run_quiet.py` (a generic quiet runner
that captures a noisy command's output to a log and surfaces only a
summary — see [context economy](context-economy.md)).
`tools/stage-backlog/` is the one remaining helper still under
`tools/` — its Python rewrite is tracked separately.

## MCP first for prototyping and fallback; harden settled workflows

The MCP servers (`mcp__github__*`, `mcp__claude_ai_Linear__*`, …) are
the right tool while a workflow is still being figured out, and the
right fallback for one-off or rarely-run operations. But once a
workflow is **established and repeated** — same calls, same shape,
every run — it should move out of per-step MCP calls into a
deterministic Python tool the skill drives.

This is the same rationale as [context economy](context-economy.md): a
fat MCP result is replayed as input on every later turn, and a
polled / repeated MCP call is paid per poll *and* per later turn; a
tool that returns only the narrow answer pays once, and a tool that
reads a large file (a transcript) in its own process keeps it out of
context entirely.

This is a guiding rule, not a mandate to rewrite every MCP call at
once. `session-metrics` itself nominates candidates: beyond ranking
token sinks, it flags **repeated, deterministic Bash command shapes**
as "hardening candidates," which `housekeeping` mines into
propose-only skill-improvement tasks — closing the loop from
"workflow we keep running by hand" to "tool we extracted."
