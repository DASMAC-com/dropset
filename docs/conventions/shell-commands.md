<!-- cspell:word PIPESTATUS -->

# Shell commands

The guiding rule: **every Bash invocation should reduce to a
reusable allow-rule** (`Bash(prefix:*)`). A call that can't —
because of a compound, a substitution, a pipe, or a one-off
literal — is unique, so the user must approve it by hand *every
single time*. When you catch yourself about to run something that
won't generalize, stop and reshape it (split it, hoist the dynamic
part into a prior step or a tool, pass values literally) before
running it.

This applies to shell you **author**, not just shell you type
ad-hoc: snippets in skills, scripts, Makefile targets, and docs get
executed verbatim, so the same patterns below re-prompt forever when
baked into them. Write committed shell to the same standard — prefer
a sequence of bare commands that each reduce to a glob (or "run X,
read its output, then run Y with the value inline") over a clever
one-liner.

It applies to work you hand to a **sub-agent**, too. The whole
objective is **the fewest permission prompts possible** across the
session, and a spawned agent's Bash calls surface to you for approval
exactly like your own — but the agent doesn't inherit the project
instructions, so it will reach for the forbidden compounds unless told
not to. Brief every agent you spawn on these rules (see
[the sub-agent brief](sub-agent-brief.md)) so its calls reduce to
allow-rules too. A session that follows the rules and briefs its
agents on them prompts only for a genuinely novel command — which
`firm-perms` then memorializes so it never prompts again.

**The dedicated Grep / Glob tools aren't always present.** Native macOS
Claude Code builds (>= 2.1.117) drop them from the default tool palette
in favor of embedded Bash search
(<https://github.com/anthropics/claude-code/issues/52004>), and we do
**not** force them back on via `--tools` (that flag is replace-not-add,
so it would mean enumerating the whole built-in set in every launcher —
too brittle). So the "use the Grep tool" guidance below is conditional:
use Grep / Glob **when they exist**, but where they don't, fall back to
a **bare, single** `grep` / `find` Bash command — never `git grep` (the
bullet below), and never a piped compound. This holds on the
**main-loop** path, not only in the sub-agent brief: whichever agent
loses the Grep tool reaches for the same fallback, so bare `grep` is the
answer for both. Bare `grep` / `find` reduce to the retained
`Bash(grep:*)` /
`Bash(find:*)` allow-rules and prompt once; it's the `grep … | head` /
`find … | xargs` **pipes** that can't generalize and re-prompt forever.
The `Bash(grep:*)`, `Bash(find:*)`, `Bash(head:*)`, and `Bash(tail:*)`
allow-rules are kept for exactly this fallback.

Concrete rules:

- Prefer the dedicated tools — Read, Grep, Glob — over `cat`, `grep`,
  `find`, `ls` in Bash. They don't prompt for in-workspace paths. This
  includes *slicing* a file: use Read with `offset`/`limit` instead of
  `sed -n 'X,Yp'`, `awk 'NR>=X'`, `head`, or `tail`. Never shell out to
  `python3` / `node` / `jq` to read or edit JSON/config (including
  `.claude/settings.local.json`) — use Read + Edit/Write. Each such
  one-liner is unique and re-prompts forever. To find **over-length
  lines** for the MD013 80-col rule, don't reach for
  `awk 'length>80'` / `sed` either — run the markdownlint hook
  (`pre-commit run markdownlint-fix … --files <path>`, with
  `--config cfg/pre-commit-lint.yml`); it reports every MD013
  violation with its line number and reduces to the existing
  `Bash(pre-commit run:*)` rule.
- Searching file *contents* — prefer the **Grep tool**; where it's
  absent (the Grep / Glob caveat above) a **bare, single** `grep` is
  the fallback, but **never** `git grep`. This is the same rule the
  sub-agent brief carries
  (see [the sub-agent brief](sub-agent-brief.md)); it holds for the
  main agent too, so the convention is one and the same — the brief
  just restates it because a sub-agent doesn't inherit these
  instructions. Grep takes a real regex (alternation is `a|b|c`, not a
  shell-quoted `a\|b\|c`), reads any path you point it at, and prompts
  zero times. `git grep` looks blessed — it's a git subcommand, so it
  seems covered by the `git -C <path> <sub>` cross-checkout rule below —
  but it isn't: a clean single pattern only re-prompts until firmed, and
  a quoted `\|` alternation trips the per-subcommand `|` guard and can't
  be firmed at all. Reserve `git -C <path>` for **metadata** subcommands
  (`log` / `show` / `diff` / `status` / `ls-files`), never `grep`. This
  rule is enforced **mechanically** by the git-grep guard hook
  (`.claude/hooks/no_git_grep.py`) — see [the guard hooks](#the-guard-hooks)
  below.
- One command per Bash call. Avoid `&&`, `;`, and pipes when separate
  calls work; a chained command can't be generalized into a glob and
  always re-prompts.
- No command substitution. `$(...)` and backticks block globbing —
  compute the value in a prior step (or a tool) and pass it literally.
- Avoid redirects (`>`, `<`, here-strings). Use the Write tool to
  create files rather than `echo … > file`.
- Pass large or special-character arguments through a **file**, not
  inline on the command line. A multi-paragraph commit message — its
  backticks, braces, and quotes trip the "brace with quote character
  (expansion obfuscation)" guard and force manual approval *every
  time*, even though the command prefix is allow-listed. Write the
  content to a throwaway file with the Write tool (e.g. under `/tmp`)
  and hand the command its path via the matching `--*-file` flag —
  `git commit -F /tmp/<f>.txt` — so only a stable, globbable path rides
  the command line and the call reduces to a `prefix:*` rule. (PR
  titles and bodies are **no longer** a shell concern: they go through
  the GitHub MCP as structured tool arguments — see
  [GitHub via MCP](github-mcp.md) — so there is no `--body-file`
  workaround to manage.)
- Keep a stable command + subcommand prefix (`pnpm lint …`,
  `cargo test …`, `git log …`) and put only the variable parts in the
  arguments, so the call matches a `:*` allow-glob.
- Stay in your worktree. The shell already starts at the worktree
  root — never `cd` into it (`cd <worktree> && …`). That compound
  forces manual approval every time (path-resolution bypass) and
  can't reduce to a glob. Run commands bare from the cwd.
- No status banners or exit-code plumbing. Don't append
  `; echo "=== exit $? ==="`, pipe through `tail` / `grep`, redirect
  `2>&1`, or read `${PIPESTATUS[0]}`. Run the bare command
  (`make lint`, `cargo fmt -p dropset`) — its full output and exit
  status already come back. Pipes and `$(…)` / `${…}` expansion
  force re-approval on every call.
- Capture a *genuinely noisy* command with the quiet runner, not a
  redirect. `python3 .claude/tools/run_quiet.py -- CMD ARGS…` does its
  capture-and-summarize inside Python with `shell=False`, so the
  model's command line stays one bare command with no `>` / `2>&1` — it
  passes the compound-shell guard and reduces to the
  `Bash(python3 .claude/tools/*)` allow-rule. It propagates the child's
  exit code, so callers still see pass/fail. Reach for it only when a
  target has no quiet flag and its success output is pure noise — see
  [context economy](context-economy.md).
- Inspect the base repo by path, not by `cd`. To read another branch
  or the base checkout from a worktree, run
  `git -C <base-repo-path> <subcommand>` with a *literal*, stable path
  (no `$(…)`). Keep the subcommand immediately after the path so the
  call reduces to a `Bash(git -C <base-repo-path> <sub>:*)` rule —
  then pre-approve the read-only subcommands (`log`, `show`, `diff`,
  `status`, `rev-parse`) once in your local `settings.local.json` so
  they never prompt again.
- Operate on a *sibling worktree* by its real path, but approve it
  with a worktree **glob**. A command like
  `git -C <base-repo-path>/.claude/worktrees/<tag> status --short`
  has to name the real worktree to run, but the allow-rule it matches
  against should be the generalized
  `Bash(git -C <base-repo-path>/.claude/worktrees/* status:*)` — the
  mid-path `*` covers every sibling tag and the `:*` covers the args,
  so one rule firms the whole family. Don't approve the per-tag,
  per-arg variant; it only ever matches that one call.
- When per-worktree or per-arg approvals have already piled up in
  `settings.local.json`, run **`/firm-perms sweep`**. It collapses the
  one-off entries into globs (per the rules above), dedupes them, and
  writes the firmed allowlist to **both** this worktree and the base
  repo so future worktrees inherit it — proposing the changes for
  your approval before it writes. That's the full sweep. To memorialize
  a *single* just-approved command instead, a bare `/firm-perms` (or the
  `/f` shorthand) takes the **fast firm** — it firms just that one
  command into both files immediately, with no propose-then-confirm gate.

## Patterns that always re-prompt — never author these

The rules above each rule out a class of command. These are the
specific forms that have actually slipped through and forced a manual
approval *every time*, because none can reduce to an allow-rule —
don't write them, in ad-hoc shell or in committed skills/scripts:

- **Heredocs** (`cat > file << 'EOF' … EOF`, `python3 << 'EOF' … EOF`).
  A heredoc is a redirect plus inline content; when the body contains
  braces it also trips the "brace with quote character (expansion
  obfuscation)" guard, which forces approval regardless of the
  allowlist. To **create a file**, use the Write tool. To **read or
  parse** one (including JSON/IDL), use Read / Grep — never `python3` /
  `node` / `jq`.
- **Ad-hoc compile-and-run scratch** — e.g. a
  `cat > /tmp/x.rs << EOF` heredoc piped into
  `rustc … && /tmp/x`. To check a language or layout question, Write a
  throwaway file and drive it with the normal target (`cargo test`, a
  `#[test]`), or reason it out — don't synthesize a one-off program
  through a heredoc-and-`&&` chain.
- **`cd <path> && <cmd>`** (e.g. `cd <repo> && git -C <worktree> …`).
  The `cd &&` compound re-prompts as a path-resolution bypass. Run
  bare from the cwd, or address another checkout with `git -C <path>`
  alone — no `cd`, no `&&`.

If a one-off like these still gets approved during a session, do
**not** allow-list it (a `*` can't generalize a compound): the
`firm-perms` skill flags it and points back here so the source stops
emitting it.

## The guard hooks

These rules are also enforced **mechanically**, not just by convention,
by two opt-in `PreToolUse` Bash guard hooks that inspect each command
before it runs:

- **`.claude/hooks/no_compound_bash.py`** blocks any unquoted compound /
  redirect operator (the `#compound-ok` marker is the escape hatch).
- **`.claude/hooks/no_git_grep.py`** blocks `git grep` (including
  `git -C <path> grep` and other global-flag variants), nudging to the
  Grep tool. It has **no** escape hatch — Grep, or a bare `grep`, covers
  every legitimate content search.

Each guard **script** is committed, but its wiring is not — like the
iTerm color integration, they are user-local configuration the repo
documents rather than enforces. Both hooks are quote-aware and fail
open. Their behavior and the exact `settings.json` wiring live with the
other local integrations in
[local-integrations](local-integrations.md).

Baseline permission allow-rules (the `Bash(prefix:*)` globs this doc's
rules produce) go in `.claude/settings.json` or `settings.local.json` —
the `firm-perms` skill maintains the local allowlist for you. Because
neither settings file is tracked, a worktree does **not** inherit the
base repo's copy automatically; `firm-perms`' full sweep is what
propagates a firmed allowlist from the base repo into a worktree (and
back), so run it once in a cold worktree if the guard or a familiar
allow-rule is missing.
