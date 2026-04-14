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

**When a send is blocked:**

- **Chat:** `/guardian-snooze` or `/guardian-once` (after `hooks/install-hooks.sh` → `~/.cursor/commands/`).
- **CLI:** `~/.guardian/guardian snooze 15` · `once` · `zeno bump|rollback` (`guardian --help`).
- **macOS:** `~/.guardian/Guardian-*.command` or `open ~/.guardian/Guardian-Snooze-15m.command`.
- **Codex:** [docs/codex.md](../docs/codex.md) — `install-codex-hooks.sh`, `codex_hooks = true`; no slash commands, use CLI / `.command`.
- **Manual:** `touch ~/.guardian/proceed_once` · ISO in `~/.guardian/snooze_until` · `bash scripts/guardian-resume.sh snooze 15`.
