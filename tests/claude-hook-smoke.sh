#!/bin/bash
# Minimal smoke test: Claude Code adapters emit valid JSON shapes.
# Run from repo root: bash tests/claude-hook-smoke.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UPS="$ROOT/hooks/codex/user-prompt-submit.sh"
PTU="$ROOT/hooks/claude/pre-tool-use.sh"

for f in "$UPS" "$PTU"; do
    if [ ! -x "$f" ] && [ -f "$f" ]; then
        chmod +x "$f"
    fi
done

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

pt_in='{"session_id":"s","cwd":"/tmp","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"true"}}'
pt_out=$(printf '%s' "$pt_in" | bash "$PTU")
echo "$pt_out" | jq empty >/dev/null

hen=$(echo "$pt_out" | jq -r '.hookSpecificOutput.hookEventName // empty')
pd=$(echo "$pt_out" | jq -r '.hookSpecificOutput.permissionDecision // empty')

if [ "$hen" != "PreToolUse" ] || [ "$pd" != "allow" ]; then
    echo "claude-hook-smoke: PreToolUse unexpected JSON: $pt_out" >&2
    exit 1
fi

echo "claude-hook-smoke: PreToolUse ok (hookEventName=$hen permissionDecision=$pd)"
echo "claude-hook-smoke: ok"
