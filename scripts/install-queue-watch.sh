#!/bin/bash
# Optional: LaunchAgent to notify when pressure returns to clear and agent_queue.jsonl is non-empty.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
LAUNCH_AGENTS="$HOME/Library/LaunchAgents"
PLIST_NAME="com.guardian.queue-watch.plist"
PLIST_TEMPLATE="$REPO_ROOT/com.guardian.queue-watch.plist"
WATCH_SCRIPT="$REPO_ROOT/scripts/guardian-queue-watch.sh"

if [ ! -f "$WATCH_SCRIPT" ]; then
    echo "ERROR: missing $WATCH_SCRIPT"
    exit 1
fi
chmod +x "$WATCH_SCRIPT"

if launchctl list | grep -q com.guardian.queue-watch 2>/dev/null; then
    launchctl unload "$LAUNCH_AGENTS/$PLIST_NAME" 2>/dev/null || true
fi

mkdir -p "$LAUNCH_AGENTS"
sed \
    -e "s|__QUEUE_WATCH_SCRIPT__|$WATCH_SCRIPT|g" \
    -e "s|__HOME__|$HOME|g" \
    "$PLIST_TEMPLATE" > "$LAUNCH_AGENTS/$PLIST_NAME"

launchctl load "$LAUNCH_AGENTS/$PLIST_NAME"

echo "Installed $PLIST_NAME (polls every 30s). Logs: ~/.guardian/queue-watch.stderr.log"
