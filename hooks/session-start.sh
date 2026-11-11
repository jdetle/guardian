#!/bin/bash
# Guardian sessionStart hook.
# Records session in DB and injects system pressure + session-budget context.
# GUARDIAN_HOOK_FORMAT=cursor (default) | claude | codex selects the output shape and wording.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib.sh"
if [ -f "$SCRIPT_DIR/tool-gate-common.sh" ]; then
    source "$SCRIPT_DIR/tool-gate-common.sh"
fi

HOOK_FORMAT="${GUARDIAN_HOOK_FORMAT:-cursor}"

emit_context() {
    local ctx="$1"
    case "$HOOK_FORMAT" in
        claude|codex)
            # Claude Code and Codex share this shape.
            json_output "$(jq -n \
                --arg ctx "$ctx" \
                '{
                    hookSpecificOutput: {
                        hookEventName: "SessionStart",
                        additionalContext: $ctx
                    }
                }')"
            ;;
        *)
            json_output "$(jq -n --arg ctx "$ctx" '{additional_context: $ctx}')"
            ;;
    esac
}

input=$(read_hook_input)
if ! printf '%s' "$input" | jq empty 2>/dev/null; then
    input="{}"
fi
ensure_db

# Cursor sends conversation_id; Claude Code and Codex send session_id.
conversation_id=$(sanitize_sql "$(echo "$input" | jq -r '.conversation_id // .session_id // "" | tostring')")
model=$(sanitize_sql "$(echo "$input" | jq -r '.model // "unknown" | tostring')")

pressure=$(read_state_pressure)

if [ -n "$conversation_id" ]; then
    db_exec "INSERT OR IGNORE INTO sessions (conversation_id, model, pressure_at_start) VALUES ('$conversation_id', '$model', '$pressure');"
fi

if declare -F guardian_advise_prune >/dev/null; then
    guardian_advise_prune
fi

G_home="${GUARDIAN_DIR:-$HOME/.guardian}"

# No fresh sample: the numbers in state.json are old, so do not present them as current.
if ! is_daemon_active; then
    emit_context "Guardian: Session on — no daemon (metrics stale). Resource gates are OFF until guardiand runs again. Check: launchctl list com.guardian.guardiand · reinstall from the Guardian repo: bash scripts/install.sh"
fi

banner="Guardian: Session on — metrics live."

cpu=$(read_state_field "cpu_percent" "0")
mem=$(read_state_field "memory_available_gb" "0")
swap=$(read_state_field "swap_used_percent" "0")
cursor_sess=$(read_state_field "cursor.active_sessions" "0")
cursor_mb=$(read_state_field "cursor.resident_memory_megabytes" "0")

policy_file=$(guardian_hook_policy_file)
warn_rss="4096"
block_on_hint="critical"
deny_subagents_on="never"
if [ -n "${policy_file:-}" ] && [ -f "$policy_file" ] && jq empty "$policy_file" 2>/dev/null; then
    warn_rss=$(jq -r '.session_budget.warn_cursor_rss_megabytes // 4096' "$policy_file" 2>/dev/null || echo "4096")
    block_on_hint=$(jq -r '.prompt_gate.block_on // "critical"' "$policy_file" 2>/dev/null || echo "critical")
    deny_subagents_on=$(jq -r '.tool_gate.deny_subagents_on // "never"' "$policy_file" 2>/dev/null || echo "never")
fi

case "$HOOK_FORMAT" in
    claude|codex) snooze_hint="${G_home}/guardian snooze 15" ;;
    *) snooze_hint="/guardian-snooze · ${G_home}/guardian snooze 15" ;;
esac

why=$(read_state_reasons_suffix)
context=""
case "$pressure" in
    critical)
        context="${banner} Load critical: CPU ${cpu}%, ${mem} GB free, swap ${swap}%.${why} Ease up; sends may block. Work serially — no subagents, no heavy containers. Snooze: ${snooze_hint}."
        ;;
    strained)
        context="${banner} Load strained: CPU ${cpu}%, ${mem} GB free, swap ${swap}%.${why} Prefer serial work; at most one subagent at a time."
        if [ "$block_on_hint" = "critical" ]; then
            context="${context} Gates block at critical — snooze early: ${snooze_hint}."
        fi
        ;;
    *)
        context="${banner} Load OK: CPU ${cpu}%, ${mem} GB free."
        ;;
esac

case "$deny_subagents_on" in
    strained|critical)
        context="${context} Subagent spawns are denied at ${deny_subagents_on} load."
        ;;
esac

case "$HOOK_FORMAT" in
    claude|codex)
        # Per-agent resident memory (guardiand >= 0.4.0); omitted when the daemon does not report it.
        agents_total=$(read_state_field "agents.total_mb" "0")
        at="${agents_total%%.*}"
        if [ "${at:-0}" -gt 0 ] 2>/dev/null; then
            claude_mb=$(read_state_field "agents.claude_mb" "0")
            codex_mb=$(read_state_field "agents.codex_mb" "0")
            agents_cursor_mb=$(read_state_field "agents.cursor_mb" "0")
            context="${context} · Agent RAM ≈ ${agents_total} MB (Claude ${claude_mb}, Codex ${codex_mb}, Cursor ${agents_cursor_mb})."
        fi
        ;;
    *)
        # Workspace folder count is diagnostic only (stale dirs accumulate); RSS reflects actual load.
        context="${context} · ~/.cursor/projects ≈ ${cursor_sess} folders · Cursor RSS ≈ ${cursor_mb} MB."
        cm="${cursor_mb%%.*}"
        wr="${warn_rss%%.*}"
        if [ "${wr:-0}" -gt 0 ] 2>/dev/null && [ "${cm:-0}" -gt 0 ] 2>/dev/null && [ "${cm:-0}" -gt "${wr:-0}" ] 2>/dev/null; then
            context="${context} High Cursor RAM (${cursor_mb} MB, warn ${warn_rss} MB) — trim threads if you can."
        fi
        ;;
esac

disk_level=$(read_state_field "disk.level" "clear")
disk_used=$(read_state_field "disk.used_percent" "0")
disk_avail=$(read_state_field "disk.available_gb" "0")
disk_vol=$(read_state_field "disk.volume_path" "")
case "$disk_level" in
    warn)
        context="${context} Disk tight: ~${disk_used}% used (~${disk_avail} GB free on ${disk_vol})."
        ;;
    critical)
        context="${context} Disk critical: ~${disk_used}% used (~${disk_avail} GB free on ${disk_vol}) — free space soon."
        ;;
esac

queue_file="${GUARDIAN_DIR:-$HOME/.guardian}/agent_queue.jsonl"
if [ -f "$queue_file" ]; then
    qc=$(grep -c '^{' "$queue_file" 2>/dev/null || echo 0)
    if [ "${qc:-0}" -gt 0 ] 2>/dev/null; then
        context="${context} Queue: ${qc} job(s) in ~/.guardian/agent_queue.jsonl (guardian-queue.sh list)."
    fi
fi

emit_context "$context"
