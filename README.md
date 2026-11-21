# Guardian

**Agent resource monitor for AI coding agents** — **Cursor**, **OpenAI Codex**, and **Claude Code** are supported today.

**Keep agents useful—and keep your Mac from melting down.**

Guardian is a macOS stack that closes the loop between *what your machine is doing* and *what your agent tools are allowed to assume*. It samples CPU, memory, swap, disk, and per-agent memory (Cursor, Claude, Codex), writes shared state under `~/.guardian/`, and wires that into each editor through hooks. Agents get live pressure context in every session; the daemon enforces limits when things get hot. **Prompt-level gates** (optional) can pause sends when CPU/memory pressure or Cursor’s own memory use is too high, with **human-in-the-loop** resume paths.

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

**Core idea:** *Observable pressure + honest agent guidance + daemon-side enforcement + optional gates with resume—so Cursor, Codex, and Claude Code stay usable on hardware that isn’t a dev workstation.*

---

## What ships in this repo

| Piece | Location | Role |
|---|---|---|
| `guardiand` | `src/` | Rust daemon — samples CPU, memory, swap, thermal, Docker, per-agent RSS for Cursor / Claude / Codex (~`ps`), home-volume disk (`statvfs`), writes `state.json` + `hook_policy.json` |
| `guardian` | `src/bin/guardian.rs` | User CLI (installed to `~/.guardian/guardian`) — **snooze** / **once** / **zeno** (relax effective limits toward full usage) |
| Cursor | `hooks/`, `hooks/install-hooks.sh` | Shell + stdlib Python — `sessionStart`, `beforeSubmitPrompt`, `beforeReadFile`, `preToolUse`, `subagentStart`, … (`~/.cursor/hooks.json`) |
| Codex | `hooks/codex/`, `scripts/install-codex-hooks.sh` | OpenAI Codex CLI + app — `SessionStart`, `UserPromptSubmit`, `PreToolUse` (`~/.codex/hooks.json`; hooks must be **trusted** in Codex once). See [docs/codex.md](docs/codex.md). |
| Claude Code | `hooks/claude/`, `scripts/install-claude-hooks.sh` | CLI, desktop app, IDE extensions — `SessionStart`, `UserPromptSubmit`, `PreToolUse` (merged into `~/.claude/settings.json` → `hooks`). See [docs/claude-code.md](docs/claude-code.md). |
| Policy | `~/.guardian/config.toml` | Thresholds, `[prompt_gate]`, `[tool_gate]`, `[session_budget]`, `[disk]`, `[queue]`, `[cursorignore_policy]` |
| Agent work queue | `scripts/guardian-queue.sh`, `~/.guardian/agent_queue.jsonl` | CLI to add/list/pop deferred prompts; optional enqueue-on-block + clear-pressure notifier |
| Guardian.app | `app/`, `scripts/install-app.sh` | SwiftUI **menu bar app** (optional, no Dock icon) — level and why, snooze / allow once, memory, swap, disk, per-agent RAM, today's time at each level, config problems, and **Find space to free**. See [Menu bar app](#menu-bar-app). |
| Stress tests | `tests/stress/` | **Containerized** hook validation (no bare-metal stress) |
| Resume + zeno | `~/.guardian/guardian`, `Guardian-*.command`, `/guardian-snooze` | CLI, macOS double-click helpers, Cursor slash commands (see `hooks/resources.md`) |

---

## How it works

1. **`guardiand`** classifies pressure as `clear`, `strained`, or `critical` (optional **memory headroom ratios** in config).
2. State is written atomically to `~/.guardian/state.json` (including **`disk`** usage on the home volume); **`hook_policy.json`** mirrors gate settings for shell hooks.
3. **`beforeSubmitPrompt`** may return `continue: false` when **`~/.guardian/hook_policy.json`** and live state say so (pressure and/or **Cursor RSS** above cap). Users can **`touch ~/.guardian/proceed_once`** or **snooze** (see below)—never a dead end.
4. **`beforeReadFile`** always **`allow`**s; it may add **advisory** text when a path matches the shipped **cursorignore checklist** and isn’t covered by `.cursorignore` or **`.guardian/cursorignore-allow`**.
5. **`preToolUse` / `subagentStart`** remain **allow** + advisory (by default). Opt in to **`[tool_gate] deny_subagents_on = "critical"`** to deny *new subagent spawns* under pressure — the one thing a prompt gate cannot stop mid-turn.
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
bash scripts/install.sh              # daemon + CLI (required)
bash hooks/install-hooks.sh          # Cursor
bash scripts/install-claude-hooks.sh # Claude Code
bash scripts/install-codex-hooks.sh  # OpenAI Codex
```

Run the hook installers for the agents you use; all three read the same **`~/.guardian/`** state and policy. Each one merges into the agent's existing config: re-running is safe, hooks you added yourself are kept (including on the same events), and a config it cannot parse is left untouched.

`install.sh` copies **`guardiand`** to **`~/.guardian/guardiand`** and points the LaunchAgent there, so `cargo clean` or a disk-space sweep of `target/` no longer kills the daemon. It exits non-zero unless the new daemon is actually writing fresh `state.json`.

Install copies default **`~/.guardian/hook_policy.json`** if missing (tune gates without recompiling).

### Claude Code

```bash
bash scripts/install-claude-hooks.sh
```

Merges **`SessionStart`**, **`UserPromptSubmit`** (prompt gate), and **`PreToolUse`** (`Bash|Agent|Task` advisory) into **`~/.claude/settings.json`**. Idempotent; other hooks and settings are preserved, and entries from earlier Guardian versions are replaced. Start a new session afterwards. Details: [docs/claude-code.md](docs/claude-code.md).

### OpenAI Codex

```bash
bash scripts/install-codex-hooks.sh
```

Merges the same three events into **`~/.codex/hooks.json`**. Hooks are on by default in current Codex, but **Codex skips new or changed hooks until you trust them** — run **`/hooks`** in the CLI or open **Settings > Hooks** in the app. Details: [docs/codex.md](docs/codex.md).

### Menu bar app

```bash
bash scripts/install-app.sh            # build, install to ~/Applications/Guardian.app, launch
bash scripts/install-app.sh --login    # also start it at every login
bash scripts/install-app.sh --no-login # stop starting it at login
```

A shield appears at the right of the menu bar; its shape follows the level (outline = clear, half-filled = strained, exclamation = critical, slashed = `guardiand` not reporting), and it adds `Disk NN%` when the disk is critical. Click it for:

- the level and **why**, in the daemon's own words
- **Snooze** (up to 2 hours) and **Allow once**, with what is currently switched off
- memory, swap, CPU, disk, the kernel's memory-pressure and thermal verdicts, per-agent RAM
- **Today**: share of time at each level and how often it changed
- any **config problem** the daemon reported
- **Find space to free**: the `guardian disk` scan of all your git repositories, with each item's age and worktree status, sized by what deleting it would free. The scan `guardiand` keeps fresh in the background is shown as soon as the panel opens; **Scan again** brings it up to date. Stale bundles come first, the longest untouched at the top; ticking one ticks everything in it that can go without a look. Tick the items to remove and press **Delete selected**: it lists them with what they free and asks before deleting (through the same checks as `guardian disk delete`). **Tick old and safe** ticks every project bundle untouched for two weeks or more. Items that need their own tool or a decision are listed after those, each with a button that copies its removal command. Every row can be shown in Finder.
- **Open dashboard** for the larger System / Sessions / Docker / Settings window

It notifies you only when load becomes critical or the disk becomes critical. The app needs macOS 14 and the Swift toolchain to build; it reads `~/.guardian/` and calls `~/.guardian/guardian`, so install the daemon first.

### Tool gate (`[tool_gate]`)

Tool hooks are advisory by default. To hard-stop subagent fan-out when the machine is already in trouble:

```toml
[tool_gate]
deny_subagents_on = "critical"   # "never" (default) | "strained" | "critical"
```

Applies to Cursor (`Task`), Claude Code (`Agent` / `Task`), and Codex (`spawn_agent`). Shell commands are never denied; a snooze lifts the deny.

### Prompt gates and resume

The default `adaptive` mode pauses a prompt only when a fresh sample shows critical pressure with critical macOS memory pressure or serious thermal pressure, and the prompt explicitly requests heavy local work. Examples include “Run the full test suite” and “Rebuild all Docker containers.” Questions, ordinary edits, ambiguous requests, and low available RAM alone pass through. The estimate uses local text rules and can miss heavy work implied by prior context.

Set a default or override individual clients in `~/.guardian/config.toml`.

```toml
[prompt_gate]
mode = "adaptive"
block_on = "critical"

[prompt_gate.clients]
cursor = "adaptive"
claude = "advisory"
codex = "adaptive"
```

- **Adaptive** uses the default workload and system-pressure checks.
- **Advisory** lets prompts through and warns on explicit heavy work during critical pressure.
- **Strict** pauses prompts at the configured `block_on` pressure (`critical` or `strained`) and supports the optional Cursor RSS cap.
- Set `enabled = false` under `[prompt_gate]` to switch off prompt gating across clients. `block_on = "never"` disables pressure-based gating.

Existing configurations without a mode pick up the adaptive default when the hooks are updated. Set `mode = "strict"` to keep pressure-only blocking.

Use the **Guardian menu bar** to snooze for 5, 15, 30, or 60 minutes, see when snooze ends, or resume early. **Allow once** stays armed for 10 minutes or until a prompt would otherwise be paused, whichever comes first. A new session is told when a snooze or allow-once is in force. Snooze and Allow once are shared across Cursor, Claude Code, and Codex.

- `~/.guardian/guardian once` allows one paused prompt, within the next 10 minutes. Resend it afterward. An unused token expires instead of waiting for some later block.
- `~/.guardian/guardian snooze 15` snoozes hooks for 15 minutes (120 at most; hooks ignore a `snooze_until` further out than that). To switch a gate off for longer, set it in `config.toml`, where it stays visible.
- `~/.guardian/guardian clear-snooze` resumes hooks immediately.
- `~/.guardian/guardian stats` shows the share of time spent at each pressure level per day.

**What zeno can and cannot relax:** `zeno bump` loosens Guardian's own numeric thresholds (CPU, available RAM, swap, disk, Cursor RSS). It does not override macOS's verdicts — kernel memory pressure and thermal pressure still escalate at any zeno step. When a block's `Why:` names one of those, use **snooze** or **once**.

### Session budget (Cursor memory)

**`[session_budget]`** uses aggregate **Cursor RSS** in megabytes (`state.json` → `cursor.resident_memory_megabytes`, summed from `Cursor*` processes via `ps`). **`warn_cursor_rss_megabytes`** drives an advisory in **`sessionStart`**.

**Strict-mode prompt blocking** from RSS is controlled by **`[prompt_gate].block_on_session_budget`** (default **off** in new installs). When enabled, **`beforeSubmitPrompt`** only blocks for RSS when **pressure is not `clear`** *and* RSS is above **`max_cursor_rss_megabytes`**—so a healthy machine (clear CPU/memory/swap) is not blocked just because Cursor is using a few GB. Set **`max_cursor_rss_megabytes = 0`** to disable RSS-based blocking entirely.

If you still see messages about **“Parallel workspace load”** and **`~/.cursor/projects` count**, your **`~/.cursor/hooks/guardian/`** scripts are outdated; run **`bash hooks/install-hooks.sh`** from this repo and restart **`guardiand`** so **`~/.guardian/hook_policy.json`** matches **`config.toml`**.

`cursor.active_sessions` still counts **directories under `~/.cursor/projects`** for diagnostics only (that count can stay high from stale folders).

### Disk (`[disk]`)

**`[disk]`** (`enabled`, `warn_used_percent`, `critical_used_percent`, `background_scan_minutes`) samples the volume containing your home directory and sets `state.json` → **`disk.level`** to `clear`, `warn`, or `critical` from **`warn_used_percent`** / **`critical_used_percent`** (defaults **85** / **93**). **`sessionStart`** and **`subagentStart`** add short advisories when space is tight; see **`hooks/resources.md`** for cleanup ideas (worktrees, Docker images, caches).

### Finding space to free (`guardian disk`)

```bash
~/.guardian/guardian disk                  # list the biggest targets (read-only)
~/.guardian/guardian disk --cached         # the last saved scan, at once, without scanning
~/.guardian/guardian disk --fresh          # measure everything again, reusing nothing
~/.guardian/guardian disk --older-than 14  # only projects and worktrees untouched for 2+ weeks
~/.guardian/guardian disk clean            # list, then choose what to delete
~/.guardian/guardian disk delete ~/code/app/target   # delete specific targets
~/.guardian/guardian disk save --delete ~/code/app-wt   # commit and push a worktree's unsaved work, then remove it
```

The scan walks your home folder for **every git repository** (skipping `Library`, media folders, and `Desktop` / `Documents` / `Downloads`, which macOS guards with a permission prompt; add one with `--root`) and reports:

- **Build output and dependencies** inside projects: `target/`, `node_modules/`, `.next/`, `.build/`, `Pods/`, virtual environments, and similar.
- **Linked git worktrees**, each checked for unsaved work: uncommitted or untracked files, commits on a detached HEAD that exist nowhere else, a lock, and ignored files git keeps no copy of (`.env` files, local databases, notes) anywhere in the worktree.
- **Tool caches**: Cargo, npm, pnpm, Bun, pip, uv, Homebrew, Go, Gradle, Maven, Xcode and Simulator data, Playwright browsers, Docker's disk image, model caches and the Trash.

**Stale things come first, bundled.** Anything untouched for a week or more is grouped by project (a repository with all its worktrees and build folders) and by how long it has sat: 90 days or more, 30 to 89, 14 to 29, 7 to 13. The longest untouched bundles are listed first, and within a band the ones that free the most. A bundle is listed when it adds up to the size floor even if none of its folders would on its own, which is how fifty small forgotten worktrees become one line. Each bundle says how much of it can be deleted now and how many of its folders need a look or their own command first. `guardian disk clean` offers a bundle as one numbered choice, and `old` picks every project bundle untouched for two weeks or more. Download caches that refill themselves are dated by when something was last added at their top level and bundled as "Tool caches"; they are never picked by `old`, and toolchains, models and Docker's disk are not dated at all, because nothing records when those were last used. What is in use is listed after the bundles, largest first.

Every target shows its size, how long the project has gone untouched, and whether it **comes back on its own** or needs a **look before deleting**. Projects are measured oldest first inside a time budget (`--seconds`, default 45), at background I/O priority, so the scan is safe to run on a machine that is already struggling; anything not reached is counted and reported.

**Sizes are what deleting would free**, which can be much less than what a folder appears to occupy. A `node_modules` installed by bun or pnpm is made of clones of the files in that tool's cache: every copy looks like a gigabyte, and deleting one frees almost nothing, because the data stays for the other copies. Such a row says so ("1.0 GB in size, of which 1.0 GB is shared with other copies"), sorts below folders that really give space back, and the totals count only what would be freed. A file with several names (hard links, common in `target/`) is counted once, and as freed only if every name is inside the folder.

**Scans remember what they measured.** Results are saved to `~/.guardian/disk_index.json`. The next scan measures only what is new, was touched since, was measured only in part, or has not been measured for 12 hours; everything else keeps its saved size. A scan that runs out of time is carried on by the next one, starting with what has never been measured, so running it again always makes progress. `--cached` prints the saved scan immediately and `--fresh` ignores it.

What git said about a worktree is saved the same way, because asking takes a second or more per worktree: git is asked again when the worktree shows a sign of use (a commit, checkout or staged change, or a change at its top level) or after 12 hours. A listing can therefore still say "no uncommitted work" about a worktree whose files were edited since without any of that. **Deleting never goes by the listing**: the index only saves measuring, and whether something may be deleted is decided afresh from the disk and from git at that moment.

**`guardiand` keeps the saved scan fresh in the background**, so the list is already there when the disk fills up. It starts `guardian disk refresh` as a separate low-priority process every **`[disk] background_scan_minutes`** (default **180**; **0** turns it off), and straight away when the disk reaches `warn` or `critical`. It does not start one at `critical` load, starts one under `strained` load only when the disk is tight, stops a running one if load becomes critical, and never runs two at once. A background scan has two minutes; when that is not enough to measure everything (the first scan, or after a lot has changed), the next one follows after ten minutes instead of a whole interval and carries on, up to six times in a row.

**Deleting is a separate, confirmed step.** `disk clean` numbers what it can delete and asks which (`1 3 5`, `2-4`, or `old` for everything that comes back on its own and has gone untouched for 14+ days), then asks once more before removing anything. Each target is re-checked at that moment:

- a build or dependency folder is deleted only if git tracks nothing inside it;
- a worktree is removed with `git worktree remove`, never `--force`, and only if it has no uncommitted work — its branch stays in the repository;
- a worktree with uncommitted work is never deleted as it is, but there is a fast way through: **`guardian disk save --delete <worktree>`** (in the menu bar, **Save & delete…**; in `disk clean`, the choices marked SAVED FIRST). It commits what is uncommitted to the worktree's branch, pushes that branch, checks that the remote has the commit, and only then removes the worktree. `--plan` shows what it would commit and push without changing anything. The branch is always pushed under its own name: a branch started from `origin/main` usually has `origin/main` recorded as what it follows, and work in progress is never pushed there. Work sitting on a shared branch itself (the remote's default branch, `main`, `develop`, `release/…`) is saved on a new branch, `guardian-save/<folder>`, and the shared one is left alone. It refuses rather than guesses: it never forces a push, never commits a new file that looks like credentials (`.env`, keys, `credentials.json`) or is larger than 50 MB, never commits into a merge or rebase under way, and stops if a commit would miss something (files hidden from git status, a nested repository). A detached HEAD gets a branch named `guardian-save/<folder>`. Commit hooks are skipped, since this is work being put somewhere safe; push hooks run as usual. Files git ignores, such as a `.env`, are not saved by a commit and are named before you confirm. If the push fails, the commit stays on its branch and the worktree is kept;
- caches that have their own cleaner (`npm cache clean`, `brew cleanup`, `docker system prune`, …) are listed with that command and left for you to run;
- nothing outside your home folder, nothing at its top level, and no symlink is ever deleted.

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

**Edits to `config.toml` take effect within a few seconds; no restart.** `guardiand` re-reads the file once it has stopped changing and rewrites `~/.guardian/hook_policy.json` for the shell hooks.

A file that no longer parses is **not applied**: the settings already in force stay, and at startup the daemon falls back to the last version that parsed (`~/.guardian/config.last_good.toml`) rather than to built-in defaults. Misspelled settings and values that cannot work (an unknown mode word, thresholds the wrong way round) are reported instead of silently ignored. Problems appear in `state.json` → `config_problems`, in the next session's start message, and in:

```bash
~/.guardian/guardian config check     # exits non-zero if anything needs fixing
```

Optional memory **ratio** thresholds (inside `[thresholds]`):

```toml
strained_memory_available_ratio = 0.15
critical_memory_available_ratio = 0.08
```

### Verify

```bash
launchctl list com.guardian.guardiand
cat ~/.guardian/state.json | jq '{pressure, agents, disk, sampled_at}'   # sampled_at must be seconds old
cat ~/.guardian/hook_policy.json
cat ~/.cursor/hooks.json | jq '.hooks | keys'
cat ~/.claude/settings.json | jq '.hooks | keys'
cat ~/.codex/hooks.json | jq '.hooks | keys'
```

### Uninstall

```bash
bash scripts/uninstall.sh
rm -rf ~/.guardian/   # optional: config, state, session history
```

`uninstall.sh` stops the daemon, removes Guardian's entries from `~/.cursor/hooks.json`, `~/.claude/settings.json` and `~/.codex/hooks.json` (your other hooks and settings are kept; the previous file is saved as `*.guardian-uninstall.bak`), then deletes the hook scripts and Cursor slash commands. If a config cannot be edited safely it is left alone, along with its scripts, and the script says so.

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

Every pull request runs the same checks in GitHub Actions ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)): `cargo build --release` + `cargo test` on macOS (plus a syntax check and smoke run of the hook scripts under macOS's bash 3.2), and the containerized hook suite + agent-lifecycle e2e on Linux. `Cargo.lock` is committed and CI builds with `--locked`; run any `cargo` command after bumping the version so the lockfile follows.

---

## Pressure levels

| Level | CPU | Memory free | Swap used |
|---|---|---|---|
| `clear` | < 70% | > 2 GB | < 25% |
| `strained` | 70–90% | 1–2 GB | 25–50% |
| `critical` | > 90% | < 1 GB | > 50% |

Two kernel signals sit alongside those thresholds:

| Signal | Source | Effect |
|---|---|---|
| **Memory pressure** | `kern.memorystatus_vm_pressure_level` — what Activity Monitor's memory graph shows | `warn` → `strained`, `critical` → `critical` |
| **Thermal pressure** | `com.apple.system.thermalpressurelevel` (Apple Silicon; Intel uses `machdep.xcpm.thermal_pressure`) | `moderate` → `strained`, `heavy` or worse → `critical` |

**Corroborated criticals.** Swap % and CPU % are noisy on their own: macOS grows swap on demand, so "90% of the current swap file" is reached moments before the file is enlarged, and a compile pegs every core without endangering anything. By default they only reach `critical` when a second signal agrees — the kernel reports **critical** memory pressure (for swap), or the machine is also heating up (for CPU). Alone they count as `strained`. Low available RAM, kernel memory pressure, thermal pressure and a full process table never need corroboration. Where the corroborating signal is unavailable (`unknown`), the uncorroborated behavior applies. Thermal pressure is `unknown` only where it cannot be read or cannot be trusted — in practice, inside a virtual machine, where the notification reads 0 whether or not anything publishes it; on physical Macs 0 is taken as `nominal`.

Before 0.8 a kernel `warn` was enough to corroborate swap. On a small-RAM Mac the kernel reports `warn` and the swap file sits near full for most of the day, so the level flipped between `strained` and `critical` every time swap crossed the threshold. A nearly full swap file now reads as `strained` until the kernel itself says `critical` or available RAM falls under `critical_memory_gb`.

To let every threshold reach `critical` by itself:

```toml
[thresholds]
corroborate_critical = false
```

Process usage ratio can also escalate. Optional **available/total RAM** ratios in config add another signal.

**Hysteresis.** The level rises after `escalate_samples` consecutive higher samples and falls only after `deescalate_secs` of consecutively lower ones. Numeric thresholds also have an exit band: a level entered at a threshold is left only once the value has moved `exit_band` past it (with the defaults, `critical` entered below 1.0 GB available is left above 1.1 GB). A value hovering at a threshold therefore holds its level. The kernel memory-pressure and thermal levels are discrete and rely on the dwell time alone.

```toml
[hysteresis]
escalate_samples = 2
deescalate_secs = 30
exit_band = 0.10      # 0 disables the band
```

These are limited so that a typo cannot freeze the level: at most 30 samples to escalate, at most one hour to de-escalate, and `sample_interval_secs` between 1 and 30 (hooks ignore a `state.json` older than 30 seconds).

### How much time is spent at each level (`guardian stats`)

`guardiand` records the time spent at each published level per day in `~/.guardian/pressure_stats.json` (30 days kept). Print it with:

```bash
~/.guardian/guardian stats            # last 7 days
~/.guardian/guardian stats --days 30
```

`critical` is meant to be the exception. If it stays above about 5% of sampled time, or `changes` runs into the dozens per day, the thresholds do not fit the machine: read `pressure_reasons` to see which signal is responsible. Level changes in `guardiand.stderr.log` carry a local timestamp.

`state.json` → **`pressure_reasons`** lists what is driving the current level in plain words (`"swap 93% used"`, `"thermal pressure heavy"`); hooks append it to advisories, subagent denials and block messages as **`Why: …`**. While hysteresis is holding a level, the reasons shown are the ones that established that level.

### Querying pressure (`state.json`)

The daemon writes the latest sample to **`~/.guardian/state.json`**. That file is the source of truth for “how much pressure am I under?” — the same metrics hooks inject into Cursor context.

**Quick check:**

```bash
cat ~/.guardian/state.json | jq '{
  pressure,
  cpu_percent,
  memory_available_gb,
  swap_used_percent,
  pressure_reasons,
  memory_pressure_level,
  thermal_state,
  disk,
  agents,
  sampled_at
}'
```

**Fields worth reading**

| Field | What it tells you |
|--------|-------------------|
| **`pressure`** | Overall label: `clear`, `strained`, or `critical` (see table above). |
| **`pressure_reasons`** | What is driving the level right now, e.g. `["macOS reports memory pressure", "swap 76% used"]`. Empty when `clear`. |
| **`memory_pressure_level`** | The kernel's verdict: `normal`, `warn`, `critical` (or `unknown`). |
| **`thermal_state`** | `nominal`, `fair` (moderate), `serious` (heavy), `critical`, or `unknown` (no trustworthy source — e.g. a virtual machine). |
| **`cpu_percent`** | Host CPU load (representative sample). |
| **`memory_available_gb`** / **`memory_total_gb`** | Free RAM vs total — tight free memory plus high swap usually means memory pressure. |
| **`swap_used_percent`** | High values (often **well above** the rough bands in the pressure table) mean the kernel is paging — treat memory as constrained even if CPU looks fine. |
| **`disk`** | Home volume: **`available_gb`**, **`used_percent`**, and **`level`** (`clear` / `warn` / `critical` from `[disk]` in `config.toml`). **Disk can hit `critical` while `pressure` is only `strained`** — free space still matters for builds, indexers, and swap files. |
| **`agents`** | Resident memory per coding agent, in MB: **`cursor_mb`**, **`claude_mb`** (desktop app + Claude Code CLI), **`codex_mb`**, and **`total_mb`**. Matched on executable path via `ps`; agents launched through a generic runtime (e.g. an npm install running as `node`) are not attributed. |
| **`cursor`** | Aggregate Cursor **`resident_memory_megabytes`** (processes running from `Cursor.app`), **`process_count`**, and related diagnostics. |
| **`docker`** | Running-container CPU/memory when Docker is available. |
| **`sampled_at`** | Time of this sample (RFC3339). Shell hooks **fail-open** if the file is missing, **stale** (older than ~30 seconds), future-dated, or has no `sampled_at` at all: they assume `clear` so a stopped daemon does not block you forever. |

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

## Contributing

Pull requests to `main` (not direct pushes); bump **`Cargo.toml`** SemVer per [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT — see [`LICENSE`](LICENSE).
