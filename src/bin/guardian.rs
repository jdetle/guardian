//! User-facing `guardian` CLI — snooze / one-shot resume / zeno relaxation / pressure history.

use chrono::{Duration, Local, SecondsFormat, Utc};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use guardian_daemon::config::{
    config_path, guardian_dir, load_config, load_config_report, ConfigSource,
};
use guardian_daemon::disk_targets;
use guardian_daemon::stats::PressureStats;
use guardian_daemon::zeno::{self, apply_zeno, ZenoState};

/// Longest snooze the hooks honor (`GUARDIAN_MAX_SNOOZE_SECS` in hooks/lib.sh).
const MAX_SNOOZE_MINUTES: u32 = 120;

#[derive(Parser)]
#[command(name = "guardian", version)]
#[command(
    about = "System Guardian — snooze prompt gates, allow one submit, or relax thresholds (zeno)"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Snooze Guardian hooks across clients (default 15 minutes, at most 120).
    /// To switch a gate off for longer, set it in config.toml where it stays visible.
    Snooze {
        #[arg(default_value_t = 15, value_parser = clap::value_parser!(u32).range(1..=i64::from(MAX_SNOOZE_MINUTES)))]
        minutes: u32,
    },
    /// Allow the next prompt that Guardian would pause (expires after 10 minutes).
    Once,
    /// Remove snooze immediately.
    ClearSnooze,
    /// Relax effective limits toward full usage (halfway toward 100% each bump).
    Zeno {
        #[command(subcommand)]
        action: ZenoAction,
    },
    /// Inspect ~/.guardian/config.toml.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Find disk space to free: old build output, git worktrees and tool caches across
    /// all your git repositories. Listing is read-only; `clean` and `delete` remove things.
    /// Scans remember what they measured (~/.guardian/disk_index.json), so each one only
    /// measures what is new or has changed.
    Disk {
        #[command(flatten)]
        scan: ScanArgs,
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
        /// Show the last saved scan at once, without scanning.
        #[arg(long, conflicts_with = "fresh")]
        cached: bool,
        #[command(subcommand)]
        action: Option<DiskAction>,
    },
    /// Whether Guardian may stop things, or only warn. It only warns until you opt in:
    /// `blocking on` lets the hooks pause prompts and deny subagents when your config says
    /// to; `blocking off` goes back to warnings; `blocking status` says which it is.
    Blocking {
        #[command(subcommand)]
        action: BlockingAction,
    },
    /// Guardian's hooks for Cursor, Claude Code and Codex: see whether each agent has
    /// them and whether they are this version's, and install or update them. The hooks are
    /// carried in this binary; no copy of the repository is needed.
    Hooks {
        #[command(subcommand)]
        action: HooksAction,
    },
    /// Show how much time was spent at each pressure level, per day.
    Stats {
        /// How many of the most recent days to show.
        #[arg(long, default_value_t = 7, value_parser = clap::value_parser!(u32).range(1..=30))]
        days: u32,
    },
}

#[derive(clap::Args, Clone)]
struct ScanArgs {
    /// How many targets to list.
    #[arg(long, global = true, default_value_t = 15, value_parser = clap::value_parser!(u32).range(1..=100))]
    top: u32,
    /// Ignore anything smaller than this many megabytes.
    #[arg(long, global = true, default_value_t = 100)]
    min_mb: u64,
    /// Time budget for the scan, in seconds.
    #[arg(long, global = true, default_value_t = 45, value_parser = clap::value_parser!(u64).range(1..=600))]
    seconds: u64,
    /// Only list build folders and worktrees untouched for at least this many days.
    #[arg(long, global = true, default_value_t = 0)]
    older_than: u64,
    /// Also search this folder (repeatable). Your home folder is always searched, except
    /// Desktop, Documents and Downloads, which macOS guards with a permission prompt.
    #[arg(long, global = true)]
    root: Vec<PathBuf>,
    /// Measure everything again instead of reusing sizes saved by earlier scans.
    #[arg(long, global = true)]
    fresh: bool,
}

impl ScanArgs {
    fn options(&self) -> disk_targets::Options {
        disk_targets::Options {
            roots: self.root.clone(),
            min_bytes: self.min_mb.saturating_mul(1024 * 1024),
            top: self.top as usize,
            budget: std::time::Duration::from_secs(self.seconds),
            older_than_days: self.older_than,
            max_age: if self.fresh {
                std::time::Duration::ZERO
            } else {
                disk_targets::DEFAULT_MAX_AGE
            },
        }
    }
}

#[derive(Subcommand)]
enum DiskAction {
    /// Bring the saved scan up to date and print one line. guardiand runs this in the
    /// background; it does nothing if another scan is already running. Exits with status 3
    /// when it ran out of time with places still to measure.
    Refresh,
    /// Scan, then choose what to delete. Asks before deleting anything. Takes the same
    /// scan options as `guardian disk`, before or after the word `clean`.
    Clean,
    /// Commit and push the unsaved work in git worktrees, so that they can be removed
    /// without losing it. Commits what is uncommitted to the worktree's branch (a new
    /// branch for a detached HEAD or a shared branch) and pushes it. A branch already on
    /// the remote is left as it is: the save goes beside it as guardian-save/<branch>.
    /// Never forces a push; refuses new files that look like credentials.
    Save {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Remove each worktree once its work is on the remote.
        #[arg(long)]
        delete: bool,
        /// Say what would be committed and pushed, and change nothing.
        #[arg(long, conflicts_with_all = ["delete", "yes"])]
        plan: bool,
        /// Skip the repository's commit hooks (push hooks always run). For when a linter
        /// refuses work in progress; it also skips any secret scanner among those hooks.
        #[arg(long)]
        no_verify: bool,
        /// Save only if the worktree still matches the plan with this id (from --plan
        /// --json). For a caller that showed someone a plan before asking. One path only.
        #[arg(long, value_name = "ID")]
        plan_id: Option<String>,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
        /// Print JSON results.
        #[arg(long)]
        json: bool,
    },
    /// Delete specific targets from a scan. Each is re-checked first: a build folder must
    /// hold nothing git tracks, a worktree no uncommitted work.
    Delete {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
        /// Print JSON results.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum BlockingAction {
    /// Let Guardian pause prompts and deny subagents, as config.toml says when.
    On,
    /// Warnings only (the default): nothing is paused or denied.
    Off,
    /// Say whether blocking is on, and what would be stopped if it is.
    Status,
}

#[derive(Subcommand)]
enum HooksAction {
    /// Say, for each agent, whether Guardian's hooks are installed and up to date.
    /// Exits non-zero if an agent that is set up on this machine lacks current hooks.
    Status {
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Install or update the hooks. With no agent named: for every agent set up on this
    /// machine. Other hooks and settings in the agents' files are left alone.
    Install {
        #[arg(long)]
        cursor: bool,
        #[arg(long)]
        claude: bool,
        #[arg(long)]
        codex: bool,
        /// Only agents that already have Guardian hooks: bring those up to date, add none.
        #[arg(long, conflicts_with_all = ["cursor", "claude", "codex"])]
        upgrade: bool,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Report syntax errors, misspelled settings and values that cannot work.
    /// Exits non-zero if anything needs fixing.
    Check,
}

#[derive(Subcommand)]
enum ZenoAction {
    /// Increment the zeno step counter (daemon picks this up on the next sample).
    Bump,
    /// Decrement the zeno step by one (restore previous effective limits; min 0).
    Rollback,
    /// Reset zeno steps to 0 (back to config.toml thresholds only).
    Reset,
    /// Show current steps and effective thresholds vs config.
    Status,
}

fn write_snooze_minutes(minutes: u32) -> Result<(), std::io::Error> {
    let expiry = Utc::now() + Duration::minutes(i64::from(minutes));
    let ts = expiry.to_rfc3339_opts(SecondsFormat::Secs, true);
    let dir = guardian_dir();
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!("snooze_until.{}.tmp", std::process::id()));
    let result = std::fs::write(&tmp, format!("{ts}\n"))
        .and_then(|()| std::fs::rename(&tmp, dir.join("snooze_until")));
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result?;
    println!(
        "Guardian snoozed until {}.",
        expiry.with_timezone(&Local).format("%H:%M %Z")
    );
    Ok(())
}

fn touch_proceed_once() -> Result<(), std::io::Error> {
    let p = guardian_dir().join("proceed_once");
    std::fs::create_dir_all(guardian_dir())?;
    // Made afresh so the token's age starts now: truncating an existing empty file is not
    // guaranteed to move its modification time, and the hooks expire the token by age.
    let _ = std::fs::remove_file(&p);
    std::fs::File::create(&p)?;
    println!("Guardian will allow the next prompt it would pause, for the next 10 minutes.");
    Ok(())
}

fn clear_snooze() -> Result<(), std::io::Error> {
    let p = guardian_dir().join("snooze_until");
    match std::fs::remove_file(p) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    println!("Guardian resumed.");
    Ok(())
}

fn home() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "could not find the home directory".to_string())
}

/// Ask a question on the terminal and read one line. Refuses when there is no terminal:
/// nothing is ever deleted on the strength of piped input.
fn ask(question: &str) -> Result<String, String> {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return Err("this needs a terminal to confirm in (or pass --yes to `disk delete`)".to_string());
    }
    print!("{question}");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
    Ok(line.trim().to_string())
}

fn said_yes(answer: &str) -> bool {
    matches!(answer.to_lowercase().as_str(), "y" | "yes")
}

/// Delete each path, reporting one line per target. Returns the bytes freed.
fn delete_all(paths: &[PathBuf], home: &std::path::Path) -> (u64, usize) {
    let (mut freed, mut failed) = (0u64, 0usize);
    for path in paths {
        let shown = disk_targets::display_path(path, home);
        match disk_targets::delete_target(path, home) {
            Ok(done) => {
                freed += done.freed_bytes;
                println!("  deleted {shown} ({})", disk_targets::human_size(done.freed_bytes));
            }
            Err(why) => {
                failed += 1;
                println!("  kept    {shown}: {why}");
            }
        }
    }
    (freed, failed)
}

fn print_free_space(home: &std::path::Path) {
    if let Some((used, avail)) = disk_targets::volume_usage(home) {
        println!("Disk now: {used:.1}% used, {avail:.1} GB free.");
    }
}

/// Bring the saved scan up to date, for running unattended.
fn disk_refresh(scan: &ScanArgs) -> Result<(), String> {
    match disk_targets::refresh(&scan.options()).ok_or("could not find the home directory")? {
        disk_targets::Refresh::Busy => println!("Another disk scan is running; left to it."),
        disk_targets::Refresh::Done(report) => {
            println!(
                "Disk scan saved: {} that comes back on its own, {} to look at first; {} sizes reused, {} places still to measure{}.",
                disk_targets::human_size(report.regenerates_freed_bytes),
                disk_targets::human_size(report.review_freed_bytes),
                report.reused,
                report.pending,
                if report.discovery_complete { "" } else { ", and the search was cut short" }
            );
            if report.work_left() {
                // Not a failure: what was measured is saved. It tells guardiand to run
                // another scan soon rather than wait a whole interval.
                std::process::exit(disk_targets::REFRESH_EXIT_MORE);
            }
        }
    }
    Ok(())
}

/// One numbered choice in `guardian disk clean`: a bundle of stale things that can go
/// together, or a single folder.
struct Offer {
    /// What deleting it would free.
    freed: u64,
    /// The line it is listed and confirmed by.
    title: String,
    /// What is said under it.
    detail: String,
    paths: Vec<PathBuf>,
    /// Needs a look before deleting; said next to it and again when confirming.
    look_first: bool,
    /// Picked by `old`: comes back on its own and has gone untouched for two weeks or more.
    old: bool,
    /// A worktree with unsaved work: it is committed and pushed before it is removed, and
    /// kept if that does not go through.
    save_first: bool,
}

/// What `guardian disk clean` can offer from a report: stale bundles first, then single
/// folders, each list in the report's own order.
fn offers(report: &disk_targets::Report, top: usize) -> Vec<Offer> {
    let single = |t: &disk_targets::Target| {
        let mut details: Vec<String> = Vec::new();
        if let Some(age) = t.age_text() {
            details.push(age);
        }
        details.push(t.note.clone().unwrap_or_else(|| t.cost.clone()));
        if let Some(shared) = t.shared_text() {
            details.push(shared);
        }
        Offer {
            freed: t.freed_bytes,
            title: format!("{} ({})", t.what, t.display_path),
            detail: details.join("; "),
            paths: vec![t.path.clone()],
            look_first: t.safety == disk_targets::Safety::Review && !t.saveable,
            old: false,
            save_first: t.saveable,
        }
    };
    let mut all = Vec::new();
    for b in &report.bundles {
        if !b.delete_paths.is_empty() {
            all.push(Offer {
                freed: b.deletable_freed_bytes,
                title: format!("{}: {} stale folder{}", b.name, b.delete_paths.len(), if b.delete_paths.len() == 1 { "" } else { "s" }),
                detail: b.detail_text(),
                paths: b.delete_paths.clone(),
                look_first: false,
                // Tool caches are shared by every project: chosen by number, never by `old`.
                old: b.is_project && b.staleness.is_old(),
                save_first: false,
            });
        }
    }
    // Stale folders that need a look first, or whose work must be saved first, are offered
    // one by one.
    for t in report.bundles.iter().flat_map(|b| &b.members) {
        if t.saveable || (t.deletable && t.safety == disk_targets::Safety::Review) {
            all.push(single(t));
        }
    }
    for t in report.targets.iter().filter(|t| t.deletable || t.saveable) {
        all.push(single(t));
    }
    all.truncate(top);
    all
}

fn disk_clean(scan: &ScanArgs) -> Result<(), String> {
    let home = home()?;
    println!("Scanning your git repositories and tool caches (up to {} s; sizes that have not changed are reused)...", scan.seconds);
    // Look at more than will be listed: only things Guardian can delete are offered.
    let mut opts = scan.options();
    opts.top = 100;
    let report = disk_targets::survey(&opts).ok_or("could not find the home directory")?;
    let offered = offers(&report, scan.top as usize);
    if offered.is_empty() {
        println!("Nothing Guardian can delete was found. `guardian disk` lists everything, including items that need a tool or a decision.");
        return Ok(());
    }

    println!(
        "\nDisk: {:.1}% used, {:.1} GB free. {} git repositories scanned. Stale things come first, bundled by project.\n",
        report.used_percent, report.available_gb, report.repos_scanned
    );
    for (i, offer) in offered.iter().enumerate() {
        let flag = match (offer.save_first, offer.look_first) {
            (true, _) => "  SAVED FIRST: committed and pushed, then removed",
            (_, true) => "  LOOK FIRST",
            _ => "",
        };
        println!(
            "{:>3}. {:>9}  {}{flag}\n                {}",
            i + 1,
            disk_targets::human_size(offer.freed),
            offer.title,
            offer.detail
        );
    }
    let old: Vec<usize> = (0..offered.len()).filter(|i| offered[*i].old).collect();
    println!();
    if !old.is_empty() {
        println!(
            "`old` selects the {} bundle(s) of things that come back on their own and have gone untouched for {}+ days ({}).",
            old.len(),
            disk_targets::OLD_DAYS,
            disk_targets::human_size(old.iter().map(|i| offered[*i].freed).sum())
        );
    }

    let answer = ask("Delete which? Numbers (1 3 5, 2-4), `old`, or Enter for none: ")?;
    let Some(picked) = disk_targets::parse_selection(&answer, offered.len(), &old)? else {
        println!("Nothing deleted.");
        return Ok(());
    };
    // Deleting a worktree takes the build folders inside it along; do not ask for them twice.
    let chosen: Vec<&Offer> = picked.iter().map(|i| &offered[*i]).collect();
    let mut all_paths: Vec<PathBuf> = chosen.iter().flat_map(|o| o.paths.iter().cloned()).collect();
    all_paths.sort();
    all_paths.dedup();
    let paths: Vec<PathBuf> = all_paths
        .iter()
        .filter(|p| !all_paths.iter().any(|outer| outer != *p && p.starts_with(outer)))
        .cloned()
        .collect();

    // Worktrees with unsaved work: say exactly what will be committed and pushed, and drop
    // the ones that cannot be saved, before asking.
    let mut to_save: Vec<(PathBuf, String)> = Vec::new();
    let mut unsaveable: Vec<PathBuf> = Vec::new();
    for offer in chosen.iter().filter(|o| o.save_first) {
        for p in &offer.paths {
            match disk_targets::plan_save(p, &home) {
                Ok(plan) => {
                    println!("{}", describe_save(&plan, &home, true));
                    to_save.push((p.clone(), plan.id));
                }
                Err(why) => {
                    println!("  will keep {}: {why}", disk_targets::display_path(p, &home));
                    unsaveable.push(p.clone());
                }
            }
        }
    }
    let paths: Vec<PathBuf> = paths
        .into_iter()
        .filter(|p| !unsaveable.contains(p))
        .collect();
    if paths.is_empty() {
        println!("Nothing to delete.");
        return Ok(());
    }
    // What is asked about is what is left: not a worktree just said to be kept.
    let chosen: Vec<&Offer> = chosen
        .into_iter()
        .filter(|o| !o.paths.iter().all(|p| unsaveable.contains(p)))
        .collect();

    println!();
    for offer in &chosen {
        let warn = if offer.look_first {
            format!("  <- {}", offer.detail)
        } else {
            String::new()
        };
        println!("  {:>9}  {}{warn}", disk_targets::human_size(offer.freed), offer.title);
    }
    let total: u64 = chosen.iter().map(|o| o.freed).sum();
    let saving = match to_save.iter().filter(|(p, _)| paths.contains(p)).count() {
        0 => String::new(),
        n => format!(" {n} of them are committed and pushed first, and kept if that fails."),
    };
    let confirm = ask(&format!(
        "Delete these {} folder(s), about {}?{saving} This cannot be undone. [y/N] ",
        paths.len(),
        disk_targets::human_size(total)
    ))?;
    if !said_yes(&confirm) {
        println!("Nothing deleted.");
        return Ok(());
    }

    // Save first. A worktree whose work did not reach the remote is not deleted.
    let mut not_saved = 0usize;
    let mut paths = paths;
    for (p, id) in to_save.iter().filter(|(p, _)| paths.contains(p)).cloned().collect::<Vec<_>>() {
        let shown = disk_targets::display_path(&p, &home);
        let options = disk_targets::SaveOptions { expected_id: Some(id), no_verify: false };
        match disk_targets::save_worktree(&p, &home, &options) {
            Ok(saved) => println!(
                "  saved   {shown}: {} file(s) committed to {}, pushed to {}",
                saved.committed_files, saved.branch, saved.pushed_to
            ),
            Err(why) => {
                not_saved += 1;
                println!("  kept    {shown}: {why}");
                paths.retain(|other| other != &p);
            }
        }
    }

    let (freed, failed) = delete_all(&paths, &home);
    let failed = failed + not_saved;
    println!("Freed about {}.", disk_targets::human_size(freed));
    print_free_space(&home);
    if failed > 0 {
        return Err(format!("{failed} item(s) were kept; see above."));
    }
    Ok(())
}

/// What saving a worktree will do, in a few indented lines.
fn describe_save(plan: &disk_targets::SavePlan, home: &std::path::Path, delete: bool) -> String {
    let mut lines = vec![format!("  {}", disk_targets::display_path(&plan.path, home))];
    let files = match plan.files.len() {
        0 => "nothing is uncommitted".to_string(),
        1 => "commit 1 file".to_string(),
        n => format!("commit {n} files"),
    };
    lines.push(if let Some(shared) = &plan.from_branch {
        format!("      {files}; {shared} is a shared branch, so the work goes on a new one, {}", plan.branch)
    } else if plan.new_branch {
        format!("      {files}; put its detached HEAD on a new branch, {}", plan.branch)
    } else if plan.files.is_empty() {
        format!("      {files} on branch {}", plan.branch)
    } else {
        format!("      {files} to branch {}", plan.branch)
    });
    lines.push(if plan.up_to_date {
        format!("      {}/{} already has it; nothing to push", plan.remote, plan.remote_branch)
    } else if plan.beside {
        format!(
            "      push to {} as a new branch, {}; {}/{} is left as it is",
            plan.remote, plan.remote_branch, plan.remote, plan.branch
        )
    } else {
        format!("      push to {}/{} (not on the remote yet)", plan.remote, plan.remote_branch)
    });
    for file in plan.files.iter().take(8) {
        lines.push(format!("        {file}"));
    }
    if plan.files.len() > 8 {
        lines.push(format!("        and {} more", plan.files.len() - 8));
    }
    if !plan.ignored.is_empty() {
        lines.push(format!(
            "      NOT saved, git ignores {}: {}{}",
            if plan.ignored.len() == 1 { "it" } else { "them" },
            plan.ignored.join(", "),
            if delete { " (deleted with the worktree)" } else { "" }
        ));
    }
    if delete {
        lines.push("      then remove the worktree".to_string());
    }
    lines.join("\n")
}

/// Save one worktree and, if asked, remove it. The JSON says how far it got.
fn save_one(
    path: &std::path::Path,
    home: &std::path::Path,
    delete: bool,
    options: &disk_targets::SaveOptions,
) -> serde_json::Value {
    let shown = path.to_string_lossy();
    let saved = match disk_targets::save_worktree(path, home, options) {
        Ok(saved) => saved,
        Err(why) => return serde_json::json!({"path": shown, "ok": false, "saved": false, "deleted": false, "error": why}),
    };
    let mut result = serde_json::json!({
        "path": shown,
        "ok": true,
        "saved": true,
        "deleted": false,
        "branch": saved.branch,
        "committed_files": saved.committed_files,
        "pushed_to": saved.pushed_to,
    });
    if delete {
        match disk_targets::delete_target(path, home) {
            Ok(done) => {
                result["deleted"] = true.into();
                result["freed_bytes"] = done.freed_bytes.into();
            }
            Err(why) => {
                result["ok"] = false.into();
                result["error"] = format!("its work is saved, but it was not removed: {why}").into();
            }
        }
    }
    result
}

fn disk_save(
    paths: &[PathBuf],
    delete: bool,
    plan_only: bool,
    no_verify: bool,
    plan_id: Option<String>,
    yes: bool,
    json: bool,
) -> Result<(), String> {
    let home = home()?;
    if plan_id.is_some() && paths.len() != 1 {
        return Err("--plan-id goes with exactly one worktree".to_string());
    }
    let paths: Vec<PathBuf> = paths
        .iter()
        .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()))
        .collect();

    if plan_only && json {
        let plans: Vec<serde_json::Value> = paths
            .iter()
            .map(|p| match disk_targets::plan_save(p, &home) {
                Ok(plan) => serde_json::json!({"path": p.to_string_lossy(), "ok": true, "plan": plan}),
                Err(why) => serde_json::json!({"path": p.to_string_lossy(), "ok": false, "error": why}),
            })
            .collect();
        println!("{}", serde_json::Value::Array(plans));
        return Ok(());
    }
    if json && !yes {
        return Err("--json needs --yes or --plan: confirmation is the caller's job".to_string());
    }
    if json {
        let options = disk_targets::SaveOptions { expected_id: plan_id, no_verify };
        let results: Vec<serde_json::Value> = paths.iter().map(|p| save_one(p, &home, delete, &options)).collect();
        println!("{}", serde_json::Value::Array(results));
        return Ok(());
    }

    // Say what will happen, and what cannot be saved, before asking. Each plan's id goes
    // with the save, so that what is committed is what was shown.
    let mut ready: Vec<(PathBuf, String)> = Vec::new();
    for p in &paths {
        match disk_targets::plan_save(p, &home) {
            Ok(plan) => {
                println!("{}", describe_save(&plan, &home, delete));
                ready.push((p.clone(), plan_id.clone().unwrap_or(plan.id)));
            }
            Err(why) => println!("  will not touch {}: {why}", disk_targets::display_path(p, &home)),
        }
    }
    if plan_only {
        return if ready.len() == paths.len() { Ok(()) } else { Err("Some cannot be saved; see above.".to_string()) };
    }
    if ready.is_empty() {
        return Err("Nothing to save.".to_string());
    }
    if !yes {
        let confirm = ask(&format!(
            "Commit and push {} worktree(s){}? {}; nothing is force-pushed. [y/N] ",
            ready.len(),
            if delete { ", then remove them" } else { "" },
            if no_verify { "Commit hooks are skipped" } else { "Commit and push hooks run as usual" }
        ))?;
        if !said_yes(&confirm) {
            println!("Nothing changed.");
            return Ok(());
        }
    }
    let mut failed = paths.len() - ready.len();
    for (p, id) in &ready {
        let shown = disk_targets::display_path(p, &home);
        let options = disk_targets::SaveOptions { expected_id: Some(id.clone()), no_verify };
        let result = save_one(p, &home, delete, &options);
        let text = |key: &str| result[key].as_str().unwrap_or_default().to_string();
        if result["saved"] == true {
            println!(
                "  saved   {shown}: {} file(s) committed to {}, pushed to {}",
                result["committed_files"], text("branch"), text("pushed_to")
            );
        }
        if result["deleted"] == true {
            println!(
                "  deleted {shown} ({})",
                disk_targets::human_size(result["freed_bytes"].as_u64().unwrap_or(0))
            );
        }
        if result["ok"] != true {
            failed += 1;
            println!("  kept    {shown}: {}", text("error"));
        }
    }
    if delete {
        print_free_space(&home);
    }
    if failed > 0 {
        return Err(format!("{failed} worktree(s) were not fully handled; see above."));
    }
    Ok(())
}

fn disk_delete(paths: &[PathBuf], yes: bool, json: bool) -> Result<(), String> {
    let home = home()?;
    let paths: Vec<PathBuf> = paths
        .iter()
        .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()))
        .collect();

    if json {
        // Machine use (the menu bar app): the caller has already confirmed.
        if !yes {
            return Err("--json needs --yes: confirmation is the caller's job".to_string());
        }
        let results: Vec<serde_json::Value> = paths
            .iter()
            .map(|p| match disk_targets::delete_target(p, &home) {
                Ok(done) => serde_json::json!({"path": p.to_string_lossy(), "ok": true, "freed_bytes": done.freed_bytes}),
                Err(why) => serde_json::json!({"path": p.to_string_lossy(), "ok": false, "error": why}),
            })
            .collect();
        println!("{}", serde_json::Value::Array(results));
        return Ok(());
    }

    // Say what cannot be deleted before asking about the rest.
    let mut deletable = Vec::new();
    for p in &paths {
        match disk_targets::why_not_deletable(p, &home) {
            Some(why) => println!("  will keep {}: {why}", disk_targets::display_path(p, &home)),
            None => deletable.push(p.clone()),
        }
    }
    if deletable.is_empty() {
        return Err("Nothing to delete.".to_string());
    }
    if !yes {
        for p in &deletable {
            println!("  will delete {}", disk_targets::display_path(p, &home));
        }
        let confirm = ask(&format!(
            "Delete {} item(s)? This cannot be undone. [y/N] ",
            deletable.len()
        ))?;
        if !said_yes(&confirm) {
            println!("Nothing deleted.");
            return Ok(());
        }
    }
    let (freed, failed) = delete_all(&deletable, &home);
    println!("Freed about {}.", disk_targets::human_size(freed));
    print_free_space(&home);
    if failed > 0 || deletable.len() < paths.len() {
        return Err("Some items were kept; see above.".to_string());
    }
    Ok(())
}

/// The file whose presence is the opt-in. The hooks test for it directly
/// (guardian_blocking_enabled in hooks/lib.sh), so it takes effect on the next hook call.
fn blocking_marker() -> PathBuf {
    guardian_dir().join("blocking_on")
}

/// When the gates would stop something, in words, from the settings in force.
fn blocking_rules() -> String {
    let cfg = load_config();
    let gate = &cfg.prompt_gate;
    let prompts = if !gate.enabled || gate.block_on == "never" {
        "prompts are never paused (the prompt gate is off in config.toml)".to_string()
    } else {
        match gate.mode.as_str() {
            "strict" => format!("prompts are paused at {} load", gate.block_on),
            "advisory" => "prompts are never paused (prompt_gate.mode is advisory)".to_string(),
            _ => "a prompt is paused when it asks for heavy local work (a full build, a full test run, container builds, several subagents) at critical load and macOS agrees memory or heat is critical".to_string(),
        }
    };
    let subagents = match cfg.tool_gate.deny_subagents_on.as_str() {
        "strained" | "critical" => format!("new subagents are denied at {} load", cfg.tool_gate.deny_subagents_on),
        _ => "subagents are never denied (tool_gate.deny_subagents_on is never)".to_string(),
    };
    format!("{prompts}; {subagents}")
}

fn blocking(action: BlockingAction) -> Result<(), String> {
    let marker = blocking_marker();
    // A symlink is not an opt-in (the hooks ignore one too) and is not written through.
    let on = || std::fs::symlink_metadata(&marker).is_ok_and(|m| m.is_file());
    match action {
        BlockingAction::On => {
            std::fs::create_dir_all(guardian_dir()).map_err(|e| e.to_string())?;
            if std::fs::symlink_metadata(&marker).is_ok_and(|m| !m.is_file()) {
                std::fs::remove_file(&marker).map_err(|e| e.to_string())?;
            }
            let stamp = format!("{}\n", Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true));
            std::fs::write(&marker, stamp).map_err(|e| e.to_string())?;
            println!("Blocking is on. With your settings: {}.", blocking_rules());
            println!("Back to warnings only: guardian blocking off");
        }
        BlockingAction::Off => {
            match std::fs::remove_file(&marker) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
            println!("Blocking is off. Guardian only warns: no prompt is paused and no subagent is denied.");
        }
        BlockingAction::Status => {
            if on() {
                println!("Blocking is on. With your settings: {}.", blocking_rules());
                println!("Back to warnings only: guardian blocking off");
            } else {
                println!("Blocking is off. Guardian only warns: no prompt is paused and no subagent is denied.");
                println!("If you turn it on (guardian blocking on): {}.", blocking_rules());
            }
        }
    }
    Ok(())
}

fn hooks_status(json: bool) -> Result<(), String> {
    use guardian_daemon::agent_hooks;
    let places = agent_hooks::Places::from_process().ok_or("could not find the home directory")?;
    let all = agent_hooks::status(&places, env!("CARGO_PKG_VERSION"));
    if json {
        println!("{}", serde_json::to_string(&all).map_err(|e| e.to_string())?);
        return Ok(());
    }
    for st in &all {
        println!("{:<12} {}", st.label, st.summary);
    }
    if all.iter().any(|st| st.client == "codex" && st.installed) {
        println!("\nCodex runs new or changed hooks only once you have approved them: /hooks in the CLI, Settings > Hooks in the app.");
    }
    if all.iter().any(|st| st.present && !st.current) {
        return Err("Some agents on this machine do not have this Guardian's hooks; see above.".to_string());
    }
    Ok(())
}

fn hooks_install(cursor: bool, claude: bool, codex: bool, upgrade: bool) -> Result<(), String> {
    use guardian_daemon::agent_hooks::{self, Client};
    let places = agent_hooks::Places::from_process().ok_or("could not find the home directory")?;
    let version = env!("CARGO_PKG_VERSION");
    let named: Vec<Client> = [(cursor, Client::Cursor), (claude, Client::Claude), (codex, Client::Codex)]
        .into_iter()
        .filter(|(wanted, _)| *wanted)
        .map(|(_, client)| client)
        .collect();
    let chosen: Vec<Client> = if !named.is_empty() {
        named
    } else {
        Client::ALL
            .into_iter()
            .filter(|c| {
                let st = agent_hooks::client_status(*c, &places, version);
                if upgrade { st.installed && !st.current } else { st.present }
            })
            .collect()
    };
    if chosen.is_empty() {
        println!(
            "{}",
            if upgrade {
                "Every agent with Guardian hooks already has this version's."
            } else {
                "No agent is set up on this machine. Name one to install for it anyway: --cursor, --claude or --codex."
            }
        );
        return Ok(());
    }
    let mut failed = 0usize;
    for client in &chosen {
        match agent_hooks::install(*client, &places, version) {
            Ok(_) => {
                let st = agent_hooks::client_status(*client, &places, version);
                println!("{:<12} {}", st.label, st.summary);
            }
            Err(why) => {
                failed += 1;
                println!("{:<12} not installed:\n{why}", client.label());
            }
        }
    }
    println!("\nSessions that are already running keep the hooks they started with; new ones pick these up.");
    if chosen.contains(&Client::Codex) {
        println!("Codex skips new or changed hooks until you approve them: /hooks in the CLI, Settings > Hooks in the app.");
    }
    if failed > 0 {
        return Err(format!("{failed} agent(s) could not be set up; see above."));
    }
    Ok(())
}

fn check_config() -> Result<(), String> {
    let path = config_path();
    let report = load_config_report();
    match report.source {
        ConfigSource::Missing => {
            println!("{}: not present — built-in defaults are in use.", path.display());
            return Ok(());
        }
        ConfigSource::File if report.problems.is_empty() => {
            println!("{}: OK", path.display());
            println!("guardiand applies edits within a few seconds; no restart needed.");
            return Ok(());
        }
        _ => {}
    }

    println!("{}:", path.display());
    for problem in &report.problems {
        println!("  - {problem}");
    }
    Err(match report.source {
        ConfigSource::File => {
            "The rest of the file is in force; the settings above are not doing what they say."
                .to_string()
        }
        _ => "Fix the file; guardiand keeps its previous settings until it parses.".to_string(),
    })
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Commands::Snooze { minutes } => write_snooze_minutes(minutes).map_err(|e| e.to_string()),
        Commands::Once => touch_proceed_once().map_err(|e| e.to_string()),
        Commands::ClearSnooze => clear_snooze().map_err(|e| e.to_string()),
        Commands::Config {
            action: ConfigAction::Check,
        } => check_config(),
        Commands::Disk { scan, json, cached, action } => match action {
            None => {
                let report = if cached {
                    disk_targets::cached(&scan.options()).ok_or_else(|| {
                        "no scan has been saved yet; run `guardian disk` once".to_string()
                    })?
                } else {
                    disk_targets::survey(&scan.options())
                        .ok_or_else(|| "could not find the home directory".to_string())?
                };
                if json {
                    println!(
                        "{}",
                        serde_json::to_string(&report).map_err(|e| e.to_string())?
                    );
                } else {
                    print!("{}", report.render());
                }
                Ok(())
            }
            Some(DiskAction::Refresh) => disk_refresh(&scan),
            Some(DiskAction::Clean) => disk_clean(&scan),
            Some(DiskAction::Delete { paths, yes, json }) => disk_delete(&paths, yes, json),
            Some(DiskAction::Save { paths, delete, plan, no_verify, plan_id, yes, json }) => {
                disk_save(&paths, delete, plan, no_verify, plan_id, yes, json)
            }
        },
        Commands::Blocking { action } => blocking(action),
        Commands::Hooks { action } => match action {
            HooksAction::Status { json } => hooks_status(json),
            HooksAction::Install { cursor, claude, codex, upgrade } => hooks_install(cursor, claude, codex, upgrade),
        },
        Commands::Stats { days } => {
            print!("{}", PressureStats::load().render(days as usize));
            Ok(())
        }
        Commands::Zeno { action } => match action {
            ZenoAction::Bump => {
                let mut z = zeno::load_zeno().unwrap_or_default();
                z.steps = z.steps.saturating_add(1);
                zeno::save_zeno(&z).map_err(|e| e.to_string())?;
                println!(
                    "Zeno steps = {} (effective limits move halfway toward full usage each bump).",
                    z.steps
                );
                Ok(())
            }
            ZenoAction::Rollback => {
                let mut z = zeno::load_zeno().unwrap_or_default();
                let before = z.steps;
                z.steps = z.steps.saturating_sub(1);
                zeno::save_zeno(&z).map_err(|e| e.to_string())?;
                if before == 0 {
                    println!("Zeno rollback — already at steps=0 (config.toml limits).");
                } else {
                    println!(
                        "Zeno steps {} -> {} (reverted to previous effective limits).",
                        before, z.steps
                    );
                }
                Ok(())
            }
            ZenoAction::Reset => {
                zeno::save_zeno(&ZenoState { steps: 0 }).map_err(|e| e.to_string())?;
                println!("Zeno reset — thresholds match config.toml (after guardiand refreshes hook policy).");
                Ok(())
            }
            ZenoAction::Status => {
                let cfg = load_config();
                let z = zeno::load_zeno().unwrap_or_default();
                let merged = apply_zeno(&cfg, z.steps);
                println!("zeno steps: {}", z.steps);
                println!(
                    "thresholds: strained_cpu {}% -> {}%, critical_cpu {}% -> {}%",
                    cfg.thresholds.strained_cpu_percent,
                    merged.thresholds.strained_cpu_percent,
                    cfg.thresholds.critical_cpu_percent,
                    merged.thresholds.critical_cpu_percent
                );
                println!(
                    "swap: strained {}% -> {}%, critical {}% -> {}%",
                    cfg.thresholds.strained_swap_percent,
                    merged.thresholds.strained_swap_percent,
                    cfg.thresholds.critical_swap_percent,
                    merged.thresholds.critical_swap_percent
                );
                println!(
                    "memory free (GiB): strained {} -> {}, critical {} -> {}",
                    cfg.thresholds.strained_memory_gb,
                    merged.thresholds.strained_memory_gb,
                    cfg.thresholds.critical_memory_gb,
                    merged.thresholds.critical_memory_gb
                );
                println!(
                    "disk used %: warn {} -> {}, critical {} -> {}",
                    cfg.disk.warn_used_percent,
                    merged.disk.warn_used_percent,
                    cfg.disk.critical_used_percent,
                    merged.disk.critical_used_percent
                );
                println!(
                    "Cursor RSS cap (MB): max {} -> {}, warn {} -> {}",
                    cfg.session_budget.max_cursor_rss_megabytes,
                    merged.session_budget.max_cursor_rss_megabytes,
                    cfg.session_budget.warn_cursor_rss_megabytes,
                    merged.session_budget.warn_cursor_rss_megabytes
                );
                Ok(())
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parses(args: &[&str]) -> bool {
        Cli::try_parse_from(args).is_ok()
    }

    #[test]
    fn snooze_is_capped() {
        assert!(parses(&["guardian", "snooze"]));
        assert!(parses(&["guardian", "snooze", "120"]));
        assert!(!parses(&["guardian", "snooze", "121"]));
        assert!(!parses(&["guardian", "snooze", "0"]));
        assert!(!parses(&["guardian", "snooze", "999999"]));
    }

    #[test]
    fn scan_options_work_on_either_side_of_clean() {
        let older_than = |args: &[&str]| match Cli::try_parse_from(args).ok().map(|c| c.command) {
            Some(Commands::Disk { scan, action: Some(DiskAction::Clean), .. }) => Some(scan.older_than),
            _ => None,
        };
        assert_eq!(older_than(&["guardian", "disk", "--older-than", "14", "clean"]), Some(14));
        assert_eq!(older_than(&["guardian", "disk", "clean", "--older-than", "14"]), Some(14));
        assert_eq!(older_than(&["guardian", "disk", "clean"]), Some(0));
    }

    #[test]
    fn config_check_and_stats_parse() {
        assert!(parses(&["guardian", "config", "check"]));
        assert!(parses(&["guardian", "stats", "--days", "30"]));
        assert!(!parses(&["guardian", "stats", "--days", "0"]));
        assert!(parses(&["guardian", "disk"]));
        assert!(parses(&["guardian", "disk", "--top", "5", "--root", "/tmp/a", "--root", "/tmp/b", "--json"]));
        assert!(!parses(&["guardian", "disk", "--seconds", "0"]));
        assert!(parses(&["guardian", "disk", "--older-than", "30"]));
        assert!(parses(&["guardian", "disk", "clean", "--top", "20", "--older-than", "14"]));
        assert!(parses(&["guardian", "disk", "delete", "/tmp/a", "/tmp/b", "--yes", "--json"]));
        assert!(parses(&["guardian", "disk", "--cached", "--json", "--top", "12"]));
        assert!(parses(&["guardian", "disk", "--fresh"]));
        assert!(!parses(&["guardian", "disk", "--cached", "--fresh"]));
        assert!(parses(&["guardian", "disk", "refresh", "--seconds", "120"]));
        assert!(parses(&["guardian", "blocking", "on"]));
        assert!(parses(&["guardian", "blocking", "off"]));
        assert!(parses(&["guardian", "blocking", "status"]));
        assert!(!parses(&["guardian", "blocking"]));
        assert!(parses(&["guardian", "hooks", "status"]));
        assert!(parses(&["guardian", "hooks", "status", "--json"]));
        assert!(parses(&["guardian", "hooks", "install"]));
        assert!(parses(&["guardian", "hooks", "install", "--claude", "--codex"]));
        assert!(parses(&["guardian", "hooks", "install", "--upgrade"]));
        assert!(!parses(&["guardian", "hooks", "install", "--upgrade", "--cursor"]));
        assert!(!parses(&["guardian", "hooks"]));
        assert!(parses(&["guardian", "disk", "save", "/tmp/wt"]));
        assert!(parses(&["guardian", "disk", "save", "/tmp/a", "/tmp/b", "--delete", "--yes", "--json"]));
        assert!(parses(&["guardian", "disk", "save", "--plan", "--json", "/tmp/wt"]));
        assert!(!parses(&["guardian", "disk", "save", "--plan", "--delete", "/tmp/wt"]));
        assert!(parses(&["guardian", "disk", "save", "--delete", "--yes", "--json", "--plan-id", "0123abcd", "/tmp/wt"]));
        assert!(parses(&["guardian", "disk", "save", "--no-verify", "/tmp/wt"]));
        assert!(!parses(&["guardian", "disk", "save"]));
        assert!(!parses(&["guardian", "disk", "delete"]));
    }

    #[test]
    fn clean_offers_stale_bundles_first_and_old_picks_them() {
        use std::time::{Duration, SystemTime};
        let home = std::env::temp_dir().join(format!("guardian-cli-offers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let write = |rel: &str, bytes: usize| {
            let path = home.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, vec![b'x'; bytes]).unwrap();
        };
        let age = |rel: &str, days: u64| {
            let when = SystemTime::now() - Duration::from_secs(days * 86_400 + 3_600);
            for entry in std::fs::read_dir(home.join(rel)).unwrap().flatten() {
                std::fs::File::open(entry.path()).unwrap().set_modified(when).unwrap();
            }
            std::fs::File::open(home.join(rel)).unwrap().set_modified(when).unwrap();
        };
        write("code/old/node_modules/a.js", 300_000);
        write("code/old/.next/b.js", 200_000);
        write("code/old/package.json", 2);
        age("code/old/node_modules", 40);
        age("code/old/.next", 40);
        write("code/live/node_modules/c.js", 900_000);
        write("Library/Caches/ms-playwright/chromium/bin", 400_000);
        age("Library/Caches/ms-playwright", 50);
        let home = std::fs::canonicalize(&home).unwrap();

        let opts = disk_targets::Options {
            min_bytes: 1,
            top: 50,
            ..disk_targets::Options::default()
        };
        let report = disk_targets::survey_home(&home, &opts);
        let offered = offers(&report, 10);
        let titles: Vec<&str> = offered.iter().map(|o| o.title.as_str()).collect();
        assert_eq!(
            titles,
            vec![
                "~/code/old: 2 stale folders",
                "Tool caches: 1 stale folder",
                "Node dependencies (~/code/live/node_modules)",
            ]
        );
        assert_eq!(offered[0].paths.len(), 2);
        assert!(offered[0].freed >= 500_000);
        // `old` takes the project's stale bundle, not a cache every project shares, and
        // nothing in use.
        let old: Vec<bool> = offered.iter().map(|o| o.old).collect();
        assert_eq!(old, vec![true, false, false]);
        assert_eq!(offers(&report, 1).len(), 1);
        let _ = std::fs::remove_dir_all(&home);
    }
}
