#!/bin/bash
# Install Guardian hooks to ~/.cursor/ for global application.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CURSOR_DIR="$HOME/.cursor"
HOOKS_DIR="$CURSOR_DIR/hooks/guardian"

echo "=== Guardian Hooks Installer ==="

# Copy hook scripts
echo "[1/3] Installing hook scripts to $HOOKS_DIR..."
mkdir -p "$HOOKS_DIR"
cp "$SCRIPT_DIR/lib.sh" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/session-start.sh" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/pre-tool-use.sh" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/subagent-start.sh" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/post-tool-use.sh" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/stop.sh" "$HOOKS_DIR/"
chmod +x "$HOOKS_DIR"/*.sh

# Merge hooks.json
echo "[2/3] Configuring hooks.json..."
HOOKS_JSON="$CURSOR_DIR/hooks.json"

if [ -f "$HOOKS_JSON" ]; then
    # Merge Guardian hooks into existing config
    # Back up the original
    cp "$HOOKS_JSON" "$HOOKS_JSON.bak"

    # Use jq to merge; if jq fails, replace entirely
    if command -v jq &>/dev/null; then
        jq -s '
            .[0] as $existing |
            .[1] as $guardian |
            $existing * {hooks: ($existing.hooks // {} | to_entries + ($guardian.hooks | to_entries) | from_entries)}
        ' "$HOOKS_JSON" "$SCRIPT_DIR/hooks.json" > "$HOOKS_JSON.tmp" && mv "$HOOKS_JSON.tmp" "$HOOKS_JSON"
        echo "  Merged into existing hooks.json (backup at hooks.json.bak)"
    else
        cp "$SCRIPT_DIR/hooks.json" "$HOOKS_JSON"
        echo "  WARNING: jq not found; replaced hooks.json (backup at hooks.json.bak)"
    fi
else
    # Install fresh
    cp "$SCRIPT_DIR/hooks.json" "$HOOKS_JSON"
    echo "  Created new hooks.json"
fi

# Update paths in hooks.json to use absolute paths for user-level hooks
echo "[3/3] Resolving hook paths..."
if command -v jq &>/dev/null; then
    jq --arg dir "$HOOKS_DIR" '
        .hooks |= with_entries(
            .value |= map(
                if .command | test("\\./hooks/") then
                    .command = ($dir + "/" + (.command | split("/") | last))
                else .
                end
            )
        )
    ' "$HOOKS_JSON" > "$HOOKS_JSON.tmp" && mv "$HOOKS_JSON.tmp" "$HOOKS_JSON"
fi

echo ""
echo "Guardian hooks installed."
echo "  Hooks: $HOOKS_DIR/"
echo "  Config: $HOOKS_JSON"
echo ""
echo "Cursor will auto-reload hooks.json. If not, restart Cursor."
