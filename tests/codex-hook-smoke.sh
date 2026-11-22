#!/bin/bash
# Minimal smoke test: Codex UserPromptSubmit adapter emits valid JSON.
# Run from repo root: bash tests/codex-hook-smoke.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UPS="$ROOT/hooks/codex/user-prompt-submit.sh"

if [ ! -x "$UPS" ] && [ -f "$UPS" ]; then
    chmod +x "$UPS"
fi

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
        echo "codex-hook-smoke: ok (shape=$kind)"
        ;;
    *)
        echo "codex-hook-smoke: unexpected JSON: $out" >&2
        exit 1
        ;;
esac
