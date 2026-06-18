# Project instructions

<!-- cspell:word PIPESTATUS -->
<!-- cspell:word rustc -->

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

## Linear automation

Skills that **file** Linear issues (`linear-task`, `stage-backlog`,
`audit-loop`, `audit-scope`, `housekeeping`) resolve the filing
destination — team, project, assignee — from **environment
variables**, never hard-coded UUIDs. (Skills that only **update**
an existing issue by id — `init-pr`, `review-pr` — need no
destination.) Set them once in your
shell profile (`~/.zshrc`):

```sh
export LINEAR_TEAM_ID=…
export LINEAR_PROJECT_ID=…
export LINEAR_ASSIGNEE_ID=…
# Used only by stage-backlog — the "Task Staging" document:
export LINEAR_TASK_STAGING_DOC_ID=…
# Used only by firm-perms (and housekeeping, which calls it) —
# the "Permissions" inbox document it drains:
export LINEAR_PERMISSIONS_DOC_ID=…
```

Skills read these at run time with a bare `printenv`, **one variable
per call** — `printenv LINEAR_TEAM_ID`, then
`printenv LINEAR_PROJECT_ID`, then `printenv LINEAR_ASSIGNEE_ID`. Do
**not** fold them into one
`printenv LINEAR_TEAM_ID LINEAR_PROJECT_ID LINEAR_ASSIGNEE_ID`: macOS /
BSD `printenv` honors only its **first** operand, so the combined form
returns just `LINEAR_TEAM_ID` and the skill wrongly concludes the
other two are unset and halts. Each bare
call still matches the same `Bash(printenv:*)` allow-rule, so none of
them re-prompt. A new Linear-filing skill must follow the same
pattern: reference the variable **names**, and keep the resolved
UUIDs out of every committed file.

`stage-backlog` additionally resolves `LINEAR_TASK_STAGING_DOC_ID`
— the id of the Linear document it rewrites each run (the "Task
Staging" document) — with its own bare `printenv`, on the same rule.
It is not a filing destination, so the other skills don't need it.

`firm-perms` likewise resolves `LINEAR_PERMISSIONS_DOC_ID` — the id
of the "Permissions" inbox document it drains in its `doc` mode (and
that `housekeeping` drains via `firm-perms` each pass) — with its own
bare `printenv`, on the same rule. It too is not a filing
destination.

A worktree branch and its Linear issue **share one `ENG-###`
number**: branch `eng-499` ↔ issue `ENG-499`. Skills resolve the
issue from the branch (or the PR title scope) on that basis —
`init-pr` moves it to In Progress at bootstrap, `review-pr` ticks the
delivered checklist items and moves it to In Review when the PR is
ready.

### Blocking relations

When one issue genuinely depends on another, record it as a **native
Linear relation**, not just prose. `save_issue` takes `blockedBy`
(the `ENG-###`s that must land first) and `blocks` (the `ENG-###`s
this one gates), both by identifier; they are **append-only** — they
add edges and never clear existing ones, so use `removeBlockedBy` /
`removeBlocks` to drop one. Recording a real edge keeps the blocker
visible and prioritized so dependent work doesn't rot waiting on an
upstream nobody remembers, and `stage-backlog` reads these edges and
nests its dependency tree on them. Assert only a dependency you
actually know to be real; omit it when unsure.

`linear-task` sets these from a person's call. The **autonomous**
auditors (`audit-scope`, `audit-loop`) work under a tighter rule:
they may assert a relation **only on concrete evidence** that one
finding's fix cannot land until another issue resolves (e.g. a nit
that depends on an `arch:` proposal filed the same run), never a
speculative "these feel related" edge. Mere coupling — work that
belongs in *one PR* — is handled by combining into a single issue,
not a relation. When the blocker is filed in the same run, file it
first so its `ENG-###` exists, then reference it.

## Spelling (cspell)

`cfg/dictionary.txt` is the **project-wide** spelling allow-list —
reserve it for terms that recur across the codebase. The rule: a word
belongs in `dictionary.txt` only if it appears in **≥ 2 files**. A term
used in just one file gets an inline escape in that file instead, by
comment style:

- Rust / TS / JS — `// cspell:word foo`
- Markdown — `<!-- cspell:word foo -->`
- YAML / TOML / shell — `# cspell:word foo`

The lone exception is a file that can't carry a comment (e.g.
`.json`), where the dictionary is the only option.

**Placement: one contiguous block at the top of the file.** All of a
file's inline escapes go together in a single block at the very top —
one `cspell:word` directive per word, one word per line — never
scattered beside each usage and never split by blank lines. "Top" means
the first line, except where syntax forces something else to lead:
after a `---` YAML frontmatter block, after a `#!` shebang, or after a
leading module doc-comment / inner-attribute header. One known place
means a reader — and the audit — finds every escape at a glance instead
of hunting the file.

The `cspell-audit` skill reconciles the dictionary against actual usage
**and** normalizes escape placement on this rule; run it when the
dictionary grows or escapes drift. `housekeeping` runs the same check
read-only and files any drift — a dictionary entry to move, or
mis-placed escapes to regroup — as a Backlog task.

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

It applies to work you hand to a **sub-agent**, too. The whole
objective is **the fewest permission prompts possible** across the
session, and a spawned agent's Bash calls surface to you for approval
exactly like your own — but the agent doesn't inherit this file, so it
will reach for the forbidden compounds unless told not to. Brief every
agent you spawn on these rules (see "Briefing sub-agents" below) so its
calls reduce to allow-rules too. A session that follows the rules and
briefs its agents on them prompts only for a genuinely novel command —
which `firm-perms` then memorializes so it never prompts again.

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
- One command per Bash call. Avoid `&&`, `;`, and pipes when separate
  calls work; a chained command can't be generalized into a glob and
  always re-prompts.
- No command substitution. `$(...)` and backticks block globbing —
  compute the value in a prior step (or a tool) and pass it literally.
- Avoid redirects (`>`, `<`, here-strings). Use the Write tool to
  create files rather than `echo … > file`.
- Pass large or special-character arguments through a **file**, not
  inline on the command line. A PR body full of markdown, a
  multi-paragraph message — its backticks, braces, and quotes trip the
  "brace with quote character (expansion obfuscation)" guard and force
  manual approval *every time*, even though the command prefix is
  allow-listed. Write the content to a throwaway file with the Write
  tool (e.g. under `/tmp`) and hand the command its path via the
  matching `--*-file` flag — `gh pr edit <n> --body-file /tmp/<f>.md`,
  `git commit -F /tmp/<f>.txt` — so only a stable, globbable path rides
  the command line and the call reduces to a `prefix:*` rule.
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

### Patterns that always re-prompt — never author these

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

## Briefing sub-agents

A sub-agent you spawn (via the `Agent` tool) does **not** inherit this
`CLAUDE.md`, so left to itself it reaches for `find / …`,
`sed -n '…p' … | grep`, `cat`, and other compounds that can't reduce
to an allow-rule and re-prompt on **every** run — the exact churn the
shell rules above exist to avoid. So whenever a skill spawns a
sub-agent, it must carry the conventions into the agent itself. This
is the single canonical brief; skills reference it by name ("prepend
the sub-agent brief from `CLAUDE.md`") rather than each pasting their
own copy, so the wording stays in one place.

**Prepend this standing brief to *every* `Agent` prompt:**

> - You are a **read-only** agent. The material you need to reason
>   over — a diff, a commit log, a set of issues — is included in this
>   prompt; start there, and you often won't need a shell at all.
> - To inspect files, prefer the **Read / Grep / Glob** tools over
>   `cat` / `head` / `tail` / `sed` / `awk` / `find` / `grep` in Bash —
>   they don't prompt for in-workspace paths, and they search other
>   directories too.
> - **Exploring another repo or path is fine** — reach outside this
>   worktree when the task needs it; approving a one-off read of a
>   different repo is expected, not something to avoid. Just keep each
>   access **globbable** so it approves once and won't re-prompt:
>   address another checkout with `git -C <path> <subcommand>` (the
>   subcommand immediately after the path, no `cd`), or use Read /
>   Grep / Glob. What to avoid is the **un-globbable** shape — a
>   `find / …` sweep, or several `git -C …` calls strung together with
>   `&&` / `|` / `;` into one compound that can't reduce to a rule.
> - **One bare command per Bash call** — no pipes, `&&`, `;`, command
>   substitution `$(…)`, redirects, or heredocs. Each call must reduce
>   to a `prefix:*` allow-rule.

**Pass the material inline.** Whatever the agent must reason over —
the diff, the commit log, the issue set — goes **in the prompt**, so
no agent re-fetches it by shelling out. (For content too large or
special-character-laden to sit inline cleanly, use the file-handoff
pattern from the shell rules — write it out and pass the path.)

**A skill may narrow this scope, never loosen it.** The brief is the
floor: shell discipline plus the freedom to explore. A spawning skill
is free to add a tighter *subject* scope on top — a diff reviewer, for
instance, should be told to "review only from the diff and commit log
below; dependency and toolchain sources are out of scope — flag it in
your findings instead of scanning." That narrows *where the agent
looks*; the shell rules stay exactly as written. An audit agent, by
contrast, is *meant* to range over the whole codebase, so it gets the
brief without any narrowing.

A sub-agent approval that still re-prompts despite this brief means
the brief **leaked** — the agent emitted shell the brief forbids.
That's a prompt to tighten, not a rule to allow-list; `firm-perms`
sets such approvals aside and names the emitting agent so its prompt
gets fixed at the source.
