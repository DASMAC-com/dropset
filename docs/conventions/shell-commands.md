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

- Searching file *contents* — prefer the **Grep tool**
  **unconditionally**; where it's absent (the Grep / Glob caveat above) a
  **bare, single** `grep` is the fallback, but **never** `git grep`.
  "Unconditionally" is load-bearing, and it is a *main-loop* rule, not
  only a sub-agent one: the Grep tool honors gitignore, which makes the
  escape-into-build-output failure below **structurally impossible**
  rather than something an agent has to remember at each call site. The
  measured record is that remembering does not work — three separate
  sessions spelled an unscoped recursive shell `grep` from the main loop
  and got back 79.2KB (a repo-wide conflict-marker sweep that walked
  `frontend/.next/`), 3.6MB (`decks/.next/`), and 8.6MB. Two of those
  landed as harness-persisted blobs, so the context hit was small — but
  only by luck of the transport, not by anything the call did right.
  This is the same rule the
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

  When you do fall back to a bare `grep`, two things the Grep tool was
  handling for you:

  - **Scope a recursive `grep` to source directories — never a package
    root.** The Grep tool honors gitignore; a bare `grep -r` does not,
    so aimed at a package root it walks `.next/`, `target/`,
    `node_modules/`, and `dist/` too. One straggler-reference check ran
    a bare recursive `grep` over the `decks/` root, matched a minified
    webpack chunk under the gitignored `decks/.next/`, and returned a
    ≈5.1k syntax-highlighter grammar blob — **48% of all Bash spend in
    that session, from one call** — answering a question whose correct
    answer was *zero hits*, as the scoped re-run confirmed. Name the
    source directories (or the specific files), or exclude the build
    output explicitly with `--exclude-dir`.

    **The agent-config tree is the trap that does not look like one.**
    `.claude/` reads as a source directory and is not: the worktrees
    live beneath it, so a bare recursive sweep of it walks **every**
    worktree checkout, Rust `target/` trees included. One housekeeping
    pass did exactly that and **timed out at 120 seconds** — a wholly
    wasted turn, costing wall-clock as well as context. Scope to the
    subdirectory you mean (`.claude/skills`, `.claude/tools`,
    `.claude/hooks`); never sweep the parent bare. It bites hardest in
    the skill family that runs from the base repo — `housekeeping` and
    `plan` — which is precisely where the worktrees are.

    **Better: don't hand-roll the exclude list.** There is a committed
    tool for this shape, which prunes the generated families and the
    never-search trees and reduces to one stable allow-rule however the
    pattern and filters vary:

    ```sh
    python3 .claude/tools/search_source.py 'PATTERN' --context 2
    ```

    **Prose needs `--ext md`.** The default extension set is source
    only, so a sweep for a string that lives in a skill or convention
    doc returns `0 match(es)` — which reads as absence but means "never
    looked". One run took that zero at face value and fell back to a
    bare `grep`, the very thing the tool replaces. The summary line now
    names the omission; pass `--ext md` (or `--all-text`) instead.

    **A `--context N` window is for adjudicating a hit, never for
    enumerating them.** It multiplies the payload by `N`: one
    `--context 8` sweep returned 65.8KB, overflowed the tool-result cap,
    spilled to disk, and answered nothing, while the re-ask without
    context returned 11 lines and settled the question. Anchor the
    pattern on the thing being enumerated and take no context. For the
    same reason, ask **one question per sweep** — alternating several
    unrelated patterns into one regex multiplies the result by the
    number of things asked at once.

    **`--context` scales with match *density*, and narrowing the scope
    is a separate axis from narrowing the output.** Both are stated in
    full in [context economy](context-economy.md) → "The levers"; the
    short forms are that clustered matches make context windows overlap
    toward buying the file (so take `--files-only` then slice-read), and
    that `--dir` / `--glob` bound *how much tree is searched*, which no
    amount of output narrowing does.

    **When the fallback bare `grep` is what you have, shrink its
    context flag to fit the question.** The prescribed form says nothing
    about width, so it gets used at whatever came to hand: one session
    without the Grep tool read a captured lint log with `grep -A4` and
    paid ~1.6k, because four trailing lines per match across many
    matches is far more than "which hook failed" needs. Pick the width
    from the question — usually none.

    When a bare `grep` really is unavoidable, take the flags from
    `python3 .claude/tools/review_diff.py --print-grep-excludes` rather
    than re-deriving them — that is the same list, with one owner.

    Don't confuse that list with the **skip-globs** in
    [the audit registry](audit-registry.md). They overlap, but they do
    different jobs: the registry's globs decide which files an audit
    rotation may *pick*, while `review_diff.py` owns which trees a
    *search* must skip. Reuse the tool's list for searching; leave the
    registry's to the audit.

  - **`grep -o -m N` bounds matched *lines*, not matches.** Two field
    extractions over a fetched page passed `-m 6` expecting six values
    and got ~150 lines back, because each matched line carried many
    `-o` matches. To pull a couple of fields out of a fetched page,
    tighten the **pattern** so it can only match what you want (or save
    the page and Read it by slice) — `-m` won't bound the output.

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

  **That message file is session-scoped — re-verify it before retrying
  a blocked commit.** The scratchpad does not survive a session
  restart, and a `-F` message staged behind a commit that *failed* is
  exactly what gets lost. One session restarted twice (a usage
  interruption, then a resume); both times the casualty was a commit
  message file sitting behind a blocked signature, and both times it
  had to be re-authored verbatim before the commit could be retried. So
  on a **failed** commit — a signing error, a hook rejection — check
  the file still exists before re-running, and re-write it if not.
  A commit that *succeeded* has the message in git and needs nothing.

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

- When per-arg approvals have already piled up in
  `settings.local.json`, run **`/firm-perms sweep`**. It collapses the
  one-off entries into globs (per the rules above), dedupes them, and
  writes the firmed allowlist back — proposing the changes for
  your approval before it writes. That's the full sweep. To memorialize
  a *single* just-approved command instead, a bare `/firm-perms` takes
  the **fast firm** — it firms just that one command immediately, with
  no propose-then-confirm gate. Both write to
  the one shared file at the main checkout, so the result is live in
  every worktree.

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
- **A search pattern containing `<` or `>`.** Angle brackets read as
  redirects, so a pattern carrying them is refused as "too complex to
  verify that it stays inside the worktree" even inside quotes. One
  sweep for the audit registry's `<->` interface notation was rejected
  on exactly this and cost a retry with a reworded pattern. Search with
  `search_source.py` (it takes the pattern as an argument, not through
  the shell), or reword to avoid the characters — `--fixed` does not
  help, since the refusal happens before the tool ever runs.
- **A multi-URL `curl` status probe carrying one `-o /dev/null`.** The
  flag binds to **one** URL, so an eight-URL probe with a single
  `-o /dev/null` writes the first response to `/dev/null` and dumps the
  other **seven bodies** — 748KB in the measured case, limited only by
  the harness's overflow guard rather than by anything the command did.
  When probing several endpoints for status alone, repeat the flag per
  URL (`-o /dev/null` once for each), or use a head-only request (`-I`)
  so there is no body to dump in the first place.

**A refused or blocked call leaves its question UNANSWERED.** That is
the shared failure mode behind every entry above, and it is silent: the
call returns no result, and no-result reads exactly like "nothing
found". Retry it in a simpler form, or record the question explicitly
as unverified — never let a refusal stand as a negative finding.

Measured: two `search_source.py` calls were refused in one session. The
second was retried in a simpler form and answered; **the first never
was**. It had been looking for a summing time-budget clause, which
turned out to be that review's most consequential finding — a page's
time budget contradicting the ledger by four seconds. A sub-agent found
it instead, and reported it as "the one item I could not settle" after
hitting its tool-call cap. One notch down the fan-out tiers and a stale
figure ships.

Read the refusal text with suspicion, because it can misdiagnose its
own cause. A pattern rejected as "too complex to verify that it stays
inside the worktree" may name no path and touch no git at all — the
real trigger being pattern complexity, such as an alternation, a `+`
quantifier, or an unusual character. That message comes from the
**harness**, not from anything this repo commits, so it cannot be
reworded here; treat it as "rephrase and retry", never as "this
question is structurally unanswerable".

If a one-off like these still gets approved during a session, do
**not** allow-list it (a `*` can't generalize a compound): the
`firm-perms` skill flags it and points back here so the source stops
emitting it.

## The guard hooks

These rules are also enforced **mechanically**, not just by convention,
by opt-in `PreToolUse` Bash guard hooks that inspect each command
before it runs:

- **`.claude/hooks/no_compound_bash.py`** blocks any unquoted compound /
  redirect operator (the `#compound-ok` marker is the escape hatch).
- **`.claude/hooks/no_git_grep.py`** blocks `git grep` (including
  `git -C <path> grep` and other global-flag variants), nudging to the
  Grep tool. It has **no** escape hatch, and that is deliberate — see
  [why the ban stays absolute](#why-the-git-grep-ban-stays-absolute)
  below for what the one legitimate use is and why it doesn't earn a
  carve-out.
- **`.claude/hooks/no_destructive_bash.py`** covers command **danger**,
  which the other two do not: they check shell *form*. Two tiers — an
  **ask** tier (recursive force-delete, force-push, hard reset,
  `git clean -fdx`, destructive SQL, a docker prune) blocked with the
  `#destructive-ok` marker as its escape hatch, and a small **deny**
  tier that no marker lifts (a recursive delete of `/` or the home
  directory, a force-push to the default branch). It is a **best-effort
  advisory stop, not a policy boundary** — it matches patterns over one
  command string and is not a sandbox.

Each guard **script** is committed, but its wiring is not — like the
iTerm color integration, they are user-local configuration the repo
documents rather than enforces. Every hook **fails open** — a parse
problem allows rather than wedging the session.

**On "quote-aware", precisely.** The compound guard's operator scan is
quote-aware character by character, which is what lets it ignore a `;`
inside a quoted commit message. The destructive guard's *comment
handling* is quote-aware too, so a quoted `#destructive-ok` cannot
disable it — but its **pattern matching is plain regex over the
command**, and that is why its SQL rules require a SQL client to be
named rather than matching the words anywhere. Do not read
"quote-aware" as a property of the destructive matching itself.

Their behavior and the exact `settings.json` wiring live with the
other local integrations in
[local-integrations](local-integrations.md).

Baseline permission allow-rules (the `Bash(prefix:*)` globs this doc's
rules produce) go in `.claude/settings.json` or `settings.local.json` —
the `firm-perms` skill maintains the local allowlist for you.
`settings.local.json` is **one shared file resolved through worktrees
to the main checkout**, so a firmed rule is live in every worktree at
once and there is nothing to propagate; see
[local-integrations](local-integrations.md) → "How settings files
resolve across worktrees".

## Why the `git grep` ban stays absolute

Adjudicated 2026-08-11. The question put was whether the ban should gain
a narrow carve-out for **revision-scoped** search, spelled
`git grep <pattern> <rev>`. That is the one capability the Grep tool
structurally lacks, since it only ever sees the working tree.

**Verdict: the ban stays absolute, and the carve-out is rejected.** But
the old justification for it ("there is no legitimate `git grep` worth
letting through") was **false as written**, and this section replaces it.
Rev-scoped content search is a real capability with no direct substitute.
The honest position is that its one legitimate use is rare here, has
adequate indirect workarounds, and does not justify weakening a guard
whose entire value is that it is unconditional.

The carve-out is rejected on **mechanism, not taste**:

- **A guard cannot reliably tell a rev from a pathspec.** The forms
  `git grep foo main`, `git grep foo -- main`, and range syntax are
  genuinely ambiguous
  to a pre-execution check. A carve-out that errs one way leaks
  working-tree searches; erring the other way blocks the legitimate form.
  A sometimes-wrong guard is worse than either pole.
- **It would re-bless the unfirmable shape.** The alternation form
  (`git grep "a\|b" <rev>`) trips the harness's per-subcommand `|` guard
  and therefore can never be firmed — which is the load-bearing reason
  this guard exists at all.

**One epistemic caveat, stated deliberately:** that unfirmable-alternation
behavior is *inherited-empirical* — it was observed when the rule was
first written and cannot be re-verified now without tripping the guard.
If the harness's firming logic ever changes, this rationale must be
**re-derived, not inherited**. The other two reasons — that `git grep`
looks blessed when it isn't, and the re-prompt friction — favor the Grep
tool regardless.

**The gap is real, and the tool that fills it is committed.** Use
`show_at_ref.py` — it reads the blob at a ref inside its own process and
prints only the slice you asked for:

```sh
python3 .claude/tools/show_at_ref.py origin/main src/lib.rs --grep '^pub fn'
python3 .claude/tools/show_at_ref.py origin/main docs/x.md --section 'Levers'
python3 .claude/tools/show_at_ref.py HEAD~5 src/lib.rs --slice 40:80
```

It reuses `read_result.py`'s renderers, so `--headings` / `--section` /
`--grep` / `--slice` / `--count` behave exactly as they do on a persisted
tool result, and it rides the existing `python3 .claude/tools/:*` grant.
A mode is **required** — there is deliberately no print-it-all default,
because that would rebuild the whole-file `git show` it replaces.

**The earlier claim that this gap had "adequate indirect workarounds"
was wrong, and is retracted.** Measured against the actual constraints,
every conforming path failed: the Grep tool reads only the working tree;
`git grep` is guard-blocked; piping `git show` into `grep` is a forbidden
compound; and a throwaway `git worktree add --detach` costs more than the
read it avoids. What was left was a **whole-blob `git show`** — measured
at ~4.7k to learn one `Duration` default, and the largest single result
of that run. The pre-registered trigger below therefore fired, and the
tool was built.

Two narrower questions still have better answers than a content search:

- `git log -S` / `-G` answers *when and where* a string appeared.
- `git show <rev>:<path>` answers *what a known file said* — but prints
  the **whole blob**, so prefer `show_at_ref.py` unless you genuinely
  want all of it. (`--no-patch` suppresses a *diff*, not a blob dump.)

**The trigger that fired, kept for the record.** The rule was: build a
committed `.claude/tools/` wrapper behind one firmable `python3 …:*`
prefix if rev-scoped reading is hit **twice** in the session-metrics
record. It was — and the shape is now routine rather than rare, because
the review flow's freshness gates make cross-reference reads ordinary in
any review that outlives a merge. **Never add a hook carve-out**, though;
that part of the verdict is unchanged, and the tool is what made it
unnecessary.
