#!/bin/bash
# Guardian sessionStart hook.
# Records session in DB and injects system pressure context.
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

if [ -n "$conversation_id" ]; then
    db_exec "INSERT OR IGNORE INTO sessions (conversation_id, model, pressure_at_start) VALUES ('$conversation_id', '$model', '$pressure');"
fi

context=""
case "$pressure" in
    critical)
        context="SYSTEM ALERT: Resources are critically low (CPU: ${cpu}%, Memory: ${mem}GB free). Minimize parallel operations, avoid spawning subagents, and defer Docker-heavy commands until pressure drops."
        ;;
    strained)
        context="SYSTEM NOTE: Resources are under moderate load (CPU: ${cpu}%, Memory: ${mem}GB free). Prefer sequential over parallel work. Avoid launching multiple subagents simultaneously."
        ;;
    *)
        context="System resources: nominal (CPU: ${cpu}%, Memory: ${mem}GB free)."
        ;;
esac

json_output "$(jq -n --arg ctx "$context" '{additional_context: $ctx}')"
