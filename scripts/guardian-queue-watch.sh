#!/bin/bash
# Poll ~/.guardian/state.json; when pressure transitions to clear from strained/critical
# and the agent queue is non-empty, post a user notification (macOS).
# Intended for LaunchAgent (StartInterval). See com.guardian.queue-watch.plist.
set -euo pipefail

GUARDIAN_DIR="${GUARDIAN_DIR:-$HOME/.guardian}"
STATE_FILE="${GUARDIAN_DIR}/state.json"
QUEUE_FILE="${GUARDIAN_DIR}/agent_queue.jsonl"
LAST_FILE="${GUARDIAN_DIR}/.agent_queue_watch_prev_pressure"
STAMP_FILE="${GUARDIAN_DIR}/.agent_queue_last_notify_at"

[ -f "$STATE_FILE" ] || exit 0

cur=$(jq -r '.pressure // "clear"' "$STATE_FILE" 2>/dev/null || echo "clear")

if [ ! -f "$LAST_FILE" ]; then
    echo "$cur" >"$LAST_FILE"
    exit 0
fi

prev=$(tr -d '\r\n' <"$LAST_FILE")
echo "$cur" >"$LAST_FILE"

if [ "$cur" != "clear" ]; then
    exit 0
fi
if [ "$prev" = "clear" ] || [ -z "$prev" ]; then
    exit 0
fi

# Transition into clear from a loaded state
count=0
if [ -f "$QUEUE_FILE" ]; then
    count=$(grep -c '^{' "$QUEUE_FILE" 2>/dev/null || echo 0)
fi
if [ "${count:-0}" -eq 0 ] 2>/dev/null; then
    exit 0
fi

# Throttle: at most one notification per 5 minutes
now=$(date +%s)
if [ -f "$STAMP_FILE" ]; then
    last=$(tr -d '\r\n' <"$STAMP_FILE" || echo 0)
    if [ $((now - last)) -lt 300 ] 2>/dev/null; then
        exit 0
    fi
fi
echo "$now" >"$STAMP_FILE"

if command -v osascript &>/dev/null; then
    osascript -e "display notification \"Guardian: pressure clear; ${count} task(s) in agent queue. Run guardian-queue peek or list.\" with title \"Guardian\""
fi
