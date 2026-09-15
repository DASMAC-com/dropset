#!/usr/bin/env zsh
# cspell:word subshell
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
# matching `task resume`.
cdds() {
  cd "$_DS_REPO" || return 1
  _ds_pull
  [[ -z "$1" ]] && return 0

  local tag="eng-${1#eng-}"
  local worktree="$_DS_REPO/.claude/worktrees/$tag"
  if [[ ! -d "$worktree" ]]; then
    print -u2 "cdds: no worktree at $worktree"
    return 1
  fi
  cd "$worktree"
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
# Three things about the shape below are load-bearing:
#
#   * It **probes before logging in**. `aws login` opens a browser, so a
#     still-valid session must not pay for one. `sts get-caller-identity` is the
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
  aws login "${profile[@]}"

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
      "the older \`aws sso login\` by hand first."
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
    "once \`aws login\` succeeds, or check DS_AWS_PROFILE in the runtime" \
    "config."
  return 1
}

# Internal: a deterministic per-day session UUID, seeded by kind + full date.
#
# The full date is in the seed so that `plan-18` in August and `plan-18` in
# September cannot collide; the display name stays day-only by operator choice,
# and this id is what actually disambiguates them. The kind prefix keeps a
# day's planning and housekeeping sessions apart for the same reason.
_ds_daily_sid() {
  local raw
  if (( $+commands[md5] )); then
    raw="$(printf 'dropset-%s-%s' "$1" "$(date +%Y%m%d)" | md5 -q)"
  else
    raw="$(printf 'dropset-%s-%s' "$1" "$(date +%Y%m%d)" | md5sum)"
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
#   $4 model to pin, or "" for the saved default
#
# The model rides BOTH branches; the name and the initial prompt ride only the
# create path. That split is the point, and getting it wrong is silent: `-n`
# sets a display name and the prompt bootstraps a skill, so re-passing either
# on a resume is meaningless — but `--model` is a per-session flag, and a
# planning session is reopened many times a day. Passing it only on create
# would honor the pin on the day's FIRST launch and quietly drop to the saved
# default on every reopen after it, which is exactly the "still works, so
# nobody notices" slip `plan` exists to remove.
_ds_daily_session() {
  _ds_session "$(_ds_daily_sid "$1")" "$2" "$3" "$4"
}

# Internal: the start-or-resume core, given an already-computed session id.
#
# Split out from `_ds_daily_session` so a session keyed by something other than
# the date can reuse it unchanged. `plan` and `housekeeping` key on the day; `architect` keys
# on a TOPIC, because a design thread outlives a day and resuming it tomorrow is
# the whole point. Everything below the id — the idempotency, the model pin
# riding both branches, the permission mode — is identical for both, and the
# operator's stated abstraction is that these launchers differ only in the
# briefing.
#
#   $1 session id   $2 display name   $3 initial prompt
#   $4 model to pin, or "" for the saved default
#   $5 worktree tag, or "" to run in the base checkout
#
# THE WORKTREE FLAG RIDES THE CREATE BRANCH ONLY. `-w` *creates* a worktree, so
# passing it on the resume branch would ask for a second one every time a
# long-lived thread is reopened. The asymmetry is the point, not an oversight.
#
# The slug below needs no worktree case, which is worth stating because the
# obvious reading is that it does: `claude -w <tag>` runs from the BASE repo, so
# Claude Code files the transcript under the base repo's project slug even
# though every `cwd` stamp inside it points into the worktree, and no project
# directory for the worktree ever exists. `_ds_task_resume` documents the same
# fact from the other side — it is why resuming such a session by id from the
# base repo is the only form that reaches it. So `$PWD` here is already right
# for both kinds of launch, and deriving a worktree slug would break the probe.
_ds_session() {
  local sid="$1" name="$2" prompt="$3" model="$4" worktree="$5"

  local slug transcript
  local -a model_flag worktree_flag prompt_arg
  [[ -n "$model" ]] && model_flag=(--model "$model")
  [[ -n "$worktree" ]] && worktree_flag=(-w "$worktree")
  # An ARRAY rather than a bare "$prompt", so a verb with no bootstrap skill to
  # run passes no positional at all. Spelled directly, an empty prompt reaches
  # the CLI as an empty argument, which is a different thing from omitting it.
  [[ -n "$prompt" ]] && prompt_arg=("$prompt")

  _ds_base || return 1
  _ds_secrets

  # The transcript path Claude Code writes: the project slug replaces every `/`
  # and `.` in the cwd with `-` — the same rule
  # .claude/tools/resolve_session.py's `slugify` encodes for finding a
  # transcript back.
  slug="${PWD//[\/.]/-}"
  transcript="$HOME/.claude/projects/$slug/$sid.jsonl"

  if [[ -f "$transcript" ]]; then
    # A resume passes no `-w`, so a worktree that has since been pruned is NOT
    # re-created: the session reopens with its cwd in the base checkout while
    # every path in its history points into a directory that no longer exists.
    # That is the worktree-edit-path pitfall arriving by default, and it is a
    # state only this change makes reachable — before it, neither verb had a
    # worktree, so resuming in the base repo was always right. Say it out loud
    # rather than letting it read as a normal reopen.
    if [[ -n "$worktree" && ! -d "$_DS_REPO/.claude/worktrees/$worktree" ]]; then
      print -u2 "dropset: worktree $worktree is gone — resuming in the base" \
        "checkout. Re-creating it restores TRACKED files only; an untracked" \
        "spec file written there is not recoverable, so work from Linear."
    fi
    claude --resume "$sid" --permission-mode auto "${model_flag[@]}"
  else
    claude --session-id "$sid" -n "$name" --permission-mode auto \
      "${model_flag[@]}" "${worktree_flag[@]}" "${prompt_arg[@]}"
  fi
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

# The profile id to fall back on when `DS_BEDROCK_MODEL` is unset.
#
# This MIRRORS the `AgentModelId` default published by `infra/aws/bedrock-agent.yml`
# as the `dropset-bedrock-agent-profile-id` export, rather than reading that
# export. Reading it would cost a CloudFormation round trip on every single
# session launch, for a value that changes about once a year — so the mirror is
# the deliberate trade, and the stack remains the source of truth. If the two
# ever disagree the stack wins, and the symptom is a session on last year's
# model rather than a failure.
_DS_BEDROCK_PROFILE_FALLBACK='us.anthropic.claude-opus-5'

# The fast tier, pinned so background sub-turns (session titles, the auto-mode
# classifier) bill to Bedrock credits alongside the primary model instead of
# silently falling back to the subscription.
_DS_BEDROCK_FAST_FALLBACK='us.anthropic.claude-haiku-4-5-20251001'

# Where a session's substrate choice is recorded. See `_ds_substrate_write`.
_DS_SUBSTRATE_DIR="$_DS_REPO/.claude/session-substrate"

# Internal: normalize a worktree tag. A bare number gets the `eng-` prefix, so
# `task 882` and `task eng-882` agree; anything else passes through untouched,
# which is what keeps a deliberate non-`eng` worktree name usable.
#
# ONE owner, because the start and resume sides MUST agree. They did not: the
# start side let a non-`eng` name through literally while the resume side did
# `eng-${1#eng-}`, which force-prefixes. So `task my-thing` recorded its
# substrate under `my-thing` and `task resume my-thing` looked for
# `eng-my-thing`, missed, and silently resumed on the seat — a miss in exactly
# the direction the design calls silent. Deriving both from here makes the
# agreement structural rather than something two call sites have to remember.
_ds_tag_of() {
  local tag="$1"
  [[ "$tag" == <-> ]] && tag="eng-$tag"
  print -r -- "$tag"
}

# Compose the model string a Bedrock launch exports as `ANTHROPIC_MODEL`.
#
# `DS_BEDROCK_MODEL` in the untracked runtime config wins and is used VERBATIM,
# suffix included — so switching model or context window is a one-line personal
# config edit with no repo change. Unset, this composes the fallback profile id
# above with the `[1m]` suffix.
#
# **The suffix is the whole reason this is a function.** The stack exports a
# bare profile id, Bedrock defaults a model with no suffix to the 200k window, and
# nothing anywhere reports the difference — so the failure is a session running
# at one fifth of its intended context, indistinguishable from a session that
# simply filled up. Appending it here rather than in the export keeps the
# stack's value honest (it really is just the profile id) and puts the
# composition somewhere a test can assert on.
#
# A configured string carrying no window suffix WARNS and is still used. The
# override is the operator's to make — refusing it would make the escape hatch
# unusable for exactly the deliberate case it exists for.
_ds_bedrock_model() {
  local model="$DS_BEDROCK_MODEL"
  if [[ -n "$model" ]]; then
    if [[ "$model" != *'[1m]' && "$model" != *'[200k]' ]]; then
      print -u2 "dropset: DS_BEDROCK_MODEL ('$model') has no context-window" \
        "suffix — Bedrock will use 200k, not 1M, and will not say so."
    fi
    print -r -- "$model"
    return 0
  fi
  print -r -- "${_DS_BEDROCK_PROFILE_FALLBACK}[1m]"
}

# Record the substrate a session launched on, keyed by worktree tag.
#
# The key is a parameter rather than a hard-coded tag because a base-repo
# session would key on its computed session id — but **no verb does that
# today**, and the only two call sites both pass a worktree tag. Said plainly
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
# so every path returns 0. The cost of a missing marker is one conservative
# default, which is the next function.
_ds_substrate_write() {
  local key="$1" substrate="$2"
  mkdir -p "$_DS_SUBSTRATE_DIR" 2>/dev/null || return 0
  print -r -- "$substrate" >| "$_DS_SUBSTRATE_DIR/$key" 2>/dev/null
  return 0
}

# Read back a recorded substrate. Prints `bedrock` or `seat`.
#
# **Absent means seat**, deliberately: every session that existed before markers
# did was a seat session, and the conservative error is spending the
# subscription window rather than spending credits on something unintended. A
# garbage value reads as seat for the same reason.
_ds_substrate_read() {
  local marker="$_DS_SUBSTRATE_DIR/$1" recorded=''
  [[ -f "$marker" ]] && recorded="$(cat "$marker" 2>/dev/null)"
  if [[ "$recorded" == 'bedrock' ]]; then
    print -r -- 'bedrock'
  else
    print -r -- 'seat'
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
_ds_bedrock_env() {
  local model
  model="$(_ds_bedrock_model)"

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
  export ANTHROPIC_MODEL="$model"
  export ANTHROPIC_DEFAULT_HAIKU_MODEL="${DS_BEDROCK_FAST_MODEL:-$_DS_BEDROCK_FAST_FALLBACK}"
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
    print -u2 '         config, or run `task local <n>` for a seat session.'
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

# Clear the Bedrock exports from the calling shell.
#
# THIS IS NOT TIDINESS, IT IS THE SEAT PIN. These helpers export into the
# CALLING shell — they have to, since a child process could not set the
# environment `claude` inherits — so the variables outlive the session that set
# them. Run `task 1234`, quit it, and that tab is still a Bedrock tab: the next
# `plan` in it would silently run against credits with the Fable pin dropped.
# The absence of `CLAUDE_CODE_USE_BEDROCK` IS how a seat launch is expressed, so
# a seat verb has to make that absence true rather than merely assert it.
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

# Seat verbs call this: warn if the shell arrived carrying Bedrock exports, then
# clear them. $1 is the verb name, for the message.
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
#   task <n>          on Bedrock (the default substrate for implementation work)
#   task local <n>    on the seat, for work that needs web research
#   task resume <n>   resume by number, on the substrate it launched with
#
# `local` is a literal first word rather than a flag: these helpers do no flag
# parsing today, and the word reads better at the call site than `-l` would.
#
# Creates the `eng-###` worktree directory whose branch arrives named
# `worktree-eng-###` — there is no CLI flag to drop the prefix, so `init-pr`
# renames it.
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
#     prompt box, the `/resume` picker, and the terminal title. `task resume` resolves
#     by directory, so this is for the human, not the tooling.
#   * `/init-pr` as the initial prompt, so the bootstrap runs without being
#     asked for — the same trick `plan` and `housekeeping` use for their own skills.
task() {
  case "$1" in
    local)
      shift
      _ds_task_start "$1" seat
      ;;
    resume)
      shift
      _ds_task_resume "$1"
      ;;
    '')
      print -u2 'Usage: task <n> | task local <n> | task resume [n]'
      return 1
      ;;
    *)
      _ds_task_start "$1" bedrock
      ;;
  esac
}

# Internal: the worktree launch itself. $1 tag-or-number, $2 substrate.
_ds_task_start() {
  local tag="$1" substrate="$2"

  if [[ -z "$tag" ]]; then
    print -u2 'Usage: task <n> | task local <n>'
    return 1
  fi

  # Shared with the resume side, which is the whole point — see `_ds_tag_of`.
  tag="$(_ds_tag_of "$tag")"

  _ds_base || return 1
  _ds_secrets

  if [[ "$substrate" == 'bedrock' ]]; then
    _ds_bedrock_env || return 1
  else
    _ds_seat_guard 'task local'
  fi

  # Recorded BEFORE the launch, not after: `claude` blocks for the life of the
  # session, so an after-the-fact write would land whenever the operator
  # happened to quit — and never at all if the terminal were closed instead.
  _ds_substrate_write "$tag" "$substrate"

  claude -w "$tag" -n "$tag" --permission-mode auto /init-pr
}

# Resume a worktree session by number: `task resume 814` resolves to the
# `eng-814` worktree and continues its most recent conversation there, on the
# substrate that session launched with. The number-to-worktree resolution is
# the whole point — you resume a number, not a UUID.
#
# **Where the session actually lives is not guessable from the directory**, which
# is why this delegates. `task` runs `claude -w <tag>` from the BASE repo, so
# Claude Code files that session's transcript under the base repo's project slug
# even though every `cwd` stamp in it points into the worktree — and no project
# directory for the worktree ever exists. This used to `cd` into the
# worktree and run `claude --continue` on the assumption that per-directory
# addressing selects the session; for a `-w`-launched session it selects nothing
# and reports "no conversation found" while the session sits intact under
# another slug. Sessions that had been resumed from inside their worktree once
# before DID have a worktree-slug transcript, which masked the bug and made it
# look intermittent.
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
    claude --resume
    return
  fi

  local tag
  tag="$(_ds_tag_of "$1")"

  # Re-export whatever this session launched with, BEFORE moving the shell.
  # Absent marker = seat, so a session predating markers resumes as it always
  # did. Order matters: `_ds_bedrock_env` can fail (no token), and resolving
  # after the `cd` below would leave the operator relocated into the worktree
  # with no session and no explanation of the move.
  if [[ "$(_ds_substrate_read "$tag")" == 'bedrock' ]]; then
    _ds_bedrock_env || return 1
  else
    _ds_seat_guard 'task resume'
  fi

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
      claude --continue
      ;;
    resume)
      # The `-w` case: resume by id from the base repo. This is the form that
      # was missing, and the only one that reaches such a session.
      claude --resume "$sid"
      ;;
    *)
      # Nothing resolved — the worktree was pruned, or the session never
      # started. `--resume <tag>` filters the picker rather than resuming, which
      # is a pick rather than a resume, but it is the last form that can reach
      # anything.
      claude --resume "$tag"
      ;;
  esac
}

# Start OR resume an EXPLORE session — research, audits, and any other task
# whose deliverable is a spec or a findings document rather than a code change.
#
#   explore <n>       an explore task with a Linear issue: worktree `eng-<n>`
#   explore <name>    one without: worktree `exp-<name>`
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
# substantive naming call. `explore 1196` lands in `eng-1196` — not `exp-1196` —
# so an explore session inherits every mechanism already keyed on `eng-###`:
# `cdds 1196` reaches it, `housekeeping` prunes it on the issue's status type,
# and an absent substrate marker correctly reads as seat. A worktree named
# anything else is invisible to the first two.
# Note the rationale is those named benefits — addressability — and NOT
# "protecting the worktree": its contents are expendable by construction, and
# framing the eng-keying as protection would make the worktree sound precious,
# which the convention doc explicitly warns against. The session's DISPLAY name
# stays `exp-<n>` so the fleet listing still reads by role: `eng-*`
# implementers, `plan-*`, `ceo-*` architecture, `exp-*` research.
#
# THE COST OF SHARING THE TAG: `eng-<n>` now names a worktree that two session
# kinds can claim, with different substrates and different model pins, while the
# substrate marker keys on the tag alone. That is why nothing is written to it
# below, and it is the root the model-pin gap shares — see ENG-1402.
#
# ONE CAVEAT, stated because an earlier draft of this comment claimed the
# benefit without it: `fleet` and `task resume <n>` do REACH such a session, but
# they resume it through `_ds_task_resume`, which restores the substrate and
# NOT the model — so it comes back on the saved default rather than the Fable
# pin. The substrate half is right (an explicit `seat` marker, and an absent one
# would also read as seat), and only the pin is lost. **`explore <n>` is the
# resume verb for an explore session**; it is idempotent precisely so there is a
# path that restores the pin. Teaching `task resume` to re-pin a model is real
# new machinery — a per-worktree model marker beside the substrate one — and is
# deliberately left to its own change rather than smuggled in here.
#
# THE `resume` TWIN IS GONE, and dropping it removes a wart rather than a
# feature. `explore <name>` now creates the session if absent and resumes it if
# present, so `explore resume <name>` would be a pure synonym — and the old
# resume branch could not resume deterministically anyway: `-r/--resume` matches
# on session ID, a name is not one, so a bare name PRE-FILTERED THE INTERACTIVE
# PICKER. Keying on `_ds_topic_sid` computes an id the way `architect` does,
# which is what makes the fix possible. A name is now REQUIRED for the same
# reason `architect` requires a topic: it names a worktree.
#
# This folds two older verbs into one: the unnamed form and the named form were
# separate launchers with identical bodies bar one flag. The idempotency above
# folds in the third.
#
# **SEAT-ONLY, and Fable-pinned.** Both halves are ratified and they are the
# same decision. Explore work is thinking-heavy, so it runs the top tier like
# `plan` and `architect` do — and a Fable-class model on Bedrock falls under
# the account's standing AWS human-review retention opt-in, which the seat is
# free of. There is deliberately no `explore local`: seat is the only
# substrate, so the word would be a no-op. The worktree home changed where this
# verb runs; it did not change the substrate, which is a capability call.
#
# The model pin, the permission mode and the start-or-resume probe all now come
# from `_ds_session`, which gets each right by construction. The old hand-rolled
# resume branch had to remember the pin itself — `--model` and
# `--permission-mode` are per-session flags, so honoring them only on the create
# path silently drops to the saved default on every reopen, and an explore
# session is reopened often. That slip is exactly what `_ds_daily_session`
# documents above ("still works, so nobody notices"), and this verb reproduced
# it until review caught it. Sharing the core retires the whole class.
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
  # shape a branch would take — the same validation `architect` applies to its
  # topic, and binding for the same reason.
  if [[ ! "$raw" =~ '^[a-z0-9][a-z0-9-]*$' ]]; then
    print -u2 'explore: name must be lowercase letters, digits and dashes'
    return 1
  fi

  # NORMALIZE AN ISSUE REFERENCE TO ITS BARE NUMBER FIRST, before the branch
  # below reads it. `eng-1196` and `1196` name one issue and must produce one
  # worktree, one session name and one session id — exactly the agreement
  # `_ds_tag_of`'s comment establishes for `task 882` / `task eng-882`.
  #
  # Deciding the branch before normalizing is what broke that: the shape
  # validation admits `eng-1196`, but the `<->` test below does not match it, so
  # it fell through to the free-form branch and produced worktree
  # `exp-eng-1196` with no bootstrap prompt — invisible to `cdds 1196`, to
  # `fleet`, and to status-type pruning, which are the very mechanisms the
  # eng-keying exists to inherit. The comment claiming it went "through the ONE
  # tag owner" was true of the tag and false of the branch decision.
  #
  # `10#` forces base ten. Without it `$(( 0123 ))` is octal 83, so a padded
  # number would silently name a different issue than it reads as. Stripping the
  # padding also keeps `explore 0123` and `explore 123` on one worktree and one
  # session id instead of two.
  if [[ "$raw" == <-> || "$raw" == eng-<-> ]]; then
    raw=$(( 10#${raw#eng-} ))
    if (( raw == 0 )); then
      print -u2 'explore: 0 is not a Linear issue number'
      return 1
    fi
  fi

  # A bare number is an issue-keyed explore task, so it goes through the ONE
  # tag owner `task` uses rather than force-prefixing here — see `_ds_tag_of`
  # for what a second spelling of this cost last time.
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
    tag="$(_ds_tag_of "$raw")"
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
    tag="exp-$raw"
  fi
  name="exp-$raw"

  _ds_seat_guard 'explore'
  # NO SUBSTRATE MARKER IS WRITTEN HERE, DELIBERATELY, and the reason is the
  # whole hazard of keying this worktree to `eng-<n>`: the marker is keyed on the
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
  _ds_session "$(_ds_topic_sid explore "$raw")" "$name" "$prompt" \
    claude-fable-5 "$tag"
}

# Start OR resume today's PLANNING session. Takes no argument: the name is
# derived from the date.
#
# Idempotent by design — a planning session is opened and reopened many times
# in a day, and having to remember which state it is in is the friction this
# removes. An `rpaps` twin was considered and rejected for that reason.
#
# Three things it makes deterministic, each of which used to be a manual step
# the operator could forget:
#
#   * The model. Planning sessions run the most capable model deliberately —
#     fidelity over tokens — and `--model` at launch is the only session-wide
#     mechanism. The `plan` skill's frontmatter is belt-and-braces for a
#     mid-session `/plan`, not a substitute.
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
#     AFTER `_ds_seat_guard` on
#     purpose: the guard clears an `AWS_REGION` inherited from a previous `task`
#     in the same tab, and the AWS CLI reads that variable, so probing first
#     would probe under the Bedrock launcher's environment rather than the
#     operator's own.
#
# `date +%-d` gives an unpadded day, so the 5th is `plan-5`, not `plan-05`.
plan() {
  if [[ -n "$1" ]]; then
    print -u2 'Usage: plan   (no arguments; the name is derived from the date)'
    return 1
  fi
  _ds_seat_guard 'plan'
  _ds_aws_login 'plan' || return 1
  _ds_daily_session plan "plan-$(date +%-d)" /plan claude-fable-5
}

# Start OR resume today's HOUSEKEEPING session — the same contract as `plan`,
# so a day's upkeep is one verb rather than a hand-started session.
#
# No model pin, deliberately: housekeeping is upkeep, not board decisions, so
# it does not inherit the planning tier. It runs on the saved default.
#
# **Seat, deliberately, and this one is not a capability call.** Housekeeping
# could run on Bedrock perfectly well; the operator uses it to OPEN the 5-hour
# subscription window at the start of a day, which only a seat session does.
# Moving it to Bedrock would silently retire that.
#
# It carries the same `_ds_aws_login` gate as `plan`, with the same two
# warn-and-launch escapes, and for the same reason: this pass reports spend and
# runs the monthly permission refresh, both of which need credentials the
# session cannot obtain for itself. `architect` is
# deliberately NOT gated — it argues design rather than reading cost data, and a
# browser login is a poor thing to stand between the operator and a design
# thought. Add it only if an architect session is actually found wanting one.
housekeeping() {
  if [[ -n "$1" ]]; then
    print -u2 'Usage: housekeeping   (no arguments; name derived from the date)'
    return 1
  fi
  _ds_seat_guard 'housekeeping'
  _ds_aws_login 'housekeeping' || return 1
  _ds_daily_session housekeeping "housekeeping-$(date +%-d)" /housekeeping ''
}

# Start OR resume an ARCHITECT session on one topic — the CEO hat. Same seat
# quality as `plan` and the same idempotency; a different job.
#
# Takes a TOPIC and keys the session on it, so each long-horizon design thread
# gets its own resumable session and parallel threads never share context:
#
#   architect volatility-telemetry
#
# The name is `ceo-<topic>`, which makes the fleet listing read by role —
# `eng-*` implementers, `plan-*` planning, `ceo-*` architecture.
#
# Model-pinned like `plan` for the same reason: this session argues strategy,
# and fidelity beats tokens. It writes nothing to the board — see the skill.
#
# **It runs in its own worktree, named `ceo-<topic>`, and that worktree is
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
# The worktree name is deliberately NOT an `eng-###`: an architect topic has no
# issue of its own, and the `ceo-` prefix keeps the fleet listing readable by
# role.
architect() {
  local topic="$1"
  if [[ -z "$topic" || -n "$2" ]]; then
    print -u2 'Usage: architect <topic>   (e.g. architect volatility-telemetry)'
    return 1
  fi
  # A topic reaches a session name, a WORKTREE name, a branch and a filename, so
  # keep it to the shape a branch would take rather than sanitizing something
  # surprising later. The worktree home is what makes the branch shape binding
  # rather than merely tidy.
  if [[ ! "$topic" =~ '^[a-z0-9][a-z0-9-]*$' ]]; then
    print -u2 'architect: topic must be lowercase letters, digits and dashes'
    return 1
  fi
  _ds_seat_guard 'architect'
  _ds_session "$(_ds_topic_sid architect "$topic")" \
    "ceo-$topic" /architect claude-fable-5 "ceo-$topic"
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
