#!/bin/bash
# Install Guardian hooks for Claude Code (CLI, desktop app, IDE extensions).
# Merges into the user-level settings.json; safe to re-run (idempotent, keeps other hooks).
# Docs: https://code.claude.com/docs/en/hooks · docs/claude-code.md
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOKS_SRC="$REPO_ROOT/hooks"
CLAUDE_DIR="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
DEST="$CLAUDE_DIR/hooks/guardian"
SETTINGS_JSON="$CLAUDE_DIR/settings.json"
GUARDIAN_DIR="${GUARDIAN_DIR:-$HOME/.guardian}"

echo "=== Guardian Claude Code hooks installer ==="

if ! command -v jq &>/dev/null; then
    echo "ERROR: jq is required to merge $SETTINGS_JSON safely." >&2
    exit 1
fi

echo "[1/3] Copying hook scripts to $DEST..."
mkdir -p "$DEST/claude" "$DEST/codex"
cp "$HOOKS_SRC/lib.sh" "$DEST/"
cp "$HOOKS_SRC/prompt-gate-common.sh" "$DEST/"
cp "$HOOKS_SRC/tool-gate-common.sh" "$DEST/"
cp "$HOOKS_SRC/session-start.sh" "$DEST/"
cp "$HOOKS_SRC/cursorignore_check.py" "$DEST/"
cp "$HOOKS_SRC/iso_to_epoch.py" "$DEST/"
cp "$HOOKS_SRC/cursorignore-checklist.json" "$DEST/"
cp "$HOOKS_SRC/hook_policy.default.json" "$DEST/"
cp "$HOOKS_SRC/resources.md" "$DEST/"
# Claude Code and Codex share one hook protocol: claude/*.sh are thin wrappers over codex/*.sh.
cp "$HOOKS_SRC/codex/"*.sh "$DEST/codex/"
cp "$HOOKS_SRC/claude/"*.sh "$DEST/claude/"
chmod +x "$DEST"/*.sh "$DEST/codex"/*.sh "$DEST/claude"/*.sh
chmod +x "$DEST/cursorignore_check.py" "$DEST/iso_to_epoch.py" 2>/dev/null || true

mkdir -p "$GUARDIAN_DIR"
if [ ! -f "$GUARDIAN_DIR/hook_policy.json" ]; then
    cp "$HOOKS_SRC/hook_policy.default.json" "$GUARDIAN_DIR/hook_policy.json"
    echo "  Installed default ~/.guardian/hook_policy.json"
fi

echo "[2/3] Merging $SETTINGS_JSON..."
bash "$HOOKS_SRC/merge-agent-hooks.sh" "$SETTINGS_JSON" "$HOOKS_SRC/claude-code-hooks.json" "__GUARDIAN_CLAUDE_HOOK_ROOT__" "$DEST" "$SETTINGS_JSON.guardian.bak"

echo "[3/3] Done."
echo ""
echo "Installed: SessionStart, UserPromptSubmit (prompt gate), PreToolUse (Bash|Agent|Task advisory)."
echo "New Claude Code sessions pick the hooks up automatically; running sessions keep the hooks"
echo "they started with — start a new session (or open /hooks) to load them."
echo "Blocked prompt? Run: $GUARDIAN_DIR/guardian snooze 15   (or: $GUARDIAN_DIR/guardian once)"
