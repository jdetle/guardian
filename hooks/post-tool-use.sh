#!/bin/bash
# Guardian postToolUse hook.
# Records tool call metadata in sessions.db for ROI tracking.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

input=$(read_hook_input)
ensure_db

conversation_id=$(sanitize_sql "$(echo "$input" | jq -r '.conversation_id // ""')")
tool_name=$(sanitize_sql "$(echo "$input" | jq -r '.tool_name // "unknown"')")
duration=$(echo "$input" | jq -r '.duration // 0')
model=$(sanitize_sql "$(echo "$input" | jq -r '.model // "unknown"')")

if [ -n "$conversation_id" ]; then
    db_exec "INSERT INTO tool_calls (conversation_id, tool_name, duration_ms, model) VALUES ('$conversation_id', '$tool_name', $duration, '$model');"
    db_exec "UPDATE sessions SET tool_call_count = (SELECT COUNT(*) FROM tool_calls WHERE conversation_id = '$conversation_id') WHERE conversation_id = '$conversation_id';"
fi

# Also sample current pressure for session-level tracking
pressure=$(read_state_pressure)
cpu=$(read_state_field "cpu_percent" "0")
mem=$(read_state_field "memory_available_gb" "0")
docker_cpu=$(read_state_field "docker.total_cpu_percent" "0")

if [ -n "$conversation_id" ]; then
    db_exec "INSERT INTO pressure_samples (conversation_id, pressure, cpu_percent, memory_available_gb, docker_cpu_percent) VALUES ('$conversation_id', '$pressure', $cpu, $mem, $docker_cpu);"
fi

exit 0
