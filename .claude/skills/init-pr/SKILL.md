---
name: init-pr
description: Bootstrap a worktree — pre-check the session's model tier and the gh credential first, then fetch main, set up the branch, push a draft PR, and warm CI caches.
disable-model-invocation: true
user-invocable: true
---

<!-- cspell:word ETIMEDOUT -->

# `init-pr`

Bootstrap the current worktree: fetch main, set up the
branch, push a draft PR so CI caches start warming while
work continues.

This is the first skill an agent should run after
`claude --worktree <tag>` starts.

Two cheap pre-checks come **first**, before anything that
mutates the worktree — the model tier this session is running
as, and the `gh` credential. Both exist because the failure
they catch is otherwise discovered late and expensively.

## Step 0: check which model this session is running as

**Before any other work — before the helper call, before the
rebase — check the model this session is running as.** The
system prompt states it.

The working split is: **planning** sessions run on a
Fable/Mythos-tier model, **implementation** sessions
(`init-pr` → run-to-completion → `review-pr`) run on the
saved default. The failure mode is forgetting to switch back
and burning Fable-tier credits on a 40-minute deterministic
implementation run — which is exactly what this skill starts.

So if this session is a **Fable/Mythos-tier** model, **stop
and ask** via `AskUserQuestion` before doing anything else,
with the recommended option first:

1. *"Restart this session on the default model and re-run
   /init-pr"* — recommended.
1. *"Continue on this model anyway"*.

On the first, stop; the user restarts. On the second, proceed
normally and don't ask again this session.

Two things to get right:

- **Gate on the Fable/Mythos tier specifically, not on "is
  not Opus".** A Haiku or Sonnet session is a deliberate
  cheap-run experiment and must not trip the guard. Keeping
  the check to a named tier list also means that if the
  pricing picture changes later, this is a one-line edit.
- **Detect-and-stop is the whole mechanism.** A skill cannot
  switch the session's model, so there is no hook and no tool
  here — just the check and the question.

`review-pr` deliberately gets **no** such guard: it runs
inside the same implementation session, which is already on
the right model by the time it is invoked.

## Step 0b: pre-check the GitHub credential

Still before anything that mutates the worktree, confirm the
`gh` credential works:

```sh
gh auth status
```

One call, free on the happy path, and it prevents a
half-finished bootstrap. One run reached **step 7 of 12** —
branch renamed, rebased, signed empty commit made — before
`git push` died with "could not read Username", because the
token had expired. Diagnosing it then took five extra calls
— `git remote -v`, `gh auth status`, the credential helper
config, `git ls-remote`, a retry — plus two `printenv` token
probes, and the first evidence was actively misleading:
**anonymous reads succeed on this repo**, so `git ls-remote`
came back clean while pushes were dead.

Read the **scope set** from the same output while you are
there. The step-9 notification-unsubscribe needs the
`notifications` scope, and a re-auth silently drops it — that
step is best-effort so it won't block, but naming the missing
scope here beats discovering it at the end. The one-time
operator fix:

```sh
gh auth refresh -h github.com -s notifications
```

If `gh auth status` reports no valid credential, **stop** and
tell the user to re-authenticate — don't rename, rebase, or
commit first.

## Input

Accepts an optional Linear tag like `eng-123`.
If not provided, infer it from the worktree
directory name (the last component of the current
working directory). If the inferred name doesn't
match `eng-###` (case-insensitive), stop and ask.

**When invoked with no other context** — just the
tag (or nothing), and no task instructions in the
session — treat the linked Linear issue as the full
specification for this worktree. After
bootstrapping, surface that issue's description and
checklist as the plan of work (final step) so the
session can proceed straight into the task without
asking what to build. Instructions the user *did*
give take precedence over the issue.

## Decision points use `AskUserQuestion`

`init-pr` brackets the whole worktree session: it
bootstraps, surfaces the task, and the session then
proceeds straight into the work. Wherever that flow
needs a decision from the user — a design choice, an
open question, a branching point — ask through the
**`AskUserQuestion`** TUI selector, not a free-text
prompt, so the human picks from the little terminal
pop-up instead of typing a reply. Offer concrete
options, and where one is the sensible default put it
**first** and label it "(Recommended)". The closing
`/review-pr` handoff (final step) is one such decision
point; the same applies to every other one the session
surfaces. This mirrors how `review-pr` already prompts
at its merge-queue handoff — the same TUI-selector
pattern, applied one stage earlier at the
init-pr → review-pr boundary.

## Surveying code: prefer the `Explore` agent — and scope it

When the surfaced task is greenfield and the work begins
with **surveying implementations** — reading one or more
repos (external *or* in-repo) to learn a pattern before
building — spawn the **`Explore`** agent for that survey,
**not** a `general-purpose` agent, and pass it an explicit
file/dir **path allowlist** scoped to what's worth reading.

`Explore` reads **excerpts** (it locates and slice-reads)
rather than ingesting whole files, so it caps the dominant
cost of a research phase: a `general-purpose` survey of
reference repos has pulled **multi-MB of external source
whole-file** into context (e.g. an entire reference repo at
2–2.5M input each), which is then replayed every later turn
(per `CLAUDE.md` → "Context economy"). Whole-repo ingestion
is somewhat inherent to "survey N references," but `Explore`
plus a scoped allowlist is the lever that bounds it. Give
the agent the canonical sub-agent brief
(`docs/conventions/sub-agent-brief.md`) and name the
specific paths it should look at, rather than turning it
loose on a whole tree.

**In-repo / in-workspace surveys need the same scoping —
they are not exempt.** An open-ended "map how the TUI + the
bots work" over this workspace was the single top token
sink of three consecutive sessions, each time answering a
question that ultimately needed only ~3–6 named modules the
main loop then Read anyway (duplicating the survey). So when
the survey is an in-repo map, don't hand the agent a broad
mandate: give it an **explicit named-path file allowlist**
(the specific modules you expect to matter) **and a turn
budget** (e.g. "≤ 8 turns, then report"), and ask for a
compact map — file → responsibility → the few symbols that
matter — not a narration of everything it read. And weigh
whether to spawn an agent at all: a **≤ ~3-file question is
cheaper Read directly** from the main loop than surveyed,
since a sub-agent survey of it just gets re-Read afterward.

## Implementing the task: keep context discipline on

The same context economy `review-pr` enforces applies during
the **implement** phase this skill hands off into — it slips
here precisely because no skill is driving. These habits, per
`CLAUDE.md` → "Context economy", apply to the **main loop**,
not only to the sub-agents you brief:

- **Slice-read large files.** To find an append point,
  confirm an import, or edit one function in a big source
  (a 600–1000-line module whose `#[cfg(test)]` block is half
  the file), **Grep to the region** then `Read` with
  `offset`/`limit` — don't pull the whole file.

- **Map the structure before any Read over ~300 lines —
  scoped to the file(s) you are about to read.** One Grep for
  `^fn |^impl |^pub` (or the language's equivalent) gives you
  the section map, and the map tells you which slice you
  actually want. A dispatcher whole-file Read (≈4.4k) to find
  **one** append point is the recurring shape this prevents.

  **Pass `--glob <the file>`.** This instruction used to name
  a pattern and no scope, and aimed at the whole source set it
  *becomes* the sink: an `^export|^function|^const` probe
  returned 747 matches across 75 files and was one session's
  single largest result (≈4.5k) — fired to map the structure of
  two files it had already identified, and answering nothing
  the run used. The map you want is of the file you are
  opening, not of the repo.

- **This covers a sibling `SKILL.md` or convention doc too —
  those are what a mid-session handoff actually reaches
  for.** The rule reads as being about large *source* files,
  which is why it gets skipped for skill docs, and several of
  those run past 1800 lines. On one two-line copy PR the two
  largest results of the entire run were `review-pr/SKILL.md`
  (≈1.6k, sliced) and `pr-title-description/SKILL.md` (≈1.3k,
  read whole) — together ~93% of its Read cost, when all that
  was needed from the latter was the title/description format
  in its steps 3–4. Grep the doc's headings (`^#`), then
  slice.

- **A planned multi-region read is ONE bounded read, not
  several.** When you already know you need three parts of a
  file, don't slice-read it three times — one run read
  `swap.rs` across four separate slices, together **more** than
  a single whole-file read would have cost. Slicing is only
  cheaper when you are reading less.

- **Reading 3+ files just to orient is the trigger, not an
  exception.** Whole-file Reads at *survey* time were the
  single largest sink of one session (top five, ≈15k) — the
  crate was small, so no per-file budget felt warranted, yet
  `model.rs` is ~40% `#[cfg(test)]` and only two signatures
  were needed. Grep to the symbol, then `Read` the slice.

- **Reading whole is licensed by any ONE of three
  conditions** — they are alternatives, not a conjunction, and
  the full statement with its evidence is in
  `docs/conventions/context-economy.md` → "The levers":

  1. you will **both** edit the file and brief agents on it
     (one session's five whole reads, ≈23k, went inline into
     all five lens briefs — paid once, amortized five times);
  1. you have planned a **multi-region** read whose regions add
     up to most of the file;
  1. the file is an **exemplar you are about to imitate N
     times**, so the read amortizes across the N outputs.

  Absent all three, slice.

- **Route any repeated verbose-on-success command through the
  quiet runner** — `cargo`, `make`, **and `pnpm`**. Run it as
  `python3 .claude/tools/run_quiet.py -- <cmd>` **during
  implementation**, not only during `review-pr`. Naming only
  cargo and make is what let a whole runner slip: one session
  ran the frontend test script 12 times and a frontend `exec`
  9 times, all unwrapped, ≈5.2k combined for output that is one
  line when it passes — and every entry in that session's
  hardening table was a `pnpm` shape.

  **Nothing prints until the command exits**, so do not poll
  the log of a *backgrounded* run — one session made seven such
  `tail` calls, all empty. Wait for the completion
  notification.

- **Verify at checkpoints, not after every edit.** Those 12
  test runs were a fix-verify loop after single-file edits,
  which `review-pr` already forbids; it slipped because that
  rule is written in terms of the Rust suites, so a frontend
  test script read as out of scope. It is not — batch a logical
  change, then verify once, whatever the runner is.

- **Lint the changed set, not the whole tree.** After an edit,
  the post-edit check is one bare command:

  ```sh
  python3 .claude/tools/run_quiet.py -- \
    python3 .claude/tools/lint_paths.py --changed
  ```

  It resolves this branch's own files (merge-base with
  `origin/main`, plus untracked-not-ignored paths) and runs the
  hooks over just those; append `-- <hook-id>` to narrow
  further. The full `make lint` is for the two checkpoints —
  once before committing, once at the end — and nowhere else.
  This exists because restating the rule demonstrably does not
  work: one session paid **13 full sweeps (≈5.8k)** while
  editing the rule that forbids them, for the plain reason that
  `make lint` needed no arguments and the scoped form did. Now
  neither does.

- **Run a fast suite whole, through the wrapper — not per
  module.** For an edit under `.claude/tools/`:

  ```sh
  python3 .claude/tools/run_quiet.py -- make tools-tests
  ```

  Not `python3 -m unittest discover … -p test_X.py`. The narrow
  form feels cheaper because it targets the one tool you edited,
  and it is not: measured at **32 calls / ≈7.1k** against **15
  calls / 516 tokens** for the whole suite, because the `make`
  target is wrapped and the discover call is not. It is ~14× per
  call for a *narrower* answer — and it missed a sibling test
  the edit had just broken, twice in one session. Reserve the
  per-module form for a suite slow enough that the wall-clock
  saving exceeds the context; this one runs in under a second.
  See `docs/conventions/context-economy.md` → "When a suite is
  fast enough to run whole".

- **Poll CI with the committed tool, not by hand.** One session
  ran `gh pr checks` four times manually (922 tokens) before
  using `python3 .claude/tools/wait_for_checks.py` once (≈200).
  The tool existed and `review-pr` prescribes it; the manual
  polls happened here, in the implement phase, where no skill
  was driving.

- **Search source with the tool, not a bare recursive grep.**
  `python3 .claude/tools/search_source.py '<pattern>'` already
  prunes the generated families and the never-search trees
  (`target/` alone is multi-GB and `grep -r` does not honor
  gitignore), and it reduces to one stable allow-rule however
  the pattern and filters vary.

- **Match the search shape to the question type.** This is the
  single most recurring trim lever across mined sessions, and
  it is missed *here*, in the implement phase, because the rule
  reads as belonging to `review-pr`'s hoisted-grep step. It
  does not — it is phase-neutral (see
  `docs/conventions/context-economy.md` → "The levers"). When
  the question is **where is it** or **does it exist**, ask
  `--files-only` and stop; take `--context N` only when the
  question is genuinely *what does this code do*. Seven
  separate sessions answered a location question with a full
  context sweep, one paying ≈3.6k to find a three-line
  function.

  **Narrowing the SCOPE is a separate axis from narrowing the
  output.** The rule above bounds what each match prints; it
  says nothing about how much tree gets searched, and three
  sessions paid for that gap. One's largest result (≈3.6k) was
  a repo-wide sweep for identifiers that were entirely
  frontend-local, and the very next call with `--dir frontend`
  answered the real question for a fraction. Pass `--dir` or
  `--glob` whenever the claim is confined to one tree.

  **And `--context N` scales with match DENSITY, not count.**
  Clustered matches make context windows overlap toward buying
  the file outright: a `--context 3` sweep hitting 21 matches
  in a single file bought that file roughly twice (≈3.1k)
  *after* `--files-only` had already identified it. When matches
  cluster in one file, take `--files-only` then slice-read the
  region. `search_source.py` now says so on its own summary
  line when a context sweep spans many files or piles up in one.

- **Verify a list-producing flag with a count, not the list.**
  One session's largest single result (≈5.8k) was a new tool's
  `--print` dumping ~600 paths to answer the yes/no question
  "did the flag work".

- **Narrowest-form applies to listing and blob commands too.**
  `git show <ref>:<path>` prints the **whole blob** (≈3.8k) —
  `--no-patch` suppresses a *diff*, not a blob dump; a
  `git ls-files` sweep cost ≈2.2k to locate one known file; and
  for a single scalar from GitHub, a field-selected
  `gh api --jq` beats the MCP getter by orders of magnitude
  (`get_latest_release` returned 60,413 characters and
  overflowed the result cap). See
  `docs/conventions/github-mcp.md` for that carve-out.

## The branch/worktree helper tool

The deterministic string/path work this bootstrap needs —
**tag validation**, **base-repo resolution**,
**branch-name normalization**, and the **two operator-file
symlinks** (`frontend/.env.local` and
`infra/localnet/secrets.local.env`) — lives in the Python
skill-tool `.claude/tools/init_pr_branch.py` (per
`CLAUDE.md` → "Skill tooling"), so the skill drives it
instead of hand-parsing `git worktree list` in prose. Run
it **once** near the top with the resolved tag; it runs
the two read-only git reads itself and prints JSON:

```sh
python3 .claude/tools/init_pr_branch.py --tag <eng-###> --link-env
```

```json
{
  "tag": "eng-603",          // the validated tag, lowercased
  "tag_valid": true,         // false (+ non-zero exit) if not eng-###
  "base_repo": "/…/dropset", // the refs/heads/main worktree, or null
  "current_branch": "worktree-eng-603",
  "normalized_branch": "eng-603",
  "rename_needed": true,     // true iff a `worktree-` prefix is stripped
  "env_link": "created",     // frontend/.env.local
  "secrets_env_link": "exists"  // infra/localnet/secrets.local.env
}
```

Both link fields carry the same five-value vocabulary —
`created` / `exists` / `no-source` / `no-base` / `failed` —
and are reported **separately**, because a machine can
legitimately have one file and not the other.

Steps 1, 2, 3, and 4 read their answers from this one call.

**Why `--link-env` is a flag and not a shell step.** The env
symlink used to be prose here: two existence checks plus an
`ln -s` against an **absolute base-repo path**, which
re-prompted on *every* bootstrap because the file-access
heuristic gates on the absolute path. Folding the step into
the call above means the command line carries **no absolute
path** at all, so there is nothing left to gate. The
enclave file rides the same flag for the same reason, and
adding it cost the command line nothing.

**A note on where allow-rules live, since this skill used to
state it wrongly.** `settings.local.json` is **one shared
file, resolved through worktrees to the main checkout** — a
fresh worktree carries no copy of its own and needs none, and
a rule firmed in any worktree is immediately live in all of
them. So `Bash(python3 .claude/tools/:*)` works fine at
project scope; the old rationale here ("user level is the
only scope a fresh worktree inherits") was **false**. Promote
a rule to `~/.claude/settings.json` when you want it in
**other repos**, which is a different question entirely. See
`docs/conventions/local-integrations.md` → "How settings
files resolve across worktrees".

What a cold worktree genuinely lacks is untracked
per-directory *content* — `frontend/node_modules`,
`frontend/.env.local`, and `infra/localnet/secrets.local.env`
— which is what step 3 handles.

## Steps

1. **Validate the tag.** Take `tag_valid` / `tag` from
   the helper's output. If `tag_valid` is `false` (the
   tool also exits non-zero), stop and ask the user for a
   valid `eng-###` tag. Otherwise use the lowercased
   `tag` from here on.

1. **Fetch the latest `main`.** The point is to start the
   rebase below from current upstream, not from whatever this
   worktree last saw. Do it from **inside this worktree**:

   ```sh
   git fetch origin main
   ```

   That updates the shared `origin/main` ref (worktrees share
   one `.git`), which is what step 5 rebases onto.

   **Don't reach for `git -C <base_repo> pull --ff-only`.**
   Earlier versions of this step did, and in a
   worktree-isolated session it is refused: a worktree
   session's git operations have to target its own worktree,
   so redirecting out of it with `-C` doesn't run. The fetch
   above achieves what this step actually needs without
   leaving the worktree.

   To be precise about the mechanism, since it is easy to
   mis-attribute: the refusal comes from the **harness's own
   worktree isolation**, not from any hook this repo commits.
   The repo's **worktree edit-path guard** is a different
   thing — it covers **file-mutating tools** (`Edit`, `Write`,
   `MultiEdit`, `NotebookEdit`) that target a base-repo
   absolute path, and never inspects `Bash` at all (see
   `docs/conventions/local-integrations.md` → "The worktree
   edit-path guard hook"). Both point the same way; only one
   of them is what stops this command.

   The one thing the fetch does *not* do is fast-forward the
   base repo's checked-out `main` working tree. That matters
   only to whoever is working in the base repo directly, not
   to this bootstrap, so leave it to them. `base_repo` from
   the helper's output is still worth keeping — the two
   symlinks used it, and a `null` value is the same condition
   that reports `"no-base"` for both of them.

1. **Confirm the two operator-file symlinks.** The
   `--link-env` flag on the helper call above **already did
   this** — it symlinks each of the base repo's copies into
   this worktree, so neither has to be copied by hand. Both
   are git-ignored, so neither link is tracked. There is no
   shell to run here; just read the two fields from that one
   JSON result:

   - **`env_link`** — `frontend/.env.local`, so `pnpm dev` /
     `make frontend` pick up the same env.
   - **`secrets_env_link`** —
     `infra/localnet/secrets.local.env`, the local secrets
     enclave's one operator file (the vault name plus one
     `op://` reference per credential). Without it,
     `make fx-collectors-up` in this worktree silently falls
     back to whatever keys happen to be exported. Unlike
     `settings.local.json`, this path is **not** resolved
     through a worktree to the main checkout, so the symlink
     is what gives it that resolution.

   Each field carries the same five values:

   - `"created"` — the link was made.
   - `"exists"` — this worktree already had the path, so it
     was left untouched (it may be a real file someone placed
     deliberately; the tool never clobbers).
   - `"no-source"` — nothing to link: either main has no such
     file, or this worktree has no containing directory to
     link it into.
   - `"no-base"` — main isn't checked out anywhere, so there
     was no base repo to link from (the same condition that
     skipped the pull above).
   - `"failed"` — the link couldn't be created (an unwritable
     parent directory, a read-only mount). Mention it and
     carry on; the file will want copying by hand.

   **Read the two independently.** A machine that has never
   run the frontend has no `.env.local`, and one that has
   never touched the FX collectors has no enclave file;
   neither absence says anything about the other, which is why
   there are two fields rather than one.

   Every outcome is fine to proceed on; none of them blocks
   the bootstrap. The tool never raises here — it reports
   `"failed"` instead, because this one call also carries the
   tag / base-repo / branch answers the next steps read.

   **Note the other cold-worktree prerequisite while you're
   here: `frontend/node_modules`.** A fresh worktree has
   none, and the `biome` and `tsc` hooks shell out to
   `pnpm -C frontend exec …`, so without it they fail with
   `Command "biome" not found` and have to be re-run:

   ```sh
   python3 .claude/tools/run_quiet.py -- pnpm --dir frontend install
   ```

   **Route it through the quiet runner.** A cold install's
   output is nearly all registry `ETIMEDOUT` retries and
   peer-dependency trees — one session's single largest result
   (≈2.0k) for a command whose informative content is "it
   worked". It is the verbose-by-refresh class
   (`docs/conventions/context-economy.md`), and the runner
   prints one line on success and the failing tail otherwise.

   **Spell it `--dir`, not `-C`** — for consistency with
   `review-pr`'s lint step, which prescribes the same form.
   Note the permission-matcher argument for it does **not**
   apply to this call any more: wrapped in the quiet runner,
   the matcher sees the `python3 .claude/tools/` prefix, so
   the two spellings are indistinguishable to it here. The
   `--dir`/`-C` distinction still matters for an *unwrapped*
   `pnpm` call, which is why the rule stands elsewhere.

   **This is not only a frontend-task concern.** `make lint`
   runs the whole hook set over the tree, and those two are
   typed on `ts` / `tsx` / `js` / `css` — which the repo has
   plenty of regardless of what *this* branch touches. So the
   first full `make lint` in a cold worktree fails on them
   whatever the task is. Install when the surfaced task
   touches `frontend/**`, or before the first full lint;
   either way it is one command paid once per worktree, and
   the alternative is a wasted lint round-trip plus a
   diagnosis of an error that says nothing about the diff.
   (`review-pr`'s lint step covers the recovery, but
   recovering is the expensive path.)

1. Normalize the branch name to the bare Linear tag.
   The `aps` shell helper starts worktree sessions with
   `claude -w <tag>`, which names the worktree directory
   `eng-###` but the **branch** `worktree-eng-###` —
   there's no CLI flag to drop the `worktree-` prefix, so
   the skill strips it here rather than leaving each
   session to rename it by hand. The helper already
   computed this: read `rename_needed`, `current_branch`,
   and `normalized_branch` from its output.

   - If `rename_needed` is `true`, rename the branch to
     the bare `eng-###` — pass both names literally so the
     call reduces to a stable allow-rule:

     ```sh
     git branch -m <current_branch> <normalized_branch>
     ```

   - If `rename_needed` is `false` (the branch is already
     `eng-###`, or any other non-`worktree-` name), this
     is a **no-op** — leave it alone. Only the
     `worktree-`-prefixed default is rewritten.

1. Rebase onto the freshly-fetched upstream main so the
   worktree starts from the latest code:

   ```sh
   git rebase origin/main
   ```

   **`origin/main`, not the local `main`.** The step-2 fetch
   updates the remote-tracking ref; the local `main` branch is
   only fast-forwarded by whoever has it checked out, so
   rebasing onto it can silently start from stale code.

   If the rebase produces conflicts, abort it
   (`git rebase --abort`) and tell the user.
   Do not attempt to resolve conflicts
   automatically in this skill.

1. Create an empty, **signed** commit so there is
   something to push. Its message is a **conforming
   semantic subject**, not the bare tag:

   ```sh
   git commit --allow-empty -S -m "chore(<ENG-###>): Bootstrap the worktree"
   ```

   The `-S` is mandatory: branch protection on
   this repo requires every commit to have a
   verified signature.

   **Why the message conforms.** The next-but-one step
   opens the PR with this exact string as the title, and
   the Semantic PR workflow validates *both* — see the
   rationale there. Keep the two byte-identical: with a
   single commit on the branch, the workflow's
   `validateSingleCommitMatchesPrTitle` compares them and
   fails on any divergence.

1. Push the branch:

   ```sh
   git push -u origin <eng-###>
   ```

1. Create a draft PR with an empty body, via the GitHub
   MCP, titled with the **same string as the bootstrap
   commit** — not the bare tag. This repo is
   `DASMAC-com/dropset`, so pass `owner: "DASMAC-com"`,
   `repo: "dropset"`; the head is the branch you just
   pushed and the base is `main`:

   ```txt
   mcp__github__create_pull_request(
     owner: "DASMAC-com",
     repo: "dropset",
     title: "chore(<ENG-###>): Bootstrap the worktree",
     head: "<eng-###>",
     base: "main",
     body: "",
     draft: true,
   )
   ```

   **Why not the bare tag, which is what this step used
   to pass.** `ENG-###` alone cannot satisfy the Semantic
   PR workflow — it requires a type and a scope
   (`scopes: ^ENG-[0-9]+$`) and a capitalized subject — so
   the `opened`-triggered run **always failed**, every
   time, on every PR this skill has ever created. Nothing
   merged unguarded (GitHub evaluates a required check
   against the *latest* run per context, and
   `pr-title-description` renames the title during review,
   whose `edited` trigger reruns and passes), but the
   failed run stays in the rollup — `gh pr checks`, the UI
   checks list — forever. On PR #329 that residue read as
   a semantic-check bypass and cost an operator
   investigation. The merge-queue leg is a deliberate
   auto-pass (a merge group cannot see a PR title), so the
   PR-path run is the *only* real enforcement, and it
   should never carry guaranteed-failure noise.

   A conforming seed costs nothing: `pr-title-description`
   still rewrites the final title during review exactly as
   before, so this changes only what the title is *between*
   open and review. `chore` is the honest type for an empty
   bootstrap commit, and it is in the action's default type
   set.

   The call returns the PR object, including its
   `html_url` and `number` — keep both (the number for
   the next step, the URL for the final one).

1. **Unsubscribe from this PR's notifications** so its
   lifecycle doesn't ping the author. Opening a PR
   auto-subscribes you to it, and the draft then generates a
   stream of notifications through its life (CI results,
   assignment, and finally the merge) — noise in a
   solo / agent-driven flow. Unsubscribe right after
   creating it. No GitHub MCP tool covers a per-PR
   subscription (`manage_notification_subscription` needs an
   existing thread; `manage_repository_notification_subscription`
   is repo-wide), so this is a **documented `gh` exception**
   (per `docs/conventions/github-mcp.md`). The working path is
   the GraphQL `updateSubscription` mutation, keyed by the
   PR's GraphQL **node id**.

   `create_pull_request` returns the PR's *numeric* database
   id, **not** the node id the mutation needs, so first
   resolve the node id from the `number` kept above — gh's
   `id` field over its GraphQL is the node id, and this reuses
   the existing `Bash(gh pr view:*)` allow-rule:

   ```sh
   gh pr view <number> --repo DASMAC-com/dropset --json id
   ```

   Then set the subscription to `IGNORED` with the node id
   (`<node_id>`, e.g. `PR_kwDO…`) — this reuses the existing
   `Bash(gh api graphql:*)` allow-rule:

   ```sh
   gh api graphql -F id=<node_id> -f query='
     mutation($id: ID!) {
       updateSubscription(
         input: { subscribableId: $id, state: IGNORED }
       ) { subscribable { viewerSubscription } }
     }'
   ```

   A success returns `viewerSubscription: "UNSUBSCRIBED"` —
   GitHub normalizes the `IGNORED` readback to `UNSUBSCRIBED`,
   which is what stops the lifecycle self-pings. The mutation
   needs the `gh` token's **`notifications`** OAuth scope; if
   it's missing the call fails with `INSUFFICIENT_SCOPES` (a
   one-time operator grant:
   `gh auth refresh -h github.com -s notifications`). Step 0b's
   pre-check already read the scope set, so if it flagged
   `notifications` as absent, expect this to fail and say so
   rather than re-diagnosing it here.

   Make it **best-effort**: if either call errors, note it and
   continue — a notification ping must never block
   bootstrapping. (`housekeeping`'s merged-PR notification
   sweep remains the catch-all for anything this misses.)
   **Tradeoff:** unsubscribing suppresses this PR's routine
   lifecycle notifications; a direct @-mention or an explicit
   review request can still re-notify — accepted in this
   solo / agent-driven flow.

1. Mark the Linear issue **In Progress** so the board
   reflects that work on this worktree has started.
   Update it by identifier (the uppercase tag) via the
   `claude.ai Linear` MCP:

   ```txt
   mcp__claude_ai_Linear__save_issue(
     id: "<ENG-###>",
     state: "In Progress"
   )
   ```

   If the issue doesn't exist or the update fails, warn
   and continue — bootstrapping shouldn't be blocked by
   Linear.

   **Keep this response — it carries the whole issue
   body.** `save_issue` echoes the complete `description`
   back even on a state-only write that sent no body at
   all, so the payload the next step needs has already
   been bought. Re-`get_issue`ing it there pays for the
   same body twice: two ≈1.1k echoes for one payload in
   one measured run, and far worse on a consolidated spec.
   The echo is a fixed cost per call and `patch` does not
   shrink it, so fewer calls is the only lever (see
   `docs/conventions/linear-automation.md`).

1. Print the new PR URL and confirm the Linear issue was
   moved to In Progress.

1. **Surface the task when no other context was
   given.** If the session was started with no
   instructions beyond the tag, the linked Linear
   issue *is* the spec.

   **Read the description out of the previous step's
   response — don't re-fetch it.** The In-Progress write
   already returned the full body; a `get_issue` here is a
   second echo of a payload the session is holding. Only
   reach for `get_issue` if that write failed or was
   skipped. Do still pull
   `mcp__claude_ai_Linear__list_comments` — acceptance
   criteria sometimes live in an anchored comment, not the
   body, and that is a payload the session does *not*
   already have.

   Present the description and any checklist as the plan
   of work so the session can proceed straight into the
   task. If the user provided their own instructions,
   those win; don't override them with the issue.

   **Treat the issue's `file:line` citations as a snapshot
   of its discovery commit.** A filed issue records where
   something was when it was *found*, which may be months
   of unrelated PRs ago — so verify each citation against
   `HEAD` before implementing it. One session's surfaced
   task named four `(§4 row 5)` citations across two files;
   **all four** had since been rewritten away by an
   unrelated PR, one named file had no relevant references
   at all, and the work item turned out to be moot —
   established at the cost of four exploratory greps
   including a ≈1.1k repo-wide sweep. The stable key is the
   `**Fingerprint**` slug, never the line number: find the
   thing the fingerprint names, then re-derive its
   location.

   The same goes for a spec's own claims about what has
   shipped. A long-lived consolidated issue often carries
   "this part already landed in PR #N" notes; those are
   reliable, but a part filed *before* an unrelated change
   may describe code that no longer exists.

1. **Hand off to `/review-pr` when the work is ready.**
   This is the closing step of the bracketed session.
   Once the surfaced task's work is complete and every
   design decision and open question has been resolved
   (each asked through `AskUserQuestion`, per "Decision
   points use `AskUserQuestion`" above), announce that
   the work is ready and ask — again **via
   `AskUserQuestion`** — whether to run `/review-pr` now.
   Offer two options: "yes, run /review-pr" (**first**,
   the recommended default) and "not yet".

   - On **yes**, route straight into `/review-pr`.
   - On **not yet**, stop and leave the PR as it is.

   **Do not write a "delivered" narrative onto the Linear
   issue at this point.** It is tempting — the work reads as
   finished — but the adversarial pass has not run yet, and
   on one measured run it *invalidated* the summary that had
   already been written (the route memoization the note
   described was removed as a blocking bug), forcing a
   second corrections append. Two full-body echoes for one
   story. The disposition is `review-pr`'s to record after
   its fan-out, when it is actually known; leave the issue
   alone here beyond the In-Progress transition in the
   earlier step.

   Do **not** surface `/pr-title-description` as its own
   step in this flow: `review-pr` already **calls** it
   for the final title and body (its steps 13–14), so
   offering it here would be redundant noise. The two
   user-facing skills are `/init-pr` then `/review-pr`;
   `pr-title-description` is a helper `review-pr` drives,
   not a freestanding stage.
