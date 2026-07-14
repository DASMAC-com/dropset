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
      # A permission_prompt is yellow. The harness ALSO fires this for an
      # AskUserQuestion selector (from its side it is blocked on user input),
      # so this branch fires there too; the sticky-green guard in the main body
      # suppresses that companion yellow so the tool's green survives. Other
      # notification types leave the tab unchanged.
      [ "$notification_type" = "permission_prompt" ] && printf '%s' "$STATE_PERMISSION"
      ;;
  esac
}

# --- AskUserQuestion "sticky green" ------------------------------------------
# The harness fires BOTH a PreToolUse(AskUserQuestion) — painted green (reply
# wanted) — AND a companion Notification(permission_prompt) for the same
# selector, and RE-fires that notification periodically while the selector
# waits for an answer. With no guard, each yellow overwrites the green by
# last-write and the tab misreads as "go approve". The permission_prompt
# payload carries no field that tells the AskUserQuestion companion apart from a
# genuine tool-permission prompt, so instead the AskUserQuestion green is made
# *sticky*: painting it drops a per-tty sentinel, and EVERY permission_prompt
# Notification is suppressed while that sentinel is present — until the selector
# is answered (its PostToolUse) or any other paint clears it. There is
# deliberately NO time window: a selector can wait indefinitely, so a fixed
# window would let a re-fired notification repaint yellow mid-wait (the bug this
# guard replaced). A stale sentinel from a crashed session is cleared by the
# next non-AskUserQuestion paint (below) and by iterm-start.sh at session start.
askq_sentinel_path() { printf '%s%s.askq' "$STATE_PREFIX" "$1"; } # $1 = tty base

# True while an unanswered AskUserQuestion green is in effect on this tty.
askq_sentinel_present() { [ -f "$(askq_sentinel_path "$1")" ]; } # $1 = tty base

COLOR="$1"
PAYLOAD=""
if [ -z "$COLOR" ]; then
  PAYLOAD="$(cat)"
  COLOR="$(color_for_event "$PAYLOAD")"
fi
[ -z "$COLOR" ] && exit 0

# Record the state (for the monitor and the attend toggle) and paint it now.
TTY_PATH=$(resolve_tty) || exit 0
TTY_BASE="$(basename "$TTY_PATH")"

EVENT="$(json_field hook_event_name "$PAYLOAD")"

# Suppress the AskUserQuestion companion permission_prompt yellow while the
# green sentinel says the selector is still awaiting a reply. This fires for
# every re-fired notification until the sentinel is cleared below.
if [ "$EVENT" = "Notification" ] && [ "$COLOR" = "$STATE_PERMISSION" ] &&
  [ "$(json_field notification_type "$PAYLOAD")" = "permission_prompt" ] &&
  askq_sentinel_present "$TTY_BASE"; then
  exit 0
fi

# Maintain the sentinel: set it when painting an AskUserQuestion green, clear it
# on any other paint so a later genuine permission prompt is not suppressed.
if [ "$EVENT" = "PreToolUse" ] && [ "$(json_field tool_name "$PAYLOAD")" = "AskUserQuestion" ]; then
  : >"$(askq_sentinel_path "$TTY_BASE")"
else
  rm -f "$(askq_sentinel_path "$TTY_BASE")"
fi

echo "$COLOR" >"$STATE_PREFIX$TTY_BASE"
emit_set_colors "$COLOR" >"$TTY_PATH"
