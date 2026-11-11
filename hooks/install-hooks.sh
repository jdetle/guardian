#!/bin/bash
# Install Guardian hooks to ~/.cursor/ for global application.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CURSOR_DIR="$HOME/.cursor"
GUARDIAN_DIR="${GUARDIAN_DIR:-$HOME/.guardian}"
HOOKS_DIR="$CURSOR_DIR/hooks/guardian"

echo "=== Guardian Hooks Installer ==="

# The hooks themselves need jq on every call; fail before copying anything.
if ! command -v jq &>/dev/null; then
    echo "ERROR: jq is required (hooks parse JSON with it). Install it — e.g. brew install jq — and re-run." >&2
    exit 1
fi

# Copy hook scripts
echo "[1/2] Installing hook scripts to $HOOKS_DIR..."
mkdir -p "$HOOKS_DIR"
cp "$SCRIPT_DIR/lib.sh" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/prompt-gate-common.sh" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/tool-gate-common.sh" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/session-start.sh" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/before-submit-prompt.sh" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/before-read-file.sh" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/pre-tool-use.sh" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/subagent-start.sh" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/post-tool-use.sh" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/stop.sh" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/cursorignore_check.py" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/iso_to_epoch.py" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/cursorignore-checklist.json" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/hook_policy.default.json" "$HOOKS_DIR/"
cp "$SCRIPT_DIR/resources.md" "$HOOKS_DIR/"
chmod +x "$HOOKS_DIR"/*.sh
chmod +x "$HOOKS_DIR/cursorignore_check.py" "$HOOKS_DIR/iso_to_epoch.py" 2>/dev/null || true
mkdir -p "$GUARDIAN_DIR"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
if [ -f "$REPO_ROOT/scripts/guardian-queue.sh" ]; then
    cp "$REPO_ROOT/scripts/guardian-queue.sh" "$GUARDIAN_DIR/"
    chmod +x "$GUARDIAN_DIR/guardian-queue.sh"
    echo "  Installed ~/.guardian/guardian-queue.sh (agent work queue CLI)"
fi
if [ -f "$REPO_ROOT/target/release/guardian" ]; then
    cp "$REPO_ROOT/target/release/guardian" "$GUARDIAN_DIR/guardian"
    chmod +x "$GUARDIAN_DIR/guardian"
    echo "  Installed ~/.guardian/guardian (snooze, once, zeno)"
fi

MAC_DIR="$SCRIPT_DIR/mac"
if [ -d "$MAC_DIR" ]; then
    shopt -s nullglob
    for f in "$MAC_DIR"/*.command; do
        b=$(basename "$f")
        cp "$f" "$GUARDIAN_DIR/$b"
        chmod +x "$GUARDIAN_DIR/$b"
    done
    shopt -u nullglob
    echo "  Installed ~/.guardian/Guardian-*.command (macOS: double-click snooze / once)"
fi

CURSOR_CMDS_SRC="$SCRIPT_DIR/cursor-commands"
CURSOR_CMDS_DST="$HOME/.cursor/commands"
if [ -d "$CURSOR_CMDS_SRC" ]; then
    mkdir -p "$CURSOR_CMDS_DST"
    shopt -s nullglob
    for f in "$CURSOR_CMDS_SRC"/*.md; do
        cp "$f" "$CURSOR_CMDS_DST/"
    done
    shopt -u nullglob
    echo "  Installed ~/.cursor/commands/guardian-*.md — in the chat composer, type /guardian-snooze or /guardian-once (Cursor slash commands)"
fi
if [ ! -f "$GUARDIAN_DIR/hook_policy.json" ] && [ -f "$SCRIPT_DIR/hook_policy.default.json" ]; then
    cp "$SCRIPT_DIR/hook_policy.default.json" "$GUARDIAN_DIR/hook_policy.json"
    echo "  Installed default ~/.guardian/hook_policy.json (edit to tune gates)"
fi

# Merge into ~/.cursor/hooks.json. Idempotent; other hooks on the same events are kept, and
# only Guardian's own template commands are rewritten to absolute paths.
echo "[2/2] Merging hooks.json..."
HOOKS_JSON="$CURSOR_DIR/hooks.json"
if ! bash "$SCRIPT_DIR/merge-agent-hooks.sh" "$HOOKS_JSON" "$SCRIPT_DIR/hooks.json" "./hooks" "$HOOKS_DIR" "$HOOKS_JSON.bak"; then
    echo "Guardian hook scripts were copied to $HOOKS_DIR but are NOT registered in $HOOKS_JSON." >&2
    echo "Fix the problem above and re-run this installer." >&2
    exit 1
fi

echo ""
echo "Guardian hooks installed."
echo "  Hooks: $HOOKS_DIR/"
echo "  Config: $HOOKS_JSON"
echo ""
echo "Cursor will auto-reload hooks.json. If not, restart Cursor."
