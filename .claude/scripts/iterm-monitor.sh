#!/bin/bash
# Per-TTY daemon: poll this session's state file and apply the iTerm tab (and
# window, if enabled) color, and continuously suppress iTerm attention/badge
# notifications so only our coloring signals state.
# Usage: iterm-monitor.sh /dev/ttysXXX

# shellcheck source=.claude/scripts/iterm-colors.sh
# shellcheck disable=SC1090,SC1091
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/iterm-colors.sh"

TTY_PATH="$1"
TTY_BASE="$(basename "$TTY_PATH")"
STATE_FILE="$STATE_PREFIX$TTY_BASE"
LAST=""

# `heal_stale_permission` lives in iterm-colors.sh (sourced above) so it can be
# unit-tested — this file ends in an infinite poll loop, so sourcing it to reach
# the function would hang. See there for why the heal exists and what each of
# its four non-healing guards protects.
#
# The heal runs on a much slower cadence than the paint poll: it shells out to
# `stat` and `date`, and at the poll interval that would be dozens of processes
# a second to answer a question that changes on a two-minute scale.
HEAL_EVERY=100
TICK=0

while true; do
  COLOR=$(cat "$STATE_FILE" 2>/dev/null)
  if [ -n "$COLOR" ] && [ "$COLOR" != "$LAST" ]; then
    # Re-read the palette on each change so edits to iterm-colors.sh apply live
    # to this long-lived monitor instead of being cached until restart.
    # shellcheck disable=SC1090,SC1091
    source "$SCRIPT_DIR/iterm-colors.sh"
    emit_set_colors "$COLOR" >"$TTY_PATH"
    LAST="$COLOR"
  fi
  # Always clear attention requests (Claude Code may set them mid-poll).
  printf '\033]1337;RequestAttention=no\a' >"$TTY_PATH" 2>/dev/null
  TICK=$((TICK + 1))
  if [ "$((TICK % HEAL_EVERY))" -eq 0 ]; then
    heal_stale_permission "$TTY_BASE"
  fi
  sleep 0.03
done
