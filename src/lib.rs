#![allow(dead_code)]

mod classifier;
pub mod config;
pub mod disk_targets;
mod docker;
mod hook_policy;
mod sampler;
mod session_db;
mod state;
pub mod stats;
mod validation;
pub mod zeno;

use classifier::{classify_with_band, ClassifierInput, PressureLevel};
use config::{load_config_report, parse_config, ConfigSource, GuardianConfig};
use hook_policy::write_hook_policy;
use docker::stats::DockerClient;
use sampler::cpu::{cpu_usage_percent, CpuSnapshot};
use sampler::disk::compute_disk_state;
use sampler::memory::{MemoryInfo, MemoryPressureLevel};
use sampler::process::{AgentRss, ProcessInfo};
use sampler::swap::SwapInfo;
use sampler::thermal::ThermalState;
use state::{AgentsState, CursorState, DiskState, DockerState, GuardianState, write_state};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

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
    // Claude / Codex agents
    "Claude",
    "claude",
    "Codex",
    "codex",
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

/// Cap for `~/.guardian/guardiand.stderr.log`; launchd never rotates it, and a full disk
/// plus per-sample error logging once grew it past 100 MB.
const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;
/// Re-check the log size every N samples (~30 min at the default 2 s interval).
const LOG_CAP_CHECK_EVERY: u64 = 900;

/// Truncate our own stderr log once it exceeds `max_bytes`. launchd opens StandardErrorPath in
/// append mode (`lsof +fg` shows the `AP` flag on fd 2), so writes continue at the new end
/// after truncation instead of leaving a sparse file.
fn cap_log_file(path: &std::path::Path, max_bytes: u64) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.file_type().is_file() || meta.len() <= max_bytes {
        return false;
    }
    match std::fs::OpenOptions::new().write(true).open(path) {
        Ok(f) => f.set_len(0).is_ok(),
        Err(_) => false,
    }
}

fn cap_daemon_log() {
    let path = config::guardian_dir().join("guardiand.stderr.log");
    if cap_log_file(&path, MAX_LOG_BYTES) {
        eprintln!("[guardiand] log exceeded {} MB — truncated", MAX_LOG_BYTES / (1024 * 1024));
    }
}

pub fn run_daemon() {
    cap_daemon_log();
    eprintln!("[guardiand] starting (pid {})", std::process::id());

    let mut watcher = ConfigWatcher::start();
    let config = watcher.config.clone();
    let initial_steps = crate::zeno::load_zeno_steps();
    let merged = crate::zeno::apply_zeno(&config, initial_steps);
    eprintln!(
        "[guardiand] config loaded: interval={}s, zeno_steps={}, escalate={} samples, deescalate={}s, exit_band={}",
        config.sample_interval(),
        initial_steps,
        config.hysteresis.escalate_samples,
        config.hysteresis.deescalate_secs,
        config.hysteresis.band()
    );
    for problem in &watcher.problems {
        eprintln!("[guardiand] config problem: {problem}");
    }

    if let Err(e) = write_hook_policy(&merged) {
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

    run_loop(&mut watcher, initial_steps);

    write_shutdown_state();
    eprintln!("[guardiand] shutdown complete");
}

fn docker_client_for(config: &GuardianConfig) -> Option<DockerClient> {
    if !config.docker.enabled {
        return None;
    }
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
}

/// The settings in force, kept in step with `config.toml` while the daemon runs.
///
/// An edit is picked up within a couple of samples — no restart. A file that no longer parses is
/// reported and **not applied**: the settings already in force stay, so a typo can never swap
/// the user's thresholds for the built-in defaults mid-session.
struct ConfigWatcher {
    config: GuardianConfig,
    /// What to tell the user about their config (published in `state.json`).
    problems: Vec<String>,
    /// Identity of the file version last examined: modification time and length.
    seen: Option<(SystemTime, u64)>,
    /// A changed version waiting to be confirmed: it is read once the file has looked the
    /// same on two samples in a row, so an editor that truncates and then writes is never
    /// caught halfway (an empty file parses, and means "all defaults").
    pending: Option<Option<(SystemTime, u64)>>,
    /// The file being watched (`~/.guardian/config.toml`).
    path: std::path::PathBuf,
    /// Where a copy of each version that parses is kept.
    last_good: std::path::PathBuf,
}

impl ConfigWatcher {
    fn fingerprint(path: &std::path::Path) -> Option<(SystemTime, u64)> {
        let meta = std::fs::metadata(path).ok()?;
        Some((meta.modified().ok()?, meta.len()))
    }

    fn start() -> Self {
        let path = config::config_path();
        let last_good = config::last_good_path();
        let seen = Self::fingerprint(&path);
        let loaded = load_config_report();
        if loaded.source == ConfigSource::File {
            // Keep a copy to fall back to if a later edit breaks the file.
            if let Ok(text) = std::fs::read_to_string(&path) {
                if parse_config(&text).is_ok() {
                    if let Err(e) = config::remember_last_good(&last_good, &text) {
                        eprintln!("[guardiand] could not save config.last_good.toml: {e}");
                    }
                }
            }
        }
        Self {
            config: loaded.config,
            problems: loaded.problems,
            seen,
            pending: None,
            path,
            last_good,
        }
    }

    /// Re-read `config.toml` if it changed. Returns true when new settings are in force.
    fn poll(&mut self) -> bool {
        let now = Self::fingerprint(&self.path);
        if now == self.seen {
            self.pending = None;
            return false;
        }
        if self.pending != Some(now) {
            self.pending = Some(now);
            return false;
        }
        self.pending = None;
        self.seen = now;

        match std::fs::read_to_string(&self.path) {
            Ok(text) => {
                let applied = self.apply_text(&text);
                if applied {
                    if let Err(e) = config::remember_last_good(&self.last_good, &text) {
                        eprintln!("[guardiand] could not save config.last_good.toml: {e}");
                    }
                }
                applied
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("[guardiand] config.toml removed — using built-in defaults");
                self.config = GuardianConfig::default();
                self.problems.clear();
                true
            }
            Err(e) => {
                // Not remembered as seen: a permissions fix changes neither the file's time
                // nor its length, so the read is tried again.
                self.seen = None;
                self.reject(format!("config.toml could not be read ({e})"));
                false
            }
        }
    }

    /// Apply new `config.toml` text. No file access, so this is what the tests exercise.
    fn apply_text(&mut self, text: &str) -> bool {
        match parse_config(text) {
            Ok((config, problems)) => {
                eprintln!("[guardiand] config.toml changed — new settings in force");
                for problem in &problems {
                    eprintln!("[guardiand] config problem: {problem}");
                }
                self.config = config;
                self.problems = problems;
                true
            }
            Err(e) => {
                self.reject(format!("config.toml has an error ({e})"));
                false
            }
        }
    }

    fn reject(&mut self, what: String) {
        let problem =
            format!("{what}; the change was not applied and the previous settings stay in force");
        if self.problems.first() != Some(&problem) {
            eprintln!("[guardiand] config problem: {problem}");
        }
        self.problems = vec![problem];
    }
}

fn run_loop(watcher: &mut ConfigWatcher, mut last_hook_steps: u32) {
    let mut docker_client = docker_client_for(&watcher.config);
    let mut prev_cpu = CpuSnapshot::sample().unwrap_or_default();
    let mut prev_pressure = PressureLevel::Clear;
    let mut docker_throttled = false;
    let mut hysteresis = Hysteresis::default();
    let mut held_reasons: Vec<String> = Vec::new();
    let mut last_strained_reasons: Vec<String> = Vec::new();
    let mut samples: u64 = 0;
    let mut stats = stats::PressureStats::load();
    let mut last_tick = Instant::now();

    loop {
        thread::sleep(Duration::from_secs(watcher.config.sample_interval()));

        // Pick up config.toml edits without a restart.
        let config_changed = watcher.poll();
        if config_changed {
            docker_client = docker_client_for(&watcher.config);
        }
        let config = &watcher.config;
        let escalate_samples = config.hysteresis.escalate();
        let deescalate_samples = config
            .hysteresis
            .deescalate_samples(config.sample_interval());
        let exit_band = config.hysteresis.band();
        // A sample that took far longer than the interval (slow Docker, a stalled disk) is
        // not evidence about the level for that whole time.
        let max_tick_secs = (config.sample_interval() * 5) as f64;
        let docker_client = docker_client.as_ref();

        samples = samples.wrapping_add(1);
        if samples % LOG_CAP_CHECK_EVERY == 0 {
            cap_daemon_log();
        }

        // Account the time just slept to the level that was published during it.
        let day = stats::today_key();
        stats.record(
            &day,
            prev_pressure,
            last_tick.elapsed().as_secs_f64().min(max_tick_secs),
        );
        last_tick = Instant::now();

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
        let memory_pressure = MemoryPressureLevel::sample();
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
            memory_pressure,
        };

        let steps = crate::zeno::load_zeno_steps();
        let merged = crate::zeno::apply_zeno(config, steps);
        if steps != last_hook_steps || config_changed {
            if let Err(e) = write_hook_policy(&merged) {
                eprintln!("[guardiand] warning: could not write hook_policy.json: {e}");
            }
            last_hook_steps = steps;
        }

        // Exit band: a level is entered at the thresholds but left only once the numbers
        // have moved past them, so a value hovering at a threshold holds its level.
        let classification =
            classify_with_band(&input, &merged.thresholds, prev_pressure, exit_band);
        let raw_pressure = classification.level;

        // Dwell: escalate after a couple of samples, de-escalate only after a sustained drop.
        let pressure = hysteresis.step(
            prev_pressure,
            raw_pressure,
            escalate_samples,
            deescalate_samples,
        );

        if raw_pressure == PressureLevel::Strained {
            last_strained_reasons = classification.reasons.clone();
        }
        // A dwell can end on a sample lower than the level it publishes (held at strained, last
        // sample clear): explain that level with the sample that last showed it.
        let reasons = if pressure != prev_pressure && raw_pressure < pressure {
            &last_strained_reasons
        } else {
            &classification.reasons
        };
        update_held_reasons(
            &mut held_reasons,
            raw_pressure,
            pressure,
            pressure != prev_pressure,
            reasons,
        );

        if pressure != prev_pressure {
            stats.record_transition(&day, pressure);
            eprintln!(
                "[guardiand] {} pressure transition: {} -> {} (cpu={:.1}%, mem_avail={:.1}GB, swap={:.1}%, kernel_mem={}, thermal={}; {})",
                chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%z"),
                prev_pressure,
                pressure,
                cpu_pct,
                mem_available_gb,
                swap_pct,
                memory_pressure,
                thermal,
                if classification.reasons.is_empty() {
                    "no active causes".to_string()
                } else {
                    classification.reasons.join("; ")
                }
            );
        }

        // Docker throttle: continuous enforcement, not just on transition
        handle_docker_throttle(config, docker_client, pressure, &mut docker_throttled);

        // Process killer: identify and kill top resource abusers
        handle_process_killer(config, &procs, pressure);

        let agent_rss = AgentRss::sample();
        let cursor_rss_mb = agent_rss.cursor_mb;
        let agents_state = AgentsState {
            cursor_mb: agent_rss.cursor_mb,
            claude_mb: agent_rss.claude_mb,
            codex_mb: agent_rss.codex_mb,
            total_mb: agent_rss.total_mb(),
        };
        let cursor_state = CursorState {
            active_sessions: count_cursor_sessions(),
            process_count: procs
                .as_ref()
                .map(|p| p.cursor_process_count)
                .unwrap_or(0),
            resident_memory_megabytes: cursor_rss_mb,
        };

        let disk_state = compute_disk_state(&merged.disk);

        let now = chrono::Utc::now().to_rfc3339();
        let guardian_state = GuardianState {
            pressure,
            pressure_reasons: held_reasons.clone(),
            config_problems: watcher.problems.clone(),
            cpu_percent: (cpu_pct * 10.0).round() / 10.0,
            memory_available_gb: (mem_available_gb * 100.0).round() / 100.0,
            memory_total_gb: (mem_total_gb * 100.0).round() / 100.0,
            swap_used_percent: (swap_pct * 10.0).round() / 10.0,
            memory_pressure_level: memory_pressure,
            thermal_state: thermal,
            docker: docker_state,
            cursor: cursor_state,
            agents: agents_state,
            disk: disk_state,
            process_count: procs.as_ref().map(|p| p.total_count).unwrap_or(0),
            max_proc_per_uid: procs.as_ref().map(|p| p.max_proc_per_uid).unwrap_or(0),
            sampled_at: now,
        };

        if let Err(e) = write_state(&guardian_state) {
            eprintln!("[guardiand] write state error: {e}");
        }

        if samples % STATS_SAVE_EVERY == 0 {
            save_stats(&mut stats);
        }

        prev_pressure = pressure;
    }

    save_stats(&mut stats);
}

/// Persist the per-day level accounting every N samples (~1 min at the default interval).
const STATS_SAVE_EVERY: u64 = 30;

fn save_stats(stats: &mut stats::PressureStats) {
    stats.prune(stats::KEEP_DAYS);
    if let Err(e) = stats.save() {
        eprintln!("[guardiand] could not write pressure_stats.json: {e}");
    }
}

fn write_shutdown_state() {
    let shutdown_state = GuardianState {
        pressure: PressureLevel::Clear,
        pressure_reasons: Vec::new(),
        config_problems: Vec::new(),
        cpu_percent: 0.0,
        memory_available_gb: 0.0,
        memory_total_gb: 0.0,
        swap_used_percent: 0.0,
        memory_pressure_level: MemoryPressureLevel::Unknown,
        thermal_state: ThermalState::Unknown,
        docker: DockerState::default(),
        cursor: CursorState::default(),
        agents: AgentsState::default(),
        disk: DiskState::default(),
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

/// Keep the reasons that explain the level hooks will actually see.
///
/// The dwell time holds a level for a while after the raw classification moves on. The
/// reasons published with a held level must be the ones that established it — not the current
/// sample's, which would justify a `critical` block with a merely `strained` cause, or with
/// nothing at all. So reasons are replaced when the raw level agrees with the held one, and
/// when the level has just changed (the current sample's causes are what moved it).
fn update_held_reasons(
    held: &mut Vec<String>,
    raw: PressureLevel,
    level: PressureLevel,
    level_changed: bool,
    reasons: &[String],
) {
    if level == PressureLevel::Clear {
        held.clear();
    } else if raw == level || level_changed {
        *held = reasons.to_vec();
    }
}

/// Dwell-time hysteresis on the published level.
///
/// A change is published only after an unbroken run of samples on the same side of the
/// current level: `escalate_samples` to go up (respond fast to danger), `deescalate_samples`
/// to come down (do not release early). A sample at the current level, or one on the other
/// side, breaks the run.
#[derive(Default, Debug)]
struct Hysteresis {
    /// Length of the current run: positive while escalating, negative while de-escalating.
    run: i32,
    /// For an escalating run: the lowest level proposed in it.
    floor: Option<PressureLevel>,
    /// For a de-escalating run: the highest level proposed in it.
    ceiling: Option<PressureLevel>,
}

impl Hysteresis {
    fn step(
        &mut self,
        current: PressureLevel,
        proposed: PressureLevel,
        escalate_samples: u32,
        deescalate_samples: u32,
    ) -> PressureLevel {
        if proposed == current {
            *self = Self::default();
            return current;
        }

        let up = proposed > current;
        if (up && self.run < 0) || (!up && self.run > 0) {
            *self = Self::default();
        }

        if up {
            // Escalate to the level every sample in the run supports: one `critical` spike
            // after one `strained` sample publishes `strained`, not `critical`.
            let floor = self.floor.map_or(proposed, |f| f.min(proposed));
            self.floor = Some(floor);
            self.run = self.run.saturating_add(1);
            if self.run.unsigned_abs() >= escalate_samples.max(1) {
                *self = Self::default();
                return floor;
            }
        } else {
            // De-escalate to the level every sample in the run stayed at or under: a dwell of
            // `strained` samples that ends on one `clear` publishes `strained`, not `clear`.
            let ceiling = self.ceiling.map_or(proposed, |c| c.max(proposed));
            self.ceiling = Some(ceiling);
            self.run = self.run.saturating_sub(1);
            if self.run.unsigned_abs() >= deescalate_samples.max(1) {
                *self = Self::default();
                return ceiling;
            }
        }
        current
    }
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

    use PressureLevel::{Clear, Critical, Strained};

    /// Feed a sequence of raw levels through the hysteresis; return the published levels.
    fn run(start: PressureLevel, raw: &[PressureLevel], up: u32, down: u32) -> Vec<PressureLevel> {
        let mut h = Hysteresis::default();
        let mut level = start;
        raw.iter()
            .map(|p| {
                level = h.step(level, *p, up, down);
                level
            })
            .collect()
    }

    #[test]
    fn hysteresis_same_level_is_stable() {
        assert_eq!(run(Clear, &[Clear, Clear, Clear], 2, 3), vec![Clear, Clear, Clear]);
    }

    #[test]
    fn hysteresis_escalation_needs_two_samples() {
        assert_eq!(run(Clear, &[Strained, Strained], 2, 3), vec![Clear, Strained]);
    }

    #[test]
    fn hysteresis_single_spike_does_not_escalate() {
        assert_eq!(run(Clear, &[Critical, Clear, Critical, Clear], 2, 3), vec![Clear; 4]);
    }

    #[test]
    fn hysteresis_deescalation_needs_the_full_dwell() {
        assert_eq!(
            run(Critical, &[Clear, Clear, Clear], 2, 3),
            vec![Critical, Critical, Clear]
        );
    }

    #[test]
    fn hysteresis_deescalation_stops_at_the_highest_level_in_the_run() {
        // Two strained samples and one clear: the machine was never clear for a whole dwell.
        assert_eq!(
            run(Critical, &[Strained, Strained, Clear], 2, 3),
            vec![Critical, Critical, Strained]
        );
        // Clear then needs a dwell of its own.
        assert_eq!(
            run(Critical, &[Strained, Strained, Clear, Clear, Clear, Clear], 2, 3),
            vec![Critical, Critical, Strained, Strained, Strained, Clear]
        );
    }

    #[test]
    fn hysteresis_return_to_level_restarts_the_dwell() {
        // Two low samples, back to critical, then two more low samples: never three in a row.
        assert_eq!(
            run(Critical, &[Strained, Strained, Critical, Strained, Strained], 2, 3),
            vec![Critical; 5]
        );
    }

    #[test]
    fn hysteresis_mixed_escalation_publishes_the_level_both_samples_support() {
        // strained then critical: two samples above clear, but only one says critical.
        assert_eq!(run(Clear, &[Strained, Critical], 2, 3), vec![Clear, Strained]);
        // ...and a second critical sample then completes the next step.
        assert_eq!(
            run(Clear, &[Strained, Critical, Critical], 2, 3),
            vec![Clear, Strained, Strained]
        );
        assert_eq!(
            run(Clear, &[Strained, Critical, Critical, Critical], 2, 3),
            vec![Clear, Strained, Strained, Critical]
        );
    }

    #[test]
    fn hysteresis_low_samples_do_not_count_toward_escalation() {
        // At strained: a long run of clear samples must not let one critical sample through.
        assert_eq!(
            run(Strained, &[Clear, Clear, Clear, Clear, Critical], 2, 15),
            vec![Strained; 5]
        );
    }

    #[test]
    fn hysteresis_high_sample_breaks_a_deescalation_run() {
        assert_eq!(
            run(Strained, &[Clear, Clear, Critical, Clear, Clear], 2, 3),
            vec![Strained; 5]
        );
    }

    #[test]
    fn hysteresis_threshold_hover_holds_for_the_dwell() {
        // The flapping case: raw level alternates every two samples. With a 15-sample dwell
        // the published level rises once and stays there.
        let raw: Vec<PressureLevel> = (0..60)
            .map(|i| if (i / 2) % 2 == 0 { Critical } else { Strained })
            .collect();
        let published = run(Strained, &raw, 2, 15);
        assert_eq!(published[0], Strained);
        assert_eq!(published[1], Critical);
        let later_changes = published[1..].windows(2).filter(|w| w[0] != w[1]).count();
        assert_eq!(later_changes, 0, "{published:?}");
    }

    #[test]
    fn hysteresis_zero_sample_counts_behave_as_one() {
        assert_eq!(run(Clear, &[Critical], 0, 0), vec![Critical]);
        assert_eq!(run(Critical, &[Clear], 0, 0), vec![Clear]);
    }

    // --- config watcher tests ---

    fn watcher() -> ConfigWatcher {
        watcher_in(&std::env::temp_dir().join("guardian-watcher-unused"))
    }

    /// A watcher over `dir/config.toml`; nothing under ~/.guardian is touched.
    fn watcher_in(dir: &std::path::Path) -> ConfigWatcher {
        ConfigWatcher {
            config: GuardianConfig::default(),
            problems: Vec::new(),
            seen: None,
            pending: None,
            path: dir.join("config.toml"),
            last_good: dir.join("config.last_good.toml"),
        }
    }

    #[test]
    fn edits_on_disk_are_picked_up_and_broken_ones_are_not() {
        let dir = std::env::temp_dir().join(format!("guardian-watcher-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut w = watcher_in(&dir);

        // No file yet: nothing to do, and asking twice changes nothing.
        assert!(!w.poll());
        assert!(!w.poll());

        std::fs::write(&w.path, "[thresholds]\ncritical_cpu_percent = 97.0\n").unwrap();
        assert!(!w.poll(), "a change is applied only once it has settled");
        assert_eq!(w.config.thresholds.critical_cpu_percent, 90.0);
        assert!(w.poll());
        assert_eq!(w.config.thresholds.critical_cpu_percent, 97.0);
        assert!(!w.poll(), "an unchanged file is not re-applied");
        let saved = std::fs::read_to_string(&w.last_good).unwrap();
        assert!(saved.contains("97.0"));

        // A typo mid-edit (different length, so the change is seen whatever the clock does).
        std::fs::write(&w.path, "[thresholds\ncritical_cpu_percent = 12.0\n# oops\n").unwrap();
        assert!(!w.poll());
        assert!(w.problems.is_empty(), "not read until it has settled");
        assert!(!w.poll());
        assert_eq!(w.config.thresholds.critical_cpu_percent, 97.0);
        assert_eq!(w.problems.len(), 1);
        // The last good copy still holds the version that worked.
        assert!(std::fs::read_to_string(&w.last_good).unwrap().contains("97.0"));

        // Fixed.
        std::fs::write(&w.path, "[thresholds]\ncritical_cpu_percent = 96.5\n# fixed now\n").unwrap();
        assert!(!w.poll());
        assert!(w.poll());
        assert_eq!(w.config.thresholds.critical_cpu_percent, 96.5);
        assert!(w.problems.is_empty());

        // Removed: defaults, which is the same as never having had a file.
        std::fs::remove_file(&w.path).unwrap();
        assert!(!w.poll());
        assert!(w.poll());
        assert_eq!(w.config.thresholds.critical_cpu_percent, 90.0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn valid_edit_is_applied() {
        let mut w = watcher();
        assert!(w.apply_text("[thresholds]\ncritical_cpu_percent = 97.0\n"));
        assert_eq!(w.config.thresholds.critical_cpu_percent, 97.0);
        assert!(w.problems.is_empty());
    }

    #[test]
    fn broken_edit_keeps_the_settings_in_force() {
        let mut w = watcher();
        assert!(w.apply_text("[thresholds]\ncritical_cpu_percent = 97.0\n"));
        // A typo while editing: not TOML any more.
        assert!(!w.apply_text("[thresholds\ncritical_cpu_percent = 12.0\n"));
        assert_eq!(w.config.thresholds.critical_cpu_percent, 97.0);
        assert_eq!(w.problems.len(), 1);
        assert!(w.problems[0].contains("previous settings stay in force"), "{:?}", w.problems);
    }

    #[test]
    fn fixing_the_file_clears_the_problem() {
        let mut w = watcher();
        assert!(!w.apply_text("not toml at all ]["));
        assert_eq!(w.problems.len(), 1);
        assert!(w.apply_text("sample_interval_secs = 3\n"));
        assert!(w.problems.is_empty());
        assert_eq!(w.config.sample_interval(), 3);
    }

    #[test]
    fn warnings_are_published_but_the_edit_still_applies() {
        let mut w = watcher();
        assert!(w.apply_text("[hysteresis]\nexit_bnad = 0.3\ndeescalate_secs = 45\n"));
        assert_eq!(w.config.hysteresis.deescalate_secs, 45);
        assert_eq!(w.problems.len(), 1);
        assert!(w.problems[0].contains("exit_bnad"), "{:?}", w.problems);
    }

    // --- held reasons tests ---

    fn r(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn held_reasons_follow_the_level_when_raw_agrees() {
        let mut held = Vec::new();
        update_held_reasons(
            &mut held,
            PressureLevel::Critical,
            PressureLevel::Critical,
            false,
            &r(&["thermal pressure heavy"]),
        );
        assert_eq!(held, r(&["thermal pressure heavy"]));
    }

    #[test]
    fn held_critical_keeps_its_cause_while_raw_is_only_strained() {
        let mut held = r(&["only 0.3 GB RAM available"]);
        update_held_reasons(
            &mut held,
            PressureLevel::Strained,
            PressureLevel::Critical,
            false,
            &r(&["CPU 75%"]),
        );
        assert_eq!(held, r(&["only 0.3 GB RAM available"]));
    }

    #[test]
    fn held_critical_keeps_its_cause_while_raw_is_clear() {
        let mut held = r(&["swap 95% used"]);
        update_held_reasons(&mut held, PressureLevel::Clear, PressureLevel::Critical, false, &[]);
        assert_eq!(held, r(&["swap 95% used"]));
    }

    #[test]
    fn pending_escalation_does_not_leak_critical_causes_into_strained() {
        let mut held = r(&["swap 30% used"]);
        update_held_reasons(
            &mut held,
            PressureLevel::Critical,
            PressureLevel::Strained,
            false,
            &r(&["thermal pressure critical"]),
        );
        assert_eq!(held, r(&["swap 30% used"]));
    }

    #[test]
    fn a_level_change_takes_the_current_samples_causes() {
        // Clear -> strained published while the raw sample already reads critical: the
        // advisory must still say why, using what the sample shows.
        let mut held = Vec::new();
        update_held_reasons(
            &mut held,
            PressureLevel::Critical,
            PressureLevel::Strained,
            true,
            &r(&["thermal pressure heavy"]),
        );
        assert_eq!(held, r(&["thermal pressure heavy"]));
    }

    #[test]
    fn clear_level_has_no_reasons() {
        let mut held = r(&["swap 30% used"]);
        update_held_reasons(&mut held, PressureLevel::Strained, PressureLevel::Clear, true, &r(&["x"]));
        assert!(held.is_empty());
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

    #[test]
    fn protected_claude_and_codex_agents() {
        assert!(is_protected("claude"));
        assert!(is_protected("Claude Helper (Renderer)"));
        assert!(is_protected("codex"));
    }

    // --- log cap tests ---

    #[test]
    fn log_cap_truncates_oversized_file() {
        let path = std::env::temp_dir().join(format!("guardian-logcap-{}.log", std::process::id()));
        std::fs::write(&path, vec![b'x'; 4096]).unwrap();
        assert!(cap_log_file(&path, 1024));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn log_cap_leaves_small_file_alone() {
        let path = std::env::temp_dir().join(format!("guardian-logcap-small-{}.log", std::process::id()));
        std::fs::write(&path, b"hello").unwrap();
        assert!(!cap_log_file(&path, 1024));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 5);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn log_cap_missing_file_is_noop() {
        let path = std::env::temp_dir().join("guardian-logcap-does-not-exist.log");
        assert!(!cap_log_file(&path, 1024));
    }
}
