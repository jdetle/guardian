#!/bin/bash
# Shared prompt-gate evaluation for Cursor (beforeSubmitPrompt) and Codex (UserPromptSubmit).
# Source after lib.sh. Call: guardian_prompt_gate_eval "$input_json"
# Guardian slash commands (/guardian-snooze, /guardian-once) and guardian-install / guardian-upgrade
# skill paths never block — users must always reach snooze/install flows under load.
# Sets PG_OUTCOME (pass, pass_msg, block) and PG_MESSAGE.

# Match explicit requests for broad local work. Anchoring at the start keeps
# questions, quoted commands and incidental mentions out of the blocking path.
guardian_prompt_heavy_work() {
    local prompt containers tests build agents
    prompt=$(printf '%s' "$1" | jq -r '
        [ .prompt, .promptText, .text, .message, .input, .content ]
        | map(select(type == "string" and test("\\S")))
        | (first // "")[:1024] | ascii_downcase
        | sub("^\\s*(please\\s+)?"; "")
    ' 2>/dev/null) || return 0
    containers='^(rebuild (all (the )?)?(docker |compose )?containers|(run )?docker( compose)? build|(run )?docker compose up --build)([^[:alnum:]_-]|$)'
    tests='^run (the )?((full|entire|whole) (test suite|regression suite)|all (the )?(tests|integration tests|end-to-end tests))([^[:alnum:]_-]|$)'
    build='^(build|rebuild|compile) (the )?(entire|whole|full) (workspace|project|monorepo|repository)([^[:alnum:]_-]|$)'
    agents='^(launch|spawn|start|run) ((multiple|several|[2-9][0-9]*|1[0-9]+) (parallel )?subagents|subagents in parallel)([^[:alnum:]_-]|$)'
    local remote='(^|[[:space:]])(in ci|remotely|on github actions)([^[:alnum:]_-]|$)'
    if [[ "$prompt" =~ $remote ]]; then
        return 0
    fi
    case "$prompt" in
        *\?|*" without running"*|*" do not run"*|*" don't run"*|*"dry run"*|*"dry-run"*) return 0 ;;
    esac
    if [[ "$prompt" =~ $containers ]]; then
        printf '%s' 'container builds'
    elif [[ "$prompt" =~ $tests ]]; then
        printf '%s' 'a full test run'
    elif [[ "$prompt" =~ $build ]]; then
        printf '%s' 'a full project build'
    elif [[ "$prompt" =~ $agents ]]; then
        printf '%s' 'multiple subagents'
    fi
    return 0
}

guardian_prompt_gate_eval() {
    local input="$1"
    PG_OUTCOME="pass"
    PG_MESSAGE=""

    if guardian_snooze_active; then
        return 0
    fi

    local policy_file policy
    policy_file=$(guardian_hook_policy_file)
    [ -n "$policy_file" ] || return 0

    if ! policy=$(jq -ce 'select(type == "object")' "$policy_file" 2>/dev/null); then
        PG_OUTCOME="pass_msg"
        PG_MESSAGE="Guardian: hook_policy.json is invalid. Prompt gate skipped."
        return 0
    fi

    local mode block_on block_sess max_rss
    if ! IFS=$'\t' read -r mode block_on block_sess max_rss < <(
        printf '%s' "$policy" | jq -r --arg client "${GUARDIAN_HOOK_FORMAT:-cursor}" '
            (.prompt_gate // {}) as $gate
            | select($gate.enabled != false)
            | [($gate.clients[$client] // $gate.mode // "adaptive"),
               ($gate.block_on // "critical"),
               ($gate.block_on_session_budget == true),
               (.session_budget.max_cursor_rss_megabytes // 8192)] | @tsv
        ' 2>/dev/null
    ); then
        return 0
    fi

    local prompt_bundle
    prompt_bundle=$(guardian_hook_prompt_text_bundle "$input")
    if guardian_hook_is_guardian_meta_prompt "$prompt_bundle"; then
        return 0
    fi

    local sample
    [ -f "$STATE_FILE" ] && [ ! -L "$STATE_FILE" ] || return 0
    sample=$(cat "$STATE_FILE" 2>/dev/null) || return 0
    if ! is_daemon_active "$sample"; then
        return 0
    fi

    local pressure memory_pressure thermal cursor_mb
    if ! IFS=$'\t' read -r pressure memory_pressure thermal cursor_mb < <(
        printf '%s' "$sample" | jq -r '
            [(.pressure // "clear"), (.memory_pressure_level // "unknown"),
             (.thermal_state // "unknown"), (.cursor.resident_memory_megabytes // 0)] | @tsv
        ' 2>/dev/null
    ); then
        return 0
    fi

    local block_reason="" workload="" advisory=""
    case "$mode" in
        adaptive|advisory)
            if [ "$pressure" = "critical" ] && [ "$block_on" != "never" ]; then
                workload=$(guardian_prompt_heavy_work "$input")
                if [ -n "$workload" ]; then
                    if [ "$mode" = "advisory" ]; then
                        advisory="Guardian: Critical system pressure. Consider postponing $workload."
                    elif [ "$memory_pressure" = "critical" ] || [[ "$thermal" = serious || "$thermal" = critical ]]; then
                        block_reason="pressure"
                    fi
                fi
            fi
            ;;
        strict)
            local cursor_rss="${cursor_mb%%.*}" rss_limit="${max_rss%%.*}"
            if [ "$block_sess" = "true" ] && [[ "$pressure" = strained || "$pressure" = critical ]] \
                && [ "$rss_limit" -gt 0 ] 2>/dev/null && [ "$cursor_rss" -gt "$rss_limit" ] 2>/dev/null; then
                block_reason="session_budget"
            elif [[ "$block_on" = critical || "$block_on" = strained ]] && [ "$pressure" = critical ]; then
                block_reason="pressure"
            elif [ "$block_on" = strained ] && [ "$pressure" = strained ]; then
                block_reason="pressure"
            fi
            ;;
    esac

    if [ -n "$block_reason" ] && guardian_consume_proceed_once; then
        return 0
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

    if [ -n "$block_reason" ]; then
        local msg
        if [ "$block_reason" = "session_budget" ]; then
            msg="Guardian paused this prompt. Cursor uses ${cursor_mb} MB of RAM (cap ${max_rss} MB) under ${pressure} system pressure."
        else
            msg="Guardian paused this prompt under ${pressure} system pressure.$(read_state_reasons_suffix "$sample")"
            if [ -n "$workload" ]; then
                msg="$msg This request appears to start $workload."
            fi
        fi
        msg="$msg"$'\n'"$(guardian_resume_hint_text)"
        if [ -n "$attach_notes" ]; then
            msg="$msg"$'\n\n'"$attach_notes"
        fi
        local qbin qid
        if printf '%s' "$policy" | jq -e '.queue.enqueue_on_blocked_submit == true' >/dev/null 2>&1 \
            && qbin=$(guardian_queue_cli); then
            qid=$(printf '%s' "$input" | "$qbin" enqueue-blocked-json 2>/dev/null || true)
            if [ -n "$qid" ]; then
                msg="$msg"$'\n'"Queued as #${qid}. Run ~/.guardian/guardian-queue.sh list to view it."
            fi
        fi
        PG_OUTCOME="block"
        PG_MESSAGE="$msg"
    elif [ -n "$attach_notes$advisory" ]; then
        PG_OUTCOME="pass_msg"
        PG_MESSAGE="$attach_notes$advisory"
    fi
    return 0
}
