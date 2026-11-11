# Guardian + Claude Code

Guardian drives the same **prompt gate**, **session context**, and **tool advisory** in Claude Code
through its [hooks](https://code.claude.com/docs/en/hooks). The hooks live in the user-level
`~/.claude/settings.json`, so they apply to the CLI, the desktop app's Code tab, and the IDE
extensions alike.

## Requirements

- `guardiand` installed and running (`bash scripts/install.sh`) — without fresh metrics every hook
  fails open and the gate is off.
- `jq` (the installer refuses to touch `settings.json` without it).

## Install

```bash
bash scripts/install-claude-hooks.sh
```

This copies scripts to **`~/.claude/hooks/guardian/`** and merges three entries into
**`~/.claude/settings.json`** (backup: `settings.json.guardian.bak`). Re-running is safe: existing
Guardian entries are replaced, never duplicated, and other hooks and settings are left alone.
`CLAUDE_CONFIG_DIR` is honored.

Sessions snapshot their hooks at startup — **start a new session** (or open `/hooks`) after installing.

## What each hook does

| Event | Script | Behavior |
|---|---|---|
| `SessionStart` | `claude/session-start.sh` | Adds one line of context: pressure, free RAM, per-agent RAM (Claude / Codex / Cursor), disk. Says so loudly when the daemon is not running. |
| `UserPromptSubmit` | `claude/user-prompt-submit.sh` | The prompt gate. Returns `decision: "block"` when `[prompt_gate]` says so (default: `critical` pressure). At `strained` it adds a short advisory instead. |
| `PreToolUse` (`Bash\|Agent\|Task`) | `claude/pre-tool-use.sh` | Silent when load is clear. At `strained` / `critical` adds an advisory as `additionalContext`, at most once per 2 minutes per session. Optionally **denies subagent spawns** (see below). |

The `PreToolUse` hook **never returns `permissionDecision: "allow"`** — that would skip Claude Code's
own permission prompt. It either says nothing, adds context, or (opt-in) denies a subagent.

## Stopping subagent fan-out under pressure (opt-in)

The prompt gate only sees *new prompts*. A long agentic turn can keep spawning subagents — each one
a separate process — after pressure has already gone critical. To stop that, set in
`~/.guardian/config.toml` and restart `guardiand`:

```toml
[tool_gate]
deny_subagents_on = "critical"   # "never" (default) | "strained" | "critical"
```

The `Agent` / `Task` tool call is then denied with a reason telling the agent to do the work
serially in the current session. `Bash` is never denied. A snooze lifts the deny. The same setting
applies to Cursor (`Task`) and Codex (`spawn_agent`).

## When a prompt is blocked

Claude Code shows the block reason and your original prompt. From any terminal:

```bash
~/.guardian/guardian snooze 15   # gates off for 15 minutes
~/.guardian/guardian once        # let exactly one prompt through
```

Then send again. macOS double-click helpers (`~/.guardian/Guardian-*.command`) work too. Cursor's
`/guardian-snooze` slash commands are not installed for Claude Code.

## Differences vs Cursor

| | Cursor | Claude Code |
|---|--------|-------------|
| Config | `~/.cursor/hooks.json` | `~/.claude/settings.json` (`hooks` key) |
| Prompt event | `beforeSubmitPrompt` | `UserPromptSubmit` |
| Block shape | `continue: false`, `user_message` | `decision: "block"`, `reason` |
| Tool advisory | `agent_message` on every call | `additionalContext`, rate limited |
| Session id | `conversation_id` | `session_id` (stored in the same `sessions.db` column) |
| Not ported | — | `beforeReadFile` (`.cursorignore` advisory), `postToolUse` / `stop` ROI tracking |

## Verify

```bash
jq '.hooks | keys' ~/.claude/settings.json
echo '{"source":"startup"}' | ~/.claude/hooks/guardian/claude/session-start.sh | jq -r '.hookSpecificOutput.additionalContext'
```

Hook behavior under simulated pressure is covered by the containerized suite
(`docker compose -f tests/stress/docker-compose.test.yml run --rm guardian-test`, group 7b).

## Uninstall

Remove the three entries whose command contains `/hooks/guardian/` from `~/.claude/settings.json`
(or restore `settings.json.guardian.bak`), then `rm -rf ~/.claude/hooks/guardian/`.

## Caveats

- **Managed / enterprise settings** may restrict hooks (`allowManagedHooksOnly`, etc.); see
  [Hook configuration](https://code.claude.com/en/settings#hook-configuration).
- **Upgrading from Guardian 0.3.1:** that version's `PreToolUse` hook ran on every tool and answered
  `permissionDecision: "allow"`, which auto-approved tool calls. Re-run
  `bash scripts/install-claude-hooks.sh` — it replaces the old entries (including duplicates left by
  repeated installs) with the advisory-only hook.

## Smoke test

```bash
bash tests/claude-hook-smoke.sh
```
