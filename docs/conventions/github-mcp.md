<!-- cspell:word Toolsets -->

# GitHub via MCP

All GitHub operations — opening PRs, updating titles and bodies,
reading the diff, watching checks, pulling failing-job logs — go
through the **GitHub MCP server** (`mcp__github__*`), not the `gh`
CLI, **with the deliberate exceptions below**. The skills (`init-pr`,
`pr-title-description`, `review-pr`, `housekeeping`, `linear-task`)
are written against it. `gh` survives in four places — two in
`review-pr`, one in `init-pr`, and the field-selected `gh pr list`
read used by `housekeeping`:

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

  **The enqueue prints nothing under this harness.** `gh` emits its
  "Merge when ready" confirmation only to a TTY, and stdout here is
  not one, so a successful `gh pr merge --auto` returns exit 0 and no
  output whatsoever. Confirm from the exit status; there is no reply
  text to read, and a run has misread the silence as a failure.

  **An all-null probe is ambiguous, and `gh run list` disambiguates
  it.** `mergeQueueEntry` and `autoMergeRequest` are *both* null during
  the registration window **and** after a dequeue, so the probe alone
  cannot tell a PR that has not registered yet from one that was kicked
  out. The queue branch settles it: a completed run set on
  `pr-<number>-…` proves the entry existed. Check that **before**
  concluding, not after — the ordering is spelled out in `review-pr`'s
  merge-queue outcome step.

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

- **The post-merge notification lookup** (`review-pr`) — reading the
  one thread id whose `subject.url` ends in a PR's number, so the
  merged PR's own notification can be dismissed. Same reasoning as the
  reads above, and it was simply missed when that carve-out was
  written: `mcp__github__list_notifications` returned **3.7k tokens in
  a single call** for that one id — the 6th-largest result of one
  session — because it embeds the complete repository object (every
  `*_url` template) once per notification, repeated per notification.
  A field-selected read is the fix:

  ```text
  gh api /notifications --jq '.[] | {id, url: .subject.url}'
  ```

  The **dismissal itself stays on the MCP**
  (`dismiss_notification`) — it is a small write with no payload
  problem, and only the *read* is the expensive half. Whichever
  transport is used, the list is a fixed cost to be **read once**:
  never re-fetch it to re-find the id.

- **Unsubscribing from a new PR's notifications** (`init-pr`) — right
  after the draft PR is created, so its lifecycle (CI, assignment,
  merge) doesn't ping the author in this solo / agent-driven flow. No
  MCP tool covers a per-PR subscription —
  `manage_notification_subscription` needs an existing thread and
  `manage_repository_notification_subscription` is repo-wide — so `gh`
  is the only path. The **REST** `PUT …/issues/{n}/subscription` route
  that reads plausible for this **does not exist** (it `404`s on every
  PR, which a missing `notifications` scope also masks as `404`); the
  working path is the GraphQL `updateSubscription` mutation, keyed by
  the PR's GraphQL **node id**:

  ```text
  gh api graphql -F id=<node_id> -f query='
    mutation($id: ID!) {
      updateSubscription(
        input: { subscribableId: $id, state: IGNORED }
      ) { subscribable { viewerSubscription } }
    }'
  ```

  The MCP `create_pull_request` returns the PR's *numeric* database id,
  not the node id, so `init-pr` first resolves the node id with
  `gh pr view <number> --json id` (gh's `id` field over its GraphQL
  *is* the node id). A success returns
  `viewerSubscription: "UNSUBSCRIBED"` (GitHub normalizes the `IGNORED`
  readback), which is what stops the lifecycle self-pings. The mutation
  needs the `gh` token's **`notifications`** OAuth scope — a one-time
  operator grant
  (`gh auth refresh -h github.com -s notifications`); without it the
  call fails with `INSUFFICIENT_SCOPES`. It's **best-effort**:
  `init-pr` continues if it errors, and `housekeeping`'s merged-PR
  notification sweep is the catch-all for anything it misses. Both
  commands reuse allow-rules already listed below —
  `Bash(gh pr view:*)` for the node-id lookup and
  `Bash(gh api graphql:*)` for the mutation — so no new permission is
  needed.

- **The field-selected `gh pr list --json` read** (`housekeeping`) —
  `gh` has a `merged` state filter the MCP `list_pull_requests` lacks
  **and** selects only the fields the decision needs, so one
  field-selected read replaces a list-every-closed-PR call (~104k
  tokens of full bodies, replayed every later turn — see
  [context economy](context-economy.md)) *plus* the per-PR body
  fetches it would otherwise need. `housekeeping`'s worktree-prune uses
  `gh pr list --state merged` selecting only
  `number,headRefName,mergedAt` (with `--limit 100`) **once** to learn
  which worktree branches merged, in place of one full-body
  `list_pull_requests` per branch. `--json` is a flag, not a pipe, so
  it reduces to the `Bash(gh pr list:*)` allow-rule (a routine,
  low-blast-radius read). (`pr-title-description` no longer does a
  style lookup at all — its title and body formats are standardized in
  the skill, so it makes no `gh pr list` read.)

- **A single-scalar commit or metadata lookup, field-selected through
  `gh api --jq`.** When the answer is one field, an MCP getter that
  returns the whole object is the wrong transport by two orders of
  magnitude. Measured twice, on different calls: `get_commit` cost ~1.3k
  to answer one boolean about signature verification, and the same
  question through `gh api --jq` cost roughly five tokens;
  `get_latest_release` returned **60,413 characters** — release payloads
  embed every asset object — overflowed the tool-result cap, and had to
  be redone. So for a version, a tag, a SHA, a timestamp or a single
  boolean, use the field-selected form:

  ```sh
  gh api repos/DASMAC-com/dropset/commits/<sha> \
    --jq '.commit.verification.verified'
  ```

  This is a **narrowness** exception, not a capability one: the MCP call
  would work, it just costs the whole object. Reach for the MCP getter
  when you genuinely want several fields. `--jq` is a flag rather than a
  pipe, so this stays inside the shell rules and reduces to a
  `Bash(gh api:*)` allow-rule.

**Field-selecting is necessary but not sufficient: a
collection-valued field reintroduces the payload.** The rules above
read as "name your fields and the cost goes away", and that is false for
any field which is itself a per-item list — `files`, `commits`,
`comments`, `reviews`. Selecting one of those across N items multiplies
rather than narrows. Measured on both transports: a
`gh pr list --json number,files` paid **~4.0k** for a two-line answer
across eleven PRs, and the MCP file-list method **with minimal output
requested** still returned **81,582 characters**.

So when you field-select, ask what shape each field returns:

- A **scalar** field (`number`, `mergeable`, `headRefName`, a SHA, a
  timestamp) is the cheap case these rules are written for.
- A **collection** field is not. If you need it, either bound it
  (one PR rather than a list) or compute over it **inside a tool** and
  return only the conclusion — which is what `review_diff.py --overlap`
  does for the "which open PRs touch my files" question: it fetches the
  per-PR file lists in its own process and prints only the
  intersection.

The one carve-out worth naming: a **path-only** file list for a single
PR is small and legitimately useful (`--json files --jq '.files[].path'`
for one PR), because the per-file additions/deletions/patch objects are
what make the full form expensive.

**A `gh api` log fetch needs the escape-sequences flag, and silence
here is dangerous.** `gh api` refuses a response carrying terminal
escape sequences unless the allow flag is passed, and Actions job logs
are colorized — so every log fetch fails with **empty stdout**. A
scanner that treats a non-zero exit or an empty body as "no matches"
then silently measures nothing: one 38-job scan reported **zero
matches having inspected zero logs**, and nearly became a wrong review
finding. Two rules follow, and the second is the general one:

- Pass the flag when fetching a job log through `gh api`. (The path
  that works without special-casing is `gh run view --log-failed`
  wrapped in `run_quiet.py`, then `Grep` the captured log — see
  `review-pr`'s CI-failure branch.)
- **Never fold a transport failure into a zero count.** A log scanner
  must distinguish *fetch failed* from *fetched and found nothing*, and
  report the first as an error rather than a result. This applies to any
  scan-and-count over a fallible fetch, not only to logs.

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
header, read from `GITHUB_MCP_PAT` — never a committed file or
`~/.claude.json`. Unlike the plain `LINEAR_*` ids, the token is a
**secret**, so it is not written into `~/.zshrc` either: a
`_ds_secrets` helper resolves it from 1Password at session launch (see
[local-integrations](local-integrations.md) → "Session secrets"), and
the registration below only references the variable:

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
  `review-pr`, `pr-title-description`, and `housekeeping` use in place
  of the full-object MCP calls (see "GitHub
  via MCP" above and [context economy](context-economy.md)).
  `init-pr`'s notification unsubscribe also rides on these two —
  `Bash(gh pr view:*)` for the node-id lookup and
  `Bash(gh api graphql:*)` for the `updateSubscription` mutation (the
  one write on the `gh api graphql` rule) — so it needs no allow-rule
  of its own. These are
  Bash globs, not `mcp__github__*` entries, but they're pre-approved on
  the same rationale (routine, low-blast-radius calls).
- **Pre-approve (routine PR-authoring writes):** `create_pull_request`
  (init-pr) and `update_pull_request` (pr-title-description, review-pr).
  The skills call these on every run to open
  and maintain the draft PR,
  and they touch only the PR's own title / body / draft-state — low
  blast radius — so gating them behind a confirm prompt each run buys no
  safety. Pre-approving them is deliberate. (`init-pr`'s notification
  unsubscribe is also a routine write, but it's a GraphQL mutation
  covered by the `Bash(gh api graphql:*)` companion rule above, not a
  dedicated allow-rule.)
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
allow-rule per read tool covers all of its methods. They go in
`settings.local.json` like any other rule, which is **one shared file
resolved through worktrees to the main checkout** — so firming them
once makes them live in every worktree, with nothing to propagate
(see `local-integrations.md` → "How settings files resolve across
worktrees"). `firm-perms` writes them at session end.
