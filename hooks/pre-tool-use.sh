#!/bin/bash
# Guardian preToolUse hook (Cursor).
# Allows by default and provides system load context so agents can be aware.
# Only denies subagent spawns when hook_policy.json tool_gate.deny_subagents_on opts in.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib.sh"
source "$SCRIPT_DIR/tool-gate-common.sh"

input=$(read_hook_input)
if ! printf '%s' "$input" | jq empty 2>/dev/null; then
    input="{}"
fi

guardian_tool_gate_eval "$input"

case "$TG_OUTCOME" in
    deny)
        json_output "$(jq -n --arg msg "$TG_MESSAGE" \
            '{permission: "deny", user_message: $msg, agent_message: $msg}')"
        ;;
    advise)
        json_output "$(jq -n --arg msg "$TG_MESSAGE" \
            '{permission: "allow", agent_message: $msg}')"
        ;;
    *)
        json_output "$(jq -n '{permission: "allow"}')"
        ;;
esac
