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

# Where the untracked secret coordinates live. Deliberately OUTSIDE the repo:
# a path under the checkout could be committed by an errant `git add -A`, and
# this boundary should not depend on .gitignore staying correct.
_DS_SECRETS_FILE="${DROPSET_SECRETS_FILE:-$HOME/.config/dropset/secrets.zsh}"

# `cd` to the base repo checkout. The starting point for anything that must not
# run inside a worktree (`housekeeping`, a planning session).
cdds() {
  cd "$_DS_REPO" || return 1
}

# Internal: the same move, for helpers that must launch from the base repo
# rather than trusting the shell's cwd.
_ds_base() {
  cd "$_DS_REPO" || return 1
}

# Resolve LINEAR_API_KEY and GITHUB_MCP_PAT from 1Password.
#
# The untracked file at $_DS_SECRETS_FILE supplies the coordinates, and only
# the coordinates:
#
#   DS_OP_ACCOUNT='<account>.1password.com'
#   DS_OP_LINEAR_REF='op://<vault>/<linear-item>/credential'
#   DS_OP_GITHUB_REF='op://<vault>/<github-item>/credential'
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
#     hand, and the escape hatch if the coordinates file is absent entirely.
#   * `--account` is explicit because the laptop is signed into more than one
#     1Password account, and a bare `op read` cannot disambiguate.
#   * An unresolved secret WARNS rather than failing the launch. An empty key
#     otherwise surfaces much later as an opaque MCP error mid-session, which
#     is far worse to debug than one line at startup.
_ds_secrets() {
  [[ -r "$_DS_SECRETS_FILE" ]] && source "$_DS_SECRETS_FILE"

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
  local kind="$1" name="$2" prompt="$3" model="$4"

  local sid slug transcript
  local -a model_flag
  [[ -n "$model" ]] && model_flag=(--model "$model")

  _ds_base || return 1
  _ds_secrets

  sid="$(_ds_daily_sid "$kind")"
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

# Start a WORKTREE session. Creates the `eng-###` worktree directory whose
# branch arrives named `worktree-eng-###` — there is no CLI flag to drop the
# prefix, so `init-pr` renames it. The implementation-session entry point.
aps() {
  if [[ -z "$1" ]]; then
    print -u2 'Usage: aps <tag>'
    return 1
  fi
  # A bare number gets the `eng-` prefix, so `aps 882` and `aps eng-882` agree
  # and the aps→raps pair composes: `raps` resolves `eng-<n>`, so without this
  # `aps 882` would create a worktree named `882` that `raps 882` then reports
  # as missing. Only an all-digit argument is rewritten — a deliberate non-`eng`
  # worktree name still passes through untouched.
  local tag="$1"
  [[ "$tag" == <-> ]] && tag="eng-$tag"

  _ds_base || return 1
  _ds_secrets
  claude -w "$tag"
}

# Resume a worktree session by number: `raps 814` resolves to the `eng-814`
# worktree and continues its most recent conversation there. The
# number-to-worktree resolution is the whole point — you resume a number, not
# a UUID.
raps() {
  if [[ -z "$1" ]]; then
    print -u2 'Usage: raps <n>'
    return 1
  fi
  local dir="$_DS_REPO/.claude/worktrees/eng-${1#eng-}"
  if [[ ! -d "$dir" ]]; then
    print -u2 "raps: no worktree at $dir"
    return 1
  fi
  cd "$dir" || return 1
  _ds_secrets
  # `-c/--continue` is per-directory, which is exactly the addressing this
  # helper wants: the cd above has already selected the session.
  claude --continue
}

# Start a NAMED session in the current directory (no worktree). The
# general-purpose named-session entry point.
naps() {
  if [[ -z "$1" ]]; then
    print -u2 'Usage: naps <name>'
    return 1
  fi
  _ds_secrets
  claude -n "$1"
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
