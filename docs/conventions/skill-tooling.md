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
`fleet_resume.py` (the fleet-resume launcher behind the `fleet` verb —
resolves the in-flight issues from Linear, skips the ones already open,
and opens and resumes the rest in one driver round trip; read-only
unless `--apply`), `iterm_api.py` (the one owner of iTerm automation,
over iTerm's Python API — the library is not stdlib and is not
installed, so this stays importable from ordinary `python3` and shells
out to the interpreter iTerm ships, which carries it; consolidating
`fleet_resume.py` and `session_dispatch.py` here retired AppleScript
from the toolbox entirely), `session_dispatch.py` (opens an iTerm tab
per session verb, in the dispatching session's own window, and types
each — the planning session's dispatch arm, authorized by the
operator's yes, and loud enough on failure to print the verbs it would
have typed),
`migration_collisions.py` (compares this branch's new
migration numbers
against other open PRs' before an enqueue — `--others-from-gh` runs that
open-PR read **inside its own process**, because the earlier
read-then-compare pair left a gap no sanctioned shell form could bridge
and re-emitting the listing put every PR's file list through context;
`--others <file>.json` remains for a caller that already holds the
inventory and keeps the compare network-free),
`convention_refs.py` (reports skill/doc citations whose target file or
anchor no longer resolves, distinguishing the two — it replaced four
prose bullets that took eight greps and ~1.2k per housekeeping pass to
print one line, and both `housekeeping` step 5 and `review-pr`'s
freshness lens call it so the periodic and PR paths cannot drift),
`planning_doc.py` (a scoped reader for the Planning document — the MCP
`get_document` has no slice accessor and returns a document that only
grows, so one measured read was a session's largest main-loop result at
≈7.9k for four short passages; ask a live planning session first, use
this when none is live), `check_home_paths.py` (the
committed-agent-material hygiene guard, wired
as a scoped pre-commit hook), `lens_preamble.py` (composes the standing
half of a lens brief from the
[sub-agent brief](sub-agent-brief.md) plus a skill's own committed
section, so a skill never reads either to quote it), and
`render_review.py` (measures or contact-sheets rendered deck pages
instead of reading them at print resolution — the one tool with an
optional dependency, per the lazy-import rule above), alongside the
`allowlist.py` / `housekeeping` / `cspell-audit` glue.
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
the whole system temp tree is exactly what the allowlist safety floor
exists to reject.

**Nothing will catch it later.** Nothing sweeps approvals into rules any
more — the `firm-perms` skill that used to is retired, and firming is now
an explicit `allowlist.py add`. So a recurring prompt that the operator
keeps approving one-off never surfaces as a pattern at all. This one was
found by hand-probing after the prompts got annoying, not by any
tooling. So the allow-rule is part of adding the tool, in the same PR,
or it does not happen.

One related note, so it does not get re-diagnosed:

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

### Exercising a Makefile macro — nothing lints or tests one

A Makefile **macro** (a multi-line shell fragment expanded into recipes)
sits in a gap: `shellcheck` reports "no files to check" for a diff that
only touches the `Makefile`, because it does not cover recipe or macro
shell, and the Makefile linter hook passes without analyzing shell
semantics at all. There is no harness either — `.claude/tools/tests/` is
Python-only. So a 16-line POSIX-sh macro can land with **no automated
check having read it**.

The only verification available is to run it, which means adding a
temporary driver target:

```make
.PHONY: macro-check
macro-check:
 @$(THE_MACRO); echo "result=[$$result]"
```

Three things to get right, because the improvised version is repeated and
easy to leave behind:

- **Write the target once and delete it in the same session.** One run
  added an equivalent target three separate times and invoked it eleven
  times; each variant is a fresh permission prompt, since the target name
  is part of the command.
- **Escape `$` as `$$`** in the recipe, or make expands it and the shell
  never sees the variable.
- **The `;` in that recipe is deliberate and is not a shell-rule
  violation.** Make runs each recipe *line* in its own shell, so the
  macro and the `echo` that reads its variable have to share one line;
  splitting them would put `$$result` in a shell that never set it. The
  one-bare-command rule is about what reaches the **Bash tool**, whose
  reusable allow-rule is what it protects — and that command here is
  `make macro-check`, which is bare. Recipe-internal shell is out of its
  reach, and `no_compound_bash.py` never sees it.
- **Say in the PR that the macro was verified this way**, since no gate
  records it — otherwise the diff looks checked when only the Python and
  Rust around it was.

The durable fix, when a macro becomes load-bearing, is the same one this
document argues for everywhere else: move the logic into a Python tool
under `.claude/tools/` where it can be tested, and let the Makefile
target call it.

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
as "hardening candidates," and **`/harden` is the consumer** — it
takes a candidate, demands the provenance that it actually recurred,
and emits the tested tool plus the skill reference that drives it.
That closes the loop from "workflow we keep running by hand" to "tool
we extracted"; before it existed the candidate list had no consumer
and sat in a report.

## Repeated prose gets one source and a freshness gate

The same argument applies to **prose**, not just to commands. Changing
a convention means editing the convention doc *and* every skill that
restates it — a hand-sync tax paid on every meta batch, and enforced
only by an agent remembering to look. One batch updated the same
search-shape rule in three separate files and the same spelling rule
in two.

So a block that is genuinely repeated gets **one source** under
`.claude/shared/`, and each skill marks the region it wants filled:

```markdown
<!-- render:begin fable-model-guard verb=plan -->
<!-- render:end fable-model-guard -->
```

`make render-skills` fills every region; `make render-check` fails on
any difference, so a hand-edited generated region is caught rather
than silently kept. It also fails on a **dangling** marker — unclosed,
mismatched, or naming a source that does not exist — because a region
that never renders is the same silent failure as a
committed-but-unwired guard hook. Unlike `hook-wiring`, this one reads
only committed files, so it is checkable in CI.

Substitution is a flat `{{name}}` replace and nothing more: no logic,
no inheritance, no partials. **A block that needs a conditional is not
one block — it is two.** An unresolved placeholder is refused rather
than emitted, since a literal `{{verb}}` reaching a rendered skill
would be read by an agent as instruction text.

**Extract sparingly, and only on verbatim repetition.** The
duplication here is thinner than it looks, which is worth recording so
the next pass does not over-build: `plan`'s and `init-pr`'s model
guards point in **opposite** directions — one catches a planning-tier
model burning an implementation run, the other an implementation-tier
model doing board work — so they are a complementary pair, not a
duplicate. Several "runs in the base repo" mentions likewise each say
something different about why. Extract when the prose is genuinely the
same in two or more places; otherwise leave it written out.
