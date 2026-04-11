#!/bin/bash
# Guardian beforeReadFile — advisory only (permission always allow).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

input=$(read_hook_input)
if ! printf '%s' "$input" | jq empty 2>/dev/null; then
    json_output "$(jq -n '{permission: "allow"}')"
fi

policy_file=$(guardian_hook_policy_file)
if [ -z "${policy_file:-}" ]; then
    json_output "$(jq -n '{permission: "allow"}')"
fi

if ! jq empty "$policy_file" 2>/dev/null; then
    json_output "$(jq -n '{permission: "allow"}')"
fi

br=$(jq -r '.cursorignore_policy.before_read_enabled // true' "$policy_file" 2>/dev/null || echo "false")
if [ "$br" != "true" ]; then
    json_output "$(jq -n '{permission: "allow"}')"
fi

file_path=$(printf '%s' "$input" | jq -r '.file_path // empty' 2>/dev/null || echo "")
if [ -z "$file_path" ]; then
    json_output "$(jq -n '{permission: "allow"}')"
fi

if ! command -v python3 &>/dev/null || [ ! -f "$SCRIPT_DIR/cursorignore_check.py" ]; then
    json_output "$(jq -n '{permission: "allow"}')"
fi

args=(python3 "$SCRIPT_DIR/cursorignore_check.py" --file "$file_path" --checklist "$SCRIPT_DIR/cursorignore-checklist.json")
while IFS= read -r root; do
    [ -n "$root" ] || continue
    args+=(--workspace-root "$root")
done < <(guardian_hook_workspace_roots "$input")

chk=$("${args[@]}" 2>/dev/null || echo '{"match":false}')
if ! echo "$chk" | jq -e '.match == true' &>/dev/null; then
    json_output "$(jq -n '{permission: "allow"}')"
fi

path_key=$(echo "$file_path" | sed 's/[^a-zA-Z0-9_/.-]/_/g')
if ! guardian_cursorignore_should_warn "$path_key" "$policy_file"; then
    json_output "$(jq -n '{permission: "allow"}')"
fi

seg=$(echo "$chk" | jq -r '.segment // "path"' 2>/dev/null || echo "path")
rat=$(echo "$chk" | jq -r '.rationale // ""' 2>/dev/null || echo "")
rel=$(echo "$chk" | jq -r '.relative_path // ""' 2>/dev/null || echo "")
um="[Guardian] Reading path with segment '${seg}' (${rel}) — ${rat} Prefer adding to .cursorignore or an exception in .guardian/cursorignore-allow (see hooks/resources.md)."
am="[Guardian] This path is usually ignored for indexing; confirm you need it in context."

json_output "$(jq -n --arg um "$um" --arg am "$am" '{permission: "allow", user_message: $um, agent_message: $am}')"
