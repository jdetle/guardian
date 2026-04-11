#!/bin/bash
# Offline demo: exercise before-submit / before-read hooks with fake state + policy.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HOOKS="$ROOT/hooks"
export HOME="${TMPDIR:-/tmp}/guardian-mock-$$"
export GUARDIAN_DIR="$HOME/.guardian"
mkdir -p "$GUARDIAN_DIR"

cp "$HOOKS/hook_policy.default.json" "$GUARDIAN_DIR/hook_policy.json"

write_state() {
    local pressure="$1"
    local sess="${2:-2}"
    local now
    now=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
    cat >"$GUARDIAN_DIR/state.json" <<EOF
{
  "pressure": "${pressure}",
  "cpu_percent": 95,
  "memory_available_gb": 0.4,
  "memory_total_gb": 16,
  "swap_used_percent": 70,
  "thermal_state": "nominal",
  "docker": {"running_containers": 0, "total_cpu_percent": 0.0, "total_memory_mb": 0},
  "cursor": {"active_sessions": ${sess}, "process_count": 10, "resident_memory_megabytes": 1200},
  "process_count": 100,
  "max_proc_per_uid": 4000,
  "sampled_at": "${now}"
}
EOF
}

echo "=== 1) beforeSubmitPrompt — should block (critical pressure) ==="
write_state critical 2
out=$(echo '{}' | bash "$HOOKS/before-submit-prompt.sh")
echo "$out" | jq .

echo ""
echo "=== 2) beforeSubmitPrompt — proceed_once ==="
touch "$GUARDIAN_DIR/proceed_once"
write_state critical 2
out=$(echo '{}' | bash "$HOOKS/before-submit-prompt.sh")
echo "$out" | jq .

echo ""
echo "=== 3) beforeReadFile — advisory on node_modules ==="
mkdir -p "$HOME/demo/ws/node_modules/pkg"
touch "$HOME/demo/ws/node_modules/pkg/x.js"
write_state clear 2
out=$(echo "{\"file_path\":\"$HOME/demo/ws/node_modules/pkg/x.js\",\"workspace_roots\":[\"$HOME/demo/ws\"]}" | bash "$HOOKS/before-read-file.sh")
echo "$out" | jq .

rm -rf "$HOME"
echo ""
echo "Demo complete (temp home removed)."
