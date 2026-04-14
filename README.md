# Guardian

**Agent resource monitor for AI coding agents** — **Cursor** and **OpenAI Codex** are supported today.

**Keep agents useful—and keep your Mac from melting down.**

Guardian is a macOS stack that closes the loop between *what your machine is doing* and *what your agent tools are allowed to assume*. It samples CPU, memory, swap, disk, and Cursor process usage, writes shared state under `~/.guardian/`, and wires that into each editor through hooks. Agents get live pressure context in every session; the daemon enforces limits when things get hot. **Prompt-level gates** (optional) can pause sends when CPU/memory pressure or Cursor’s own memory use is too high, with **human-in-the-loop** resume paths.

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

**Core idea:** *Observable pressure + honest agent guidance + daemon-side enforcement + optional gates with resume—so Cursor and Codex stay usable on hardware that isn’t a dev workstation.*

---

## What ships in this repo

| Piece | Location | Role |
|---|---|---|
| `guardiand` | `src/` | Rust daemon — samples CPU, memory, swap, thermal, Docker, Cursor RSS (~`ps`), home-volume disk (`statvfs`), writes `state.json` + `hook_policy.json` |
| `guardian` | `src/bin/guardian.rs` | User CLI (installed to `~/.guardian/guardian`) — **snooze** / **once** / **zeno** (relax effective limits toward full usage) |
| Cursor | `hooks/`, `hooks/install-hooks.sh` | Shell + stdlib Python — `sessionStart`, `beforeSubmitPrompt`, `beforeReadFile`, `preToolUse`, `subagentStart`, … (`~/.cursor/hooks.json`) |
| Codex | `hooks/codex/`, `scripts/install-codex-hooks.sh` | OpenAI Codex CLI — `UserPromptSubmit`, `SessionStart` (`~/.codex/hooks.json`; enable `[features] codex_hooks` in Codex config). See [docs/codex.md](docs/codex.md). |
| Policy | `~/.guardian/config.toml` | Thresholds, `[prompt_gate]`, `[session_budget]`, `[disk]`, `[queue]`, `[cursorignore_policy]` |
| Agent work queue | `scripts/guardian-queue.sh`, `~/.guardian/agent_queue.jsonl` | CLI to add/list/pop deferred prompts; optional enqueue-on-block + clear-pressure notifier |
| Guardian.app | `app/` | SwiftUI menu bar app (optional) — shows pressure + **queued agent jobs**; click a job to `open -a Cursor` on its workspace folder when `workspace_path` is stored |
| Stress tests | `tests/stress/` | **Containerized** hook validation (no bare-metal stress) |
| Resume + zeno | `~/.guardian/guardian`, `Guardian-*.command`, `/guardian-snooze` | CLI, macOS double-click helpers, Cursor slash commands (see `hooks/resources.md`) |

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

### OpenAI Codex CLI (optional)

Codex uses **`~/.codex/hooks.json`** (not Cursor’s file). Hooks are **experimental**; enable **`codex_hooks = true`** in Codex’s **`config.toml`**, then:

```bash
bash scripts/install-codex-hooks.sh
```

This installs **`UserPromptSubmit`** + **`SessionStart`** with the same policy as Cursor’s prompt gate and session hook. Details: [docs/codex.md](docs/codex.md).

### Prompt gates and resume

| Action | Command / file |
|--------|------------------|
| Allow **one** blocked send | `~/.guardian/guardian once` (or `touch ~/.guardian/proceed_once`) then submit again |
| Snooze gates ~15 minutes | `~/.guardian/guardian snooze 15` (or `bash scripts/guardian-resume.sh snooze 15`) |
| Clear snooze | `~/.guardian/guardian clear-snooze` |
| Relax limits (zeno) | `~/.guardian/guardian zeno bump` — `zeno rollback` (undo one bump) — `zeno status` / `zeno reset` |

Copy in blocked **`user_message`** repeats these hints.

### Session budget (Cursor memory)

**`[session_budget]`** uses aggregate **Cursor RSS** in megabytes (`state.json` → `cursor.resident_memory_megabytes`, summed from `Cursor*` processes via `ps`). **`warn_cursor_rss_megabytes`** drives an advisory in **`sessionStart`**.

**Prompt blocking** from RSS is controlled by **`[prompt_gate].block_on_session_budget`** (default **off** in new installs). When enabled, **`beforeSubmitPrompt`** only blocks for RSS when **pressure is not `clear`** *and* RSS is above **`max_cursor_rss_megabytes`**—so a healthy machine (clear CPU/memory/swap) is not blocked just because Cursor is using a few GB. Set **`max_cursor_rss_megabytes = 0`** to disable RSS-based blocking entirely.

If you still see messages about **“Parallel workspace load”** and **`~/.cursor/projects` count**, your **`~/.cursor/hooks/guardian/`** scripts are outdated; run **`bash hooks/install-hooks.sh`** from this repo and restart **`guardiand`** so **`~/.guardian/hook_policy.json`** matches **`config.toml`**.

`cursor.active_sessions` still counts **directories under `~/.cursor/projects`** for diagnostics only (that count can stay high from stale folders).

### Disk (`[disk]`)

**`[disk]`** samples the volume containing your home directory and sets `state.json` → **`disk.level`** to `clear`, `warn`, or `critical` from **`warn_used_percent`** / **`critical_used_percent`** (defaults **85** / **93**). **`sessionStart`** and **`subagentStart`** add short advisories when space is tight; see **`hooks/resources.md`** for cleanup ideas (worktrees, Docker images, caches).

### Deferred work queue (`[queue]`)

Guardian cannot auto-submit prompts into Cursor (no API for that). It **can** persist a **local queue** so you remember what to run when the machine cools off:

- **Storage:** `~/.guardian/agent_queue.jsonl` (append-only JSON lines; treat as **sensitive** — may contain API keys — keep file mode **0600**).
- **CLI:** after `bash hooks/install-hooks.sh`, use **`~/.guardian/guardian-queue.sh`** (`add`, `list`, `peek`, `pop`, `count`). Example:  
  `~/.guardian/guardian-queue.sh add "Refactor foo" "Please refactor src/foo.rs …"`
- **Optional enqueue on block:** set **`[queue].enqueue_on_blocked_submit = true`** in **`config.toml`** and restart **`guardiand`**. When **`beforeSubmitPrompt`** blocks, the hook tries to extract prompt text from Cursor’s JSON (`prompt`, `promptText`, `text`, …) and append it to the queue. If Cursor does not send the text, nothing is stored (blocking still works).
- **Notifications:** **`bash scripts/install-queue-watch.sh`** installs a small LaunchAgent that polls **`state.json`** and shows a **macOS notification** when pressure returns to **`clear`** and the queue is non-empty (throttled to about once per 5 minutes). **Execution is still manual** (paste into Composer).
- **Menu bar (Swift app):** build with **`cd app && swift build -c release`** — the extra shows **Queued jobs** from the same JSONL. Rows with a **`workspace_path`** (set when enqueueing from a blocked submit that included **`workspace_roots`**) open that folder in Cursor via **`open -a Cursor <path>`**. Jobs without a path still open the Cursor app.

### `.cursorignore` hygiene

Shipped patterns live in `hooks/cursorignore-checklist.json`. Per-repo exceptions: **`.guardian/cursorignore-allow`** (one glob per line, `#` comments). See `hooks/resources.md`.

### Configure

Edit `~/.guardian/config.toml` (see `scripts/install.sh` for a full default including `[prompt_gate]`, `[session_budget]`, `[disk]`, `[queue]`, `[cursorignore_policy]`).

**Restart `guardiand` after changing `config.toml`** (for example re-run `bash scripts/install.sh`, or `launchctl unload` / `launchctl load` on `~/Library/LaunchAgents/com.guardian.guardiand.plist`). The daemon writes **`~/.guardian/hook_policy.json` at startup** from config; until it restarts, shell hooks keep using the previous snapshot, so prompt gates and cursorignore policy flags may not match your new thresholds or `[prompt_gate]` / `[session_budget]` / `[disk]` / `[queue]` / `[cursorignore_policy]` sections.

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

### Querying pressure (`state.json`)

The daemon writes the latest sample to **`~/.guardian/state.json`**. That file is the source of truth for “how much pressure am I under?” — the same metrics hooks inject into Cursor context.

**Quick check:**

```bash
cat ~/.guardian/state.json | jq '{
  pressure,
  cpu_percent,
  memory_available_gb,
  swap_used_percent,
  disk,
  cursor,
  sampled_at
}'
```

**Fields worth reading**

| Field | What it tells you |
|--------|-------------------|
| **`pressure`** | Overall label: `clear`, `strained`, or `critical` (see table above). |
| **`cpu_percent`** | Host CPU load (representative sample). |
| **`memory_available_gb`** / **`memory_total_gb`** | Free RAM vs total — tight free memory plus high swap usually means memory pressure. |
| **`swap_used_percent`** | High values (often **well above** the rough bands in the pressure table) mean the kernel is paging — treat memory as constrained even if CPU looks fine. |
| **`disk`** | Home volume: **`available_gb`**, **`used_percent`**, and **`level`** (`clear` / `warn` / `critical` from `[disk]` in `config.toml`). **Disk can hit `critical` while `pressure` is only `strained`** — free space still matters for builds, indexers, and swap files. |
| **`cursor`** | Aggregate Cursor **`resident_memory_megabytes`**, **`process_count`**, and related diagnostics. |
| **`docker`** | Running-container CPU/memory when Docker is available. |
| **`sampled_at`** | Time of this sample (RFC3339). Shell hooks **fail-open** if the file is missing or **stale** (typically older than ~30 seconds): they assume `clear` so a stopped daemon does not block you forever. |

**Plain-language read**

- **`clear`** — Headroom for parallel agents, Docker, and heavier terminal work.
- **`strained`** — Workable, but prefer **sequential** work, fewer parallel chats/subagents, and lighter Docker/indexing until numbers improve.
- **`critical`** — Strong signal to **avoid adding load**; optional prompt gates may block until you use **`proceed_once`**, **snooze**, or the machine recovers.

**Illustrative snapshot** (shape only; your machine will differ):

```json
{
  "pressure": "strained",
  "cpu_percent": 57.3,
  "memory_available_gb": 1.36,
  "swap_used_percent": 86.8,
  "disk": {
    "used_percent": 98.1,
    "level": "critical",
    "available_gb": 4.2
  },
  "cursor": {
    "resident_memory_megabytes": 2181,
    "process_count": 21
  }
}
```

In a case like this, the **headline** level is **strained** (between clear and critical), but **swap** is very high and **disk** is already **`critical`** — the limiting factor is often **disk space** (and memory pressure shows up in swap), not CPU percentage alone. See **`hooks/resources.md`** for cleanup ideas.

---

## Python and [Monty](https://github.com/pydantic/monty)

This repo includes small **Python** helpers under `hooks/` (stdlib only). We reference **[Monty](https://github.com/pydantic/monty)**—a **minimal Python interpreter in Rust** for **safe execution of LLM-generated code**—because it reduces attack surface versus unconstrained CPython in agent loops. Guardian hook scripts still run under **`python3`** on the host for Cursor; keep them small and auditable. See `.cursor/rules/monty-python.mdc`.

---

## License

MIT — see [`LICENSE`](LICENSE).
