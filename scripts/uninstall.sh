#!/bin/bash
set -euo pipefail

PLIST_NAME="com.guardian.guardiand.plist"
LAUNCH_AGENTS="$HOME/Library/LaunchAgents"

echo "=== Guardian Daemon Uninstaller ==="

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

echo ""
echo "Guardian daemon uninstalled."
echo "Data preserved at ~/.guardian/ (remove manually if desired)."
echo "  rm -rf ~/.guardian"
