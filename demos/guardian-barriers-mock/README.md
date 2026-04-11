# Guardian barriers — offline mock

Runs the real shell hooks against a **temporary** `HOME` with synthetic `~/.guardian/state.json` and `hook_policy.json`.

```bash
bash demos/guardian-barriers-mock/run-demo.sh
```

Requires `bash`, `jq`, and `python3` on `PATH` (same as production hooks). Does **not** start `guardiand` or Cursor.
