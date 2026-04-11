# Guardian

**Keep AI coding agents useful—and keep your Mac from melting down.**

Guardian is a macOS stack that closes the loop between *what your machine is doing* and *what Cursor agents are allowed to assume*. Agents get live pressure context in every session; the daemon enforces limits when things get hot. **Prompt-level gates** (optional) can pause sends when CPU/memory pressure or Cursor’s own memory use is too high, with **human-in-the-loop** resume paths.

---

## Why this exists

### Modest laptops and Cursor

Cursor on an **older MacBook, 8 GB RAM, a nearly full disk, or a thermally limited CPU** is a different product than Cursor on a desktop with headroom. The agent stack still wants to parallelize: terminals, subagents, Docker, indexers, and long chats all compete for the same small pool of RAM and disk bandwidth. When the machine is already strained, **swap spikes, the fan never stops, UI stutters, and completions time out**—not because the model is dumb, but because the host is out of air.

Guardian targets that gap: **it makes “how hard are we pushing this laptop?” explicit and actionable** instead of invisible. Hooks inject real metrics into agent context; the daemon can throttle Docker and clamp fork storms; optional gates stop prompt floods when memory or policy says you’re past safe operating limits. You still get work done—you’re just less likely to **lose a session to overload** by stacking heavy operations on a machine that’s already redlining.

### What it mitigates in practice

| Pain point | How Guardian helps |
|---|---|
| **No visibility** — agents don’t “feel” 90% CPU or 1 GB free RAM | **Session / subagent hooks** surface CPU, memory, swap, Cursor RSS, and **disk** pressure so the model (and you) see the same numbers. |
| **Death by parallel work** — shells + Docker + subagents at once on a weak machine | **Pressure levels** and advisories steer toward sequential work; optional **prompt gates** when things are critical or Cursor memory is huge. |
| **Docker and dev services eating the box** | Optional **Docker CPU throttling** for non-essential containers under strain. |
| **Runaway process trees** (builds, test runners) | **Fork guard** with optional kill for pathological spawns. |
| **Indexing and huge contexts on a full disk** | **Disk** sampling on the home volume + **cursorignore**-style warnings for paths that blow up context and disk. |
| **Sending prompts into a machine that’s already swapping hard** | **`beforeSubmitPrompt`** can block (with **resume** / snooze) so you don’t add load at the worst moment. |

**Core idea:** *Observable pressure + honest agent guidance + daemon-side enforcement + optional gates with resume—so Cursor stays usable on hardware that isn’t a dev workstation.*

---

## What ships in this repo

| Piece | Location | Role |
|---|---|---|
| `guardiand` | `src/` | Rust daemon — samples CPU, memory, swap, thermal, Docker, Cursor RSS (~`ps`), home-volume disk (`statvfs`), writes `state.json` + `hook_policy.json` |
| Cursor hooks | `hooks/` | Shell + stdlib Python — `sessionStart`, `beforeSubmitPrompt`, `beforeReadFile`, `preToolUse`, `subagentStart`, … |
| Policy | `~/.guardian/config.toml` | Thresholds, `[prompt_gate]`, `[session_budget]`, `[disk]`, `[cursorignore_policy]` |
| Guardian.app | `app/` | SwiftUI menu bar app (optional) |
| Stress tests | `tests/stress/` | **Containerized** hook validation (no bare-metal stress) |
| Resume helper | `scripts/guardian-resume.sh` | Snooze gates or one-shot `proceed_once` |

---

## How it works

1. **`guardiand`** classifies pressure as `clear`, `strained`, or `critical` (optional **memory headroom ratios** in config).
2. State is written atomically to `~/.guardian/state.json` (including **`disk`** usage on the home volume); **`hook_policy.json`** mirrors gate settings for shell hooks.
3. **`beforeSubmitPrompt`** may return `continue: false` when **`~/.guardian/hook_policy.json`** and live state say so (pressure and/or **Cursor RSS** above cap). Users can **`touch ~/.guardian/proceed_once`** or **snooze** (see below)—never a dead end.
4. **`beforeReadFile`** always **`allow`**s; it may add **advisory** text when a path matches the shipped **cursorignore checklist** and isn’t covered by `.cursorignore` or **`.guardian/cursorignore-allow`**.
5. **`preToolUse` / `subagentStart`** remain **allow** + advisory (by default).
6. Under load, the **daemon** enforces Docker throttling, fork guard, etc.

---

## Quick start

### Prerequisites

- macOS 14+ (Sonoma)
- Rust (`rustup`)
- `jq` and `sqlite3` (bundled on macOS)
- `python3` (for `hooks/cursorignore_check.py`; macOS/Xcode CLI usually provide it)

### Install

```bash
bash scripts/install.sh
bash hooks/install-hooks.sh
```

Install copies default **`~/.guardian/hook_policy.json`** if missing (tune gates without recompiling).

### Prompt gates and resume

| Action | Command / file |
|--------|------------------|
| Allow **one** blocked send | `touch ~/.guardian/proceed_once` then submit again |
| Snooze gates ~15 minutes | `bash scripts/guardian-resume.sh snooze 15` |
| Clear snooze | `bash scripts/guardian-resume.sh clear-snooze` |

Copy in blocked **`user_message`** repeats these hints.

### Session budget (Cursor memory)

**`[session_budget]`** uses aggregate **Cursor RSS** in megabytes (`state.json` → `cursor.resident_memory_megabytes`, summed from `Cursor*` processes via `ps`). When RSS exceeds **`max_cursor_rss_megabytes`**, **`beforeSubmitPrompt`** can block (or use resume overrides). **`warn_cursor_rss_megabytes`** drives an advisory in **`sessionStart`**. Set **`max_cursor_rss_megabytes = 0`** to disable RSS-based blocking.

`cursor.active_sessions` still counts **directories under `~/.cursor/projects`** for diagnostics only (that count can stay high from stale folders).

### Disk (`[disk]`)

**`[disk]`** samples the volume containing your home directory and sets `state.json` → **`disk.level`** to `clear`, `warn`, or `critical` from **`warn_used_percent`** / **`critical_used_percent`** (defaults **85** / **93**). **`sessionStart`** and **`subagentStart`** add short advisories when space is tight; see **`hooks/resources.md`** for cleanup ideas (worktrees, Docker images, caches).

### `.cursorignore` hygiene

Shipped patterns live in `hooks/cursorignore-checklist.json`. Per-repo exceptions: **`.guardian/cursorignore-allow`** (one glob per line, `#` comments). See `hooks/resources.md`.

### Configure

Edit `~/.guardian/config.toml` (see `scripts/install.sh` for a full default including `[prompt_gate]`, `[session_budget]`, `[disk]`, `[cursorignore_policy]`).

**Restart `guardiand` after changing `config.toml`** (for example re-run `bash scripts/install.sh`, or `launchctl unload` / `launchctl load` on `~/Library/LaunchAgents/com.guardian.guardiand.plist`). The daemon writes **`~/.guardian/hook_policy.json` at startup** from config; until it restarts, shell hooks keep using the previous snapshot, so prompt gates and cursorignore policy flags may not match your new thresholds or `[prompt_gate]` / `[session_budget]` / `[disk]` / `[cursorignore_policy]` sections.

Optional memory **ratio** thresholds (inside `[thresholds]`):

```toml
strained_memory_available_ratio = 0.15
critical_memory_available_ratio = 0.08
```

### Verify

```bash
launchctl list com.guardian.guardiand
cat ~/.guardian/state.json | jq '{pressure, cursor, disk}'
cat ~/.guardian/hook_policy.json
cat ~/.cursor/hooks.json | jq '.hooks | keys'
```

### Uninstall

```bash
bash scripts/uninstall.sh
rm -rf ~/.cursor/hooks/guardian/
rm -rf ~/.guardian/
```

---

## Development

```bash
cargo build
cargo test

docker compose -f tests/stress/docker-compose.test.yml run --rm guardian-test
docker compose -f tests/stress/docker-compose.test.yml run --rm guardian-e2e

# Mock hook I/O (no Docker)
bash demos/guardian-barriers-mock/run-demo.sh
```

---

## Pressure levels

| Level | CPU | Memory free | Swap used |
|---|---|---|---|
| `clear` | < 70% | > 2 GB | < 25% |
| `strained` | 70–90% | 1–2 GB | 25–50% |
| `critical` | > 90% | < 1 GB | > 50% |

Thermal state and process usage ratio can also escalate. Optional **available/total RAM** ratios in config add another signal. **Hysteresis** dampens flapping.

---

## Python and [Monty](https://github.com/pydantic/monty)

This repo includes small **Python** helpers under `hooks/` (stdlib only). We reference **[Monty](https://github.com/pydantic/monty)**—a **minimal Python interpreter in Rust** for **safe execution of LLM-generated code**—because it reduces attack surface versus unconstrained CPython in agent loops. Guardian hook scripts still run under **`python3`** on the host for Cursor; keep them small and auditable. See `.cursor/rules/monty-python.mdc`.

---

## License

MIT — see [`LICENSE`](LICENSE).
