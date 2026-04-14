#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PLIST_TEMPLATE="$REPO_ROOT/com.guardian.guardiand.plist"
PLIST_NAME="com.guardian.guardiand.plist"
LAUNCH_AGENTS="$HOME/Library/LaunchAgents"
GUARDIAN_DIR="$HOME/.guardian"

echo "=== Guardian Daemon Installer ==="

# Build the daemon in release mode
echo "[1/5] Building guardiand and guardian CLI..."
cd "$REPO_ROOT"
cargo build --release
GUARDIAND_BIN="$REPO_ROOT/target/release/guardiand"
GUARDIAN_CLI_BIN="$REPO_ROOT/target/release/guardian"

if [ ! -f "$GUARDIAND_BIN" ]; then
    echo "ERROR: guardiand binary not found at $GUARDIAND_BIN"
    exit 1
fi
if [ ! -f "$GUARDIAN_CLI_BIN" ]; then
    echo "ERROR: guardian CLI not found at $GUARDIAN_CLI_BIN"
    exit 1
fi

# Create guardian directory
echo "[2/5] Creating ~/.guardian/..."
mkdir -p "$GUARDIAN_DIR"
cp "$GUARDIAN_CLI_BIN" "$GUARDIAN_DIR/guardian"
chmod +x "$GUARDIAN_DIR/guardian"
echo "  Installed ~/.guardian/guardian (snooze, once, clear-snooze, zeno)"

# Symlink onto PATH so `guardian` works outside full paths (prefer /usr/local/bin, else ~/.local/bin).
if ln -sf "$GUARDIAN_DIR/guardian" /usr/local/bin/guardian 2>/dev/null; then
    echo "  PATH:    /usr/local/bin/guardian -> ~/.guardian/guardian"
elif mkdir -p "$HOME/.local/bin" && ln -sf "$GUARDIAN_DIR/guardian" "$HOME/.local/bin/guardian"; then
    echo "  PATH:    ~/.local/bin/guardian -> ~/.guardian/guardian"
    case ":${PATH:-}:" in
        *":$HOME/.local/bin:"*) ;;
        *)
            echo "           Add to shell profile if \`guardian\` is not found: export PATH=\"\$HOME/.local/bin:\$PATH\""
            ;;
    esac
else
    echo "  PATH:    could not create symlink — run: sudo ln -sf $GUARDIAN_DIR/guardian /usr/local/bin/guardian"
fi

# Write default config if none exists
if [ ! -f "$GUARDIAN_DIR/config.toml" ]; then
    echo "[3/5] Writing default config..."
    cat > "$GUARDIAN_DIR/config.toml" << 'TOML'
# Guardian Daemon Configuration
# Adjust thresholds to match your system's capabilities.

sample_interval_secs = 2

[thresholds]
strained_cpu_percent = 70.0
critical_cpu_percent = 90.0
strained_memory_gb = 2.0
critical_memory_gb = 1.0
strained_swap_percent = 25.0
critical_swap_percent = 50.0

[docker]
enabled = true
auto_throttle = true
# Containers that should never be paused or throttled
essential_containers = ["postgres"]

[fork_guard]
enabled = true
warn_ratio = 0.6
kill_ratio = 0.8
# Set to true to allow guardian to kill runaway processes
kill_enabled = false

# Prompt / session gates (also written to ~/.guardian/hook_policy.json by guardiand)
[prompt_gate]
enabled = true
block_on = "critical"
# When true, block if Cursor RSS exceeds [session_budget] max while pressure is strained/critical
block_on_session_budget = false

[session_budget]
# Aggregate Cursor RSS (MB) from guardiand — not ~/.cursor/projects folder count
max_cursor_rss_megabytes = 8192
warn_cursor_rss_megabytes = 4096

[disk]
enabled = true
warn_used_percent = 85.0
critical_used_percent = 93.0

[queue]
# When true, blocked beforeSubmitPrompt may save prompt text to ~/.guardian/agent_queue.jsonl (if Cursor sends it)
enqueue_on_blocked_submit = false

[cursorignore_policy]
warn_once_per_path = true
before_read_enabled = true
TOML
else
    echo "[3/5] Config already exists, skipping..."
fi

# Unload existing service if running
echo "[4/5] Installing LaunchAgent..."
if launchctl list | grep -q com.guardian.guardiand 2>/dev/null; then
    launchctl unload "$LAUNCH_AGENTS/$PLIST_NAME" 2>/dev/null || true
fi

mkdir -p "$LAUNCH_AGENTS"

# Generate plist with actual paths
sed \
    -e "s|__GUARDIAND_PATH__|$GUARDIAND_BIN|g" \
    -e "s|__HOME__|$HOME|g" \
    "$PLIST_TEMPLATE" > "$LAUNCH_AGENTS/$PLIST_NAME"

# Load the service
echo "[5/5] Starting guardiand..."
launchctl load "$LAUNCH_AGENTS/$PLIST_NAME"

sleep 1

if launchctl list | grep -q com.guardian.guardiand; then
    echo ""
    echo "Guardian daemon installed and running."
    echo "  Config:  $GUARDIAN_DIR/config.toml"
    echo "  State:   $GUARDIAN_DIR/state.json"
    echo "  Logs:    $GUARDIAN_DIR/guardiand.stderr.log"
    echo "  CLI:     $GUARDIAN_DIR/guardian --help"
    echo ""
    echo "To check status:  launchctl list com.guardian.guardiand"
    echo "To view state:    cat ~/.guardian/state.json"
else
    echo ""
    echo "WARNING: daemon may not have started. Check logs:"
    echo "  cat $GUARDIAN_DIR/guardiand.stderr.log"
fi
