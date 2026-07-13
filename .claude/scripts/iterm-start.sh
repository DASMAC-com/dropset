#!/bin/bash
# Start the per-TTY monitor for this session if it is not already running, and
# set the initial neutral state. Called by the SessionStart hook.

# shellcheck source=.claude/scripts/iterm-colors.sh
# shellcheck disable=SC1090,SC1091
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/iterm-colors.sh"

TTY_PATH=$(resolve_tty) || exit 0
TTY_NAME=$(basename "$TTY_PATH")
PID_FILE="$MONITOR_PID_PREFIX$TTY_NAME.pid"

# Don't start a second monitor for this TTY.
if [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
  exit 0
fi

nohup "$SCRIPT_DIR/iterm-monitor.sh" "$TTY_PATH" &>/dev/null &
echo "$!" >"$PID_FILE"

# Seed the neutral state so the monitor paints something immediately.
echo "$STATE_NEUTRAL" >"$STATE_PREFIX$TTY_NAME"
