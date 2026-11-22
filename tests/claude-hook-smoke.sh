#!/bin/bash
# Minimal smoke test: Claude Code adapters emit valid JSON shapes against whatever state is live.
# Run from repo root: bash tests/claude-hook-smoke.sh
# Behavior under simulated pressure is covered by the containerized suite (group 7b).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UPS="$ROOT/hooks/claude/user-prompt-submit.sh"
PTU="$ROOT/hooks/claude/pre-tool-use.sh"

# The hooks record what they see (sessions.db) and keep small state files, so they are run
# against a scratch home that holds a copy of the live readings: nothing is written to the
# real ~/.guardian.
REAL_GUARDIAN="$HOME/.guardian"
SCRATCH="$(mktemp -d)"
if [ -z "$SCRATCH" ] || [ ! -d "$SCRATCH" ]; then
    echo "hook-smoke: could not make a scratch directory" >&2
    exit 2
fi
trap 'rm -rf "$SCRATCH"' EXIT
mkdir -p "$SCRATCH/.guardian"
for f in state.json hook_policy.json; do
    if [ -f "$REAL_GUARDIAN/$f" ]; then
        cp "$REAL_GUARDIAN/$f" "$SCRATCH/.guardian/$f"
    fi
done
export HOME="$SCRATCH"
unset GUARDIAN_HOOK_FORMAT CLAUDE_CONFIG_DIR CODEX_HOME

out=$(printf '%s' '{"prompt":"smoke","cwd":"/tmp","session_id":"test"}' | bash "$UPS")
echo "$out" | jq empty >/dev/null

kind=$(echo "$out" | jq -r 'if .decision then "block" elif .hookSpecificOutput then "advisory" elif .["continue"] == true then "allow" else "other" end')

case "$kind" in
    allow|advisory|block)
        echo "claude-hook-smoke: UserPromptSubmit ok (shape=$kind)"
        ;;
    *)
        echo "claude-hook-smoke: UserPromptSubmit unexpected JSON: $out" >&2
        exit 1
        ;;
esac

# PreToolUse: silent when load is clear, otherwise context (or an opt-in subagent deny).
# It must NEVER answer "allow" — that would skip Claude Code's own permission prompt.
pt_in='{"session_id":"smoke-test","cwd":"/tmp","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"true"}}'
pt_out=$(printf '%s' "$pt_in" | bash "$PTU")

if [ -z "$pt_out" ]; then
    echo "claude-hook-smoke: PreToolUse ok (silent — no decision, no context)"
else
    echo "$pt_out" | jq empty >/dev/null
    hen=$(echo "$pt_out" | jq -r '.hookSpecificOutput.hookEventName // empty')
    pd=$(echo "$pt_out" | jq -r '.hookSpecificOutput.permissionDecision // "none"')
    if [ "$hen" != "PreToolUse" ] || [ "$pd" = "allow" ]; then
        echo "claude-hook-smoke: PreToolUse unexpected JSON: $pt_out" >&2
        exit 1
    fi
    echo "claude-hook-smoke: PreToolUse ok (hookEventName=$hen permissionDecision=$pd)"
fi

echo "claude-hook-smoke: ok"
