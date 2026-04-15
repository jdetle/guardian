#!/bin/bash
# Claude Code PreToolUse — advisory-only load context; always allow (hookSpecificOutput).
# stdin: Claude Code JSON (tool_name, tool_input, …)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
source "$SCRIPT_DIR/lib.sh"

read_hook_input >/dev/null

pressure=$(read_state_pressure)
cpu=$(read_state_field "cpu_percent" "?")
mem=$(read_state_field "memory_available_gb" "?")
swap=$(read_state_field "swap_used_percent" "?")

claude_emit_allow() {
    jq -n '{
        hookSpecificOutput: {
            hookEventName: "PreToolUse",
            permissionDecision: "allow"
        }
    }'
}

claude_emit_allow_advisory() {
    local text="$1"
    jq -n \
        --arg msg "$text" \
        '{
            hookSpecificOutput: {
                hookEventName: "PreToolUse",
                permissionDecision: "allow",
                additionalContext: $msg
            }
        }'
}

case "$pressure" in
    strained)
        json_output "$(claude_emit_allow_advisory "Guardian: Moderate load — CPU ${cpu}%, ${mem} GB RAM, swap ${swap}%. Go easy on parallelism.")"
        ;;
    critical)
        json_output "$(claude_emit_allow_advisory "Guardian: High load — CPU ${cpu}%, ${mem} GB RAM, swap ${swap}%. Serial work; skip heavy containers.")"
        ;;
    clear|*)
        json_output "$(claude_emit_allow)"
        ;;
esac
