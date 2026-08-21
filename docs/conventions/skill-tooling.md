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

  Where a tool genuinely needs a third-party package — `render_review.py`
  needs Pillow to touch pixels at all — **import it lazily, at the use
  site**, so the module still imports without it. Three things follow,
  and they are what keep the exception from eroding the rule: the
  dependency-free paths (argument parsing, path/ordering logic) keep
  working and stay testable; the tests that *do* need it are guarded
  with `unittest.skipUnless` so `make tools-tests` passes either way;
  and the failure, when it comes, is one clear line naming the install
  rather than an `ImportError` traceback. CI's lint job installs
  `pre-commit` and nothing else, so a hard import would make the whole
  suite un-runnable there.

- **Cover them with stdlib `unittest`** in `.claude/tools/tests/`
  (one `test_<tool>.py` per tool), run via `make tools-tests` (no
  pytest dependency). The tests `import <tool>` bare, so discovery uses
  the tests dir as start and `.claude/tools` as the top-level
  (`-t .claude/tools`) to keep those imports resolving — an empty
  `tests/__init__.py` marks the package.

- A skill drives its tool through a stable interface — usually a
  `make` target (e.g. `make session-metrics`) so the skill's
  allow-rule (`Bash(make session-metrics:*)`) is unchanged if the tool
  is later rewritten.

Today `.claude/tools/` holds `session_metrics.py` (the
`session-metrics` core), `init_pr_branch.py` (the `init-pr`
branch/worktree checks **and**, under `--link-env`, the two
operator-file symlinks `frontend/.env.local` and
`infra/localnet/secrets.local.env` — so it is not purely
read-only),
`run_quiet.py` (a generic quiet runner that captures a noisy command's
output to a log and surfaces only a summary — see
[context economy](context-economy.md)), `review_diff.py` (`review-pr`
step 5's diff-and-freshness gate, which also **owns** the three path
lists that decide the review's excludes and which CI-mirroring gates
run), `board_batch.py` (the planning session's
batched board writes and its compact board read — `list`, `fields`,
`priorities`, `edges`; it exists because every MCP write echoes the
issue's whole body back, and `issueUpdate` selecting `success` alone
does not), `search_source.py` (the one scoped-search
shape, which takes its exclude lists from `review_diff.py`),
`lens_preamble.py` (composes the standing half of a lens brief from the
[sub-agent brief](sub-agent-brief.md) plus a skill's own committed
section, so a skill never reads either to quote it), and
`render_review.py` (measures or contact-sheets rendered deck pages
instead of reading them at print resolution — the one tool with an
optional dependency, per the lazy-import rule above), alongside the
`firm-perms` / `housekeeping` / `cspell-audit` glue.
`.claude/tools/` is the single home for skill glue: there is **no**
top-level `tools/` tree.

A `make` target is the usual interface, but not the only one:
`review_diff.py`, `board_batch.py`, and
`init_pr_branch.py` are all driven directly with `python3`. Where a
skill does that, the allow-rule it needs is the **directory-wide**
`Bash(python3 .claude/tools/:*)` rather than a per-tool rule, so that
one rule covers every tool however its arguments vary.

Put that rule in the project scope like any other. A worktree needs no
copy of its own: `settings.local.json` is one shared file resolved
through worktrees to the main checkout, so a rule firmed anywhere is
live everywhere (see [local-integrations](local-integrations.md) →
"How settings files resolve across worktrees"). The criterion for
promoting a rule to `~/.claude/settings.json` is **cross-*repo*
portability** — you want it in other projects too — and nothing to do
with worktrees.

### Temp output goes in a `claude-<tool-name>/` directory

A skill-tool that writes temp output writes it to a directory named
`claude-<tool-name>/` under the system temp root — `run_quiet.py` to
`claude-run-quiet/`, `render_review.py` to `claude-render-review/`,
and so on. The matching **tool-scoped** Read glob goes into the
documented allowlist setup **in the same PR that adds the tool**:

```txt
Read(/var/folders/**/claude-run-quiet/**)
```

Two reasons this is structural rather than a preference.

**The temp root's prefix rotates.** On macOS the per-boot temp root is
`/var/folders/<hash>/T/…` and the hash changes **every boot**, so a
literal firmed path under it can never survive a reboot. The leading
`**` is what absorbs the rotating prefix; a per-tool directory name is
what keeps the glob narrow enough to grant. The broad
`Read(/var/folders/**)` form stays **refused** — an unscoped root over
the whole system temp tree is exactly what the `firm-perms` safety
floor exists to reject.

**Nothing will catch it later.** `firm-perms`' sweep can only
generalize approvals it can *see*; a recurring prompt that the operator
keeps approving one-off never surfaces as a pattern to harvest. This
one was found by hand-probing after the prompts got annoying, not by
any tooling. So the allow-rule is part of adding the tool, in the same
PR, or it does not happen.

Two related notes, so neither gets re-diagnosed:

- **The harvest blind spot is a known bound, not a bug to fix.** A
  sweep over approvals cannot see a prompt that was approved without
  being firmed. Rather than have `firm-perms` probe the known
  `claude-*` temp directories on every run — speculative work for a
  case this convention now prevents at the source — the limitation is
  recorded here and the fix is placed at tool-authoring time.
- **After a reboot, `allowlist.py cruft` flags previously-firmed
  literal `/var/folders/<old-hash>` rules under its
  `machine-path-stale` category.** That is **expected rot**, resolved
  by dropping those rules in favor of the tool-scoped globs above — not
  a fresh diagnosis.

Repo build tooling that is neither a workspace crate nor Claude-skill
glue lives **with what it serves**, not in a tooling tree:

- `brand-assets/copy-brand-assets.mjs` — a shared JS/Node build script
  run from the apps' `predev` / `prebuild` hooks. It copies the
  repo-root `brand-assets/` into each app's `public/` (skipping its own
  file, recursing into subdirectories), and both `frontend` and `decks`
  invoke it as `../brand-assets/…`. It lives among the assets it copies
  rather than in a separate scripts tree. A build script that only one
  app uses stays in that app's own `scripts/` (e.g.
  `frontend/scripts/`).

  `brand-assets/` holds **every** brand asset, not just the ones more
  than one app renders — an asset's home shouldn't depend on its current
  consumer count, or gaining a second consumer means noticing a split
  and moving a file. The whole folder is copied to every app; the set is
  tens of KB, so a per-app subset would buy nothing. Consequently each
  app's `public/` is **generated output and gitignored** — the frontend's
  wholesale, the deck's by entry glob with a carve-out for its committed
  `public/screens/` captures.

- **The linter/formatter configs stay in `cfg/`, not `.claude/`.**
  `cfg/` holds `pre-commit-lint.yml` (the pre-commit config) and the
  per-linter configs it points at — `taplo.toml`, `yamllint.yml`,
  `markdownlint.yml`, `cspell.yml`, `dictionary.txt`, `sqlfluff.cfg`.
  These are consumed by **pre-commit, the `Makefile`, and CI** (the lint
  job passes `--config cfg/pre-commit-lint.yml`), so they run
  independent of any agent — they are not Claude-skill glue. `.claude/`
  is *agent infrastructure* (skills, hooks, tools, settings); moving
  CI-critical lint config there would conflate build tooling with the
  agent directory and couple CI to it (a contributor who never runs
  Claude would still need `.claude/` intact for `make lint` to pass).
  So `cfg/` is the correct tool-agnostic home, by the same "lives with
  what it serves" rule as `brand-assets/`. This is recorded so the
  `cfg/` ↔ `.claude/` split isn't re-litigated: a move would touch a
  broad reference surface (`pre-commit-lint.yml`'s own `--config`
  paths, `Makefile`, both CI workflows, `.claude/tools/cspell_place.py`,
  and the docs) for no structural gain.

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
