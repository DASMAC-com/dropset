<!-- cspell:word ETIMEDOUT -->

# `init-pr` history ledger

The measured incidents behind the rules in [`SKILL.md`](SKILL.md),
kept here so the entry file states each rule once. Read on demand,
never on invocation. The entry file grew 12k → 15k → 45k → 83k bytes
from July to October before this split. A figure
`docs/conventions/context-economy.md` already carries lives there and
is not repeated here.

## Pre-checks

- **Credential (step 0b).** One run reached the push step — renamed,
  rebased, signed empty commit — before `git push` died with "could
  not read Username" on an expired token. Diagnosis took five calls
  plus two `printenv` probes, and `git ls-remote` came back clean
  because anonymous reads succeed on this repo.
- **Signing dispatch.** An unconditional `ssh-add -l` pre-check
  hard-stopped every bootstrap on an external-signer (`op-ssh-sign`)
  machine four times, each an operator round trip; the fourth session
  fixed it, and its own bootstrap commit signed in the state the probe
  called broken. `ssh-keygen -Y sign` under the ambient environment
  fails the same way; `--show-signature` errors when local
  verification has no `allowedSignersFile`. A locked agent with only
  `user.signingkey` checked passes silently — measured on a locked
  1Password agent. The real step-6 failure reads
  `failed to fill whole buffer`, then
  `fatal: failed to write commit object`.
- **Already merged (step 0c).** 2026-09-03, the ENG-1060 session: the
  issue merged as PR #381, landed In Review, then moved back to
  Backlog fourteen minutes later (In Progress 09-01 22:45, In Review
  09-02 00:14, Backlog 09-02 00:28). A worker bootstrapped against it
  a day later and hit the signing pre-check before discovering there
  was nothing to build. The backwards move's cause is unknown.

## Implement-phase discipline

- **Disposition first.** One session spent ≈3.5k mapping every
  reference for a tier deletion; a planning ruling minutes later
  reversed the premise and the map was reverted.
- **Measured premise.** An issue's 285k-CU "cold flush" was a
  one-atom priming sell, found only after the SDK and three clients
  were built against it (≈\$25, 239 turns, most traced to it); a probe
  test up front was ≈5 turns.
- **Scoped surveys.** A `general-purpose` survey of reference repos
  pulled 2–2.5M input each; an open-ended in-repo "map the TUI and the
  bots" survey was the top sink of three consecutive sessions, each
  needing only ~3–6 named modules the main loop then Read anyway. A
  scoped survey briefed "≤ 10 turns / ≤ 15 tool calls" took 11 and 18
  (≈585k summed input, more than any of six review lenses), and the
  main loop then re-read its exemplar regions by slice to imitate
  them.
- **Log cascades.** `grep -nE -A4 '^error…'` over a 70-error E0603
  cascade (one private module) gave a session its two largest results,
  ≈6.8k each; the first three blocks answered it.
- **Planning document.** A whole-document read cost ≈9.6k (2.4× the
  next result) to answer a ~six-line question; the live
  scheduler then answered better, correcting a stale fact the document
  still carried. A worker that needed it as source material took it
  through `get_document` (≈9.1k, its largest result, replayed for
  ~120 turns) where `planning_doc.py --out` prints only a heading map.
- **Whole reads.** Router modules read whole after a map (≈8.6k, 62%
  of a session's Read cost).
- **Section maps.** `^///` over `schema_fence.rs` cost ≈1.8k.
- **Quiet runner.** Unwrapped cspell cascade (≈2.5k); 7 bare
  collector-stack runs (3.5k); a cold `pnpm install` full of registry
  `ETIMEDOUT` retries (≈2.0k).
- **Lint.** 13 full `make lint` sweeps (≈5.8k) while editing the rule
  forbidding them; a crate-scoped clippy reported five false dead-code
  errors CI did not.
- **`replace_all`.** `MAX_ATTEMPTS` → `REALIZED_FILL_MAX_ATTEMPTS`
  produced `REALIZED_FILL_REALIZED_FILL_MAX_ATTEMPTS` and 14 failing
  tests reading `ReferenceError: REALIZED_FILL_MAX_ATTEMPTS is not defined`.
- **Diffs.** A bare `git diff` was one session's largest result
  (≈2.9k, 4.3k over 6 calls); another diffed its own authored file
  (≈4.3k).
- **Manual CI polls.** Four `gh pr checks` calls (922 tokens) before
  one `wait_for_checks.py` (≈200).
- **Makefile dry-run.** A late edit tightened a grep inside a
  `$(shell …)` held in a recursive `=` variable to `'^[^#]*op://'`;
  the bare `#` started a make comment and left the call unterminated.
  `make -n` had run before that edit, not after. Lint (the Makefile
  linter included), `make test`, CI and a five-lens review all passed it;
  a post-rebase dry-run caught it, at one extra commit, lint pass and
  CI cycle.

## Steps

- **Cold frontend (step 3).** A docs-only diff's first `make lint`
  failed only on `Command "biome" not found` and
  `Command "tsc" not found`; the install had been skipped because the
  diff did not touch the frontend.
- **Missing program (step 3).** A cold worktree fails every litesvm
  test with a 125-line tail; the branch that measured it changed only
  `programs/dropset/tests/**`.
- **Bare-tag PR title (step 8).** Every PR this skill created failed
  its `opened` Semantic PR run; on PR #329 the residue read as a check
  bypass and cost an operator investigation.
- **Echo reuse (steps 10, 12).** Re-fetching after the In-Progress
  write cost two ≈1.1k echoes for one payload; one session read the
  same long body three times (≈14.8k); a speculative fetch to
  disambiguate an "8.5.9" reference cost ≈4.5k and settled nothing.
- **Stale citations (step 1).** All four `(§4 row 5)` citations in
  one spec had been rewritten away by an unrelated PR, making the item
  moot, established via four greps including a ≈1.1k sweep.
- **Subject re-scope (step 1).** A wallet-bug issue carried a
  same-day re-scope to a different feature; the session built both
  under the old issue, and the split (new issue, retitle, strip,
  recommit, rebase, force-push, second preview) cost ≈30 turns at a
  200k+ prefix.
- **Scope pre-flight (step 1).** An issue named two conformance
  suites; the implement phase covered one, and the gap surfaced at
  `review-pr`'s grading — a fix commit, a second rebase and re-run
  gates at a ≈300k+ prefix (189 turns, ≈\$21).
- **Scope sizing (step 1).** A TUI feature estimated at ≈550 lines
  landed +1,261 / −137 (\$29, 193 turns): rustfmt expanded the new
  module ≈1.4×, and each new `Action` variant touched ≈10 match tables
  plus the all-variants fixture. The overage surfaced only after
  implementing, when a further split was no longer allowed.
- **Migration numbers (step 13).** Two branches took the same number
  twice in one week; the two-command form routed every open PR's file
  list through context (~4.0k).
- **Stale base at handoff (step 14).** A base three commits behind
  returned 85 files from main's own commits (≈2.0k) against a 6-file
  branch diff; a premature "delivered" note was invalidated by the
  adversarial pass, forcing a second corrections append.
