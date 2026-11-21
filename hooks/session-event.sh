#!/bin/bash
# Guardian session recorder: writes one hook event to ~/.guardian/sessions.db so the menu bar
# app can follow agent sessions (which are working, which are idle, what they have done).
#
#   session-event.sh prompt   a prompt was sent          (state: working)
#   session-event.sh tool     a tool call finished       (tool_calls row, pressure sample)
#   session-event.sh end      the session ended          (state: ended)
#
# A turn ending is recorded by stop.sh, which also scores the session.
# GUARDIAN_HOOK_FORMAT (cursor | claude | codex) says which agent is calling.
# stdin: the hook's JSON.
#
# It only records. It prints nothing, never blocks, and exits 0 whatever happens: in Cursor,
# Claude Code and Codex alike, no output and exit 0 means "nothing to say". It costs one jq
# and one sqlite3, because it runs after every tool call on machines that are short of room.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh" 2>/dev/null || exit 0

event="${1:-}"
case "$event" in
    prompt|tool|end) ;;
    *) exit 0 ;;
esac
client=$(guardian_hook_client)

input=$(cat 2>/dev/null || true)
[ -n "$input" ] || exit 0
command -v jq >/dev/null 2>&1 && command -v sqlite3 >/dev/null 2>&1 || exit 0

ensure_db

state_text=""
if [ "$event" = "tool" ] && [ -f "$STATE_FILE" ] && [ ! -L "$STATE_FILE" ]; then
    state_text=$(cat "$STATE_FILE" 2>/dev/null || true)
fi

sql=$(printf '%s' "$input" | jq -r \
    --arg event "$event" --arg client "$client" --arg state_text "$state_text" \
    -f "$SCRIPT_DIR/session-event.jq" 2>/dev/null) || exit 0
[ -n "$sql" ] || exit 0

sqlite3 "$SESSIONS_DB" ".timeout 5000" "$sql" >/dev/null 2>&1 || true
exit 0
