# Guardian Daemon Adversarial Stress Test Findings

**Date**: 2026-03-22
**Tester**: Adversarial stress test suite (`tests/stress/`)
**Target**: `guardiand` v0.1.0 + Cursor hooks + Docker throttling

## Summary

**12 failures discovered across 22 test scenarios.** The daemon successfully detects system pressure and writes state, but has critical gaps in enforcement, fault tolerance, and platform compatibility.

| Severity | Count | Category |
|----------|-------|----------|
| Critical | 4     | Security bypass, fail-open, platform incompatibility |
| High     | 4     | Design gaps allowing resource abuse |
| Medium   | 3     | Robustness and data integrity |
| Low      | 1     | Cosmetic / test infrastructure |

---

## Critical Failures

### F1: Fail-Open on Missing state.json (Security)

**Severity**: Critical
**Test**: `test_06_state_json_deletion`
**Reproduction**:

```bash
rm -f ~/.guardian/state.json
echo '{"tool":"Shell"}' | bash hooks/guardian/pre-tool-use.sh
# Output: {"permission": "allow"}
```

**Root cause**: `lib.sh:read_state_pressure()` returns "clear" when `state.json` doesn't exist (line 11-13). The `*) allow` case in the hook's `case` statement then permits execution. If the daemon crashes, is not installed, or `state.json` is deleted by a malicious process, all protection disappears.

**Impact**: An agent or process can bypass all throttling by simply deleting `~/.guardian/state.json`.

**Fix**: Hooks should default to "deny" (fail-closed) when `state.json` is missing or older than a configurable staleness window (e.g., 10 seconds).

---

### F2: Corruption Bypass (Security)

**Severity**: Critical
**Test**: `test_07_state_json_corruption`
**Reproduction**:

```bash
echo "NOT VALID JSON {{{" > ~/.guardian/state.json
echo '{"tool":"Shell"}' | bash hooks/guardian/pre-tool-use.sh
# Output: {"permission": "allow"}
```

**Root cause**: Same as F1 — `jq -r '.pressure // "clear"'` returns "clear" when the JSON is malformed (jq fails, the `||` in the subshell returns empty string, the `${pressure:-clear}` default kicks in).

**Impact**: Corrupted state file = no protection.

**Fix**: Same as F1 — fail-closed default.

---

### F3: Fork Guard Broken on macOS (Platform)

**Severity**: Critical
**Test**: `TEST A: Fork guard kill_newest_processes broken on macOS`
**Reproduction**:

```bash
ps -u "$(id -u)" -o pid= --sort=-start_time
# ps: illegal option -- -
# usage: ps [-AaCcEefhjlMmrSTvwXx] ...
```

**Root cause**: `kill_newest_processes()` in `main.rs:183` uses `--sort=-start_time`, which is a GNU coreutils flag. macOS `ps` does not support `--sort`. The function silently returns without killing anything.

**Impact**: The fork bomb guard's kill mechanism is completely non-functional on macOS. Runaway process spawning will never be stopped by the daemon.

**Fix**: Use macOS-compatible `ps` flags: `ps -u $UID -o pid=,lstart=` and sort in Rust, or use `sysctl kern.proc` directly.

---

### F4: Garbage Pressure Value Bypasses Hooks (Security)

**Severity**: Critical
**Test**: `TEST D: Garbage pressure value`
**Reproduction**:

```bash
echo '{"pressure":"anything_else"}' > ~/.guardian/state.json
echo '{"tool":"Shell"}' | bash hooks/guardian/pre-tool-use.sh
# Output: {"permission": "allow"}
```

**Root cause**: The hook `case` statement has `critical)`, `strained)`, `*) allow`. Any value other than exactly "critical" or "strained" — including typos, empty strings, "CRITICAL" (wrong case), or garbage — falls to the `*)` allow branch.

**Impact**: If the daemon ever writes an unexpected pressure value (bug, serialization change, new enum variant), hooks silently allow everything.

**Fix**: Invert the logic — explicitly allow only on "clear", deny on everything else.

---

## High-Severity Failures

### F5: New Containers Escape Throttle (Design Gap)

**Severity**: High
**Test**: `TEST B: New container started after critical transition escapes throttle`
**Reproduction**:

```bash
# 1. Start stress containers to trigger critical
docker compose -f tests/stress/docker-compose.stress.yml up -d cpu-hog workload-a
# 2. Wait for daemon to transition to critical and throttle
sleep 15
# 3. Start a new container AFTER the transition
docker compose -f tests/stress/docker-compose.stress.yml --profile late up -d late-joiner
# 4. Check its CPU limit
docker inspect late-joiner --format '{{.HostConfig.NanoCpus}}'
# Output: 0 (NO throttle)
```

**Root cause**: `handle_docker_throttle()` in `main.rs:140` only fires on the `Critical if !*throttled` transition. Once `docker_throttled = true`, the function skips all critical iterations. New containers started after the transition are never seen.

**Impact**: Users or agents can launch new resource-hungry containers that completely bypass throttling.

**Fix**: On every critical sample (not just the transition), scan for un-throttled containers and apply limits. Alternatively, re-scan container list periodically while in critical state.

---

### F6: Essential Container Name Spoofing (Design Gap)

**Severity**: High
**Test**: `test_10_essential_container_name_spoofing`
**Reproduction**:

```rust
// In throttle.rs:52
if essential.iter().any(|e| name.contains(e)) { continue; }
// A container named "evil-postgres-miner" matches "postgres"
```

**Root cause**: `contains()` does substring matching. Any container with "postgres" anywhere in its name is treated as essential.

**Impact**: Malicious or accidental container naming can bypass throttling entirely.

**Fix**: Use exact match or prefix match: `name == e || name.starts_with(&format!("{e}-"))`.

---

### F7: Strained State Provides No Docker Protection (Design Gap)

**Severity**: High
**Test**: `test_12_strained_no_docker_throttle`

**Root cause**: Docker throttling only engages at `Critical`. At `Strained`, the pre-tool-use hook ALLOWS operations and Docker containers are not throttled. The system is degrading but nothing prevents further degradation.

**Impact**: Between 70-90% CPU (or 1-2GB memory available), the system is strained but Docker containers continue burning resources unchecked. Hooks allow tool calls with only a warning message. The system can slide from strained to critical without intervention.

**Fix**: Apply soft Docker throttling at strained (e.g., CPU cap of 0.75 instead of 0.5 at critical).

---

### F8: Symlink Attack on state.json (Security)

**Severity**: High
**Test**: `TEST E: Symlink attack`
**Reproduction**:

```bash
echo '{"pressure":"clear","cpu_percent":0.0}' > /tmp/fake-state.json
rm ~/.guardian/state.json
ln -sf /tmp/fake-state.json ~/.guardian/state.json
echo '{"tool":"Shell"}' | bash hooks/guardian/pre-tool-use.sh
# Output: {"permission": "allow"}
```

**Root cause**: Neither the daemon nor the hooks verify that `state.json` is a regular file (not a symlink). The daemon writes via atomic rename (`tmp -> final`), but if `final` is a symlink, `fs::rename` follows it.

**Impact**: A process with write access to `~/.guardian/` can redirect state reads to an attacker-controlled file.

**Fix**: Check `!state_file_path().is_symlink()` before reading in hooks. In the daemon, verify the target isn't a symlink before writing, or use `O_NOFOLLOW`.

---

## Medium-Severity Failures

### F9: Daemon Crash Leaves Stale State (Robustness)

**Severity**: Medium
**Test**: `TEST F: Daemon crash leaves system unprotected`

**Root cause**: After the daemon is killed, `state.json` persists with the last-written state. Hooks read this stale file with no staleness check. If the last state was "critical", hooks continue denying (over-protective). If it was "clear", hooks continue allowing (under-protective).

**Impact**: Stale state persists indefinitely. The daemon may not restart promptly (launchd has retry delays). During the gap, hooks operate on phantom data.

**Fix**: Add a `sampled_at` staleness check in hooks — if the timestamp is older than 30 seconds, treat as "daemon down" and fail-closed.

---

### F10: Concurrent SQLite Writes Fail Silently (Data Integrity)

**Severity**: Medium
**Test**: `TEST G: Concurrent sqlite3 writes from multiple hooks`
**Result**: 0 out of 20 concurrent writes succeeded.

**Root cause**: `db_exec()` in `lib.sh:41` uses `sqlite3 "$SESSIONS_DB" "$sql" 2>/dev/null || true`. The `|| true` and `2>/dev/null` silently swallow all errors, including `SQLITE_BUSY` from concurrent access. The sqlite3 CLI has no built-in retry or WAL mode.

**Impact**: Session tracking data is unreliable. Tool calls, pressure samples, and session records may be silently dropped under concurrent hook execution.

**Fix**: Enable WAL mode in `ensure_db()`: `PRAGMA journal_mode=WAL;`. Add retry logic in `db_exec()`: loop with `.timeout 5000` or up to 3 retries.

---

### F11: Partial/Incomplete state.json Defaults to Allow (Robustness)

**Severity**: Medium
**Test**: `TEST C: Missing pressure field defaults to allow`
**Reproduction**:

```bash
echo '{"cpu_percent": 99.0}' > ~/.guardian/state.json
# pressure field is missing
echo '{"tool":"Shell"}' | bash hooks/guardian/pre-tool-use.sh
# Output: {"permission": "allow"}
```

**Root cause**: `jq -r '.pressure // "clear"'` treats a missing field the same as "clear". The hook allows execution even though the state file indicates severe conditions (99% CPU, 0.1GB memory).

**Impact**: Daemon bugs, partial writes, or schema changes that omit the pressure field result in no protection.

**Fix**: Validate that required fields exist before trusting the state. Fail-closed if pressure field is missing.

---

## Low-Severity Issues

### F12: No Hysteresis / Debounce on Throttle Transitions

**Severity**: Low (did not manifest in testing due to test duration, but exists by code inspection)

**Root cause**: The classifier has no hysteresis band. If CPU oscillates around 90% (the default critical threshold), the daemon flips between critical and strained every 2 seconds. Each flip triggers a Docker throttle/unthrottle cycle, which itself consumes resources.

**Impact**: Rapid throttle oscillation during borderline conditions. Docker `update` commands fire repeatedly.

**Fix**: Add hysteresis: require N consecutive samples above threshold before escalating, and N consecutive samples below threshold before de-escalating.

---

## Passes (10 scenarios that worked correctly)

1. Daemon starts and produces state.json
2. CPU stress triggers strained/critical classification
3. pre-tool-use denies on explicit critical state
4. subagent-start denies on explicit critical state
5. pre-tool-use allows on explicit clear state
6. No TOCTOU race in serial test (hooks read current file)
7. 200 rapid hook invocations complete in 6s with 0 errors
8. Atomic state.json writes prevent corrupt reads (0/100 corrupt)
9. Docker compose stress containers run and create load
10. Daemon correctly detects and logs pressure transitions

---

## Attack Surface Map

```
┌─────────────────────────────────────────────────────────┐
│                    Attack Surfaces                        │
├──────────────────┬──────────────────────────────────────┤
│ state.json       │ F1: Delete → fail-open               │
│                  │ F2: Corrupt → fail-open              │
│                  │ F4: Garbage pressure → fail-open     │
│                  │ F8: Symlink → read attacker data     │
│                  │ F9: Stale after crash → phantom data │
│                  │ F11: Missing fields → fail-open      │
├──────────────────┼──────────────────────────────────────┤
│ Hook logic       │ F4: *) case = allow-by-default       │
│                  │ F10: Silent DB write failures        │
├──────────────────┼──────────────────────────────────────┤
│ Docker throttle  │ F5: New containers escape throttle   │
│                  │ F6: Name spoofing via contains()     │
│                  │ F7: No throttle at strained          │
│                  │ F12: No hysteresis on transitions    │
├──────────────────┼──────────────────────────────────────┤
│ Fork guard       │ F3: ps --sort broken on macOS        │
├──────────────────┼──────────────────────────────────────┤
│ Daemon lifecycle │ F9: No staleness check               │
│                  │ F9: No watchdog beyond launchd       │
└──────────────────┴──────────────────────────────────────┘
```

## Recommendations (Priority Order)

1. **Fail-closed hooks**: Default to deny when state.json is missing, corrupt, stale, or has unexpected values. Only allow on explicit "clear" with a fresh timestamp.
2. **Continuous Docker scanning**: Re-scan container list on every critical sample, not just on transition.
3. **Fix fork guard for macOS**: Replace GNU `ps --sort` with macOS-compatible process enumeration.
4. **Exact-match essential containers**: Replace `contains()` with exact or prefix match.
5. **Staleness check**: Hooks reject state older than 30 seconds.
6. **WAL mode + retry for SQLite**: Prevent silent data loss under concurrent access.
7. **Hysteresis**: Require 3+ consecutive samples before escalating/de-escalating pressure.
8. **Strained-level Docker throttle**: Apply soft limits before critical.
9. **Symlink protection**: Verify state.json is a regular file, not a symlink.
10. **State validation**: Require pressure field to be one of the known enum values.
