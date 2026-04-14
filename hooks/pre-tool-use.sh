#!/bin/bash
# Guardian preToolUse hook.
# ALWAYS allows. Provides system load context so agents can be aware.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

input=$(read_hook_input)

pressure=$(read_state_pressure)
cpu=$(read_state_field "cpu_percent" "?")
mem=$(read_state_field "memory_available_gb" "?")
swap=$(read_state_field "swap_used_percent" "?")

case "$pressure" in
    strained)
        json_output "$(jq -n \
            --arg msg "Guardian: Moderate load — CPU ${cpu}%, ${mem} GB RAM, swap ${swap}%. Go easy on parallelism." \
            '{permission: "allow", agent_message: $msg}')"
        ;;
    critical)
        json_output "$(jq -n \
            --arg msg "Guardian: High load — CPU ${cpu}%, ${mem} GB RAM, swap ${swap}%. Serial work; skip heavy containers." \
            '{permission: "allow", agent_message: $msg}')"
        ;;
    clear|*)
        json_output "$(jq -n '{permission: "allow"}')"
        ;;
esac
