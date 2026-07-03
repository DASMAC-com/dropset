#!/bin/bash
# The hook painter. This is the single entry point every Claude Code hook calls.
# It derives the desired state color from the hook event on stdin, writes it to
# this session's state file, and applies it to the TTY immediately.
#
# Why one script (not one hook per color): matching PreToolUse hooks run in
# parallel with no ordering guarantee, so wiring `*`->neutral alongside
# `AskUserQuestion`->green raced and the tab color was non-deterministic. Here a
# single hook per event calls this painter, which picks the color itself, so the
# color is a deterministic function of the event.
#
# Usage:
#   iterm-paint.sh              # hook mode: read the hook JSON on stdin
#   iterm-paint.sh <state-hex>  # direct mode: paint this state now (attend etc.)

# shellcheck source=.claude/scripts/iterm-colors.sh
# shellcheck disable=SC1090,SC1091
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/iterm-colors.sh"

# Pull a JSON string field out of the hook payload without a jq dependency.
# Hook payloads are single-object JSON with flat string fields (tool_name,
# hook_event_name, notification_type), so a targeted regex is enough.
json_field() { # $1 = field name, $2 = payload
  local re="\"$1\"[[:space:]]*:[[:space:]]*\"([^\"]*)\""
  if [[ "$2" =~ $re ]]; then
    printf '%s' "${BASH_REMATCH[1]}"
  fi
}

# Map a hook event to a state color. Prints nothing for events that should not
# repaint (e.g. a non-permission Notification), so the caller leaves the tab as
# it is.
color_for_event() { # $1 = payload
  local event tool notification_type
  event="$(json_field hook_event_name "$1")"
  case "$event" in
    PreToolUse)
      tool="$(json_field tool_name "$1")"
      case "$tool" in
        AskUserQuestion) printf '%s' "$STATE_REPLY" ;;
        Edit | Write | MultiEdit | NotebookEdit) printf '%s' "$STATE_PERMISSION" ;;
        *) printf '%s' "$STATE_NEUTRAL" ;;
      esac
      ;;
    PostToolUse | UserPromptSubmit) printf '%s' "$STATE_NEUTRAL" ;;
    Stop) printf '%s' "$STATE_REPLY" ;;
    Notification)
      notification_type="$(json_field notification_type "$1")"
      # Only a genuine permission prompt is yellow. AskUserQuestion does not
      # fire a Notification; other notification types leave the tab unchanged.
      [ "$notification_type" = "permission_prompt" ] && printf '%s' "$STATE_PERMISSION"
      ;;
  esac
}

COLOR="$1"
if [ -z "$COLOR" ]; then
  COLOR="$(color_for_event "$(cat)")"
fi
[ -z "$COLOR" ] && exit 0

# Walk up the process tree to this session's controlling TTY, then record the
# state (for the monitor and the attend toggle) and paint it now.
pid=$PPID
while [ "$pid" -gt 1 ] 2>/dev/null; do
  t=$(ps -o tty= -p "$pid" 2>/dev/null | tr -d ' ')
  if [ -n "$t" ] && [ "$t" != "??" ] && [ -c "/dev/$t" ]; then
    echo "$COLOR" >"$STATE_PREFIX$t"
    emit_set_colors "$COLOR" >"/dev/$t"
    exit 0
  fi
  pid=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ')
done
