# Guardian

System resource monitor daemon for macOS that protects Cursor agent sessions from resource exhaustion.

## Components

| Component | Path | Purpose |
|---|---|---|
| `guardiand` | `src/` | Rust daemon — samples CPU, memory, swap, thermal, Docker stats every 2s |
| Cursor hooks | `hooks/` | Shell scripts — inject system pressure advisories into agent context |
| Guardian.app | `app/` | SwiftUI menu bar app — visualize metrics, manage Docker, view sessions |
| Stress tests | `tests/stress/` | Containerized stress/chaos tests for the daemon |

## How It Works

1. **`guardiand`** samples system metrics every 2 seconds and classifies pressure as `clear`, `strained`, or `critical`
2. State is written atomically to `~/.guardian/state.json` with symlink protection
3. **Cursor hooks** read the state file and inject advisory messages into agent sessions
4. At `strained`/`critical` pressure, the daemon directly enforces limits:
   - Docker CPU throttling on non-essential containers
   - Fork guard warnings and optional process killing for runaway spawns
5. Hooks never deny tool calls — they only inform. The daemon handles enforcement.

## Quick Start

### Prerequisites

- macOS 14+ (Sonoma)
- Rust toolchain (`rustup`)
- `jq` and `sqlite3` (pre-installed on macOS)

### Install

```bash
# 1. Build and install the daemon as a LaunchAgent
bash scripts/install.sh

# 2. Install Cursor hooks globally
bash hooks/install-hooks.sh
```

Both steps are required. The daemon monitors system resources and writes
pressure state to `~/.guardian/state.json`. The hooks read that state and
inject it into Cursor agent sessions — without global hook installation,
agents have no visibility into system pressure.

### What Global Hook Installation Does

`hooks/install-hooks.sh` performs three things:

1. Copies hook scripts to `~/.cursor/hooks/guardian/`
2. Merges Guardian's hook registrations into `~/.cursor/hooks.json` (backs up the original to `hooks.json.bak`)
3. Rewrites relative command paths to absolute paths so Cursor can find them from any workspace

After installation, every new Cursor agent session will display:

```
[Guardian] Agent registered and monitored (daemon active).
System resources: nominal (CPU: 15%, Memory: 6GB free).
```

If the daemon is not running, agents see:

```
[Guardian] Agent registered (daemon not detected — resource data unavailable).
```

This message appears in the `sessionStart` hook's `additional_context`.
The `preToolUse` and `subagentStart` hooks also inject advisory messages
when pressure is `strained` or `critical`, guiding agents to throttle
their own parallelism.

### Verify

```bash
# Check the daemon is running
launchctl list com.guardian.guardiand
cat ~/.guardian/state.json | jq .pressure

# Check hooks are registered globally
cat ~/.cursor/hooks.json | jq '.hooks | keys'

# Check hook scripts are installed
ls ~/.cursor/hooks/guardian/
```

### Configure

Edit `~/.guardian/config.toml`:

```toml
sample_interval_secs = 2

[thresholds]
strained_cpu_percent = 70.0
critical_cpu_percent = 90.0
strained_memory_gb = 2.0
critical_memory_gb = 1.0

[docker]
essential_containers = ["postgres"]
auto_throttle = true

[fork_guard]
enabled = true
kill_enabled = false
```

Restart the daemon after config changes:

```bash
launchctl unload ~/Library/LaunchAgents/com.guardian.guardiand.plist
launchctl load ~/Library/LaunchAgents/com.guardian.guardiand.plist
```

### Uninstall

```bash
# Remove the daemon
bash scripts/uninstall.sh

# Remove global hooks
rm -rf ~/.cursor/hooks/guardian/
# Edit ~/.cursor/hooks.json to remove Guardian entries, or restore the backup:
cp ~/.cursor/hooks.json.bak ~/.cursor/hooks.json

# Remove all Guardian data
rm -rf ~/.guardian/
```

## Development

```bash
# Build
cargo build

# Run unit tests
cargo test

# Run containerized hook tests (requires Docker)
docker compose -f tests/stress/docker-compose.test.yml run --rm guardian-test

# Run e2e agent lifecycle test (requires Docker)
docker compose -f tests/stress/docker-compose.test.yml run --rm guardian-e2e

# Build release
cargo build --release
```

## Pressure Levels

| Level | CPU | Memory Available | Swap Used |
|---|---|---|---|
| `clear` | < 70% | > 2 GB | < 25% |
| `strained` | 70-90% | 1-2 GB | 25-50% |
| `critical` | > 90% | < 1 GB | > 50% |

Thermal state (`Serious`/`Critical`) and process usage ratio (> 0.6 / > 0.8) also trigger escalation.

Hysteresis prevents oscillation: escalation requires 2 consecutive samples, de-escalation requires 3.
