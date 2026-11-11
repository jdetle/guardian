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
GUARDIAND_BUILT="$REPO_ROOT/target/release/guardiand"
GUARDIAN_CLI_BUILT="$REPO_ROOT/target/release/guardian"

if [ ! -f "$GUARDIAND_BUILT" ]; then
    echo "ERROR: guardiand binary not found at $GUARDIAND_BUILT"
    exit 1
fi
if [ ! -f "$GUARDIAN_CLI_BUILT" ]; then
    echo "ERROR: guardian CLI not found at $GUARDIAN_CLI_BUILT"
    exit 1
fi

# Copy to a temp name then rename: never rewrite a binary in place (a running or
# previously-executed Mach-O that is overwritten can be killed on next launch).
install_binary() {
    local src="$1" dst="$2"
    cp "$src" "$dst.new"
    chmod +x "$dst.new"
    mv -f "$dst.new" "$dst"
}

# Create guardian directory
echo "[2/5] Creating ~/.guardian/..."
mkdir -p "$GUARDIAN_DIR"
install_binary "$GUARDIAN_CLI_BUILT" "$GUARDIAN_DIR/guardian"
echo "  Installed ~/.guardian/guardian (snooze, once, clear-snooze, zeno)"

# The LaunchAgent runs this copy, NOT target/release/guardiand: cleaning build artifacts
# (cargo clean, disk-space sweeps) used to delete the daemon out from under launchd.
GUARDIAND_BIN="$GUARDIAN_DIR/guardiand"
install_binary "$GUARDIAND_BUILT" "$GUARDIAND_BIN"
echo "  Installed ~/.guardian/guardiand (daemon; survives cargo clean)"

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

# macOS double-click helpers (same as `guardian snooze 15` / `guardian once`)
if [ -d "$REPO_ROOT/hooks/mac" ]; then
    shopt -s nullglob
    for f in "$REPO_ROOT/hooks/mac"/*.command; do
        b=$(basename "$f")
        cp "$f" "$GUARDIAN_DIR/"
        chmod +x "$GUARDIAN_DIR/$b"
    done
    shopt -u nullglob
    echo "  Click:   ~/.guardian/Guardian-*.command (Finder) for snooze / once"
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

# Tool-call gate (Cursor preToolUse, Claude Code / Codex PreToolUse). Advisory by default.
[tool_gate]
# Deny NEW subagent spawns at this pressure: "never" | "strained" | "critical".
# Subagent fan-out is the main way one agent turn multiplies memory use.
deny_subagents_on = "never"

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

echo "[4/5] Installing LaunchAgent..."

# Render and lint the plist BEFORE touching the running service, so a bad path can never
# leave the machine with the old daemon unloaded and no new one to load.
mkdir -p "$LAUNCH_AGENTS"
PLIST_TMP="$(mktemp "$LAUNCH_AGENTS/.$PLIST_NAME.XXXXXX")"
trap 'rm -f "$PLIST_TMP"' EXIT

# Values go into XML via sed: escape XML metacharacters first, then sed's replacement ones.
plist_value() {
    printf '%s' "$1" \
        | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' \
        | sed -e 's/[&|\\]/\\&/g'
}
sed \
    -e "s|__GUARDIAND_PATH__|$(plist_value "$GUARDIAND_BIN")|g" \
    -e "s|__HOME__|$(plist_value "$HOME")|g" \
    "$PLIST_TEMPLATE" > "$PLIST_TMP"
plutil -lint "$PLIST_TMP" >/dev/null

# launchd never rotates the daemon log; keep the tail of an oversized one and start fresh.
STDERR_LOG="$GUARDIAN_DIR/guardiand.stderr.log"
if [ -f "$STDERR_LOG" ] && [ ! -L "$STDERR_LOG" ]; then
    log_bytes=$(wc -c < "$STDERR_LOG" 2>/dev/null | tr -d ' ' || echo 0)
    if [ "${log_bytes:-0}" -gt 10485760 ] 2>/dev/null; then
        tail -n 500 "$STDERR_LOG" > "$STDERR_LOG.old" 2>/dev/null || true
        : > "$STDERR_LOG" 2>/dev/null || true
        echo "  Rotated oversized daemon log ($(( log_bytes / 1048576 )) MB) — last 500 lines kept in guardiand.stderr.log.old"
    fi
fi

guardiand_pid() {
    # "PID" = 1234; is present only while the job has a live process.
    launchctl list com.guardian.guardiand 2>/dev/null | awk '/"PID"/ { gsub(/[^0-9]/, ""); print; exit }'
}

state_mtime() {
    stat -f %m "$GUARDIAN_DIR/state.json" 2>/dev/null || echo 0
}

# Swap the service: unload the old job (if any), move the new plist into place, load it.
if launchctl list com.guardian.guardiand >/dev/null 2>&1; then
    launchctl unload "$LAUNCH_AGENTS/$PLIST_NAME" 2>/dev/null || true
fi
mv -f "$PLIST_TMP" "$LAUNCH_AGENTS/$PLIST_NAME"

echo "[5/5] Starting guardiand..."
# The old daemon writes one last state.json as it shuts down. Only samples written strictly
# after this point count as proof that the NEW daemon is sampling.
sleep 1
load_ts=$(date +%s)
launchctl load "$LAUNCH_AGENTS/$PLIST_NAME"

# Success = two successive samples newer than the load, written while the same PID stays up.
# ("launchctl list" alone also lists a job that is crash-looping.)
daemon_pid=""
first_pid=""
first_mtime=0
healthy=false
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do
    sleep 1
    daemon_pid=$(guardiand_pid)
    m=$(state_mtime)
    case "$daemon_pid" in
        ''|*[!0-9]*) first_pid=""; first_mtime=0; continue ;;
    esac
    if [ "$m" -le "$load_ts" ]; then
        continue
    fi
    if [ -z "$first_pid" ] || [ "$daemon_pid" != "$first_pid" ]; then
        first_pid="$daemon_pid"
        first_mtime="$m"
        continue
    fi
    if [ "$m" -gt "$first_mtime" ]; then
        healthy=true
        break
    fi
done

if [ "$healthy" = "true" ]; then
    echo ""
    echo "Guardian daemon installed and running (pid $daemon_pid)."
    echo "  Daemon:  $GUARDIAND_BIN"
    echo "  Config:  $GUARDIAN_DIR/config.toml"
    echo "  State:   $GUARDIAN_DIR/state.json"
    echo "  Logs:    $GUARDIAN_DIR/guardiand.stderr.log"
    echo "  CLI:     $GUARDIAN_DIR/guardian --help"
    echo ""
    echo "To check status:  launchctl list com.guardian.guardiand"
    echo "To view state:    cat ~/.guardian/state.json"
    echo ""
    echo "Hooks (run the ones you use):"
    echo "  Cursor:       bash hooks/install-hooks.sh"
    echo "  Claude Code:  bash scripts/install-claude-hooks.sh"
    echo "  Codex:        bash scripts/install-codex-hooks.sh"
else
    echo ""
    echo "ERROR: guardiand is not sampling — no fresh $GUARDIAN_DIR/state.json from a stable process (launchd pid: ${daemon_pid:-none})."
    echo "  Status:  launchctl list com.guardian.guardiand"
    echo "  Logs:    tail -n 40 $GUARDIAN_DIR/guardiand.stderr.log"
    exit 1
fi
