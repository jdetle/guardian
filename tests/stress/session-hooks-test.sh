#!/bin/bash
# Session-tracking hooks for Cursor, Claude Code and Codex: do they record a session's life
# in sessions.db, and do they do it without ever saying anything to the agent?
#
# Safe to run anywhere. It makes a scratch HOME of its own and every hook it runs reads and
# writes only there: nothing touches a real ~/.guardian, and no daemon is started. CI runs it
# on macOS under /bin/bash 3.2 and inside the hook container.
#
#   bash tests/stress/session-hooks-test.sh
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
if [ -z "${HOOKS_DIR:-}" ] || [ ! -f "${HOOKS_DIR:-}/session-event.sh" ]; then
    HOOKS_DIR="$(cd "$HERE/../../hooks" 2>/dev/null && pwd)"
fi
if [ ! -f "$HOOKS_DIR/session-event.sh" ]; then
    echo "session-hooks-test: hooks not found (HOOKS_DIR=$HOOKS_DIR)" >&2
    exit 2
fi
for tool in jq sqlite3; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "session-hooks-test: $tool is required" >&2
        exit 2
    fi
done

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
export HOME="$SCRATCH"
unset GUARDIAN_HOOK_FORMAT CLAUDE_CONFIG_DIR CODEX_HOME
G="$HOME/.guardian"
DB="$G/sessions.db"

PASSED=0
FAILED=0
pass() { PASSED=$((PASSED + 1)); echo "  PASS: $1"; }
fail() { FAILED=$((FAILED + 1)); echo "  FAIL: $1"; }
# check <what> <expected> <actual>
check() {
    if [ "$2" = "$3" ]; then
        pass "$1"
    else
        fail "$1 (expected [$2], got [$3])"
    fi
}

fresh_home() {
    rm -rf "$G"
    mkdir -p "$G"
    cp "$HOOKS_DIR/hook_policy.default.json" "$G/hook_policy.json"
}
# write_state <pressure> [seconds ago]
write_state() {
    local ago="${2:-0}" now
    now=$(( $(date +%s) - ago ))
    local stamp
    stamp=$(date -u -r "$now" +%Y-%m-%dT%H:%M:%S 2>/dev/null || date -u -d "@$now" +%Y-%m-%dT%H:%M:%S)
    printf '{"pressure":"%s","pressure_reasons":["test"],"cpu_percent":42.5,"memory_available_gb":1.5,"swap_used_percent":10,"docker":{"total_cpu_percent":3},"sampled_at":"%s.000000+00:00"}\n' \
        "$1" "$stamp" > "$G/state.json"
}
# hook <script relative to hooks/> <json>   sets HOOK_OUT (what it printed) and HOOK_RC
HOOK_OUT=""
HOOK_RC=0
hook() {
    HOOK_OUT=$(printf '%s' "$2" | bash "$HOOKS_DIR/$1" 2>/dev/null)
    HOOK_RC=$?
}
q() { sqlite3 "$DB" "$1" 2>/dev/null; }

# ─── Claude Code: a whole session ───────────────────────────────────────
echo "=== Claude Code: a session from start to end ==="
fresh_home
write_state strained
hook claude/session-start.sh '{"session_id":"c1","source":"startup","model":"claude-opus-5-5","cwd":"/work/my app"}'; out=$HOOK_OUT
check "session start still answers with context" "SessionStart" "$(printf '%s' "$out" | jq -r '.hookSpecificOutput.hookEventName' 2>/dev/null)"
check "start: the session is Claude's, idle, with its model and folder" "claude|idle|claude-opus-5-5|/work/my app" "$(q "select client, state, model, cwd from sessions where conversation_id='c1'")"

hook claude/user-prompt-submit.sh '{"session_id":"c1","prompt":"hello","cwd":"/work/my app"}'; out=$HOOK_OUT
check "prompt: the gate still answers" "true" "$(printf '%s' "$out" | jq -r '.continue' 2>/dev/null)"
check "prompt: the session is working" "working|1" "$(q "select state, prompt_count from sessions where conversation_id='c1'")"

for tool in Bash Read Agent; do
    hook claude/post-tool-use.sh "{\"session_id\":\"c1\",\"tool_name\":\"$tool\",\"duration_ms\":120,\"cwd\":\"/work/my app\"}"; out=$HOOK_OUT
    check "tool $tool: records and says nothing (exit $HOOK_RC)" "0|" "$HOOK_RC|$out"
done
check "tools: three calls, one of them a subagent" "3|1" "$(q "select tool_call_count, subagent_count from sessions where conversation_id='c1'")"
check "tools: one row each, with the time they took" "3|360" "$(q "select count(*), sum(duration_ms) from tool_calls where conversation_id='c1'")"
check "tools: the load each ran under is sampled" "3|strained|42.5" "$(q "select count(*), min(pressure), max(cpu_percent) from pressure_samples where conversation_id='c1'")"

hook claude/stop.sh '{"session_id":"c1","stop_hook_active":false,"last_assistant_message":"done"}'; out=$HOOK_OUT
check "stop: records and says nothing (a stop hook that speaks keeps the agent going)" "0|" "$HOOK_RC|$out"
check "stop: the turn is over, the session is idle and scored" "idle|1|completed|strained|1" "$(q "select state, turn_count, status, pressure_avg, roi_score is not null from sessions where conversation_id='c1'")"

hook claude/user-prompt-submit.sh '{"session_id":"c1","prompt":"again"}' >/dev/null
check "a second prompt: working again" "working|2" "$(q "select state, prompt_count from sessions where conversation_id='c1'")"
hook claude/stop.sh '{"session_id":"c1"}' >/dev/null

hook claude/session-end.sh '{"session_id":"c1","reason":"prompt_input_exit"}'; out=$HOOK_OUT
check "end: records and says nothing" "0|" "$HOOK_RC|$out"
check "end: the session is over, and how its last turn went is kept" "ended|completed|2|1" "$(q "select state, status, turn_count, ended_at is not null from sessions where conversation_id='c1'")"

# The tool hook runs in the background in Claude Code and Codex and can land last.
hook claude/post-tool-use.sh '{"session_id":"c1","tool_name":"Bash"}'
check "a tool call recorded after the session ended does not bring it back" "ended|4" "$(q "select state, tool_call_count from sessions where conversation_id='c1'")"

hook claude/session-start.sh '{"session_id":"c1","source":"resume"}' >/dev/null
check "resumed: an ended session is idle again, still one row" "idle|1" "$(q "select state, (select count(*) from sessions) from sessions where conversation_id='c1'")"

# ─── Codex ──────────────────────────────────────────────────────────────
echo "=== Codex ==="
hook codex/post-tool-use.sh '{"session_id":"x1","model":"gpt-5.5","cwd":"/work/x","tool_name":"spawn_agent","turn_id":"t1"}'; out=$HOOK_OUT
check "a session first heard of mid-way gets a row (hooks installed while it ran)" "codex|working|gpt-5.5|/work/x|1|1" "$(q "select client, state, model, cwd, tool_call_count, subagent_count from sessions where conversation_id='x1'")"
check "tool: says nothing" "0|" "$HOOK_RC|$out"
hook codex/session-start.sh '{"session_id":"x2","source":"startup","cwd":"/work/y"}' >/dev/null
hook codex/post-tool-use.sh '{"session_id":"x2","model":"gpt-5.5-codex","tool_name":"shell"}' >/dev/null
check "the model, unknown at the start, is filled in from a later event" "gpt-5.5-codex" "$(q "select model from sessions where conversation_id='x2'")"
hook codex/stop.sh '{"session_id":"x2","turn_id":"t1","stop_hook_active":false}'; out=$HOOK_OUT
check "stop: says nothing" "0|" "$HOOK_RC|$out"
hook codex/session-end.sh '{"session_id":"x2","reason":"other"}'; out=$HOOK_OUT
check "end: says nothing, and the session is over" "0||ended|completed" "$HOOK_RC|$out|$(q "select state, status from sessions where conversation_id='x2'")"

# ─── Cursor ─────────────────────────────────────────────────────────────
echo "=== Cursor ==="
hook session-start.sh '{"conversation_id":"u1","session_id":"u1","model":"composer-2","workspace_roots":["/work/u","/work/other"]}' >/dev/null
check "start: Cursor's, with the first workspace folder" "cursor|idle|composer-2|/work/u" "$(q "select client, state, model, cwd from sessions where conversation_id='u1'")"
hook before-submit-prompt.sh '{"conversation_id":"u1","prompt":"hi","workspace_roots":["/work/u"]}'; out=$HOOK_OUT
check "prompt: Cursor's gate still answers, and the session is working" "true|working|1" "$(printf '%s' "$out" | jq -r '.continue' 2>/dev/null)|$(q "select state, prompt_count from sessions where conversation_id='u1'")"
hook post-tool-use.sh '{"conversation_id":"u1","tool_name":"Task","duration":900,"model":"composer-2"}'; out=$HOOK_OUT
check "tool: records Cursor's duration field and says nothing" "0||1|1|900" "$HOOK_RC|$out|$(q "select tool_call_count, subagent_count, (select duration_ms from tool_calls where conversation_id='u1') from sessions where conversation_id='u1'")"
hook stop.sh '{"conversation_id":"u1","status":"aborted","loop_count":3}'; out=$HOOK_OUT
check "stop: how the turn went and how many loops it took" "0||idle|aborted|3|1" "$HOOK_RC|$out|$(q "select state, status, loop_count, turn_count from sessions where conversation_id='u1'")"
hook session-end.sh '{"conversation_id":"u1","reason":"window_close","final_status":"completed","duration_ms":5000}'; out=$HOOK_OUT
check "end: over, without overwriting how the last turn went" "0||ended|aborted" "$HOOK_RC|$out|$(q "select state, status from sessions where conversation_id='u1'")"

fresh_home
write_state critical
jq '.prompt_gate.mode = "strict"' "$HOOKS_DIR/hook_policy.default.json" > "$G/hook_policy.json"
: > "$G/blocking_on"
hook before-submit-prompt.sh '{"conversation_id":"ub","prompt":"run the full test suite"}'
check "Cursor: a prompt Guardian paused does not mark its session working" "false|" "$(printf '%s' "$HOOK_OUT" | jq -r '.continue' 2>/dev/null)|$(q "select state from sessions where conversation_id='ub'")"

# ─── What must not be recorded ──────────────────────────────────────────
echo "=== What is not recorded ==="
fresh_home
write_state critical
jq '.prompt_gate.mode = "strict"' "$HOOKS_DIR/hook_policy.default.json" > "$G/hook_policy.json"
: > "$G/blocking_on"   # pausing is opt-in; this case is about a prompt that is paused
hook claude/user-prompt-submit.sh '{"session_id":"b1","prompt":"cargo build --release and run the whole test suite","cwd":"/w"}'; out=$HOOK_OUT
if [ "$(printf '%s' "$out" | jq -r '.decision // ""' 2>/dev/null)" = "block" ]; then
    check "a prompt Guardian paused does not mark its session working" "" "$(q "select state from sessions where conversation_id='b1'")"
else
    fail "expected the strict gate to block at critical load (got $out)"
fi

fresh_home
write_state strained 600
hook claude/post-tool-use.sh '{"session_id":"s1","tool_name":"Bash"}' >/dev/null
check "a state file ten minutes old gives no pressure sample (the tool call is still recorded)" "1|0" "$(q "select (select count(*) from tool_calls), (select count(*) from pressure_samples)")"

rm -f "$G/state.json"
hook claude/post-tool-use.sh '{"session_id":"s1","tool_name":"Bash"}' >/dev/null
check "no state file at all: still recorded, still no sample" "2|0" "$(q "select (select count(*) from tool_calls), (select count(*) from pressure_samples)")"

hook claude/post-tool-use.sh '{"tool_name":"Bash"}'; out=$HOOK_OUT
check "an event that names no session records nothing and does not fail" "0||1" "$HOOK_RC|$out|$(q "select count(*) from sessions")"
garbage_ok=yes
for bad in 'not json' '' '[]' '{"session_id":{"a":1}}' 'null'; do
    for script in codex/session-end.sh claude/post-tool-use.sh claude/stop.sh session-end.sh; do
        hook "$script" "$bad"; out=$HOOK_OUT
        if [ "$HOOK_RC" != "0" ] || [ -n "$out" ]; then
            garbage_ok=no
            fail "garbage input [$bad] to $script must exit 0 in silence (rc=$HOOK_RC out=$out)"
        fi
    done
done
check "garbage on stdin, every recording hook: exit 0, nothing printed" "yes" "$garbage_ok"

# A home where ~/.guardian cannot be made: nothing to record to, and still no failure.
real_home="$HOME"
export HOME="$SCRATCH/no-such-place/file"
: > "$SCRATCH/no-such-place" 2>/dev/null || true
for script in claude/post-tool-use.sh claude/stop.sh claude/session-end.sh; do
    hook "$script" '{"session_id":"n1","tool_name":"Bash"}'
    check "$script with nowhere to write: exit 0, nothing printed" "0|" "$HOOK_RC|$HOOK_OUT"
done
export HOME="$real_home"

# ─── Hostile values stay values ─────────────────────────────────────────
echo "=== Hostile input ==="
fresh_home
write_state clear
evil='{"session_id":"a'"'"'); DROP TABLE sessions;--","tool_name":"x'"'"'y","duration_ms":"9; DROP TABLE tool_calls","model":"m'"'"'); DELETE FROM sessions;--","cwd":"/w'"'"'; DROP TABLE sessions;--"}'
hook claude/post-tool-use.sh "$evil" >/dev/null
hook claude/session-start.sh "$evil" >/dev/null
hook claude/stop.sh "$evil" >/dev/null
hook claude/session-end.sh "$evil" >/dev/null
check "quotes and SQL in every field are stored as text; the tables are intact" "1|1|0|x'y" "$(q "select (select count(*) from sessions), (select count(*) from tool_calls), duration_ms, tool_name from tool_calls")"
check "the session id is kept exactly as sent" "a'); DROP TABLE sessions;--" "$(q "select conversation_id from sessions")"

# ─── Upgrading a database from before session tracking ──────────────────
echo "=== Upgrade from the earlier database ==="
fresh_home
write_state clear
sqlite3 "$DB" "
CREATE TABLE sessions (conversation_id TEXT PRIMARY KEY, model TEXT NOT NULL, started_at TEXT NOT NULL DEFAULT (datetime('now')), ended_at TEXT, status TEXT, loop_count INTEGER DEFAULT 0, tool_call_count INTEGER DEFAULT 0, pressure_at_start TEXT, pressure_avg TEXT, duration_ms INTEGER, files_modified INTEGER DEFAULT 0, lines_changed INTEGER DEFAULT 0, estimated_cost_usd REAL DEFAULT 0.0, roi_score REAL);
CREATE TABLE tool_calls (id INTEGER PRIMARY KEY AUTOINCREMENT, conversation_id TEXT NOT NULL, tool_name TEXT NOT NULL, duration_ms INTEGER, model TEXT, recorded_at TEXT NOT NULL DEFAULT (datetime('now')));
CREATE TABLE pressure_samples (id INTEGER PRIMARY KEY AUTOINCREMENT, conversation_id TEXT, pressure TEXT NOT NULL, cpu_percent REAL, memory_available_gb REAL, docker_cpu_percent REAL, sampled_at TEXT NOT NULL DEFAULT (datetime('now')));
INSERT INTO sessions (conversation_id, model, ended_at, tool_call_count, status) VALUES ('old-cursor', 'composer-2', datetime('now'), 4, 'completed');
INSERT INTO sessions (conversation_id, model) VALUES ('old-unknown', 'unknown');
INSERT INTO tool_calls (conversation_id, tool_name, recorded_at) VALUES ('old-cursor', 'Shell', datetime('now', '-90 days'));
INSERT INTO tool_calls (conversation_id, tool_name) VALUES ('old-cursor', 'Read');
"
hook claude/post-tool-use.sh '{"session_id":"new","tool_name":"Bash"}' >/dev/null
check "the first event brings the old database up to date and is recorded" "claude|working|1" "$(q "select client, state, tool_call_count from sessions where conversation_id='new'")"
check "earlier sessions that ended or used tools were Cursor's; they keep everything they had" "cursor|ended|completed|4" "$(q "select client, state, status, tool_call_count from sessions where conversation_id='old-cursor'")"
check "an earlier session that only ever started is left unlabelled" "|ended" "$(q "select coalesce(client, ''), state from sessions where conversation_id='old-unknown'")"
hook claude/session-start.sh '{"session_id":"new","source":"resume"}' >/dev/null
check "tool calls older than sixty days are cleared out at a session start" "0|2" "$(q "select (select count(*) from tool_calls where recorded_at < datetime('now', '-60 days')), (select count(*) from tool_calls)")"
# Run the upgrade again for real (it is skipped while its marker file is there), twice at once,
# as two hooks finishing together would.
rm -f "$G"/.sessions_schema_v*
printf '%s' '{"session_id":"new","tool_name":"Read"}' | bash "$HOOKS_DIR/claude/post-tool-use.sh" >/dev/null 2>&1 &
printf '%s' '{"session_id":"new","tool_name":"Grep"}' | bash "$HOOKS_DIR/claude/post-tool-use.sh" >/dev/null 2>&1 &
wait
check "upgrading again, from two hooks at once, loses nothing and relabels nothing" "3|3|cursor|" "$(q "select tool_call_count, (select count(*) from sessions), (select client from sessions where conversation_id='old-cursor'), (select coalesce(client, '') from sessions where conversation_id='old-unknown') from sessions where conversation_id='new'")"

echo ""
echo "session-hooks-test: $PASSED passed, $FAILED failed"
[ "$FAILED" -eq 0 ]
