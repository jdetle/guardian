# Guardian + OpenAI Codex CLI

Guardian can drive the same **prompt gate** and **session context** logic via Codex’s experimental hooks ([official hooks documentation](https://developers.openai.com/codex/hooks/)).

## Requirements

- macOS or Linux (Codex hooks are **not** available on Windows today).
- `jq` for the installer merge step.
- Enable hooks in Codex **`config.toml`** (often `~/.codex/config.toml`):

```toml
[features]
codex_hooks = true
```

## Install

From your Guardian git clone (after `cargo build --release` if you want the latest `guardian` binary copied to `~/.guardian/`):

```bash
bash scripts/install-codex-hooks.sh
```

This installs scripts under **`~/.codex/hooks/guardian/`** and merges **`~/.codex/hooks.json`** with:

- **`UserPromptSubmit`** — same policy as Cursor’s `beforeSubmitPrompt` (pressure, RSS, snooze, `proceed_once`, queue enqueue).
- **`SessionStart`** — same context string as Cursor’s `sessionStart`, wrapped in Codex’s `hookSpecificOutput` shape.

Restart the Codex CLI after changing `hooks.json` or config.

## Differences vs Cursor

| | Cursor | Codex |
|---|--------|--------|
| Config | `~/.cursor/hooks.json` | `~/.codex/hooks.json` |
| Prompt event | `beforeSubmitPrompt` | `UserPromptSubmit` |
| Block shape | `continue: false`, `user_message` | `decision: "block"`, `reason` |
| Slash commands | `~/.cursor/commands/guardian-*.md` | Not installed; use **`~/.guardian/guardian`** or macOS **`Guardian-*.command`** |

Resume paths (`guardian snooze`, `once`, zeno) are identical; see [hooks/resources.md](../hooks/resources.md).

## Smoke test

```bash
bash tests/codex-hook-smoke.sh
```
