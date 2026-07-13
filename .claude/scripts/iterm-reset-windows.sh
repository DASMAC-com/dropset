#!/bin/bash
# Recovery: clear our coloring back to the iTerm profile default on every TTY we
# have a state file for. Use when a tab or window color is left stuck (e.g. a
# session that exited without its SessionEnd hook firing).

# shellcheck source=.claude/scripts/iterm-colors.sh
# shellcheck disable=SC1090,SC1091
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/iterm-colors.sh"

reset=0
for state_file in "$STATE_PREFIX"*; do
  [ -e "$state_file" ] || continue
  tty_name="${state_file#"$STATE_PREFIX"}"
  if [ -c "/dev/$tty_name" ]; then
    emit_reset >"/dev/$tty_name"
    reset=$((reset + 1))
  fi
  rm -f "$state_file"
done

echo "iterm-reset-windows: cleared coloring on $reset TTY(s)."
