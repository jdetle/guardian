#!/bin/bash
# Install Guardian hooks for Claude Code (~/.claude/settings.json hooks).
# See https://code.claude.com/docs/en/hooks
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOKS_SRC="$REPO_ROOT/hooks"
DEST="${HOME}/.claude/hooks/guardian"
CLAUDE_SETTINGS="${HOME}/.claude/settings.json"
GUARDIAN_DIR="${GUARDIAN_DIR:-$HOME/.guardian}"

echo "=== Guardian Claude Code hooks installer ==="

mkdir -p "$DEST/codex" "$DEST/claude"

echo "[1/3] Copying hook scripts to $DEST..."
cp "$HOOKS_SRC/lib.sh" "$DEST/"
cp "$HOOKS_SRC/prompt-gate-common.sh" "$DEST/"
cp "$HOOKS_SRC/session-start.sh" "$DEST/"
cp "$HOOKS_SRC/cursorignore_check.py" "$DEST/"
cp "$HOOKS_SRC/iso_to_epoch.py" "$DEST/"
cp "$HOOKS_SRC/cursorignore-checklist.json" "$DEST/"
cp "$HOOKS_SRC/hook_policy.default.json" "$DEST/"
cp "$HOOKS_SRC/resources.md" "$DEST/"
cp "$HOOKS_SRC/codex/user-prompt-submit.sh" "$DEST/codex/"
cp "$HOOKS_SRC/codex/session-start.sh" "$DEST/codex/"
cp "$HOOKS_SRC/claude/pre-tool-use.sh" "$DEST/claude/"
chmod +x "$DEST"/*.sh "$DEST/codex"/*.sh "$DEST/claude"/*.sh 2>/dev/null || true
chmod +x "$DEST/cursorignore_check.py" "$DEST/iso_to_epoch.py" 2>/dev/null || true

mkdir -p "$GUARDIAN_DIR"
if [ -f "$REPO_ROOT/target/release/guardian" ]; then
    cp "$REPO_ROOT/target/release/guardian" "$GUARDIAN_DIR/guardian"
    chmod +x "$GUARDIAN_DIR/guardian"
    echo "  Updated ~/.guardian/guardian"
fi

TMP_MERGE="$(mktemp)"
sed "s|__GUARDIAN_CLAUDE_HOOK_ROOT__|$DEST|g" "$HOOKS_SRC/claude-code-hooks.json" > "$TMP_MERGE"
jq . "$TMP_MERGE" > "${TMP_MERGE}.ok"
mv "${TMP_MERGE}.ok" "$TMP_MERGE"

echo "[2/3] Merging hooks into $CLAUDE_SETTINGS..."
mkdir -p "$(dirname "$CLAUDE_SETTINGS")"

if [ -f "$CLAUDE_SETTINGS" ]; then
    cp "$CLAUDE_SETTINGS" "${CLAUDE_SETTINGS}.bak"
    if command -v jq &>/dev/null; then
        jq -s '
            .[0] as $base |
            .[1] as $g |
            $base
            | .hooks |= (. // {})
            | .hooks.UserPromptSubmit = ((.hooks.UserPromptSubmit // []) + ($g.hooks.UserPromptSubmit // []))
            | .hooks.SessionStart = ((.hooks.SessionStart // []) + ($g.hooks.SessionStart // []))
            | .hooks.PreToolUse = ((.hooks.PreToolUse // []) + ($g.hooks.PreToolUse // []))
        ' "$CLAUDE_SETTINGS" "$TMP_MERGE" > "${CLAUDE_SETTINGS}.tmp" && mv "${CLAUDE_SETTINGS}.tmp" "$CLAUDE_SETTINGS"
        echo "  Merged Guardian entries into existing settings.json (backup: settings.json.bak)"
    else
        echo "  WARNING: jq not found; cannot merge — install jq or merge $TMP_MERGE manually."
    fi
else
    cp "$TMP_MERGE" "$CLAUDE_SETTINGS"
    echo "  Created $CLAUDE_SETTINGS"
fi
rm -f "$TMP_MERGE"

echo "[3/3] Done."
echo ""
echo "Restart Claude Code so it reloads hooks."
echo "Installed: UserPromptSubmit + SessionStart + PreToolUse."
echo "Docs: https://code.claude.com/docs/en/hooks"
