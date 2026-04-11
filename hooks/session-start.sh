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
warn_sess="5"
if [ -n "${policy_file:-}" ] && [ -f "$policy_file" ]; then
    warn_sess=$(jq -r '.session_budget.warn_active_sessions // 5' "$policy_file")
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

# Heuristic parallel load (see README): count of dirs under ~/.cursor/projects
context="${context} Cursor workspaces tracked ≈ ${cursor_sess}; Cursor RSS ≈ ${cursor_mb} MB (best-effort)."
if [ "${cursor_sess%%.*}" -ge "${warn_sess%%.*}" ] 2>/dev/null; then
    context="${context} NOTE: High parallel workspace count — finish or archive other Agent/Composer sessions before heavy work. See hooks/resources.md."
fi

json_output "$(jq -n --arg ctx "$context" '{additional_context: $ctx}')"
