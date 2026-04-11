#!/bin/bash
# End-to-end test: simulates a full Cursor agent lifecycle through Guardian hooks.
#
# Runs INSIDE a Docker container — never on the host.
# Validates:
#   1. Agent registration confirmation on session-start
#   2. Daemon-active vs daemon-inactive banner
#   3. Pressure transitions (clear → strained → critical → clear)
#   4. Throttle advisories appear/disappear with pressure
#   5. DB records: sessions, tool_calls, pressure_samples, ROI score
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

if [ "${TEST_MODE:-}" != "containerized" ]; then
    echo "Running e2e agent lifecycle test inside Docker..."
    docker compose -f "$SCRIPT_DIR/docker-compose.test.yml" build guardian-e2e
    docker compose -f "$SCRIPT_DIR/docker-compose.test.yml" run --rm guardian-e2e
    exit $?
fi

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

GUARDIAN_DIR="${GUARDIAN_DIR:-/root/.guardian}"
STATE_FILE="$GUARDIAN_DIR/state.json"
SESSIONS_DB="$GUARDIAN_DIR/sessions.db"
HOOKS_DIR="${HOOKS_DIR:-/opt/guardian/hooks}"

FAILURES=()
PASSES=()

pass() { echo -e "  ${GREEN}PASS${NC}: $1"; PASSES+=("$1"); }
fail() { echo -e "  ${RED}FAIL${NC}: $1"; FAILURES+=("$1"); }

CONV_ID="e2e-test-$(date +%s)"
MODEL="claude-sonnet-4-20250514"

fresh_state() {
    local pressure="$1"
    local cpu="${2:-25}"
    local mem="${3:-6.0}"
    local swap="${4:-5}"
    local now
    now=$(date -u +"%Y-%m-%dT%H:%M:%S.000Z")
    cat > "$STATE_FILE" <<EOF
{
  "pressure": "$pressure",
  "cpu_percent": $cpu,
  "memory_available_gb": $mem,
  "memory_total_gb": 16.0,
  "swap_used_percent": $swap,
  "thermal_state": "nominal",
  "docker": {"running_containers": 2, "total_cpu_percent": 10.0, "total_memory_mb": 512},
  "cursor": {"active_sessions": 1, "process_count": 4},
  "disk": {"volume_path": "/root", "available_gb": 80.0, "total_gb": 100.0, "used_percent": 20.0, "level": "clear"},
  "process_count": 150,
  "max_proc_per_uid": 2048,
  "sampled_at": "$now"
}
EOF
}

# ─── Phase 1: Registration with Active Daemon ──────────────────────────

echo -e "\n${BOLD}${CYAN}=== Phase 1: Agent Registration (daemon active) ===${NC}"

rm -f "$STATE_FILE" "$SESSIONS_DB"
mkdir -p "$GUARDIAN_DIR"
fresh_state "clear" 25 6.0 5

result=$(echo "{\"conversation_id\":\"$CONV_ID\",\"model\":\"$MODEL\"}" \
    | bash "$HOOKS_DIR/session-start.sh" 2>/dev/null)

ctx=$(echo "$result" | jq -r '.additional_context // ""' 2>/dev/null)

if echo "$ctx" | grep -q '\[Guardian\]'; then
    pass "registration banner present in additional_context"
else
    fail "registration banner missing (got: $ctx)"
fi

if echo "$ctx" | grep -q 'daemon active'; then
    pass "daemon-active status detected"
else
    fail "daemon-active status missing (got: $ctx)"
fi

if echo "$ctx" | grep -q 'Agent registered'; then
    pass "registration confirmation text present"
else
    fail "registration confirmation missing (got: $ctx)"
fi

# ─── Phase 2: Registration without Daemon ───────────────────────────────

echo -e "\n${BOLD}${CYAN}=== Phase 2: Agent Registration (daemon inactive) ===${NC}"

rm -f "$STATE_FILE" "$SESSIONS_DB"
CONV_ID_INACTIVE="e2e-inactive-$(date +%s)"

result=$(echo "{\"conversation_id\":\"$CONV_ID_INACTIVE\",\"model\":\"$MODEL\"}" \
    | bash "$HOOKS_DIR/session-start.sh" 2>/dev/null)

ctx=$(echo "$result" | jq -r '.additional_context // ""' 2>/dev/null)

if echo "$ctx" | grep -q '\[Guardian\]'; then
    pass "registration banner present without daemon"
else
    fail "registration banner missing without daemon (got: $ctx)"
fi

if echo "$ctx" | grep -q 'daemon not detected'; then
    pass "daemon-inactive status detected"
else
    fail "daemon-inactive status missing (got: $ctx)"
fi

# ─── Phase 3: Tool Use Under Clear Pressure ─────────────────────────────

echo -e "\n${BOLD}${CYAN}=== Phase 3: Tool Use — Clear Pressure ===${NC}"

rm -f "$STATE_FILE" "$SESSIONS_DB"
fresh_state "clear" 25 6.0 5

# Re-register session for remaining phases
echo "{\"conversation_id\":\"$CONV_ID\",\"model\":\"$MODEL\"}" \
    | bash "$HOOKS_DIR/session-start.sh" > /dev/null 2>&1

result=$(echo '{"tool_name":"Shell","tool_input":"echo hello"}' \
    | bash "$HOOKS_DIR/pre-tool-use.sh" 2>/dev/null)

permission=$(echo "$result" | jq -r '.permission // "missing"' 2>/dev/null)
msg=$(echo "$result" | jq -r '.agent_message // ""' 2>/dev/null)

if [ "$permission" = "allow" ]; then
    pass "pre-tool-use allows under clear pressure"
else
    fail "pre-tool-use should allow under clear (got: $permission)"
fi

if [ -z "$msg" ] || [ "$msg" = "null" ]; then
    pass "no advisory message under clear pressure"
else
    fail "unexpected advisory under clear pressure (got: $msg)"
fi

# Record tool call
echo "{\"conversation_id\":\"$CONV_ID\",\"tool_name\":\"Shell\",\"duration\":150,\"model\":\"$MODEL\"}" \
    | bash "$HOOKS_DIR/post-tool-use.sh" 2>/dev/null

# ─── Phase 4: Escalate to Strained ──────────────────────────────────────

echo -e "\n${BOLD}${CYAN}=== Phase 4: Pressure Escalation — Strained ===${NC}"

fresh_state "strained" 82 1.1 40

result=$(echo '{"tool_name":"Shell","tool_input":"npm install"}' \
    | bash "$HOOKS_DIR/pre-tool-use.sh" 2>/dev/null)

permission=$(echo "$result" | jq -r '.permission // "missing"' 2>/dev/null)
msg=$(echo "$result" | jq -r '.agent_message // ""' 2>/dev/null)

if [ "$permission" = "allow" ]; then
    pass "pre-tool-use allows under strained pressure"
else
    fail "pre-tool-use should allow under strained (got: $permission)"
fi

if [ -n "$msg" ] && [ "$msg" != "null" ]; then
    pass "advisory message present under strained pressure"
else
    fail "advisory message missing under strained pressure"
fi

if echo "$msg" | grep -qi 'moderate load\|parallel'; then
    pass "strained advisory mentions load context"
else
    fail "strained advisory lacks load context (got: $msg)"
fi

# Record tool call
echo "{\"conversation_id\":\"$CONV_ID\",\"tool_name\":\"Shell\",\"duration\":3200,\"model\":\"$MODEL\"}" \
    | bash "$HOOKS_DIR/post-tool-use.sh" 2>/dev/null

# Test subagent-start under strained
result=$(echo '{}' | bash "$HOOKS_DIR/subagent-start.sh" 2>/dev/null)
msg=$(echo "$result" | jq -r '.agent_message // ""' 2>/dev/null)

if [ -n "$msg" ] && [ "$msg" != "null" ]; then
    pass "subagent-start advisory present under strained"
else
    fail "subagent-start advisory missing under strained"
fi

# ─── Phase 5: Escalate to Critical ──────────────────────────────────────

echo -e "\n${BOLD}${CYAN}=== Phase 5: Pressure Escalation — Critical ===${NC}"

fresh_state "critical" 96 0.3 85

result=$(echo '{"tool_name":"Task","tool_input":"run tests"}' \
    | bash "$HOOKS_DIR/pre-tool-use.sh" 2>/dev/null)

permission=$(echo "$result" | jq -r '.permission // "missing"' 2>/dev/null)
msg=$(echo "$result" | jq -r '.agent_message // ""' 2>/dev/null)

if [ "$permission" = "allow" ]; then
    pass "pre-tool-use allows under critical pressure (never deny)"
else
    fail "pre-tool-use should allow under critical (got: $permission)"
fi

if [ -n "$msg" ] && [ "$msg" != "null" ]; then
    pass "advisory message present under critical pressure"
else
    fail "advisory message missing under critical pressure"
fi

if echo "$msg" | grep -qi 'high load\|sequential\|container'; then
    pass "critical advisory mentions severity context"
else
    fail "critical advisory lacks severity context (got: $msg)"
fi

result=$(echo '{}' | bash "$HOOKS_DIR/subagent-start.sh" 2>/dev/null)
msg=$(echo "$result" | jq -r '.agent_message // ""' 2>/dev/null)

if [ -n "$msg" ] && [ "$msg" != "null" ]; then
    pass "subagent-start advisory present under critical"
else
    fail "subagent-start advisory missing under critical"
fi

# Record tool call under critical
echo "{\"conversation_id\":\"$CONV_ID\",\"tool_name\":\"Task\",\"duration\":8500,\"model\":\"$MODEL\"}" \
    | bash "$HOOKS_DIR/post-tool-use.sh" 2>/dev/null

# ─── Phase 6: De-escalate to Clear ──────────────────────────────────────

echo -e "\n${BOLD}${CYAN}=== Phase 6: Pressure De-escalation — Back to Clear ===${NC}"

fresh_state "clear" 15 7.2 3

result=$(echo '{"tool_name":"Shell","tool_input":"echo done"}' \
    | bash "$HOOKS_DIR/pre-tool-use.sh" 2>/dev/null)

permission=$(echo "$result" | jq -r '.permission // "missing"' 2>/dev/null)
msg=$(echo "$result" | jq -r '.agent_message // ""' 2>/dev/null)

if [ "$permission" = "allow" ]; then
    pass "pre-tool-use allows after de-escalation"
else
    fail "pre-tool-use should allow after de-escalation (got: $permission)"
fi

if [ -z "$msg" ] || [ "$msg" = "null" ]; then
    pass "advisory disappears after de-escalation to clear"
else
    fail "advisory should disappear on clear (got: $msg)"
fi

# Record final tool call
echo "{\"conversation_id\":\"$CONV_ID\",\"tool_name\":\"Shell\",\"duration\":50,\"model\":\"$MODEL\"}" \
    | bash "$HOOKS_DIR/post-tool-use.sh" 2>/dev/null

# ─── Phase 7: Session Stop + ROI ────────────────────────────────────────

echo -e "\n${BOLD}${CYAN}=== Phase 7: Session Stop and ROI Computation ===${NC}"

echo "{\"conversation_id\":\"$CONV_ID\",\"status\":\"completed\",\"loop_count\":6}" \
    | bash "$HOOKS_DIR/stop.sh" 2>/dev/null

# ─── Phase 8: Database Validation ────────────────────────────────────────

echo -e "\n${BOLD}${CYAN}=== Phase 8: Database State Validation ===${NC}"

session_exists=$(sqlite3 "$SESSIONS_DB" "SELECT COUNT(*) FROM sessions WHERE conversation_id='$CONV_ID';" 2>/dev/null)
if [ "$session_exists" = "1" ]; then
    pass "session row exists in DB"
else
    fail "session row missing (count=$session_exists)"
fi

db_model=$(sqlite3 "$SESSIONS_DB" "SELECT model FROM sessions WHERE conversation_id='$CONV_ID';" 2>/dev/null)
if [ "$db_model" = "$MODEL" ]; then
    pass "session model matches ($MODEL)"
else
    fail "session model mismatch (expected $MODEL, got $db_model)"
fi

db_status=$(sqlite3 "$SESSIONS_DB" "SELECT status FROM sessions WHERE conversation_id='$CONV_ID';" 2>/dev/null)
if [ "$db_status" = "completed" ]; then
    pass "session status is 'completed'"
else
    fail "session status mismatch (expected completed, got $db_status)"
fi

db_pressure=$(sqlite3 "$SESSIONS_DB" "SELECT pressure_at_start FROM sessions WHERE conversation_id='$CONV_ID';" 2>/dev/null)
if [ "$db_pressure" = "clear" ]; then
    pass "pressure_at_start recorded as 'clear'"
else
    fail "pressure_at_start mismatch (expected clear, got $db_pressure)"
fi

tool_count=$(sqlite3 "$SESSIONS_DB" "SELECT COUNT(*) FROM tool_calls WHERE conversation_id='$CONV_ID';" 2>/dev/null)
if [ "$tool_count" -ge 4 ] 2>/dev/null; then
    pass "tool_calls recorded ($tool_count entries)"
else
    fail "tool_calls count too low (expected >=4, got $tool_count)"
fi

tool_call_count_col=$(sqlite3 "$SESSIONS_DB" "SELECT tool_call_count FROM sessions WHERE conversation_id='$CONV_ID';" 2>/dev/null)
if [ "$tool_call_count_col" -ge 4 ] 2>/dev/null; then
    pass "sessions.tool_call_count updated ($tool_call_count_col)"
else
    fail "sessions.tool_call_count wrong (expected >=4, got $tool_call_count_col)"
fi

sample_count=$(sqlite3 "$SESSIONS_DB" "SELECT COUNT(*) FROM pressure_samples WHERE conversation_id='$CONV_ID';" 2>/dev/null)
if [ "$sample_count" -ge 4 ] 2>/dev/null; then
    pass "pressure_samples recorded ($sample_count entries)"
else
    fail "pressure_samples count too low (expected >=4, got $sample_count)"
fi

distinct_pressures=$(sqlite3 "$SESSIONS_DB" "SELECT COUNT(DISTINCT pressure) FROM pressure_samples WHERE conversation_id='$CONV_ID';" 2>/dev/null)
if [ "$distinct_pressures" -ge 2 ] 2>/dev/null; then
    pass "multiple pressure levels sampled ($distinct_pressures distinct)"
else
    fail "expected multiple distinct pressure levels (got $distinct_pressures)"
fi

roi=$(sqlite3 "$SESSIONS_DB" "SELECT roi_score FROM sessions WHERE conversation_id='$CONV_ID';" 2>/dev/null)
if [ -n "$roi" ] && [ "$roi" != "" ]; then
    pass "ROI score computed ($roi)"
else
    fail "ROI score is null or missing"
fi

ended_at=$(sqlite3 "$SESSIONS_DB" "SELECT ended_at FROM sessions WHERE conversation_id='$CONV_ID';" 2>/dev/null)
if [ -n "$ended_at" ] && [ "$ended_at" != "" ]; then
    pass "session ended_at timestamp set"
else
    fail "session ended_at not set"
fi

duration=$(sqlite3 "$SESSIONS_DB" "SELECT duration_ms FROM sessions WHERE conversation_id='$CONV_ID';" 2>/dev/null)
if [ -n "$duration" ] && [ "$duration" != "" ]; then
    pass "session duration_ms computed ($duration ms)"
else
    fail "session duration_ms not computed"
fi

# ─── Summary ────────────────────────────────────────────────────────────

echo -e "\n${BOLD}${CYAN}════════════════════════════════════════${NC}"
echo -e "${BOLD}E2E Agent Lifecycle Test Results${NC}"
echo -e "${CYAN}════════════════════════════════════════${NC}"
echo -e "${GREEN}Passed: ${#PASSES[@]}${NC}"
echo -e "${RED}Failed: ${#FAILURES[@]}${NC}"

if [ ${#FAILURES[@]} -gt 0 ]; then
    echo -e "\n${RED}Failed tests:${NC}"
    for f in "${FAILURES[@]}"; do
        echo -e "  ${RED}x${NC} $f"
    done
fi

echo ""

if [ ${#FAILURES[@]} -eq 0 ]; then
    echo -e "${GREEN}${BOLD}All e2e tests passed.${NC}"
    exit 0
else
    echo -e "${RED}${BOLD}${#FAILURES[@]} e2e test(s) failed.${NC}"
    exit 1
fi
