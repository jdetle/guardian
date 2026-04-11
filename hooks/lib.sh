#!/bin/bash
# Shared utilities for Guardian hooks.
# Sourced by all hook scripts -- not executed directly.
#
# DESIGN: Tool hooks (preToolUse, etc.) default to allow with advisory text.
# beforeSubmitPrompt may return continue:false when policy + load require it;
# users can always resume via ~/.guardian/proceed_once or snooze (see resources.md).
# The daemon handles enforcement (Docker throttling, fork guard).

GUARDIAN_DIR="$HOME/.guardian"
STATE_FILE="$GUARDIAN_DIR/state.json"
SESSIONS_DB="$GUARDIAN_DIR/sessions.db"
STALE_THRESHOLD_SECS=30

# RFC3339 / ISO-8601 with fractional seconds and offsets (chrono-compatible).
# Uses hooks/iso_to_epoch.py when available; never truncates at the first "." (that broke +00:00).
parse_iso_epoch() {
    local ts="$1"
    if [ -z "$ts" ]; then
        echo 0
        return 0
    fi
    local lib_dir
    lib_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    if [ -f "$lib_dir/iso_to_epoch.py" ] && command -v python3 &>/dev/null; then
        local py_epoch
        if py_epoch=$(python3 "$lib_dir/iso_to_epoch.py" "$ts" 2>/dev/null); then
            echo "$py_epoch"
            return 0
        fi
    fi
    # GNU date (Linux): often parses full RFC3339
    if epoch=$(date -ud "$ts" +%s 2>/dev/null); then
        echo "$epoch"
        return 0
    fi
    # Unknown format — fail safe (epoch 0 => very "old" => hooks treat daemon as stale)
    echo 0
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

# --- Prompt gates & resume (beforeSubmitPrompt) ---

guardian_hook_policy_file() {
    if [ -f "$GUARDIAN_DIR/hook_policy.json" ]; then
        echo "$GUARDIAN_DIR/hook_policy.json"
    elif [ -n "${SCRIPT_DIR:-}" ] && [ -f "$SCRIPT_DIR/hook_policy.default.json" ]; then
        echo "$SCRIPT_DIR/hook_policy.default.json"
    fi
}

# Safe JSON extractors (never fail the shell under set -e).
guardian_hook_attachment_paths() {
    local json="$1"
    echo "$json" | jq -r 'try ((.attachments // []) | if type == "array" then .[] | (if type == "object" then .file_path // empty elif type == "string" then . else empty end) else empty end) catch empty' 2>/dev/null || true
}

guardian_hook_workspace_roots() {
    local json="$1"
    echo "$json" | jq -r 'try ((.workspace_roots // []) | if type == "array" then .[] else empty end) catch empty' 2>/dev/null || true
}

guardian_snooze_active() {
    local f="$GUARDIAN_DIR/snooze_until"
    [ -f "$f" ] || return 1
    local until_s until_epoch now_epoch
    until_s=$(head -1 "$f" | tr -d '\r\n')
    [ -n "$until_s" ] || return 1
    until_epoch=$(parse_iso_epoch "$until_s")
    now_epoch=$(date -u +%s)
    [ "$now_epoch" -lt "$until_epoch" ]
}

# Removes ~/.guardian/proceed_once if present; returns 0 if consumed.
guardian_consume_proceed_once() {
    local f="$GUARDIAN_DIR/proceed_once"
    if [ -f "$f" ]; then
        rm -f "$f"
        return 0
    fi
    return 1
}

guardian_resume_hint_text() {
    printf '[Guardian] Resume: one-shot `touch %s/proceed_once` then submit again; or `bash scripts/guardian-resume.sh snooze 15` from the Guardian repo to snooze gates; or write a future ISO timestamp into %s/snooze_until.' \
        "$GUARDIAN_DIR" "$GUARDIAN_DIR"
}

# Path to guardian-queue CLI (installed to ~/.guardian by hooks/install-hooks.sh).
guardian_queue_cli() {
    local g="${GUARDIAN_DIR:-$HOME/.guardian}/guardian-queue.sh"
    if [ -x "$g" ]; then
        echo "$g"
        return 0
    fi
    echo ""
    return 1
}

# Returns 0 if we should show cursorignore warning for this path (warn-once cache).
guardian_cursorignore_should_warn() {
    local path_key="$1"
    local policy_file="$2"
    local once
    once=$(jq -r '.cursorignore_policy.warn_once_per_path // true' < "$policy_file" 2>/dev/null || echo "true")
    if [ "$once" != "true" ]; then
        return 0
    fi
    local cache="$GUARDIAN_DIR/cursorignore_warned.json"
    mkdir -p "$GUARDIAN_DIR"
    [ -f "$cache" ] || echo '{}' > "$cache"
    local hit
    hit=$(jq -r --arg p "$path_key" '.[$p] // empty' "$cache" 2>/dev/null || echo "")
    if [ -n "$hit" ]; then
        return 1
    fi
    jq --arg p "$path_key" '. + {($p): true}' "$cache" > "${cache}.tmp" 2>/dev/null && mv "${cache}.tmp" "$cache" 2>/dev/null || true
    return 0
}
