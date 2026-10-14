#!/bin/bash
# Recovery: kill every running per-TTY monitor and drop its pid file. Use when a
# monitor has drifted (it now re-reads the palette on each state change, so this
# is rarely needed), and to put live sessions on new monitor code after a
# deploy. Each session's next hook starts a fresh monitor (`ensure_monitor` in
# iterm-colors.sh) and repaints.

# shellcheck source=.claude/scripts/iterm-colors.sh
# shellcheck disable=SC1090,SC1091
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/iterm-colors.sh"

killed=0
for pid_file in "$MONITOR_PID_PREFIX"*.pid; do
  [ -e "$pid_file" ] || continue
  pid=$(cat "$pid_file" 2>/dev/null)
  if [ -n "$pid" ]; then
    kill "$pid" 2>/dev/null && killed=$((killed + 1))
  fi
  rm -f "$pid_file"
done

echo "iterm-restart-monitors: stopped $killed monitor(s); each restarts on its session's next hook."
