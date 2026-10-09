---
name: init-pr
description: Bootstrap a worktree — pre-check the gh credential and signing first, then fetch main, set up the branch, push a draft PR, and warm CI caches.
disable-model-invocation: true
user-invocable: true
---

# `init-pr`

Bootstrap the current worktree: fetch main, set up the branch, push a
draft PR so CI caches start warming while work continues. This is the
first skill an agent runs after `claude --worktree <tag>` starts.

Two cheap pre-checks come **first**, before anything that mutates the
worktree: the `gh` credential (with commit signing), and whether the
issue already has a merged PR. Each catches a failure that is
otherwise discovered late and expensively.

There is **no model check**: `task` pins the executor tier's model at
launch and refuses to start when it does not resolve (see
`docs/conventions/local-integrations.md`), so a turn spent asking
which model is running buys nothing.

The measured incidents behind the rules below live in
[`history.md`](history.md) — one provenance line here, the figures
there.

## Step 0b: pre-check the GitHub credential and signing

```sh
gh auth status
```

If it reports no valid credential, **stop** and tell the user to
re-authenticate — don't rename, rebase, or commit first (anonymous
reads succeed on this repo, so a dead token otherwise surfaces only at
`git push`). Read the **scope set** too: step 9 needs `notifications`,
which a re-auth silently drops; the one-time fix is
`gh auth refresh -h github.com -s notifications`.

**Signing is read from the helper call below — its `signing` field —
never by probing the ssh agent here.** Act on it before step 4's
rename, the first thing that costs anything to undo:

| `signing`                 | Configuration                                                                      | Action                                                      |
| ------------------------- | ---------------------------------------------------------------------------------- | ----------------------------------------------------------- |
| `external-signer`         | `gpg.format = ssh`, `gpg.ssh.program` is a non-`ssh-keygen` signer (`op-ssh-sign`) | Proceed — the signer reaches its own backend, not an agent. |
| `external-signer-missing` | Same, but the signer resolves neither on disk nor on `PATH`                        | **Stop and ask**: reinstall it or fix `gpg.ssh.program`.    |
| `agent-ok`                | `gpg.format = ssh`, agent-based (no program, or `ssh-keygen`), agent holds keys    | Proceed.                                                    |
| `agent-locked`            | Same, agent empty or unreachable                                                   | **Stop and ask**: unlock the 1Password app.                 |
| `gpg`                     | `gpg.format` unset or non-`ssh`                                                    | Proceed; nothing to check.                                  |

On any stop in this step, ask rather than retrying more than once or working
around it; nothing is lost, since no commit was written yet.

Add no agent probe (`ssh-add -l`, `ssh-keygen -Y sign`) or
`--show-signature` read: with an external signer git never consults
the agent, so they fail unconditionally. `external-signer` proves only
that the signing path is **wired**; a locked app still fails at step 6
(`failed to fill whole buffer`). Only a signed commit proves signing,
and **a signing failure is an unpushed-state alarm.**

## Step 0c: pre-check that the issue is not already merged

```sh
gh pr list --repo DASMAC-com/dropset --search "<ENG-###>" \
  --state all --json number,title,state,mergedAt
```

- **A merged PR is a STOP.** Touch nothing. Say "ENG-### already
  merged as PR #N; the issue looks mis-stated or mis-queued" and ask
  via `AskUserQuestion` whether to stand down or proceed with genuine
  follow-up scope. Done means operator-ratified, not merged (per
  `docs/conventions/linear-automation.md`), so a merged issue sitting
  in Backlog is a contradiction, not an instruction.
- **A closed-not-merged PR is a warning** — name it and carry on.
- **None, or only open ones**, is normal; say nothing.

`gh` rather than the MCP because this is a field-selected read of a
few rows and reuses the existing `Bash(gh pr list:*)` rule.

## Input

An optional Linear tag like `eng-123`; otherwise infer it from the
worktree directory name. If it doesn't match `eng-###`
(case-insensitive), stop and ask.

**With no other context** — just the tag, no task instructions — the
linked Linear issue is the full specification: surface its
description and checklist as the plan of work (step 12) and proceed
straight into the task. Instructions the user *did* give win.

## Decision points use `AskUserQuestion`

Every decision the session needs — a design choice, an open question,
the closing `/review-pr` handoff — goes through the `AskUserQuestion`
selector, never a free-text prompt, with concrete options and the
sensible default **first**, labeled "(Recommended)". This mirrors
`review-pr`'s merge-queue handoff one stage earlier.

## The implement phase: context discipline

The context economy `review-pr` enforces applies equally to the
implement phase this skill hands into — it slips here because no skill
is driving. The rules live in `docs/conventions/context-economy.md` →
"The levers" and bind the **main loop**, not only sub-agents. Read
them there; what follows is only what is specific to this phase or not
stated there.

The three rules specific to `init-pr`:

1. **Confirm a disposition another session owns before mapping
   anything.** The tell is a spec whose headline item is phrased as a
   settled decision you did not watch being made ("drop the X tier",
   "now that Y is retired"). A filed issue is a snapshot of its
   discovery commit; one message to the owning session costs a
   fraction of one map, and a reversed premise invalidates the map.

1. **Survey with a scoped `Explore` agent, or not at all.** For a
   survey of reference code (external or in-repo), spawn `Explore`,
   not `general-purpose`, with an explicit **named-path allowlist**,
   caps in **turns and tool calls** (e.g. "≤ 8 turns, ≤ 12 tool
   calls, then report"), and a compact file → responsibility → key
   symbols map as the deliverable. A ≤ ~3-file question is cheaper
   Read directly. Compose the brief with the committed tool, never by
   reading the convention doc to quote it:

   ```sh
   python3 .claude/tools/lens_preamble.py --out <scratchpad>/brief.md \
     --no-facts
   ```

   `--no-facts` is required when you hold no verified facts (the tool
   exits 2 with none of `--fact`, `--facts-file`, `--no-facts`); pass
   facts instead when you have them.

1. **Ask a live planning session before reading the Planning
   document.** `ListAgents` is the liveness check. The document getter
   returns the whole, growing body with no slice accessor, and a stale
   line reads exactly like a current one; a planning session answers
   with the current ruling. Read the document only when none is live.

The convention owns the rules this phase slips on most — the four
whole-read licenses, declaration-only section maps, `run_quiet.py` by
shape (dry runs included), never polling a backgrounded log,
`make tools-tests` whole, `wait_for_checks.py` for CI, and never
re-reading what you authored. A refused search leaves its question
unanswered, never zero hits (`docs/conventions/shell-commands.md`).
Three reminders neither states:

- **Lint the changed set** with one bare command; full `make lint`
  only before committing and at the end:

  ```sh
  python3 .claude/tools/run_quiet.py -- \
    python3 .claude/tools/lint_paths.py --changed
  ```

  Append `-- <hook-id>` to narrow it to one hook. Scope the **file
  list**, never the crate set — a crate-scoped
  `cargo clippy` reports false dead-code errors; verify in the form
  CI runs.

- **Don't re-derive a diff.** Read a `review_diff.py --split` diff
  from its slices; reach for `git diff` only for a change you have not
  read (a rebase, a hook autofix, a sibling session), and take
  `--stat` first when the question is which files moved.

- **`replace_all` is safe only when search and replacement are
  disjoint.** A replacement that contains the search string rewrites
  sites already renamed (`MAX_ATTEMPTS` → `REALIZED_FILL_MAX_ATTEMPTS`
  mangles an existing import), and surfaces as an error naming the
  *correct* symbol.

## The branch/worktree helper tool

Tag validation, base-repo resolution, branch-name normalization, the
signing verdict, and the two operator-file symlinks
(`frontend/.env.local`, `infra/localnet/secrets.local.env`) live in
`.claude/tools/init_pr_branch.py`. Run it **once**, near the top:

```sh
python3 .claude/tools/init_pr_branch.py --tag <eng-###> --link-env
```

```json
{
  "tag": "eng-603",          // validated, lowercased
  "tag_valid": true,         // false (+ non-zero exit) if not eng-###
  "base_repo": "/…/dropset", // the refs/heads/main worktree, or null
  "current_branch": "worktree-eng-603",
  "normalized_branch": "eng-603",
  "rename_needed": true,     // true iff a `worktree-` prefix is stripped
  "env_link": "created",     // frontend/.env.local
  "secrets_env_link": "exists",  // infra/localnet/secrets.local.env
  "frontend_node_modules": "absent",  // present / absent / no-frontend
  "program_so": "absent",    // present / absent / no-program
  "signing": "external-signer",       // the step-0b table
  "signing_program": "/…/op-ssh-sign" // null when none is configured
}
```

Steps 1–4 and step 0b read their answers from this one call. The
`frontend_node_modules`, `program_so` and `signing` fields are
**measured facts** rather than predictions: act on them, don't reason
from the diff. `--link-env` keeps the command line free of absolute
paths, so it reduces to one stable allow-rule. Allow-rules already
reach every worktree (`docs/conventions/local-integrations.md` → "How
settings files resolve across worktrees"); what a cold one lacks is
untracked per-directory content, which step 3 handles.

## Steps

1. **Validate the tag.** If `tag_valid` is `false`, stop and ask for a
   valid `eng-###`. Otherwise use the lowercased `tag`.

1. **Fetch the latest `main`**, from inside this worktree:

   ```sh
   git fetch origin main
   ```

   This updates the shared `origin/main` ref that step 5 rebases onto.
   Never `git -C <base_repo> pull --ff-only` — the harness's worktree
   isolation refuses git operations outside this worktree. (The
   repo's worktree edit-path guard is a different thing; it covers
   file-mutating tools, not `Bash`.) Fast-forwarding the base repo's
   checkout is whoever works there's job.

1. **Read the cold-worktree fields** from the helper's JSON. Nothing
   here blocks the bootstrap.

   - **`env_link`** (`frontend/.env.local`) and **`secrets_env_link`**
     (the secrets enclave file `make collectors-up` reads; without it
     the keyed venues are skipped) each report `created` / `exists`
     (left untouched, never clobbered) / `no-source` / `no-base` (main
     isn't checked out anywhere) / `failed` (mention it; copy by hand).
     Read the two independently.

   - **`frontend_node_modules`: on `absent`, install now**, whatever the
     task touches — the `biome` and `tsc` hooks run on the whole tree,
     so the first full `make lint` fails without it:

     ```sh
     python3 .claude/tools/run_quiet.py -- pnpm --dir frontend install
     ```

   - **`program_so`: on `absent`, do not build at bootstrap.** Note
     that any litesvm test under `programs/dropset/tests/` needs
     `python3 .claude/tools/run_quiet.py -- make program` first (it
     copies the committed keypair; the failing tests' own
     `anchor keys sync && anchor build` suggestion is wrong for this
     repo). Also note `make test-no-teardown` leaves a
     `--no-default-features` `.so` behind that fails ~15 teardown tests
     in a later scoped `cargo test`.

1. **Normalize the branch name.** `claude -w <tag>` names the branch
   `worktree-eng-###`. If `rename_needed` is `true`:

   ```sh
   git branch -m <current_branch> <normalized_branch>
   ```

   Otherwise this is a no-op.

1. **Rebase onto the fetched upstream:**

   ```sh
   git rebase origin/main
   ```

   `origin/main`, never the local `main`, which may be stale. On
   conflicts, `git rebase --abort` and tell the user; never resolve
   them here.

1. **Create an empty, signed commit** with a conforming semantic
   subject:

   ```sh
   git commit --allow-empty -S -m "chore(<ENG-###>): Bootstrap the worktree"
   ```

   `-S` is mandatory (branch protection). The message must be
   byte-identical to the step-8 PR title: with one commit on the
   branch, the Semantic PR workflow compares the two.

1. **Push:**

   ```sh
   git push -u origin <eng-###>
   ```

1. **Open a draft PR** with an empty body and the bootstrap commit's
   subject as its title:

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

   Never the bare tag: it cannot satisfy the Semantic PR workflow
   (type, `^ENG-[0-9]+$` scope, capitalized subject), so the
   `opened` run always fails and stays in the checks rollup forever.
   `pr-title-description` rewrites the title during review. Keep the
   returned `number` and URL.

1. **Unsubscribe from the PR's notifications** — best-effort; on any
   error, note it and continue. No MCP tool covers a per-PR
   subscription, so this is a documented `gh` exception
   (`docs/conventions/github-mcp.md`). Resolve the GraphQL node id
   (the MCP returns the numeric id), then set `IGNORED`:

   ```sh
   gh pr view <number> --repo DASMAC-com/dropset --json id
   ```

   ```sh
   gh api graphql -F id=<node_id> -f query='
     mutation($id: ID!) {
       updateSubscription(
         input: { subscribableId: $id, state: IGNORED }
       ) { subscribable { viewerSubscription } }
     }'
   ```

   Success reads back `viewerSubscription: "UNSUBSCRIBED"`. Without
   the `notifications` scope it fails with `INSUFFICIENT_SCOPES`; if
   step 0b flagged that, say so rather than re-diagnosing.
   `housekeeping`'s notification sweep catches what this misses.

1. **Mark the Linear issue In Progress** via the MCP; on failure, warn
   and continue:

   ```txt
   mcp__claude_ai_Linear__save_issue(
     id: "<ENG-###>",
     state: "In Progress"
   )
   ```

   **Keep the response — it echoes the whole issue body**, which step
   12 needs. This is the one deliberate exception to routing a
   state-only write through the zero-echo `board_batch.py state`: here
   the echo *is* how the session obtains the spec, so the zero-echo
   path would only move the cost to a `get_issue`. Do not "optimize"
   it (see `docs/conventions/linear-automation.md`).

1. **Print the PR URL** and confirm the issue moved to In Progress.

1. **Surface the task when no other context was given.**

   - **Read the description from step 10's response — don't
     re-fetch.** Use `get_issue` only if that write failed or was
     skipped. Do pull `list_comments`: acceptance criteria sometimes live in an
     anchored comment.

   - **On a long spec, spill the body to a scratchpad file on the
     first read** (or use the harness's persisted copy if it
     overflowed) and work from it thereafter:

     ```sh
     python3 .claude/tools/read_result.py --field description \
       --headings <persisted-result-or-spill>
     python3 .claude/tools/read_result.py --field description \
       --section 'What changes' <persisted-result-or-spill>
     ```

     Read the whole body once, to plan; after that, a heading map or
     a section, never the field again.

   - **An ambiguous tag gets an `AskUserQuestion`, never a
     speculative fetch of candidates.**

   - **Treat the issue's `file:line` citations — and its "already
     landed" claims — as a snapshot of its discovery commit.** Verify
     each against `HEAD`; the stable key is the `**Fingerprint**`
     slug, never a line number.

   Present the description and checklist as the plan of work. The
   user's own instructions win.

1. **Claim the migration number now if the task adds one** under
   `db-schema/migrations/`:

   ```sh
   python3 .claude/tools/migration_collisions.py --others-from-gh
   ```

   At branch time the file doesn't exist, so the tool reports
   `status: "nothing_claimed"` and `clear: true` — a vacuous
   all-clear. **Read `next_free_number`** (one past the highest in the
   tree or any open PR, never a hole) and take it; re-run once the
   file is written so the compare actually runs. This is a branch-time
   step because the in-tree ascend guard first fires at rebase, and an
   applied migration is immutable — renumbering the wrong side against
   the shared dev database wedges it.

1. **Hand off to `/review-pr` when the work is ready.** Once the
   task is complete and every open question is resolved, ask via
   `AskUserQuestion` whether to run `/review-pr` now. **This question
   is `review-pr`'s entry gate** — the chosen tier authorizes its
   sub-agent fan-out, and it asks no separate spawn question (see its
   "The entry gate" section). Compute the signals from a real diff,
   **rebasing first** — a stale base makes the `files` array describe
   main's commits, not the branch:

   ```sh
   git fetch origin main
   git rebase origin/main
   python3 .claude/tools/review_diff.py --base main \
     --out <scratchpad>/review-diff.txt
   ```

   (To skip the rebase, call `--gate-only` first and take the full
   verdict only once `base_fresh` holds.) Read `files` and per-file
   `changes`, then offer:

   - **"Yes — full adversarial suite"** — first, recommended.
   - **"Yes — reduced tier"** — *only* under `review-pr`'s small-diff
     threshold (≤ 5 files, ≤ 60 changed lines, no program / SDK /
     migration / generation-input path, single crate), naming the
     actual signals in the option.
   - **"Not yet"** — stop and leave the PR as it is.

   On either yes, route straight into `/review-pr` with the tier.
   **Write no "delivered" narrative onto the Linear issue here** — the
   adversarial pass has not run and may invalidate it; the disposition
   is `review-pr`'s to record. Don't surface `/pr-title-description`
   as its own step: `review-pr` calls it.
