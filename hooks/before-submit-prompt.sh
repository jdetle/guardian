#!/bin/bash
# Guardian beforeSubmitPrompt — may return continue:false for pressure or session budget.
# Overrides: ~/.guardian/snooze_until (ISO time), ~/.guardian/proceed_once (one-shot).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

input=$(read_hook_input)
if ! printf '%s' "$input" | jq empty 2>/dev/null; then
    input="{}"
fi

if guardian_snooze_active; then
    json_output "$(jq -n '{continue: true}')"
fi

if guardian_consume_proceed_once; then
    json_output "$(jq -n --arg m "[Guardian] proceed_once consumed — submit allowed." '{continue: true, user_message: $m}')"
fi

policy_file=$(guardian_hook_policy_file)
if [ -z "${policy_file:-}" ]; then
    json_output "$(jq -n '{continue: true}')"
fi

if ! jq empty "$policy_file" 2>/dev/null; then
    json_output "$(jq -n '{continue: true, user_message: "[Guardian] hook_policy.json is not valid JSON; skipping prompt gate."}')"
fi

enabled="true"
e=$(jq -r '.prompt_gate.enabled // true' "$policy_file" 2>/dev/null) && enabled="$e"
if [ "$enabled" != "true" ]; then
    json_output "$(jq -n '{continue: true}')"
fi

# Fail-open when daemon not updating state
if ! is_daemon_active; then
    json_output "$(jq -n '{continue: true, user_message: "[Guardian] Prompt gate skipped (daemon stale or not running)."}')"
fi

pressure=$(read_state_pressure)
mem=$(read_state_field "memory_available_gb" "?")
cpu=$(read_state_field "cpu_percent" "?")
swap=$(read_state_field "swap_used_percent" "?")
cursor_mb=$(read_state_field "cursor.resident_memory_megabytes" "0")

block_on=$(jq -r '.prompt_gate.block_on // "critical"' "$policy_file" 2>/dev/null || echo "never")
case "$block_on" in
    never|strained|critical) ;;
    *) block_on="never" ;; # unknown / typo => fail-open (do not block on pressure)
esac

block_sess="true"
b=$(jq -r '.prompt_gate.block_on_session_budget // true' "$policy_file" 2>/dev/null) && block_sess="$b"
max_rss="8192"
m=$(jq -r '.session_budget.max_cursor_rss_megabytes // 8192' "$policy_file" 2>/dev/null) && max_rss="$m"

hints=$(guardian_resume_hint_text)

blocked=0
block_reason=""

# RSS gate: fail-open when RSS is 0 (unmeasured) or max is 0 (disabled)
if [ "$block_sess" = "true" ]; then
    cm="${cursor_mb%%.*}"
    mx="${max_rss%%.*}"
    if [ "${mx:-0}" -gt 0 ] 2>/dev/null && [ "${cm:-0}" -gt 0 ] 2>/dev/null && [ "${cm:-0}" -gt "${mx:-0}" ] 2>/dev/null; then
        blocked=1
        block_reason="session_budget"
    fi
fi

if [ "$blocked" -eq 0 ]; then
    case "$block_on" in
        never) ;;
        critical)
            if [ "$pressure" = "critical" ]; then
                blocked=1
                block_reason="pressure"
            fi
            ;;
        strained)
            if [ "$pressure" = "critical" ] || [ "$pressure" = "strained" ]; then
                blocked=1
                block_reason="pressure"
            fi
            ;;
    esac
fi

attach_notes=""
if command -v python3 &>/dev/null && [ -f "$SCRIPT_DIR/cursorignore_check.py" ]; then
    while IFS= read -r fp; do
        [ -n "$fp" ] || continue
        args=(python3 "$SCRIPT_DIR/cursorignore_check.py" --file "$fp" --checklist "$SCRIPT_DIR/cursorignore-checklist.json")
        while IFS= read -r root; do
            [ -n "$root" ] || continue
            args+=(--workspace-root "$root")
        done < <(guardian_hook_workspace_roots "$input")
        chk=$("${args[@]}" 2>/dev/null || echo '{"match":false}')
        if echo "$chk" | jq -e '.match == true' &>/dev/null; then
            seg=$(echo "$chk" | jq -r '.segment // "path"' 2>/dev/null || echo "?")
            rat=$(echo "$chk" | jq -r '.rationale // ""' 2>/dev/null || echo "")
            attach_notes="${attach_notes}[Guardian] Attachment touches '${seg}': ${rat} Add to .cursorignore or .guardian/cursorignore-allow — see hooks/resources.md"$'\n'
        fi
    done < <(guardian_hook_attachment_paths "$input")
fi

if [ "$blocked" -eq 1 ]; then
    msg=""
    case "$block_reason" in
        session_budget)
            msg="[Guardian] Cursor aggregate memory is high (~${cursor_mb} MB RSS, configured max ${max_rss} MB). Reduce Cursor load or close heavy work before adding more. ${hints}"
            ;;
        pressure)
            msg="[Guardian] System pressure: ${pressure} (CPU ${cpu}%, memory ${mem} GB free, swap ${swap}%). Wait for clear/strained or use override. ${hints}"
            ;;
        *)
            msg="[Guardian] Submit blocked by policy. ${hints}"
            ;;
    esac
    if [ -n "$attach_notes" ]; then
        msg="${msg}"$'\n\n'"${attach_notes}"
    fi
    json_output "$(jq -n --arg msg "$msg" '{continue: false, user_message: $msg}')"
fi

if [ -n "$attach_notes" ]; then
    json_output "$(jq -n --arg msg "$attach_notes" '{continue: true, user_message: $msg}')"
fi

json_output "$(jq -n '{continue: true}')"
