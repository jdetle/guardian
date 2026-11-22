---
date: 2026-11-20
slug: disk-scan-efficiency
mode: diff-audit
status: proceed
scope: src/disk_targets.rs, src/bin/guardian.rs, app/Guardian/Core/GuardianClient.swift, app/Guardian/App/MenuBarView.swift
diff_base: origin/main
veto_fired: false
follow_up_open: true
caught_real_issue: pending
---

# Adversarial review — disk scan efficiency

**Date:** 2026-11-20
**Branch:** `main` at `5091e1b` (guardian 0.11.0, same version as the installed CLI)
**Scope:** No diff. This audits the shipped `guardian disk` scan and asks how to make finding
space to free cost the user less time. There is no persona roster in this repository
(`docs/adversarial-review/personas/` does not exist), so this is a single-reviewer
offense / defense / synthesis.

## How it was measured

All measurements are read-only and were taken on the author's machine (228 GB volume, 93% used,
memory pressure "strained", swap about 90%) with the installed `~/.guardian/guardian`.

| Run | Budget | Wall time | Targets of 100 MB or more | Space found | Not measured |
|---|---|---|---|---|---|
| 1 | 45 s | 45.0 s | 24 | 21.0 GB | 43 places |
| 2 (straight after run 1) | 45 s | 45.0 s | 24 | 21.8 GB | 43 places |
| 3 | 240 s | about 157 s awake | 58 | 49.7 GB | 0 |

Run 3 took 1264 s by the wall clock because the lid was closed from 19:38:41 to 19:57:14
(`pmset -g log`). The budget uses a clock that stops during sleep, so the scan used about 157 s
of awake time: 34 s before sleep, 6 s in a dark wake, 117 s after. Treat 157 s as an estimate.

A small C benchmark (not in the repository) compared two ways to total one 1022 MB
`node_modules` of 73,445 entries, and read how much space deleting each file would free:

| Method | Run A | Run B | System calls |
|---|---|---|---|
| `readdir` plus one `lstat` per entry (what `dir_size` does) | 5.30 s | 3.12 s | 80,300 |
| `getattrlistbulk` | 2.54 s | 2.35 s | 20,564 |

| Folder | Size Guardian reports | Freed at once if deleted (`ATTR_CMNEXT_PRIVATESIZE`) |
|---|---|---|
| a worktree's `node_modules` (installed by bun) | 1022 MB | 0 MB |
| the main checkout's `node_modules` of the same project | 1047 MB | 0 MB |
| `~/.bun/install/cache` | 2362 MB | 387 MB |
| `guardian/target` | 1448 MB | 1385 MB |

The private-size reading was checked against controls: a plain 3 MB file reports 3 MB, a 3 MB
file and its `cp -c` clone report 0 MB between them.

## Offense

1. **There is no background scanning. Every scan starts from nothing, and the default budget
   never finishes.** `survey_home` keeps nothing between runs (`src/disk_targets.rs:751-868`);
   the menu bar holds its report in memory only (`GuardianClient.swift:21`) and scans only when
   the button is pressed (`MenuBarView.swift:259`). Runs 1 and 2 both used the whole 45 s and
   both left the same 43 places unmeasured, so repeating the scan does not converge. The 45 s
   scan found 21.8 of 49.7 GB (44%).

2. **What the budget drops is the largest, most recent work.** Candidates are measured oldest
   first (`disk_targets.rs:765-786`). Missing from the 45 s report: a 3.2 GB worktree and a
   2.1 GB worktree touched today, 2.9 GB and 2.8 GB worktrees touched 7 to 9 days ago, and
   `guardian/target` at 1.4 GB.

3. **Sizes are not what deleting frees.** `dir_size` sums allocated blocks per file
   (`disk_targets.rs:607-624`) and `delete_target` reports that same sum as "freed"
   (`disk_targets.rs:1090`, `1106-1109`). Both projects with large `node_modules` here use bun,
   which installs by cloning from its cache. The two folders sampled report about 1 GB each and
   would free 0 MB. The full scan lists 20 `node_modules` targets totalling 15.1 GB under
   "comes back on its own"; the two sampled suggest most of that is the same blocks counted
   repeatedly. A person can spend the whole cleanup deleting those and get almost nothing back.

4. **The scan is first run at the worst moment.** The session hook suggests `guardian disk`
   only at the critical level (`hooks/session-start.sh:140`), when the machine is already
   short of disk and usually of memory. The first useful list is then 45 s to 2.5 minutes away.

5. **Git safety checks run for every worktree during discovery, before sizes are known.**
   `worktree_candidates` calls `worktree_state` for each linked worktree
   (`disk_targets.rs:549-557`), which starts two or three git processes
   (`disk_targets.rs:466-505`), inside the third of the budget reserved for discovery
   (`disk_targets.rs:755`). Entries under the size floor are then discarded
   (`disk_targets.rs:822`): in run 2, 7 of the 13 worktrees listed at a 1 MB floor were under
   100 MB. One repository here has 27 linked worktrees. Deleting repeats the checks anyway
   (`disk_targets.rs:1038`).

6. **Deleting walks the folder to report its size, and checks twice.** `delete_target` runs
   `plan_delete`, then `dir_size` for up to 20 s, then `plan_delete` again
   (`disk_targets.rs:1089-1092`). For a worktree that is two rounds of `git status` plus
   `git ls-files`, then git's own check inside `git worktree remove`.

7. **Discovery can stop early without saying so.** `walk` returns at the discovery deadline
   (`disk_targets.rs:225-227`) and so does the worktree loop (`disk_targets.rs:551-553`). The
   report has no field for this and still prints "Scanned N git repositories"
   (`disk_targets.rs:930`). Not observed here: 53 repositories at both budgets.

8. **Menu bar: a delete can wait behind a scan.** Scan and delete share one serial queue
   (`GuardianClient.swift:42`, `121`, `188`), and the bin is disabled only while another delete
   runs (`MenuBarView.swift:347-354`), so a delete confirmed during a scan waits up to 45 s.
   Any scan failure shows "needs guardian 0.8 or newer" (`GuardianClient.swift:138`).

## Defense

- Deleting is careful and should stay exactly as it is: nothing is trusted from an earlier
  scan (`plan_delete`, `disk_targets.rs:1022-1079`), worktrees go through `git worktree remove`
  without `--force`, and `guard_path` refuses anything outside or near the top of home. A
  cached index does not weaken this as long as delete keeps reading the disk.
- Stateless scanning has one real advantage: it can never show a stale list. A cache must show
  its age and must not be used to decide whether something is safe.
- Oldest first is a sensible order for a budget that runs out. The fault is the lack of memory
  between runs, not the order.
- The scan is light: 9 MB resident, about 12 s of CPU per 45 s, utility I/O priority
  (`disk_targets.rs:688-701`). A background scan can keep that profile.
- Bulk attribute reads gave 1.3 to 2.1 times on one folder, on a warm cache. That is worth
  having but is not the main win; it was not tested cold or with several threads.
- Private size understates a group of clones: deleting every copy frees the data, although
  each copy alone reports 0. A report needs both numbers, not just the smaller one.

## Synthesis

**Decision:** Proceed. Recommended order, by time saved for the person freeing space:

1. **Keep an index and make the scan resumable.** Write candidates, sizes and measured-at
   times to `~/.guardian/disk_index.json`. A budgeted run measures what has gone longest
   without measuring, so runs add up instead of repeating. `guardian disk` and the menu bar
   show the index at once with its age, then refresh. Delete keeps re-checking from disk.
2. **Refresh in the background, in its own process.** `guardiand` starts a short-lived
   `guardian disk refresh` when pressure is clear and the index is old, and promptly when the
   disk reaches `warn` (85%), so the list is ready before `critical` (93%). Not inside
   `run_loop`, which samples every 2 s (`src/lib.rs:362` already guards against slow samples).
   Keep `HOME_SKIP` so a background scan never raises a macOS permission prompt.
3. **Report what deleting frees.** Read sizes with `getattrlistbulk`, taking
   `ATTR_CMNEXT_PRIVATESIZE` next to allocated size. Rank by what is freed, and show clone
   groups as one row ("13 copies share about 1 GB; freed only when all go, cache included").
   Report "freed" after a delete from the volume's free space before and after.
4. **Re-measure only what changed.** Ask FSEvents for changes since the saved event ID and
   mark the owning candidates dirty; fall back to a full pass when history has been dropped.
   Not measured in this review.
5. **Cheaper steps.** Run worktree git checks after the size floor, not before. Drop the
   pre-delete walk once the free-space change gives the number, which also removes the need
   to run `plan_delete` twice. Give scan and delete separate queues in the app. Add a
   `discovery_complete` field to the report.

**Unresolved risk:** A background scan adds I/O on a machine Guardian exists to protect; the
gating in step 2 has not been tried under load.

**Follow-up:** author — implement steps 1 to 3 as one MINOR release (0.12.0), tests in a
scratch home as the existing disk tests do.

## Addendum — what was built (branch `feat/disk-index-background-scan`, 0.12.0)

Steps 1 to 3 are implemented, plus the worktree half of step 5, which turned out to be
required rather than optional: with the index in place, the search itself was the cost that
remained. Each scan ran `git status` in every linked worktree (1.4 s in a large one, 92
worktrees under the two roots measured), so a search needed about 80 s and never finished
inside the 15 s a 45 s scan allows it. That also corrects finding 7 above: the 45 s scans in
this review were cutting discovery short, in the worktree checks, without saying so. Git is
now asked as part of measuring a worktree, so it is budgeted, resumed and reused like a size.

Not built: FSEvents (step 4), the pre-delete walk, separate app queues.
Not verified: `guardiand` starting the scan (the daemon is never run on the host; only the
decision function is unit-tested), and the menu bar app beyond compiling.
