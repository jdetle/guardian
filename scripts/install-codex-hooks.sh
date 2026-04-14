#!/bin/bash
# Install Guardian hooks for OpenAI Codex CLI (~/.codex/hooks.json).
# Requires: [features] codex_hooks = true in Codex config.toml (see https://developers.openai.com/codex/hooks/)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOKS_SRC="$REPO_ROOT/hooks"
DEST="${HOME}/.codex/hooks/guardian"
CODEX_HOOKS_JSON="${HOME}/.codex/hooks.json"
GUARDIAN_DIR="${GUARDIAN_DIR:-$HOME/.guardian}"

echo "=== Guardian Codex hooks installer ==="

mkdir -p "$DEST/codex"

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
chmod +x "$DEST"/*.sh "$DEST/codex"/*.sh 2>/dev/null || true
chmod +x "$DEST/cursorignore_check.py" "$DEST/iso_to_epoch.py" 2>/dev/null || true

mkdir -p "$GUARDIAN_DIR"
if [ -f "$REPO_ROOT/target/release/guardian" ]; then
    cp "$REPO_ROOT/target/release/guardian" "$GUARDIAN_DIR/guardian"
    chmod +x "$GUARDIAN_DIR/guardian"
    echo "  Updated ~/.guardian/guardian"
fi

TMP_MERGE="$(mktemp)"
sed "s|__GUARDIAN_CODEX_HOOK_ROOT__|$DEST|g" "$HOOKS_SRC/codex-hooks.json" > "$TMP_MERGE"
jq . "$TMP_MERGE" > "${TMP_MERGE}.ok"
mv "${TMP_MERGE}.ok" "$TMP_MERGE"

echo "[2/3] Merging $CODEX_HOOKS_JSON..."
mkdir -p "$(dirname "$CODEX_HOOKS_JSON")"

if [ -f "$CODEX_HOOKS_JSON" ]; then
    cp "$CODEX_HOOKS_JSON" "$CODEX_HOOKS_JSON.bak"
    if command -v jq &>/dev/null; then
        jq -s '
            .[0] as $base |
            .[1] as $g |
            $base
            | .hooks |= (. // {})
            | .hooks.UserPromptSubmit = ((.hooks.UserPromptSubmit // []) + ($g.hooks.UserPromptSubmit // []))
            | .hooks.SessionStart = ((.hooks.SessionStart // []) + ($g.hooks.SessionStart // []))
        ' "$CODEX_HOOKS_JSON" "$TMP_MERGE" > "$CODEX_HOOKS_JSON.tmp" && mv "$CODEX_HOOKS_JSON.tmp" "$CODEX_HOOKS_JSON"
        echo "  Merged Guardian entries into existing hooks.json (backup: hooks.json.bak)"
    else
        echo "  WARNING: jq not found; cannot merge — install jq or merge $TMP_MERGE manually."
    fi
else
    cp "$TMP_MERGE" "$CODEX_HOOKS_JSON"
    echo "  Created $CODEX_HOOKS_JSON"
fi
rm -f "$TMP_MERGE"

echo "[3/3] Done."
echo ""
echo "Enable Codex hooks in your Codex config (usually ~/.codex/config.toml):"
echo "  [features]"
echo "  codex_hooks = true"
echo ""
echo "Restart the Codex CLI. Installed: UserPromptSubmit + SessionStart."
echo "Docs: https://developers.openai.com/codex/hooks/"
