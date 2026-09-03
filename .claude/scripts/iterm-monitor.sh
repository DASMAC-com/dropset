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

# Clear a permission yellow that has stopped being refreshed.
#
# The harness re-fires permission_prompt while a prompt waits, so the painter's
# sentinel mtime tracks "still waiting". Once the prompt is resolved — approved
# OR denied — the re-fires stop and the sentinel goes stale. That is the only
# signal available for the denial path, on which no painter event fires at all,
# so without this the tab stays yellow indefinitely over a session that needs
# nothing. Writing the state file rather than painting directly lets the normal
# change-detection above do the paint, so there is one painting path.
heal_stale_permission() {
  local sentinel mtime now
  sentinel="$(perm_wait_path "$TTY_BASE")"
  [ -f "$sentinel" ] || return 0
  [ "$(cat "$STATE_FILE" 2>/dev/null)" = "$STATE_PERMISSION" ] || return 0
  mtime=$(stat -f %m "$sentinel" 2>/dev/null) || return 0
  now=$(date +%s)
  if [ "$((now - mtime))" -ge "$PERM_WAIT_STALE_SECONDS" ]; then
    rm -f "$sentinel"
    echo "$STATE_NEUTRAL" >"$STATE_FILE"
  fi
}

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
    heal_stale_permission
  fi
  sleep 0.03
done
