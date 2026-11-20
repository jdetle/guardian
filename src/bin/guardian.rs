//! User-facing `guardian` CLI — snooze / one-shot resume / zeno relaxation / pressure history.

use chrono::{Duration, Local, SecondsFormat, Utc};
use clap::{Parser, Subcommand};
use guardian_daemon::config::{
    config_path, guardian_dir, load_config, load_config_report, ConfigSource,
};
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
    /// Show how much time was spent at each pressure level, per day.
    Stats {
        /// How many of the most recent days to show.
        #[arg(long, default_value_t = 7, value_parser = clap::value_parser!(u32).range(1..=30))]
        days: u32,
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
    fn config_check_and_stats_parse() {
        assert!(parses(&["guardian", "config", "check"]));
        assert!(parses(&["guardian", "stats", "--days", "30"]));
        assert!(!parses(&["guardian", "stats", "--days", "0"]));
    }
}
