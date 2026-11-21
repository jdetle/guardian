#!/bin/bash
# Guardian warns; it stops nothing until the person has opted in (`guardian blocking on`,
# which makes ~/.guardian/blocking_on). This checks both halves for Cursor, Claude Code and
# Codex: without the opt-in no hook pauses a prompt or denies a tool call, however the gates
# are configured and however bad the load; with it, they do what the configuration says.
#
# Safe to run anywhere: it makes a scratch HOME of its own and every hook reads and writes
# only there. CI runs it on macOS under /bin/bash 3.2 and inside the hook container.
#
#   bash tests/stress/blocking-opt-in-test.sh
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
if [ -z "${HOOKS_DIR:-}" ] || [ ! -f "${HOOKS_DIR:-}/prompt-gate-common.sh" ]; then
    HOOKS_DIR="$(cd "$HERE/../../hooks" 2>/dev/null && pwd)"
fi
if [ ! -f "$HOOKS_DIR/prompt-gate-common.sh" ]; then
    echo "blocking-opt-in-test: hooks not found (HOOKS_DIR=$HOOKS_DIR)" >&2
    exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
    echo "blocking-opt-in-test: jq is required" >&2
    exit 2
fi

SCRATCH="$(mktemp -d)"
if [ -z "$SCRATCH" ] || [ ! -d "$SCRATCH" ]; then
    echo "could not make a scratch directory" >&2
    exit 2
fi
trap 'rm -rf "$SCRATCH"' EXIT
export HOME="$SCRATCH"
unset GUARDIAN_HOOK_FORMAT CLAUDE_CONFIG_DIR CODEX_HOME GUARDIAN_ADVISE_INTERVAL_SECS
G="$HOME/.guardian"

PASSED=0
FAILED=0
pass() { PASSED=$((PASSED + 1)); echo "  PASS: $1"; }
fail() { FAILED=$((FAILED + 1)); echo "  FAIL: $1"; }
check() {
    if [ "$2" = "$3" ]; then
        pass "$1"
    else
        fail "$1 (expected [$2], got [$3])"
    fi
}
contains() {
    case "$3" in
        *"$2"*) pass "$1" ;;
        *) fail "$1 (no [$2] in [$3])" ;;
    esac
}

# setup <pressure> <jq edit of the default policy>: a fresh home, load as given, corroborated
# by the kernel so that even the cautious default mode would pause.
setup() {
    rm -rf "$G"
    mkdir -p "$G"
    jq "$2" "$HOOKS_DIR/hook_policy.default.json" > "$G/hook_policy.json"
    local stamp
    stamp=$(date -u +%Y-%m-%dT%H:%M:%S)
    printf '{"pressure":"%s","pressure_reasons":["test"],"memory_pressure_level":"critical","thermal_state":"nominal","cpu_percent":97,"memory_available_gb":0.3,"swap_used_percent":90,"cursor":{"resident_memory_megabytes":9000},"sampled_at":"%s.000000+00:00"}\n' \
        "$1" "$stamp" > "$G/state.json"
}
HOOK_OUT=""
HOOK_RC=0
hook() {
    HOOK_OUT=$(printf '%s' "$2" | bash "$HOOKS_DIR/$1" 2>/dev/null)
    HOOK_RC=$?
}
# What a prompt hook decided: block | warn | allow | other
prompt_verdict() {
    printf '%s' "$1" | jq -r '
        if (.decision? == "block") or (.["continue"]? == false) then "block"
        elif (.user_message? // .hookSpecificOutput.additionalContext? // "") != "" then "warn"
        elif .["continue"]? == true then "allow"
        else "other" end' 2>/dev/null
}
prompt_text() { printf '%s' "$1" | jq -r '.user_message // .hookSpecificOutput.additionalContext // .reason // ""' 2>/dev/null; }
# What a tool hook decided: deny | allow-with-warning | silent | other
tool_verdict() {
    if [ -z "$1" ]; then
        echo "silent"
        return
    fi
    printf '%s' "$1" | jq -r '
        if (.permission? == "deny") or (.hookSpecificOutput.permissionDecision? == "deny") then "deny"
        elif (.hookSpecificOutput.permissionDecision? == "allow") then "auto-allow"
        elif (.agent_message? // .hookSpecificOutput.additionalContext? // "") != "" then "warn"
        elif .permission? == "allow" then "allow"
        else "other" end' 2>/dev/null
}

HEAVY='run the full test suite'
STRICT='.prompt_gate.mode = "strict" | .prompt_gate.block_on = "strained" | .prompt_gate.block_on_session_budget = true | .tool_gate.deny_subagents_on = "strained"'

# ─── Not opted in: nothing is stopped, whatever the settings ────────────
echo "=== Without the opt-in, no hook stops anything ==="
for pressure in strained critical; do
    setup "$pressure" "$STRICT"
    hook before-submit-prompt.sh "{\"conversation_id\":\"a\",\"prompt\":\"$HEAVY\"}"
    check "Cursor prompt, strictest settings, $pressure load: sent with a warning" "0|warn" "$HOOK_RC|$(prompt_verdict "$HOOK_OUT")"
    check "Cursor: continue is true" "true" "$(printf '%s' "$HOOK_OUT" | jq -r '.continue')"
    for agent in claude codex; do
        hook "$agent/user-prompt-submit.sh" "{\"session_id\":\"a-$agent\",\"prompt\":\"$HEAVY\"}"
        check "$agent prompt, strictest settings, $pressure load: sent with a warning" "0|warn" "$HOOK_RC|$(prompt_verdict "$HOOK_OUT")"
        for tool in Agent Task spawn_agent; do
            hook "$agent/pre-tool-use.sh" "{\"session_id\":\"t-$agent-$tool-$pressure\",\"tool_name\":\"$tool\"}"
            check "$agent $tool, deny configured, $pressure load: allowed with a warning, no decision" "warn" "$(tool_verdict "$HOOK_OUT")"
        done
    done
    hook pre-tool-use.sh '{"tool_name":"Task"}'
    check "Cursor Task, deny configured, $pressure load: allowed with a warning" "allow|warn" "$(printf '%s' "$HOOK_OUT" | jq -r '.permission')|$(tool_verdict "$HOOK_OUT")"
done

setup critical "$STRICT"
hook claude/user-prompt-submit.sh "{\"session_id\":\"w\",\"prompt\":\"$HEAVY\"}"
contains "the warning says what is wrong" "critical system pressure" "$(prompt_text "$HOOK_OUT")"
contains "the warning says the prompt went through" "sent anyway" "$(prompt_text "$HOOK_OUT")"
for agent in claude codex; do
    hook "$agent/user-prompt-submit.sh" "{\"session_id\":\"w-$agent\",\"prompt\":\"$HEAVY\"}"
    case "$HOOK_OUT" in
        *"blocking on"*) fail "$agent: the warning hands the agent the command that turns pausing on" ;;
        *) pass "$agent: the warning does not tell the agent how to turn pausing on" ;;
    esac
    contains "$agent: the warning says pausing is the user's choice" "user's choice" "$(prompt_text "$HOOK_OUT")"
done
hook before-submit-prompt.sh "{\"conversation_id\":\"w-cursor\",\"prompt\":\"$HEAVY\"}"
contains "Cursor, whose message the person reads: the warning says how to opt in" "guardian blocking on" "$(prompt_text "$HOOK_OUT")"
: > "$G/proceed_once"
hook claude/session-start.sh '{"session_id":"ws","source":"startup"}'
case "$HOOK_OUT" in
    *"Allow-once is armed"*) fail "session start mentions allow-once while nothing can be paused" ;;
    *) pass "session start does not mention allow-once while nothing can be paused" ;;
esac
case "$HOOK_OUT" in
    *"blocking on"*) fail "session start hands the agent the command that turns pausing on" ;;
    *) pass "session start does not tell the agent how to turn pausing on" ;;
esac
rm -f "$G/proceed_once"
hook claude/pre-tool-use.sh '{"session_id":"w","tool_name":"Agent"}'
contains "the subagent warning asks for serial work" "serially" "$(printf '%s' "$HOOK_OUT" | jq -r '.hookSpecificOutput.additionalContext')"

setup critical '.'
hook claude/user-prompt-submit.sh "{\"session_id\":\"d\",\"prompt\":\"$HEAVY\"}"
check "default settings, critical load, a heavy request: sent with a warning" "warn" "$(prompt_verdict "$HOOK_OUT")"
hook claude/user-prompt-submit.sh '{"session_id":"d","prompt":"what does this function do?"}'
check "default settings, an ordinary prompt: allowed without comment" "allow" "$(prompt_verdict "$HOOK_OUT")"

setup critical "$STRICT"
: > "$G/proceed_once"
hook claude/user-prompt-submit.sh "{\"session_id\":\"o\",\"prompt\":\"$HEAVY\"}"
if [ -f "$G/proceed_once" ]; then
    pass "allow-once is not used up by a prompt that was never going to be paused"
else
    fail "allow-once was consumed while blocking is off"
fi

setup critical "$STRICT"
ln -s /dev/null "$G/blocking_on"
hook claude/user-prompt-submit.sh "{\"session_id\":\"l\",\"prompt\":\"$HEAVY\"}"
check "a symlink in place of the opt-in file is not an opt-in" "warn" "$(prompt_verdict "$HOOK_OUT")"

setup critical "$STRICT"
hook claude/session-start.sh '{"session_id":"s","source":"startup"}'
context=$(printf '%s' "$HOOK_OUT" | jq -r '.hookSpecificOutput.additionalContext')
contains "session start says Guardian only warns" "only warns" "$context"
case "$context" in
    *"may block"*|*"Gates block"*|*"are denied"*) fail "session start still talks of blocking while it is off: $context" ;;
    *) pass "session start does not promise blocks that will not happen" ;;
esac

# ─── Opted in: the gates do what the settings say ───────────────────────
echo "=== With the opt-in, the settings decide ==="
setup critical "$STRICT"
: > "$G/blocking_on"
hook before-submit-prompt.sh "{\"conversation_id\":\"b\",\"prompt\":\"$HEAVY\"}"
check "Cursor prompt, strict, critical load: paused" "block" "$(prompt_verdict "$HOOK_OUT")"
for agent in claude codex; do
    hook "$agent/user-prompt-submit.sh" "{\"session_id\":\"b-$agent\",\"prompt\":\"$HEAVY\"}"
    check "$agent prompt, strict, critical load: paused" "block" "$(prompt_verdict "$HOOK_OUT")"
    hook "$agent/pre-tool-use.sh" "{\"session_id\":\"bt-$agent\",\"tool_name\":\"Agent\"}"
    check "$agent subagent, deny configured, critical load: denied" "deny" "$(tool_verdict "$HOOK_OUT")"
    hook "$agent/pre-tool-use.sh" "{\"session_id\":\"bb-$agent\",\"tool_name\":\"Bash\"}"
    check "$agent Bash is never denied, and never auto-allowed" "warn" "$(tool_verdict "$HOOK_OUT")"
done
hook pre-tool-use.sh '{"tool_name":"Task"}'
check "Cursor Task, deny configured, critical load: denied" "deny" "$(tool_verdict "$HOOK_OUT")"

setup critical '.'
: > "$G/blocking_on"
hook claude/user-prompt-submit.sh "{\"session_id\":\"e\",\"prompt\":\"$HEAVY\"}"
check "opted in, default settings, a heavy request at critical load: paused" "block" "$(prompt_verdict "$HOOK_OUT")"
hook claude/pre-tool-use.sh '{"session_id":"e","tool_name":"Agent"}'
check "opted in, default settings: subagents are still not denied (that is its own setting)" "warn" "$(tool_verdict "$HOOK_OUT")"
hook claude/user-prompt-submit.sh '{"session_id":"e2","prompt":"what does this function do?"}'
check "opted in, an ordinary prompt: allowed" "allow" "$(prompt_verdict "$HOOK_OUT")"

setup critical "$STRICT"
: > "$G/blocking_on"
: > "$G/proceed_once"
hook claude/user-prompt-submit.sh "{\"session_id\":\"p\",\"prompt\":\"$HEAVY\"}"
check "opted in, allow-once armed: the prompt goes through and the token is used" "allow|gone" "$(prompt_verdict "$HOOK_OUT")|$([ -f "$G/proceed_once" ] && echo there || echo gone)"

rm -f "$G/blocking_on"
hook claude/user-prompt-submit.sh "{\"session_id\":\"z\",\"prompt\":\"$HEAVY\"}"
check "opting out again takes effect on the next prompt" "warn" "$(prompt_verdict "$HOOK_OUT")"

echo ""
echo "blocking-opt-in-test: $PASSED passed, $FAILED failed"
[ "$FAILED" -eq 0 ]
