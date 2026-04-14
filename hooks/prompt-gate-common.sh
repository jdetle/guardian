#!/bin/bash
# Shared prompt-gate evaluation for Cursor (beforeSubmitPrompt) and Codex (UserPromptSubmit).
# Source after lib.sh. Call: guardian_prompt_gate_eval "$input_json"
# Sets:
#   PG_OUTCOME=pass|pass_msg|block
#   PG_MESSAGE=""           # primary user-facing string (block reason or advisory)
#   PG_ATTACH_ONLY=false    # if true and outcome pass_msg, message is attachment advisory only

guardian_prompt_gate_eval() {
    local input="$1"
    PG_OUTCOME="pass"
    PG_MESSAGE=""
    PG_ATTACH_ONLY=false

    if guardian_snooze_active; then
        return 0
    fi

    if guardian_consume_proceed_once; then
        PG_OUTCOME="pass_msg"
        PG_MESSAGE="Guardian: One-shot bypass applied — send again."
        return 0
    fi

    local policy_file
    policy_file=$(guardian_hook_policy_file)
    if [ -z "${policy_file:-}" ]; then
        return 0
    fi

    if ! jq empty "$policy_file" 2>/dev/null; then
        PG_OUTCOME="pass_msg"
        PG_MESSAGE="Guardian: hook_policy.json not valid JSON — prompt gate off."
        return 0
    fi

    local enabled="true"
    local e
    e=$(jq -r '.prompt_gate.enabled // true' "$policy_file" 2>/dev/null) && enabled="$e"
    if [ "$enabled" != "true" ]; then
        return 0
    fi

    if ! is_daemon_active; then
        PG_OUTCOME="pass_msg"
        PG_MESSAGE="Guardian: No fresh metrics (guardiand quiet?) — gate off."
        return 0
    fi

    local pressure mem cpu swap cursor_mb block_on block_sess max_rss hints
    pressure=$(read_state_pressure)
    mem=$(read_state_field "memory_available_gb" "?")
    cpu=$(read_state_field "cpu_percent" "?")
    swap=$(read_state_field "swap_used_percent" "?")
    cursor_mb=$(read_state_field "cursor.resident_memory_megabytes" "0")

    block_on=$(jq -r '.prompt_gate.block_on // "critical"' "$policy_file" 2>/dev/null || echo "never")
    case "$block_on" in
        never|strained|critical) ;;
        *) block_on="never" ;;
    esac

    block_sess="true"
    local b
    b=$(jq -r '.prompt_gate.block_on_session_budget // false' "$policy_file" 2>/dev/null) && block_sess="$b"
    max_rss="8192"
    local m
    m=$(jq -r '.session_budget.max_cursor_rss_megabytes // 8192' "$policy_file" 2>/dev/null) && max_rss="$m"

    hints=$(guardian_resume_hint_text)

    local blocked=0
    local block_reason=""

    if [ "$block_sess" = "true" ]; then
        local cm mx
        cm="${cursor_mb%%.*}"
        mx="${max_rss%%.*}"
        if [ "$pressure" != "clear" ] 2>/dev/null \
            && [ "${mx:-0}" -gt 0 ] 2>/dev/null \
            && [ "${cm:-0}" -gt 0 ] 2>/dev/null \
            && [ "${cm:-0}" -gt "${mx:-0}" ] 2>/dev/null; then
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

    local attach_notes=""
    if command -v python3 &>/dev/null && [ -f "$SCRIPT_DIR/cursorignore_check.py" ]; then
        local fp
        while IFS= read -r fp; do
            [ -n "$fp" ] || continue
            local pg_args
            pg_args=(python3 "$SCRIPT_DIR/cursorignore_check.py" --file "$fp" --checklist "$SCRIPT_DIR/cursorignore-checklist.json")
            local root
            while IFS= read -r root; do
                [ -n "$root" ] || continue
                pg_args+=(--workspace-root "$root")
            done < <(guardian_hook_workspace_roots "$input")
            local chk
            chk=$("${pg_args[@]}" 2>/dev/null || echo '{"match":false}')
            if echo "$chk" | jq -e '.match == true' &>/dev/null; then
                local seg rat
                seg=$(echo "$chk" | jq -r '.segment // "path"' 2>/dev/null || echo "?")
                rat=$(echo "$chk" | jq -r '.rationale // ""' 2>/dev/null || echo "")
                attach_notes="${attach_notes}Guardian: Attachment hits ${seg} (${rat}). Add .cursorignore / .guardian/cursorignore-allow if needed."$'\n'
            fi
        done < <(guardian_hook_attachment_paths "$input")
    fi

    if [ "$blocked" -eq 1 ]; then
        local msg=""
        case "$block_reason" in
            session_budget)
                msg="Guardian: Blocked — Cursor RAM ${cursor_mb} MB (cap ${max_rss} MB) with ${pressure} load. Lighten work or snooze."$'\n'"${hints}"
                ;;
            pressure)
                msg="Guardian: Blocked — ${pressure} load (CPU ${cpu}%, ${mem} GB RAM free, swap ${swap}%). Cool down or snooze."$'\n'"${hints}"
                ;;
            *)
                msg="Guardian: Blocked by policy."$'\n'"${hints}"
                ;;
        esac
        if [ -n "$attach_notes" ]; then
            msg="${msg}"$'\n\n'"${attach_notes}"
        fi
        local q_enqueue qid
        q_enqueue=$(jq -r '.queue.enqueue_on_blocked_submit // false' "$policy_file" 2>/dev/null || echo "false")
        qid=""
        if [ "$q_enqueue" = "true" ]; then
            local qbin
            if qbin=$(guardian_queue_cli); then
                qid=$(printf '%s' "$input" | "$qbin" enqueue-blocked-json 2>/dev/null || true)
            fi
        fi
        if [ -n "$qid" ]; then
            msg="${msg}"$'\n'"Queued as #${qid} — ~/.guardian/guardian-queue.sh list"
        fi
        PG_OUTCOME="block"
        PG_MESSAGE="$msg"
        return 0
    fi

    local preempt
    preempt=$(guardian_preempt_snooze_hint "$pressure" "$block_on" "$block_sess" "$cursor_mb" "$max_rss")

    if [ -n "$attach_notes" ]; then
        PG_OUTCOME="pass_msg"
        PG_MESSAGE="$attach_notes"
        if [ -n "$preempt" ]; then
            PG_MESSAGE="${attach_notes}"$'\n'"${preempt}"
        fi
        PG_ATTACH_ONLY=true
        return 0
    fi

    if [ -n "$preempt" ]; then
        PG_OUTCOME="pass_msg"
        PG_MESSAGE="$preempt"
        return 0
    fi

    return 0
}
