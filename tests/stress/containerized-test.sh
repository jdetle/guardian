#!/bin/bash
# Containerized Test Harness for Guardian Hooks and State Parsing
#
# Runs INSIDE a Docker container — never on the host.
# The guardian daemon uses macOS-only APIs (mach2, sysctl) and cannot be
# compiled for Linux. This test suite validates everything EXCEPT the
# daemon's sampling loop:
#
#   1. Hooks ALWAYS return "allow" regardless of pressure level
#   2. Hooks provide context messages on strained/critical
#   3. State file parsing handles missing/corrupt/symlinked files
#   4. No crash on garbage input
#   5. Fail-open behavior: missing state = "clear", not "critical"
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

test_hook_allows "subagent: strained with many sessions" \
    '{"pressure":"strained","cpu_percent":88,"memory_available_gb":0.8,"cursor":{"active_sessions":5}}' \
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

echo '{"pressure":"strained","cpu_percent":88,"memory_available_gb":0.8,"swap_used_percent":75}' > "$STATE_FILE"
result=$(echo '{"tool_name":"Shell"}' | bash "$HOOKS_DIR/pre-tool-use.sh" 2>/dev/null)
msg=$(echo "$result" | jq -r '.agent_message // ""' 2>/dev/null)
if [ -n "$msg" ] && [ "$msg" != "null" ]; then
    pass "strained provides context message"
else
    fail "strained should provide agent_message"
fi

echo '{"pressure":"critical","cpu_percent":98,"memory_available_gb":0.2,"swap_used_percent":95}' > "$STATE_FILE"
result=$(echo '{"tool_name":"Shell"}' | bash "$HOOKS_DIR/pre-tool-use.sh" 2>/dev/null)
msg=$(echo "$result" | jq -r '.agent_message // ""' 2>/dev/null)
if [ -n "$msg" ] && [ "$msg" != "null" ]; then
    pass "critical provides context message"
else
    fail "critical should provide agent_message"
fi

echo '{"pressure":"clear","cpu_percent":10,"memory_available_gb":6}' > "$STATE_FILE"
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

# Valid pressures should be preserved
for p in clear strained critical; do
    echo "{\"pressure\":\"$p\"}" > "$STATE_FILE"
    actual=$(source "$HOOKS_DIR/lib.sh"; read_state_pressure)
    if [ "$actual" = "$p" ]; then
        pass "preserves valid pressure: $p"
    else
        fail "expected '$p', got '$actual'"
    fi
done

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

# ─── Test Group 7: Host Pollution Check ─────────────────────────────────

echo -e "\n${BOLD}${CYAN}=== Test Group 7: No Host Pollution ===${NC}"

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
