# Guardian — help Cursor avoid overexertion

Short checklist (also surfaced by hooks when load is high):

- **Fewer parallel Agent/Composer chats** — finish or archive threads before opening new heavy ones.
- **`.cursorignore`** — exclude `node_modules`, build outputs (`target/`, `dist/`, `build/`), caches, large assets. See [Cursor: Ignore files](https://cursor.com/docs/context/ignore-files).
- **Context** — see [Cursor: Context](https://cursor.com/docs/context) for how indexing uses disk and memory.
- **Extensions** — disable nonessential extensions; test with `cursor --disable-extensions` if diagnosing spikes.
- **Long chats** — export or start a fresh thread when a conversation grows huge (renderer memory).
- **Docker / dev servers** — Guardian can throttle nonessential containers when pressure is high (`~/.guardian/config.toml` `[docker]`).
- **Disk space** — Guardian reports home-volume usage in `state.json` and session advisories when usage crosses `[disk]` thresholds in config. Common space hogs: stale **git worktrees** (`git worktree list`, remove unused checkouts), **Docker** images/containers (`docker system df`, `docker image prune`, `docker builder prune`), **build outputs** (`target/`, `dist/`, `build/`, `node_modules`), **Xcode DerivedData**, and **`~/.cache`**.
- **Deferred prompts** — `~/.guardian/agent_queue.jsonl` + `~/.guardian/guardian-queue.sh` (installed by `hooks/install-hooks.sh`). Use when a send is blocked or you want to run work later; Cursor does not auto-run queued text—paste manually or use optional `scripts/install-queue-watch.sh` for a clear-pressure notification.

**Resume blocked prompts (human in the loop):**

- **CLI (preferred):** `~/.guardian/guardian` — `snooze 15`, `once`, `zeno bump` / `zeno rollback`, etc. (`guardian --help`). If missing, run `bash scripts/install.sh` from your Guardian clone.
- **macOS (click):** double-click `~/.guardian/Guardian-Snooze-15m.command` or `Guardian-Once.command` in Finder (installed by `install.sh` / `hooks/install-hooks.sh`). Or run `open ~/.guardian/Guardian-Snooze-15m.command` in Terminal.
- **Cursor (slash commands):** after `hooks/install-hooks.sh`, use **`/guardian-snooze`** or **`/guardian-once`** in chat — these paste the steps above (they do not run shell by themselves).
- **OpenAI Codex CLI:** install with **`bash scripts/install-codex-hooks.sh`**, enable **`codex_hooks = true`** in Codex **`config.toml`**, restart Codex — see [docs/codex.md](../docs/codex.md). Slash commands are Cursor-only; use the **`guardian`** CLI or **`.command`** files here too.
- One-shot fallback: `touch ~/.guardian/proceed_once` then submit again.
- Snooze fallback: write a future ISO timestamp into `~/.guardian/snooze_until`, or `bash scripts/guardian-resume.sh snooze 15` from a repo clone.
- **Zeno** (relax effective limits — each bump moves percent-style caps halfway toward 100% used): `~/.guardian/guardian zeno bump`; undo with `zeno rollback`; `zeno status` / `zeno reset`.
