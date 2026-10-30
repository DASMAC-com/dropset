<!-- cspell:word Toolsets -->

<!-- cspell:word DOCKERHUB -->

# GitHub via MCP

All GitHub operations — opening PRs, updating titles and bodies,
reading the diff, watching checks, pulling failing-job logs — go
through the **GitHub MCP server** (`mcp__github__*`), not the `gh`
CLI, **with the deliberate exceptions below**. The skills (`init-pr`,
`pr-title-description`, `review-pr`, `linear-task`)
are written against it. `gh` survives in the cases enumerated below —
count them from the list rather than from a number in this sentence,
which drifted behind it twice:

- **The merge-queue handoff** — the enqueue (a `gh pr merge` write,
  plain where the `merge_queue` rule is active and `--auto` otherwise
  or as its fallback, **no** strategy flag: this repo's merge queue
  sets the strategy, so a `--squash` only warns) and a read-only
  dequeue probe (a `gh api graphql … mergeQueueEntry` read), both made
  inside `wait_for_checks.py --enqueue`. The enqueue stays on
  `gh` because the server exposes no auto-merge / merge-queue tool
  (`merge_pull_request` does an *immediate* merge, which bypasses the
  queue); the probe stays on `gh` because the server's
  `pull_request_read` omits the merge-queue state — and on a
  merge-queue repo a still-queued PR reports `autoMergeRequest: null`,
  so the probe must read `mergeQueueEntry` (non-null while queued)
  over GraphQL to tell a still-queued PR from one that was dequeued.

  **The enqueue prints nothing under this harness, and its exit 0 is
  not proof.** `gh` emits its confirmation only to a TTY, and an
  `--auto` exit 0 shows only that auto-merge was enabled — a PR has
  exited 0 and never queued. So the tool probes `mergeQueueEntry`
  right after each attempt and returns one verdict.

  **An all-null probe is ambiguous, and the run list disambiguates
  it.** `mergeQueueEntry` and `autoMergeRequest` are *both* null during
  the registration window **and** after a dequeue, so the probe alone
  cannot tell a PR that has not registered yet from one that was kicked
  out. The queue branch settles it: a completed run set on
  `pr-<number>-…` proves the entry existed. The queue watch reads those
  runs itself, before concluding, and a `not_queued` verdict says
  whether any earlier cycle's run exists. **The fourth state is not
  all-null**: a non-null `autoMergeRequest` beside a null
  `mergeQueueEntry` means auto-merge is enabled and the PR has **not**
  entered the queue. Auto-merge waits for every pending check,
  required or not, so the head commit's check rollup names the one it
  is waiting on; with the `merge_queue` rule active, prefer the plain
  `gh pr merge <n>`. The queue watch reports this state as
  `auto_merge_waiting`.

  **But rule out "already merged" first, and read that as an
  invariant rather than a lag direction.** The all-null probe has a
  *third* reading beyond those two, because this GraphQL probe and the
  `gh pr view --json state,mergedAt` read disagree in **both**
  directions — measured on three PRs within an hour, one with GraphQL
  ahead of the state read, one behind it, and one with both stale
  together. So neither source is reliably the fresher one, and a rule
  naming either as the tiebreak is wrong half the time. What holds:
  **a terminal `MERGED` from any source is authoritative** — merge
  is terminal and no source reports it spuriously, so only
  lateness is possible, never a false positive. Conclude "still
  queued" or "dequeued" only when **no** source says the PR is merged;
  when none does and the queue entry looks impossible for
  the run state, wait and re-probe. Every read — the PR timeline and
  then the REST pull (the state read's successor) each poll, the
  GraphQL probe and the queue-branch runs once a minute — now runs
  inside `wait_for_checks.py --queue`, which applies the invariant on
  every poll (a merge writes a `removed_from_merge_queue` event too,
  so a removal never concludes a dequeue ahead of it), including the
  `AWAITING_CHECKS` one that otherwise polls
  a merged PR indefinitely; `review-pr`'s history ledger carries the
  per-PR evidence.

- **The CI-wait and PR-state reads** — conditional `gh api -i` REST
  reads for the CI-wait and queue polls (an unchanged poll answers
  `304`, which the rate limit does not count), `gh pr checks --json`
  for the CI verdict, and `gh pr view <number> --json <fields>` for
  the one-shot `mergeable` / PR-lookup reads. These reads are
  **polled repeatedly** across the CI and merge-queue waits, and the MCP
  equivalents (`pull_request_read` `get` / `get_check_runs`) return
  the **full** PR object or check array on every poll — a fat payload
  that, because a tool result is replayed as input on every later
  turn (see [context economy](context-economy.md)), is paid many times
  over. The polls run inside the tool and land in its log, never in
  context; `gh pr checks` is one compact line per check, and
  `--json <fields>` selects only the fields the decision needs.
  `--json` / `--jq` are command **flags**, not shell pipes, so they
  stay shell-rule-clean and reduce to `Bash(gh pr checks:*)` /
  `Bash(gh pr view:*)` allow-rules. This is the one place a `gh` read
  is preferred *over* the MCP: when the call repeats and the payload —
  not the transport — is the cost. `review-pr` issues them through
  `wait_for_checks.py` (`--pr`, `--mergeable`), which owns the pacing,
  never a shell `while … sleep` loop or a `jq` filter; its failure path
  names a per-step `next_command` (a `gh run view --log-failed` through
  the quiet runner) rather than `get_job_logs`.

- **Any INVENTORY read** — a few fields across many PRs: which PRs are
  open, what files they touch, what their heads are. The carve-out above
  reads as being about *CI polling*, so an open-PR inventory question
  routes to the MCP by default, and the cost there does not scale with
  the fields wanted but with the **number of PRs**: an MCP list response
  carries a full repository object per item, and `minimal_output: true`
  does **not** remove it. Measured, twice in one session for the same
  question — checking whether another open PR had claimed a migration
  number:

  - `mcp__github__list_pull_requests` with `state: "open"`,
    `perPage: 20` and `minimal_output: true` — **3.7k**, with minimal
    output set, to answer what one PR body line answered;
  - `gh pr list --state open --json number,files --limit 30 --jq '…'`
    filtered to the migrations directory — a two-line result.

  So take the field-selected form, and take `--jq` with it, since that
  is what makes it cheap and is the part most easily left off:

  ```sh
  gh pr list --state open --json number,files --limit 30 --jq '<filter>'
  ```

  `--jq` is a command **flag**, not a shell pipe, so this stays a single
  bare command under the existing `Bash(gh pr list:*)` rule. (A tool
  that needs this inventory should do the read **in its own process**
  instead — see `migration_collisions.py --others-from-gh` and
  `review_diff.py --overlap`, which exist so the per-PR file lists never
  reach a transcript at all.)

- **The queue-exit notification dismissal** (`review-pr`) — done
  **inside** `wait_for_checks.py --queue`, which on every exit but
  `probe_failed` calls `pr_notifications.py`'s `--pr` path in its own
  process, before it exits: every thread on the PR is marked done
  (`DELETE /notifications/threads/{id}`). **It unsubscribes only on
  merge** — on a `merged` verdict each thread is first ignored
  (`PUT /notifications/threads/{id}/subscription`, `ignored=true`) so a
  later bot comment cannot re-create it; a PR still open keeps its
  subscription, which is what brings its thread back on new activity.
  That leaves no turn between the
  outcome and the dismissal, and no list payload in any transcript:
  `mcp__github__list_notifications` once returned **3.7k tokens in a
  single call** for that one id, because it embeds the complete
  repository object (every `*_url` template) once per notification.
  The dismissal is best-effort — a failure is reported in the verdict's
  `notifications` field, and the settled sweep below is the fallback.

- **The settled-PR notification sweep** (`pr_notifications.py`) — the
  one rule every other caller applies: `dismiss --settled` dismisses
  every thread whose PR is merged or closed, **whatever its issue's
  Linear state** (a merged PR whose issue sits In Review is settled
  code), never an open PR's. `dismiss --pr N` instead dismisses every
  thread on the named PRs **whatever their state**, unsubscribing unless
  `--keep-subscription` says they are not merged. **Unsubscribe only on
  merge**: a merged PR's thread is two calls, never mark-all — ignore
  it, then mark it done — while a closed PR's (which can be reopened,
  and must resurface) is marked done only; dry run unless `--apply`.
  The ignore is
  a `PUT` with `ignored=true`, never a `DELETE` on the subscription,
  which GitHub documents as not holding for a watcher of the
  repository. Dismissal waits for the PR to settle, so the merge or
  close is the earliest point. The upkeep pass
  runs `--settled` armed every morning, the planning close-out runs it,
  and `close_session.py` calls
  the `--pr` path in-process for its branch's PRs (having already
  refused an open one) before removing the worktree.

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

  `gh pr create` prints only the PR's URL (and the MCP
  `create_pull_request` only its *numeric* database id), so the
  bootstrap first resolves the node id with
  `gh pr view <number> --json id` (gh's `id` field over its GraphQL
  *is* the node id). A success returns
  `viewerSubscription: "UNSUBSCRIBED"` (GitHub normalizes the `IGNORED`
  readback), which is what stops the lifecycle self-pings. The mutation
  needs the `gh` token's **`notifications`** OAuth scope — a one-time
  operator grant
  (`gh auth refresh -h github.com -s notifications`); without it the
  call fails with `INSUFFICIENT_SCOPES`. It's **best-effort**:
  `init-pr` continues if it errors, and the upkeep pass's finished-PR
  notification sweep is the catch-all for anything it misses. Both
  commands run inside `init_pr_bootstrap.py`'s own process, so the
  directory-wide `Bash(python3 .claude/tools/:*)` rule covers them and
  no new permission is needed.

- **The draft PR at bootstrap** (`init_pr_bootstrap.py`) —
  `gh pr create --draft`. The bootstrap is one tool run so that none
  of its deterministic acts costs a turn, and a tool process cannot
  call the MCP; routing this one write back through the skill would
  restore the turn the tool exists to remove. It touches only the new
  PR's own title, body and draft state, the same low blast radius as
  the pre-approved `create_pull_request`.

- **The credential pre-check** (`init_pr_bootstrap.py`'s `auth`
  step) — `gh auth status`,
  run before anything that mutates the worktree. This one needs no
  payload or transport argument at all: the question *is* whether the
  `gh` CLI itself holds a working credential, so there is nothing for
  the MCP to answer. The server carries its own PAT (see
  "Authentication" below) and so stays connected while `gh`'s token is
  expired — which is exactly the state this check exists to catch, and
  why an MCP call would give a **false all-clear**. The same output
  reports the **scope set**, which is where the `notifications` scope
  above is confirmed. Run in-process, as is the bootstrap's next read.

- **The merged-PR pre-check** (`init_pr_bootstrap.py`'s
  `merged_check` step) — `gh pr list --search <ENG-###> --state all`
  with a `--json` field selection, a few rows. Same reasoning as the
  `upkeep.py` read below (only the fields the decision needs), and
  in-process for the same reason as the draft PR: a tool cannot call
  the MCP.

- **The field-selected `gh pr list --json` read** (`upkeep.py`) —
  `gh` selects only the fields the decision needs, so a field-selected
  read replaces a list-every-closed-PR call (~104k tokens of full
  bodies, replayed every later turn — see
  [context economy](context-economy.md)) *plus* the per-PR body
  fetches it would otherwise need. The upkeep pass's worktree-prune
  reads `gh pr list --state closed` and `--state open`, each selecting
  only `number,headRefName,state`, **once** apiece: the closed set ties
  a finished branch to its PR, and the open set is the open-PR
  survival test, in place of one full-body `list_pull_requests` per
  branch. `--json` is a flag, not a pipe, so
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
what make the full form expensive. Take it **instead of**
`pull_request_read` `get_files`, whose `minimal_output: true` reads as
protection and is not: it still returns every file's full **patch
text**. One such call, made only to learn whether a PR carried a given
migration, cost **≈5.2k** — a 136-line migration and a 211-line module
inline — for a question the filenames answered.

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

**Remote, but locally configured — which is what makes it usable
everywhere.** The server runs at a GitHub URL, yet it is registered in
user-local settings with our own credential rather than riding claude.ai
account auth. That distinction, not MCP-vs-CLI, is the one that
determines availability: a locally configured server is offered to a
**Bedrock** session and a subscription session alike, while a
claude.ai-hosted connector is structurally absent on Bedrock. Confirmed
2026-09-10 on a Bedrock worktree session, which received this server
along with the other eight locally configured ones and neither hosted
connector.

So **this server stays as-is**, and an operator-reported failure of it on
a Bedrock session should be diagnosed against that split before anyone
edits config. The same reasoning is set out at length, with the separate
context-economy argument kept apart from it, in
[aws-infra](aws-infra.md) → "Hosted versus locally configured".

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

**This pattern generalizes — reach for it before building around a
server.** Linear is registered the same way, user scope with the key in
an `Authorization` header, which is what is expected to give a Bedrock
session first-class Linear access; see `linear-automation.md` → "Two ways
to reach Linear's MCP". Both **MCP** endpoints take the credential the
same way, as `Bearer <credential>`; what differs is that Linear's *other*
surface does not — its GraphQL API takes the key bare, and sending that
bare form to its MCP endpoint returns an OAuth challenge indistinguishable
from OAuth-only. So try both shapes before concluding a server has no
key-based path.

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
  `review-pr`, `pr-title-description`, and `upkeep.py` use in place
  of the full-object MCP calls (see "GitHub
  via MCP" above and [context economy](context-economy.md)).
  `init-pr`'s bootstrap makes its `gh` calls inside its own process,
  under the tools-directory rule, so it needs none of these. These are
  Bash globs, not `mcp__github__*` entries, but they're pre-approved on
  the same rationale (routine, low-blast-radius calls).

- **Pre-approve (the `gh api` REST reads):** `Bash(gh api repos/:*)` —
  a third `gh` shape, distinct from `gh api graphql` above and easy to
  miss because the two share a verb. `review-pr` uses it for the branch
  ruleset's check list (`repos/{owner}/{repo}/rules/branches/main`);
  the failing job's step conclusions
  (`actions/jobs/<id>`, or a run's `actions/runs/<id>/jobs`) are read
  inside `wait_for_checks.py`, which filters in Python. Reads only — the
  `gh api` **writes** are the bootstrap's `updateSubscription`
  mutation and the notification-thread `DELETE`s of
  `wait_for_checks.py --queue` and `upkeep.py`, each made in its own
  tool's process.

  The ruleset call site passes a `--jq` filter containing a `|`. That is a
  command **flag**, not a shell pipe, so the compound guard does not
  fire on it (a quoted separator never reaches its unquoted scan — see
  [shell commands](shell-commands.md)). Whether the *harness's* own
  per-subcommand prompt treats a quoted `|` as firmable is a separate
  question this doc does not settle either way; if these turn out to
  re-prompt on every run, the fix is to move the filtering into the
  consuming tool rather than to drop the carve-out.

- **Pre-approve (routine PR-authoring writes):** `create_pull_request`
  (`init-pr`'s fallback when the bootstrap's `pr` step stops twice —
  its `stopped_at` table) and `update_pull_request`
  (pr-title-description, review-pr). The skills call these to open
  and maintain the draft PR,
  and they touch only the PR's own title / body / draft-state — low
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
allow-rule per read tool covers all of its methods. They go in
`settings.local.json` like any other rule, which is **one shared file
resolved through worktrees to the main checkout** — so firming them
once makes them live in every worktree, with nothing to propagate
(see `local-integrations.md` → "How settings files resolve across
worktrees"). Write one with
`python3 .claude/tools/allowlist.py add '<rule>'`.

## Actions: secrets, variables, and required checks

Nothing governed GitHub Actions configuration before this section, and
the absence was the finding rather than any specific drift. It is
deliberately **inventory-shaped**: the job is that the *next* secret or
required check gets added against a stated rule instead of extending
per-PR folklore.

### Repository secrets

Three are configured, and the whole set is worth naming because a
secret is invisible from the checkout:

| Secret                | Consumed by          | For                |
| --------------------- | -------------------- | ------------------ |
| `DOCKERHUB_USERNAME`  | `explorer-image.yml` | Docker Hub login   |
| `DOCKERHUB_TOKEN`     | `explorer-image.yml` | Docker Hub login   |
| `PLATFORM_FEE_WALLET` | `frontend.yml`       | the fee-vault gate |

Values obviously never appear here. `secrets.GITHUB_TOKEN` also appears
in the workflows (`semantic-pr.yml`) but is **not** one of these: Actions
provides it automatically per run, so it needs no configuration and
cannot be inventoried alongside the three above.

The fee wallet is a secret **by operator direction** — the address stays
out of version control.

### Secret vs variable vs committed constant

The rule, hoisted from where it was previously recorded only in a
`frontend.yml` comment:

- **A secret** when the value must be **masked in logs**. This is the
  operative test, and it is about log exposure rather than
  confidentiality: the fee-vault step's script *echoes* the address, and
  **Actions masks only secrets**, so a repository *variable* would
  print it in plain text. A value that is merely non-public but never
  echoed does not need this.
- **A repository variable** for non-sensitive configuration that
  differs by environment and is never echoed.
- **A committed constant** for anything the repo can legitimately
  publish — the default, since it is reviewable in a diff and needs no
  out-of-band setup for a fresh clone or a fork.

Scope a secret to the **step** that needs it, not the job or the
workflow — the fee-vault step does exactly this, and it keeps the
credential out of every build step that has no business seeing it.

### Required status checks

Nine contexts gate `main`, via a repository **ruleset** (not classic
branch protection — the branch-protection REST endpoint returns 404, so
read them with `gh api repos/{owner}/{repo}/rulesets/{id}`):

`Semantic PR`, `Lint`, `Detect changes`, `Tests`,
`Tests (no teardown)`, `Tests (asm parity)`, `Tests (Postgres)`, `SDK`,
`Frontend`.

Note these are **job** names, not step names. A workflow can therefore
add or remove a *step* inside a required job with no ruleset change at
all, and adding a whole new **job** leaves the required set untouched
unless someone also adds its context **to the ruleset** (and mirrors it
here). Both directions have bitten: the token-icon liveness check was
assumed to be a required check needing a ruleset edit to relax, when it
was a step inside `Frontend` — relaxing it was a workflow edit and
nothing more.

Keep the direction of authority straight, since this paragraph exists to
say which artifact to change: the **ruleset** is the gate and the list
above is a mirror of it. Editing the list changes no gating, and leaving
the list alone does not keep a job non-required.

**`Token icon upstream audit` is deliberately absent** from the list
above. It fetches issuer URLs to check the committed token icons for
drift, and it is advisory by three independent layers: it is not in the
required set, it does not trigger on `merge_group`, and its **audit path
exits 0 whatever it finds** (only its `--write` refresh mode can fail,
and CI never runs that). Keep it out of the **ruleset** — adding it
there would recreate the exact merge-queue exposure it was built to
remove.

**`Explorer image` is non-required by construction.** It runs on
`pull_request` behind a trigger-level `paths` filter, so most PRs never
emit the context at all; made required, it would leave those PRs
pending forever and hang the queue (the hazard the `changes` job in
`test.yml` is designed around). Keep it out of the ruleset too.

### Token permissions

Every job names a `permissions:` block (`semantic-pr.yml` at workflow
level), so no job inherits the repository's default `GITHUB_TOKEN`
grant (which a session token cannot read:
`actions/permissions/workflow` returns 403). The grants are
`contents: read` and `pull-requests: read` (`Detect changes`,
`Semantic PR`), with **one non-read grant**: `actions: write` on the
three toolchain jobs (`Tests`, `Tests (no teardown)`,
`Tests (asm parity)`). It lets the `install-toolchain` action delete a
cache entry that restored incomplete, so the run that reinstalls
republishes a good entry rather than leaving a poisoned one; the probe
spends it only on a push to `main`. A new job adds its own block at
the narrowest grant it uses.

### Forks get no secrets — so a secret-dependent required check is fork-hostile

A `pull_request` run from a fork receives **no secrets**. This is a
GitHub security property, not a setting, and it interacts badly with the
required-checks list above: any required check with a secret dependency
**cannot pass on a fork PR**, and the contributor cannot fix it.

`Frontend` is the first required check to acquire such a dependency, via
the fee-vault step. The failure is **safe** — the secret is absent, so
nothing leaks, and the step fails closed — but it is unfixable from the
fork side and its message will not explain why.

**Dormant while the repo has no external contributors**, which is why
this is recorded rather than fixed. The revisit condition is the first
fork PR, and the two shapes available then are to guard the step on
`github.event.pull_request.head.repo.fork` (skipping the gate for fork
PRs, which weakens it exactly where review is least trusted), or to
split the fee-vault gate into its own non-required check. Prefer the
split: it keeps the gate's coverage on the merge path, where the
motivating failure actually occurred, without making a required check
unsatisfiable.
