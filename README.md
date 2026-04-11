# Guardian

**Keep AI coding agents fast—and keep your Mac usable.**

Guardian is a macOS stack that closes the loop between *what your machine is doing* and *what Cursor agents are allowed to assume*. Agents get live pressure context in every session; the daemon enforces limits when things get hot. **Prompt-level gates** (optional) can pause sends when pressure or parallel workspace load is too high, with **human-in-the-loop** resume paths.

---

## Why this exists

Modern agents parallelize aggressively: shells, subagents, `docker compose`, test runners. That’s great for throughput until CPU pegs, memory vanishes, and swap turns your machine into sludge. **Guardian exists so agents can see the wall before they hit it**—and so protection isn’t “hope the model reads the room.”

| Without Guardian | With Guardian |
|---|---|
| Agents guess load from vibes | Every session starts with **real** CPU, memory, swap, thermal, Docker signal |
| No guard on user sends under load | **`beforeSubmitPrompt`** can block sends when policy says so (with **resume** options) |
| Runaway containers amplify pain | Optional **Docker CPU throttling** on non-essential services under strain |
| Fork storms take down the session | **Fork guard** with optional kill for pathological spawns |

**Core value proposition:** *Observable pressure + honest agent guidance + daemon-side enforcement + optional prompt gates with resume.*

---

## What ships in this repo

| Piece | Location | Role |
|---|---|---|
| `guardiand` | `src/` | Rust daemon — samples CPU, memory, swap, thermal, Docker, Cursor RSS (~`ps`), writes `state.json` + `hook_policy.json` |
| Cursor hooks | `hooks/` | Shell + stdlib Python — `sessionStart`, `beforeSubmitPrompt`, `beforeReadFile`, `preToolUse`, `subagentStart`, … |
| Policy | `~/.guardian/config.toml` | Thresholds, `[prompt_gate]`, `[session_budget]`, `[cursorignore_policy]` |
| Guardian.app | `app/` | SwiftUI menu bar app (optional) |
| Stress tests | `tests/stress/` | **Containerized** hook validation (no bare-metal stress) |
| Resume helper | `scripts/guardian-resume.sh` | Snooze gates or one-shot `proceed_once` |

---

## How it works

1. **`guardiand`** classifies pressure as `clear`, `strained`, or `critical` (optional **memory headroom ratios** in config).
2. State is written atomically to `~/.guardian/state.json`; **`hook_policy.json`** mirrors gate settings for shell hooks.
3. **`beforeSubmitPrompt`** may return `continue: false` when **`~/.guardian/hook_policy.json`** and live state say so (pressure + **session budget**). Users can **`touch ~/.guardian/proceed_once`** or **snooze** (see below)—never a dead end.
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

### Session budget (heuristic)

`cursor.active_sessions` counts **directories under `~/.cursor/projects`** (proxy for parallel workspace load—not a perfect Agent-tab count). When above **`[session_budget].max_active_sessions`**, **`beforeSubmitPrompt`** can block until you close or finish other sessions (or use resume overrides).

### `.cursorignore` hygiene

Shipped patterns live in `hooks/cursorignore-checklist.json`. Per-repo exceptions: **`.guardian/cursorignore-allow`** (one glob per line, `#` comments). See `hooks/resources.md`.

### Configure

Edit `~/.guardian/config.toml` (see `scripts/install.sh` for a full default including `[prompt_gate]`, `[session_budget]`, `[cursorignore_policy]`). Restart the LaunchAgent after changes.

Optional memory **ratio** thresholds (inside `[thresholds]`):

```toml
strained_memory_available_ratio = 0.15
critical_memory_available_ratio = 0.08
```

### Verify

```bash
launchctl list com.guardian.guardiand
cat ~/.guardian/state.json | jq '{pressure, cursor}'
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
