#!/bin/bash
# Codex UserPromptSubmit — same policy as Cursor beforeSubmitPrompt; emits Codex hook JSON.
# stdin: Codex JSON (prompt, session_id, cwd, …)
set -euo pipefail

# Installed to ~/.codex/hooks/guardian/codex/ — shared assets live in parent directory.
SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
source "$SCRIPT_DIR/lib.sh"
source "$SCRIPT_DIR/prompt-gate-common.sh"

input=$(read_hook_input)
if ! printf '%s' "$input" | jq empty 2>/dev/null; then
    input="{}"
fi

guardian_prompt_gate_eval "$input"

codex_emit_allow() {
    jq -n '{continue: true}'
}

codex_emit_allow_advisory() {
    local text="$1"
    jq -n \
        --arg ctx "$text" \
        '{
            continue: true,
            hookSpecificOutput: {
                hookEventName: "UserPromptSubmit",
                additionalContext: $ctx
            }
        }'
}

codex_emit_block() {
    local reason="$1"
    jq -n --arg r "$reason" '{decision: "block", reason: $r}'
}

case "$PG_OUTCOME" in
    block)
        json_output "$(codex_emit_block "$PG_MESSAGE")"
        ;;
    pass_msg)
        json_output "$(codex_emit_allow_advisory "$PG_MESSAGE")"
        ;;
    pass)
        json_output "$(codex_emit_allow)"
        ;;
    *)
        json_output "$(codex_emit_allow)"
        ;;
esac
