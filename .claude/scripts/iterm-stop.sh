#!/bin/bash
# Stop this session's monitor and reset colors. Called by the SessionEnd hook.

# shellcheck source=.claude/scripts/iterm-colors.sh
# shellcheck disable=SC1090,SC1091
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/iterm-colors.sh"

# Find the controlling TTY from the process tree.
TTY_PATH=$(resolve_tty) || exit 0
TTY_NAME=$(basename "$TTY_PATH")

PID_FILE="$MONITOR_PID_PREFIX$TTY_NAME.pid"

if [ -f "$PID_FILE" ]; then
  kill "$(cat "$PID_FILE")" 2>/dev/null
  rm -f "$PID_FILE"
fi

# Clear our tab tint (and window background if it was enabled).
emit_reset >"/dev/$TTY_NAME"

rm -f "$STATE_PREFIX$TTY_NAME"
