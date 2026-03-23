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
echo "[1/5] Building guardiand..."
cd "$REPO_ROOT"
cargo build --release
GUARDIAND_BIN="$REPO_ROOT/target/release/guardiand"

if [ ! -f "$GUARDIAND_BIN" ]; then
    echo "ERROR: guardiand binary not found at $GUARDIAND_BIN"
    exit 1
fi

# Create guardian directory
echo "[2/5] Creating ~/.guardian/..."
mkdir -p "$GUARDIAN_DIR"

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
    echo ""
    echo "To check status:  launchctl list com.guardian.guardiand"
    echo "To view state:    cat ~/.guardian/state.json"
else
    echo ""
    echo "WARNING: daemon may not have started. Check logs:"
    echo "  cat $GUARDIAN_DIR/guardiand.stderr.log"
fi
