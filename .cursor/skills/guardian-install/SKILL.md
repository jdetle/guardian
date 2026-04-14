# Guardian Install

Install and configure the System Guardian daemon, Cursor hooks, and optional SwiftUI app.

## Prerequisites

- macOS 14+ (Sonoma or later)
- Rust toolchain (`rustup` installed)
- `jq` and `sqlite3` (pre-installed on macOS)
- Xcode (only for the SwiftUI app — the daemon and hooks work without it)

## Installation Steps

### 1. Build and Install the Daemon

```bash
bash scripts/install.sh
```

This script:
1. Builds `guardiand` and the `guardian` CLI in release mode (CLI copied to `~/.guardian/guardian`)
2. Creates `~/.guardian/` with a default `config.toml`
3. Symlinks `guardian` onto your PATH when possible (`/usr/local/bin/guardian`, or `~/.local/bin/guardian` if `/usr/local/bin` is not writable—add `export PATH="$HOME/.local/bin:$PATH"` to `~/.zshrc` if needed)
4. Installs a LaunchAgent plist to `~/Library/LaunchAgents/`
5. Starts the daemon immediately

Verify it's running:

```bash
launchctl list com.guardian.guardiand
cat ~/.guardian/state.json | jq .pressure
```

### 2. Install Cursor Hooks

```bash
bash hooks/install-hooks.sh
```

This script:
1. Copies hook scripts to `~/.cursor/hooks/guardian/`
2. Creates or merges `~/.cursor/hooks.json` with Guardian registrations
3. Resolves paths to absolute references

Verify hooks are registered:

```bash
cat ~/.cursor/hooks.json | jq '.hooks | keys'
```

### 3. Build the SwiftUI App (Optional)

Requires Xcode (not just CommandLineTools):

```bash
cd app
swift build -c release
```

The binary will be at `.build/release/Guardian`. To install as a macOS app:

```bash
# Copy to Applications (or create a proper .app bundle)
cp .build/release/Guardian /usr/local/bin/guardian-app
```

Or open in Xcode for a proper .app bundle build:

```bash
open Package.swift
# Then Product → Archive → Distribute
```

### 4. Configure Docker Resource Limits

The `docker-compose.yml` already includes resource limits. Verify they're active:

```bash
docker compose config | grep -A 4 "resources:"
```

### 5. Customize Configuration

Edit `~/.guardian/config.toml` to adjust thresholds:

```toml
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

After editing, restart the daemon:

```bash
launchctl unload ~/Library/LaunchAgents/com.guardian.guardiand.plist
launchctl load ~/Library/LaunchAgents/com.guardian.guardiand.plist
```

## Uninstallation

### Remove the daemon

```bash
bash scripts/uninstall.sh
```

### Remove hooks

```bash
rm -rf ~/.cursor/hooks/guardian/
# Edit ~/.cursor/hooks.json to remove guardian entries
```

### Remove data

```bash
rm -rf ~/.guardian/
```

## Troubleshooting

### Daemon not writing state.json

Check logs:

```bash
cat ~/.guardian/guardiand.stderr.log
```

Common issues:
- Docker socket not found (Docker Desktop not running)
- Permission denied on `~/.guardian/` directory

### Hooks not firing

1. Verify `~/.cursor/hooks.json` exists and is valid JSON
2. Check that hook scripts are executable: `ls -la ~/.cursor/hooks/guardian/`
3. Restart Cursor to reload hooks

### High CPU from guardiand itself

Increase the sample interval in `config.toml`:

```toml
sample_interval_secs = 5
```

## Components

| Component | Location | Purpose |
|---|---|---|
| `guardiand` | `src/` | Rust daemon — samples system metrics, classifies pressure |
| Cursor hooks | `hooks/` | Shell scripts — inform agents of system pressure |
| Guardian.app | `app/` | SwiftUI app — visualize metrics, manage Docker |
| Docker limits | `docker-compose.yml` | Resource caps on all compose services |
| Cursor rule | `.cursor/rules/system-guardian.mdc` | Agent behavior guidance |
