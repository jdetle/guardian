# Guardian + OpenAI Codex

Guardian drives the same **prompt gate**, **session context**, and **tool advisory** in Codex (CLI
and desktop app) through its [hooks](https://developers.openai.com/codex/hooks/).

## Requirements

- macOS or Linux.
- `guardiand` installed and running (`bash scripts/install.sh`) — without fresh metrics every hook
  fails open and the gate is off.
- `jq` (the installer refuses to touch `hooks.json` without it).
- Hooks are **on by default** in current Codex. If you turned them off, re-enable them in
  `~/.codex/config.toml` (`codex_hooks` is a deprecated alias of `hooks`):

```toml
[features]
hooks = true
```

## Install

```bash
bash scripts/install-codex-hooks.sh
```

This copies scripts to **`~/.codex/hooks/guardian/`** and merges into **`~/.codex/hooks.json`**
(backup: `hooks.json.bak`). Re-running is safe: existing Guardian entries are replaced, never
duplicated. `CODEX_HOME` is honored.

### Trust the hooks (required, manual)

Codex **skips new or changed hooks until you review and trust them**, and it re-asks whenever a
hook definition changes. After installing or upgrading:

- **CLI:** run `/hooks` and approve the Guardian entries.
- **Desktop app:** **Settings > Hooks** (or **Review hooks** in the composer).

Until you do, Guardian is installed but inert in Codex.

## What each hook does

| Event | Script | Behavior |
|---|---|---|
| `SessionStart` (`startup\|resume`) | `codex/session-start.sh` | Same context line as Cursor's `sessionStart`, in Codex's `hookSpecificOutput` shape, plus per-agent RAM. |
| `UserPromptSubmit` | `codex/user-prompt-submit.sh` | Same policy as Cursor's `beforeSubmitPrompt` (pressure, RSS, snooze, `proceed_once`, queue enqueue). |
| `PreToolUse` (`Bash\|Agent\|spawn_agent`) | `codex/pre-tool-use.sh` | Silent when clear; rate-limited advisory at `strained` / `critical`; opt-in deny of `spawn_agent` via `[tool_gate] deny_subagents_on` (see [claude-code.md](claude-code.md#stopping-subagent-fan-out-under-pressure-opt-in)). |

## Differences vs Cursor

| | Cursor | Codex |
|---|--------|--------|
| Config | `~/.cursor/hooks.json` | `~/.codex/hooks.json` |
| Prompt event | `beforeSubmitPrompt` | `UserPromptSubmit` |
| Block shape | `continue: false`, `user_message` | `decision: "block"`, `reason` |
| Activation | Automatic | Needs the trust step above |
| Slash commands | `~/.cursor/commands/guardian-*.md` | Not installed; use **`~/.guardian/guardian`** or macOS **`Guardian-*.command`** |

Resume paths (`guardian snooze`, `once`, zeno) are identical; see [hooks/resources.md](../hooks/resources.md).

## Smoke test

```bash
bash tests/codex-hook-smoke.sh
```
