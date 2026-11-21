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

fn disk_clean(scan: &ScanArgs) -> Result<(), String> {
    let home = home()?;
    println!("Scanning your git repositories and tool caches (up to {} s; sizes that have not changed are reused)...", scan.seconds);
    // Look at more than will be listed: only things Guardian can delete are offered.
    let mut opts = scan.options();
    opts.top = 100;
    let report = disk_targets::survey(&opts).ok_or("could not find the home directory")?;
    let offered: Vec<disk_targets::Target> = report
        .targets
        .into_iter()
        .filter(|t| t.deletable)
        .take(scan.top as usize)
        .collect();
    if offered.is_empty() {
        println!("Nothing Guardian can delete was found. `guardian disk` lists everything, including items that need a tool or a decision.");
        return Ok(());
    }

    println!(
        "\nDisk: {:.1}% used, {:.1} GB free. {} git repositories scanned.\n",
        report.used_percent, report.available_gb, report.repos_scanned
    );
    for (i, t) in offered.iter().enumerate() {
        let mut details: Vec<String> = Vec::new();
        if let Some(age) = t.age_text() {
            details.push(age);
        }
        details.push(t.note.clone().unwrap_or_else(|| t.cost.clone()));
        if let Some(shared) = t.shared_text() {
            details.push(shared);
        }
        let flag = if t.safety == disk_targets::Safety::Review { "  LOOK FIRST" } else { "" };
        println!(
            "{:>3}. {:>9}  {} ({}){flag}\n                {}",
            i + 1,
            t.size_text(),
            t.what,
            t.display_path,
            details.join("; ")
        );
    }
    let old: Vec<&disk_targets::Target> = offered.iter().filter(|t| t.is_old_and_safe()).collect();
    let old_bytes: u64 = old.iter().map(|t| t.freed_bytes).sum();
    println!();
    if !old.is_empty() {
        println!(
            "`old` selects the {} item(s) that come back on their own and have gone untouched for {}+ days ({}).",
            old.len(),
            disk_targets::OLD_DAYS,
            disk_targets::human_size(old_bytes)
        );
    }

    let answer = ask("Delete which? Numbers (1 3 5, 2-4), `old`, or Enter for none: ")?;
    let Some(picked) = disk_targets::parse_selection(&answer, &offered)? else {
        println!("Nothing deleted.");
        return Ok(());
    };
    // Deleting a worktree takes the build folders inside it along; do not ask for them twice.
    let chosen: Vec<&disk_targets::Target> = picked.iter().map(|i| &offered[*i]).collect();
    let paths: Vec<PathBuf> = chosen
        .iter()
        .filter(|t| {
            !chosen
                .iter()
                .any(|outer| outer.path != t.path && t.path.starts_with(&outer.path))
        })
        .map(|t| t.path.clone())
        .collect();
    let total: u64 = chosen
        .iter()
        .filter(|t| paths.contains(&t.path))
        .map(|t| t.freed_bytes)
        .sum();

    println!();
    for t in chosen.iter().filter(|t| paths.contains(&t.path)) {
        let warn = if t.safety == disk_targets::Safety::Review {
            format!("  <- {}", t.note.clone().unwrap_or_else(|| t.cost.clone()))
        } else {
            String::new()
        };
        println!("  {:>9}  {}{warn}", t.size_text(), t.display_path);
    }
    let confirm = ask(&format!(
        "Delete these {} item(s), about {}? This cannot be undone. [y/N] ",
        paths.len(),
        disk_targets::human_size(total)
    ))?;
    if !said_yes(&confirm) {
        println!("Nothing deleted.");
        return Ok(());
    }

    let (freed, failed) = delete_all(&paths, &home);
    println!("Freed about {}.", disk_targets::human_size(freed));
    print_free_space(&home);
    if failed > 0 {
        return Err(format!("{failed} item(s) were kept; see above."));
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
        assert!(!parses(&["guardian", "disk", "delete"]));
    }
}
