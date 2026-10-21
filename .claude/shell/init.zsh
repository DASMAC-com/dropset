#!/usr/bin/env zsh
# cspell:word subshell
# cspell:word localoptions
# cspell:word localtraps
# cspell:word ncpu
#
# Dropset session helpers — source this from ~/.zshrc:
#
#   [[ -r ~/repos/dropset/.claude/shell/init.zsh ]] &&
#     source ~/repos/dropset/.claude/shell/init.zsh
#
# Source the BASE checkout's copy, mirroring how Claude Code resolves
# settings.local.json: exactly one live version exists, and the copy inside any
# worktree is inert. Guard the line so a moved or missing checkout costs a
# no-op rather than a broken shell.
#
# WHY THIS FILE EXISTS. Every helper below used to live only as a reference
# implementation in docs/conventions/local-integrations.md that the operator
# hand-copied into an untracked ~/.zshrc — the same failure class as a guard
# hook with no wiring: documented, executable nowhere, drifting silently with
# nobody able to see the drift. One of those copies had been wrong the whole
# time (see `plan` below). Committing the functions makes the doc describe
# something that actually runs.
#
# WHAT CANNOT RIDE THIS FILE: the guard hooks' settings.json wiring. That is
# JSON read by the harness, not shell read by zsh, so sourcing this changes
# nothing about it — `make hook-wiring` remains the answer there.
#
# SECRETS BOUNDARY. This file is committed, so it carries no real 1Password
# coordinates — only the placeholder shapes. `_ds_secrets` resolves the real
# vault and item names from an untracked file outside the repo (see below).
# op:// references are pointers rather than values, so naming them here would
# not leak a credential, but it would publish the layout of a personal secret
# store into permanent git history, which is exactly what the convention keeps
# out.
#
# CONTRACT: sourced, never executed. The helpers must change the calling
# shell's directory and environment, which a subshell could not do.

if [[ -n "$BASH_VERSION" ]]; then
  # `echo … >&2`, not `print -u2`: `print` is a zsh builtin, so under bash the
  # message explaining the problem would itself fail with "print: command not
  # found". The guard still worked either way — bash parses command by command,
  # so it returns before reaching the zsh-only expansion below — but it worked
  # without ever telling the operator why.
  echo 'dropset shell helpers: zsh only' >&2
  return 1 2>/dev/null || exit 1
fi

# The repo root, derived from this file's own location rather than hardcoded,
# so a checkout at a different path needs no edit. `%x` is the sourced file;
# `:A` resolves it absolutely through symlinks; three `:h` climb
# .claude/shell/init.zsh back to the repo root.
_DS_REPO="${${(%):-%x}:A:h:h:h}"

# Sourcing a WORKTREE's copy would make every helper below treat that worktree
# as the base repo — `cdds` lands in it, `task resume` looks for worktrees nested
# inside it. It fails quietly and plausibly, which is the worst way to fail, so
# say it out loud. (The file is identical in every checkout; only which copy
# gets sourced matters.)
if [[ "$_DS_REPO" == */.claude/worktrees/* ]]; then
  print -u2 "dropset shell helpers: sourced from a worktree copy" \
    "($_DS_REPO) — source the base checkout's copy instead"
fi

# THE CARGO POOL: every cargo in a dropset checkout — the base repo or any
# worktree under it — shares one build directory and asks for at most the cores
# minus two. A `chpwd` hook, not a lazy call like `_ds_secrets`, because the
# point is that every shell in a checkout has it — a plain tab, a session, and
# every Bash command a session's agent runs (the verbs `cd` into the repo before
# launching, so the client inherits it) — with no Makefile in the loop.
# Unbounded, each worktree's cargo planned for the whole machine and kept its
# own 20–30 GB target/ tree; shared, dependencies compile once, cargo's lock on
# the directory makes concurrent builds take turns, and the job bound caps
# anything outside that lock.
#
# SCOPED TO DROPSET, never machine-wide: in another Anchor repo the pool is
# actively harmful — anchor reads the program keypair from the target dir's
# deploy/ and creates a random one when it is missing there, so `anchor keys
# sync` would re-key that repo. Leaving a checkout unsets only what the hook
# itself exported, so a value pinned by hand survives.
#
# `DS_CARGO_POOL=0` in the runtime config turns it off, and must be set BEFORE
# the line that sources this file. An already-exported value of either variable
# wins, the same override shape as `_ds_secrets`. The directory sits under Cargo
# home, beside the registry, so it is cleaned with the rest of Cargo's state.
# See docs/conventions/local-integrations.md → "The cargo pool".
# Each variable the hook exported, mapped to the value it exported. Leaving a
# checkout unsets a variable only while it still holds that value, so a pin
# made by hand after the export survives too. Declared, never reset:
# `_ds_reload_init` re-sources this file after a pull, and emptying the map
# then would orphan the exports it owns.
typeset -gA _DS_CARGO_POOL_OWNED
_ds_cargo_pool() {
  local cores var
  if [[ "${DS_CARGO_POOL:-1}" == 0 || "${PWD:A}/" != "$_DS_REPO/"* ]]; then
    for var in "${(k)_DS_CARGO_POOL_OWNED[@]}"; do
      [[ "${(P)var-}" == "${_DS_CARGO_POOL_OWNED[$var]}" ]] && unset "$var"
    done
    _DS_CARGO_POOL_OWNED=()
    return 0
  fi
  if [[ -z "${CARGO_TARGET_DIR-}" ]]; then
    export CARGO_TARGET_DIR="${CARGO_HOME:-$HOME/.cargo}/target/dropset"
    _DS_CARGO_POOL_OWNED[CARGO_TARGET_DIR]="$CARGO_TARGET_DIR"
  fi
  # Probed only when the bound is unset, so a `cd` within a pooled checkout
  # forks nothing.
  [[ -n "${CARGO_BUILD_JOBS-}" ]] && return 0
  cores="$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null)"
  if [[ "$cores" == <-> ]] && (( cores > 2 )); then
    export CARGO_BUILD_JOBS="$(( cores - 2 ))"
    _DS_CARGO_POOL_OWNED[CARGO_BUILD_JOBS]="$CARGO_BUILD_JOBS"
  fi
}
autoload -Uz add-zsh-hook
add-zsh-hook chpwd _ds_cargo_pool
_ds_cargo_pool

# How recently a fast-forward must have happened for the next one to be skipped.
#
# This is not a micro-optimization. `fleet go` opens many tabs at once and each
# runs a session verb, so without a throttle they race for the base repo's
# `index.lock` and print git errors over one another — a pull is a checkout, not
# just a fetch, so it takes the lock. A minute is far shorter than a working
# session and long enough to collapse a whole fleet launch into one pull.
_DS_PULL_THROTTLE_SECONDS=60

# Fast-forward the BASE checkout's `main` so anything starting from it starts on
# current code. Called by `cdds` and by `_ds_base`, so every verb in the family
# inherits it: entering the repo — or entering a worktree *from* the repo —
# means having the latest code. Standing operator direction.
#
# Four properties, each load-bearing:
#
#   * **Fast-forward only.** It must never merge or rebase local work. A
#     divergence is reported and otherwise left alone; resolving it is the
#     operator's call, not a side effect of navigation.
#   * **The base checkout only, never a worktree.** A worktree's checkout is a
#     work branch, and pulling inside it would mutate that branch. Worktrees
#     share the object store, so fast-forwarding the base is precisely what
#     makes fresh `origin` refs reachable from them — the whole benefit at none
#     of the risk. On any branch other than `main` this fetches and stops.
#   * **Quiet on success, loud on trouble, fatal never.** A dead network, an
#     expired credential or a diverged `main` must not brick session startup, so
#     every failure path warns and returns 0. That is what makes it safe to hang
#     off a navigation command.
#   * **Bounded.** A stalled transfer is capped by git's own low-speed limit
#     rather than an external `timeout`, which macOS does not ship.
#
# This REPLACES an earlier deliberate choice not to pull here, whose stated
# objection was that a navigation command should not make a network call. The
# objection is answered rather than simply overridden: the call is quiet,
# bounded, throttled and non-fatal, so a bare `cdds` still lands where it says
# it lands and still looks like it succeeded.
# The last pull's outcome, for diagnosis. Set on every path so the three
# indistinguishable-looking cases can be told apart after the fact:
#
#   not-a-repo | throttled | fetched | ok | failed
#
# This exists because an operator reported that `cdds` "did not pull" and the
# observation had three candidate causes that all look identical from outside:
# a stale shell still running pre-pull definitions, a throttled skip (silent by
# design), and a silent failure whose one-line warning scrolls past above a
# screen of session output. Recording the outcome makes the second and third
# distinguishable, and the self-refresh below removes the first.
_DS_PULL_LAST_OUTCOME=""
_DS_PULL_LAST_ERROR=""

# Re-source the helpers when a pull moved them, so an operator's already-open
# terminals pick up verb changes without a new tab. Nothing else refreshed
# them, which made "my shell is stale" the most likely explanation for any
# reported verb misbehavior — and the hardest to tell from a real bug.
#
# $1 = the pre-pull HEAD. The question "did this pull change init.zsh?" is asked
# of GIT, not of the filesystem: an mtime comparison was the first attempt and
# it is wrong, because `stat` reports whole seconds and a fast-forward
# completing in the same second as the check reads as unchanged. A commit range
# is exact and costs one more git call on the only path that can need it.
_ds_reload_init() {
  local head_before="$1" init="$_DS_REPO/.claude/shell/init.zsh" changed
  [[ -n "$head_before" && -f "$init" ]] || return 0
  changed="$(git -C "$_DS_REPO" diff --name-only "$head_before" HEAD \
    -- .claude/shell/init.zsh 2>/dev/null)"
  [[ -n "$changed" ]] || return 0
  # Recursion guard. Sourcing the file redefines these functions while one of
  # them is executing, which is legal in zsh, but a future top-level statement
  # in init.zsh that called a verb would re-enter the pull. Cheap insurance
  # against a change made far from here.
  [[ -n "$_DS_RELOADING" ]] && return 0
  _DS_RELOADING=1
  # shellcheck disable=SC1090
  source "$init"
  unset _DS_RELOADING
  print -u2 "dropset: session helpers reloaded (init.zsh changed)"
}

# The wrapper exists so the debug line runs on EVERY path, including the early
# returns. `DS_PULL_DEBUG=1 cdds` is how a "it didn't pull" report gets
# answered without guessing which of the three causes it was.
_ds_pull() {
  _ds_pull_impl
  if [[ -n "$DS_PULL_DEBUG" ]]; then
    print -u2 "dropset: pull ${_DS_PULL_LAST_OUTCOME}${_DS_PULL_LAST_ERROR:+ — $_DS_PULL_LAST_ERROR}"
  fi
  return 0
}

_ds_pull_impl() {
  _DS_PULL_LAST_OUTCOME="not-a-repo"
  _DS_PULL_LAST_ERROR=""
  [[ -d "$_DS_REPO/.git" ]] || return 0

  # Claim the throttle slot BEFORE pulling, not after. Two tabs launched in the
  # same instant would otherwise both read a stale stamp and both pull, which is
  # the lock contention this exists to prevent.
  local stamp="$_DS_REPO/.git/.ds-last-pull"
  local now last
  now="$(date +%s)"
  # `== <->` (all-digits), not `-n`. zsh math context RE-EVALUATES a non-numeric
  # parameter value as an arithmetic expression, which can assign and can index
  # arrays — so a garbage stamp is not merely wrong, it is evaluated. Nothing
  # but this function writes the file, so the risk is theoretical; the guard
  # costs one token and removes the class.
  if [[ -f "$stamp" ]]; then
    last="$(cat "$stamp" 2>/dev/null)"
    if [[ "$last" == <-> ]] && (( now - last < _DS_PULL_THROTTLE_SECONDS )); then
      # Silent by design — but RECORDED, because a throttled skip and a
      # successful quiet pull were previously byte-identical from outside, and
      # that ambiguity is what made the operator's report unfalsifiable.
      _DS_PULL_LAST_OUTCOME="throttled"
      return 0
    fi
  fi
  print -r -- "$now" >| "$stamp" 2>/dev/null

  # Captured before the pull so `_ds_reload_init` can ask git what moved.
  local head_before
  head_before="$(git -C "$_DS_REPO" rev-parse HEAD 2>/dev/null)"

  local branch err
  branch="$(git -C "$_DS_REPO" symbolic-ref --quiet --short HEAD 2>/dev/null)"

  # GIT_TERMINAL_PROMPT=0 is what makes "fatal never" true. The low-speed knobs
  # bound a stalled TRANSFER, but not an interactive credential prompt: an
  # expired keychain entry makes git block on /dev/tty asking for a username,
  # and with stderr silenced that reads as a hang. Since every session verb now
  # runs this, that would hang session startup itself — the one outcome this
  # helper must never produce. With it set, a missing credential fails fast and
  # takes the warning path below.
  # stderr is CAPTURED rather than discarded, so a failure can report git's own
  # last line. The previous generic sentence made a diverged base, a dirty
  # tree, an expired credential and an offline network all read identically —
  # so the warning told the operator only that something went wrong, which is
  # the part they could already see.
  if [[ "$branch" != "main" ]]; then
    if err="$(GIT_TERMINAL_PROMPT=0 git -c http.lowSpeedLimit=1000 \
      -c http.lowSpeedTime=10 -C "$_DS_REPO" fetch --quiet origin main 2>&1)"; then
      _DS_PULL_LAST_OUTCOME="fetched"
    else
      _DS_PULL_LAST_OUTCOME="failed"
      _DS_PULL_LAST_ERROR="${${err##*$'\n'}:-no detail from git}"
      print -u2 "dropset: could not fetch origin/main — $_DS_PULL_LAST_ERROR"
    fi
    return 0
  fi

  if err="$(GIT_TERMINAL_PROMPT=0 git -c http.lowSpeedLimit=1000 \
    -c http.lowSpeedTime=10 -C "$_DS_REPO" pull --ff-only --quiet 2>&1)"; then
    _DS_PULL_LAST_OUTCOME="ok"
    _ds_reload_init "$head_before"
  else
    _DS_PULL_LAST_OUTCOME="failed"
    _DS_PULL_LAST_ERROR="${${err##*$'\n'}:-diverged, dirty or offline}"
    print -u2 "dropset: could not fast-forward main — $_DS_PULL_LAST_ERROR" \
      "Continuing on the current checkout."
  fi
}

# `cd` to the base repo checkout. The starting point for anything that must not
# run inside a worktree (`housekeeping`, a planning session).
#
# It fast-forwards `main` on the way in, via `_ds_pull` — see there for why that
# is safe on a navigation command, and why it never touches a worktree's own
# branch. With a tag, the pull happens in the base repo BEFORE the `cd`, so the
# worktree gets fresh `origin` refs through the shared object store without its
# work branch being touched.
# Takes an OPTIONAL tag: bare `cdds` lands in the base repo, `cdds 1077` lands
# in that issue's worktree. Dropping the argument was a parity gap and a
# silent one — the committed version ignored it and reported success from the
# base repo, which is the worst way to fail: every later edit then targets the
# base copy the worktree build never sees, the exact slip the worktree
# edit-path guard exists to catch downstream. Accepts `1077` or `eng-1077`,
# matching `task resume`, and reaches a pre-rename `eng-` / `ceo-` / `exp-`
# directory through `_ds_worktree_of`.
cdds() {
  cd "$_DS_REPO" || return 1
  _ds_pull
  [[ -z "$1" ]] && return 0

  local tag dir
  tag="$(_ds_tag_of "$1")"
  if ! dir="$(_ds_worktree_of "$tag")"; then
    print -u2 "cdds: no worktree for $tag under $_DS_REPO/.claude/worktrees"
    return 1
  fi
  cd "$_DS_REPO/.claude/worktrees/$dir"
}

# Internal: the same move, for helpers that must launch from the base repo
# rather than trusting the shell's cwd.
#
# It does not restore the previous directory, so a session helper leaves the
# calling shell in the base repo after the session exits. That is a real side
# effect and worth knowing — quit a session started from a worktree and the
# next command runs in the base checkout, which is the slip the worktree
# edit-path guard exists to catch. Left as-is deliberately: a subshell would
# discard the `_ds_secrets` exports these helpers exist to set.
#
# It fast-forwards `main` too, so every session verb that launches from the base
# repo — including the worktree ones, which start here before `claude -w` — gets
# current code. `_ds_pull`'s throttle is what keeps a fleet launch from turning
# that into N racing pulls.
_ds_base() {
  cd "$_DS_REPO" || return 1
  _ds_pull
}

# Resolve LINEAR_API_KEY and GITHUB_MCP_PAT from 1Password.
#
# The coordinates — and only the coordinates — come from three shell variables:
#
#   DS_OP_ACCOUNT='<account>.1password.com'
#   DS_OP_LINEAR_REF='op://<vault>/<linear-item>/credential'
#   DS_OP_GITHUB_REF='op://<vault>/<github-item>/credential'
#
# **Define them in the untracked runtime config**, alongside the `LINEAR_*` ids
# that already live there. ONE personal config file, and only one: the separate
# coordinates file existed to keep *scripts* out of the shell profile, and with
# the function bodies committed here there is nothing left to keep out. Its
# opt-in path was removed rather than left dormant — a second supported location
# for the same three variables is a place for them to disagree, and the resulting
# failure is silent (a stale copy wins and the wrong credential resolves). The
# secrets boundary is unchanged — the runtime config is equally outside the
# repo, and anything tracked carries placeholder shapes only.
#
# Four things about the shape below are load-bearing:
#
#   * Resolution is lazy — at session launch, not at shell init. `op read`
#     costs a round trip and can raise a Touch ID prompt, and every plain
#     terminal tab would otherwise pay both for secrets it never uses. Only the
#     session helpers call this, so an ordinary tab stays instant.
#   * The `${VAR:-…}` guard makes it at most one fetch per shell, so helpers
#     that chain into one another don't re-prompt. It also lets an
#     already-exported value win — the override path when a key is pinned by
#     hand, and the escape hatch when no coordinates are set at all.
#   * `--account` is explicit because the laptop is signed into more than one
#     1Password account, and a bare `op read` cannot disambiguate.
#   * An unresolved secret WARNS rather than failing the launch. An empty key
#     otherwise surfaces much later as an opaque MCP error mid-session, which
#     is far worse to debug than one line at startup.
_ds_secrets() {
  if [[ -n "$DS_OP_ACCOUNT" && -n "$DS_OP_LINEAR_REF" ]]; then
    export LINEAR_API_KEY="${LINEAR_API_KEY:-$(op read --account \
      "$DS_OP_ACCOUNT" "$DS_OP_LINEAR_REF")}"
  fi
  if [[ -n "$DS_OP_ACCOUNT" && -n "$DS_OP_GITHUB_REF" ]]; then
    export GITHUB_MCP_PAT="${GITHUB_MCP_PAT:-$(op read --account \
      "$DS_OP_ACCOUNT" "$DS_OP_GITHUB_REF")}"
  fi

  [[ -z "$LINEAR_API_KEY" ]] &&
    print -u2 '_ds_secrets: LINEAR_API_KEY unresolved'
  [[ -z "$GITHUB_MCP_PAT" ]] &&
    print -u2 '_ds_secrets: GITHUB_MCP_PAT unresolved'
  return 0
}

# Ensure this shell holds a USABLE AWS session before a seat verb launches, and
# refuse the launch if it does not.
#
# Why a gate rather than a mid-session fix: the login is **interactive**, so a
# session that discovers an expired token mid-conversation cannot self-heal.
# Measured — a planning session's Cost Explorer read failed on an expired
# session and the spend report had to be deferred a day. Launch time is the one
# moment interactivity is free: the operator is at the keyboard typing the verb
# anyway.
#
# Four things about the shape below are load-bearing:
#
#   * It **probes before logging in**. `aws login` stops for the operator, so
#     a still-valid session must not pay for one. `sts get-caller-identity` is the
#     cheapest call that actually proves the credentials resolve, and it is what
#     fails with `Your session has expired` when they do not.
#   * **The probe is what decides, not the login's exit status.** `aws login` can
#     exit 0 having left a profile that still cannot call STS, so the gate
#     re-probes afterwards and trusts only that. The invariant defended is
#     "this session's credentials RESOLVE", never "a login ran".
#
#     Be precise about that invariant, because the obvious stronger phrasing is
#     wrong: `sts:GetCallerIdentity` is authorization-free, succeeding for any
#     signature that verifies, so a green gate does **not** prove Cost Explorer
#     is readable — on this account cost access is separately gated (it needs a
#     root-only billing toggle, and `PowerUserAccess` does not cover
#     everything). The gate rules out an EXPIRED session, which is the measured
#     failure; it does not rule out a live session lacking `ce:*`.
#
#     It also checks validity NOW, not remaining lifetime: a token with a minute
#     left passes, and SSO tokens are hours-scoped while a planning session is
#     long-lived and resumable. So this narrows the mid-conversation failure
#     rather than closing it. A stricter form would read the SSO cache's
#     `expiresAt` and re-login below a threshold; that is deliberately not built
#     yet.
#   * **The profile comes from the untracked runtime config**, never from here.
#     `DS_AWS_PROFILE` is honored when set; unset, the CLI resolves its own
#     default, which is the common case. Naming a profile in a committed file
#     would hard-code one machine's SSO config — and this account's profile
#     names carry the account id.
#   * **The login is `--remote`, for every profile.** The plain flow opens the
#     default browser, which silently signs in as whichever identity that
#     browser already holds — an admin login can complete as the PowerUser and
#     then fails at role-credential fetch. `--remote` prints the URL and takes
#     the code back by paste, so the operator chooses the browser (incognito if
#     need be) and with it the identity. The operator is at the keyboard
#     anyway, so the paste costs nothing.
#
# `aws login` is the spelling this CLI itself prescribes: measured on
# aws-cli/2.35.22, an expired session fails with "Please reauthenticate using
# 'aws login'". `aws sso login` is the older spelling and is deliberately not
# coded as a fallback — an unverified command in a committed launcher is worse
# than a clear failure, and `docs/conventions/aws-infra.md` already prescribes
# `aws login` for the MCP wiring.
#
# **An absent CLI warns rather than blocking.** The gate exists to catch an
# EXPIRED token, which is the measured failure; a machine with no `aws` at all
# has nothing to log into, and refusing to start a planning session there would
# make the committed verb unusable on any checkout without AWS. A failed or
# dismissed login is a different condition and does stop the launch — that is a
# live credential problem the operator can act on.
_ds_aws_login() {
  local verb="$1"
  local -a profile=()
  [[ -n "$DS_AWS_PROFILE" ]] && profile=(--profile "$DS_AWS_PROFILE")

  if ! command -v aws >/dev/null 2>&1; then
    print -u2 "$verb: no \`aws\` CLI on PATH — launching WITHOUT cost-read" \
      "access. Install AWS CLI v2 to restore the login gate."
    return 0
  fi

  if aws sts get-caller-identity "${profile[@]}" >/dev/null 2>&1; then
    return 0
  fi

  print -u2 "$verb: AWS session expired or absent — logging in before launch."
  aws login --remote "${profile[@]}"

  if aws sts get-caller-identity "${profile[@]}" >/dev/null 2>&1; then
    return 0
  fi

  # The re-probe failed, so this is a refusal — UNLESS this `aws` is simply too
  # old to have a top-level `login` at all, which is the same "AWS is not set up
  # here" shape as no `aws` and takes the same warn-and-launch branch. Blocking
  # it would be strictly worse than having no CLI, which launches fine.
  #
  # **This check sits here, on the already-failing path, and not before the
  # login where it reads more naturally.** The placement IS the safety property.
  # `help` renders through groff and a pager — the one part of the CLI that can
  # fail for reasons having nothing to do with whether a subcommand exists (no
  # `groff`, a misconfigured `AWS_PAGER`, a pager that exits non-zero on closed
  # stdout). Deciding the hot path on that call means a broken pager on a
  # CURRENT CLI silently warn-and-launches every expired session, which is
  # exactly the failure this gate exists to catch. Placed here it can only ever
  # widen an outcome that is already a refusal. `AWS_PAGER=` removes the
  # likeliest cause on top of that.
  #
  # The cost of the later placement is one stray `Invalid choice: 'login'` from
  # the attempted login on a genuinely old CLI — worth paying to keep a fragile
  # call off the path that matters, and the message below says to ignore it.
  if ! AWS_PAGER= aws login help >/dev/null 2>&1; then
    print -u2 "$verb: this \`aws\` has no top-level \`login\` command (needs a" \
      "recent AWS CLI v2; measured on 2.35.22) — launching WITHOUT cost-read" \
      "access, and ignore any \`Invalid choice\` above. Upgrade the CLI, or run" \
      "the older \`aws sso login --use-device-code\` by hand first."
    return 0
  fi

  # Name BOTH causes. The probe cannot tell an expired session from an
  # unreachable STS, so a session with perfectly good credentials that is
  # merely offline lands here — and a message naming only the profile sends
  # the operator to the wrong knob.
  print -u2 "$verb: AWS credentials still do not resolve, so this session is" \
    "NOT launching — it would hit the same error mid-conversation with no way" \
    "to fix it from inside. Either the login did not complete, or AWS was" \
    "unreachable (offline, captive portal, STS throttle). Re-run \`$verb\`" \
    "once \`aws login --remote\` succeeds, or check DS_AWS_PROFILE in the runtime" \
    "config."
  return 1
}

# Internal: a deterministic per-day session UUID, seeded by kind + full date.
#
# The full date is in the seed so that `plan-18` in August and `plan-18` in
# September cannot collide; the display name stays day-only by operator choice,
# and this id is what actually disambiguates them. The kind prefix keeps a
# day's planning and housekeeping sessions apart for the same reason.
#
# An optional $2 is the planning CYCLE: N > 0 appends `-cN`, a fresh id under
# the same display name (see `plan`). Cycle 0 is the bare seed, so ids computed
# before cycling existed are unchanged. An optional $3 is the date (YYYYMMDD),
# so `plan` hashes the launch day it fixed rather than sampling `date` again.
# resolve_session.py's `daily_session_id` mirrors this seed, and
# .claude/scripts/test_plan_cycle_loop.py pins the two against each other.
_ds_daily_sid() {
  local raw seed="dropset-$1-${3:-$(date +%Y%m%d)}"
  (( ${2:-0} > 0 )) && seed+="-c$2"
  if (( $+commands[md5] )); then
    raw="$(printf '%s' "$seed" | md5 -q)"
  else
    raw="$(printf '%s' "$seed" | md5sum)"
    raw="${raw%% *}"
  fi
  print -r -- \
    "${raw:0:8}-${raw:8:4}-${raw:12:4}-${raw:16:4}-${raw:20:12}"
}

# Internal: start-or-resume today's session of one kind, in the base repo.
#
# THE IDEMPOTENCY IS FORCED, NOT OVER-ENGINEERED. It would be simpler to ask
# the CLI whether a named session exists, and there is no way to: `claude` has
# no session-listing flag in any spelling, `-n/--name` sets only a DISPLAY
# name, and `-r/--resume` takes a session ID (a bare string opens an
# interactive picker filtered by that term — which is not a resume). So the
# only deterministic handle is a session id we compute ourselves, with the
# on-disk transcript as the existence check.
#
#   $1 kind (seeds the id, e.g. `plan`)   $2 display name   $3 initial prompt
#   $4 model to pin, resolved by `_ds_tier`
#
# The model rides BOTH branches; the name and the initial prompt ride only the
# create path. That split is the point, and getting it wrong is silent: `-n`
# sets a display name and the prompt bootstraps a skill, so re-passing either
# on a resume is meaningless — but the model is per-launch, and a planning
# session is reopened many times a day. Pinning it only on create would honor
# the pin on the day's FIRST launch and quietly drop to the saved default on
# every reopen after it — the "still works, so nobody notices" slip.
_ds_daily_session() {
  _ds_session "$(_ds_daily_sid "$1")" "$2" "$3" "$4"
}

# Internal: the start-or-resume core, given an already-computed session id.
#
# Split out from `_ds_daily_session` so a session keyed by something other than
# the date can reuse it unchanged. `plan` and `housekeeping` key on the day; `architect` keys
# on its ISSUE, because a design thread outlives a day and resuming it tomorrow is
# the whole point. Everything below the id — the idempotency, the model pin
# riding both branches, the permission mode — is identical for both, and the
# operator's stated abstraction is that these launchers differ only in the
# briefing.
#
#   $1 session id   $2 display name   $3 initial prompt
#   $4 model to pin, resolved by `_ds_tier` — required
#   $5 worktree tag, or "" to run in the base checkout
#
# THE MODEL TRAVELS AS `ANTHROPIC_MODEL`, SCOPED TO THE ONE `claude` COMMAND —
# not as a `--model` flag, and not exported. The environment is the delivery
# the Bedrock path already used, so both substrates now pin the same way; and a
# command-scoped assignment cannot outlive the session into the tab, so a bare
# `claude` typed there afterwards is unmanaged rather than silently pinned.
# The caller has already put the shell on its substrate (`_ds_substrate_enter`).
#
# THE WORKTREE FLAG RIDES THE CREATE BRANCH ONLY. `-w` *creates* a worktree, so
# passing it on the resume branch would ask for a second one every time a
# long-lived thread is reopened. The asymmetry is the point, not an oversight.
#
# THE EXISTENCE CHECK IS BY ID, NOT BY SLUG. It used to build one path from the
# base repo's slug, on the belief that `claude -w <tag>` files its transcript
# there. Current Claude Code files it under the WORKTREE's slug instead
# (measured: a `-w` session's transcript sits only under
# `…-claude-worktrees-<tag>`), so every reopen of a worktree-launched architect
# or explore session missed, took the create branch, and re-ran the bootstrap
# prompt on top of the existing history. The id is a UUID we computed, so a
# glob over every project directory answers "does this session exist" without
# knowing the filing rule — older sessions filed under the base slug included.
# A worktree-slug hit resumes from inside the worktree, the directory Claude
# Code addresses it by; anything else resumes from the base checkout as before
# (whether `--resume <id>` from there reaches a copy under some THIRD slug is
# unverified, but the create branch on an existing id was no better).
_ds_session() {
  local sid="$1" name="$2" prompt="$3" model="$4" worktree="$5"

  if [[ -z "$model" ]]; then
    print -u2 'dropset: no model resolved for this session — refusing to launch.'
    return 1
  fi

  local wt_dir wt_slug
  local -a worktree_flag prompt_arg found
  [[ -n "$worktree" ]] && worktree_flag=(-w "$worktree")
  # An ARRAY rather than a bare "$prompt", so a verb with no bootstrap skill to
  # run passes no positional at all. Spelled directly, an empty prompt reaches
  # the CLI as an empty argument, which is a different thing from omitting it.
  [[ -n "$prompt" ]] && prompt_arg=("$prompt")

  _ds_base || return 1
  _ds_secrets

  # `(N)` makes a miss an empty array rather than a glob error. The config dir
  # follows `CLAUDE_CONFIG_DIR` the way resolve_session.py's `claude_home` does.
  found=("${CLAUDE_CONFIG_DIR:-$HOME/.claude}"/projects/*/"$sid".jsonl(N))

  if (( ${#found} )); then
    # The project slug replaces every `/` and `.` in the cwd with `-` — the
    # rule resolve_session.py's `slugify` encodes. `$PWD` is the base checkout
    # here (`_ds_base`), so this is the worktree path Claude Code slugged.
    # Every hit is tested, not the first: a session once resumed from inside
    # its worktree can have a copy under BOTH slugs, and glob order is not a
    # contract. `(I)` matches per element; a quoted `(M)…:#` would match the
    # JOINED array as one string and miss whenever the base copy sorts last.
    [[ -n "$worktree" ]] && wt_dir="$PWD/.claude/worktrees/$worktree"
    wt_slug="${wt_dir//[\/.]/-}"
    if [[ -n "$wt_dir" && -d "$wt_dir" \
      && ${found[(I)*/$wt_slug/$sid.jsonl]} -gt 0 ]]; then
      cd "$wt_dir" || return 1
    fi
    # A resume passes no `-w`, so a worktree that has since been pruned is NOT
    # re-created: the session reopens with its cwd in the base checkout while
    # every path in its history points into a directory that no longer exists.
    # That is the worktree-edit-path pitfall arriving by default, and it is a
    # state only this change makes reachable — before it, neither verb had a
    # worktree, so resuming in the base repo was always right. Say it out loud
    # rather than letting it read as a normal reopen.
    if [[ -n "$worktree" ]] && ! _ds_worktree_of "$worktree" >/dev/null; then
      print -u2 "dropset: worktree $worktree is gone — resuming in the base" \
        "checkout. Re-creating it restores TRACKED files only; an untracked" \
        "spec file written there is not recoverable, so work from Linear."
    fi
    ANTHROPIC_MODEL="$model" claude --resume "$sid" --permission-mode auto
  else
    ANTHROPIC_MODEL="$model" claude --session-id "$sid" -n "$name" \
      --permission-mode auto "${worktree_flag[@]}" "${prompt_arg[@]}"
  fi
}

# Internal: set this iTerm session's `user.ds_role` variable to $1 ('' clears).
#
# A session variable rather than a title match, so a rename never breaks it, and
# rather than a /tmp file keyed by tty, so a reused tty never inherits a stale
# role. iTerm takes the value base64-encoded. A no-op outside iTerm or with no
# terminal on stdout.
_ds_iterm_role() {
  [[ "$TERM_PROGRAM" == iTerm.app && -t 1 ]] || return 0
  printf '\e]1337;SetUserVar=ds_role=%s\a' "$(print -rn -- "$1" | base64)"
}

# Internal: a deterministic per-TOPIC session UUID, seeded by kind + topic.
#
# Deliberately no date in the seed, which is the one substantive difference from
# `_ds_daily_sid`: an architect session is a long-horizon thread that is meant to
# be resumed days later. Putting the date in would silently start a fresh
# conversation each morning and lose the thread — the exact failure the verb
# exists to prevent.
_ds_topic_sid() {
  local raw
  if (( $+commands[md5] )); then
    raw="$(printf 'dropset-%s-%s' "$1" "$2" | md5 -q)"
  else
    raw="$(printf 'dropset-%s-%s' "$1" "$2" | md5sum)"
    raw="${raw%% *}"
  fi
  print -r -- \
    "${raw:0:8}-${raw:8:4}-${raw:12:4}-${raw:16:4}-${raw:20:12}"
}

# ---------------------------------------------------------------------------
# Substrate: which provider a session runs against.
#
# THE RULE IS CAPABILITY, NOT ATTENDANCE. A session runs on Bedrock unless it
# needs something Bedrock lacks — web search, web fetch, deep research — or it
# is a seat session by role (`plan`, `architect`, `housekeeping`, `explore`).
# Sub-agents are NOT a differentiator: Opus-spawning-Opus sub-agents are
# verified working on Bedrock.
#
# An earlier framing split on attendance (unattended work goes to Bedrock) and
# was superseded: who is watching says nothing about which tools the session
# needs, and the verbs that actually broke on Bedrock broke on capability.
# ---------------------------------------------------------------------------

# ---------------------------------------------------------------------------
# Model tiers: which model a verb launches, named by ROLE, never by model.
#
#   ADVISOR      plan, architect, explore        default substrate: anthropic
#   EXECUTOR     task, housekeeping              default substrate: bedrock
#   HELPER       chore                           default substrate: bedrock
#   BACKGROUND   Claude Code's small-model slot  follows the session's substrate
#
# The names are capability-per-role on purpose, after the advisor/executor
# pairing in Claude's own docs: the advisor tier decides, the executor tier
# carries the work out, and the helper tier does the minor non-repo jobs that
# merit neither (a Sonnet-class model is the natural value). A family name
# (opus, haiku) drifts the first time the role changes family, and Claude
# Code owns those as its own alias slots; a substrate name ("seat") breaks the first time the top
# model leaves the subscription. The verb → tier table is the code below, and
# the tier → model + substrate table is the operator's untracked runtime config:
#
#   DS_MODEL_ADVISOR,  DS_MODEL_ADVISOR_SUBSTRATE
#   DS_MODEL_EXECUTOR, DS_MODEL_EXECUTOR_SUBSTRATE
#   DS_MODEL_HELPER,   DS_MODEL_HELPER_SUBSTRATE
#   DS_MODEL_BACKGROUND                (applied on Bedrock launches only)
#
# NO MODEL ID IS COMMITTED, deliberately, and there is no fallback. A model
# ships every couple of months and the point of the tiers is to hot-swap it, so
# the one place a model is pinned is the runtime config; the repo carries the
# variable names and the schema only. A tier left unset refuses to launch and
# names the variable, rather than quietly running a committed id that has gone
# a generation stale — the slip that motivated the tiers in the first place.
#
# ONE MODEL ID SERVES BOTH SUBSTRATES. Claude Code maps a first-party id
# (`claude-<family>-<version>`) to the `us.anthropic.` cross-region profile when
# it runs on Bedrock (measured from its request log), so a substrate override
# flips the provider without a second spelling. A Bedrock-form id still works on
# Bedrock; it just stops being portable.
#
# The BACKGROUND slot is Claude Code's small/fast model, exported as
# `ANTHROPIC_DEFAULT_HAIKU_MODEL`. Its docs name session title generation as the
# example background task and give no fuller list, so this claims no more; a
# failure there degrades a nicety, never task output. Pinned on Bedrock launches
# only — on the subscription Claude Code's own default is right — and unset
# there it is an informational line, not a refusal, since per the same docs a
# Bedrock session with `ANTHROPIC_MODEL` set routes background tasks to that
# model. Claude Code maps a first-party id in this slot to its `us.anthropic.`
# profile, as it does the primary model's (measured from its request log), so a
# first-party id is portable; a Bedrock-form id must be the exact profile id.
#
# The auto-mode permission classifier is deliberately absent: Claude Code picks
# its model itself and exposes no configuration for it, so it has no tier.
# ---------------------------------------------------------------------------

# Where a session's substrate choice is recorded. See `_ds_substrate_write`.
_DS_SUBSTRATE_DIR="$_DS_REPO/.claude/session-substrate"

# Internal: normalize a worktree tag. An issue reference becomes its BARE
# NUMBER — `882`, `eng-882`, and the retired role spellings `ceo-882` and
# `exp-882` all print `882` — so every issue-keyed session, tab and worktree
# directory is named by the number alone (operator ruling: the prefix is dead
# screen estate, and the number is what the operator looks up). Anything else
# passes through lowercased, which keeps a free-form `exp-<name>` usable. Git
# BRANCHES are the one place the `eng-` prefix survives: `init-pr` renames to
# it, because Linear's GitHub integration links on that form.
#
# `10#` forces base ten, so a padded `0123` names issue 123, not octal 83.
#
# ONE owner, because the start and resume sides MUST agree. They did not: the
# start side let a non-`eng` name through literally while the resume side did
# `eng-${1#eng-}`, which force-prefixes. So `task my-thing` recorded its
# substrate under `my-thing` and `task resume my-thing` looked for
# `eng-my-thing`, missed, and silently resumed on the seat — a miss in exactly
# the direction the design calls silent. Deriving both from here makes the
# agreement structural rather than something two call sites have to remember.
_ds_tag_of() {
  local tag="${1:l}"
  [[ "$tag" == (eng|ceo|exp)-<-> ]] && tag="${tag#*-}"
  [[ "$tag" == <-> ]] && tag=$(( 10#$tag ))
  print -r -- "$tag"
}

# Internal: the worktree directory that holds a tag, or non-zero when none does.
#
# The bare tag first, then the spellings worktrees were created under before
# the bare-number rename — `eng-<n>` for `task` and issue-keyed `explore`,
# `ceo-<n>` for an `architect` launched with a numeric topic, `exp-<n>` for an
# older explore. Those directories are grandfathered rather than moved: an
# in-flight session's transcript stamps its cwd into the old path, so renaming
# the directory would orphan the conversation. This lookup is the compatibility
# seam, and it retires itself as the old worktrees are pruned. A worded
# `ceo-<topic>` names no issue, so no number reaches it.
_ds_worktree_of() {
  local tag="$1" dir
  local -a candidates=("$tag")
  [[ "$tag" == <-> ]] && candidates+=("eng-$tag" "ceo-$tag" "exp-$tag")
  for dir in "${candidates[@]}"; do
    if [[ -d "$_DS_REPO/.claude/worktrees/$dir" ]]; then
      print -r -- "$dir"
      return 0
    fi
  done
  return 1
}

# Internal: the worktree a launch should pass to `claude -w` — the existing one
# for this tag, pre-rename spellings included, or the bare tag when none exists.
# Creating a bare `<n>` beside a live `eng-<n>` / `ceo-<n>` would split the
# thread: every resume prefers the bare directory, so the old conversation
# would go unreachable by number. It does not know which session kind is
# launching: with two pre-rename kinds on one number it takes the first in
# `_ds_worktree_of`'s order, the same cross-kind sharing the bare `<n>` accepts.
_ds_launch_dir() {
  _ds_worktree_of "$1" || print -r -- "$1"
}

# Internal: the substrate-marker key for a tag. A marker written before the
# rename sits under `eng-<n>`; reading it there keeps a pre-rename Bedrock
# session resuming on Bedrock instead of silently moving to the seat.
_ds_marker_key() {
  local tag="$1"
  if [[ "$tag" == <-> && ! -f "$_DS_SUBSTRATE_DIR/$tag" \
    && -f "$_DS_SUBSTRATE_DIR/eng-$tag" ]]; then
    tag="eng-$tag"
  fi
  print -r -- "$tag"
}

# Resolve a tier to the model and substrate a launch uses. Prints two lines,
# model then substrate. NON-ZERO MEANS DO NOT LAUNCH.
#
#   $1 tier (`advisor` | `executor` | `helper`)
#   $2 substrate override, or "" for the config
#
# This is the deterministic, pre-session half of fail-fast: a tier whose config
# does not resolve refuses here, offline and free, rather than starting a
# session that fails on its first turn. Measured, that is what the CLI does
# with an unknown id — it starts, then the first request returns `400 The
# provided model identifier is invalid`. Whether an id names a real model is a
# network question and is NOT asked per launch; `models check` asks it for free.
#
# A configured model on Bedrock carrying no window suffix WARNS and is still
# used: the override is the operator's to make, and Bedrock defaults an
# unsuffixed id to 200k without saying so.
_ds_tier() {
  local tier="$1" override="$2" model substrate var
  case "$tier" in
    advisor)
      var=DS_MODEL_ADVISOR
      model="$DS_MODEL_ADVISOR"
      substrate="${override:-${DS_MODEL_ADVISOR_SUBSTRATE:-anthropic}}"
      ;;
    executor)
      var=DS_MODEL_EXECUTOR
      model="$DS_MODEL_EXECUTOR"
      substrate="${override:-${DS_MODEL_EXECUTOR_SUBSTRATE:-bedrock}}"
      ;;
    helper)
      var=DS_MODEL_HELPER
      model="$DS_MODEL_HELPER"
      substrate="${override:-${DS_MODEL_HELPER_SUBSTRATE:-bedrock}}"
      ;;
    *)
      print -u2 "dropset: unknown model tier '$tier'"
      return 1
      ;;
  esac

  if [[ -z "$model" ]]; then
    print -u2 "dropset: $var is unset — set it to a model id in the runtime" \
      'config (the repo pins no model). Refusing to launch.'
    # The retired spelling is named rather than left as a mystery.
    if [[ "$tier" == executor && -n "$DS_BEDROCK_MODEL" ]]; then
      print -u2 'dropset: DS_BEDROCK_MODEL is retired — rename it to' \
        'DS_MODEL_EXECUTOR.'
    fi
    return 1
  fi

  if [[ "$substrate" != (anthropic|bedrock) ]]; then
    # `-r` throughout: these echo operator config, so escapes stay literal.
    print -ru2 -- "dropset: ${var}_SUBSTRATE ('$substrate') must be anthropic or" \
      'bedrock — refusing to launch.'
    return 1
  fi
  if [[ "$model" == *[[:space:]]* ]]; then
    print -ru2 -- "dropset: $var ('$model') is not a model id — refusing to launch."
    return 1
  fi
  if [[ "$substrate" == bedrock \
    && "$model" != *'[1m]' && "$model" != *'[200k]' ]]; then
    print -ru2 -- "dropset: $var ('$model') has no context-window suffix —" \
      'Bedrock will use 200k, not 1M, and will not say so.'
  fi
  print -r -- "$model"
  print -r -- "$substrate"
}

# Internal: put the calling shell on a tier's substrate. $1 substrate, $2 verb
# name for messages. Non-zero means DO NOT LAUNCH.
#
# THE ORDER IS THE SEAT PIN. An anthropic launch clears whatever an earlier
# Bedrock launch in this tab left behind FIRST, and only then does the verb hand
# its own model to `claude` — so an inherited pin can never win.
_ds_substrate_enter() {
  if [[ "$1" == bedrock ]]; then
    _ds_bedrock_env
  else
    _ds_seat_guard "$2"
  fi
}

# Record the substrate a session launched on, keyed by worktree tag.
#
# The key is a parameter rather than a hard-coded tag because a base-repo
# session would key on its computed session id — but **no verb does that
# today**, and every call site (`task`, `architect`, `chore`) passes a
# worktree tag. Said plainly
# because the earlier wording here described session-id keying as if it
# shipped, which it does not: every base-repo verb is seat-only, so none of
# them has a substrate worth recording.
#
# WHY A MARKER AT ALL: a resume must land on the substrate its session started
# on, and the slip is silent in BOTH directions. A Bedrock session resumed onto
# the seat quietly eats the 5-hour subscription window; a seat session resumed
# onto Bedrock quietly spends credits on attended work. Neither errors, so
# neither gets noticed until the bill or the window does the telling.
#
# Best-effort by design — an unwritable state directory must not fail a launch,
# so every path returns 0. The cost of a missing marker is the conservative
# default — anthropic substrate, and (see `_ds_resume_tier`) the advisor tier's
# model rather than the executor's — which is the next function.
#
# An optional $3 tier (`advisor` | `helper`) is written to `<key>.tier`;
# omitting it removes any such file, so the last launch that writes the marker decides
# its resume tier.
_ds_substrate_write() {
  local key="$1" substrate="$2" tier="$3"
  mkdir -p "$_DS_SUBSTRATE_DIR" 2>/dev/null || return 0
  print -r -- "$substrate" >| "$_DS_SUBSTRATE_DIR/$key" 2>/dev/null
  if [[ -n "$tier" ]]; then
    print -r -- "$tier" >| "$_DS_SUBSTRATE_DIR/$key.tier" 2>/dev/null
  else
    rm -f "$_DS_SUBSTRATE_DIR/$key.tier" 2>/dev/null
  fi
  return 0
}

# Read back a recorded substrate. Prints `bedrock` or `anthropic`.
#
# **Absent means anthropic**, deliberately: every session that existed before
# markers did was a seat session, and the conservative error is spending the
# subscription window rather than spending credits on something unintended. A
# garbage value — and the retired `seat` spelling — reads as anthropic for the
# same reason.
_ds_substrate_read() {
  local marker="$_DS_SUBSTRATE_DIR/$1" recorded=''
  [[ -f "$marker" ]] && recorded="$(cat "$marker" 2>/dev/null)"
  if [[ "$recorded" == 'bedrock' ]]; then
    print -r -- 'bedrock'
  else
    print -r -- 'anthropic'
  fi
}

# Which tier a `task resume <n>` re-pins. Prints `executor`, `advisor` or
# `helper`.
#
# A `<key>.tier` file reading `advisor` or `helper` wins: `architect` and
# `chore` write one beside their substrate marker, so `architect <n> bedrock`
# resumes on Bedrock AND on the advisor model, and a chore on the helper model.
# Otherwise a marker's PRESENCE is the tier record — present
# means an executor session (`task`, which clears any `.tier` it finds), absent
# means the issue-keyed `explore` that writes none (see `explore` for why).
# The tier lives in its own file because session_metrics.py reads the marker
# as one bare word, so a second field in it would break that reader.
_ds_resume_tier() {
  local tier=''
  [[ -f "$_DS_SUBSTRATE_DIR/$1.tier" ]] \
    && tier="$(cat "$_DS_SUBSTRATE_DIR/$1.tier" 2>/dev/null)"
  if [[ "$tier" == (advisor|helper) ]]; then
    print -r -- "$tier"
  elif [[ -f "$_DS_SUBSTRATE_DIR/$1" ]]; then
    print -r -- 'executor'
  else
    print -r -- 'advisor'
  fi
}

# Export the Bedrock environment, or fail loudly. Non-zero means DO NOT LAUNCH.
#
# This is the hard gate the substrate rule needs on the Bedrock side: launching
# with `CLAUDE_CODE_USE_BEDROCK=1` and no bearer token produces an opaque
# provider error several turns in, long after the operator has started working.
# Failing here costs one line and names the fix.
#
# **What this gate does NOT do is call the provider.** A live probe would cost a
# round trip on every launch to answer a question the session's own first turn
# answers for free, so the ratified "provider answering" half is served by
# making that first failure legible rather than by pre-flighting it. Set
# `DS_BEDROCK_PROBE=1` to pay for the pre-flight when diagnosing a launch.
# `chore` is the exception that probes the model itself (`_ds_model_probe`).
#
# The MODEL is not exported here: the launching verb hands it to `claude` for
# that one command, the same way on both substrates (see `_ds_session`).
_ds_bedrock_env() {
  # Background is the one tier that only NOTES an unset value: its failures
  # degrade auxiliary niceties only, and with `ANTHROPIC_MODEL` set Claude Code
  # routes background tasks to the session's model instead.
  if [[ -z "$DS_MODEL_BACKGROUND" ]]; then
    print -u2 'dropset: DS_MODEL_BACKGROUND is unset — background tasks on' \
      'Bedrock fall back to the session model.'
    [[ -n "$DS_BEDROCK_FAST_MODEL" ]] && print -u2 \
      'dropset: DS_BEDROCK_FAST_MODEL is retired — rename it to DS_MODEL_BACKGROUND.'
  fi

  # Two of the variables below — `AWS_REGION` and the bearer token — are SHARED
  # with the operator's own environment rather than owned by this launcher, so
  # what they held before this launch is recorded and later restored. See
  # `_ds_substrate_unset`, which undoes a launch rather than blanket-clearing.
  #
  # FIRST launch in a shell wins. Recording again on a second `task` in the
  # same tab would capture the FIRST launch's own values as the "prior" ones,
  # so the restore would put Bedrock's region and token back instead of the
  # operator's — the staleness this guard exists to avoid, one level up.
  if [[ -z "$_DS_SUBSTRATE_TOUCHED" ]]; then
    _DS_PRIOR_REGION="${AWS_REGION-}"
    _DS_PRIOR_TOKEN="${AWS_BEARER_TOKEN_BEDROCK-}"
    _DS_SUBSTRATE_TOUCHED=1
  fi

  export CLAUDE_CODE_USE_BEDROCK=1
  export AWS_REGION="${DS_BEDROCK_REGION:-us-west-2}"
  if [[ -n "$DS_MODEL_BACKGROUND" ]]; then
    export ANTHROPIC_DEFAULT_HAIKU_MODEL="$DS_MODEL_BACKGROUND"
  else
    unset ANTHROPIC_DEFAULT_HAIKU_MODEL
  fi
  export ENABLE_PROMPT_CACHING_1H=1

  # Resolved at launch, never held in a long-lived shell — the same lazy shape
  # and the same `${VAR:-…}` override as `_ds_secrets`, for the same reasons.
  if [[ -n "$DS_OP_ACCOUNT" && -n "$DS_OP_BEDROCK_REF" ]]; then
    export AWS_BEARER_TOKEN_BEDROCK="${AWS_BEARER_TOKEN_BEDROCK:-$(op read \
      --account "$DS_OP_ACCOUNT" "$DS_OP_BEDROCK_REF")}"
  fi

  # What this launch actually installed. The restore compares against these, so
  # a value the operator swapped by hand afterwards is recognized as theirs and
  # left alone.
  _DS_LAUNCHER_REGION="${AWS_REGION-}"
  _DS_LAUNCHER_TOKEN="${AWS_BEARER_TOKEN_BEDROCK-}"

  if [[ -z "$AWS_BEARER_TOKEN_BEDROCK" ]]; then
    print -u2 'dropset: no Bedrock bearer token — cannot start a Bedrock session.'
    print -u2 '         Set DS_OP_ACCOUNT and DS_OP_BEDROCK_REF in the runtime'
    print -u2 '         config, or launch on the anthropic substrate instead.'
    _ds_substrate_unset
    return 1
  fi

  if [[ -n "$DS_BEDROCK_PROBE" ]]; then
    if ! aws bedrock list-inference-profiles --region "$AWS_REGION" \
      --max-results 1 >/dev/null 2>&1; then
      print -u2 'dropset: Bedrock pre-flight failed (DS_BEDROCK_PROBE=1).' \
        'Check the key and the region.'
      _ds_substrate_unset
      return 1
    fi
  fi
  return 0
}

# Internal: a model id's Bedrock inference-profile id — a first-party
# `claude-*` becomes `us.anthropic.<id>` and any window suffix is dropped.
# Prints nothing for an alias (`fable`, `opus`), which Claude Code resolves and
# Bedrock cannot.
_ds_bedrock_profile() {
  local profile="${1%%\[*}"
  [[ "$profile" == claude-* ]] && profile="us.anthropic.$profile"
  [[ "$profile" == *anthropic.* ]] && print -r -- "$profile"
}

# Internal: ask Bedrock whether `$1` actually ANSWERS for this shell's agent
# key, by sending it a one-token request. Non-zero means DO NOT LAUNCH, with
# the error printed. Run after `_ds_bedrock_env`.
#
# This is the live half `_ds_bedrock_env` deliberately leaves out, paid for
# where it earns its keep: the first chore (1843) opened a tab whose every turn
# failed with a 403 — the agent identity lacked the Marketplace actions that
# enable the helper model — so the session never read its issue and only looked
# alive. `models check` cannot see that: a control-plane read passes for a model
# the account may not invoke. A Converse call with `maxTokens: 1` costs a few
# input tokens, not the eight cents a Claude Code turn costs.
#
# The header rides stdin (`-H @-`), not argv, so `ps` never shows the token;
# `print` is a builtin, so no child's argv carries it either.
_ds_model_probe() {
  local profile out code
  profile="$(_ds_bedrock_profile "$1")"
  if [[ -z "$profile" ]]; then
    print -ru2 -- "dropset: '$1' is an alias, so it cannot be probed — skipping."
    return 0
  fi
  out="$(print -r -- "Authorization: Bearer $AWS_BEARER_TOKEN_BEDROCK" \
    | curl -sS --max-time 15 -H @- -w '\n%{http_code}' -X POST \
      -H 'Content-Type: application/json' \
      -d '{"messages":[{"role":"user","content":[{"text":"ok"}]}],"inferenceConfig":{"maxTokens":1}}' \
      "https://bedrock-runtime.$AWS_REGION.amazonaws.com/model/$profile/converse" 2>&1)"
  code="${out##*$'\n'}"
  if [[ "$code" != 200 ]]; then
    print -ru2 -- "dropset: $profile does not answer on Bedrock (HTTP $code) —" \
      'refusing to open a session that would fail every turn:'
    print -ru2 -- "  ${out%$'\n'*}"
    return 1
  fi
}

# Clear the Bedrock exports from the calling shell.
#
# THIS IS NOT TIDINESS, IT IS THE SEAT PIN. These helpers export into the
# CALLING shell — they have to, since a child process could not set the
# environment `claude` inherits — so the variables outlive the session that set
# them. Run `task 1234`, quit it, and that tab is still a Bedrock tab: the next
# `plan` in it would silently run against credits on the executor's model.
# The absence of `CLAUDE_CODE_USE_BEDROCK` IS how an anthropic launch is
# expressed, so such a launch has to make that absence true rather than merely
# assert it. `ANTHROPIC_MODEL` is cleared with the rest, and the verb then
# passes its own tier's model to `claude` for that one command only — so an
# anthropic launch leaves no model pin behind in the tab either.
#
# TWO CLASSES OF VARIABLE, and conflating them is what made earlier versions of
# this wrong in both directions.
#
# The four Claude ones are **owned**: nothing but this launcher sets them, so
# clearing them outright is always right.
#
# `AWS_REGION` and `AWS_BEARER_TOKEN_BEDROCK` are **shared** with the operator's
# own environment. Clearing those outright destroys values the launcher never
# owned — an `AWS_REGION` from the shell profile, or a bearer token exported by
# hand (which `_ds_bedrock_env`'s `${VAR:-…}` form exists to honor, so running
# with no 1Password coordinates at all is supported). Destroying the token in
# particular makes the NEXT `task` in the tab fail, pointing at config the
# operator deliberately did not set.
#
# So this UNDOES a launch instead of clearing: `_ds_bedrock_env` records what
# each shared variable held beforehand and what it then installed, and this
# restores the former — but only where the current value is still the latter.
# Three cases fall out, all of them wrong under a blanket clear:
#
#   * no launch recorded in this shell — the values are not ours; leave them.
#   * a second `task` in the same tab — the FIRST launch's record wins, so the
#     restore reaches the operator's values rather than Bedrock's.
#   * the operator swapped one by hand after a launch — it no longer matches
#     what we installed, so it is recognized as theirs and kept.
_ds_substrate_unset() {
  # Owned outright by this launcher: nothing else in the operator's shell sets
  # them, so they are cleared unconditionally.
  unset CLAUDE_CODE_USE_BEDROCK ANTHROPIC_MODEL ANTHROPIC_DEFAULT_HAIKU_MODEL
  unset ENABLE_PROMPT_CACHING_1H

  # `AWS_REGION` and the bearer token are SHARED with the operator, so this
  # UNDOES a launch rather than clearing them. Absent a recorded launch they
  # were never ours — a `plan` in a fresh tab must not destroy an `AWS_REGION`
  # the profile exported.
  [[ -n "$_DS_SUBSTRATE_TOUCHED" ]] || return 0

  # Restore only what is still what the launch installed. If the operator has
  # since swapped either by hand, that value is theirs and stays.
  if [[ "${AWS_REGION-}" == "$_DS_LAUNCHER_REGION" ]]; then
    if [[ -n "$_DS_PRIOR_REGION" ]]; then
      export AWS_REGION="$_DS_PRIOR_REGION"
    else
      unset AWS_REGION
    fi
  fi
  if [[ "${AWS_BEARER_TOKEN_BEDROCK-}" == "$_DS_LAUNCHER_TOKEN" ]]; then
    if [[ -n "$_DS_PRIOR_TOKEN" ]]; then
      export AWS_BEARER_TOKEN_BEDROCK="$_DS_PRIOR_TOKEN"
    else
      unset AWS_BEARER_TOKEN_BEDROCK
    fi
  fi

  unset _DS_PRIOR_REGION _DS_PRIOR_TOKEN _DS_SUBSTRATE_TOUCHED
  unset _DS_LAUNCHER_REGION _DS_LAUNCHER_TOKEN
}

# Anthropic-substrate launches call this: warn if the shell arrived carrying
# Bedrock exports, then clear them — the model pin included, so the verb's own
# tier model is the one that reaches `claude`. $1 is the verb name.
#
# Warning alone was the ratified behavior and is not sufficient on its own — it
# tells the operator about a slip it then allows to happen. The warning is kept
# because a silent correction hides that the tab was in an unexpected state.
_ds_seat_guard() {
  if [[ -n "$CLAUDE_CODE_USE_BEDROCK" ]]; then
    print -u2 "$1: this shell carried Bedrock exports (a previous \`task\` in" \
      "the same tab); clearing them — $1 is a seat verb."
  fi
  _ds_substrate_unset
}

# Start a WORKTREE session on one Linear task. THE implementation entry point.
#
#   task <n>             the executor tier on its configured substrate (Bedrock)
#   task anthropic <n>   the executor tier on the Anthropic subscription
#   task bedrock <n>     the executor tier on Bedrock, whatever the config says
#   task resume <n>      resume by number, on the substrate and tier it had
#
# THE OVERRIDE VOCABULARY IS THE TWO SUBSTRATE NAMES AND NOTHING ELSE, the same
# literal first word on every verb that takes one. Putting an executor on
# `anthropic` exists for ONE capability reason — web search and web fetch are
# unavailable on Bedrock — so reach for it when the task needs live web
# research, not by habit. `local` was the old spelling; it never meant local
# inference, only the subscription, so it is retired and kept briefly as an
# alias that says so.
#
# Creates the bare-number worktree directory (`task 882` → `882`) whose branch
# arrives named `worktree-882` — there is no CLI flag to choose the branch, so
# `init-pr` renames it to `eng-882`, the form Linear's GitHub integration links.
#
# Three things ride the launch, and each was a parity gap when this helper was
# committed — the operator's own profile had been passing all three, and the
# committed copy silently did not, so a session started by verb differed
# from one started by hand:
#
#   * `--permission-mode auto`. The shared `settings.local.json` sets no
#     default permission mode, and `permissions.defaultMode` is honored ONLY in
#     user-level or managed settings — a project file setting it is silently
#     ignored — so the launch flag is the only lever the repo actually has.
#
#     This was `acceptEdits`, which was worse in both directions at once.
#     `auto` is the CLI's own default on this plan tier, so passing
#     `acceptEdits` was actively opting OUT of it, and nobody decided to:
#     the flag predates auto mode existing. And `auto` is the more supervised
#     of the two — a background classifier reviews each action, approving safe
#     ones and BLOCKING dangerous ones (force pushes, mass deletion, secret
#     exfiltration) — where `acceptEdits` auto-accepts every edit with no
#     review at all. It also prompts less. The old comment here claimed the
#     flag was needed or the session would "prompt on every edit", which
#     inverted the truth once auto became the default.
#
#     Explicit ask and deny rules still apply, and the five PreToolUse guard
#     hooks fire regardless of permission mode — the policy layers compose
#     rather than substitute.
#   * `-n "$tag"` — a display name, so the session is identifiable in the
#     prompt box, the `/resume` picker, and the terminal title — and it is
#     load-bearing for `fleet`, whose already-open check matches the iTerm
#     session name, so dropping it makes fleet double-resume. `task resume`
#     finds the session through `resolve_session.py` — the tag's worktree, then
#     the transcript whose cwd stamps point into it — never by the name.
#   * `/init-pr` as the initial prompt, so the bootstrap runs without being
#     asked for — the same trick `plan` and `housekeeping` use for their own skills.
task() {
  case "$1" in
    local)
      print -u2 'task: `local` is retired — use `task anthropic <n>`.'
      shift
      _ds_task_start "$1" anthropic
      ;;
    anthropic|bedrock)
      local substrate="$1"
      shift
      _ds_task_start "$1" "$substrate"
      ;;
    resume)
      shift
      _ds_task_resume "$1"
      ;;
    '')
      print -u2 'Usage: task <n> | task anthropic|bedrock <n> | task resume [n]'
      return 1
      ;;
    *)
      _ds_task_start "$1" ''
      ;;
  esac
}

# Internal: the worktree launch itself. $1 tag-or-number, $2 substrate override
# or "" for the executor tier's configured one.
_ds_task_start() {
  local tag="$1" override="$2" model substrate

  if [[ -z "$tag" ]]; then
    print -u2 'Usage: task <n> | task anthropic|bedrock <n>'
    return 1
  fi

  # Shared with the resume side, which is the whole point — see `_ds_tag_of`.
  tag="$(_ds_tag_of "$tag")"

  { read -r model; read -r substrate; } <<< "$(_ds_tier executor "$override")"
  [[ -n "$substrate" ]] || return 1

  _ds_base || return 1
  _ds_secrets
  _ds_substrate_enter "$substrate" 'task' || return 1

  # Recorded BEFORE the launch, not after: `claude` blocks for the life of the
  # session, so an after-the-fact write would land whenever the operator
  # happened to quit — and never at all if the terminal were closed instead.
  _ds_substrate_write "$tag" "$substrate"

  ANTHROPIC_MODEL="$model" claude -w "$(_ds_launch_dir "$tag")" -n "$tag" \
    --permission-mode auto \
    /init-pr
}

# Resume a worktree session by number: `task resume 814` resolves to the `814`
# worktree — or a pre-rename `eng-814` / `ceo-814` / `exp-814` one — and
# continues its most recent conversation there, on the substrate that session
# launched with. So `fleet`, which types this verb for every in-flight issue,
# reaches an architect or explore thread too, not only a task. The number-to-worktree resolution is
# the whole point — you resume a number, not a UUID.
#
# **Where the session actually lives is not guessable from the directory**, which
# is why this delegates. Older Claude Code filed a `claude -w <tag>` session's
# transcript under the BASE repo's project slug, with every `cwd` stamp pointing
# into the worktree, so a bare `claude --continue` inside the worktree found
# nothing. Current Claude Code files it under the worktree's own slug instead
# (measured 2026-10-13), which the resolver's `continue` mode reaches; both
# layouts are still on disk, so the resolver handles both.
#
# `resolve_session.py` decides which of the three addressing forms reaches the
# session; this verb only launches. `fleet` types `task resume`, so fleet
# resume inherits the fix.
_ds_task_resume() {
  # No number: the picker, from wherever the shell already is. The operator's
  # form, and worth keeping for a reason the tag form cannot cover — a session
  # whose worktree has already been pruned is still reachable this way.
  #
  # The picker cannot know which session will be chosen, so it cannot re-export
  # the right substrate — but it must not leave the tab's CURRENT one in place
  # either, or picking a seat session in a tab that last ran `task` resumes it
  # on Bedrock. Clearing makes the residual leak one-directional and matches the
  # standing "absent marker = seat" default: the worst case becomes a Bedrock
  # session resumed on the seat, which is loud (the model banner changes) rather
  # than silent.
  if [[ -z "$1" ]]; then
    _ds_seat_guard 'task resume'
    _ds_secrets
    claude --resume --permission-mode auto
    return
  fi

  local tag model substrate
  tag="$(_ds_tag_of "$1")"

  # Re-pin the MODEL from the tier table as well as re-exporting the substrate,
  # BEFORE moving the shell. The tier comes from the marker's presence (see
  # `_ds_resume_tier`), so an issue-keyed explore session comes back on the
  # advisor model rather than the saved default; the substrate is the one the
  # session recorded, not today's config, so a resume never switches provider
  # mid-conversation. Order matters: either step can refuse, and resolving
  # after the `cd` below would leave the operator relocated into the worktree
  # with no session and no explanation of the move.
  local key
  key="$(_ds_marker_key "$tag")"
  # A chore has no worktree to resolve, so its own verb resumes it — by the id
  # it computed, on the recorded substrate, model probe included.
  if [[ "$(_ds_resume_tier "$key")" == helper ]]; then
    chore "$tag"
    return
  fi
  substrate="$(_ds_substrate_read "$key")"
  { read -r model; read -r substrate; } <<< \
    "$(_ds_tier "$(_ds_resume_tier "$key")" "$substrate")"
  [[ -n "$substrate" ]] || return 1
  _ds_substrate_enter "$substrate" 'task resume' || return 1

  local mode sid run_from
  {
    read -r mode
    read -r sid
    read -r run_from
  } <<< "$(python3 "$_DS_REPO/.claude/tools/resolve_session.py" \
    --tag "$tag" --repo "$_DS_REPO" --format lines 2>/dev/null)"

  # Fall back to the base repo rather than returning: `${...:-}` guards an EMPTY
  # run_from, not a STALE one. A transcript's cwd stamps outlive the worktree
  # they name, so a pruned worktree yields a path that no longer exists — and a
  # bare `|| return 1` would make this exit silently with no session and no
  # picker, which is worse than every pre-change failure path.
  cd "${run_from:-$_DS_REPO}" || cd "$_DS_REPO" || return 1
  _ds_pull
  _ds_secrets

  case "$mode" in
    continue)
      # The worktree has its own transcript, so per-directory addressing works
      # and this is the original fast path.
      ANTHROPIC_MODEL="$model" claude --continue --permission-mode auto
      ;;
    resume)
      # The `-w` case: resume by id from the base repo. This is the form that
      # was missing, and the only one that reaches such a session.
      ANTHROPIC_MODEL="$model" claude --resume "$sid" --permission-mode auto
      ;;
    *)
      # Nothing resolved — the worktree was pruned, or the session never
      # started. `--resume <tag>` filters the picker rather than resuming, which
      # is a pick rather than a resume, but it is the last form that can reach
      # anything.
      ANTHROPIC_MODEL="$model" claude --resume "$tag" --permission-mode auto
      ;;
  esac
}

# Start OR resume an EXPLORE session — research, audits, and any other task
# whose deliverable is a spec or a findings document rather than a code change.
#
#   explore <n>       an explore task with a Linear issue: worktree and name `<n>`
#   explore <name>    one without: worktree and name `exp-<name>`
#
# **It runs in its own worktree, and it is IDEMPOTENT** — both reverse what this
# verb shipped with, and both are the same operator ruling that moved
# `architect`. As there, the worktree is **temporary working state** and the
# session is **read-only toward the repo**: no commits, no PR, durable state in
# Linear, anything repo-bound deferred to a follow-up worker task. See that
# verb's comment for the two superseded framings.
#
# For an audit this is the whole point: findings land as parked Linear issues,
# which is durable, so nothing of value is lost when the worktree goes.
#
# A NUMBER KEYS THE WORKTREE TO THE ISSUE, deliberately, and this is the
# substantive naming call. `explore 1196` lands in `1196`, named `1196` — the
# same bare number `task` uses — so an explore session inherits every mechanism
# keyed on the issue: `cdds 1196` and `task resume 1196` reach it, and the
# pruner reclaims it on the issue's status type. Note the rationale is
# addressability and NOT "protecting the worktree": its contents are expendable
# by construction, and the convention doc warns against making it sound
# precious. The role-prefixed display names (`exp-<n>`, `ceo-<n>`) are retired
# for issue-keyed sessions: the number is what the operator looks up in Linear,
# so a prefix is screen estate that carries nothing.
#
# THE COST OF SHARING THE TAG: `<n>` names a worktree that three session
# kinds (`task`, `explore`, `architect`) can claim, with different substrates
# and different tiers, while the
# substrate marker keys on the tag alone. That is why nothing is written to it
# below — and the marker's resulting ABSENCE is what tells `task resume <n>`
# (and `fleet`, which types it) to re-pin the advisor tier on the anthropic
# substrate. See `_ds_resume_tier`. `explore <n>` remains the natural resume
# verb; the other two now land on the same model.
#
# THE `resume` TWIN IS GONE, and dropping it removes a wart rather than a
# feature. `explore <name>` now creates the session if absent and resumes it if
# present, so `explore resume <name>` would be a pure synonym — and the old
# resume branch could not resume deterministically anyway: `-r/--resume` matches
# on session ID, a name is not one, so a bare name PRE-FILTERED THE INTERACTIVE
# PICKER. Keying on `_ds_topic_sid` computes an id the way `architect` does,
# which is what makes the fix possible. A name is now REQUIRED for the same
# reason `architect` requires an issue number: it names a worktree.
#
# This folds two older verbs into one: the unnamed form and the named form were
# separate launchers with identical bodies bar one flag. The idempotency above
# folds in the third.
#
# **The advisor tier, and no substrate override.** Explore work is
# thinking-heavy, so it runs the top tier like `plan` and `architect` do. It
# takes no `bedrock` word because the one override the operator ratified is the
# credit-pinch case for `plan` and `architect`; explore carries no such pinch.
# It is PINNED to the anthropic substrate, deliberately ignoring
# `DS_MODEL_ADVISOR_SUBSTRATE`: it writes no substrate marker (see below), so a
# marker-less `task resume <n>` always resumes it on anthropic, and launching it
# anywhere else would make that resume switch provider mid-conversation.
#
# The model pin, the permission mode and the start-or-resume probe all come
# from `_ds_session`, which gets each right on both branches by construction —
# the old hand-rolled resume branch here dropped the pin on every reopen.
explore() {
  local raw="$1" tag name

  if [[ "$raw" == 'resume' ]]; then
    print -u2 'explore: `resume` is retired —' \
      '`explore <n|name>` now resumes an existing session itself'
    return 1
  fi
  if [[ -z "$raw" || -n "$2" ]]; then
    print -u2 'Usage: explore <n> | explore <name>   (a name is required: it names a worktree)'
    return 1
  fi
  # The name reaches a session name, a worktree and a branch, so hold it to the
  # shape a branch would take, which the worktree home makes binding rather
  # than merely tidy.
  if [[ ! "$raw" =~ '^[a-z0-9][a-z0-9-]*$' ]]; then
    print -u2 'explore: name must be lowercase letters, digits and dashes'
    return 1
  fi

  # NORMALIZE AN ISSUE REFERENCE TO ITS BARE NUMBER FIRST, through the ONE tag
  # owner `task` uses, before the branch below reads it. `eng-1196` and `1196`
  # name one issue and must produce one worktree, one session name and one
  # session id. Deciding the branch before normalizing once sent `eng-1196`
  # down the free-form path to a worktree `exp-eng-1196` with no bootstrap
  # prompt — invisible to `cdds`, `fleet` and status-type pruning alike.
  raw="$(_ds_tag_of "$raw")"
  if [[ "$raw" == 0 ]]; then
    print -u2 'explore: 0 is not a Linear issue number'
    return 1
  fi

  # A bare number is an issue-keyed explore task.
  #
  # THE ISSUE-KEYED FORM CARRIES A BOOTSTRAP PROMPT, and it has to. A session
  # launched with no initial prompt SITS IDLE until a human types something,
  # which is silent — it looks started. Measured on the 1196 audit: dispatched
  # into its own tab, the verb typed correctly, and still idle five minutes
  # later beside five sessions that were working, because typing the verb is not
  # the same as giving the session its task. `task <n>` never had this exposure
  # (it passes `/init-pr`), so the gap was explore-shaped from the start.
  #
  # There is no `explore` skill to name here, so the prompt is written out. It
  # deliberately states the read-only posture rather than assuming the issue
  # body does: an explore session that commits or opens a PR is the failure this
  # launch shape exists to prevent.
  local prompt=''
  if [[ "$raw" == <-> ]]; then
    tag="$raw" name="$raw"
    prompt="Bootstrap this explore task from Linear issue ENG-$raw. Read that"
    prompt+=" issue as the spec, mark it In Progress, and verify its claims and"
    prompt+=" any file:line citations against HEAD before acting on them."
    prompt+=" This session is READ-ONLY toward the repo: no commits, no PR."
    prompt+=" The worktree is temporary working state and its contents are"
    prompt+=" expendable; Linear is the durable record, so findings go on the"
    prompt+=" issue and anything repo-bound is filed as a follow-up worker"
    prompt+=" task. Move the issue to In Review when you hand the deliverable"
    prompt+=" off; only the operator marks it Done. The full shape is in"
    prompt+=" docs/conventions/local-integrations.md."
  else
    # Free-form: the operator names the subject in their first message, so
    # inventing one here would only be a guess to correct.
    tag="exp-$raw" name="exp-$raw"
  fi

  local model substrate
  { read -r model; read -r substrate; } <<< "$(_ds_tier advisor anthropic)"
  [[ -n "$substrate" ]] || return 1
  _ds_substrate_enter "$substrate" 'explore' || return 1
  # NO SUBSTRATE MARKER IS WRITTEN HERE, DELIBERATELY, and the reason is the
  # whole hazard of keying this worktree to `<n>`: the marker is keyed on the
  # WORKTREE TAG ALONE, and `_ds_task_start` writes that same key. So
  # `task 1196` (bedrock) followed by `explore 1196` would flip the marker to
  # `seat`, and the next `task resume 1196` — or `fleet`, which types exactly
  # that for every in-flight issue — would silently resume the IMPLEMENTATION
  # session on the seat and eat the subscription window. That is the failure this
  # file calls silent in both directions, caused by the fix for it.
  #
  # Not writing costs nothing: an absent marker already reads as seat, which is
  # correct for a seat-only verb. An intermediate revision did write it, to make
  # a comment's "the substrate marker records it" claim true; the honest fix was
  # to correct the claim instead.
  #
  # One residue: `architect <n>` DOES write a marker (with an advisor `.tier`),
  # so an `explore <n>` on a number an architect thread already holds inherits
  # that thread's substrate on `task resume <n>`. The two would share one
  # worktree anyway, and `task resume` reaches one session per worktree, so it
  # is accepted rather than coded around.
  _ds_session "$(_ds_topic_sid explore "$raw")" "$name" "$prompt" \
    "$model" "$(_ds_launch_dir "$tag")"
}

# Internal: the model-free morning upkeep pass (.claude/tools/upkeep.py), run in
# the operator's terminal before a planning client starts, so a hung step is
# visible here rather than buried in a session. The tool writes the Planning
# document's upkeep heading, which the bootstrap reads; destructive items wait
# for the planning tab's one consolidated question.
#
# Secrets first: the pass reads the board and the PR set. `--if-stale` makes a
# same-day reopen free — the tool's own stamp answers "ran today", and it stamps
# only a run whose report reached the document. DS_SKIP_UPKEEP=1 is the off
# switch. A failure WARNS and the launch proceeds: upkeep is best-effort, and a
# planning session must never be held hostage to it.
#
# The local INT trap is what makes Ctrl-C on a hung step mean "skip upkeep"
# rather than "abort plan": without it interactive zsh abandons the calling
# function when a foreground child dies by SIGINT, and since an interrupted
# pass writes no stamp, every retry would hang on the same step. The child
# still receives the signal — a trap, unlike an ignore, is reset across exec.
_ds_upkeep() {
  setopt localoptions localtraps
  local rc interrupted=''
  if [[ "$DS_SKIP_UPKEEP" == 1 ]]; then
    print -u2 'dropset: DS_SKIP_UPKEEP=1 — skipping the upkeep pass.'
    return 0
  fi
  if ! cd "$_DS_REPO"; then
    print -u2 'dropset: no base checkout to run the upkeep pass in — skipping.'
    return 0
  fi
  # Before the secrets fetch, so a hung `op read` is covered too.
  trap 'interrupted=1' INT
  _ds_secrets
  python3 "$_DS_REPO/.claude/tools/upkeep.py" run --if-stale
  rc=$?
  if [[ -n "$interrupted" ]]; then
    print -u2 'dropset: upkeep interrupted — launching anyway.'
  elif (( rc != 0 )); then
    print -u2 "dropset: the upkeep pass failed (exit $rc) — launching anyway;" \
      'DS_SKIP_UPKEEP=1 skips it.'
  fi
  return 0
}

# Start OR resume a CHORE session — a minor job whose deliverable lands OUTSIDE
# the repo (a key file for 1Password, a local deploy, a one-off probe).
#
#   chore <n>             the helper tier on its configured substrate (Bedrock)
#   chore anthropic <n>   the helper tier on the subscription — for web search
#   chore bedrock <n>     the helper tier on Bedrock, whatever the config says
#
# The gap it fills: `task` is the executor tier plus a bootstrap PR, `explore`
# is the advisor tier on the seat, and nothing was both cheap and PR-less. The
# session is READ-ONLY toward the repo, files anything repo-bound as a
# follow-up task, and goes to In Review at handoff, Done only on the operator's
# word; it takes `task`'s argument shapes. Idempotent like `explore`, so there
# is no `resume` word.
#
# NO WORKTREE: it launches in the base checkout. A chore never produces a PR,
# so a worktree would protect nothing and cost a checkout, a branch and a prune
# entry. The prompt is just `/chore <n>` — the skill bootstraps and the Linear
# issue is the whole brief, so this verb carries no prose of its own.
#
# A Bedrock launch — a resume included — PROBES THE MODEL first
# (`_ds_model_probe`) and refuses on anything but an answer (an alias, which
# Bedrock cannot resolve, is skipped with a note): a tab that fails every turn
# is worse than no tab.
#
# A chore and a task are separate Linear issues (operator ruling), so they
# should never share a number — but that is discipline, not a guarantee, so a
# `task` session's marker is left alone exactly as `architect` leaves it:
# clobbering it would resume a PR-bearing task on the helper model, silently.
#
# The marker carries a `helper` tier, so `task resume <n>` — and `fleet`, which
# types it — hands back to this verb, which resumes by its computed id. A
# reopen with no substrate word keeps the recorded substrate, as `architect`
# does.
chore() {
  local override='' tag model substrate
  if [[ "$1" == (anthropic|bedrock) ]]; then
    override="$1"
    shift
  fi
  if [[ -z "$1" || -n "$2" ]]; then
    print -u2 'Usage: chore <n> | chore anthropic|bedrock <n>'
    return 1
  fi
  tag="$(_ds_tag_of "$1")"
  if [[ "$tag" != <-> || "$tag" == 0 ]]; then
    print -u2 'chore: takes a Linear issue number — file the issue first'
    return 1
  fi
  if [[ -z "$override" && "$(_ds_resume_tier "$tag")" == helper ]]; then
    override="$(_ds_substrate_read "$tag")"
  fi
  { read -r model; read -r substrate; } <<< "$(_ds_tier helper "$override")"
  [[ -n "$substrate" ]] || return 1
  _ds_substrate_enter "$substrate" 'chore' || return 1
  if [[ "$substrate" == bedrock ]] && ! _ds_model_probe "$model"; then
    _ds_substrate_unset
    return 1
  fi
  if [[ "$(_ds_resume_tier "$tag")" == executor ]]; then
    print -u2 "chore: $tag carries a task session's substrate marker —" \
      'leaving it, so a task resume of it re-pins the executor tier.'
  else
    _ds_substrate_write "$tag" "$substrate" helper
  fi
  _ds_session "$(_ds_topic_sid chore "$tag")" "$tag" "/chore $tag" "$model" ''
}

# Start OR resume today's PLANNING session. The name is derived from the date.
#
#   plan             the advisor tier on its configured substrate (anthropic)
#   plan bedrock     the advisor tier on Bedrock — the credit-pinch override
#   plan anthropic   the advisor tier on the subscription, whatever the config
#
# Retention: on Bedrock a Fable-class model falls under the account's standing
# AWS human-review opt-in (a model allowing mode `none` does not); the operator
# accepts that in a pinch.
#
# Idempotent by design — a planning session is opened and reopened many times
# in a day, and having to remember which state it is in is the friction this
# removes. An `rpaps` twin was considered and rejected for that reason.
#
# Things it makes deterministic, each of which used to be a manual step the
# operator could forget:
#
#   * The model. Planning sessions run the advisor tier deliberately —
#     fidelity over tokens — resolved from the runtime config and refused
#     before launch when it does not resolve, on create and resume alike. The
#     skill's `model:` frontmatter covers a mid-session `/plan` only.
#   * The directory. A planning session touches the board, not a branch, so it
#     runs in the base repo.
#   * The bootstrap. Passing `/plan` as the initial prompt means the skill's
#     bootstrap read happens without being asked for.
#   * The AWS session. `_ds_aws_login` refuses the launch when credentials do
#     not resolve — a planning session reads cost and account data, the login is
#     interactive, and a session cannot fix an expired token from the inside.
#     Deliberately not called a "hard" gate: it has two documented
#     warn-and-launch escapes (no `aws` at all, and an `aws` too old for
#     `login`), so it is hard only when AWS is present and current. It runs
#     AFTER the seat guard on purpose: the guard clears an `AWS_REGION`
#     inherited from a previous `task` in the same tab, and the AWS CLI reads
#     that variable, so probing first would probe under the Bedrock launcher's
#     environment rather than the operator's own. On `plan bedrock` the probe
#     runs under the launcher's region, which is the region the session uses.
#
# `date +%-d` gives an unpadded day, so the 5th is `plan-5`, not `plan-05`.
#
# THE UPKEEP PASS runs next, BEFORE the client starts — see `_ds_upkeep`. It sits
# after the AWS gate so a refused login stops the morning at this terminal before
# anything else runs, and outside the cycle loop so a relaunch never repeats it.
#
# THE CYCLE LOOP. A planning session that cycles (the `plan` skill's "Cycling")
# closes out, writes `plan-cycle-pending` beside the substrate markers and ends
# its own client; this loop sees the marker and relaunches IN THIS TAB — no tab
# opened or closed — under a fresh id (the launch day's counter,
# `plan-cycle-<date>`, bumped) and the same display name, so worker addressing
# never changes. The launch day is fixed ONCE, before the loop: a cycle past
# midnight keeps `plan-<launch day>` and that day's ids rather than turning
# into tomorrow's session under a new name. It refuses to relaunch while a live
# session already holds the name, since a duplicate name would break that
# addressing. The counter is per day, so a later bare `plan` the same day
# resumes the latest cycle; plan_cycle.py and resolve_session.py read the same
# counter, and DS_PLAN_DATE carries the launch day into the client.
#
# THE ROLE MARKER. Around the loop the tab carries the iTerm user variable
# `ds_role=plan`, which `iterm-reorder.py` reads to keep a resting planning tab
# rightmost (Command-9) and `session_dispatch.py` reads to open new tabs just
# left of it. Set once outside the loop, so a cycle relaunch inherits it, and
# cleared by the `always` block on every way out. `DS_PLAN_TAB_RIGHTMOST=0` in
# the runtime config skips it, and the planning tab then sorts like any other.
plan() {
  local model substrate day counter cycle name rc relaunch=''
  local marker="$_DS_SUBSTRATE_DIR/plan-cycle-pending"
  if [[ -n "$2" || ( -n "$1" && "$1" != (anthropic|bedrock) ) ]]; then
    print -u2 'Usage: plan [anthropic|bedrock]   (the name is derived from the date)'
    return 1
  fi
  { read -r model; read -r substrate; } <<< "$(_ds_tier advisor "$1")"
  [[ -n "$substrate" ]] || return 1
  _ds_substrate_enter "$substrate" 'plan' || return 1
  # A refused login must not leave a `plan bedrock`'s exports — the bearer token
  # included — behind in the tab with no session to show for them.
  if ! _ds_aws_login 'plan'; then
    _ds_substrate_unset
    return 1
  fi
  _ds_upkeep
  day="$(date +%Y%m%d)" name="plan-$(date +%-d)"
  counter="$_DS_SUBSTRATE_DIR/plan-cycle-$day"
  [[ "${DS_PLAN_TAB_RIGHTMOST:-1}" == 1 ]] && _ds_iterm_role plan
  {
  while true; do
    cycle=0
    [[ -r "$counter" ]] && cycle="$(<"$counter")"
    # `10#` so a hand-edited `01` reads as 1, matching resolve_session's int().
    [[ "$cycle" == <-> ]] && cycle=$(( 10#$cycle )) || cycle=0
    if [[ -n "$relaunch" ]]; then
      # Fail CLOSED: 0 is "held", 1 is "free", anything else is unanswered.
      python3 "$_DS_REPO/.claude/tools/plan_cycle.py" live-name "$name"
      rc=$?
      if (( rc != 1 )); then
        print -u2 "dropset: not cycling — a live $name session exists, or" \
          "the check failed (exit $rc); relaunching would break the name."
        return 1
      fi
    fi
    rm -f "$marker"
    # DS_PLAN_DATE tells the session its LAUNCH day, so plan_cycle.py and
    # resolve_session.py still name this session after midnight.
    DS_PLAN_DATE="$day" _ds_session "$(_ds_daily_sid plan "$cycle" "$day")" \
      "$name" /plan "$model"
    rc=$?
    [[ -f "$marker" ]] || return $rc
    mkdir -p "$_DS_SUBSTRATE_DIR"
    print -r -- $(( cycle + 1 )) >| "$counter"
    relaunch=1
  done
  } always {
    _ds_iterm_role ''
  }
}

# Start OR resume today's HOUSEKEEPING session — the same contract as `plan`,
# so a day's upkeep is one verb rather than a hand-started session.
#
# The EXECUTOR tier's model: housekeeping is upkeep, not board decisions, so it
# does not inherit the advisor tier — but every managed verb pins through the
# config, so it no longer floats on the saved default either.
#
# **The anthropic substrate, always, and this one is not a capability call.**
# Housekeeping could run on Bedrock perfectly well; the operator uses it to OPEN
# the 5-hour subscription window at the start of a day, which only a
# subscription session does. So it takes the executor MODEL but never the
# executor tier's substrate, and takes no override.
#
# It carries the same `_ds_aws_login` gate as `plan`, with the same two
# warn-and-launch escapes, and for the same reason: this pass reports spend and
# runs the monthly permission refresh, both of which need credentials the
# session cannot obtain for itself. `architect` is
# deliberately NOT gated — it argues design rather than reading cost data, and an
# interactive login is a poor thing to stand between the operator and a design
# thought. Add it only if an architect session is actually found wanting one.
housekeeping() {
  if [[ -n "$1" ]]; then
    print -u2 'Usage: housekeeping   (no arguments; name derived from the date)'
    return 1
  fi
  local model substrate
  { read -r model; read -r substrate; } <<< "$(_ds_tier executor anthropic)"
  [[ -n "$substrate" ]] || return 1
  _ds_seat_guard 'housekeeping'
  _ds_aws_login 'housekeeping' || return 1
  _ds_daily_session housekeeping "housekeeping-$(date +%-d)" /housekeeping \
    "$model"
}

# Start OR resume an ARCHITECT session on one design thread — the CEO hat. Same
# seat quality as `plan` and the same idempotency; a different job.
#
# Takes the thread's GOVERNING ISSUE NUMBER as its only argument (operator
# direction — there is no topic argument), and keys the session on it, so each
# long-horizon thread gets its own resumable session and parallel threads never
# share context:
#
#   architect 1572
#
# A design thread is a board-tracked task like any other that happens to do
# read-only spec work, so it is named like one: the session, the tab and the
# worktree are all the bare number. A thread with no governing issue files or
# names one first. The session id seeds on the number, and a number outlives a
# day, so the thread stays resumable by construction — and a pre-rename thread
# launched as `architect 1572` (name `ceo-1572`) keeps its id, since the old
# topic seed was that same string. Its resume passes no `-w`; should the create
# branch be taken anyway, `_ds_launch_dir` reuses `ceo-1572` rather than
# creating a bare `1572` beside it.
#
# The advisor tier like `plan`, for the same reason: this session argues
# strategy, and fidelity beats tokens. It makes no SCHEDULING writes — it files
# its own tasks and moves its own governing issue to In Review, but places no
# edges, re-prioritizes nothing, closes nothing and edits no other issue; see
# the skill. It takes the same optional substrate word after the number
# (`architect <n> bedrock`), with the same retention line as `plan`.
#
# **It runs in its own worktree, named `<n>`, and that worktree is
# TEMPORARY WORKING STATE** — somewhere to iterate a spec or plan file with the
# operator, nothing more (operator ruling, 2026-09-14). The session is
# **read-only toward the repo: no commits, no PR.** Durable state lives in
# **Linear**; anything repo-bound, a ratified spec included, lands later via a
# follow-up worker task.
#
# TWO SUPERSEDED FRAMINGS, both worth naming so neither gets resurrected. The
# verb originally ran in the **base repo**, and a spec left untracked there was
# protected by nothing — the first one sat awaiting feedback with its issue
# already marked Done and nothing recognized it as live work. The fix attempted
# on 09-11 was a worktree plus an **open PR**, on the reasoning that
# `housekeeping` will not prune a worktree holding unpushed work. That is now
# superseded rather than unsolved: the failure was the spec file being the only
# copy of live work, and what prevents it is **durable state living in Linear**,
# which makes the worktree's contents expendable by construction. So the
# protection no longer needs a PR to exist.
#
# Its worktree, being keyed on the issue, is eligible for the pruner once that
# issue's status type is completed or canceled. The pruner still refuses a dirty
# tree, so one holding an untracked spec file — the usual case — is reported on
# every pass and removed by hand.
architect() {
  local tag model substrate
  if [[ -z "$1" || -n "$3" || ( -n "$2" && "$2" != (anthropic|bedrock) ) ]]
  then
    print -u2 'Usage: architect <n> [anthropic|bedrock]' \
      '  (n is the governing Linear issue, e.g. architect 1572)'
    return 1
  fi
  tag="$(_ds_tag_of "$1")"
  if [[ "$tag" != <-> || "$tag" == 0 ]]; then
    print -u2 'architect: takes the governing issue number, not a topic —' \
      'file or name the issue first'
    return 1
  fi
  # A reopen with no substrate word keeps the one this thread recorded, so
  # `architect <n> bedrock` followed by a bare `architect <n>` does not switch
  # provider mid-conversation. An explicit word still wins and is re-recorded.
  local override="$2" executor_owned=''
  if [[ -z "$override" \
    && "$(_ds_resume_tier "$tag")" == advisor && -f "$_DS_SUBSTRATE_DIR/$tag" ]]
  then
    override="$(_ds_substrate_read "$tag")"
  fi
  [[ "$(_ds_resume_tier "$tag")" == executor ]] && executor_owned=1
  { read -r model; read -r substrate; } <<< "$(_ds_tier advisor "$override")"
  [[ -n "$substrate" ]] || return 1
  _ds_substrate_enter "$substrate" 'architect' || return 1
  # Recorded so `task resume <n>` and `fleet` bring the thread back on its own
  # substrate and the advisor tier. A marker a `task` session wrote on the same
  # number is left alone: clobbering it would resume the IMPLEMENTATION
  # session on the wrong tier — the hazard `explore` documents.
  if [[ -n "$executor_owned" ]]; then
    print -u2 "architect: $tag carries a task session's substrate marker —" \
      'leaving it, so a task resume of it re-pins the executor tier.'
  else
    _ds_substrate_write "$tag" "$substrate" advisor
  fi
  _ds_session "$(_ds_topic_sid architect "$tag")" \
    "$tag" "/architect ENG-$tag" "$model" "$(_ds_launch_dir "$tag")"
}

# Resume the whole FLEET: one iTerm tab per in-flight Linear issue, each with
# its session resumed and flagged green for attention. The batch counterpart to
# `task resume`, for after a machine restart.
#
# `fleet` prints the plan and opens nothing; `fleet go` applies it. The default
# is read-only deliberately — this one verb can open many tabs and resume many
# sessions, so seeing the list first is worth one extra word.
#
# It resolves the fleet itself (state type `started`, so In Progress *and* In
# Review) and skips any issue whose tab is already open, so it is safe to run
# twice. The deterministic work — the Linear query, the tag derivation, the
# already-live check, the window driving — lives in the committed tool; this is
# the thin verb over it, per the skill-tooling convention.
#
# Each tab it opens types `task resume <n>`, which reads that session's own
# substrate marker — so a mixed fleet of Bedrock and seat sessions comes back
# on the right provider per session, with no substrate knowledge here.
fleet() {
  _ds_base || return 1
  _ds_secrets
  if [[ "$1" == "go" ]]; then
    python3 "$_DS_REPO/.claude/tools/fleet_resume.py" --apply
  elif [[ -z "$1" ]]; then
    python3 "$_DS_REPO/.claude/tools/fleet_resume.py"
  else
    print -u2 'Usage: fleet [go]   (no argument = show the plan; `go` = open the tabs)'
    return 1
  fi
}

# Show the model-tier table as this shell resolves it, or check it.
#
#   models         print tier, model and substrate — offline, no network
#   models check   also ask Bedrock whether each tier's model exists
#
# THE CHECK IS FREE, AND THAT IS WHY IT IS A SEPARATE STEP. Proving an id
# names a real model by CALLING the model costs real money per launch
# (measured: about eight cents for one minimal Fable turn), while
# `get-inference-profile` is a control-plane read that costs nothing. It needs
# the admin login rather than the agent key, so the bearer token is stripped —
# with it set, the call silently runs as the agent user and reads as
# AccessDenied. One limit, stated so a pass is not over-read: an `ACTIVE`
# profile does not prove the ACCOUNT may invoke it (a model whose use-case
# form was never submitted still reads `ACTIVE`).
#
# Every tier is checked in its BEDROCK form, including an anthropic-substrate
# advisor tier: `plan bedrock` sends that same id to Bedrock, so it has to
# resolve there too. Every id, background included, is mapped to its Bedrock
# profile: a first-party `claude-*` becomes `us.anthropic.<id>`, and any window
# suffix is dropped. An advisor, executor or helper alias (`fable`, `opus`)
# cannot be checked: Claude Code resolves it, not Bedrock, so it is reported
# rather than failed. A background alias fails instead: Claude Code resolves
# `haiku` THROUGH this slot, so an alias here is circular.
models() {
  if [[ -n "$1" && "$1" != check ]]; then
    print -u2 'Usage: models [check]'
    return 1
  fi
  local tier model substrate profile entry rc=0
  local -a profile_flag
  for tier in advisor executor helper; do
    # Reset first: a failed `_ds_tier` prints nothing, and the previous tier's
    # values must not survive into this row.
    model='' substrate=''
    { read -r model; read -r substrate; } <<< "$(_ds_tier "$tier" '')"
    if [[ -n "$substrate" ]]; then
      print -r -- "$tier  $model  ($substrate)"
    else
      rc=1
    fi
  done
  print -r -- "background  ${DS_MODEL_BACKGROUND:-<unset>}  (Bedrock launches only)"
  [[ "$1" == check ]] || return $rc

  # The same admin profile `_ds_aws_login` probes, or the check would report a
  # false FAIL for an operator whose admin login lives under a named profile.
  [[ -n "$DS_AWS_PROFILE" ]] && profile_flag=(--profile "$DS_AWS_PROFILE")
  for entry in "advisor:$DS_MODEL_ADVISOR" "executor:$DS_MODEL_EXECUTOR" \
    "helper:$DS_MODEL_HELPER" "background:$DS_MODEL_BACKGROUND"; do
    tier="${entry%%:*}" model="${entry#*:}"
    [[ -n "$model" ]] || continue
    profile="$(_ds_bedrock_profile "$model")"
    if [[ -z "$profile" ]]; then
      if [[ "$tier" == background ]]; then
        print -r -- "FAIL   $tier $model — an alias; this slot needs a model id"
        rc=1
      else
        print -r -- "skip   $tier $model — an alias; Claude Code resolves it"
      fi
      continue
    fi
    if env -u AWS_BEARER_TOKEN_BEDROCK aws bedrock get-inference-profile \
      "${profile_flag[@]}" --region "${DS_BEDROCK_REGION:-us-west-2}" \
      --inference-profile-identifier "$profile" >/dev/null 2>&1; then
      print -r -- "ok     $tier $profile"
    else
      print -r -- "FAIL   $tier $profile — no such inference profile, or the" \
        'admin login has expired'
      rc=1
    fi
  done
  return $rc
}
