<!-- cspell:word Toolsets -->

# GitHub via MCP

All GitHub operations — opening PRs, updating titles and bodies,
reading the diff, watching checks, pulling failing-job logs — go
through the **GitHub MCP server** (`mcp__github__*`), not the `gh`
CLI, **with the deliberate exceptions below**. The skills (`init-pr`,
`pr-title-description`, `review-pr`, `housekeeping`, `linear-task`)
are written against it. `gh` survives in four places — two in
`review-pr`, one in `init-pr`, one in `pr-title-description`:

- **The merge-queue handoff** — the enqueue (a `gh pr merge --auto`
  write, **no** strategy flag: this repo's merge queue sets the
  strategy, so a `--squash` only warns) and a read-only dequeue probe
  (a `gh api graphql … mergeQueueEntry` read). The enqueue stays on
  `gh` because the server exposes no auto-merge / merge-queue tool
  (`merge_pull_request` does an *immediate* merge, which bypasses the
  queue); the probe stays on `gh` because the hosted MCP's
  `pull_request_read` omits the merge-queue state — and on a
  merge-queue repo a still-queued PR reports `autoMergeRequest: null`,
  so the probe must read `mergeQueueEntry` (non-null while queued)
  over GraphQL to tell a still-queued PR from one that was dequeued.

- **The CI-wait and PR-state reads** — `gh pr checks <number>` for the
  CI-wait poll, and `gh pr view <number> --json <fields>` for the
  one-shot `mergeable` / PR-lookup reads. These reads are **polled
  repeatedly** across the CI and merge-queue waits, and the MCP
  equivalents (`pull_request_read` `get` / `get_check_runs`) return
  the **full** PR object or check array on every poll — a fat payload
  that, because a tool result is replayed as input on every later
  turn (see [context economy](context-economy.md)), is paid many times
  over. `gh pr checks` is one compact line per check, and
  `--json <fields>` selects only the fields the decision needs.
  `--json` / `--jq` are command **flags**, not shell pipes, so they
  stay shell-rule-clean and reduce to `Bash(gh pr checks:*)` /
  `Bash(gh pr view:*)` allow-rules. This is the one place a `gh` read
  is preferred *over* the MCP: when the call repeats and the payload —
  not the transport — is the cost. Keep the poll **model-driven** (a
  fresh call paced by `ScheduleWakeup`), never a shell `while … sleep`
  loop or a `jq` filter; the failure path still pulls logs via
  `get_job_logs`.

- **Ignoring a new PR's notification subscription** (`init-pr`) — a
  one-shot `gh api --method PUT` against the PR's `/subscription`
  endpoint with `-F ignored=true`, right after the draft PR is created,
  so its lifecycle (CI, assignment, merge) doesn't ping the author in
  this solo / agent-driven flow. No MCP tool covers a per-PR
  subscription — `manage_notification_subscription` needs an existing
  thread and `manage_repository_notification_subscription` is repo-wide
  — so the `gh api` write is the only path. It's **best-effort**:
  `init-pr` continues if it errors, and `housekeeping`'s merged-PR
  notification sweep is the catch-all for anything it misses. It reduces
  to a narrow allow-rule (`firm-perms` can memorialize it):

  ```text
  Bash(gh api --method PUT /repos/DASMAC-com/dropset/issues/*/subscription:*)
  ```

- **The recent-merged-PR style lookup** (`pr-title-description`) — a
  one-shot `gh pr list --json number,title,body --state merged --limit 3`
  to read the last few merged PRs' bodies for style. `gh` has a
  `merged` state filter the MCP `list_pull_requests` lacks **and**
  returns the `body` in the same call, so it replaces a list-every-closed-PR
  call (~104k tokens of full bodies, replayed every later turn — see
  [context economy](context-economy.md)) *plus* a per-PR
  `pull_request_read` body fetch with one field-selected read. `--json`
  is a flag, not a pipe, so it reduces to a `Bash(gh pr list:*)`
  allow-rule (a routine, low-blast-radius read).

Everything else stays MCP-first; `gh` is not a general-purpose escape
hatch.

Every tool takes `owner` and `repo`. This repo is
`DASMAC-com/dropset`, so pass `owner: "DASMAC-com"`, `repo: "dropset"`
on every call. The server collapses most reads into a single tool
dispatched by a `method` enum — `pull_request_read` covers `get` /
`get_diff` / `get_files` / `get_check_runs` / …, and `actions_list` /
`actions_get` do the same for Actions — so one tool name covers many
reads.

## Authentication (PAT header, not OAuth)

The server is added at **user scope** with a PAT in an `Authorization`
header, on the same env-var convention as the `LINEAR_*` ids — the
token lives in `~/.zshrc`, never in a committed file or
`~/.claude.json`:

```sh
export GITHUB_MCP_PAT=…
```

```sh
claude mcp add --transport http --scope user github \
  https://api.githubcopilot.com/mcp/ \
  --header 'Authorization: Bearer ${GITHUB_MCP_PAT}' \
  --header 'X-MCP-Toolsets: all'
```

Two gotchas, both learned the hard way:

- **OAuth doesn't work.** Claude Code's built-in OAuth needs dynamic
  client registration, which this server doesn't support
  (`does not support dynamic client registration`). The PAT header is
  the only path; a classic `repo` token already covers PRs and Actions
  read+write, so nothing extra is needed.
- **A newly added or reconfigured server loads on the next
  conversation, not mid-session.** After `claude mcp add` (or any
  header change), relaunch and start a fresh chat before the
  `mcp__github__*` tools appear.

The `X-MCP-Toolsets: all` header exposes the `actions` toolset (check
runs, job logs) alongside the defaults. The tradeoff: it also surfaces
write tools across every toolset (Dependabot, secret-scanning,
notifications, …); per-tool permission prompts are the backstop.

## Permission rules

Pre-approve the **reads** *and* the routine **PR-authoring writes** so
they don't re-prompt, and leave the genuinely destructive / irreversible
writes to confirm-on-use:

- **Pre-approve (reads):** `pull_request_read`, `list_pull_requests`,
  `actions_list`, `actions_get`, `get_job_logs`, `get_me`, and the
  `search_*` family.
- **Pre-approve (the companion `gh` reads, as `Bash(…)` rules):**
  `Bash(gh pr checks:*)`, `Bash(gh pr view:*)`,
  `Bash(gh api graphql:*)`, and `Bash(gh pr list:*)` — the polled /
  field-selected reads
  `review-pr` and `pr-title-description` use in place of the
  full-object MCP calls (see "GitHub
  via MCP" above and [context economy](context-economy.md)). These are
  Bash globs, not `mcp__github__*` entries, but they're pre-approved on
  the same rationale (routine, low-blast-radius reads) and propagated to
  the base repo so future worktrees inherit them.
- **Pre-approve (routine PR-authoring writes):** `create_pull_request`
  (init-pr) and `update_pull_request` (pr-title-description, review-pr),
  plus `init-pr`'s notification-subscription ignore (the narrow
  `gh api … /subscription` allow-rule shown above — the one `gh` write
  of the four exceptions). The skills call these on every run to open
  and maintain the draft PR,
  and they touch only the PR's own title / body / draft-state /
  subscription — low
  blast radius — so gating them behind a confirm prompt each run buys no
  safety. Pre-approving them is deliberate.
- **Confirm-on-use (merges, deletes, pushes, issue/actions
  mutations):** `merge_pull_request`, `delete_file`, `push_files`,
  `create_or_update_file`, `issue_write`, `actions_run_trigger`. These
  either land code, delete content, or mutate issues/workflows — the
  irreversible or far-reaching writes that warrant a per-use confirm.

The split, in one line: **pre-approve reads + the routine PR-authoring
writes; confirm-on-use for merges, deletes, pushes, and issue/actions
mutations.**

The MCP entries are `mcp__github__<tool>` permission strings, not
`Bash(…)`
globs — and because of the single-tool-many-methods shape, one
allow-rule per read tool covers all of its methods. Propagate the
pre-approved allow-rules (the reads, the companion `gh` reads, and the
routine PR-authoring writes incl. the notification-subscription ignore)
to the **base-repo** settings so future worktrees inherit them (per the
per-worktree settings rule); `firm-perms` does this at session end.
