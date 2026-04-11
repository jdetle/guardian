#![allow(dead_code)]

mod classifier;
mod config;
mod docker;
mod hook_policy;
mod sampler;
mod session_db;
mod state;
mod validation;

use classifier::{classify, ClassifierInput, PressureLevel};
use config::{load_config, GuardianConfig};
use hook_policy::write_hook_policy;
use docker::stats::DockerClient;
use sampler::cpu::{cpu_usage_percent, CpuSnapshot};
use sampler::memory::MemoryInfo;
use sampler::process::{cursor_rss_megabytes, ProcessInfo};
use sampler::swap::SwapInfo;
use sampler::thermal::ThermalState;
use state::{CursorState, DockerState, GuardianState, write_state};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_signal(_sig: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

const PROTECTED_NAMES: &[&str] = &[
    // Guardian itself
    "guardiand",
    // macOS system
    "Finder",
    "WindowServer",
    "loginwindow",
    "Dock",
    "SystemUIServer",
    "kernel_task",
    "launchd",
    "sshd",
    "notifyd",
    "mds",
    "mds_stores",
    "coreaudiod",
    "bluetoothd",
    "airportd",
    "cfprefsd",
    "distnoted",
    "trustd",
    "secd",
    "UserEventAgent",
    "corebrightnessd",
    "CrashReporter",
    // Cursor / VS Code
    "Cursor",
    "Cursor Helper",
    "Cursor Helper (GPU)",
    "Cursor Helper (Renderer)",
    "Cursor Helper (Plugin)",
    "Code Helper",
    "Code Helper (GPU)",
    "Code Helper (Renderer)",
    "Code Helper (Plugin)",
    "Electron",
    // Docker
    "Docker",
    "Docker Desktop",
    "docker",
    "com.docker",
    "com.docker.vmnetd",
    "containerd",
    "dockerd",
    // Development tools
    "node",
    "bun",
    "deno",
    "cargo",
    "rustc",
    "rustup",
    "rust-analyzer",
    "git",
    "gh",
    "swift",
    "swiftc",
    "clang",
    "clangd",
    "sourcekit-lsp",
    "tsc",
    "biome",
    // Terminals
    "Terminal",
    "iTerm2",
    "Alacritty",
    "kitty",
    "WezTerm",
    "tmux",
    "zsh",
    "bash",
    "fish",
    // Browsers
    "Safari",
    "Google Chrome",
    "Google Chrome Helper",
    "Firefox",
    "Arc",
    // Common apps
    "Slack",
    "Messages",
    "Mail",
    "Notes",
    "Preview",
    "Activity Monitor",
    "Spotify",
];

fn main() {
    eprintln!("[guardiand] starting (pid {})", std::process::id());

    let config = load_config();
    eprintln!(
        "[guardiand] config loaded: interval={}s",
        config.sample_interval_secs
    );

    if let Err(e) = write_hook_policy(&config) {
        eprintln!("[guardiand] warning: could not write hook_policy.json: {e}");
    }

    if let Err(e) = session_db::ensure_db() {
        eprintln!("[guardiand] session db init warning: {e}");
    }

    validation::stamp_platform();

    unsafe {
        libc::signal(libc::SIGTERM, handle_signal as libc::sighandler_t);
        libc::signal(libc::SIGINT, handle_signal as libc::sighandler_t);
    }

    let docker_client = if config.docker.enabled {
        let client = DockerClient::new(&config.docker.socket_path);
        if client.available() {
            eprintln!(
                "[guardiand] docker socket found at {}",
                config.docker.socket_path
            );
            Some(client)
        } else {
            eprintln!("[guardiand] docker socket not found, docker monitoring disabled");
            None
        }
    } else {
        None
    };

    run_loop(&config, docker_client.as_ref());

    write_shutdown_state();
    eprintln!("[guardiand] shutdown complete");
}

fn run_loop(config: &GuardianConfig, docker_client: Option<&DockerClient>) {
    let interval = Duration::from_secs(config.sample_interval_secs);
    let mut prev_cpu = CpuSnapshot::sample().unwrap_or_default();
    let mut prev_pressure = PressureLevel::Clear;
    let mut docker_throttled = false;
    let mut hysteresis_counter: i32 = 0;
    let hysteresis_threshold: i32 = 3;

    loop {
        thread::sleep(interval);

        if SHUTDOWN.load(Ordering::SeqCst) {
            eprintln!("[guardiand] received shutdown signal, exiting gracefully");
            break;
        }

        let curr_cpu = CpuSnapshot::sample().unwrap_or_default();
        let cpu_pct = cpu_usage_percent(&prev_cpu, &curr_cpu);
        prev_cpu = curr_cpu;

        let mem = MemoryInfo::sample();
        let swap = SwapInfo::sample();
        let thermal = ThermalState::sample();
        let procs = ProcessInfo::sample();

        let docker_state = docker_client
            .map(|c| c.aggregate_stats())
            .unwrap_or_default();

        let mem_available_gb = mem.as_ref().map(|m| m.available_gb()).unwrap_or(4.0);
        let mem_total_gb = mem.as_ref().map(|m| m.total_gb()).unwrap_or(16.0);
        let swap_pct = swap.as_ref().map(|s| s.used_percent()).unwrap_or(0.0);
        let proc_ratio = procs.as_ref().map(|p| p.usage_ratio()).unwrap_or(0.0);

        let input = ClassifierInput {
            cpu_percent: cpu_pct,
            memory_available_gb: mem_available_gb,
            memory_total_gb: mem_total_gb,
            swap_used_percent: swap_pct,
            thermal,
            process_usage_ratio: proc_ratio,
        };

        let raw_pressure = classify(&input, &config.thresholds);

        // Hysteresis: require consecutive samples before escalating/de-escalating
        let pressure = apply_hysteresis(
            prev_pressure,
            raw_pressure,
            &mut hysteresis_counter,
            hysteresis_threshold,
        );

        if pressure != prev_pressure {
            eprintln!(
                "[guardiand] pressure transition: {} -> {} (cpu={:.1}%, mem_avail={:.1}GB, swap={:.1}%)",
                prev_pressure, pressure, cpu_pct, mem_available_gb, swap_pct
            );
        }

        // Docker throttle: continuous enforcement, not just on transition
        handle_docker_throttle(config, docker_client, pressure, &mut docker_throttled);

        // Process killer: identify and kill top resource abusers
        handle_process_killer(config, &procs, pressure);

        let cursor_rss_mb = cursor_rss_megabytes();
        let cursor_state = CursorState {
            active_sessions: count_cursor_sessions(),
            process_count: procs
                .as_ref()
                .map(|p| p.cursor_process_count)
                .unwrap_or(0),
            resident_memory_megabytes: cursor_rss_mb,
        };

        let now = chrono::Utc::now().to_rfc3339();
        let guardian_state = GuardianState {
            pressure,
            cpu_percent: (cpu_pct * 10.0).round() / 10.0,
            memory_available_gb: (mem_available_gb * 100.0).round() / 100.0,
            memory_total_gb: (mem_total_gb * 100.0).round() / 100.0,
            swap_used_percent: (swap_pct * 10.0).round() / 10.0,
            thermal_state: thermal,
            docker: docker_state,
            cursor: cursor_state,
            process_count: procs.as_ref().map(|p| p.total_count).unwrap_or(0),
            max_proc_per_uid: procs.as_ref().map(|p| p.max_proc_per_uid).unwrap_or(0),
            sampled_at: now,
        };

        if let Err(e) = write_state(&guardian_state) {
            eprintln!("[guardiand] write state error: {e}");
        }

        prev_pressure = pressure;
    }
}

fn write_shutdown_state() {
    let shutdown_state = GuardianState {
        pressure: PressureLevel::Clear,
        cpu_percent: 0.0,
        memory_available_gb: 0.0,
        memory_total_gb: 0.0,
        swap_used_percent: 0.0,
        thermal_state: ThermalState::Unknown,
        docker: DockerState::default(),
        cursor: CursorState::default(),
        process_count: 0,
        max_proc_per_uid: 0,
        sampled_at: chrono::Utc::now().to_rfc3339(),
    };
    if let Err(e) = write_state(&shutdown_state) {
        eprintln!("[guardiand] failed to write shutdown state: {e}");
    }

    let tmp_path = config::guardian_dir().join("state.json.tmp");
    if tmp_path.exists() {
        let _ = std::fs::remove_file(&tmp_path);
    }
}

fn apply_hysteresis(
    current: PressureLevel,
    proposed: PressureLevel,
    counter: &mut i32,
    threshold: i32,
) -> PressureLevel {
    if proposed == current {
        *counter = 0;
        return current;
    }

    let direction = match (&current, &proposed) {
        (PressureLevel::Clear, PressureLevel::Strained)
        | (PressureLevel::Clear, PressureLevel::Critical)
        | (PressureLevel::Strained, PressureLevel::Critical) => 1,
        _ => -1,
    };

    if direction > 0 {
        // Escalation: only need 2 consecutive samples (respond fast to danger)
        *counter += 1;
        if *counter >= 2 {
            *counter = 0;
            return proposed;
        }
    } else {
        // De-escalation: need full threshold (don't release too early)
        *counter += 1;
        if *counter >= threshold {
            *counter = 0;
            return proposed;
        }
    }
    current
}

fn handle_docker_throttle(
    config: &GuardianConfig,
    docker_client: Option<&DockerClient>,
    pressure: PressureLevel,
    throttled: &mut bool,
) {
    let Some(client) = docker_client else {
        return;
    };
    if !config.docker.auto_throttle {
        return;
    }

    match pressure {
        PressureLevel::Critical => {
            // Continuous enforcement: re-scan every cycle to catch new containers
            let names = client.container_names();
            if !*throttled {
                eprintln!("[guardiand] throttling non-essential docker containers (critical)");
            }
            docker::throttle::throttle_non_essential(
                &names,
                &config.docker.essential_containers,
                0.5,
            );
            *throttled = true;
        }
        PressureLevel::Strained => {
            // Soft throttle at strained: cap to 0.75 CPUs instead of 0.5
            let names = client.container_names();
            if !*throttled {
                eprintln!("[guardiand] soft-throttling docker containers (strained)");
            }
            docker::throttle::throttle_non_essential(
                &names,
                &config.docker.essential_containers,
                0.75,
            );
            *throttled = true;
        }
        PressureLevel::Clear if *throttled => {
            eprintln!("[guardiand] removing docker throttles (clear)");
            let names = client.container_names();
            docker::throttle::unthrottle_all(&names);
            *throttled = false;
        }
        _ => {}
    }
}

fn handle_process_killer(
    config: &GuardianConfig,
    procs: &Option<ProcessInfo>,
    _pressure: PressureLevel,
) {
    if !config.fork_guard.enabled {
        return;
    }
    let Some(procs) = procs else { return };
    let ratio = procs.usage_ratio();

    if ratio > config.fork_guard.kill_ratio && config.fork_guard.kill_enabled {
        eprintln!(
            "[guardiand] FORK GUARD: process count {}/{} ({:.0}%) — killing runaway processes",
            procs.total_count, procs.max_proc_per_uid, ratio * 100.0
        );
        kill_runaway_processes(5);
    } else if ratio > config.fork_guard.warn_ratio {
        eprintln!(
            "[guardiand] FORK GUARD WARNING: process count {}/{} ({:.0}%)",
            procs.total_count, procs.max_proc_per_uid, ratio * 100.0
        );
    }
}

const MIN_CPU_FOR_KILL: f64 = 80.0;
const MAX_AGE_SECONDS_FOR_KILL: u64 = 60;

/// Kill up to N runaway processes that are:
///   1. NOT in the protected list
///   2. Using > 80% of a CPU core
///   3. Younger than 60 seconds (likely fork-bomb spawn, not user work)
///
/// Uses macOS-compatible `ps` flags (no GNU --sort).
fn kill_runaway_processes(count: usize) {
    let uid = unsafe { libc::getuid() }.to_string();
    let output = std::process::Command::new("ps")
        .args(["-u", &uid, "-o", "pid=,pcpu=,etime=,comm="])
        .output();

    let Ok(output) = output else {
        eprintln!("[guardiand] kill_runaway: ps command failed");
        return;
    };
    let text = String::from_utf8_lossy(&output.stdout);

    let mut procs: Vec<(i32, f64, u64, String)> = text
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 4 {
                return None;
            }
            let pid = parts[0].parse::<i32>().ok()?;
            let cpu = parts[1].parse::<f64>().ok()?;
            let age_secs = parse_etime(parts[2]);
            let comm = parts[3..].join(" ");
            let basename = comm.rsplit('/').next().unwrap_or(&comm);
            Some((pid, cpu, age_secs, basename.to_string()))
        })
        .collect();

    procs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let my_pid = std::process::id() as i32;
    let mut killed = 0;

    for (pid, cpu, age_secs, name) in &procs {
        if killed >= count {
            break;
        }
        if *cpu < MIN_CPU_FOR_KILL {
            break;
        }
        if *pid == my_pid || *pid == 1 {
            continue;
        }
        if is_protected(name) {
            continue;
        }
        if *age_secs > MAX_AGE_SECONDS_FOR_KILL {
            continue;
        }
        eprintln!(
            "[guardiand] killing runaway: pid={pid} name={name} cpu={cpu:.1}% age={age_secs}s"
        );
        unsafe { libc::kill(*pid, libc::SIGTERM) };
        killed += 1;
    }

    if killed == 0 {
        eprintln!("[guardiand] no runaway processes eligible for kill");
    }
}

fn is_protected(name: &str) -> bool {
    PROTECTED_NAMES
        .iter()
        .any(|p| name == *p || name.starts_with(p))
}

/// Parse ps etime format: [[dd-]hh:]mm:ss
fn parse_etime(etime: &str) -> u64 {
    let parts: Vec<&str> = etime.split(':').collect();
    match parts.len() {
        2 => {
            let mm = parts[0].parse::<u64>().unwrap_or(0);
            let ss = parts[1].parse::<u64>().unwrap_or(0);
            mm * 60 + ss
        }
        3 => {
            let first = parts[0];
            let (days, hh) = if first.contains('-') {
                let dp: Vec<&str> = first.split('-').collect();
                let d = dp[0].parse::<u64>().unwrap_or(0);
                let h = dp.get(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                (d, h)
            } else {
                (0, first.parse::<u64>().unwrap_or(0))
            };
            let mm = parts[1].parse::<u64>().unwrap_or(0);
            let ss = parts[2].parse::<u64>().unwrap_or(0);
            days * 86400 + hh * 3600 + mm * 60 + ss
        }
        _ => u64::MAX,
    }
}

fn count_cursor_sessions() -> u32 {
    let sessions_dir = dirs::home_dir().map(|h| h.join(".cursor/projects"));
    let Some(dir) = sessions_dir else { return 0 };
    if !dir.exists() {
        return 0;
    }

    std::fs::read_dir(dir)
        .map(|entries| entries.count() as u32)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Hysteresis tests ---

    #[test]
    fn hysteresis_same_level_resets_counter() {
        let mut counter = 5;
        let result = apply_hysteresis(PressureLevel::Clear, PressureLevel::Clear, &mut counter, 3);
        assert_eq!(result, PressureLevel::Clear);
        assert_eq!(counter, 0);
    }

    #[test]
    fn hysteresis_escalation_needs_two_samples() {
        let mut counter = 0;
        let r1 = apply_hysteresis(PressureLevel::Clear, PressureLevel::Strained, &mut counter, 3);
        assert_eq!(r1, PressureLevel::Clear);
        assert_eq!(counter, 1);

        let r2 = apply_hysteresis(PressureLevel::Clear, PressureLevel::Strained, &mut counter, 3);
        assert_eq!(r2, PressureLevel::Strained);
        assert_eq!(counter, 0);
    }

    #[test]
    fn hysteresis_single_escalation_does_not_trigger() {
        let mut counter = 0;
        let result = apply_hysteresis(
            PressureLevel::Clear,
            PressureLevel::Critical,
            &mut counter,
            3,
        );
        assert_eq!(result, PressureLevel::Clear);
        assert_eq!(counter, 1);
    }

    #[test]
    fn hysteresis_deescalation_needs_three_samples() {
        let mut counter = 0;
        let r1 = apply_hysteresis(PressureLevel::Critical, PressureLevel::Clear, &mut counter, 3);
        assert_eq!(r1, PressureLevel::Critical);
        assert_eq!(counter, 1);

        let r2 = apply_hysteresis(PressureLevel::Critical, PressureLevel::Clear, &mut counter, 3);
        assert_eq!(r2, PressureLevel::Critical);
        assert_eq!(counter, 2);

        let r3 = apply_hysteresis(PressureLevel::Critical, PressureLevel::Clear, &mut counter, 3);
        assert_eq!(r3, PressureLevel::Clear);
        assert_eq!(counter, 0);
    }

    #[test]
    fn hysteresis_two_deescalation_samples_insufficient() {
        let mut counter = 0;
        apply_hysteresis(PressureLevel::Strained, PressureLevel::Clear, &mut counter, 3);
        let r = apply_hysteresis(PressureLevel::Strained, PressureLevel::Clear, &mut counter, 3);
        assert_eq!(r, PressureLevel::Strained);
        assert_eq!(counter, 2);
    }

    #[test]
    fn hysteresis_interleaved_resets_accumulation() {
        let mut counter = 0;
        apply_hysteresis(PressureLevel::Clear, PressureLevel::Strained, &mut counter, 3);
        assert_eq!(counter, 1);

        // Back to same level resets counter
        apply_hysteresis(PressureLevel::Clear, PressureLevel::Clear, &mut counter, 3);
        assert_eq!(counter, 0);

        // Start over — one sample is not enough
        let r = apply_hysteresis(PressureLevel::Clear, PressureLevel::Strained, &mut counter, 3);
        assert_eq!(r, PressureLevel::Clear);
        assert_eq!(counter, 1);
    }

    // --- parse_etime tests ---

    #[test]
    fn etime_mm_ss() {
        assert_eq!(parse_etime("00:30"), 30);
    }

    #[test]
    fn etime_mm_ss_five_minutes() {
        assert_eq!(parse_etime("05:00"), 300);
    }

    #[test]
    fn etime_hh_mm_ss() {
        assert_eq!(parse_etime("1:30:00"), 1 * 3600 + 30 * 60);
    }

    #[test]
    fn etime_dd_hh_mm_ss() {
        assert_eq!(
            parse_etime("2-03:30:00"),
            2 * 86400 + 3 * 3600 + 30 * 60
        );
    }

    #[test]
    fn etime_just_seconds_is_malformed() {
        assert_eq!(parse_etime("42"), u64::MAX);
    }

    #[test]
    fn etime_empty_is_malformed() {
        assert_eq!(parse_etime(""), u64::MAX);
    }

    // --- is_protected tests ---

    #[test]
    fn protected_exact_match() {
        assert!(is_protected("guardiand"));
    }

    #[test]
    fn protected_prefix_match() {
        assert!(is_protected("Cursor Helper (GPU)"));
    }

    #[test]
    fn protected_cursor_base() {
        assert!(is_protected("Cursor"));
    }

    #[test]
    fn not_protected_unknown_name() {
        assert!(!is_protected("my-script"));
    }

    #[test]
    fn protected_by_prefix_match() {
        // "guardiand" is in the list and "guardiand-test-runner" starts with it
        assert!(is_protected("guardiand-test-runner"));
    }

    #[test]
    fn protected_docker_desktop() {
        assert!(is_protected("Docker Desktop"));
    }

    #[test]
    fn not_protected_empty_string() {
        assert!(!is_protected(""));
    }
}
