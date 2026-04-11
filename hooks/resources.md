# Guardian — help Cursor avoid overexertion

Short checklist (also surfaced by hooks when load is high):

- **Fewer parallel Agent/Composer chats** — finish or archive threads before opening new heavy ones.
- **`.cursorignore`** — exclude `node_modules`, build outputs (`target/`, `dist/`, `build/`), caches, large assets. See [Cursor: Ignore files](https://cursor.com/docs/context/ignore-files).
- **Context** — see [Cursor: Context](https://cursor.com/docs/context) for how indexing uses disk and memory.
- **Extensions** — disable nonessential extensions; test with `cursor --disable-extensions` if diagnosing spikes.
- **Long chats** — export or start a fresh thread when a conversation grows huge (renderer memory).
- **Docker / dev servers** — Guardian can throttle nonessential containers when pressure is high (`~/.guardian/config.toml` `[docker]`).

**Resume blocked prompts (human in the loop):**

- One-shot: `touch ~/.guardian/proceed_once` then submit again.
- Snooze gates ~N minutes: `bash scripts/guardian-resume.sh snooze 15` (from the Guardian repo) or write an ISO timestamp into `~/.guardian/snooze_until` (see script).
