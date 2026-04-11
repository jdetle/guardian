---
name: guardian-upgrade
description: >-
  Upgrades and reinstalls the System Guardian stack (guardiand, hook_policy.json,
  Cursor hooks, optional queue-watch and Swift menu bar app) from a local clone of
  the guardian repo. Use when the user wants the latest Guardian everywhere, after
  git pull, or when hooks/daemon are stale; includes explicit restart order for
  LaunchAgent and Cursor.
---

# Guardian — install / upgrade / restart (global workflow)

Guardian is installed **per machine** under `~/.guardian/` and `~/.cursor/hooks/guardian/`. The **source of truth** for binaries and scripts is a **git clone** of the [guardian](https://github.com/jdetle/guardian) repository (or your fork). This skill gives a **single ordered checklist** agents should follow so upgrades are consistent.

## When to apply this skill

- User asks to **install**, **upgrade**, **update**, or get the **latest Guardian**.
- After **`git pull`** on the guardian repo or switching branches.
- Hooks behave oddly (old `before-submit-prompt.sh`, missing `hook_policy` keys, queue CLI missing).
- **`~/.guardian/hook_policy.json`** does not match **`~/.guardian/config.toml`** (daemon not restarted).

## Prerequisites

- **macOS 14+**
- **Rust** (`rustup`) for `scripts/install.sh`
- **`jq`**, **`bash`**, **`python3`**
- Path to the guardian repo: `$GUARDIAN_REPO` (e.g. `~/github/guardian` or `~/src/guardian`)

## Global skill install (optional)

To use this skill in **all** Cursor projects, symlink or copy the skill into your user skills directory:

```bash
mkdir -p ~/.cursor/skills
ln -sf "$GUARDIAN_REPO/.cursor/skills/guardian-upgrade" ~/.cursor/skills/guardian-upgrade
```

Or copy the `guardian-upgrade` folder there if you prefer not to symlink.

---

## Upgrade checklist (run in order)

Set the repo root (adjust path):

```bash
export GUARDIAN_REPO="$HOME/github/guardian"   # change if needed
cd "$GUARDIAN_REPO"
```

### 1. Get latest sources

```bash
git fetch origin
git checkout main
git pull origin main
```

Resolve merge conflicts before continuing.

### 2. Build daemon, install LaunchAgent, start `guardiand`

```bash
bash scripts/install.sh
```

This **release-builds** `guardiand`, updates **`~/Library/LaunchAgents/com.guardian.guardiand.plist`**, **reloads** the service, and writes **`~/.guardian/hook_policy.json`** from **`config.toml`** at daemon startup.

**Do not skip** if `config.toml` or Rust code changed — otherwise hooks still read an old **`hook_policy.json`**.

### 3. Refresh Cursor hooks and queue CLI

```bash
bash hooks/install-hooks.sh
```

Copies scripts to **`~/.cursor/hooks/guardian/`**, installs **`~/.guardian/guardian-queue.sh`** when present in the repo, and merges **`~/.cursor/hooks.json`**.

### 4. Restart Cursor (required for hook script changes)

Cursor loads **`hooks.json`** and hook paths at startup. After hook updates:

- **Fully quit Cursor** (Cmd+Q), then reopen, **or** use the command palette “Reload Window” if hooks still misbehave.

### 5. Optional — queue pressure notifier

If the repo ships **`scripts/install-queue-watch.sh`** and you use the agent queue:

```bash
bash scripts/install-queue-watch.sh
```

Reloads **`com.guardian.queue-watch`**. Skip if you do not use queue notifications.

### 6. Optional — Swift menu bar app

Only if the user wants the menu bar extra (queue + metrics):

```bash
cd "$GUARDIAN_REPO/app"
swift build -c release
```

Binary: **`app/.build/release/Guardian`**. There is no single system-wide “install” path in-tree; copying to **`/usr/local/bin/`** or running from the build path is fine.

---

## Restart order (summary)

| Step | What restarts |
|------|-----------------|
| `bash scripts/install.sh` | **LaunchAgent** `com.guardian.guardiand` — new binary + **`hook_policy.json`** |
| `bash hooks/install-hooks.sh` | Files under **`~/.cursor/hooks/guardian/`** — **Cursor must reload** |
| Quit/reopen Cursor | Picks up **`hooks.json`** and new hook scripts |
| `install-queue-watch.sh` | **`com.guardian.queue-watch`** only |

---

## Verification (agent should run or suggest)

```bash
launchctl list com.guardian.guardiand
cat ~/.guardian/state.json | jq '{pressure, sampled_at}'
cat ~/.guardian/hook_policy.json | jq 'keys'
test -x ~/.guardian/guardian-queue.sh && echo "queue CLI ok"
cat ~/.cursor/hooks.json | jq '.hooks | keys'
```

- **`sampled_at`** should be recent (within ~30s if daemon is healthy).
- **`hook_policy.json`** should include keys the current README expects (e.g. `prompt_gate`, `session_budget`, `disk`, `queue`).

---

## Troubleshooting

| Symptom | Action |
|--------|--------|
| Daemon not listed in `launchctl` | `cat ~/.guardian/guardiand.stderr.log` — fix Rust build / paths, rerun **`install.sh`**. |
| Old hook messages (e.g. folder-count session budget) | Hooks not updated — rerun **`hooks/install-hooks.sh`**, **restart Cursor**. |
| **`hook_policy`** out of date vs **`config.toml`** | Restart daemon: **`launchctl unload ~/Library/LaunchAgents/com.guardian.guardiand.plist`** then **`launchctl load ...`**, or rerun **`install.sh`**. |
| User has no local clone | Clone the repo first, then run this checklist from **`$GUARDIAN_REPO`**. |

---

## Related project docs

- Full install detail: [`.cursor/skills/guardian-install/SKILL.md`](../guardian-install/SKILL.md) (same repo).
- Human README: [`README.md`](../../../README.md).

## Agent behavior

- Prefer **running commands** from the user’s actual **`$GUARDIAN_REPO`** path rather than guessing.
- If the user has no clone, **clone first** (`git clone …`) then run the checklist.
- After edits to **`config.toml`**, remind that **`install.sh`** (or LaunchAgent reload) is needed so **`hook_policy.json`** matches.
