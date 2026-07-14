---
name: init-pr
description: Bootstrap a worktree — pull main, set up the branch, push a draft PR, and warm CI caches.
disable-model-invocation: true
user-invocable: true
---

# `init-pr`

Bootstrap the current worktree: pull main in the
base repo, set up the branch, push a draft PR so
CI caches start warming while work continues.

This is the first skill an agent should run after
`claude --worktree <tag>` starts.

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

## The branch/worktree helper tool

The deterministic string/path work this bootstrap needs —
**tag validation**, **base-repo resolution**, and
**branch-name normalization** — lives in the Python
skill-tool `.claude/tools/init_pr_branch.py` (per
`CLAUDE.md` → "Skill tooling"), so the skill drives it
instead of hand-parsing `git worktree list` in prose. Run
it **once** near the top with the resolved tag; it runs
the two read-only git reads itself and prints JSON:

```sh
python3 .claude/tools/init_pr_branch.py --tag <eng-###>
```

```json
{
  "tag": "eng-603",          // the validated tag, lowercased
  "tag_valid": true,         // false (+ non-zero exit) if not eng-###
  "base_repo": "/…/dropset", // the refs/heads/main worktree, or null
  "current_branch": "worktree-eng-603",
  "normalized_branch": "eng-603",
  "rename_needed": true      // true iff a `worktree-` prefix is stripped
}
```

Steps 1, 2, and 4 read their answers from this one call.

## Steps

1. **Validate the tag.** Take `tag_valid` / `tag` from
   the helper's output. If `tag_valid` is `false` (the
   tool also exits non-zero), stop and ask the user for a
   valid `eng-###` tag. Otherwise use the lowercased
   `tag` from here on.

1. **Pull main in the base repository** (not this
   worktree). Take `base_repo` from the helper's output —
   it is the worktree whose branch is `refs/heads/main`.
   If `base_repo` is `null`, no worktree has `main`
   checked out: skip the pull and warn the user.
   Otherwise pull, passing the path inline so the call
   reduces to a stable allow-rule (no `$(…)`):

   ```sh
   git -C <base_repo> pull --ff-only
   ```

1. Symlink `frontend/.env.local` from the main
   worktree so `pnpm dev` / `make frontend` pick up
   the same env without a manual copy. `.env*` is
   in `frontend/.gitignore`, so the symlink isn't
   tracked. Skip if main has no env file, or if
   this worktree already has one (don't clobber a
   real file someone placed deliberately).

   Do the existence checks with the **Glob/Read
   tools**, not a shell `test`/`if`. A
   `test … && … || …` compound never reduces to an
   allow-rule and re-prompts every run:

   - Glob `frontend/.env.local` in **this** worktree.
     If it matches, a file already exists — skip.
   - Glob (or Read) `frontend/.env.local` under the
     base repo (`base_repo` from the helper). If it
     doesn't exist, main has no env file — skip and move
     on.

   **If Glob is unavailable this session** (the harness
   sometimes reports "No such tool available: Glob"), fall
   back to a **bare `find <path>`** — one path per call, and
   crucially **no `2>/dev/null` redirect**: the redirect
   trips the `no_compound_bash` guard and burns blocked
   attempts (a bare `find <missing-path>` already prints its
   own "No such file" to stderr and exits non-zero, which is
   the signal you want). A `Read` of the path works too — a
   read error means "doesn't exist." Don't reach for
   `test`/`if` or a redirected `find` as the fallback.

   Only when this worktree has none and main has one,
   create the link (the bare `ln` matches an existing
   allow-rule, so it won't prompt):

   ```sh
   ln -s <base_repo>/frontend/.env.local frontend/.env.local
   ```

   If main isn't checked out anywhere (previous
   step), skip this one too.

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

1. Rebase onto the freshly-pulled main so the
   worktree starts from the latest code:

   ```sh
   git rebase main
   ```

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
   `gh auth refresh -h github.com -s notifications`).

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

1. Print the new PR URL and confirm the Linear issue was
   moved to In Progress.

1. **Surface the task when no other context was
   given.** If the session was started with no
   instructions beyond the tag, the linked Linear
   issue *is* the spec. Fetch it with
   `mcp__claude_ai_Linear__get_issue` (id = the
   uppercase tag) and pull
   `mcp__claude_ai_Linear__list_comments` too —
   acceptance criteria sometimes live in an anchored
   comment, not the body. Present its description and
   any checklist as the plan of work so the session
   can proceed straight into the task. If the user
   provided their own instructions, those win; don't
   override them with the issue.

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
