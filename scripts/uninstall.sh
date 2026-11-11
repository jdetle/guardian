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

# Remove Guardian's hook entries from each agent's config, then its hook scripts. The entries
# go first: deleting only the scripts would leave every agent calling files that no longer
# exist. A config that cannot be edited safely is left alone, and so are its scripts.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MERGE_HELPER="$SCRIPT_DIR/../hooks/merge-agent-hooks.sh"

remove_agent_hooks() {
    local label="$1" config="$2" hook_dir="$3"
    if [ ! -e "$config" ] && [ ! -d "$hook_dir" ]; then
        return 0
    fi
    echo "Removing $label hooks..."
    if [ -e "$config" ]; then
        if ! bash "$MERGE_HELPER" --remove "$config" "$hook_dir" "$config.guardian-uninstall.bak"; then
            echo "  WARNING: left $config and $hook_dir in place — delete the entries containing /hooks/guardian/ by hand."
            return 0
        fi
    fi
    case "$hook_dir" in
        */hooks/guardian)
            if [ -d "$hook_dir" ] && [ ! -L "$hook_dir" ]; then
                rm -rf "$hook_dir"
                echo "  Removed $hook_dir"
            fi
            ;;
    esac
}

remove_agent_hooks "Cursor" "$HOME/.cursor/hooks.json" "$HOME/.cursor/hooks/guardian"
# The default location always, plus the override if one is set now (it may not have been set
# at install time, or the other way round).
remove_agent_hooks "Claude Code" "$HOME/.claude/settings.json" "$HOME/.claude/hooks/guardian"
if [ -n "${CLAUDE_CONFIG_DIR:-}" ] && [ "${CLAUDE_CONFIG_DIR%/}" != "$HOME/.claude" ]; then
    remove_agent_hooks "Claude Code ($CLAUDE_CONFIG_DIR)" "${CLAUDE_CONFIG_DIR%/}/settings.json" "${CLAUDE_CONFIG_DIR%/}/hooks/guardian"
fi
remove_agent_hooks "Codex" "$HOME/.codex/hooks.json" "$HOME/.codex/hooks/guardian"
if [ -n "${CODEX_HOME:-}" ] && [ "${CODEX_HOME%/}" != "$HOME/.codex" ]; then
    remove_agent_hooks "Codex ($CODEX_HOME)" "${CODEX_HOME%/}/hooks.json" "${CODEX_HOME%/}/hooks/guardian"
fi

# Cursor slash commands installed by hooks/install-hooks.sh
for cmd in guardian-snooze.md guardian-once.md; do
    if [ -f "$HOME/.cursor/commands/$cmd" ]; then
        rm -f "$HOME/.cursor/commands/$cmd" && echo "  Removed ~/.cursor/commands/$cmd"
    fi
done

echo ""
echo "Guardian daemon and agent hooks uninstalled."
echo "Data preserved at ~/.guardian/ (remove manually if desired)."
echo "  rm -rf ~/.guardian"
