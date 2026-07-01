---
name: review-pr
description: Adversarial pre-review — mark the Linear issue In Progress on invocation, verify its checklist is fully addressed, lint, catalogue issues, fix what's mechanical, ready the PR, wait for GitHub CI to pass, print the review summary, then at the merge-queue handoff re-check the PR still merges cleanly before moving the issue In Review and offering to enqueue the PR, firm up permissions and capture session metrics while it sits in the queue, and report whether it merges or gets taken back out.
user-invocable: true
---

<!-- cspell:word oneline -->

<!-- cspell:word unstarted -->

# `review-pr`

Act as an adversarial reviewer before the human
looks at the PR. Run lint, audit the diff,
catalogue every issue, fix what can be fixed
mechanically, and mark the PR ready only when
it's clean. Then wait for the real GitHub CI to go
green and print the review summary. Invoking this skill
moves the Linear issue to **In Progress** at the start
(reclaiming it even from In Review if a prior run
advanced it), and it stays In Progress through all of
this. It moves to In Review
only at the merge-queue handoff, the point at which
it's the human's turn to look at the ready, CI-green PR
and approve enqueueing it — so when this skill prompts,
the human can merge (or let "Merge when ready" land it)
with nothing left to check.

Run this after autonomous work is complete and
all changes are committed and pushed.

GitHub reads and writes go through the **GitHub MCP**, with
the deliberate `gh` exceptions in `CLAUDE.md` → "GitHub via
MCP": the merge-queue **enqueue** (a `gh pr merge --auto`
write) and **dequeue probe** (a `gh api graphql` read) at
the handoff, because the MCP exposes no merge-queue tool and
its `pull_request_read` omits `mergeQueueEntry`; plus the
**polled / one-shot reads** this skill makes with the
compact `gh pr checks` (the CI-wait poll) and field-selected
`gh pr view --json` (the PR lookup in step 1 and the
merge-clean check) — chosen because those reads repeat, and
a full-object MCP payload would be replayed into context on
every later turn (`CLAUDE.md` → "Context economy"). The
PR-authoring **writes** (`create_pull_request`,
`update_pull_request`) stay on the MCP. This repo is
`DASMAC-com/dropset`, so every MCP call takes
`owner: "DASMAC-com"`, `repo: "dropset"`.

## Steps

1. **Locate the PR.** Identify the current branch
   (`git branch --show-current`), then look it up with a
   **field-selected** `gh pr view` — passing only the fields
   the later steps need, so the lookup doesn't drag the full
   PR object into context (per `CLAUDE.md` → "Context
   economy" / "GitHub via MCP"):

   ```sh
   gh pr view <branch> --json number,title,state,isDraft
   ```

   That returns just `number`, `title`, `state`, and
   `isDraft`. If the branch has no PR, `gh` exits non-zero
   ("no pull requests found") — treat that as "no PR" and
   stop, telling the user to run `/init-pr` first. (This is a
   read; the routine PR-authoring **writes** in later steps
   stay on the MCP.)

1. **Clean tree, then rebase onto `main`.** First run
   `git status` — if there are uncommitted changes,
   stop and tell the user to commit first (or run
   `/commit-changes`). Then rebase onto the latest
   `main` so the review runs on the state the branch
   will actually merge as, instead of a base that has
   drifted while the work was in flight — this is what
   minimizes file conflicts at merge time:

   ```sh
   git fetch origin main
   git rebase origin/main
   ```

   - If the rebase **conflicts**, abort it
     (`git rebase --abort`), catalogue the conflict as
     a **blocking** issue (step 7), and tell the user
     to rebase and resolve manually, then re-run — this
     skill does not auto-resolve conflicts.
   - If it **succeeds but integrated new commits from
     `main`**, the diff now reflects that integration.
     A clean *textual* rebase can still leave a
     *semantic* conflict (main renamed or changed
     something this branch still calls), so flag those
     for the adversarial review (step 5) and the test
     run (step 10) to catch. The rebase rewrote history,
     so the branch must be force-pushed — step 11 does
     this with `--force-with-lease`.

1. **Check the Linear task, mark it In Progress, and
   tick what's done.** The PR exists to satisfy a
   Linear issue, and autonomous runs have a habit of
   shipping a diff that covers only *some* of the
   task's checklist. Establish what the task asked
   for, record progress on the issue, and confirm the
   diff delivers all of it before reviewing anything
   else.

   - Resolve the tag. The branch and its Linear issue
     **share one `ENG-###` number** by convention
     (branch `eng-499` ↔ issue `ENG-499`; see
     `CLAUDE.md`), so take the `ENG-###` from the PR
     title scope (`type(ENG-###): …`), falling back to
     the branch name. If neither yields an `ENG-###`,
     skip this step and note in the report that no
     Linear task was checked.

   - Fetch the issue with `mcp__claude_ai_Linear__get_issue`
     (id = the uppercase tag, e.g. `"ENG-490"`). Read
     the description, and also pull
     `mcp__claude_ai_Linear__list_comments` — checklist
     items and acceptance criteria sometimes live in an
     inline (anchored) comment, not the body.

   - Plan to move the issue to **In Progress** to reflect
     that review work is underway — invoking `review-pr`
     always moves it there, **including reclaiming it from
     In Review** if a prior `review-pr` run advanced it. In
     Review now belongs to the merge-queue handoff (the
     final steps), so a re-run should pull the issue back
     to In Progress rather than leave it sitting In Review
     while the review is actively redone. The one thing
     not to regress is a **Done** / **Canceled** issue —
     leave those as-is. **Do not issue this state change
     as its own `save_issue`**: fold it into the single
     box-tick write below (see "Minimize Linear echoes"),
     so the In-Progress move and the ticked checklist land
     in **one** call rather than two full-body echoes. (In
     Review can't be folded the same way — it's gated on
     CI-green at the merge-queue handoff, a different point
     in the flow, so it stays its own write at step 18.)

   - Extract every actionable requirement: markdown
     checkboxes (`- [ ]` open, `- [x]` already done),
     plus any acceptance-criteria or scope bullets
     phrased as requirements even if not checkbox
     syntax. Treat an already-`[x]`-checked box as a
     claim to verify, not a given — confirm the diff
     actually contains it.

   - For each requirement, decide from the diff
     (`git diff main..HEAD`) and the branch's commits
     whether it is **addressed**, **partial**, or
     **missing**. A requirement that is out of scope
     for this PR by design (e.g. explicitly deferred,
     or split into a follow-up issue) counts as
     addressed *only if* the deferral is recorded —
     a commit, a PR-body note, or a linked follow-up
     filed via `/linear-task`. Silent omission is
     **missing**.

   - **Tick the addressed items in the same write that
     moves the issue to In Progress.** For every
     requirement the diff genuinely delivers, check its
     box (`- [ ]` → `- [x]`), then write the updated
     `description` **and** `state: "In Progress"` back in a
     **single** `save_issue` (id + state + the full edited
     `description`). Leave **partial** and **missing**
     boxes unchecked, and don't invent boxes for
     non-checkbox requirements. Diff against the live body
     you just fetched so you never clobber a box the author
     already ticked or other edits made since.

     ```txt
     mcp__claude_ai_Linear__save_issue(
       id: "<ENG-###>",
       state: "In Progress",
       description: "<the full edited body, boxes ticked>"
     )
     ```

     If there are **no** boxes to tick (no checklist, or
     none newly delivered), **and** the `get_issue` above
     already shows the issue **In Progress** (which `init-pr`
     set at bootstrap), there is **nothing to write** —
     skip the `save_issue` entirely. Only when the state
     actually needs to change (it's not yet In Progress, or
     it's being reclaimed from In Review) does a
     description-less `state: "In Progress"` write fire.
     Either way it is **at most one** write.

     **Minimize Linear echoes** (per `CLAUDE.md` →
     "Context economy"): each `save_issue` / `get_issue`
     **echoes the full issue body** back into context, and
     that echo is then replayed every later turn — worst on
     a large consolidated-spec body. So fetch
     the issue **once** (the `get_issue` above), don't
     re-`get_issue` it, and collapse the In-Progress move
     and **all** the box-ticks into the **one** `save_issue`
     above — never a separate state write, and never one
     write per box. On a **re-run / rework**, don't re-flip
     the state unless it genuinely changed (the fetched state
     tells you), and if a state-change echo comes back not
     reflecting the change, **verify once and report the
     discrepancy** — do not retry, since each retry re-echoes
     the whole body.

   - Catalogue every **partial** or **missing**
     requirement as a **blocking** issue (step 7),
     quoting the checklist text and the `file:line`
     (or absence) that decides it.

1. **Run lint.** `make lint` runs the full pre-commit hook
   set, and on a failure its cspell hook alone dumps a
   ~450-line per-file cascade — pure noise that, once it's
   in context, is replayed every later turn. So run it
   **through the quiet runner** (per `CLAUDE.md` → "Context
   economy") — it captures the hook output to a temp log and
   prints only a one-line summary on success, or the failing
   tail + log path on failure:

   ```sh
   python3 .claude/tools/run_quiet.py -- make lint
   ```

   **When you go into the captured log, Grep it for the
   failure — never `Read` it whole.** A whole-file read of a
   captured lint log is how a 500-line per-file cspell dump
   became the single largest result of a run (PR #207). Grep
   the log for the failure markers (`Failed`, `error[`,
   `Error`) to find the offending hook, or read only its
   tail; slice from there.

   If lint fails, first separate **environmental**
   failures from **real violations** — they're not the
   same problem:

   - A hook that fails because its binary isn't
     installed is **not** a diff problem. The
     frontend hooks — `biome`, `tsc` — report
     "Command … not found" whenever this worktree has
     no frontend `node_modules` (each worktree is a
     fresh checkout, so deps aren't installed until you
     ask). Install them once and re-run, so the hooks
     actually evaluate the diff:

     ```sh
     pnpm --dir frontend install
     ```

     If the deps still can't be installed, treat those
     hooks as **unverifiable locally** — exactly like an
     absent Solana toolchain (steps 9–10), not as a
     blocking failure. When the diff touches none of the
     files such a stalled hook covers (e.g. a docs-only
     change vs. `biome` / `tsc`, which only target
     JS / TS / CSS), note in the report that they'll
     pass in CI and move on. **Never gate the PR on a
     hook that couldn't run.**

   - For genuine violations, parse the failing tail (and
     the log by slice) and fix
     every issue that can be fixed mechanically
     (formatting, import order, trailing whitespace,
     spelling, etc.).

   - Stage the fixes by explicit path and commit as a
     single signed commit:

     ```sh
     git add <fixed files...>
     git commit -S -m "Fix lint violations"
     ```

   - **Re-run only the failing hook, scoped to the diff's
     changed files** — not the whole `make lint` /
     `--all-files` cascade. The full run re-checks every file
     in the repo (the cspell hook's ~450-line cascade is the
     worst of it); the fix only touched the diff's files, so
     confirm it against just those, with the failing hook id
     from the run above:

     ```sh
     pre-commit run <hook-id> --files <changed files...>
     ```

     Take `<changed files...>` from
     `git diff --name-only main..HEAD`. Only fall back to a
     full `python3 .claude/tools/run_quiet.py -- make lint`
     when a hook is repo-global (it has no per-file scope) or
     when you've changed enough that a scoped re-run wouldn't
     be representative.

   - If real violations still fail after the fix
     attempt, catalogue the remaining failures as
     **blocking** issues (step 7) and do **not** mark
     the PR ready.

1. **Adversarial diff review.** Collect the diff and log —
   but write the **diff to a single file** rather than into
   context, so the fan-out below hands each agent a path
   instead of inlining N resident copies (per `CLAUDE.md` →
   "Context economy"; the file-handoff pattern). `git diff`'s
   `--output=<file>` flag writes straight to the file with no
   shell redirect (so it stays a `Bash(git diff:*)`
   allow-rule) **and** keeps the bulky diff out of the main
   transcript entirely.

   **Write it to the session scratchpad, not `/tmp`.** The
   environment designates a per-session scratchpad directory
   (the harness prints its path at session start) that **is
   shared with the sub-agents** you spawn. `/tmp` is **not**
   safe here: it's shared across sessions, so a sibling
   session's stale `review-diff.txt` can sit at the same path
   and the fan-out then reviews the **wrong diff** — a real
   bug that has cost an entire 6-agent pass. Write to
   `<scratchpad>/review-diff.txt` (substitute the actual
   scratchpad path), then **verify the file is this branch's
   diff before fanning out** — a one-line `wc -l` (and, if in
   any doubt, a `head`) so a zero-length or stale file is
   caught now, not after N agents have read it:

   ```sh
   git diff main..HEAD --output=<scratchpad>/review-diff.txt
   git log main..HEAD --oneline
   wc -l <scratchpad>/review-diff.txt
   ```

   The commit log is small, so it prints to context and is
   passed inline; the diff lives only in
   `<scratchpad>/review-diff.txt`. Do not fan out until the
   `wc -l` confirms the file holds this branch's diff.

   **Brief every sub-agent on the shell rules.** Prepend
   the standing sub-agent brief from
   `docs/conventions/sub-agent-brief.md` to **each** Agent
   prompt — the review agents here *and* the cross-check
   agent in step 6. That brief is the canonical wording
   (read-only framing, Read/Grep/Glob over shell, one bare
   command per Bash call, each reducible to an allow-rule);
   it exists so sub-agents — which inherit neither that
   brief nor `CLAUDE.md` — don't reach for the
   `find` / `sed … | grep` / `cat`
   compounds that re-prompt on every run.

   **Then narrow the scope for these reviewers.** The
   brief deliberately lets an agent explore other repos
   and paths, but a *diff review* doesn't need that —
   tell each reviewer to work **only from the diff and
   commit log provided below**. Dependency and toolchain
   sources (`~/.cargo`, `node_modules`, another repo) are
   out of scope here; if a reviewer thinks it needs a
   library's source, it should say so in its findings
   rather than scanning for it. This narrows *where the
   agent looks* on top of the brief — it does not relax
   the shell rules.

   **State the negative scope explicitly.** Give every lens
   prompt (and the step-6 cross-check) a one-line bound so
   an on-topic agent can't wander into a settings /
   permissions / git audit: **"review the code diff only;
   do not audit permissions, settings, or git history."**
   Review lenses have drifted into a `firm-perms`-style
   permission-allowlist audit or run the full test suite
   instead of reviewing the diff, forcing an expensive
   redo; the negative-scope line is what kept the redo on
   task. The only exception is the two **freshness** lenses
   below, which are *meant* to read named convention files —
   give them their positive scope (the named files) instead.

   The two **freshness** lenses below are the deliberate
   exception: they *do* read a handful of named
   in-workspace files (`CLAUDE.md`, the skill files under
   `.claude/skills/`, `.github/workflows/test.yml`).
   Tell those two reviewers to open and search those files
   **only through the Read / Grep tools** — never shell
   `grep` or `git grep` (including `git -C <path> grep`),
   which re-prompt and, with a quoted `\|` alternation,
   can't even be firmed. And tell them to **slice-read**:
   `CLAUDE.md` and the larger SKILL.md files are big, so
   Grep to the relevant section and `Read` it with
   `offset`/`limit` rather than pulling the whole file to
   check one rule (per `CLAUDE.md` → "Context economy") —
   a whole-file Read of each is a top token sink otherwise.

   **Hand each agent the diff by path, not inline.** Tell
   every reviewer (and the step-6 cross-check agent) to
   **Read `<scratchpad>/review-diff.txt`** for the full diff, and
   pass the small commit log inline. This holds **one**
   resident copy of the diff (read into each agent's own
   context) instead of N copies inlined across the prompts;
   no agent re-fetches the diff by shelling out.

   **Tell each reviewer to read every file it needs once,
   up front, and reason from that copy.** A lens that
   re-`Read`s or re-greps the same file on each turn pays
   for it every turn (review lenses have run 197k–469k input
   each doing this). Brief each agent to open the handful of
   files its lens touches a single time at the start —
   slice-reading the large ones (Grep to the section, then
   `Read` with `offset`/`limit`) — and then work from what
   it has read, not re-fetch. Combined with the diff-by-path
   handoff above, an agent should rarely need to shell out
   again.

   **Scale the fan-out to the diff.** The full lens set
   below plus the step-6 cross-check is the right spend for
   a substantial diff with real new logic (a new
   instruction, a non-trivial refactor, new on-chain or SDK
   surface — e.g. PRs #178, #184). It is near-pure fixed
   cost on a **trivial** diff, where each lens and the
   cross-check re-read the same few lines for nothing
   (a 4-line reword spawned a 70.4k-input agent; a 3-file
   doc-only diff a 375.4k one; a 24-line infra diff four
   agents including a 277.8k cross-check for a single nit).
   So first size the diff from the commit log and the line
   count (`git diff --stat main..HEAD`), and **short-circuit
   when it is trivial** — small and confined to one of:
   comment / doc / Markdown-only, a config or workflow
   tweak, a rename, or a handful of lines with no new
   control flow. For a trivial diff, spawn **one** scoped
   reviewer (correctness + anything the diff's own nature
   calls for, e.g. the freshness lenses for a `CLAUDE.md` /
   skill edit) and **skip the step-6 cross-check** — note
   in the summary that the fan-out was scaled down. Reserve
   the full multi-lens fan-out below for a diff that earns
   it. When in doubt, fan out — the short-circuit is for the
   clearly-trivial, not the merely-small-but-subtle.

   **Reduced-fan-out cases — one scoped lens, cross-check
   skipped.** Between "trivial" and "full fan-out" sits a
   band of diffs that are *large* but whose real risk is
   **narrow**, where the full multi-lens spend returned only
   nits (each ≈0.5M–2.3M combined sub-agent input across
   PRs #202/#203/#204/#206/#210). For any of these, run a
   **single scoped lens** matched to the actual risk and
   **skip the cross-check**, noting the reduced fan-out in
   the summary:

   - **infra/ops diff** touching no program / SDK / app
     control flow (Dockerfile, compose, a make target, CI
     YAML, docs) — better verified by *building / running*
     the image than by a prose fan-out; scope to one
     ops-correctness lens.
   - **faithful extraction / move refactor** — code deleted
     in one place reappears verbatim as additions elsewhere;
     scope to one lens checking the move is faithful (no
     dropped or altered lines), not six re-reviewing
     unchanged logic.
   - **mechanical repo-wide reformat** (a formatter / lint
     autofix applied tree-wide) — discount the reformat
     noise from the sizing and scope to one lens spot-checking
     that no semantic change rode along.
   - **value / default rewiring with no new control flow** (a
     constant, default, or config value changed; no new
     branches) — one lens confirming the new values and their
     call sites.
   - **test-only diff** (`#[cfg(test)]` blocks, `tests/`, no
     production change) — cannot alter runtime behavior, so a
     single **test-validity** lens (do the tests assert the
     right thing?) is the whole review.

   **Gate the two freshness lenses on the diff's touched
   surfaces.** The four substantive lenses below
   (correctness, security, style, completeness) are
   **unconditional** — run them on every non-trivial diff.
   The two **freshness** lenses, by contrast, near-always
   return an "in sync, no-op" verdict on a pure source diff
   yet each costs ~100k+ of sub-agent input, so spawn them
   **only** when the diff actually touches the surfaces they
   police: it edits `CLAUDE.md`, `docs/conventions/**`, or
   `.github/**`, **or it adds a new top-level tree**. The
   new-tree case is load-bearing and not optional — a new
   subsystem is "source" that *does* need the conventions
   lens (e.g. a new `indexer/` tree the audit registry must
   learn about), so don't blanket-skip on "source-only". On a
   diff that touches none of those surfaces and adds no new
   top-level tree, **skip both freshness lenses** and note
   the skip in the summary. (This surface gate is narrower
   than — and composes with — the trivial-diff short-circuit
   above: a trivial `CLAUDE.md` edit still runs the freshness
   lenses; a large pure-source refactor still skips them.)

   **Scope every broad-scan lens to the diff — don't turn it
   loose on the whole convention set.** The freshness /
   conventions / completeness lenses have repeatedly
   dominated sub-agent input (≈471k, ≈627k, and one ≈5.4M /
   71-turn run) by re-reading the *whole* `CLAUDE.md` +
   `docs/conventions/` + `test.yml` and re-running repo-wide
   greps for rules the diff barely touches. Tighten the
   briefing:

   - **Name the specific implicated doc**, not "read
     `CLAUDE.md` and the relevant convention doc(s)". Point
     the lens at the one section the diff actually bears on.
   - **Run any needed repo-wide grep once, here in the main
     loop**, and hand the result set to the lens; cap its
     shell budget to "adjudicate from the diff + the provided
     grep — don't re-derive".
   - **Confirm a rule's presence or absence by `Read`ing the
     current file, never by inferring from the diff's `-`/`+`
     lines.** On a *removal* diff the freshness lens has read
     `-` lines as still-present and returned false-positive
     "stale doc" findings the cross-check then had to refute.
   - When the diff adds **no new top-level tree / build
     manifest**, fold or skip the CI-skip-list and
     audit-registry checks (nothing new for them to learn).

   Spawn parallel sub-agents via the `Agent` tool
   (single message, multiple calls) to review the
   diff — each with the brief above prepended. At
   minimum:

   - **Correctness** — logic errors, off-by-ones,
     unhandled edge cases, incorrect assumptions,
     broken invariants.
   - **Security** — injection, unchecked input,
     missing validation, unsafe operations,
     secrets in code.
   - **Style & consistency** — naming, patterns,
     idioms that diverge from the rest of the
     codebase.
   - **Completeness** — missing tests, TODO/FIXME
     left behind, partial implementations,
     unused imports or dead code introduced by
     the diff.
   - **`CLAUDE.md` + `docs/conventions/` freshness**
     (conditional — spawn only when the surface gate above
     fires) — does the project's convention set still match
     reality after this diff? `CLAUDE.md` is the index; the full
     rules live in `docs/conventions/**`. Read `CLAUDE.md`
     and the relevant convention doc(s), and check their
     rules, command examples, paths, and tooling references
     against the current codebase and the diff. Flag
     guidance the diff outdates (a command, path, target,
     or convention it renames, moves, or removes), any rule
     that has silently gone stale, **and any skill that
     references a `CLAUDE.md` section or
     `docs/conventions/` doc that the diff renamed or moved
     without the skill being updated to match** (the
     index ↔ doc ↔ skill sync). Treat a rule the diff
     **directly violates or invalidates** — or a dangling
     reference — as **blocking**; merely-stale prose as a
     **warning** with the suggested correction.
   - **CI skip-list freshness** (conditional — spawn only
     when the surface gate above fires) — the `Tests` workflow
     (`.github/workflows/test.yml`) skips the Rust suite
     only when **every** changed file lands in a known
     test-irrelevant tree. Its `changes` job encodes that
     as a fail-**closed** `code` filter: a catch-all
     `'**'` minus a list of **negated** exclude patterns
     (`'!frontend/**'`, `'!docs/**'`, …) under
     `predicate-quantifier: 'every'`, so an unlisted new
     path counts as relevant and **runs** the suite
     automatically (safe, no maintenance needed). This
     lens is therefore **not** about Rust/manifest
     additions. It is about the opposite — a diff that
     **adds or renames a test-IRRELEVANT tree** (a new
     frontend-like dir, a TS-only SDK package, a docs or
     config tree, a non-test workflow) leaves the
     exclude-list stale: PRs touching only that tree will
     needlessly run the full suite, and a renamed exclude
     points at a path that no longer exists. Read
     `.github/workflows/test.yml`, compare the `code`
     filter's `'!…'` excludes against the trees the diff
     adds or renames, and if one is not yet excluded (or
     now misnamed) flag the one-line `'!tree/**'` exclude
     addition/rename. Severity is **warning**, never
     blocking — a stale exclude-list only over-runs tests
     (the safe direction), never under-runs.

   Each sub-agent must return findings with file
   path, line number, severity (**blocking** /
   **warning** / **nit**), and a one-line
   rationale.

1. **Adversarial cross-check.** (Skipped for a trivial
   diff that took the scaled-down path above.) Spawn a fresh
   sub-agent that receives the collected findings
   and the diff (prepend the same `CLAUDE.md`
   sub-agent brief to its prompt too, and hand it the
   diff **by path** — `<scratchpad>/review-diff.txt` — as the
   review agents got it, not inlined), and is told to
   act adversarially:

   - Challenge weak or speculative findings.
     Flag false positives.
   - Identify issues the first pass missed.
   - Push back on rationale that doesn't hold up.

   **Challenge from what it was given, not by re-deriving
   the codebase.** The cross-check's inputs are the
   collected findings and the diff at `<scratchpad>/review-diff.txt`
   — tell it to reason from those plus a single up-front read
   of any file a finding cites, and to shell out again only
   to settle a **genuine** dispute it can't resolve from
   them. A cross-check that re-reads and re-greps the whole
   diff's files from scratch has cost 676.7k input
   re-deriving facts the primary lenses already passed it;
   the findings + diff are enough to adjudicate almost
   every call.

   If the cross-check produces material
   disagreements, iterate: re-spawn the relevant
   topic agent with the challenge and have it
   defend or retract. Iterate at most 2 additional
   rounds, then accept the surviving findings.

1. **Fix blocking issues** that are mechanical
   (e.g. unused imports, missing error handling,
   trivial bugs). For each fix, commit signed:

   ```sh
   git add <files...>
   git commit -S -m "<description of fix>"
   ```

   Do **not** fix issues that require design
   decisions — leave those as warnings for the
   human reviewer.

   Any inline quick-check you run to confirm a fix — a
   scoped `cargo test -p <crate> --lib`, a `cargo clippy`,
   a targeted `cargo test` — emits a `Compiling …` cascade
   ahead of its result that is pure noise once it passes, so
   run it **through the quiet runner** too
   (`python3 .claude/tools/run_quiet.py -- cargo test -p <crate> --lib`,
   per `CLAUDE.md` → "Context economy") — only the
   `test result:` / error line needs to reach context.

1. **Re-lint after fixes.** If any fix commits
   were made in the previous step, re-run
   `make lint` **through the quiet runner**
   (`python3 .claude/tools/run_quiet.py -- make lint`) to
   catch violations introduced by
   those fixes. Apply the same fix-and-retry
   logic as the lint step (step 4) — including its
   scoped per-hook re-run on a failure.

1. **Regenerate committed generated artifacts
   (mirror the IDL / SDK / vectors CI gates).** Three
   workflows fail the PR if a committed generated file is
   stale relative to its source: `test.yml` regenerates
   the **IDL**, and `sdk.yml` regenerates the **SDK
   clients** and the **conformance vectors** — each via a
   `git diff --exit-code` that fails on a dirty tree. The
   author's diff (or a fix from step 8) may have changed
   the program without regenerating these, so refresh them
   here and commit any diff; otherwise the ready PR fails
   CI on a stale artifact. Regenerate **in dependency
   order** — the SDK is generated from the IDL, which is
   built from the program. Each of these targets emits a
   full `Compiling …` cascade that is pure noise once it
   succeeds, so run them **through the quiet runner**
   (`python3 .claude/tools/run_quiet.py -- <make …>`, per
   `CLAUDE.md` → "Context economy") — it captures the build
   log to a temp file and prints only a one-line summary on
   success, or the failing tail + log path on failure (which
   you then `Read` by slice). Only the `git diff` result,
   not the build cascade, needs to reach context:

   - **IDL** (needs the Solana/Anchor toolchain):

     ```sh
     python3 .claude/tools/run_quiet.py -- make idl
     ```

     ```sh
     git diff --exit-code -- sdk/idl/dropset.json
     ```

     If the toolchain is absent and `make idl` aborts at
     `check-toolchain`, you **cannot** refresh the IDL
     locally — say so in the report (the `test.yml` IDL
     gate is then unverifiable, like the test targets) and
     continue with the committed IDL. If the diff is
     non-empty, commit it:

     ```sh
     git add sdk/idl/dropset.json
     git commit -S -m "Rebuild IDL"
     ```

   - **SDK clients** (Node + pnpm + Rust; no Solana
     toolchain needed, so always runnable):

     ```sh
     python3 .claude/tools/run_quiet.py -- make sdk
     ```

     ```sh
     git add -A -- sdk/ts/src/generated sdk/rs/src/generated
     ```

     ```sh
     git diff --cached --exit-code -- sdk/ts/src/generated sdk/rs/src/generated
     ```

     If staged changes remain, commit them:

     ```sh
     git commit -S -m "Regenerate SDK clients"
     ```

   - **Conformance vectors** (Rust only; no Solana
     toolchain needed):

     ```sh
     python3 .claude/tools/run_quiet.py -- make check-conformance-vectors
     ```

     That target regenerates the price/quoting vectors,
     stages `sdk/conformance/`, then
     `git diff --cached --exit-code`s it — so a **non-zero
     exit means the vectors were stale and are now
     staged**. Commit them:

     ```sh
     git commit -S -m "Regenerate conformance vectors"
     ```

   If any artifact commit was made, re-run `make lint`
   through the quiet runner
   (`python3 .claude/tools/run_quiet.py -- make lint`) —
   a regenerated-file commit can still trip whitespace /
   EOF hooks — applying the step-4 fix-and-retry logic
   (including its scoped per-hook re-run on a failure).

1. **Refresh the Audit registry if the diff changed the
   platform shape.** `audit` reads its subsystems,
   inter-subsystem interfaces, and skip-globs from
   `docs/conventions/audit-registry.md`; that registry is
   kept current on the PR path — here, on every run.
   Inspect the diff for any of three additions and, when
   one is present, **append** the matching entry:

   - a **new subsystem / platform** (a new top-level tree
     or build manifest the registry doesn't list) → add a
     `name (kind, risk): roots` line to the subsystems
     block;
   - a **new seam between subsystems** (a new contract
     crossing a boundary — an event schema, a generated
     surface, a documented interface) → add an
     `A <-> B: contract` line to the interfaces block;
   - a **new generated-file family** (a tree or extension
     the audit should never pick) → add its glob to the
     skip-globs block. This also covers **data-only or
     fixture JSON with no auditable logic** — e.g. the
     committed `keys/*.json` throwaway localnet keypairs
     (skipped as a family, while `keys/**` stays a
     `ci-infra` root so `keys/README.md` keeps
     doc-freshness coverage).

   **Append only** — never drop an existing entry — and
   keep the three blocks lint-clean (MD013, mdformat). If
   the diff introduces none of these, this is a no-op.
   Commit any change signed:

   ```sh
   git add docs/conventions/audit-registry.md
   git commit -S -m "Update audit registry"
   ```

1. **Run the test suite (mirror CI).** The `Tests`
   workflow runs `make test` and
   `make test-no-teardown`; run both locally so the
   green checks GitHub needs for auto-merge are
   already verified here. Both emit a long `Compiling …`
   cascade ahead of the test result, so run them **through
   the quiet runner** (per `CLAUDE.md` → "Context economy")
   — it routes the build/test log to a temp file and
   surfaces only the one-line pass summary, or the failing
   tail + log path you then `Read` by slice:

   ```sh
   python3 .claude/tools/run_quiet.py -- make test
   python3 .claude/tools/run_quiet.py -- make test-no-teardown
   ```

   - Both depend on the Solana/Anchor toolchain via
     `check-toolchain`. If the toolchain is absent
     and a target aborts before any test runs, you
     **cannot** verify that workflow locally — say
     so explicitly in the report (do not claim CI
     will pass), rather than counting it as green.
   - If a test fails, fix it when the fix is
     mechanical (commit signed, then re-run the
     failed target), otherwise catalogue it as a
     **blocking** issue. Never mark the PR ready
     with a failing or unverified test target.

1. **Push the branch.** The step-1 rebase rewrote
   history, so push with lease — this lands the rebased
   history together with any review-fix commits, and
   refuses to clobber a concurrent push:

   ```sh
   git push --force-with-lease
   ```

1. **Update the PR title and description.** Invoke
   `/pr-title-description` to ensure the title
   and body reflect the final state of the branch
   (after lint and review fixes).

   **No Linear tags in the body or in any PR comment**
   (per `CLAUDE.md` → "Keep Linear tags out of PR bodies
   and comments"): `pr-title-description` already keeps
   them out of the body, and any comment this skill posts
   on the PR must do the same — refer to other work by
   title or a plain GitHub link, never `ENG-###`. The
   `ENG-###` scope in the **title** is the one exception
   (required by `Semantic PR`). This rule does **not**
   touch the terminal `AskUserQuestion` prompts below,
   which deliberately print the tag + PR number as
   terminal chrome.

1. **Verify the PR title passes `Semantic PR`.**
   The `semantic-pr` workflow rejects the PR unless
   the title has a Conventional-Commits type, a
   scope matching `^ENG-[0-9]+$`, and a subject
   matching `^[A-Z].*$` (capitalized first letter).
   Confirm the final title looks like
   `feat(ENG-451): Add …`; if it doesn't conform,
   re-run `/pr-title-description` to fix it. The
   workflow also sets `validateSingleCommit`, so if
   the branch has exactly one commit, that commit's
   message must itself match the title — squash or
   reword so they agree.

1. **Confirm the PR still merges cleanly.** Step 1
   already rebased onto `main`, so this is normally
   clean — but `main` can advance again during a
   long review, so confirm rather than assume. Fetch
   the latest base, then ask GitHub with a
   **field-selected** `gh pr view` — only the two merge
   fields, so this one-shot read doesn't pull the full PR
   object into context (per `CLAUDE.md` → "Context
   economy" / "GitHub via MCP"):

   ```sh
   git fetch origin main
   ```

   ```sh
   gh pr view <number> --json mergeable,mergeStateStatus
   ```

   `mergeable` is the tri-state conflict signal
   (`MERGEABLE` / `CONFLICTING` / `UNKNOWN`);
   `mergeStateStatus` is the detail (`CLEAN`, `BLOCKED`,
   `BEHIND`, `UNSTABLE`, `HAS_HOOKS`, `DIRTY`, …). Key the
   decision on `mergeable`, which is the gh equivalent of
   the MCP `mergeable_state`:

   - `mergeable: "CONFLICTING"` (or `mergeStateStatus: "DIRTY"`) → the
     PR has merge conflicts. Catalogue this as a **blocking** issue
     and do **not** mark the PR ready. Tell the user to rebase onto
     `main` and resolve the conflicts (this skill does not
     auto-resolve them), then re-run `/review-pr`.
   - `mergeable: "UNKNOWN"` → GitHub hasn't finished
     computing mergeability yet. Wait a few seconds and
     re-run the `gh pr view` call until it settles.
   - `mergeable: "MERGEABLE"` (any `mergeStateStatus` —
     `CLEAN`, `BLOCKED`, `BEHIND`, `UNSTABLE`,
     `HAS_HOOKS`) → **no merge conflict** — proceed to the
     gate. `BLOCKED` / `UNSTABLE` just mean branch
     protection, the required checks, or human review
     haven't cleared yet (expected for a draft PR
     mid-review); `BEHIND` means `main` moved (the step-1
     rebase already handled it). None of these are a
     conflict, and the gate + CI wait below cover them.

1. **Gate.** Mark the PR ready only when **every**
   local CI-mirroring check is green: **zero blocking
   issues** (including every Linear checklist item
   addressed), `make lint` passes — real violations
   resolved, with any hook that couldn't run locally
   (e.g. `biome` / `tsc` without frontend deps) noted as
   unverifiable, not gated — the generated
   artifacts are fresh and committed (IDL — or honestly
   reported unverifiable if the toolchain is absent — SDK
   clients, conformance vectors), `make test` and
   `make test-no-teardown` pass (or are honestly
   reported as unverifiable locally), the title
   passes `Semantic PR`, and `mergeable` is not
   `CONFLICTING` (no merge conflict). Take the PR out of draft
   with
   `mcp__github__update_pull_request` (`draft: false`):

   ```txt
   mcp__github__update_pull_request(
     owner: "DASMAC-com",
     repo: "dropset",
     pullNumber: <number>,
     draft: false,
   )
   ```

   Marking it ready (out of draft) is what lets the
   human enable GitHub's **"Merge when ready"**
   auto-merge while the real CI finishes. But ready is
   **not** the end of this skill: the Linear issue stays
   **In Progress** (it does *not* move to In Review here),
   and the run does **not** report success, until the
   actual CI is green and the review summary is in front
   of the human — the next steps. In Review comes later
   still, at the merge-queue handoff.

   If any blocking issue remains — an unaddressed
   Linear checklist item, failing or unverified
   tests, a non-conforming title, or a merge
   conflict with `main` — do **not** mark the PR ready.
   Leave it in draft and the issue in its current state,
   **skip the CI wait below**, and report the blockers.

1. **Wait for GitHub CI to pass.** The issue stays
   **In Progress** throughout this step — it does *not*
   move to In Review at CI-green; that transition belongs
   to the merge-queue handoff a few steps down, once the
   review summary is in front of the human. The local
   checks only *mirror* CI;
   the authoritative signal is the real run on the
   pushed commits — and when the toolchain was absent
   locally (tests / IDL reported unverifiable), CI is
   the *only* signal. This repo runs CI on the PR even
   while it was a draft (that's how `init-pr` warms the
   caches), so the checks are already in flight. There's no
   streaming `--watch`, so **poll** — but poll with the
   **compact** `gh pr checks`, one line per check, rather
   than the MCP `get_check_runs`, which returns the full
   check-run array on **every** poll and replays it into
   context each later turn (per `CLAUDE.md` → "Context
   economy" / "GitHub via MCP"). Re-issue this until every
   check is no longer pending:

   ```sh
   gh pr checks <number>
   ```

   Each line names a check and its status — `pass`, `fail`,
   `pending`, or `skipping` — with its elapsed time and URL; the exit
   is `0` (all passed), `8` (some still pending), or `1` (one or more
   failed). That one-line-per-check snapshot — not a full object — is
   the signal.

   This is a **model-driven** poll, not a shell watcher.
   Re-issue the single `gh pr checks` call above as a fresh
   tool call across successive turns — **never** a shell
   `while … sleep … done` loop, a `--watch`, or a `jq`
   filter (a compound that can't reduce to an allow-rule,
   and foreground `sleep` is blocked anyway). To pace the
   re-calls across turns rather than busy-looping, schedule
   a wakeup (e.g. `ScheduleWakeup`) — one probe per wake.
   Tell the human **once**, up front, that CI is in flight
   and you're standing by, then stay silent: don't narrate
   each poll. Ping again only on a **terminal** outcome (no
   check still `pending` — the branch below). Two
   operational notes:

   - Polling is naturally resumable: each call returns the
     current snapshot, so if a wait is interrupted, just
     call again until nothing is `pending`.
   - If `gh pr checks` reports **no checks** on the head
     commit (it says so and exits non-zero), there is
     nothing to wait on — note that in the report and treat
     it as green rather than polling forever.

   Then branch on the outcome — a check line reading `pass`
   / `skipping` is passing, `fail` is failing:

   - **All checks green** (no `fail`, none `pending`) → the
     PR is now ready **and** CI-green. Leave the Linear
     issue **In Progress** (it moves to In Review at the
     merge-queue handoff, not here) and proceed to print the
     review summary — the human reviews that summary, then
     approves enqueueing.

   - **Any check failed** → the PR is not actually clean,
     so don't leave it reading as merge-ready. Catalogue
     each failing check as **blocking**, naming it and its
     URL (the last column of the `gh pr checks` line). To
     pull the actual failing-job output in one call, take
     the workflow run id from that URL — it is
     `…/actions/runs/<run_id>/job/<job_id>` — and fetch
     every failed job's log together over the MCP (this
     failure path stays on the MCP — `get_job_logs` already
     caps its output with `tail_lines`):

     ```txt
     mcp__github__get_job_logs(
       owner: "DASMAC-com",
       repo: "dropset",
       run_id: <run_id>,
       failed_only: true,
       return_content: true,
       tail_lines: 100,
     )
     ```

     Then convert the PR back to draft — which also
     cancels any pending "Merge when ready" — and leave
     the Linear issue in its current state (do **not**
     move it to In Review):

     ```txt
     mcp__github__update_pull_request(
       owner: "DASMAC-com",
       repo: "dropset",
       pullNumber: <number>,
       draft: true,
     )
     ```

     Report the failures and do **not** report the run
     as finished; the user fixes them and re-runs
     `/review-pr`.

1. **Print the review summary.** With CI green, print the
   structured summary now — *before* the merge-queue
   prompt — so the human reviews the full picture at the
   moment they decide whether to enqueue. The merge-queue
   outcome, the `firm-perms` results, and the session-metrics
   capture aren't known yet (all resolve in the steps below);
   they're surfaced separately as they land, not folded in
   here.

   - Linear coverage: the resolved tag, and each
     checklist item marked addressed / partial /
     missing (or "no Linear task checked" if none
     was resolvable).
   - Lint status: pass, fail with details, or which
     hooks were unverifiable locally (deps not
     installed) and why that's safe for this diff.
   - Generated artifacts: IDL / SDK clients /
     conformance vectors — regenerated clean, committed a
     refresh, or (IDL only) unverifiable without the
     toolchain.
   - Test status: `make test` and
     `make test-no-teardown` — pass, fail, or
     unverified locally (toolchain absent).
   - Title status: passes `Semantic PR` or not.
   - Merge status: `mergeable` — `CONFLICTING` vs.
     `MERGEABLE` (no conflict).
   - CI status: all GitHub checks green, or "no checks"
     treated as green — by this point CI has passed (a
     failure would have stopped the run at the CI wait).
   - Linear status: currently **In Progress**; it moves to
     **In Review** at the merge-queue handoff that follows
     (or stays put if no tag was resolvable).
   - `CLAUDE.md` + `docs/conventions/` freshness: in sync,
     or each stale rule / dangling skill reference the diff
     outdated, with the suggested correction — or **skipped**
     (the surface gate didn't fire: the diff touched no
     `CLAUDE.md` / `docs/conventions/` / `.github/` surface
     and added no new top-level tree).
   - CI skip-list freshness: the `test.yml` `code`-filter
     exclude-list is in sync, or each test-irrelevant tree
     the diff added/renamed that should be excluded, with
     the suggested one-line edit (warning only) — or
     **skipped** (surface gate didn't fire, as above).
   - Issues found / fixed / remaining.
   - Remaining warnings and nits for human review,
     each with `file:line` and rationale.
   - Whether the PR was marked ready.

1. **Move the issue to In Review and offer to add the PR
   to the merge queue.** Run this step **only** when the
   CI wait took the **all-checks-green** path and the
   review summary above has been printed — the PR is
   ready, CI is green, and the human has the full picture
   in front of them. (If CI failed, no checks ran, or the
   gate was never reached, skip this entirely.)

   **First, re-check mergeability — a PR that was
   `MERGEABLE` at the ready gate can turn `CONFLICTING`
   while CI ran, if `main` advanced.** So before moving the
   issue and before the prompt, re-read the conflict signal
   (the same read the ready gate used):

   ```sh
   gh pr view <number> --json mergeable,mergeStateStatus
   ```

   - `mergeable: "CONFLICTING"`
     (or `mergeStateStatus: "DIRTY"`) → **do not** offer to
     enqueue and **do not** advance the issue. Report the
     conflict, tell the human to rebase onto `main` to
     resolve it (this skill does not auto-resolve), and leave
     the issue **In Progress**. Stop here — the enqueue offer
     is off the table until the rebase clears the conflict.
   - `mergeable: "UNKNOWN"` → GitHub hasn't finished
     computing mergeability; re-poll a few times (a short
     wait between reads) before deciding rather than
     offering blindly. If it stays `UNKNOWN`, say so and
     hold rather than enqueue.
   - `mergeable` not conflicting (`MERGEABLE`) → proceed to
     the In Review move and the enqueue prompt below.

   This prompt is the handoff: per `CLAUDE.md`, **In
   Review** means "okay for the human to look at the PR
   and approve enqueueing it." So move the Linear issue
   (the tag resolved in step 3) to **In Review** here —
   with, or just before, the prompt — not earlier. Skip
   only if no tag was resolvable:

   ```txt
   mcp__claude_ai_Linear__save_issue(
     id: "<ENG-###>",
     state: "In Review"
   )
   ```

   **One write, no retry loop.** If the response echo comes
   back still showing In Progress, do **not** re-issue the
   write chasing it (that loop cost five body-echoing
   `save_issue`/`get_issue` round-trips on PR #207) — verify
   once and, if it still disagrees, report the discrepancy
   and move on. The transition is idempotent; a silent echo
   is not worth another full-body round-trip.

   Then ask with `AskUserQuestion` — always this tool, so
   the human gets the little TUI pop-up selector and picks
   "yes, add it to the merge queue" (or "skip, I'll merge
   by hand") right in the terminal instead of typing a
   reply. In the question text, **clearly print both
   identifiers the human needs to pull up the PR**: the
   Linear tag (e.g. `ENG-536`) and the GitHub PR number
   (e.g. `#138`) — so it's unambiguous which PR they're
   approving.

   - **If the user approves**, add it to the merge queue.
     Enqueueing is the one `gh` **write** the skill makes
     (the dequeue probe in the final step is the one `gh`
     **read**): the MCP server exposes no auto-merge /
     merge-queue tool (`merge_pull_request` does an
     *immediate* merge, which bypasses the queue), so use
     `gh pr merge` with `--auto`, which enables "Merge
     when ready" / enqueues behind the required checks:

     ```sh
     gh pr merge <number> --auto
     ```

     **Pass no merge-strategy flag** (no `--squash` /
     `--merge` / `--rebase`). This repo is governed by a
     GitHub **merge queue**, which sets the strategy itself;
     an explicit `--squash` conflicts with it and `gh` warns
     that the merge strategy for `main` is set by the merge
     queue. The enqueue still takes (exit 0), but the flag is
     pure noise — omit it and let the queue decide.

     **Confirm the enqueue from the `gh` exit, not from a
     polled field.** A zero exit means "Merge when ready"
     is now enabled — the enqueue took. (The hosted GitHub
     MCP's `pull_request_read` `get` response does **not**
     carry `auto_merge`, so there is no MCP field to poll
     for the enqueue; the `gh` exit is the signal.) Report
     the enqueue and move on — the queue *outcome* (landed
     vs. taken out) lands asynchronously and is surfaced by
     the final step, after `firm-perms`; do **not** block
     here waiting for the merge.

   - **If the user declines**, leave the PR ready and the
     issue In Review, and note that they can merge it (or
     enable "Merge when ready") themselves.

1. **Firm up the permission allowlist** (while the PR
   sits in the queue). A review run approves a lot of
   one-off commands, so it is the natural moment to
   generalize them. Run this **after** the enqueue,
   **not** gated on the merge landing — the merge resolves
   asynchronously in the queue, so this is the productive
   thing to do while it does.

   **Ask first, via `AskUserQuestion`.** This is a
   skill-to-skill handoff, so gate it on the same TUI
   selector the merge-queue prompt uses (per `CLAUDE.md` →
   "The PR workflow and skill handoffs"): ask whether to
   firm permissions now, offering "yes, run /firm-perms"
   (**first**, the recommended default) and "skip". This
   is a second, lighter gate *in front of* `firm-perms`'
   own propose-then-confirm gate — intentional: the
   `AskUserQuestion` decides *whether to firm at all this
   run*; `firm-perms`' internal gate still governs *what
   gets written*.

   - On **decline**, skip this step and note in the
     report that permissions were **not** firmed this run.
   - On **approve**, invoke `/firm-perms` to collapse the
     per-worktree and per-arg `permissions.allow` entries
     into reusable globs and propagate them to the base
     repo so future worktrees inherit them. This is
     housekeeping on the gitignored
     `.claude/settings.local.json` — it does **not** affect
     the PR diff or its ready state, so run it regardless
     of the gate or CI outcome.

   **Account for what the review agents requested.**
   The diff-review and cross-check agents (steps 5–6)
   run in this session, so every command they made you
   approve is part of this run's churn. Tell
   `/firm-perms` to fold those in too — its harvest
   covers sub-agent approvals, not just commands you
   typed. Two outcomes, and report both:

   - A request that **can** be firmed (a bare command
     that just needed a `:*` glob) gets generalized
     and propagated like any other.
   - A request that **can't** — a `find / … | head`, a
     `sed … | grep`, a heredoc — is malformed, not
     missing a glob; `/firm-perms` sets it aside. When
     it does, that's a signal the **step-5 reviewer
     brief leaked**: an agent emitted shell the brief
     forbids. Tighten the brief (or the prompt) so the
     pattern stops recurring, rather than trying to
     allow-list it.

1. **Capture session metrics** (while the PR sits in the
   queue). A review run is long and tool-heavy, so it is the
   natural moment to account for where its tokens went and
   bank trim recommendations for the skill suite. Like
   `firm-perms`, this is productive work to do while the
   merge resolves asynchronously — run it **after** the
   enqueue, **not** gated on the merge landing, and run it
   regardless of the merge outcome (it analyzes the session,
   not the PR).

   **Ask first, via `AskUserQuestion`.** This is a
   skill-to-skill handoff, so gate it on the same TUI
   selector the merge-queue and `firm-perms` prompts use
   (per `CLAUDE.md` → "The PR workflow and skill handoffs"):
   ask whether to capture session metrics now, offering
   "yes, run /session-metrics" (**first**, the recommended
   default) and "skip".

   - On **decline**, skip this step and note in the report
     that session metrics were **not** captured this run.
   - On **approve**, invoke `/session-metrics`. It derives
     this session's id from the scratchpad path, runs the
     `session_metrics.py` tool to rank the run's token sinks
     and hardening candidates (the transcript is read in the
     tool's own process, so it never enters context), and
     appends a dated entry — measured sinks plus tailored
     trim recommendations — to the Linear "Session Metrics" inbox
     document that `housekeeping` later drains. It authors no
     source edit, so it's safe to run regardless of the gate
     or CI outcome. If `LINEAR_SESSION_METRICS_DOC_ID` is
     unset, the skill no-ops with a clear message — note that
     in the report.

   **Ground the recommendations in this run.** As the review
   progressed you may have noticed wasteful payloads (a
   whole-file Read, a verbose build log, a repeated full PR
   read, an inlined-diff fan-out). Per `CLAUDE.md`'s "track
   consumption ideas as you go" habit, carry those
   observations into `/session-metrics` so its prose names
   concrete levers, not just the tool's raw sink ranking.

1. **Surface the merge-queue outcome** (separately). Run
   this **only** if the user approved the enqueue (skip it
   if they declined — there's nothing queued to watch). The
   merge lands asynchronously, so this is its own note,
   printed after `firm-perms` and after the review summary
   above — the summary couldn't know this outcome yet.

   Watch whether the PR lands or gets kicked back out with
   the **same model-driven poll** as the CI wait, but with a
   **single** probe per poll: the `gh api graphql` dequeue
   probe below already selects `state` and `merged` **and**
   the merge-queue fields, so it answers "landed?", "still
   queued?", and "dequeued?" in one read — the old
   `pull_request_read` `get` poll that used to run first was
   redundant (it carried the full PR object every poll just
   to read `state`/`merged`) and is dropped (per `CLAUDE.md`
   → "Context economy"). Re-issue the one probe as a fresh
   tool call across successive turns, paced with a scheduled
   wakeup (e.g. `ScheduleWakeup`) — **never** a shell
   `while … sleep … done` loop or a `jq` filter. Say once,
   up front, that the PR is queued and you're standing by;
   then stay silent until a **terminal** outcome (merged,
   or taken out of the queue), pinging the human only then.
   Each poll is resumable — a fresh call returns the current
   snapshot.

   This is the one `gh` **read** the skill makes (mirror of
   the enqueue write). The signal that distinguishes "still
   queued" from "silently removed" is the PR's
   **`mergeQueueEntry`**: it is non-null exactly while the PR
   sits in the merge queue and flips to `null` the moment it
   leaves. **Do not** key on `autoMergeRequest` for that — on
   a merge-queue repo a genuinely-queued PR reports
   `autoMergeRequest: null` (and a `CLEAN` `mergeStateStatus`,
   not `QUEUED`), so the old `autoMergeRequest`-null test was
   a **false positive** that announced "taken out of the
   queue" on every run. `mergeQueueEntry` isn't exposed by
   the MCP `get` (nor by `gh pr view --json`), so query it
   over GraphQL, where the same query also returns `state`,
   `merged`, and `autoMergeRequest` (the last keeps the
   **classic-auto-merge** path — repos with no merge queue —
   working).

   Keep the command globbable: the query body has braces and
   quotes that trip the brace-with-quote guard, so write it
   to a file with the **Write** tool rather than inlining it,
   then pass the PR number as a typed variable and the file
   on a stable command line (per the file-handoff rule in
   `CLAUDE.md`). The query (write it to e.g.
   `/tmp/mq-probe.graphql`):

   ```graphql
   query($number: Int!) {
     repository(owner: "DASMAC-com", name: "dropset") {
       pullRequest(number: $number) {
         state
         merged
         mergeQueueEntry { state }
         autoMergeRequest { enabledAt }
       }
     }
   }
   ```

   ```sh
   gh api graphql -F number=<number> -F query=@/tmp/mq-probe.graphql
   ```

   This reduces to a `Bash(gh api graphql:*)` allow-rule —
   only `<number>` varies; the brace-heavy query rides in the
   file, not the command line. Branch on the single result:

   - `merged: true` (or `state: "MERGED"` / `"CLOSED"`) → it
     landed; report the merge. Key on `merged` / `state`.
     Then **dismiss this PR's own GitHub notification** so it
     doesn't linger (the immediate companion to
     `housekeeping`'s merged-PR notification sweep): list the
     notifications and dismiss the one whose
     `subject.url` ends in this PR's number — never
     `mark_all_notifications_read`:

     ```txt
     mcp__github__list_notifications(
       owner: "DASMAC-com",
       repo: "dropset",
     )
     ```

     ```txt
     mcp__github__dismiss_notification(
       threadID: "<this PR's notification id>",
       state: "read",
     )
     ```

     If no notification matches (already cleared), skip it.

   - `state: "OPEN"` with `mergeQueueEntry` non-null (or, on
     a classic-auto-merge repo, `autoMergeRequest` non-null)
     → still queued; keep polling.

   - `state: "OPEN"` with both `mergeQueueEntry` **and**
     `autoMergeRequest` null → it was **taken out** of the
     queue (a required check went red, a conflict appeared,
     or someone dequeued it). Report the removal, naming the
     cause from a fresh `gh pr checks <number>` if a required
     check shows `fail`.
