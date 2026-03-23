#!/bin/bash
# Guardian subagentStart hook.
# ALWAYS allows. Provides system load context for agent awareness.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

input=$(read_hook_input)

pressure=$(read_state_pressure)
cpu=$(read_state_field "cpu_percent" "?")
mem=$(read_state_field "memory_available_gb" "?")
active_sessions=$(read_state_field "cursor.active_sessions" "0")

case "$pressure" in
    strained)
        json_output "$(jq -n \
            --arg msg "System Guardian: moderate load (CPU: ${cpu}%, Mem free: ${mem}GB, ${active_sessions} active sessions). Consider limiting parallel subagents." \
            '{permission: "allow", agent_message: $msg}')"
        ;;
    critical)
        json_output "$(jq -n \
            --arg msg "System Guardian: high load (CPU: ${cpu}%, Mem free: ${mem}GB, ${active_sessions} active sessions). Prefer sequential work to reduce system pressure." \
            '{permission: "allow", agent_message: $msg}')"
        ;;
    *)
        json_output '{"permission": "allow"}'
        ;;
esac
