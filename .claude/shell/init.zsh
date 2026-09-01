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
# time (see `paps` below). Committing the functions makes the doc describe
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
# as the base repo — `cdds` lands in it, `raps` looks for worktrees nested
# inside it. It fails quietly and plausibly, which is the worst way to fail, so
# say it out loud. (The file is identical in every checkout; only which copy
# gets sourced matters.)
if [[ "$_DS_REPO" == */.claude/worktrees/* ]]; then
  print -u2 "dropset shell helpers: sourced from a worktree copy" \
    "($_DS_REPO) — source the base checkout's copy instead"
fi

# `cd` to the base repo checkout. The starting point for anything that must not
# run inside a worktree (`housekeeping`, a planning session).
#
# **It does not `git pull`, deliberately.** The operator's own copy did, and the
# committed one is not adopting it: a navigation command should not make a
# network call. A pull can be slow, can fail, and can print — so a bare `cd`
# would sometimes leave the shell somewhere unexpected, or leave a `cd` looking
# like it errored. And it is not needed: `housekeeping` step 1 fast-forwards
# `main` as its **first** step, where the operation is visible, its failure is
# reportable, and the pass that depends on fresh skills is the thing asking for
# it. Fast-forwarding on every `cdds` would move `main` under a session that
# never wanted it.
cdds() {
  cd "$_DS_REPO" || return 1
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
_ds_base() {
  cd "$_DS_REPO" || return 1
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
# nobody notices" slip `paps` exists to remove.
_ds_daily_session() {
  _ds_session "$(_ds_daily_sid "$1")" "$2" "$3" "$4"
}

# Internal: the start-or-resume core, given an already-computed session id.
#
# Split out from `_ds_daily_session` so a session keyed by something other than
# the date can reuse it unchanged. `paps` and `haps` key on the day; `caps` keys
# on a TOPIC, because a design thread outlives a day and resuming it tomorrow is
# the whole point. Everything below the id — the idempotency, the model pin
# riding both branches, the permission mode — is identical for both, and the
# operator's stated abstraction is that these launchers differ only in the
# briefing.
#
#   $1 session id   $2 display name   $3 initial prompt
#   $4 model to pin, or "" for the saved default
_ds_session() {
  local sid="$1" name="$2" prompt="$3" model="$4"

  local slug transcript
  local -a model_flag
  [[ -n "$model" ]] && model_flag=(--model "$model")

  _ds_base || return 1
  _ds_secrets

  # The transcript path Claude Code writes: the project slug replaces every `/`
  # and `.` in the cwd with `-` — the same rule .claude/tools/firm_last.py
  # encodes for reading transcripts back.
  slug="${PWD//[\/.]/-}"
  transcript="$HOME/.claude/projects/$slug/$sid.jsonl"

  if [[ -f "$transcript" ]]; then
    claude --resume "$sid" --permission-mode acceptEdits "${model_flag[@]}"
  else
    claude --session-id "$sid" -n "$name" --permission-mode acceptEdits \
      "${model_flag[@]}" "$prompt"
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

# Start a WORKTREE session. Creates the `eng-###` worktree directory whose
# branch arrives named `worktree-eng-###` — there is no CLI flag to drop the
# prefix, so `init-pr` renames it. The implementation-session entry point.
#
# Three things ride the launch, and each was a parity gap when this helper was
# committed — the operator's own profile had been passing all three, and the
# committed copy silently did not, so a session started with `aps` differed
# from one started by hand:
#
#   * `--permission-mode acceptEdits`. The shared `settings.local.json` sets no
#     default permission mode, so without this an implementation session starts
#     in the default mode and prompts on every edit. This is the gap with teeth.
#   * `-n "$tag"` — a display name, so the session is identifiable in the
#     prompt box, the `/resume` picker, and the terminal title. `raps` resolves
#     by directory, so this is for the human, not the tooling.
#   * `/init-pr` as the initial prompt, so the bootstrap runs without being
#     asked for — the same trick `paps` and `haps` use for their own skills.
aps() {
  _ds_base || return 1
  _ds_secrets

  # No tag: a plain session in the base repo. This form is the operator's, and
  # dropping it was a parity gap rather than a decision — it is the entry point
  # for work that is not tied to a worktree yet.
  if [[ -z "$1" ]]; then
    claude --permission-mode acceptEdits
    return
  fi

  # A bare number gets the `eng-` prefix, so `aps 882` and `aps eng-882` agree
  # and the aps→raps pair composes: `raps` resolves `eng-<n>`, so without this
  # `aps 882` would create a worktree named `882` that `raps 882` then reports
  # as missing. Only an all-digit argument is rewritten — a deliberate non-`eng`
  # worktree name still passes through untouched.
  local tag="$1"
  [[ "$tag" == <-> ]] && tag="eng-$tag"

  claude -w "$tag" -n "$tag" --permission-mode acceptEdits /init-pr
}

# Resume a worktree session by number: `raps 814` resolves to the `eng-814`
# worktree and continues its most recent conversation there. The
# number-to-worktree resolution is the whole point — you resume a number, not
# a UUID.
raps() {
  # No number: the picker, from wherever the shell already is. The operator's
  # form, and worth keeping for a reason the tag form cannot cover — a session
  # whose worktree has already been pruned is still reachable this way.
  if [[ -z "$1" ]]; then
    _ds_secrets
    claude --resume
    return
  fi

  local tag="eng-${1#eng-}"
  local dir="$_DS_REPO/.claude/worktrees/$tag"

  if [[ -d "$dir" ]]; then
    cd "$dir" || return 1
    _ds_secrets
    # `-c/--continue` is per-directory, which is exactly the addressing this
    # helper wants: the cd above has already selected the session.
    claude --continue
    return
  fi

  # The worktree is gone — pruned after a merge, say — but the transcript
  # outlives it, so fall back to the base repo rather than refusing. `--continue`
  # is wrong here precisely because the cd no longer selects the right session:
  # it would continue whatever ran last in the base repo. `--resume <tag>` filters
  # the picker instead, which is a pick rather than a resume, and is the only
  # form that still reaches the session.
  _ds_base || return 1
  _ds_secrets
  claude --resume "$tag"
}

# Start a NAMED session in the BASE REPO (no worktree). The general-purpose
# named-session entry point.
#
# It runs `_ds_base` first, so `naps <name>` is `cdds` plus a named session.
# That is the operator's behavior and the intended one: a named session is for
# board or repo-wide work, which belongs in the base checkout, not in whatever
# worktree the shell happened to be sitting in. An earlier committed revision
# omitted the `_ds_base` and so inherited the caller's directory — a parity gap,
# not a decision, and a quiet one: the session still starts, just somewhere
# unintended.
#
# `--permission-mode acceptEdits` for the same reason as `aps`: the shared
# settings file sets no default, so omitting it starts every session in the
# default mode. It is deliberate here too rather than inherited — a named
# session is a working session, not a read-only one.
naps() {
  if [[ -z "$1" ]]; then
    print -u2 'Usage: naps <name>'
    return 1
  fi
  _ds_base || return 1
  _ds_secrets
  claude -n "$1" --permission-mode acceptEdits
}

# Resume a named session by the same name — the counterpart to `naps`, as
# `raps` is to `aps`, so a long-running session survives a closed terminal.
#
# CAVEAT, and it is the same one that made the old `paps` wrong: a bare name
# here PRE-FILTERS THE INTERACTIVE PICKER rather than resuming deterministically
# — `-r/--resume` matches on session ID, and a name is not one. Expect to pick
# from a list. `paps`/`haps` avoid this entirely by computing their own id.
rnaps() {
  if [[ -z "$1" ]]; then
    print -u2 'Usage: rnaps <name>'
    return 1
  fi
  _ds_base || return 1
  _ds_secrets
  claude --resume "$1"
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
#
# `date +%-d` gives an unpadded day, so the 5th is `plan-5`, not `plan-05`.
paps() {
  if [[ -n "$1" ]]; then
    print -u2 'Usage: paps   (no arguments; the name is derived from the date)'
    return 1
  fi
  _ds_daily_session plan "plan-$(date +%-d)" /plan claude-fable-5
}

# Start OR resume today's HOUSEKEEPING session — the same contract as `paps`,
# so a day's upkeep is one verb rather than a hand-started session.
#
# No model pin, deliberately: housekeeping is upkeep, not board decisions, so
# it does not inherit the planning tier. It runs on the saved default.
haps() {
  if [[ -n "$1" ]]; then
    print -u2 'Usage: haps   (no arguments; the name is derived from the date)'
    return 1
  fi
  _ds_daily_session housekeeping "housekeeping-$(date +%-d)" /housekeeping ''
}

# Start OR resume an ARCHITECT session on one topic — the CEO hat. Same seat
# quality as `paps` and the same idempotency; a different job.
#
# Takes a TOPIC and keys the session on it, so each long-horizon design thread
# gets its own resumable session and parallel threads never share context:
#
#   caps volatility-telemetry
#
# The name is `ceo-<topic>`, which makes the fleet listing read by role —
# `eng-*` implementers, `plan-*` planning, `ceo-*` architecture.
#
# Model-pinned like `paps` for the same reason: this session argues strategy,
# and fidelity beats tokens. It writes nothing to the board — see the skill.
caps() {
  local topic="$1"
  if [[ -z "$topic" || -n "$2" ]]; then
    print -u2 'Usage: caps <topic>   (e.g. caps volatility-telemetry)'
    return 1
  fi
  # A topic reaches a session name and a filename, so keep it to the shape a
  # branch would take rather than sanitizing something surprising later.
  if [[ ! "$topic" =~ '^[a-z0-9][a-z0-9-]*$' ]]; then
    print -u2 'caps: topic must be lowercase letters, digits and dashes'
    return 1
  fi
  _ds_session "$(_ds_topic_sid architect "$topic")" \
    "ceo-$topic" /architect claude-fable-5
}

# Resume the whole FLEET: one iTerm tab per in-flight Linear issue, each with
# its session resumed and flagged green for attention. The batch counterpart to
# `raps`, for after a machine restart.
#
# `faps` prints the plan and opens nothing; `faps go` applies it. The default is
# read-only deliberately — this one verb can open many tabs and resume many
# sessions, so seeing the list first is worth one extra word.
#
# It resolves the fleet itself (state type `started`, so In Progress *and* In
# Review) and skips any issue whose tab is already open, so it is safe to run
# twice. The deterministic work — the Linear query, the tag derivation, the
# already-live check, the AppleScript — lives in the committed tool; this is the
# thin verb over it, per the skill-tooling convention.
faps() {
  _ds_base || return 1
  _ds_secrets
  if [[ "$1" == "go" ]]; then
    python3 "$_DS_REPO/.claude/tools/fleet_resume.py" --apply
  elif [[ -z "$1" ]]; then
    python3 "$_DS_REPO/.claude/tools/fleet_resume.py"
  else
    print -u2 'Usage: faps [go]   (no argument = show the plan; `go` = open the tabs)'
    return 1
  fi
}
