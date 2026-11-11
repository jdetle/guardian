#!/bin/bash
# Claude Code / Codex PreToolUse — load advisory, plus optional subagent deny (tool_gate policy).
#
# NEVER emits permissionDecision "allow": that would skip the agent's own permission prompt.
# With no decision in the output, the normal permission flow is untouched.
# stdin: hook JSON (tool_name, tool_input, session_id, …)
set -euo pipefail
export GUARDIAN_HOOK_FORMAT="${GUARDIAN_HOOK_FORMAT:-codex}"

# Installed to <agent config>/hooks/guardian/codex/ — shared assets live in the parent directory.
SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
source "$SCRIPT_DIR/lib.sh"
source "$SCRIPT_DIR/tool-gate-common.sh"

input=$(read_hook_input)
if ! printf '%s' "$input" | jq empty 2>/dev/null; then
    input="{}"
fi

guardian_tool_gate_eval "$input"

case "$TG_OUTCOME" in
    deny)
        json_output "$(jq -n --arg r "$TG_MESSAGE" \
            '{
                hookSpecificOutput: {
                    hookEventName: "PreToolUse",
                    permissionDecision: "deny",
                    permissionDecisionReason: $r
                }
            }')"
        ;;
    advise)
        session_id=$(printf '%s' "$input" | jq -r 'try (.session_id // "" | tostring) catch ""' 2>/dev/null || echo "")
        if guardian_advise_due "$session_id" "$TG_PRESSURE"; then
            json_output "$(jq -n --arg ctx "$TG_MESSAGE" \
                '{
                    hookSpecificOutput: {
                        hookEventName: "PreToolUse",
                        additionalContext: $ctx
                    }
                }')"
        fi
        ;;
esac

# Clear pressure (or a rate-limited advisory): no output, no decision.
exit 0
