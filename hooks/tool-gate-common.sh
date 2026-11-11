#!/bin/bash
# Shared tool-gate evaluation for Cursor (preToolUse) and Claude Code / Codex (PreToolUse).
# Source after lib.sh. Call: guardian_tool_gate_eval "$input_json"
#
# Default is advisory only: every tool call is allowed and the agent is told about load.
# Optional enforcement: hook_policy.json `tool_gate.deny_subagents_on` ("never" | "strained" |
# "critical") denies NEW subagent spawns at that pressure — subagent fan-out is the main way a
# single agent turn multiplies memory use. A snooze (`guardian snooze 15`) lifts the deny.
#
# Fail-open: missing / stale / corrupt state reads as "clear" (see read_state_pressure).
# Sets:
#   TG_OUTCOME=pass|advise|deny
#   TG_MESSAGE=""     # advisory or deny reason
#   TG_PRESSURE=""    # clear|strained|critical

guardian_tool_is_subagent_spawn() {
    case "$1" in
        Agent|Task|spawn_agent) return 0 ;;
    esac
    return 1
}

guardian_tool_gate_eval() {
    local input="$1"
    TG_OUTCOME="pass"
    TG_MESSAGE=""
    TG_PRESSURE=$(read_state_pressure)

    if [ "$TG_PRESSURE" = "clear" ]; then
        return 0
    fi

    local cpu mem swap tool why
    cpu=$(read_state_field "cpu_percent" "?")
    mem=$(read_state_field "memory_available_gb" "?")
    swap=$(read_state_field "swap_used_percent" "?")
    why=$(read_state_reasons_suffix)
    tool=$(printf '%s' "$input" | jq -r 'try (.tool_name // "" | tostring) catch ""' 2>/dev/null || echo "")

    if guardian_tool_is_subagent_spawn "$tool" && ! guardian_snooze_active; then
        local policy_file deny_on="never"
        policy_file=$(guardian_hook_policy_file)
        if [ -n "${policy_file:-}" ] && jq empty "$policy_file" 2>/dev/null; then
            deny_on=$(jq -r '.tool_gate.deny_subagents_on // "never"' "$policy_file" 2>/dev/null || echo "never")
        fi
        case "${deny_on}:${TG_PRESSURE}" in
            critical:critical|strained:critical|strained:strained)
                TG_OUTCOME="deny"
                TG_MESSAGE="Guardian: Subagent blocked — ${TG_PRESSURE} load (CPU ${cpu}%, ${mem} GB RAM free, swap ${swap}%).${why} Do this work yourself, serially, in the current session; do not retry the spawn. The user can lift this with: ${GUARDIAN_DIR}/guardian snooze 15"
                return 0
                ;;
        esac
    fi

    TG_OUTCOME="advise"
    case "$TG_PRESSURE" in
        strained)
            TG_MESSAGE="Guardian: Moderate load — CPU ${cpu}%, ${mem} GB RAM, swap ${swap}%. Go easy on parallelism.${why}"
            ;;
        critical)
            TG_MESSAGE="Guardian: High load — CPU ${cpu}%, ${mem} GB RAM, swap ${swap}%. Serial work; skip heavy containers.${why}"
            ;;
    esac
    return 0
}

# Rate limit for advisories on agents that append hook context to the transcript (Claude Code,
# Codex): repeat the same pressure level at most once per interval per session.
# Args: session_id level. Returns 0 when the advisory should be emitted now.
GUARDIAN_ADVISE_INTERVAL_SECS="${GUARDIAN_ADVISE_INTERVAL_SECS:-120}"

guardian_advise_due() {
    local sid level="$2" dir f now prev_level="" prev_epoch=""
    sid=$(printf '%s' "$1" | tr -cd 'A-Za-z0-9_-' | head -c 64)
    [ -n "$sid" ] || sid="nosession"
    dir="$GUARDIAN_DIR/advise"
    f="$dir/$sid"
    now=$(date -u +%s)

    if [ -f "$f" ] && [ ! -L "$f" ]; then
        read -r prev_level prev_epoch < "$f" 2>/dev/null || true
        case "$prev_epoch" in
            ''|*[!0-9]*) prev_epoch=0 ;;
        esac
        if [ "$prev_level" = "$level" ] && [ $(( now - prev_epoch )) -lt "$GUARDIAN_ADVISE_INTERVAL_SECS" ]; then
            return 1
        fi
    fi

    # Best-effort stamp; if it cannot be written the advisory is simply not rate limited.
    if [ ! -L "$dir" ] && mkdir -p "$dir" 2>/dev/null && [ ! -L "$f" ]; then
        printf '%s %s\n' "$level" "$now" > "$f" 2>/dev/null || true
    fi
    return 0
}

# Remove advisory stamps from sessions that ended long ago (called from sessionStart).
guardian_advise_prune() {
    local dir="$GUARDIAN_DIR/advise"
    [ -d "$dir" ] && [ ! -L "$dir" ] || return 0
    find "$dir" -type f -mtime +1 -delete 2>/dev/null || true
}
