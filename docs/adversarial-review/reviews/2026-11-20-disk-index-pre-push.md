---
date: 2026-11-20
slug: disk-index-pre-push
mode: diff-audit
status: proceed
scope: src/disk_targets.rs, src/disk_targets/save.rs, src/dir_list.rs, src/background_scan.rs, src/bin/guardian.rs, src/lib.rs, src/config.rs, app/Guardian
diff_base: origin/main
veto_fired: true
follow_up_open: true
caught_real_issue: yes
---

# Adversarial review — disk index, bundles and save (pre-push)

**Date:** 2026-11-20
**Branch:** `feat/disk-index-background-scan`
**Scope:** `git diff origin/main...HEAD --stat`: 18 files changed, about 5,400 insertions, 300 deletions.

Two independent reviews by two different models, one after the other, each given the diff
and the code and not the other's findings or the author's conclusions. The second reviewed
the branch after the first review's fixes. Both were read-only.

## Offense

First review, 19 findings. The ones that changed the code:

- **Critical: deleting a worktree destroyed a checkout nested in one of its ignored
  folders.** `worktree_state` treated ignored files as "look first, deletable";
  `git worktree remove` deletes an ignored folder holding a second worktree or a clone,
  unsaved work included. Reproduced by the reviewer in a scratch repository.
- `disk save` pushed onto a branch that already exists on the remote, which is usually a
  pull request (CI, reviewers, auto-merge).
- The remote's default branch was read from a local ref that only exists after a clone, so
  shared branches were recognised by a short list of names alone.
- `branch.<b>.pushRemote` and `remote.pushDefault` were ignored: in a fork setup the push
  went to the upstream repository.
- Secrets were checked by file name only, and commit hooks (where scanners run) were skipped.
- A search that used its whole share of time left no time to list worktrees, and was
  repeated unchanged by every later scan.
- Smaller: unbounded `git add -A` of an unignored dependency folder; a commit could include
  files not shown; a failed commit left the worktree on a new branch; the push was
  "verified" against a local ref; a build folder in a nested repository was counted twice;
  folders inside the Go module cache were listed as project folders; a locked worktree
  could be re-checked as unlocked; ticks survived a rescan in the panel; the `clean` prompt
  counted worktrees it had just dropped; copied commands broke on `!`; a listing that failed
  part-way counted as complete.

Second review, 15 findings on the corrected branch. The ones that changed the code:

- Files already staged (`git add -f .env`) skipped every size and secret check, and edits to
  tracked files were never scanned.
- The nested-checkout guard missed an ignored folder with a non-ASCII name, because
  `worktree_state` read `git status` without `-z` and got an escaped path. Reproduced.
- Rolling back a failed commit did not check whether the commit had in fact been made.
- A push remote that is set but is not a named remote fell through to `origin`.
- The plan id covered file names, not their contents.
- The daemon waited on a killed scan inside its sampling loop.
- Smaller: the worktree listing kept asking git after its deadline; a folder that is no
  longer a worktree could be kept in the list; background scans ignored `[disk] enabled`;
  `--set-upstream` changed repository configuration unasked; a malformed directory entry
  ended a listing as if complete; the tests read the developer's git configuration; a
  package cloned into `node_modules` was not looked for.

## Defense

- Every finding above is fixed on the branch, each with a test where one could be written
  (228 unit tests; the save tests push only to bare repositories inside a scratch folder,
  with the host's git configuration shut out).
- Both reviews examined and found sound: `plan_delete` / `delete_target` (re-checked from
  disk, never forced, home-relative guard), the `getattrlistbulk` parser's bounds and field
  order, the index lock and atomic save, the daemon's scheduling and its cap on follow-up
  scans, argument passing to `Process` in the app, and that a failed save never leads to a
  delete.
- Not fixed, by decision:
  - A scan can overrun its budget by one worktree check when git hangs (three git calls of
    up to 20 s). The budget is checked between checks, not inside git.
  - A worktree inside another worktree is listed in its own row as well as inside the outer
    one's size where the outer one is not listed; the outer one cannot be deleted then.
  - Results found only through `--root` are dropped by the next scan without that root;
    keeping them would have background scans reading folders macOS guards. Documented.
  - The search for repositories is not resumable. It is given more time after being cut
    short, and the README no longer claims otherwise.

## Synthesis

**Decision:** Proceed. One finding was a veto (data loss through a nested checkout); it is
fixed and tested, including the variant the second review found.

**Unresolved risk:** `disk save` has only been run against scratch remotes. Its behaviour
with real credentials, push hooks and branch protection on a hosted remote is untested, and
its secret checks are a backstop, not a scanner.

**Follow-up:** author — run one real `guardian disk save --plan` and one real save on a
throwaway branch before relying on it; make the scan budget hold inside git calls.
