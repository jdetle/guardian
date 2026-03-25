#!/bin/bash
# Shared utilities for Guardian hooks.
# Sourced by all hook scripts -- not executed directly.
#
# DESIGN: Hooks NEVER deny. They provide informational context about
# system load so agents can make their own decisions. The daemon handles
# enforcement (killing fork bombs, throttling Docker).

GUARDIAN_DIR="$HOME/.guardian"
STATE_FILE="$GUARDIAN_DIR/state.json"
SESSIONS_DB="$GUARDIAN_DIR/sessions.db"
STALE_THRESHOLD_SECS=30

parse_iso_epoch() {
    local ts="${1%%.*}"
    if date -jf "%Y-%m-%dT%H:%M:%S" "$ts" +%s 2>/dev/null; then
        return
    fi
    date -d "${ts}" +%s 2>/dev/null || echo 0
}

sanitize_sql() {
    # Escape single quotes by doubling them (SQL standard escaping).
    # This prevents SQL injection when interpolating into string literals.
    printf '%s' "$1" | sed "s/'/''/g"
}

is_daemon_active() {
    if [ ! -f "$STATE_FILE" ] || [ -L "$STATE_FILE" ]; then
        return 1
    fi

    local sampled_at
    sampled_at=$(jq -r '.sampled_at // ""' < "$STATE_FILE" 2>/dev/null)
    if [ -z "$sampled_at" ]; then
        return 1
    fi

    local state_epoch now_epoch age
    state_epoch=$(parse_iso_epoch "$sampled_at")
    now_epoch=$(date -u +%s)
    age=$(( now_epoch - state_epoch ))
    if [ "$age" -gt "$STALE_THRESHOLD_SECS" ]; then
        return 1
    fi

    return 0
}

read_state_pressure() {
    # Fail-open: default to clear if state is unavailable.
    # The daemon enforces safety; hooks only inform.
    if [ ! -f "$STATE_FILE" ]; then
        echo "clear"
        return
    fi

    if [ -L "$STATE_FILE" ]; then
        echo "clear"
        return
    fi

    # Staleness check: if state.json is older than STALE_THRESHOLD_SECS,
    # the daemon may have crashed. Fail-open to "clear".
    local sampled_at
    sampled_at=$(jq -r '.sampled_at // ""' < "$STATE_FILE" 2>/dev/null)
    if [ -n "$sampled_at" ]; then
        local state_epoch now_epoch age
        state_epoch=$(parse_iso_epoch "$sampled_at")
        now_epoch=$(date -u +%s)
        age=$(( now_epoch - state_epoch ))
        if [ "$age" -gt "$STALE_THRESHOLD_SECS" ]; then
            echo "clear"
            return
        fi
    fi

    local pressure
    pressure=$(jq -r '.pressure // ""' < "$STATE_FILE" 2>/dev/null)

    case "$pressure" in
        clear|strained|critical)
            echo "$pressure"
            ;;
        *)
            echo "clear"
            ;;
    esac
}

read_state_field() {
    local field="$1"
    local default="${2:-}"
    if [ ! -f "$STATE_FILE" ] || [ -L "$STATE_FILE" ]; then
        echo "$default"
        return
    fi
    local value
    value=$(jq -r ".$field // \"$default\"" < "$STATE_FILE" 2>/dev/null)
    echo "${value:-$default}"
}

read_full_state() {
    if [ ! -f "$STATE_FILE" ] || [ -L "$STATE_FILE" ]; then
        echo "{}"
        return
    fi
    cat "$STATE_FILE" 2>/dev/null || echo "{}"
}

db_exec() {
    local sql="$1"
    sqlite3 "$SESSIONS_DB" ".timeout 5000" "$sql" 2>/dev/null || true
}

ensure_db() {
    if [ ! -f "$SESSIONS_DB" ]; then
        mkdir -p "$GUARDIAN_DIR"
        sqlite3 "$SESSIONS_DB" > /dev/null 2>&1 << 'SQL'
PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS sessions (
    conversation_id TEXT PRIMARY KEY,
    model TEXT NOT NULL,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    ended_at TEXT,
    status TEXT,
    loop_count INTEGER DEFAULT 0,
    tool_call_count INTEGER DEFAULT 0,
    pressure_at_start TEXT,
    pressure_avg TEXT,
    duration_ms INTEGER,
    files_modified INTEGER DEFAULT 0,
    lines_changed INTEGER DEFAULT 0,
    estimated_cost_usd REAL DEFAULT 0.0,
    roi_score REAL
);
CREATE TABLE IF NOT EXISTS tool_calls (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    duration_ms INTEGER,
    model TEXT,
    recorded_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (conversation_id) REFERENCES sessions(conversation_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS pressure_samples (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT,
    pressure TEXT NOT NULL,
    cpu_percent REAL,
    memory_available_gb REAL,
    docker_cpu_percent REAL,
    sampled_at TEXT NOT NULL DEFAULT (datetime('now'))
);
SQL
    fi
}

json_output() {
    echo "$1"
    exit 0
}

read_hook_input() {
    cat
}
