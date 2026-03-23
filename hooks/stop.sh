#!/bin/bash
# Guardian stop hook.
# Records session end data and computes ROI score.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

input=$(read_hook_input)
ensure_db

conversation_id=$(sanitize_sql "$(echo "$input" | jq -r '.conversation_id // ""')")
status=$(sanitize_sql "$(echo "$input" | jq -r '.status // "unknown"')")
loop_count=$(echo "$input" | jq -r '.loop_count // 0')

if [ -z "$conversation_id" ]; then
    exit 0
fi

# Update session end data
db_exec "UPDATE sessions SET
    ended_at = datetime('now'),
    status = '$status',
    loop_count = $loop_count,
    duration_ms = CAST((julianday('now') - julianday(started_at)) * 86400000 AS INTEGER)
WHERE conversation_id = '$conversation_id';"

# Compute average pressure during session
avg_pressure=$(db_exec "SELECT pressure FROM pressure_samples WHERE conversation_id = '$conversation_id' GROUP BY pressure ORDER BY COUNT(*) DESC LIMIT 1;")
if [ -n "$avg_pressure" ]; then
    db_exec "UPDATE sessions SET pressure_avg = '$avg_pressure' WHERE conversation_id = '$conversation_id';"
fi

# Estimate cost based on model and tool calls
tool_count=$(db_exec "SELECT COUNT(*) FROM tool_calls WHERE conversation_id = '$conversation_id';")
tool_count=${tool_count:-0}

model=$(db_exec "SELECT model FROM sessions WHERE conversation_id = '$conversation_id';")

# Rough cost estimation (USD per tool call by model tier)
cost_per_call="0.01"
case "$model" in
    *opus*|*claude-4*) cost_per_call="0.05" ;;
    *sonnet*) cost_per_call="0.01" ;;
    *haiku*|*fast*) cost_per_call="0.002" ;;
esac

estimated_cost=$(echo "$tool_count * $cost_per_call" | bc -l 2>/dev/null || echo "0")
db_exec "UPDATE sessions SET estimated_cost_usd = $estimated_cost WHERE conversation_id = '$conversation_id';"

# Compute ROI score (0-100)
# Factors: status (30%), tool_count proxy for work done (25%),
# efficiency = tool_calls/duration (25%), resource cost inverse (20%)
status_score=0
case "$status" in
    completed) status_score=100 ;;
    aborted) status_score=30 ;;
    error) status_score=10 ;;
esac

# Tool count score: more tools = more work done (capped at 50 calls = 100)
tool_score=0
if [ "$tool_count" -gt 0 ] 2>/dev/null; then
    tool_score=$(( tool_count > 50 ? 100 : tool_count * 2 ))
fi

# Duration from DB
duration_ms=$(db_exec "SELECT duration_ms FROM sessions WHERE conversation_id = '$conversation_id';")
duration_ms=${duration_ms:-1}

# Efficiency: tool calls per minute (higher is better, capped at 10/min = 100)
if [ "$duration_ms" -gt 0 ] 2>/dev/null; then
    calls_per_min=$(echo "scale=2; $tool_count / ($duration_ms / 60000.0)" | bc -l 2>/dev/null || echo "0")
    efficiency_score=$(echo "scale=0; e = $calls_per_min * 10; if (e > 100) 100 else e" | bc -l 2>/dev/null || echo "50")
else
    efficiency_score=50
fi

# Resource cost: average CPU during session (lower avg CPU = higher score)
avg_cpu=$(db_exec "SELECT AVG(cpu_percent) FROM pressure_samples WHERE conversation_id = '$conversation_id';")
avg_cpu=${avg_cpu:-50}
resource_score=$(echo "scale=0; 100 - $avg_cpu" | bc -l 2>/dev/null || echo "50")
if [ "${resource_score:-0}" -lt 0 ] 2>/dev/null; then
    resource_score=0
fi

# Weighted ROI
roi=$(echo "scale=1; ($status_score * 0.30) + ($tool_score * 0.25) + ($efficiency_score * 0.25) + ($resource_score * 0.20)" | bc -l 2>/dev/null || echo "50")
db_exec "UPDATE sessions SET roi_score = $roi WHERE conversation_id = '$conversation_id';"

exit 0
