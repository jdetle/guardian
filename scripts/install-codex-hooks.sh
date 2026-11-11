#!/bin/bash
# Install Guardian hooks for OpenAI Codex (CLI and desktop app) into ~/.codex/hooks.json.
# Safe to re-run (idempotent, keeps other hooks). Docs: docs/codex.md
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOKS_SRC="$REPO_ROOT/hooks"
CODEX_DIR="${CODEX_HOME:-$HOME/.codex}"
DEST="$CODEX_DIR/hooks/guardian"
CODEX_HOOKS_JSON="$CODEX_DIR/hooks.json"
GUARDIAN_DIR="${GUARDIAN_DIR:-$HOME/.guardian}"

echo "=== Guardian Codex hooks installer ==="

if ! command -v jq &>/dev/null; then
    echo "ERROR: jq is required to merge $CODEX_HOOKS_JSON safely." >&2
    exit 1
fi

mkdir -p "$DEST/codex"

echo "[1/3] Copying hook scripts to $DEST..."
cp "$HOOKS_SRC/lib.sh" "$DEST/"
cp "$HOOKS_SRC/prompt-gate-common.sh" "$DEST/"
cp "$HOOKS_SRC/tool-gate-common.sh" "$DEST/"
cp "$HOOKS_SRC/session-start.sh" "$DEST/"
cp "$HOOKS_SRC/cursorignore_check.py" "$DEST/"
cp "$HOOKS_SRC/iso_to_epoch.py" "$DEST/"
cp "$HOOKS_SRC/cursorignore-checklist.json" "$DEST/"
cp "$HOOKS_SRC/hook_policy.default.json" "$DEST/"
cp "$HOOKS_SRC/resources.md" "$DEST/"
cp "$HOOKS_SRC/codex/"*.sh "$DEST/codex/"
chmod +x "$DEST"/*.sh "$DEST/codex"/*.sh
chmod +x "$DEST/cursorignore_check.py" "$DEST/iso_to_epoch.py" 2>/dev/null || true

mkdir -p "$GUARDIAN_DIR"
if [ ! -f "$GUARDIAN_DIR/hook_policy.json" ]; then
    cp "$HOOKS_SRC/hook_policy.default.json" "$GUARDIAN_DIR/hook_policy.json"
    echo "  Installed default ~/.guardian/hook_policy.json"
fi

echo "[2/3] Merging $CODEX_HOOKS_JSON..."
bash "$HOOKS_SRC/merge-agent-hooks.sh" "$CODEX_HOOKS_JSON" "$HOOKS_SRC/codex-hooks.json" "__GUARDIAN_CODEX_HOOK_ROOT__" "$DEST" "$CODEX_HOOKS_JSON.bak"

echo "[3/3] Done."
echo ""
echo "Installed: SessionStart, UserPromptSubmit (prompt gate), PreToolUse (Bash|Agent|spawn_agent advisory)."
echo ""
echo "ONE MANUAL STEP — Codex skips new or changed hooks until you trust them:"
echo "  CLI:          run /hooks and approve the Guardian entries"
echo "  Desktop app:  Settings > Hooks (or 'Review hooks' in the composer)"
echo "Hooks are on by default in current Codex; if you disabled them, set"
echo "  [features] hooks = true   in $CODEX_DIR/config.toml  (codex_hooks is a deprecated alias)."
echo "Blocked prompt? Run: $GUARDIAN_DIR/guardian snooze 15   (or: $GUARDIAN_DIR/guardian once)"
