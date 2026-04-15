# Guardian + Claude Code

Guardian can drive the same **prompt gate**, **session context**, and **pre-tool advisories** via Claude Code’s hooks ([Hooks reference](https://code.claude.com/docs/en/hooks)).

## Requirements

- `jq` for the installer merge step.
- Claude Code reads hook definitions from **`~/.claude/settings.json`** (or project `.claude/settings.json`). The installer only updates the user-level file unless you merge manually.

## Install

From your Guardian git clone (after `cargo build --release` if you want the latest `guardian` binary copied to `~/.guardian/`):

```bash
bash scripts/install-claude-hooks.sh
```

This installs scripts under **`~/.claude/hooks/guardian/`** and merges the **`hooks`** object into **`~/.claude/settings.json`** (backup: `settings.json.bak` when merging), registering:

- **`UserPromptSubmit`** — same policy as Cursor’s `beforeSubmitPrompt` / Codex’s `UserPromptSubmit` (pressure, RSS, snooze, `proceed_once`, queue enqueue). Implemented by the shared Codex adapter script.
- **`SessionStart`** — same context as Cursor’s `sessionStart`, in Claude’s `hookSpecificOutput` shape (matchers: `startup`, `resume`, `clear`, `compact`).
- **`PreToolUse`** — always allow; under strain or critical load, injects **`additionalContext`** with system pressure (Claude’s `hookSpecificOutput` shape).

Restart Claude Code after changing `settings.json` or hook scripts.

## Differences vs Cursor and Codex

| | Cursor | Codex | Claude Code |
|---|--------|--------|--------------|
| Config | `~/.cursor/hooks.json` | `~/.codex/hooks.json` | `~/.claude/settings.json` → `hooks` |
| Prompt event | `beforeSubmitPrompt` | `UserPromptSubmit` | `UserPromptSubmit` |
| Block shape | `continue: false`, `user_message` | `decision: "block"`, `reason` | `decision: "block"`, `reason` |
| Tool advisories | `preToolUse` (`permission`, `agent_message`) | (not installed by Guardian) | `PreToolUse` → `hookSpecificOutput.permissionDecision` + `additionalContext` |
| Slash commands | `~/.cursor/commands/guardian-*.md` | — | — |

Resume paths (`guardian snooze`, `once`, zeno) are identical; see [hooks/resources.md](../hooks/resources.md).

Session metrics in `session-start` still describe **Cursor** process memory and `~/.cursor/projects` from `state.json` (daemon-sourced). That remains accurate if you use Cursor alongside Claude Code on the same machine.

## Caveats

- **Managed / enterprise settings** may restrict hooks (`allowManagedHooksOnly`, etc.); see [Hook configuration](https://code.claude.com/en/settings#hook-configuration).
- **Blocking in subagent or multi-tool flows** may not always match expectations; if you rely on hard blocking, check recent issues in the upstream Claude Code repo.

## Smoke test

```bash
bash tests/claude-hook-smoke.sh
```
