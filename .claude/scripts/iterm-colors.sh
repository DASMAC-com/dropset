#!/bin/bash
# Shared palette + emit helpers, sourced by the other iterm-* scripts.
#
# The tab is a coarse attention signal: green when Claude wants a reply (it is
# done, or asking you a question), yellow when it needs a permission approval or
# is editing a file (so you go to it quickly), and no tint while working or
# after you acknowledge it with the attend shortcut. iTerm mutes the color of a
# non-selected tab and there is no setting to stop it, so the tints are picked
# bright enough to stay legible.
#
# By default only the tab is tinted; the window keeps the profile default. Flip
# PAINT_WINDOW_BG to 1 to also paint the window background per state (the code
# for it is kept intact below).
#
# This file is sourced by the other iterm-* scripts, so several vars it defines
# are referenced only there; silence shellcheck's "appears unused" for the file.
# shellcheck disable=SC2034
PAINT_WINDOW_BG=0

# The four semantic states. The value is the *window-background* hex (used only
# when PAINT_WINDOW_BG=1); bg_to_tab maps each to the tab tint that is always
# applied. Keeping the state keyed by a bg hex preserves the window-bg mode.
STATE_NEUTRAL="16191e"    # working / acknowledged -> no tint
STATE_REPLY="080c2a"      # done, or asking a question -> green (reply wanted)
STATE_PERMISSION="3a2c08" # permission request or file edit -> yellow (go now)
STATE_MARK="082a0c"       # attend mark -> green

# Where per-TTY state and monitor pid files live. Every script derives its file
# paths from these, so a rename only happens here.
STATE_PREFIX="/tmp/iterm-color-"
MONITOR_PID_PREFIX="/tmp/iterm-monitor-"
# The $ITERM_SESSION_ID (UUID) -> tty map written by shell startup (see the
# local-integrations convention doc); read by the attend toggle.
SESSION_TTY_PREFIX="/tmp/iterm-session-tty-"

# How long a permission-prompt yellow may sit UNREFRESHED before the monitor
# heals it back to neutral.
#
# This exists because nothing on the denial path clears yellow. The painter's
# only neutral-painting events are PostToolUse, UserPromptSubmit and a
# non-permission PreToolUse — and a tool that a PreToolUse guard blocks *after*
# the operator has already approved it runs none of them. The tab then stays
# yellow over a session that needs nothing, which is the measured wedge: approve,
# a guard denies, the tool errors red, and the tint never clears.
#
# The heal keys on a fact the painter already relies on elsewhere: the harness
# RE-FIRES permission_prompt periodically while a prompt waits. So a prompt that
# genuinely still wants an answer keeps refreshing this sentinel and never goes
# stale, while a resolved one — approved or denied — stops being re-fired and
# heals.
#
# The default is deliberately GENEROUS, because the two errors are not
# symmetric. Healing too eagerly drops the yellow on a prompt that really is
# waiting, and a missed prompt stalls the session — strictly worse than a
# lingering tint, which is merely the annoyance being fixed. Two minutes bounds
# the wedge without racing a re-fire interval this integration does not control.
# Raise it with ITERM_PERM_WAIT_STALE_SECONDS if that interval proves longer.
PERM_WAIT_STALE_SECONDS="${ITERM_PERM_WAIT_STALE_SECONDS:-120}"

# Per-tty sentinel recording when a permission-prompt yellow was last refreshed.
perm_wait_path() { printf '%s%s.permwait' "$STATE_PREFIX" "$1"; } # $1 = tty base

# Per-tty state file (the color the monitor paints from).
state_file_path() { printf '%s%s' "$STATE_PREFIX" "$1"; } # $1 = tty base

# Clear a permission yellow that has stopped being refreshed. $1 = tty base.
#
# Lives HERE rather than in iterm-monitor.sh so it can be sourced and tested:
# the monitor ends in an infinite poll loop, so sourcing that file to reach this
# function would hang. Behavior is unchanged by the move — it is still the
# monitor that calls it, on the monitor's slow cadence.
#
# Writes the state file rather than painting directly, so the monitor's normal
# change-detection does the paint and there stays one painting path.
#
# All four guards matter, and each is a case that must NOT heal:
#   * no sentinel      — nothing is waiting (an edit-tool yellow carries none,
#                        and its lifetime is not governed by permission
#                        re-fires, so it must never be healed);
#   * state not yellow — a green or neutral tab is not a lingering permission;
#   * unreadable mtime — answer unknown, so leave it alone;
#   * fresh sentinel   — the prompt is still being re-fired, i.e. still waiting.
heal_stale_permission() {
  local tty_base="$1" sentinel state_file mtime now
  [ -n "$tty_base" ] || return 0
  sentinel="$(perm_wait_path "$tty_base")"
  state_file="$(state_file_path "$tty_base")"
  [ -f "$sentinel" ] || return 0
  [ "$(cat "$state_file" 2>/dev/null)" = "$STATE_PERMISSION" ] || return 0
  mtime=$(stat -f %m "$sentinel" 2>/dev/null) || return 0
  [ -n "$mtime" ] || return 0
  now=$(date +%s)
  if [ "$((now - mtime))" -ge "$PERM_WAIT_STALE_SECONDS" ]; then
    rm -f "$sentinel"
    echo "$STATE_NEUTRAL" >"$state_file"
  fi
}

# Map a state hex to the *tab* tint hex (or the literal "default" for no tint).
bg_to_tab() {
  case "$1" in
    "$STATE_NEUTRAL") printf 'default' ;;   # working / acknowledged -> no tint
    "$STATE_REPLY") printf '35b54a' ;;      # reply wanted -> green
    "$STATE_PERMISSION") printf 'e0b020' ;; # permission / edit -> yellow
    "$STATE_MARK") printf '35b54a' ;;       # attend mark -> green
    *) printf '%s' "$1" ;;                  # unknown: tab = bg
  esac
}

# Emit the iTerm SetColors escape for a state hex (no trailing newline).
# Always tints the tab. When PAINT_WINDOW_BG=1 the window background tracks the
# state; otherwise it is actively held at the profile default, so a color left
# over from a previous paint (or from flipping the flag off) clears itself.
emit_set_colors() { # $1 = state hex
  if [ "$PAINT_WINDOW_BG" = "1" ]; then
    printf '\033]1337;SetColors=tab=%s,bg=%s\a' "$(bg_to_tab "$1")" "$1"
  else
    printf '\033]1337;SetColors=tab=%s,bg=default\a' "$(bg_to_tab "$1")"
  fi
}

# Emit the escape that clears our coloring back to iTerm defaults.
emit_reset() {
  if [ "$PAINT_WINDOW_BG" = "1" ]; then
    printf '\033]1337;SetColors=tab=default,bg=default\a'
  else
    printf '\033]1337;SetColors=tab=default\a'
  fi
}

# Walk up the process tree to this session's controlling TTY and echo its
# /dev/ttys* path (empty + non-zero exit if none is found). Shared by the
# painter and the start/stop hooks so the walk lives in one place.
resolve_tty() {
  local pid=$PPID t
  while [ "$pid" -gt 1 ] 2>/dev/null; do
    t=$(ps -o tty= -p "$pid" 2>/dev/null | tr -d ' ')
    if [ -n "$t" ] && [ "$t" != "??" ] && [ -c "/dev/$t" ]; then
      echo "/dev/$t"
      return 0
    fi
    pid=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ')
  done
  return 1
}
