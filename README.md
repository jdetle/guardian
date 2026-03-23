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
# Build and install the daemon + LaunchAgent
bash scripts/install.sh

# Install Cursor hooks
bash hooks/install-hooks.sh
```

### Verify

```bash
launchctl list com.guardian.guardiand
cat ~/.guardian/state.json | jq .pressure
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

### Uninstall

```bash
bash scripts/uninstall.sh
rm -rf ~/.cursor/hooks/guardian/
rm -rf ~/.guardian/
```

## Development

```bash
# Build
cargo build

# Run tests (48 unit tests covering classifier, hysteresis, etime parser, etc.)
cargo test

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
