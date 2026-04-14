#!/bin/bash
# Minimal smoke test: Codex UserPromptSubmit adapter emits valid JSON.
# Run from repo root: bash tests/codex-hook-smoke.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UPS="$ROOT/hooks/codex/user-prompt-submit.sh"

if [ ! -x "$UPS" ] && [ -f "$UPS" ]; then
    chmod +x "$UPS"
fi

out=$(printf '%s' '{"prompt":"smoke","cwd":"/tmp","session_id":"test"}' | bash "$UPS")
echo "$out" | jq empty >/dev/null

kind=$(echo "$out" | jq -r 'if .decision then "block" elif .hookSpecificOutput then "advisory" elif .["continue"] == true then "allow" else "other" end')

case "$kind" in
    allow|advisory|block)
        echo "codex-hook-smoke: ok (shape=$kind)"
        ;;
    *)
        echo "codex-hook-smoke: unexpected JSON: $out" >&2
        exit 1
        ;;
esac
