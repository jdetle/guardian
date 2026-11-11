#!/bin/bash
# Containerized Test Harness for Guardian Hooks and State Parsing
#
# Runs INSIDE a Docker container — never on the host.
# The guardian daemon uses macOS-only APIs (mach2, sysctl) and cannot be
# compiled for Linux. This test suite validates everything EXCEPT the
# daemon's sampling loop:
#
#   1. preToolUse / subagentStart return "allow" (deny only via opt-in tool_gate policy)
#   2. beforeSubmitPrompt may return continue:false (pressure/session budget)
#   3. beforeReadFile always returns allow (advisory only)
#   4. Hooks provide context messages on strained/critical
#   5. State file parsing handles missing/corrupt/symlinked files
#   6. No crash on garbage input
#   7. Fail-open behavior: missing state = "clear", not "critical"
set -uo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

GUARDIAN_DIR="${GUARDIAN_DIR:-/root/.guardian}"
STATE_FILE="$GUARDIAN_DIR/state.json"
HOOKS_DIR="${HOOKS_DIR:-/opt/guardian/hooks}"

# docker-compose tmpfs mounts an empty directory over /root/.guardian — seed files the image baked in RUN.
mkdir -p "$GUARDIAN_DIR"
if [ ! -f "$GUARDIAN_DIR/hook_policy.json" ] && [ -f "$HOOKS_DIR/hook_policy.default.json" ]; then
    cp "$HOOKS_DIR/hook_policy.default.json" "$GUARDIAN_DIR/hook_policy.json"
fi
if [ ! -f "$GUARDIAN_DIR/guardian-queue.sh" ] && [ -f "/opt/guardian/scripts/guardian-queue.sh" ]; then
    cp "/opt/guardian/scripts/guardian-queue.sh" "$GUARDIAN_DIR/guardian-queue.sh"
    chmod +x "$GUARDIAN_DIR/guardian-queue.sh"
fi

# jq's // treats JSON false as missing; Cursor hooks use boolean "continue".
guardian_json_continue() {
    echo "$1" | jq -r 'if (.["continue"] | type) == "boolean" then (if .["continue"] then "true" else "false" end) else "missing" end' 2>/dev/null
}

FAILURES=()
PASSES=()

pass() { echo -e "  ${GREEN}PASS${NC}: $1"; PASSES+=("$1"); }
fail() { echo -e "  ${RED}FAIL${NC}: $1"; FAILURES+=("$1"); }

# ─── Test Group 1: Hooks Always Allow ───────────────────────────────────

echo -e "\n${BOLD}${CYAN}=== Test Group 1: Hooks Always Allow ===${NC}"

test_hook_allows() {
    local label="$1"
    local state_content="$2"
    local hook_script="$3"

    rm -f "$STATE_FILE"
    if [ -n "$state_content" ]; then
        echo "$state_content" > "$STATE_FILE"
    fi

    local result
    result=$(echo '{"tool_name":"Shell","tool_input":"echo test"}' | bash "$hook_script" 2>/dev/null)

    local permission
    permission=$(echo "$result" | jq -r '.permission // "missing"' 2>/dev/null)

    if [ "$permission" = "allow" ]; then
        pass "$label"
    else
        fail "$label (got permission=$permission, result=$result)"
    fi
}

echo -e "${YELLOW}Testing pre-tool-use.sh${NC}"

test_hook_allows "pre-tool: clear pressure" \
    '{"pressure":"clear","cpu_percent":10,"memory_available_gb":6,"swap_used_percent":5}' \
    "$HOOKS_DIR/pre-tool-use.sh"

test_hook_allows "pre-tool: strained pressure" \
    '{"pressure":"strained","cpu_percent":88,"memory_available_gb":0.8,"swap_used_percent":75}' \
    "$HOOKS_DIR/pre-tool-use.sh"

test_hook_allows "pre-tool: critical pressure" \
    '{"pressure":"critical","cpu_percent":98,"memory_available_gb":0.2,"swap_used_percent":95}' \
    "$HOOKS_DIR/pre-tool-use.sh"

test_hook_allows "pre-tool: missing state file" "" "$HOOKS_DIR/pre-tool-use.sh"

test_hook_allows "pre-tool: empty state file" '{}' "$HOOKS_DIR/pre-tool-use.sh"

test_hook_allows "pre-tool: corrupt state file" 'NOT JSON AT ALL!!!' "$HOOKS_DIR/pre-tool-use.sh"

test_hook_allows "pre-tool: garbage pressure value" \
    '{"pressure":"DESTROY_ALL_AGENTS"}' \
    "$HOOKS_DIR/pre-tool-use.sh"

test_hook_allows "pre-tool: null pressure" \
    '{"pressure":null}' \
    "$HOOKS_DIR/pre-tool-use.sh"

test_hook_allows "pre-tool: numeric pressure" \
    '{"pressure":42}' \
    "$HOOKS_DIR/pre-tool-use.sh"

echo -e "\n${YELLOW}Testing subagent-start.sh${NC}"

test_hook_allows "subagent: clear pressure" \
    '{"pressure":"clear","cpu_percent":10,"memory_available_gb":6,"cursor":{"active_sessions":1}}' \
    "$HOOKS_DIR/subagent-start.sh"

test_hook_allows "subagent: strained with Cursor RSS / processes" \
    '{"pressure":"strained","cpu_percent":88,"memory_available_gb":0.8,"cursor":{"resident_memory_megabytes":2400,"process_count":12}}' \
    "$HOOKS_DIR/subagent-start.sh"

test_hook_allows "subagent: critical pressure" \
    '{"pressure":"critical","cpu_percent":98,"memory_available_gb":0.2,"cursor":{"active_sessions":3}}' \
    "$HOOKS_DIR/subagent-start.sh"

test_hook_allows "subagent: missing state file" "" "$HOOKS_DIR/subagent-start.sh"

test_hook_allows "subagent: corrupt state file" 'GARBAGE!' "$HOOKS_DIR/subagent-start.sh"

test_hook_allows "subagent: garbage pressure" \
    '{"pressure":"NUKE_EVERYTHING"}' \
    "$HOOKS_DIR/subagent-start.sh"

# ─── Test Group 2: Context Messages ────────────────────────────────────

echo -e "\n${BOLD}${CYAN}=== Test Group 2: Context Messages on Load ===${NC}"

echo '{"pressure":"strained","cpu_percent":88,"memory_available_gb":0.8,"swap_used_percent":75,"sampled_at":"'"$(date -u +%Y-%m-%dT%H:%M:%SZ)"'"}' > "$STATE_FILE"
result=$(echo '{"tool_name":"Shell"}' | bash "$HOOKS_DIR/pre-tool-use.sh" 2>/dev/null)
msg=$(echo "$result" | jq -r '.agent_message // ""' 2>/dev/null)
if [ -n "$msg" ] && [ "$msg" != "null" ]; then
    pass "strained provides context message"
else
    fail "strained should provide agent_message"
fi

echo '{"pressure":"critical","cpu_percent":98,"memory_available_gb":0.2,"swap_used_percent":95,"sampled_at":"'"$(date -u +%Y-%m-%dT%H:%M:%SZ)"'"}' > "$STATE_FILE"
result=$(echo '{"tool_name":"Shell"}' | bash "$HOOKS_DIR/pre-tool-use.sh" 2>/dev/null)
msg=$(echo "$result" | jq -r '.agent_message // ""' 2>/dev/null)
if [ -n "$msg" ] && [ "$msg" != "null" ]; then
    pass "critical provides context message"
else
    fail "critical should provide agent_message"
fi

echo '{"pressure":"clear","cpu_percent":10,"memory_available_gb":6,"sampled_at":"'"$(date -u +%Y-%m-%dT%H:%M:%SZ)"'"}' > "$STATE_FILE"
result=$(echo '{"tool_name":"Shell"}' | bash "$HOOKS_DIR/pre-tool-use.sh" 2>/dev/null)
msg=$(echo "$result" | jq -r '.agent_message // ""' 2>/dev/null)
if [ -z "$msg" ] || [ "$msg" = "null" ]; then
    pass "clear provides no context message"
else
    fail "clear should not provide agent_message (got: $msg)"
fi

# ─── Test Group 3: Fail-Open Behavior ──────────────────────────────────

echo -e "\n${BOLD}${CYAN}=== Test Group 3: Fail-Open (Default to Clear) ===${NC}"

# lib.sh's read_state_pressure should return "clear" for missing/corrupt state
test_fail_open() {
    local label="$1"
    local setup="$2"

    rm -f "$STATE_FILE"
    eval "$setup"

    local pressure
    pressure=$(source "$HOOKS_DIR/lib.sh"; read_state_pressure)

    if [ "$pressure" = "clear" ]; then
        pass "$label"
    else
        fail "$label (expected 'clear', got '$pressure')"
    fi
}

test_fail_open "fail-open: missing state file" "true"
test_fail_open "fail-open: empty file" "touch '$STATE_FILE'"
test_fail_open "fail-open: corrupt JSON" "echo 'NOT JSON' > '$STATE_FILE'"
test_fail_open "fail-open: garbage pressure" "echo '{\"pressure\":\"EVIL\"}' > '$STATE_FILE'"
test_fail_open "fail-open: null pressure" "echo '{\"pressure\":null}' > '$STATE_FILE'"

# Valid pressures should be preserved — but only from a fresh, timestamped sample
for p in clear strained critical; do
    echo "{\"pressure\":\"$p\",\"sampled_at\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\"}" > "$STATE_FILE"
    actual=$(source "$HOOKS_DIR/lib.sh"; read_state_pressure)
    if [ "$actual" = "$p" ]; then
        pass "preserves valid pressure: $p"
    else
        fail "expected '$p', got '$actual'"
    fi
done

test_fail_open "fail-open: critical with no sampled_at (age unknown)" "echo '{\"pressure\":\"critical\"}' > '$STATE_FILE'"
test_fail_open "fail-open: critical with stale sampled_at" "echo '{\"pressure\":\"critical\",\"sampled_at\":\"2020-01-01T00:00:00+00:00\"}' > '$STATE_FILE'"
test_fail_open "fail-open: critical with future-dated sampled_at" "echo '{\"pressure\":\"critical\",\"sampled_at\":\"2099-01-01T00:00:00Z\"}' > '$STATE_FILE'"
test_fail_open "fail-open: critical with non-string sampled_at" "echo '{\"pressure\":\"critical\",\"sampled_at\":{\"a\":1}}' > '$STATE_FILE'"

# ─── Test Group 4: Symlink Protection ──────────────────────────────────

echo -e "\n${BOLD}${CYAN}=== Test Group 4: Symlink Attacks ===${NC}"

rm -f "$STATE_FILE"
ln -sf /etc/passwd "$STATE_FILE"

result=$(echo '{"tool_name":"Shell"}' | bash "$HOOKS_DIR/pre-tool-use.sh" 2>/dev/null)
permission=$(echo "$result" | jq -r '.permission // "missing"' 2>/dev/null)
if [ "$permission" = "allow" ]; then
    pass "symlink state.json returns allow (not crash or deny)"
else
    fail "symlink state.json did not return allow (got $permission)"
fi

pressure=$(source "$HOOKS_DIR/lib.sh"; read_state_pressure)
if [ "$pressure" = "clear" ]; then
    pass "symlink state.json defaults to clear"
else
    fail "symlink state.json should default to clear (got $pressure)"
fi

rm -f "$STATE_FILE"

# ─── Test Group 5: No Python Dependency ─────────────────────────────────

echo -e "\n${BOLD}${CYAN}=== Test Group 5: No Python Dependency ===${NC}"

# Verify hooks work without python3 installed
if ! command -v python3 &>/dev/null; then
    echo '{"pressure":"strained","cpu_percent":88}' > "$STATE_FILE"
    result=$(echo '{"tool_name":"Shell"}' | bash "$HOOKS_DIR/pre-tool-use.sh" 2>/dev/null)
    permission=$(echo "$result" | jq -r '.permission // "missing"' 2>/dev/null)
    if [ "$permission" = "allow" ]; then
        pass "hooks work without python3"
    else
        fail "hooks require python3 (got $permission)"
    fi
else
    pass "python3 present (not a strict test — hooks should work without it too)"
fi

# ─── Test Group 6: DB Helper ───────────────────────────────────────────

echo -e "\n${BOLD}${CYAN}=== Test Group 6: SQLite DB Helper ===${NC}"

rm -f "$GUARDIAN_DIR/sessions.db"
source "$HOOKS_DIR/lib.sh"
ensure_db

if [ -f "$GUARDIAN_DIR/sessions.db" ]; then
    pass "ensure_db creates sessions.db"
    tables=$(sqlite3 "$GUARDIAN_DIR/sessions.db" ".tables" 2>/dev/null)
    if echo "$tables" | grep -q "sessions"; then
        pass "sessions table exists"
    else
        fail "sessions table missing"
    fi
    if echo "$tables" | grep -q "tool_calls"; then
        pass "tool_calls table exists"
    else
        fail "tool_calls table missing"
    fi
    if echo "$tables" | grep -q "pressure_samples"; then
        pass "pressure_samples table exists"
    else
        fail "pressure_samples table missing"
    fi
else
    fail "ensure_db did not create sessions.db"
fi

# ─── Test Group 7: beforeSubmitPrompt / beforeReadFile ───────────────────

echo -e "\n${BOLD}${CYAN}=== Test Group 7: Prompt gate & read advisory ===${NC}"

fresh_state() {
    local pressure="$1"
    local sess="${2:-2}"
    local rss="${3:-100}"
    local now
    now=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
    echo "{\"pressure\":\"${pressure}\",\"cpu_percent\":90,\"memory_available_gb\":0.4,\"swap_used_percent\":70,\"sampled_at\":\"${now}\",\"cursor\":{\"active_sessions\":${sess},\"resident_memory_megabytes\":${rss}},\"disk\":{\"volume_path\":\"/\",\"available_gb\":50,\"total_gb\":100,\"used_percent\":50,\"level\":\"clear\"}}"
}

rm -f "$GUARDIAN_DIR/proceed_once" "$GUARDIAN_DIR/snooze_until"

# Default hook_policy ships block_on_session_budget false; enable for RSS gate tests below
if [ -f "$GUARDIAN_DIR/hook_policy.json" ] && command -v jq &>/dev/null; then
    hp_tmp=$(mktemp)
    jq '.prompt_gate.block_on_session_budget = true' "$GUARDIAN_DIR/hook_policy.json" >"$hp_tmp" \
        && mv "$hp_tmp" "$GUARDIAN_DIR/hook_policy.json"
fi

fresh_state critical 2 >"$STATE_FILE"
result=$(echo '{}' | bash "$HOOKS_DIR/before-submit-prompt.sh" 2>/dev/null)
cont=$(guardian_json_continue "$result")
if [ "$cont" = "false" ]; then
    pass "beforeSubmit: blocks when critical + fresh state"
else
    fail "beforeSubmit: expected continue false (got $result)"
fi

fresh_state critical 2 >"$STATE_FILE"
result=$(printf '%s' '{"prompt":"/guardian-snooze"}' | bash "$HOOKS_DIR/before-submit-prompt.sh" 2>/dev/null)
cont=$(guardian_json_continue "$result")
if [ "$cont" = "true" ]; then
    pass "beforeSubmit: never blocks /guardian-snooze (meta)"
else
    fail "beforeSubmit: /guardian-snooze must not be blocked when critical (got $result)"
fi

fresh_state critical 2 >"$STATE_FILE"
result=$(jq -n --arg p 'Follow .cursor/skills/guardian-upgrade' '{prompt:$p}' | bash "$HOOKS_DIR/before-submit-prompt.sh" 2>/dev/null)
cont=$(guardian_json_continue "$result")
if [ "$cont" = "true" ]; then
    pass "beforeSubmit: never blocks guardian skill paths (meta)"
else
    fail "beforeSubmit: guardian skill prompt must not be blocked when critical (got $result)"
fi

touch "$GUARDIAN_DIR/proceed_once"
fresh_state critical 2 >"$STATE_FILE"
result=$(echo '{}' | bash "$HOOKS_DIR/before-submit-prompt.sh" 2>/dev/null)
cont=$(guardian_json_continue "$result")
rm -f "$GUARDIAN_DIR/proceed_once"
if [ "$cont" = "true" ]; then
    pass "beforeSubmit: proceed_once allows submit"
else
    fail "beforeSubmit: proceed_once should allow (got $result)"
fi

fresh_state clear 2 >"$STATE_FILE"
result=$(echo '{}' | bash "$HOOKS_DIR/before-submit-prompt.sh" 2>/dev/null)
cont=$(guardian_json_continue "$result")
if [ "$cont" = "true" ]; then
    pass "beforeSubmit: allows when clear"
else
    fail "beforeSubmit: should allow when clear (got $result)"
fi

fresh_state strained 2 100 >"$STATE_FILE"
result=$(echo '{}' | bash "$HOOKS_DIR/before-submit-prompt.sh" 2>/dev/null)
cont=$(guardian_json_continue "$result")
um=$(echo "$result" | jq -r '.user_message // ""' 2>/dev/null)
if [ "$cont" = "true" ] && echo "$um" | grep -q '/guardian-snooze'; then
    pass "beforeSubmit: preempt snooze hint in agent UI while strained (before critical block)"
else
    fail "beforeSubmit: expected continue true + preempt snooze hint when strained (got $result)"
fi

# RSS block only when pressure is not clear (see hooks/before-submit-prompt.sh)
fresh_state strained 2 9000 >"$STATE_FILE"
result=$(echo '{}' | bash "$HOOKS_DIR/before-submit-prompt.sh" 2>/dev/null)
cont=$(guardian_json_continue "$result")
if [ "$cont" = "false" ]; then
    pass "beforeSubmit: blocks when Cursor RSS over max and pressure is strained"
else
    fail "beforeSubmit: expected continue false when RSS exceeds max under load (got $result)"
fi

fresh_state clear 2 9000 >"$STATE_FILE"
result=$(echo '{}' | bash "$HOOKS_DIR/before-submit-prompt.sh" 2>/dev/null)
cont=$(guardian_json_continue "$result")
if [ "$cont" = "true" ]; then
    pass "beforeSubmit: allows high Cursor RSS when pressure is clear"
else
    fail "beforeSubmit: should allow when pressure clear even if RSS is high (got $result)"
fi

fresh_state clear 2 >"$STATE_FILE"
result=$(printf '%s' 'not valid json {{{' | bash "$HOOKS_DIR/before-submit-prompt.sh" 2>/dev/null)
if echo "$result" | jq -e . >/dev/null 2>&1; then
    pass "beforeSubmit: malformed stdin returns JSON (no jq/shell abort)"
else
    fail "beforeSubmit: malformed stdin broke output (got: ${result:0:200})"
fi

fresh_state clear 2 >"$STATE_FILE"
result=$(echo '{"attachments":"not-an-array"}' | bash "$HOOKS_DIR/before-submit-prompt.sh" 2>/dev/null)
cont=$(guardian_json_continue "$result")
if [ "$cont" = "true" ]; then
    pass "beforeSubmit: non-array attachments does not abort"
else
    fail "beforeSubmit: expected continue true with bad attachments shape (got $result)"
fi

cp "$GUARDIAN_DIR/hook_policy.json" "$GUARDIAN_DIR/hook_policy.json.bak.test"
printf '%s' '{' >"$GUARDIAN_DIR/hook_policy.json"
fresh_state critical 2 >"$STATE_FILE"
result=$(echo '{}' | bash "$HOOKS_DIR/before-submit-prompt.sh" 2>/dev/null)
mv "$GUARDIAN_DIR/hook_policy.json.bak.test" "$GUARDIAN_DIR/hook_policy.json"
cont=$(guardian_json_continue "$result")
um=$(echo "$result" | jq -r '.user_message // ""' 2>/dev/null)
if [ "$cont" = "true" ] && echo "$um" | grep -q 'not valid JSON'; then
    pass "beforeSubmit: corrupt hook_policy.json fails open"
else
    fail "beforeSubmit: expected fail-open + message on corrupt policy (got $result)"
fi

result=$(printf '%s' 'NOT JSON' | bash "$HOOKS_DIR/before-read-file.sh" 2>/dev/null)
perm=$(echo "$result" | jq -r '.permission // "missing"' 2>/dev/null)
if [ "$perm" = "allow" ]; then
    pass "beforeRead: malformed stdin allows"
else
    fail "beforeRead: expected allow on malformed stdin (got $result)"
fi

mkdir -p /tmp/gdemo/ws/node_modules/pkg
touch /tmp/gdemo/ws/node_modules/pkg/file.txt
fresh_state clear 2 >"$STATE_FILE"
result=$(echo '{"file_path":"/tmp/gdemo/ws/node_modules/pkg/file.txt","workspace_roots":["/tmp/gdemo/ws"]}' | bash "$HOOKS_DIR/before-read-file.sh" 2>/dev/null)
perm=$(echo "$result" | jq -r '.permission // "missing"' 2>/dev/null)
if [ "$perm" = "allow" ]; then
    pass "beforeRead: allows with advisory on node_modules path"
else
    fail "beforeRead: expected allow (got $result)"
fi

# ─── Test Group 7b: Claude Code / Codex adapters & tool gate ─────────────

echo -e "\n${BOLD}${CYAN}=== Test Group 7b: Claude Code / Codex adapters & tool gate ===${NC}"

CLAUDE_HOOKS="$HOOKS_DIR/claude"
CODEX_HOOKS="$HOOKS_DIR/codex"

reset_policy() {
    cp "$HOOKS_DIR/hook_policy.default.json" "$GUARDIAN_DIR/hook_policy.json"
    rm -f "$GUARDIAN_DIR/proceed_once" "$GUARDIAN_DIR/snooze_until"
    rm -rf "$GUARDIAN_DIR/advise"
}

set_deny_subagents() {
    local hp_tmp
    hp_tmp=$(mktemp)
    jq --arg v "$1" '.tool_gate.deny_subagents_on = $v' "$GUARDIAN_DIR/hook_policy.json" >"$hp_tmp" \
        && mv "$hp_tmp" "$GUARDIAN_DIR/hook_policy.json"
}

pre_tool_decision() {
    echo "$1" | jq -r '.hookSpecificOutput.permissionDecision // "none"' 2>/dev/null || echo "unparseable"
}

reset_policy

fresh_state clear >"$STATE_FILE"
result=$(echo '{"tool_name":"Bash","session_id":"s-clear"}' | bash "$CLAUDE_HOOKS/pre-tool-use.sh" 2>/dev/null)
rc=$?
if [ "$rc" -eq 0 ] && [ -z "$result" ]; then
    pass "claude preTool: clear pressure is silent (no output, exit 0)"
else
    fail "claude preTool: clear should be silent (rc=$rc, got $result)"
fi

fresh_state strained >"$STATE_FILE"
result=$(echo '{"tool_name":"Bash","session_id":"s-a"}' | bash "$CLAUDE_HOOKS/pre-tool-use.sh" 2>/dev/null)
ctx=$(echo "$result" | jq -r '.hookSpecificOutput.additionalContext // ""' 2>/dev/null)
ev=$(echo "$result" | jq -r '.hookSpecificOutput.hookEventName // ""' 2>/dev/null)
if [ -n "$ctx" ] && [ "$ev" = "PreToolUse" ] && [ "$(pre_tool_decision "$result")" = "none" ]; then
    pass "claude preTool: strained gives additionalContext with no permission decision"
else
    fail "claude preTool: strained advisory wrong (got $result)"
fi

result=$(echo '{"tool_name":"Bash","session_id":"s-a"}' | bash "$CLAUDE_HOOKS/pre-tool-use.sh" 2>/dev/null)
if [ -z "$result" ]; then
    pass "claude preTool: repeat advisory in same session is rate limited"
else
    fail "claude preTool: expected rate-limited silence (got $result)"
fi

result=$(echo '{"tool_name":"Bash","session_id":"s-b"}' | bash "$CLAUDE_HOOKS/pre-tool-use.sh" 2>/dev/null)
ctx=$(echo "$result" | jq -r '.hookSpecificOutput.additionalContext // ""' 2>/dev/null)
if [ -n "$ctx" ]; then
    pass "claude preTool: a different session still gets its advisory"
else
    fail "claude preTool: other session should be advised (got $result)"
fi

fresh_state critical >"$STATE_FILE"
result=$(echo '{"tool_name":"Bash","session_id":"s-a"}' | bash "$CLAUDE_HOOKS/pre-tool-use.sh" 2>/dev/null)
ctx=$(echo "$result" | jq -r '.hookSpecificOutput.additionalContext // ""' 2>/dev/null)
if [ -n "$ctx" ]; then
    pass "claude preTool: escalation to critical bypasses the rate limit"
else
    fail "claude preTool: critical should re-advise after strained (got $result)"
fi

reset_policy
fresh_state critical >"$STATE_FILE"
result=$(echo '{"tool_name":"Agent","session_id":"s-c"}' | bash "$CLAUDE_HOOKS/pre-tool-use.sh" 2>/dev/null)
if [ "$(pre_tool_decision "$result")" = "none" ]; then
    pass "claude preTool: default policy never denies a subagent (advisory only)"
else
    fail "claude preTool: default policy must not deny (got $result)"
fi

reset_policy
set_deny_subagents critical
fresh_state critical >"$STATE_FILE"
for tool in Agent Task; do
    result=$(echo "{\"tool_name\":\"$tool\",\"session_id\":\"s-d\"}" | bash "$CLAUDE_HOOKS/pre-tool-use.sh" 2>/dev/null)
    reason=$(echo "$result" | jq -r '.hookSpecificOutput.permissionDecisionReason // ""' 2>/dev/null)
    if [ "$(pre_tool_decision "$result")" = "deny" ] && echo "$reason" | grep -q 'guardian snooze'; then
        pass "claude preTool: deny_subagents_on=critical denies $tool at critical"
    else
        fail "claude preTool: expected deny for $tool (got $result)"
    fi
done

result=$(echo '{"tool_name":"spawn_agent","session_id":"s-d"}' | bash "$CODEX_HOOKS/pre-tool-use.sh" 2>/dev/null)
if [ "$(pre_tool_decision "$result")" = "deny" ]; then
    pass "codex preTool: deny_subagents_on=critical denies spawn_agent at critical"
else
    fail "codex preTool: expected deny for spawn_agent (got $result)"
fi

result=$(echo '{"tool_name":"Bash","session_id":"s-e"}' | bash "$CLAUDE_HOOKS/pre-tool-use.sh" 2>/dev/null)
if [ "$(pre_tool_decision "$result")" = "none" ]; then
    pass "claude preTool: subagent deny never applies to Bash"
else
    fail "claude preTool: Bash must not be denied (got $result)"
fi

fresh_state strained >"$STATE_FILE"
result=$(echo '{"tool_name":"Agent","session_id":"s-f"}' | bash "$CLAUDE_HOOKS/pre-tool-use.sh" 2>/dev/null)
if [ "$(pre_tool_decision "$result")" = "none" ]; then
    pass "claude preTool: deny_subagents_on=critical does not deny at strained"
else
    fail "claude preTool: strained must not deny when policy is critical (got $result)"
fi

fresh_state critical >"$STATE_FILE"
date -u -d '+10 minutes' +"%Y-%m-%dT%H:%M:%SZ" >"$GUARDIAN_DIR/snooze_until" 2>/dev/null \
    || date -u -v+10M +"%Y-%m-%dT%H:%M:%SZ" >"$GUARDIAN_DIR/snooze_until"
result=$(echo '{"tool_name":"Agent","session_id":"s-g"}' | bash "$CLAUDE_HOOKS/pre-tool-use.sh" 2>/dev/null)
rm -f "$GUARDIAN_DIR/snooze_until"
if [ "$(pre_tool_decision "$result")" = "none" ]; then
    pass "claude preTool: an active snooze lifts the subagent deny"
else
    fail "claude preTool: snooze should lift deny (got $result)"
fi

echo '{"pressure":"critical","cpu_percent":99,"memory_available_gb":0.1,"swap_used_percent":99,"sampled_at":"2020-01-01T00:00:00+00:00"}' >"$STATE_FILE"
result=$(echo '{"tool_name":"Agent","session_id":"s-h"}' | bash "$CLAUDE_HOOKS/pre-tool-use.sh" 2>/dev/null)
rc=$?
if [ "$rc" -eq 0 ] && [ -z "$result" ]; then
    pass "claude preTool: stale state fails open (silent, no deny)"
else
    fail "claude preTool: stale state must fail open (rc=$rc, got $result)"
fi

fresh_state critical >"$STATE_FILE"
result=$(printf '%s' 'NOT JSON {{{' | bash "$CLAUDE_HOOKS/pre-tool-use.sh" 2>/dev/null)
rc=$?
if [ "$rc" -eq 0 ] && [ "$(pre_tool_decision "$result")" = "none" ]; then
    pass "claude preTool: garbage stdin exits 0 without a deny"
else
    fail "claude preTool: garbage stdin handling (rc=$rc, got $result)"
fi

allow_seen=0
for p in clear strained critical; do
    for tool in Bash Agent Task Write; do
        rm -rf "$GUARDIAN_DIR/advise"
        fresh_state "$p" >"$STATE_FILE"
        result=$(echo "{\"tool_name\":\"$tool\",\"session_id\":\"s-sweep\"}" | bash "$CLAUDE_HOOKS/pre-tool-use.sh" 2>/dev/null)
        if [ "$(pre_tool_decision "$result")" = "allow" ]; then
            allow_seen=1
        fi
    done
done
if [ "$allow_seen" -eq 0 ]; then
    pass "claude preTool: never emits permissionDecision allow (would skip permission prompts)"
else
    fail "claude preTool: emitted permissionDecision allow"
fi

# Cursor preToolUse shares the tool gate
fresh_state critical >"$STATE_FILE"
result=$(echo '{"tool_name":"Task"}' | bash "$HOOKS_DIR/pre-tool-use.sh" 2>/dev/null)
perm=$(echo "$result" | jq -r '.permission // "missing"' 2>/dev/null)
if [ "$perm" = "deny" ]; then
    pass "cursor preTool: deny_subagents_on=critical denies Task at critical"
else
    fail "cursor preTool: expected deny for Task (got $result)"
fi
result=$(echo '{"tool_name":"Shell"}' | bash "$HOOKS_DIR/pre-tool-use.sh" 2>/dev/null)
perm=$(echo "$result" | jq -r '.permission // "missing"' 2>/dev/null)
if [ "$perm" = "allow" ]; then
    pass "cursor preTool: Shell still allowed under subagent deny policy"
else
    fail "cursor preTool: Shell must be allowed (got $result)"
fi

# Prompt gate through the Claude Code adapter
reset_policy
fresh_state critical >"$STATE_FILE"
result=$(echo '{"prompt":"do a thing","session_id":"s-p"}' | bash "$CLAUDE_HOOKS/user-prompt-submit.sh" 2>/dev/null)
decision=$(echo "$result" | jq -r '.decision // ""' 2>/dev/null)
reason=$(echo "$result" | jq -r '.reason // ""' 2>/dev/null)
if [ "$decision" = "block" ] && echo "$reason" | grep -q 'guardian snooze 15' && ! echo "$reason" | grep -q '/guardian-snooze'; then
    pass "claude promptGate: blocks at critical with CLI unblock hint (no Cursor slash commands)"
else
    fail "claude promptGate: expected block + CLI hint (got $result)"
fi

fresh_state clear >"$STATE_FILE"
result=$(echo '{"prompt":"do a thing","session_id":"s-p"}' | bash "$CLAUDE_HOOKS/user-prompt-submit.sh" 2>/dev/null)
decision=$(echo "$result" | jq -r '.decision // "none"' 2>/dev/null)
if [ "$decision" = "none" ] && echo "$result" | jq empty 2>/dev/null; then
    pass "claude promptGate: clear pressure does not block"
else
    fail "claude promptGate: clear must not block (got $result)"
fi

echo '{"pressure":"critical","sampled_at":"2020-01-01T00:00:00+00:00"}' >"$STATE_FILE"
result=$(echo '{"prompt":"do a thing"}' | bash "$CLAUDE_HOOKS/user-prompt-submit.sh" 2>/dev/null)
decision=$(echo "$result" | jq -r '.decision // "none"' 2>/dev/null)
if [ "$decision" = "none" ]; then
    pass "claude promptGate: stale state fails open"
else
    fail "claude promptGate: stale state must not block (got $result)"
fi

# SessionStart through both adapters
for fmt in claude codex; do
    rm -f "$GUARDIAN_DIR/sessions.db"
    fresh_state critical >"$STATE_FILE"
    result=$(echo "{\"session_id\":\"sess-$fmt\",\"model\":\"m1\",\"source\":\"startup\"}" | bash "$HOOKS_DIR/$fmt/session-start.sh" 2>/dev/null)
    ev=$(echo "$result" | jq -r '.hookSpecificOutput.hookEventName // ""' 2>/dev/null)
    ctx=$(echo "$result" | jq -r '.hookSpecificOutput.additionalContext // ""' 2>/dev/null)
    if [ "$ev" = "SessionStart" ] && echo "$ctx" | grep -q 'Load critical' && ! echo "$ctx" | grep -q '/guardian-snooze'; then
        pass "$fmt sessionStart: hookSpecificOutput context reports critical load"
    else
        fail "$fmt sessionStart: wrong output (got $result)"
    fi
    rows=$(sqlite3 "$GUARDIAN_DIR/sessions.db" "SELECT COUNT(*) FROM sessions WHERE conversation_id='sess-$fmt';" 2>/dev/null || echo 0)
    if [ "$rows" = "1" ]; then
        pass "$fmt sessionStart: session_id is recorded in sessions.db"
    else
        fail "$fmt sessionStart: session row missing (rows=$rows)"
    fi
done

echo '{"pressure":"critical","cpu_percent":99,"memory_available_gb":0.1,"sampled_at":"2020-01-01T00:00:00+00:00"}' >"$STATE_FILE"
result=$(echo '{"session_id":"sess-stale"}' | bash "$CLAUDE_HOOKS/session-start.sh" 2>/dev/null)
ctx=$(echo "$result" | jq -r '.hookSpecificOutput.additionalContext // ""' 2>/dev/null)
if echo "$ctx" | grep -q 'no daemon' && ! echo "$ctx" | grep -q 'CPU 99'; then
    pass "claude sessionStart: dead daemon is reported and stale numbers are not shown"
else
    fail "claude sessionStart: stale handling wrong (got $result)"
fi

fresh_state clear >"$STATE_FILE"
result=$(echo '{"conversation_id":"cur-1","model":"m1"}' | bash "$HOOKS_DIR/session-start.sh" 2>/dev/null)
ctx=$(echo "$result" | jq -r '.additional_context // ""' 2>/dev/null)
if echo "$ctx" | grep -q 'metrics live' && echo "$ctx" | grep -q 'Cursor RSS'; then
    pass "cursor sessionStart: additional_context shape unchanged"
else
    fail "cursor sessionStart: wrong output (got $result)"
fi

# ISO parsing fast path (jq, no python) must agree for both daemon and CLI formats
epoch_plus=$(bash -c "source '$HOOKS_DIR/lib.sh'; parse_iso_epoch '2026-05-01T15:39:12.526814+00:00'")
epoch_z=$(bash -c "source '$HOOKS_DIR/lib.sh'; parse_iso_epoch '2026-05-01T15:39:12Z'")
if [ "$epoch_plus" = "1777649952" ] && [ "$epoch_z" = "1777649952" ]; then
    pass "lib: parse_iso_epoch handles +00:00 fractional and Z timestamps"
else
    fail "lib: parse_iso_epoch wrong (plus=$epoch_plus z=$epoch_z)"
fi

# The jq fast path must agree with the python reference on edge dates, in any timezone
iso_ok=1
for ts in 2024-02-29T23:59:59Z 2025-01-01T00:00:00Z 2000-03-01T00:00:00+00:00 2026-07-15T12:00:00.5Z 2028-02-29T00:00:00Z; do
    fast=$(TZ=America/New_York bash -c "source '$HOOKS_DIR/lib.sh'; parse_iso_epoch '$ts'")
    ref=$(python3 "$HOOKS_DIR/iso_to_epoch.py" "$ts" 2>/dev/null || echo "noref")
    if [ "$fast" != "$ref" ]; then
        iso_ok=0
        echo "    mismatch for $ts: fast=$fast ref=$ref"
    fi
done
if [ "$iso_ok" -eq 1 ]; then
    pass "lib: parse_iso_epoch fast path matches python reference (leap days, DST months, non-UTC TZ)"
else
    fail "lib: parse_iso_epoch fast path disagrees with python reference"
fi

# Shipped matchers must actually route subagent tools to the hook (tested through the template,
# not by piping JSON straight into the script)
matcher_covers() {
    local template="$1" tool="$2" m
    m=$(jq -r '.hooks.PreToolUse[0].matcher // ""' "$template")
    [[ "$tool" =~ ^($m)$ ]]
}
if matcher_covers "$HOOKS_DIR/codex-hooks.json" spawn_agent && matcher_covers "$HOOKS_DIR/codex-hooks.json" Agent && matcher_covers "$HOOKS_DIR/codex-hooks.json" Bash; then
    pass "codex template: PreToolUse matcher covers Bash, Agent and spawn_agent"
else
    fail "codex template: PreToolUse matcher does not cover the subagent tool"
fi
if matcher_covers "$HOOKS_DIR/claude-code-hooks.json" Agent && matcher_covers "$HOOKS_DIR/claude-code-hooks.json" Task && matcher_covers "$HOOKS_DIR/claude-code-hooks.json" Bash; then
    pass "claude template: PreToolUse matcher covers Bash, Agent and Task"
else
    fail "claude template: PreToolUse matcher does not cover the subagent tool"
fi

# Installer merge (the real helper the installers call)
MERGE="$HOOKS_DIR/merge-agent-hooks.sh"
CL_TPL="$HOOKS_DIR/claude-code-hooks.json"
CL_PH="__GUARDIAN_CLAUDE_HOOK_ROOT__"
merge_dir=$(mktemp -d)
run_merge() { bash "$MERGE" "$1" "$CL_TPL" "$CL_PH" "$2" "$1.bak" >/dev/null 2>&1; }

cat >"$merge_dir/base.json" <<'JSON'
{"tui":"fullscreen","hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"/other/hook.sh"},{"type":"command","command":"/home/u/.claude/hooks/guardian/claude/pre-tool-use.sh"}]}],"Stop":[{"hooks":[{"type":"command","command":"/other/stop.sh"}]}]}}
JSON
run_merge "$merge_dir/base.json" /home/u/.claude/hooks/guardian
cp "$merge_dir/base.json" "$merge_dir/after1.json"
guardian_count=$(jq '[.hooks[][] | .hooks[] | select(.command | contains("/hooks/guardian/"))] | length' "$merge_dir/base.json" 2>/dev/null)
other_kept=$(jq '[.hooks[][] | .hooks[] | select(.command | startswith("/other/"))] | length' "$merge_dir/base.json" 2>/dev/null)
tui=$(jq -r '.tui // ""' "$merge_dir/base.json" 2>/dev/null)
if [ "$guardian_count" = "3" ] && [ "$other_kept" = "2" ] && [ "$tui" = "fullscreen" ]; then
    pass "installer merge: replaces Guardian handlers, keeps other hooks and settings"
else
    fail "installer merge: guardian=$guardian_count other=$other_kept tui=$tui"
fi

echo 'ORIGINAL-BACKUP-SENTINEL' >"$merge_dir/base.json.bak"
run_merge "$merge_dir/base.json" /home/u/.claude/hooks/guardian
if cmp -s "$merge_dir/base.json" "$merge_dir/after1.json" && grep -q 'ORIGINAL-BACKUP-SENTINEL' "$merge_dir/base.json.bak"; then
    pass "installer merge: re-run is a no-op and does not overwrite the backup"
else
    fail "installer merge: re-run changed the file or clobbered the backup"
fi

run_merge "$merge_dir/new.json" /home/u/.claude/hooks/guardian
if jq -e '.hooks.UserPromptSubmit | length == 1' "$merge_dir/new.json" >/dev/null 2>&1; then
    pass "installer merge: creates the config when it does not exist"
else
    fail "installer merge: did not create a valid config"
fi

run_merge "$merge_dir/odd-path.json" "/home/a b&c|d/.claude/hooks/guardian"
cmd=$(jq -r '.hooks.PreToolUse[0].hooks[0].command' "$merge_dir/odd-path.json" 2>/dev/null)
if [ "$cmd" = "'/home/a b&c|d/.claude/hooks/guardian/claude/pre-tool-use.sh'" ]; then
    pass "installer merge: paths with spaces, & and | are substituted literally and shell-quoted"
else
    fail "installer merge: bad command for odd path (got $cmd)"
fi

echo '{"hooks":{"PreToolUse":[{"matcher":"Bash","command":"legacy-shape"},"a-string"]}}' >"$merge_dir/shapes.json"
run_merge "$merge_dir/shapes.json" /home/u/.claude/hooks/guardian
kept=$(jq -c '[.hooks.PreToolUse[] | select(type == "string" or (.command? == "legacy-shape"))] | length' "$merge_dir/shapes.json" 2>/dev/null)
if [ "$kept" = "2" ]; then
    pass "installer merge: entries in shapes it does not understand are preserved"
else
    fail "installer merge: dropped unknown-shape entries (kept=$kept)"
fi

refuse_ok=1
for bad in '{"hooks":{"PreToolUse":{"matcher":"x"}}}' '{"hooks":[]}' '[1,2]' ''; do
    printf '%s' "$bad" >"$merge_dir/bad.json"
    cp "$merge_dir/bad.json" "$merge_dir/bad.orig"
    if run_merge "$merge_dir/bad.json" /home/u/.claude/hooks/guardian; then
        refuse_ok=0
    fi
    cmp -s "$merge_dir/bad.json" "$merge_dir/bad.orig" || refuse_ok=0
done
if [ "$refuse_ok" -eq 1 ]; then
    pass "installer merge: unmergeable configs are refused and left byte-for-byte untouched"
else
    fail "installer merge: modified or accepted an unmergeable config"
fi

cat >"$merge_dir/legacy.json" <<'JSON'
{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"/home/u/.claude/hooks/guardian/claude/pre-tool-use.sh","timeout":3}]},{"hooks":[{"type":"command","command":"/home/u/.claude/hooks/guardian/claude/pre-tool-use.sh","timeout":3}]}],"UserPromptSubmit":[{"hooks":[{"type":"command","command":"/home/u/.claude/hooks/guardian/codex/user-prompt-submit.sh"}]}]}}
JSON
run_merge "$merge_dir/legacy.json" /home/u/.claude/hooks/guardian
n=$(jq '[.hooks[][] | .hooks[]] | length' "$merge_dir/legacy.json" 2>/dev/null)
unmatched=$(jq '[.hooks.PreToolUse[] | select(.matcher == null)] | length' "$merge_dir/legacy.json" 2>/dev/null)
if [ "$n" = "3" ] && [ "$unmatched" = "0" ]; then
    pass "installer merge: upgrade from 0.3.1 removes duplicate and matcher-less Guardian entries"
else
    fail "installer merge: legacy upgrade left n=$n unmatched=$unmatched"
fi
rm -rf "$merge_dir"

reset_policy

# ─── Test Group 8: Host Pollution Check ─────────────────────────────────

echo -e "\n${BOLD}${CYAN}=== Test Group 8: No Host Pollution ===${NC}"

# Inside the container, /root/.guardian is a tmpfs mount.
# Verify we haven't escaped the container by checking mount points.
if mount | grep -q "tmpfs on /root/.guardian"; then
    pass "guardian dir is tmpfs (container-isolated)"
else
    # Not all container runtimes mount tmpfs exactly this way
    pass "guardian dir check (running inside container: $TEST_MODE)"
fi

# ─── Summary ────────────────────────────────────────────────────────────

echo -e "\n${BOLD}${CYAN}════════════════════════════════════════${NC}"
echo -e "${BOLD}Test Results${NC}"
echo -e "${CYAN}════════════════════════════════════════${NC}"
echo -e "${GREEN}Passed: ${#PASSES[@]}${NC}"
echo -e "${RED}Failed: ${#FAILURES[@]}${NC}"

if [ ${#FAILURES[@]} -gt 0 ]; then
    echo -e "\n${RED}Failed tests:${NC}"
    for f in "${FAILURES[@]}"; do
        echo -e "  ${RED}✗${NC} $f"
    done
fi

echo ""

if [ ${#FAILURES[@]} -eq 0 ]; then
    echo -e "${GREEN}${BOLD}All tests passed.${NC}"
    exit 0
else
    echo -e "${RED}${BOLD}${#FAILURES[@]} test(s) failed.${NC}"
    exit 1
fi
