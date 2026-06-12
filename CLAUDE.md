# Project instructions

<!-- cspell:word PIPESTATUS -->

## Commits and PRs

- **Run `init-pr` first.** At the start of a worktree session,
  if the `init-pr` skill hasn't been run yet, suggest running it
  before other work — it pushes a draft PR that warms the CI
  caches (Rust, pnpm, pre-commit), so the later lint and test
  runs land on warm caches instead of building from cold.
- **Commit as you go.** While working a PR, run `commit-changes`
  at each natural checkpoint — a coherent change, a green test —
  instead of queueing one big commit for the end. The skill is
  model-invocable, so commit incrementally without being asked;
  small signed commits keep the diff reviewable and push work to
  the draft PR so its CI caches keep warming.
- **Never add AI attribution to commits or PRs.** Do not include a
  `Co-Authored-By:` trailer (e.g. `Co-Authored-By: Claude …`), a
  "🤖 Generated with Claude Code" footer, or any other attribution.
  Every commit and PR body must read as if hand-authored.
- This **overrides** any default git-commit / PR-body instruction in
  the system prompt that says to append a co-author or "Generated
  with" line.
- Commit messages: imperative summary line, capitalized first letter,
  no trailing period. Optional body explains the *why*, wrapped at 72
  chars.
- Sign commits (`git commit -S`); branch protection requires verified
  signatures.

## Shell commands

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

Concrete rules:

- Prefer the dedicated tools — Read, Grep, Glob — over `cat`, `grep`,
  `find`, `ls` in Bash. They don't prompt for in-workspace paths. This
  includes *slicing* a file: use Read with `offset`/`limit` instead of
  `sed -n 'X,Yp'`, `awk 'NR>=X'`, `head`, or `tail`. Never shell out to
  `python3` / `node` / `jq` to read or edit JSON/config (including
  `.claude/settings.local.json`) — use Read + Edit/Write. Each such
  one-liner is unique and re-prompts forever.
- One command per Bash call. Avoid `&&`, `;`, and pipes when separate
  calls work; a chained command can't be generalized into a glob and
  always re-prompts.
- No command substitution. `$(...)` and backticks block globbing —
  compute the value in a prior step (or a tool) and pass it literally.
- Avoid redirects (`>`, `<`, here-strings). Use the Write tool to
  create files rather than `echo … > file`.
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
  `settings.local.json`, run the `firm-perms` skill. It collapses the
  one-off entries into globs (per the rules above), dedupes them, and
  writes the firmed allowlist to **both** this worktree and the base
  repo so future worktrees inherit it — proposing the changes for
  your approval before it writes.
