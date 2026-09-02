---
name: commit-changes
description: Stage, commit, and push changes from this worktree with a clean hand-authored commit message.
user-invocable: true
---

# `commit-changes`

Commit and push the changes in this worktree.
Each worktree is an isolated copy of the repo
owned by a single agent, so all uncommitted
changes here belong to this session.

## Steps

1. Inspect the working tree:

   ```sh
   git status
   git diff --stat
   ```

1. Review all changed and untracked files. Stage
   them by explicit path:

   ```sh
   git add <path1> <path2> ...
   ```

   Never use `git add -A`, `git add .`, or
   `git add -u`. Always list paths explicitly
   so nothing unintended slips in (build
   artifacts, generated files, secrets).

   **A bare `pre-commit --all-files` does not check
   new files.** It enumerates via `git ls-files`, so
   an untracked file is simply *not checked* — one
   run's two freshly written test files passed a
   green sweep and then failed cspell on the next
   run, once committing had made them visible.
   `make lint` is no longer affected: it goes
   through `.claude/tools/lint_paths.py`, which
   lints tracked **and** untracked-not-ignored files
   alike. But any hand-rolled sweep over
   `--all-files` still proves nothing about a file
   git has never seen — run `make lint` instead.

   **For a fast suite the lever is FREQUENCY, not
   scope — run it whole, here, at the checkpoint.**
   Both halves matter and they are easy to confuse:

   - **Scope:** run it **whole, through the wrapper**.
     A `-p test_X.py` discover run is *narrower* and
     **more** expensive — 32 calls / ≈7.1k against 15
     calls / 516 tokens — because the `make` target is
     wrapped and the discover call is not, and it
     missed sibling tests the edit had just broken.
     This is what `review-pr` and `init-pr` also say.

   - **Frequency:** run it at a **checkpoint**, not
     after every single-file edit. Measured: one session
     ran `make tools-tests` **53 times** (≈12.0k of
     result bytes, classified `context (failures)`) in an
     edit-one-tool loop, most runs re-buying confidence
     the previous run had already established. The
     wrapper is already in place there, so those bytes
     are failure tails — the lever is fewer round trips,
     not more redirection.

     That session's own diff **documented this
     discipline**, here and in `review-pr`, and then
     violated it 53 times. A rule stated in a skill the
     session is actively editing, and still not followed,
     wants a cheaper trigger than prose — so **gate the
     run on a content fingerprint**, exactly as
     `review-pr` step 5 already gates `make lint`. After a
     passing run:

     ```sh
     python3 .claude/tools/tree_fingerprint.py record --check tools-tests
     ```

     and before any later run that would repeat it:

     ```sh
     python3 .claude/tools/tree_fingerprint.py check --check tools-tests
     ```

     It grades **`fresh`** (recorded against this exact
     content — assert it and skip the re-run), **`stale`**
     (the content moved — re-run), or **`missing`** (never
     recorded — run it and record it), exiting 0 only on
     the first.

     Content, not a commit SHA, is the right key here for
     the same reason it is there: an amend, a squash and a
     no-overlap rebase all change the SHA while changing
     no bytes, so a SHA-keyed gate would let precisely the
     redundant re-runs through. A later run against an
     unchanged tree now has to **assert** the prior result
     rather than silently re-buy it.

     This is the gate that this passage previously only
     *proposed*, in the same breath as observing that
     restating the rule had not worked — the proposal was
     written in the PR that then ran the suite 53 times,
     and a later session ran it 26 more.

     (This figure is the **frequency** measurement and is
     a different quantity from the scope bullet's above.
     Attaching one number to both is what previously made
     these two findings read as contradictory.)

   So: whole suite, fewer times. The two rules point
   the same way once you see that one is about scope
   and the other about cadence — reading the
   frequency finding as permission to narrow the scope
   inverts it, and the narrow form is the one measured
   to miss real breakage.

   For a suite slow enough that wall-clock dominates,
   the per-module form earns its keep during
   iteration; this one runs in under a second, so it
   does not.

   **Don't re-derive the diff you already have.**
   If `review_diff.py` has already written slices
   for this range, self-review reads those files —
   re-running a bare `git diff` buys the same bytes
   twice, and did so in two measured sessions
   (≈3.4k and ≈3.1k, the second-largest single
   result of each). This is the "never re-fetch
   what's already in context" rule applied to a
   payload the session itself produced.

   **The same waste arrives without any slices,
   from a file you wrote yourself.** Content that
   reached the tree through `Edit` or `Write` is
   already in context by definition, so a `git diff`
   over it buys it a second time — one session paid
   **≈4.3k** re-deriving the diff of a file it had
   just authored. So the rule is not "read the
   slices if they exist": it is that a diff you
   could reconstruct from this session's own edits
   needs no command at all. Reach for `git diff`
   when the change came from somewhere you have not
   read — a rebase, a hook's autofix, a sibling
   session — and `--stat` first when the question is
   only *what moved*.

1. Draft a concise commit message:

   - Summary line in imperative voice, capital
     first letter, no trailing period.
   - Optional body explaining the *why* (not the
     *what*), wrapped at 72 chars.
   - **Do not** include a `Co-Authored-By:`
     trailer, a "Generated with …" footer, or
     any other attribution. The commit must
     look like a regular hand-authored commit.

1. Commit, **signed**:

   ```sh
   git commit -S -m "<message>"
   ```

   The `-S` is mandatory — branch protection on
   this repo requires every commit to have a
   verified signature.

   **A `failed to fill whole buffer` error means the
   1Password SSH agent is locked** — it is not a git
   or a key problem, and the
   message says nothing to suggest otherwise. It
   appears mid-session, after earlier commits in
   the same run signed fine, because the agent
   locks on its own timer. Nothing an agent can do
   fixes it: ask the user to unlock 1Password, then
   retry the same commit. One run stalled on two
   consecutive attempts before this was diagnosed.

   **Treat a failed signing attempt as an
   unpushed-state alarm, and say so loudly.** A
   blocked commit leaves verified work sitting
   uncommitted for as long as the signer stays
   locked — and that is precisely the window in
   which a worktree has twice been removed from
   under a session, the second time losing a
   tested review fix that had to be reauthored
   from the transcript. The interim mitigation for
   that hazard is **load-bearing**: the
   checkpoint-commit convention is the only reason
   the first instance lost nothing, because every
   commit had already been pushed. A locked signer
   is exactly what prevents it working.

   So on a signing failure, do not simply retry
   quietly. State plainly that there is
   **uncommitted work at risk**, name what is
   unsaved, and ask the user to unlock. If the
   session is about to wait on anything long (a
   sub-agent fan-out, a CI wait, a backoff), say
   that the wait is happening with work unsaved —
   an unattended stretch is how the window gets
   long enough to matter.

   **And a message file for `git commit -F` is
   session-scoped.** The scratchpad does not
   survive a restart, so a message staged behind a
   *blocked* commit is the thing that gets lost —
   one run restarted twice and had to re-author the
   same message verbatim both times. Before
   retrying a failed `-F` commit, check the file
   still exists and re-write it if not. A commit
   that *succeeded* has its message in git and
   needs nothing.

1. Push to the branch's upstream:

   ```sh
   git push
   ```

   If that fails because the branch has no
   upstream yet, get the branch name on its own
   and pass it to `git push -u` literally (no
   command substitution, no redirect, no `||`
   compound — each call reduces to a stable
   allow-rule):

   ```sh
   git branch --show-current
   ```

   ```sh
   git push -u origin <branch>
   ```

1. Print the commit hash, short summary, and push
   result.
