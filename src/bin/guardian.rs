//! User-facing `guardian` CLI — snooze / one-shot resume / zeno relaxation.

use clap::{Parser, Subcommand};
use guardian_daemon::config::{guardian_dir, load_config};
use guardian_daemon::zeno::{self, apply_zeno, ZenoState};

#[derive(Parser)]
#[command(name = "guardian")]
#[command(
    about = "System Guardian — snooze prompt gates, allow one submit, or relax thresholds (zeno)"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Snooze beforeSubmitPrompt gates until N minutes from now (default 15).
    Snooze {
        #[arg(default_value_t = 15)]
        minutes: u32,
    },
    /// Allow the next gated submit once (same as touching ~/.guardian/proceed_once).
    Once,
    /// Remove snooze immediately.
    ClearSnooze,
    /// Relax effective limits toward full usage (halfway toward 100% each bump).
    Zeno {
        #[command(subcommand)]
        action: ZenoAction,
    },
}

#[derive(Subcommand)]
enum ZenoAction {
    /// Increment the zeno step counter (daemon picks this up on the next sample).
    Bump,
    /// Reset zeno steps to 0 (back to config.toml thresholds only).
    Reset,
    /// Show current steps and effective thresholds vs config.
    Status,
}

fn write_snooze_minutes(minutes: u32) -> Result<(), std::io::Error> {
    let ts = compute_snooze_iso(minutes)?;
    let dir = guardian_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("snooze_until"), format!("{ts}\n"))?;
    println!("Guardian snoozed until {ts} (UTC)");
    Ok(())
}

fn compute_snooze_iso(minutes: u32) -> Result<String, std::io::Error> {
    // Prefer BSD date (macOS), then GNU date.
    let bsd = std::process::Command::new("date")
        .args(["-u", "-v", &format!("+{minutes}M"), "+%Y-%m-%dT%H:%M:%SZ"])
        .output();
    if let Ok(out) = &bsd {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            let line = s.lines().next().unwrap_or("").trim();
            if !line.is_empty() {
                return Ok(line.to_string());
            }
        }
    }
    let gnu = std::process::Command::new("date")
        .args([
            "-u",
            "-d",
            &format!("+{minutes} minutes"),
            "+%Y-%m-%dT%H:%M:%SZ",
        ])
        .output()?;
    if gnu.status.success() {
        let s = String::from_utf8_lossy(&gnu.stdout);
        let line = s.lines().next().unwrap_or("").trim();
        if !line.is_empty() {
            return Ok(line.to_string());
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        "could not compute snooze time (need BSD or GNU date)",
    ))
}

fn touch_proceed_once() -> Result<(), std::io::Error> {
    let p = guardian_dir().join("proceed_once");
    std::fs::create_dir_all(guardian_dir())?;
    std::fs::File::create(&p)?;
    println!("Created {} — your next gated submit will consume this.", p.display());
    Ok(())
}

fn clear_snooze() -> Result<(), std::io::Error> {
    let p = guardian_dir().join("snooze_until");
    let _ = std::fs::remove_file(p);
    println!("Cleared snooze.");
    Ok(())
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
