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
