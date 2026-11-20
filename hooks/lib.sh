#!/bin/bash
# Shared utilities for Guardian hooks.
# Sourced by all hook scripts -- not executed directly.
#
# DESIGN: Tool hooks (preToolUse, etc.) default to allow with advisory text.
# beforeSubmitPrompt may return continue:false when policy + load require it;
# users can always resume via ~/.guardian/guardian CLI, .command clickables, or snooze (see resources.md). Codex uses hooks/codex/.
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
    # Fast path, no python spawn: UTC timestamps as written by guardiand (…+00:00) and the
    # guardian CLI (…Z). Hooks run on every tool call, so this matters under memory pressure.
    # Pure integer arithmetic (days-from-civil) rather than jq's strptime/mktime, which has
    # been off by an hour under DST in some jq 1.6 builds — that would silently turn gates off.
    case "$ts" in
        *Z|*+00:00)
            local jq_epoch
            if jq_epoch=$(jq -rn --arg t "$ts" '
                $t
                | capture("^(?<y>[0-9]{4})-(?<m>[0-9]{2})-(?<d>[0-9]{2})T(?<H>[0-9]{2}):(?<M>[0-9]{2}):(?<S>[0-9]{2})(?:\\.[0-9]+)?(?:Z|\\+00:00)$")
                | map_values(tonumber)
                | (if .m <= 2 then .y - 1 else .y end) as $y
                | ($y / 400 | floor) as $era
                | ($y - $era * 400) as $yoe
                | (((153 * (.m + (if .m > 2 then -3 else 9 end)) + 2) / 5 | floor) + .d - 1) as $doy
                | ($yoe * 365 + ($yoe / 4 | floor) - ($yoe / 100 | floor) + $doy) as $doe
                | ($era * 146097 + $doe - 719468) * 86400 + .H * 3600 + .M * 60 + .S
                ' 2>/dev/null) \
                && [ -n "$jq_epoch" ]; then
                echo "$jq_epoch"
                return 0
            fi
            ;;
    esac
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

    # No timestamp means we cannot tell how old the sample is: treat it as not live.
    local sampled_at timestamp_filter='try (.sampled_at // "" | tostring) catch ""'
    if [ "$#" -gt 0 ]; then
        sampled_at=$(printf '%s' "$1" | jq -r "$timestamp_filter" 2>/dev/null) || return 1
    else
        sampled_at=$(jq -r "$timestamp_filter" < "$STATE_FILE" 2>/dev/null) || return 1
    fi
    if [ -z "$sampled_at" ]; then
        return 1
    fi

    local state_epoch now_epoch age
    state_epoch=$(parse_iso_epoch "$sampled_at")
    case "$state_epoch" in
        ''|*[!0-9]*) return 1 ;;
    esac
    now_epoch=$(date -u +%s)
    age=$(( now_epoch - state_epoch ))
    if [ "$age" -gt "$STALE_THRESHOLD_SECS" ]; then
        return 1
    fi
    # A sample from the future (clock change, hand-edited file) is not a live sample either.
    if [ "$age" -lt "-$STALE_THRESHOLD_SECS" ]; then
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

    # Staleness check: only a fresh, timestamped sample is trusted. A state.json that is old,
    # future-dated, or has no sampled_at at all means the daemon is not reporting — fail open.
    # Same rule as is_daemon_active, so "gates are off" messages and gate behavior agree.
    if ! is_daemon_active; then
        echo "clear"
        return
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

# Why the daemon classified the current sample above clear, e.g. "swap 93% used; thermal
# pressure heavy" (guardiand >= 0.6.0 writes pressure_reasons). Prints " Why: ….", or nothing
# when the daemon gave no reasons, so callers can append it unconditionally.
read_state_reasons_suffix() {
    if [ ! -f "$STATE_FILE" ] || [ -L "$STATE_FILE" ]; then
        return 0
    fi
    local reasons filter='try (
        (.pressure_reasons // [])
        | if type == "array" then (
              map(select(type == "string")) | .[0:4] | join("; ")
              # state.json is user-writable and this text lands in agent context:
              # keep it one plain line.
              | gsub("[[:cntrl:]]"; " ") | .[0:200] | sub("[. ]+$"; "")
          ) else "" end
    ) catch ""'
    if [ "$#" -gt 0 ]; then
        reasons=$(jq -r "$filter" <<< "$1" 2>/dev/null) || reasons=""
    else
        reasons=$(jq -r "$filter" < "$STATE_FILE" 2>/dev/null) || reasons=""
    fi
    if [ -n "$reasons" ]; then
        printf ' Why: %s.' "$reasons"
    fi
}

# Problems guardiand found in config.toml (guardiand >= 0.8 writes config_problems), as one
# plain line: "a; b". Empty when the file is clean. At most two, since this lands in agent
# context; `guardian config check` lists them all.
read_state_config_problems() {
    if [ ! -f "$STATE_FILE" ] || [ -L "$STATE_FILE" ]; then
        return 0
    fi
    jq -r 'try (
        (.config_problems // [])
        | if type == "array" then (
              map(select(type == "string")) | .[0:2] | join("; ")
              | gsub("[[:cntrl:]]"; " ") | .[0:300] | sub("[. ]+$"; "")
          ) else "" end
    ) catch ""' < "$STATE_FILE" 2>/dev/null || true
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

# Cursor beforeSubmitPrompt / Codex UserPromptSubmit — merge fields that may carry the user message.
guardian_hook_prompt_text_bundle() {
    local json="$1"
    printf '%s' "$json" | jq -r 'try (
        [ (.prompt // ""), (.promptText // ""), (.text // ""), (.message // ""), (.input // ""), (.content // "") ]
        | map(tostring) | join("\n")
    ) catch ""' 2>/dev/null || echo ""
}

# Never block Guardian “meta” prompts: slash helpers + guardian-install / guardian-upgrade skills.
# Returns 0 when the prompt gate should skip blocking (PG_OUTCOME stays pass).
guardian_hook_is_guardian_meta_prompt() {
    local raw="$1"
    [ -n "$raw" ] || return 1
    local t
    t=$(printf '%s' "$raw" | head -c 250000)

    local first
    first=$(printf '%s' "$t" | sed '/./!d' | head -1 | sed 's/^[[:space:]]*//')
    case "$first" in
        /guardian-snooze*) return 0 ;;
        /guardian-once*) return 0 ;;
    esac

    if printf '%s' "$t" | grep -qF 'skills/guardian-install'; then return 0; fi
    if printf '%s' "$t" | grep -qF 'skills/guardian-upgrade'; then return 0; fi
    if printf '%s' "$t" | grep -qF 'guardian-install/SKILL.md'; then return 0; fi
    if printf '%s' "$t" | grep -qF 'guardian-upgrade/SKILL.md'; then return 0; fi
    if printf '%s' "$t" | grep -qF '.cursor/skills/guardian-install'; then return 0; fi
    if printf '%s' "$t" | grep -qF '.cursor/skills/guardian-upgrade'; then return 0; fi

    return 1
}

# Longest snooze the hooks honor. `guardian snooze` refuses more (MAX_SNOOZE_MINUTES in the
# CLI); a snooze_until further out than this is ignored rather than trusted. A gate that
# should be off for longer belongs in config.toml, where it stays visible.
GUARDIAN_MAX_SNOOZE_SECS=7200

# Seconds of snooze left; 0 when not snoozed (no file, expired, unreadable, or beyond the cap).
guardian_snooze_remaining_secs() {
    local f="$GUARDIAN_DIR/snooze_until" until_s until_epoch now_epoch left
    if [ ! -f "$f" ]; then
        echo 0
        return 0
    fi
    until_s=$(head -1 "$f" 2>/dev/null | tr -d '\r\n')
    until_epoch=$(parse_iso_epoch "$until_s")
    case "$until_epoch" in
        ''|*[!0-9]*) until_epoch=0 ;;
    esac
    now_epoch=$(date -u +%s)
    left=$(( until_epoch - now_epoch ))
    # Two minutes of slack: clock adjustments, and the moment between writing and reading.
    if [ "$left" -le 0 ] || [ "$left" -gt $(( GUARDIAN_MAX_SNOOZE_SECS + 120 )) ]; then
        echo 0
        return 0
    fi
    echo "$left"
}

guardian_snooze_active() {
    [ "$(guardian_snooze_remaining_secs)" -gt 0 ]
}

# An allow-once token is good for about this long after `guardian once` (or
# `touch ~/.guardian/proceed_once`). Without an expiry, a token armed and then not needed
# would sit there and wave through some unrelated block days later.
GUARDIAN_ONCE_TTL_MINUTES=10

# Is an unexpired allow-once token present? Does not consume it.
guardian_proceed_once_armed() {
    local f="$GUARDIAN_DIR/proceed_once"
    [ -f "$f" ] || return 1
    # BSD find rounds the age up to whole minutes, GNU find does not: +1 keeps both at
    # "at least the TTL".
    [ -n "$(find "$f" -mmin "-$(( GUARDIAN_ONCE_TTL_MINUTES + 1 ))" 2>/dev/null)" ]
}

# Only the hook that removes the marker gets the bypass. An expired token is removed too,
# but grants nothing.
guardian_consume_proceed_once() {
    local fresh=1
    guardian_proceed_once_armed || fresh=0
    rm "$GUARDIAN_DIR/proceed_once" 2>/dev/null || return 1
    [ "$fresh" = 1 ]
}

guardian_resume_hint_text() {
    local cli
    printf -v cli '%q' "${GUARDIAN_DIR}/guardian"
    printf 'Use the Guardian menu bar or run %s once (one prompt) or %s snooze 15, then resend.' "$cli" "$cli"
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
