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

# Which agent this is and where it is working, so its sessions can be told apart and
# followed (Cursor names its folders in workspace_roots; the others send cwd).
client=$(guardian_hook_client)
session_cwd=$(sanitize_sql "$(echo "$input" | jq -r '(.cwd // (.workspace_roots | if type == "array" then .[0] else null end) // "") | tostring')")

pressure=$(read_state_pressure)

if [ -n "$conversation_id" ]; then
    cwd_update=""
    if [ -n "$session_cwd" ]; then
        cwd_update=", cwd = '$session_cwd'"
    fi
    # A session can start more than once (resume, clear, compact): the row is made the
    # first time and brought back to life after that. Starting, it is idle whatever it was:
    # a session killed in the middle of a turn and resumed is not still working.
    db_exec "INSERT OR IGNORE INTO sessions (conversation_id, model, pressure_at_start, client, state) VALUES ('$conversation_id', '$model', '$pressure', '$client', 'idle');
UPDATE sessions SET client = COALESCE(client, '$client'), last_activity_at = datetime('now'), state = 'idle', model = CASE WHEN model IN ('', 'unknown') THEN '$model' ELSE model END$cwd_update WHERE conversation_id = '$conversation_id';
DELETE FROM tool_calls WHERE recorded_at < datetime('now', '-60 days');
DELETE FROM pressure_samples WHERE sampled_at < datetime('now', '-60 days');"
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
        if guardian_blocking_enabled; then
            context="${banner} Load critical: CPU ${cpu}%, ${mem} GB free, swap ${swap}%.${why} Ease up; sends may block. Work serially — no subagents, no heavy containers. Snooze: ${snooze_hint}."
        else
            context="${banner} Load critical: CPU ${cpu}%, ${mem} GB free, swap ${swap}%.${why} Ease up. Work serially — no subagents, no heavy containers. Guardian only warns; it will not pause anything."
        fi
        ;;
    strained)
        context="${banner} Load strained: CPU ${cpu}%, ${mem} GB free, swap ${swap}%.${why} Prefer serial work; at most one subagent at a time."
        if [ "$block_on_hint" = "critical" ] && guardian_blocking_enabled; then
            context="${context} Gates block at critical — snooze early: ${snooze_hint}."
        fi
        ;;
    *)
        context="${banner} Load OK: CPU ${cpu}%, ${mem} GB free."
        ;;
esac

case "$deny_subagents_on" in
    strained|critical)
        if guardian_blocking_enabled; then
            context="${context} Subagent spawns are denied at ${deny_subagents_on} load."
        else
            context="${context} Avoid subagents at ${deny_subagents_on} load (Guardian will warn, not stop them)."
        fi
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
        context="${context} Disk critical: ~${disk_used}% used (~${disk_avail} GB free on ${disk_vol}) — free space soon; ${G_home}/guardian disk lists quick targets."
        ;;
esac

# Overrides in force, so a forgotten snooze or allow-once is never silent.
snooze_left=$(guardian_snooze_remaining_secs)
if [ "${snooze_left:-0}" -gt 0 ] 2>/dev/null; then
    context="${context} Snoozed for about $(( (snooze_left + 59) / 60 )) more min: pressure gates and subagent limits are off until then (end it: ${G_home}/guardian clear-snooze)."
fi
if guardian_proceed_once_armed && guardian_blocking_enabled; then
    context="${context} Allow-once is armed: the next prompt Guardian would pause goes through."
fi

# A config.toml the daemon could not apply as written must not go unnoticed.
config_problems=$(read_state_config_problems)
if [ -n "$config_problems" ]; then
    context="${context} Config problem: ${config_problems}. Run: ${G_home}/guardian config check"
fi

queue_file="${GUARDIAN_DIR:-$HOME/.guardian}/agent_queue.jsonl"
if [ -f "$queue_file" ]; then
    qc=$(grep -c '^{' "$queue_file" 2>/dev/null || echo 0)
    if [ "${qc:-0}" -gt 0 ] 2>/dev/null; then
        context="${context} Queue: ${qc} job(s) in ~/.guardian/agent_queue.jsonl (guardian-queue.sh list)."
    fi
fi

emit_context "$context"
