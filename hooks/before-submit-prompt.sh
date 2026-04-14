#!/bin/bash
# Guardian beforeSubmitPrompt — may return continue:false for pressure or session budget.
# Overrides: ~/.guardian/snooze_until (ISO time), ~/.guardian/proceed_once (one-shot).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib.sh"
source "$SCRIPT_DIR/prompt-gate-common.sh"

input=$(read_hook_input)
if ! printf '%s' "$input" | jq empty 2>/dev/null; then
    input="{}"
fi

guardian_prompt_gate_eval "$input"

if [ "$PG_OUTCOME" = "block" ]; then
    json_output "$(jq -n --arg msg "$PG_MESSAGE" '{continue: false, user_message: $msg}')"
fi

if [ "$PG_OUTCOME" = "pass_msg" ]; then
    json_output "$(jq -n --arg msg "$PG_MESSAGE" '{continue: true, user_message: $msg}')"
fi

json_output "$(jq -n '{continue: true}')"
