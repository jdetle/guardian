---
date: 2026-11-21
slug: hooks-all-clients-pre-push
mode: diff-audit
status: proceed
scope: hooks/, scripts/install*.sh, src/agent_hooks.rs, src/bin/guardian.rs, src/session_db.rs, app/Guardian, tests/stress, docs
diff_base: feat/disk-index-background-scan
veto_fired: false
follow_up_open: true
caught_real_issue: yes
---

# Adversarial review — hooks for three agents, session following, warn-only default (pre-push)

**Date:** 2026-11-21
**Branch:** `feat/hooks-all-clients`, stacked on `feat/disk-index-background-scan`
**Scope:** `git diff feat/disk-index-background-scan...HEAD`: about 45 files, 2,400 insertions.

Two independent reviews by two different models, one after the other, each given the diff
and the code and not the other's findings or the author's conclusions. The second reviewed
the branch after the first review's fixes, and was also asked to try to make any hook pause
a prompt or deny a tool call without the opt-in. Both were read-only.

## Offense

First review, 14 findings. The ones that changed the code:

- A tool call recorded after its session had ended brought the session back to "working":
  that hook runs in the background in Claude Code and can land last.
- Recording a prompt or a session's end waited up to 5 s for a busy database while the agent
  waited on the hook (Claude Code gives a session-end hook about 1.5 s in all).
- `guardian hooks status` failed for an agent that has no Guardian hooks at all, and the app
  offered to "update" such an agent, which would have installed hooks nobody asked for.
- Hook status did not recognise hooks installed under a path that had to be shell-quoted, or
  `CLAUDE_CONFIG_DIR` / `CODEX_HOME` given with a trailing slash.
- Installers stamped the hooks' version before the hooks were registered, so a failed merge
  read as up to date.
- The hooks embedded in the binary were unpacked to one fixed folder, shared between runs.
- Smaller: a missing `~/.guardian` was a hook failure; a pressure sample was dropped when
  `state.json` had no Docker section; the docs understated what the end of a turn costs and
  did not say that an interrupted turn reads as working.

Second review, 13 findings on the corrected branch. Its attempt to get a block or a deny
without the opt-in failed (540 runs over agents, load levels, policies and inputs); so did
its attempts to make a recording hook print anything or exit non-zero. What it did find:

- **The warning that stands in for a pause told the agent how to turn pausing on.** In
  Claude Code and Codex the hook's text goes to the model, and it ended with the exact
  command. An agent reading "run `guardian blocking on`" as a suggestion would have made the
  opt-in for the person.
- Codex documents `"async": true` for hooks but may skip handlers that set it. The two
  recorders registered that way could have recorded nothing.
- `tests/claude-hook-smoke.sh` and `tests/codex-hook-smoke.sh`, which predate session
  recording, now wrote rows into the developer's real `~/.guardian/sessions.db`.
- A session killed in the middle of a turn and resumed stayed "working".
- The database upgrade marked every earlier session ended, including ones still open.
- README, `hooks/resources.md` and the header of `hooks/lib.sh` still described blocking as
  the default in five places; two Markdown tables were broken by a blank line.
- Smaller: session start mentioned allow-once while nothing could be paused; the app's "Add
  hooks" text left out what Cursor's installer also writes; `blocking status` ignored
  per-agent modes and the memory-cap rule; an absurd or negative tool duration was stored
  as sent; the two new test scripts did not check that `mktemp` worked; the app does not
  see `CLAUDE_CONFIG_DIR` / `CODEX_HOME` set in a shell.

## Defense

- Every finding above is fixed on the branch, with a test where one could be written
  (229 unit tests; 47 checks of session recording and 51 of the opt-in, both run against a
  scratch home on macOS under `/bin/bash` 3.2 and inside the hook container).
- The opt-in now has three parts: the warning an agent reads in Claude Code and Codex does
  not name the command; `guardian blocking on` says what it will mean and asks, and without
  a terminal does nothing unless given `--yes`; the app asks in a dialog before passing
  `--yes`. This is a guard against an agent drifting into the opt-in, not a security
  boundary: an agent that sets out to run `guardian blocking on --yes` still can.
- Codex's recorders run in the foreground with short timeouts and a 0.4 s wait for a busy
  database; only Claude Code's background tool recorder waits longer.
- Both reviews examined and found sound: the SQL built in `jq` (every value quoted, hostile
  input stored as text), the schema upgrade run twice at once, the embedded-hook install
  into scratch homes for all three agents, the merge into existing hook settings with a
  backup, and that no recording hook can continue a turn or change a permission decision.
- Not fixed, by decision:
  - Events can arrive out of order, and a turn the person interrupts never reports its end.
    Such a session reads as working until it has been quiet for half an hour.
  - Codex runs new or changed hooks only after the person approves them; until then its
    sessions are not followed. The installer and the app say so.
  - The app looks in `~/.claude` and `~/.codex` only. Documented; the command honours the
    environment.

## Synthesis

**Decision:** Proceed. No veto. The finding that mattered most was the warning text: the
default was already warn-only in every path tested, but the text made the opt-in something
an agent could plausibly perform. That is closed as far as a prompt can close it.

**Unresolved risk:** the hooks have been run against recorded and hand-written event
payloads, not against live Cursor and Codex sessions on this branch; the container suite was
not run locally (Docker was not running) and is left to CI. The fixes made after the second
review were tested but not reviewed again by a second model.

**Follow-up:** author — after installing, open one real session in each of Cursor, Claude
Code and Codex and confirm each appears in the app and goes working → idle → ended.
