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

Every deterministic act of the bootstrap is **one tool call**; the
skill keeps only the turns that hold a judgment — adjudicating a stop.
The tool's two cheap pre-checks still come **first**, before anything
that mutates the worktree: the `gh` credential, and whether the issue
already has a merged PR. Commit signing is probed too, but only to
decide whether to commit signed — it never stops the run.

There is **no model check**: `task` pins the executor tier's model at
launch and refuses to start when it does not resolve (see
`docs/conventions/local-integrations.md`), so a turn spent asking
which model is running buys nothing.

The measured incidents behind the rules below live in
[`history.md`](history.md) — one provenance line here, the figures
there.

## Step 0: run the bootstrap

```sh
python3 .claude/tools/init_pr_bootstrap.py --tag <n> \
  --spec-out <scratchpad>/spec.md
```

In order: the `gh` credential, the merged-PR pre-check, the branch
helper (`init_pr_branch.py --link-env` — tag, operator-file symlinks,
measured facts, signing verdict), `git fetch origin main`, the
`worktree-` rename, the rebase onto `origin/main`, the frontend install
when `node_modules` is absent, the empty commit (signed when the
signer answers)
`chore(<ENG-###>): Bootstrap the worktree`, the push, the draft PR
with that subject as its title (byte-identical, which the Semantic PR
workflow compares while one commit is on the branch), the notification
mute, the Linear In Progress move, and the issue body spilled to
`--spec-out` (a missing parent directory is created). The manifest's
shape is in the tool's docstring; report the PR URL, any `failed`
step and any `notes` in a line each.

**Every step is idempotent, so the remedy for any stop is to fix the
cause and re-run the same command** — it resumes, never redoes. Read
`stopped_at`:

| `stopped_at`   | Cause                                                                                                             | Action                                                                                                                                                                                                                                                                           |
| -------------- | ----------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `auth`         | No valid `gh` credential                                                                                          | Tell the user to re-authenticate. Nothing was mutated.                                                                                                                                                                                                                           |
| `merged_check` | A **merged** PR on this branch or scoped to this issue; or an invalid tag                                         | Merged: `AskUserQuestion` — stand down, or proceed with genuine follow-up scope (re-run with `--allow-merged`). Done means operator-ratified, not merged, so a merged issue in the queue is a contradiction, not an instruction. Invalid tag: ask for a valid `###` / `eng-###`. |
| `branch`       | The helper failed, or `signing_probe.py` printed a `cannot run` line (git failing, not the signer) — see `detail` | Fix what `detail` names, then re-run.                                                                                                                                                                                                                                            |
| `rebase`       | A conflict or a dirty tree, or a signer that locked after the probe; any rebase in progress was already aborted   | A signer: re-run (its probe now defers). Otherwise tell the user; never resolve it here.                                                                                                                                                                                         |
| `commit`       | The commit failed **even unsigned** — git itself, not the signer                                                  | **An unpushed-state alarm**: say so, fix the cause, re-run.                                                                                                                                                                                                                      |
| `pr`           | `gh pr create` failed or printed no URL                                                                           | If a re-run fails the same way, open the draft with `mcp__github__create_pull_request` (same title, empty body, `draft: true`), then re-run — the open PR is reused.                                                                                                             |
| anything else  | The step's `detail` (an unexpected reply reads `<ExceptionType>: …`)                                              | Fix the cause and re-run. Steps never reached are in `remaining`.                                                                                                                                                                                                                |

On a stop, ask rather than retrying more than once or working around
it.

**A signer that cannot sign is never a stop** — signing defers to the
merge gate (`docs/conventions/commits-and-prs.md` → "Signing defers to
the merge gate"). `external-signer` proves only that the signing path
is **wired**, so the `branch` step also runs the dangling-commit probe
(`signing_probe.py`); on `LOCKED`, `external-signer-missing` or
`agent-locked`, the tool rebases and commits with `--no-gpg-sign`;
when only the signed bootstrap commit fails, the commit alone falls
back. Either way it pushes and sets `facts.signing_deferred`. A signer
that locks between the probe and a rebase with commits to replay stops
at `rebase`; the re-run's probe reads `LOCKED` and defers. Report the
note's unlock instruction in one line and **start the task anyway** —
never wait for the unlock; `review-pr` signs the branch in one pass
at the gate. Add no agent probe (`ssh-add -l`, `ssh-keygen -Y sign`),
which an external signer never consults, nor a `--show-signature`
read, which tests verification, not signing.

A `failed` step is best-effort and never stops the run: `mute` fails
without the `notifications` scope (the note names the one-time
`gh auth refresh -h github.com -s notifications`, and the upkeep
pass's notification sweep catches the rest once the issue closes —
armed, or by hand via `upkeep.py prune --apply`); `linear_state` —
say so; `install` — re-run
`python3 .claude/tools/run_quiet.py -- pnpm --dir frontend install`
by hand, since the `biome` and `tsc` hooks fail the first full
`make lint` without it; `spec` — fall back to `get_issue`.

Two **measured facts** in `facts` are acted on, not reasoned about
from the diff:

- **`env_link`** / **`secrets_env_link`** (`frontend/.env.local`, and
  the secrets enclave file `make collectors-up` reads): `failed` means
  copy that file by hand; `no-source` / `no-base` just mean there was
  nothing to link.
- **`program_so`: on `absent`, do not build at bootstrap.** Any
  litesvm test under `programs/dropset/tests/` needs
  `python3 .claude/tools/run_quiet.py -- make program` first (it
  copies the committed keypair; the failing tests' own
  `anchor keys sync && anchor build` suggestion is wrong for this
  repo).

The tool fetches from inside this worktree and never runs
`git -C <base_repo> pull` — the harness's worktree isolation refuses
git operations outside it; fast-forwarding the base checkout is
whoever works there's job.

## Input

An optional Linear tag like `eng-123`; otherwise infer it from the
worktree directory name — the bare number (`123`), or `eng-123` in
a pre-rename worktree; the tool normalizes either to `eng-123`,
the form the branch and the PR scope carry. If it is neither
(case-insensitive), stop and ask.

**With no other context** — just the tag, no task instructions — the
linked Linear issue is the full specification: surface its
description and checklist as the plan of work (step 1) and proceed
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

1. **Ask a live scheduler before reading the Planning
   document.** `ListAgents` is the liveness check. The document getter
   returns the whole, growing body with no slice accessor, and a stale
   line reads exactly like a current one; a scheduler session answers
   with the current ruling. Read the document only when none is live.

The convention owns the rules this phase slips on most — the four
whole-read licenses, declaration-only section maps, `run_quiet.py` by
shape (dry runs included), never polling a backgrounded log,
`make tools-tests` whole, `wait_for_checks.py` for CI, and never
re-reading what you authored. A refused search leaves its question
unanswered, never zero hits (`docs/conventions/shell-commands.md`).
Seven reminders neither states:

- **Test one tools module with the wrapper**:
  `make tools-test T=test_<tool>` (`K=<name>` narrows). It resolves the
  tree and the `-s` / `-t` pair, which a hand-typed
  `python3 -m unittest <path>` gets wrong. `make tools-tests` whole
  stays the checkpoint run.

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

- **Write the issue body on the zero-echo path.** The bootstrap read
  it without an echo; keep it that way. A body **append** (a progress
  or deferral note) goes through
  `python3 .claude/tools/linear_issue.py append --id <ENG-###> --file <path>`,
  never `save_issue`, which echoes the whole body again, larger
  each time it grows. Only an **anchored** edit stays on the MCP
  `patch`, where the abort is worth the echo — checklist ticks
  excepted, via `linear_issue.py tick`.

- **A false compile error on code you just wrote is the cargo pool
  first.** When `RUSTC_WORKSPACE_WRAPPER` does not name this
  worktree's own wrapper, rebuild under the right key, never a clean:
  `cargo clean -p` wipes that crate for every checkout on the lane
  (`docs/conventions/local-integrations.md` → "The cargo pool").

- **`replace_all` is safe only when search and replacement are
  disjoint.** A replacement that contains the search string rewrites
  sites already renamed (`MAX_ATTEMPTS` → `REALIZED_FILL_MAX_ATTEMPTS`
  mangles an existing import), and surfaces as an error naming the
  *correct* symbol.

- **A Makefile edit is verified by `make -n` on every target that
  expands the changed variable or recipe, re-run after each edit.**
  Lint (the Makefile linter included), the suites and CI all pass a
  lazily expanded `=` variable that fails to parse (before GNU make
  4.3, macOS's 3.81 included, a bare `#` inside `$(shell …)` starts a
  comment); only the recipes that expand it fail
  ([`history.md`](history.md) → "Makefile dry-run").

## Steps

1. **Surface the task when no other context was given.**

   - **Read the spec from the manifest's `spec_path` — once, whole,
     to plan.** The bootstrap wrote it there with zero echo; after
     that, Grep its `^#` headings and slice-read a section, never the
     whole file again. Use `get_issue` only if the `spec` step
     failed. Do pull `list_comments`: acceptance criteria sometimes
     live in an anchored comment.

   - **An ambiguous tag gets an `AskUserQuestion`, never a
     speculative fetch of candidates.**

   - **Treat the issue's `file:line` citations — and its "already
     landed" claims — as a snapshot of its discovery commit.** Verify
     each against `HEAD`; the stable key is the `**Fingerprint**`
     slug, never a line number.

   - **Check the declared parts against the scope band** (4–5 parts,
     300–800 lines, split at 1,000;
     `docs/conventions/context-economy.md`). Over it, ask via
     `AskUserQuestion` before implementing: message the live
     scheduler session the split now, or keep one PR — only for a
     test-heavy or prose-deletion overage, where a split would ship
     inert code. **Never a second PR on this
     issue** later (`docs/conventions/commits-and-prs.md`).

   Present the description and checklist as the plan of work. The
   user's own instructions win.

   **Then start implementing in the same turn — the bootstrap is not
   a checkpoint.** The dispatch was the authorization to build, so a
   plan stated and then left waiting for "proceed" costs the operator
   one guaranteed turn per launch. State the plan as a report in
   passing, never as a question. The only legitimate stops are a
   genuine decision point (consult the live scheduler first,
   then `AskUserQuestion`) or a blocked precondition.

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
   is `review-pr`'s entry gate** — a yes is the invocation, which
   pre-approves the regular fan-out, and the reduced-tier option is
   its abridged-tier opt-down (see its "The entry gate" section).
   Compute the signals from a real diff. The tool diffs from the merge
   base, so a stale base cannot leak main's commits into `files`; no
   rebase is needed first:

   ```sh
   python3 .claude/tools/review_diff.py --base main \
     --out <scratchpad>/review-diff.txt
   ```

   Read `files` and per-file `changes`, then offer. A non-zero exit
   with `base_drift.disjoint: false` still leaves `files` correct; the
   rebase it asks for is `review-pr` step 2's, so offer anyway:

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
