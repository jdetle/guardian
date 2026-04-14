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
    banner="[Guardian] Agent registered and monitored (daemon active)."
else
    banner="[Guardian] Agent registered (daemon not detected — resource data unavailable)."
fi

context=""
case "$pressure" in
    critical)
        context="$banner SYSTEM ALERT: Resources are critically low (CPU: ${cpu}%, Memory: ${mem}GB free). Minimize parallel operations, avoid spawning subagents, and defer Docker-heavy commands until pressure drops."
        ;;
    strained)
        context="$banner SYSTEM NOTE: Resources are under moderate load (CPU: ${cpu}%, Memory: ${mem}GB free). Prefer sequential over parallel work. Avoid launching multiple subagents simultaneously."
        ;;
    *)
        context="$banner System resources: nominal (CPU: ${cpu}%, Memory: ${mem}GB free)."
        ;;
esac

G_home="${GUARDIAN_DIR:-$HOME/.guardian}"
if [ "$pressure" = "strained" ] && [ "$block_on_hint" = "critical" ]; then
    context="${context} [Before a hard block] Prompt gates block at critical load — snooze in the agent UI now: type /guardian-snooze in chat, or ${G_home}/guardian snooze 15."
fi
if [ "$pressure" = "critical" ]; then
    context="${context} Prompt submits may be blocked while load is critical — snooze: /guardian-snooze or ${G_home}/guardian snooze 15."
fi

# Workspace folder count is diagnostic only (stale dirs accumulate); RSS reflects actual load.
context="${context} Cursor workspace folders under ~/.cursor/projects ≈ ${cursor_sess}; Cursor RSS ≈ ${cursor_mb} MB (best-effort)."
cm="${cursor_mb%%.*}"
wr="${warn_rss%%.*}"
if [ "${wr:-0}" -gt 0 ] 2>/dev/null && [ "${cm:-0}" -gt 0 ] 2>/dev/null && [ "${cm:-0}" -gt "${wr:-0}" ] 2>/dev/null; then
    context="${context} NOTE: Cursor memory use is high (${cursor_mb} MB RSS, warn above ${warn_rss} MB) — prefer finishing or archiving heavy threads before more parallel work. See hooks/resources.md."
fi

disk_level=$(read_state_field "disk.level" "clear")
disk_used=$(read_state_field "disk.used_percent" "0")
disk_avail=$(read_state_field "disk.available_gb" "0")
disk_vol=$(read_state_field "disk.volume_path" "")
case "$disk_level" in
    warn)
        context="${context} NOTE: Home volume disk use is elevated (~${disk_used}% used, ~${disk_avail} GB free at ${disk_vol}). Free space: prune stale git worktrees, Docker images (docker system df / docker image prune), and large build caches — see hooks/resources.md."
        ;;
    critical)
        context="${context} DISK ALERT: Home volume is very full (~${disk_used}% used, ~${disk_avail} GB free at ${disk_vol}). Free space urgently: worktrees, docker image prune, target/node_modules/DerivedData caches — see hooks/resources.md."
        ;;
esac

queue_file="${GUARDIAN_DIR:-$HOME/.guardian}/agent_queue.jsonl"
if [ -f "$queue_file" ]; then
    qc=$(grep -c '^{' "$queue_file" 2>/dev/null || echo 0)
    if [ "${qc:-0}" -gt 0 ] 2>/dev/null; then
        context="${context} NOTE: ${qc} deferred agent task(s) in ~/.guardian/agent_queue.jsonl — run ~/.guardian/guardian-queue.sh list when pressure is clear (optional notifications: scripts/install-queue-watch.sh in the Guardian repo)."
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
