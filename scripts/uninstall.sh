#!/bin/bash
set -euo pipefail

PLIST_NAME="com.guardian.guardiand.plist"
QUEUE_PLIST="com.guardian.queue-watch.plist"
LAUNCH_AGENTS="$HOME/Library/LaunchAgents"

echo "=== Guardian Daemon Uninstaller ==="

if launchctl list | grep -q com.guardian.queue-watch 2>/dev/null; then
    echo "Stopping queue-watch..."
    launchctl unload "$LAUNCH_AGENTS/$QUEUE_PLIST" 2>/dev/null || true
fi
if [ -f "$LAUNCH_AGENTS/$QUEUE_PLIST" ]; then
    rm -f "$LAUNCH_AGENTS/$QUEUE_PLIST"
fi

# Stop and unload the service
if launchctl list | grep -q com.guardian.guardiand 2>/dev/null; then
    echo "Stopping guardiand..."
    launchctl unload "$LAUNCH_AGENTS/$PLIST_NAME" 2>/dev/null || true
else
    echo "guardiand is not running."
fi

# Remove the plist
if [ -f "$LAUNCH_AGENTS/$PLIST_NAME" ]; then
    echo "Removing LaunchAgent plist..."
    rm "$LAUNCH_AGENTS/$PLIST_NAME"
fi

# Remove PATH symlinks created by install.sh (best-effort).
GUARDIAN_CLI="$HOME/.guardian/guardian"
for candidate in /usr/local/bin/guardian "$HOME/.local/bin/guardian"; do
    if [ -L "$candidate" ] && [ "$(readlink "$candidate")" = "$GUARDIAN_CLI" ]; then
        rm -f "$candidate" && echo "Removed PATH symlink $candidate"
    fi
done

echo ""
echo "Guardian daemon uninstalled."
echo "Data preserved at ~/.guardian/ (remove manually if desired)."
echo "  rm -rf ~/.guardian"
