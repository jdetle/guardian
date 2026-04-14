#!/bin/bash
# Guardian sessionStart hook.
# Records session in DB and injects system pressure + session-budget context.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

input=$(read_hook_input)
ensure_db

conversation_id=$(sanitize_sql "$(echo "$input" | jq -r '.conversation_id // ""')")
model=$(sanitize_sql "$(echo "$input" | jq -r '.model // "unknown"')")

pressure=$(read_state_pressure)
cpu=$(read_state_field "cpu_percent" "0")
mem=$(read_state_field "memory_available_gb" "0")
cursor_sess=$(read_state_field "cursor.active_sessions" "0")
cursor_mb=$(read_state_field "cursor.resident_memory_megabytes" "0")

policy_file=$(guardian_hook_policy_file)
warn_rss="4096"
block_on_hint="critical"
if [ -n "${policy_file:-}" ] && [ -f "$policy_file" ]; then
    warn_rss=$(jq -r '.session_budget.warn_cursor_rss_megabytes // 4096' "$policy_file")
    block_on_hint=$(jq -r '.prompt_gate.block_on // "critical"' "$policy_file" 2>/dev/null || echo "critical")
fi

if [ -n "$conversation_id" ]; then
    db_exec "INSERT OR IGNORE INTO sessions (conversation_id, model, pressure_at_start) VALUES ('$conversation_id', '$model', '$pressure');"
fi

banner=""
if is_daemon_active; then
    banner="Guardian: Session on — metrics live."
else
    banner="Guardian: Session on — no daemon (metrics stale)."
fi

G_home="${GUARDIAN_DIR:-$HOME/.guardian}"
context=""
case "$pressure" in
    critical)
        context="${banner} Load critical: CPU ${cpu}%, ${mem} GB free. Ease up; sends may block. Snooze: /guardian-snooze · ${G_home}/guardian snooze 15."
        ;;
    strained)
        context="${banner} Load strained: CPU ${cpu}%, ${mem} GB free. Prefer serial work."
        if [ "$block_on_hint" = "critical" ]; then
            context="${context} Gates block at critical — snooze early: /guardian-snooze."
        fi
        ;;
    *)
        context="${banner} Load OK: CPU ${cpu}%, ${mem} GB free."
        ;;
esac

# Workspace folder count is diagnostic only (stale dirs accumulate); RSS reflects actual load.
context="${context} · ~/.cursor/projects ≈ ${cursor_sess} folders · Cursor RSS ≈ ${cursor_mb} MB."
cm="${cursor_mb%%.*}"
wr="${warn_rss%%.*}"
if [ "${wr:-0}" -gt 0 ] 2>/dev/null && [ "${cm:-0}" -gt 0 ] 2>/dev/null && [ "${cm:-0}" -gt "${wr:-0}" ] 2>/dev/null; then
    context="${context} High Cursor RAM (${cursor_mb} MB, warn ${warn_rss} MB) — trim threads if you can."
fi

disk_level=$(read_state_field "disk.level" "clear")
disk_used=$(read_state_field "disk.used_percent" "0")
disk_avail=$(read_state_field "disk.available_gb" "0")
disk_vol=$(read_state_field "disk.volume_path" "")
case "$disk_level" in
    warn)
        context="${context} Disk tight: ~${disk_used}% used (~${disk_avail} GB free on ${disk_vol})."
        ;;
    critical)
        context="${context} Disk critical: ~${disk_used}% used (~${disk_avail} GB free on ${disk_vol}) — free space soon."
        ;;
esac

queue_file="${GUARDIAN_DIR:-$HOME/.guardian}/agent_queue.jsonl"
if [ -f "$queue_file" ]; then
    qc=$(grep -c '^{' "$queue_file" 2>/dev/null || echo 0)
    if [ "${qc:-0}" -gt 0 ] 2>/dev/null; then
        context="${context} Queue: ${qc} job(s) in ~/.guardian/agent_queue.jsonl (guardian-queue.sh list)."
    fi
fi

# Cursor expects additional_context; Codex expects hookSpecificOutput (see hooks/codex/session-start.sh).
if [ "${GUARDIAN_HOOK_FORMAT:-cursor}" = "codex" ]; then
    json_output "$(jq -n \
        --arg ctx "$context" \
        '{
            hookSpecificOutput: {
                hookEventName: "SessionStart",
                additionalContext: $ctx
            }
        }')"
else
    json_output "$(jq -n --arg ctx "$context" '{additional_context: $ctx}')"
fi
