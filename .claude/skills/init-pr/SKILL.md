---
name: init-pr
description: Bootstrap a worktree — pre-check the session's model tier and the gh credential first, then fetch main, set up the branch, push a draft PR, and warm CI caches.
disable-model-invocation: true
user-invocable: true
---

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
- **Map the structure before any Read over ~300 lines.** One
  Grep for `^fn |^impl |^pub` (or the language's equivalent)
  gives you the section map, and the map tells you which
  slice you actually want. A dispatcher whole-file Read
  (≈4.4k) to find **one** append point is the recurring shape
  this prevents.
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
- **Read whole only when you will BOTH edit the file and
  brief agents on it.** This is the one case where the whole
  file is the cheaper choice overall, and the plain "never
  read whole" reading gets it wrong. One session's top five
  main-loop results were whole-file Reads (≈23k) of the crate
  it was about to modify — and those same excerpts were then
  pasted into all five lens briefs, which is what kept every
  lens under its turn cap. Paid once, amortized five times.
  Absent that second use, slice.
- **Route `cargo` / `make` through the quiet runner.** Run
  `cargo test` / `cargo check` / `make …` through
  `python3 .claude/tools/run_quiet.py -- <cmd>` **during
  implementation**, not only during `review-pr` — an
  unwrapped `cargo test` lands its whole `Compiling …`
  cascade in context for a result that is one line.
- **Search source with the tool, not a bare recursive grep.**
  `python3 .claude/tools/search_source.py '<pattern>'` already
  prunes the generated families and the never-search trees
  (`target/` alone is multi-GB and `grep -r` does not honor
  gitignore), and it reduces to one stable allow-rule however
  the pattern and filters vary.

## The branch/worktree helper tool

The deterministic string/path work this bootstrap needs —
**tag validation**, **base-repo resolution**,
**branch-name normalization**, and the
**`frontend/.env.local` symlink** — lives in the Python
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
  "env_link": "created"      // created|exists|no-source|no-base|failed
}
```

Steps 1, 2, 3, and 4 read their answers from this one call.

**Why `--link-env` is a flag and not a shell step.** The env
symlink used to be prose here: two existence checks plus an
`ln -s` against an **absolute base-repo path**. That
re-prompted on *every* bootstrap, and firming it can't help —
the allow-rule lands in the new worktree's
`settings.local.json`, and every `/init-pr` runs in a
brand-new worktree that has none (`.claude/settings.json` is
deliberately gitignored). Folding the step into the call
above means the command line carries **no absolute path** for
the file-access heuristic to gate. For the same reason
`Bash(python3 .claude/tools/:*)` belongs in
**`~/.claude/settings.json`** — user level is the only scope a
fresh worktree inherits.

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
   the helper's output is still worth keeping — the env
   symlink used it, and a `null` value is the same condition
   that reports `env_link: "no-base"`.

1. **Confirm the `frontend/.env.local` symlink.** The
   `--link-env` flag on the helper call above **already did
   this** — it symlinks the base repo's env file into this
   worktree so `pnpm dev` / `make frontend` pick up the same
   env without a manual copy (`.env*` is in
   `frontend/.gitignore`, so the link isn't tracked). There
   is no shell to run here; just read `env_link` from that
   one JSON result:

   - `"created"` — the link was made.
   - `"exists"` — this worktree already had the path, so it
     was left untouched (it may be a real file someone placed
     deliberately; the tool never clobbers).
   - `"no-source"` — nothing to link: either main has no env
     file, or this worktree has no `frontend/` directory to
     link it into.
   - `"no-base"` — main isn't checked out anywhere, so there
     was no base repo to link from (the same condition that
     skipped the pull above).
   - `"failed"` — the link couldn't be created (an unwritable
     `frontend/`, a read-only mount). Mention it and carry
     on; `pnpm dev` will want the env file copied by hand.

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
   pnpm --dir frontend install
   ```

   **Spell it `--dir`, not `-C`.** The two are synonyms to
   pnpm but *not* to the permission matcher: `review-pr`'s
   lint step already prescribes the `--dir` form and it is
   firmed, so a `-C` here would be a second rule for the same
   command — and a fresh prompt in every cold worktree, which
   is exactly the population this step exists to serve.

   **This is not only a frontend-task concern.** `make lint`
   runs `pre-commit --all-files`, and those two hooks are
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
   something to push:

   ```sh
   git commit --allow-empty -S -m "<ENG-###>"
   ```

   The `-S` is mandatory: branch protection on
   this repo requires every commit to have a
   verified signature.

1. Push the branch:

   ```sh
   git push -u origin <eng-###>
   ```

1. Create a draft PR with the Linear tag as the
   title and an empty body, via the GitHub MCP. This
   repo is `DASMAC-com/dropset`, so pass
   `owner: "DASMAC-com"`, `repo: "dropset"`; the head
   is the branch you just pushed and the base is `main`:

   ```txt
   mcp__github__create_pull_request(
     owner: "DASMAC-com",
     repo: "dropset",
     title: "<ENG-###>",
     head: "<eng-###>",
     base: "main",
     body: "",
     draft: true,
   )
   ```

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

   Do **not** surface `/pr-title-description` as its own
   step in this flow: `review-pr` already **calls** it
   for the final title and body (its steps 13–14), so
   offering it here would be redundant noise. The two
   user-facing skills are `/init-pr` then `/review-pr`;
   `pr-title-description` is a helper `review-pr` drives,
   not a freestanding stage.
