use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct GuardianConfig {
    #[serde(default = "default_sample_interval_secs")]
    pub sample_interval_secs: u64,
    #[serde(default)]
    pub thresholds: ThresholdConfig,
    #[serde(default)]
    pub docker: DockerConfig,
    #[serde(default)]
    pub fork_guard: ForkGuardConfig,
    #[serde(default)]
    pub prompt_gate: PromptGateConfig,
    #[serde(default)]
    pub session_budget: SessionBudgetConfig,
    #[serde(default)]
    pub cursorignore_policy: CursorIgnorePolicyConfig,
    #[serde(default)]
    pub disk: DiskConfig,
    #[serde(default)]
    pub queue: QueueConfig,
    #[serde(default)]
    pub tool_gate: ToolGateConfig,
    #[serde(default)]
    pub hysteresis: HysteresisConfig,
}

impl Default for GuardianConfig {
    fn default() -> Self {
        Self {
            sample_interval_secs: default_sample_interval_secs(),
            thresholds: ThresholdConfig::default(),
            docker: DockerConfig::default(),
            fork_guard: ForkGuardConfig::default(),
            prompt_gate: PromptGateConfig::default(),
            session_budget: SessionBudgetConfig::default(),
            cursorignore_policy: CursorIgnorePolicyConfig::default(),
            disk: DiskConfig::default(),
            queue: QueueConfig::default(),
            tool_gate: ToolGateConfig::default(),
            hysteresis: HysteresisConfig::default(),
        }
    }
}

/// How quickly the published pressure level may change (`[hysteresis]`).
///
/// Escalation stays fast; leaving a level takes both an exit band on the numeric thresholds
/// and a dwell time, so a signal hovering at a threshold cannot flip the level every few
/// seconds.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct HysteresisConfig {
    /// Consecutive higher samples needed to raise the level.
    #[serde(default = "default_escalate_samples")]
    pub escalate_samples: u32,
    /// Seconds of consecutively lower samples needed before the level drops.
    #[serde(default = "default_deescalate_secs")]
    pub deescalate_secs: u64,
    /// Exit band as a fraction of each numeric threshold (0.10 = leave `critical` swap at 90%
    /// only below 81%; leave 1.0 GB available only above 1.1 GB). `0` disables the band.
    #[serde(default = "default_exit_band")]
    pub exit_band: f64,
}

impl Default for HysteresisConfig {
    fn default() -> Self {
        Self {
            escalate_samples: default_escalate_samples(),
            deescalate_secs: default_deescalate_secs(),
            exit_band: default_exit_band(),
        }
    }
}

/// Longest sampling interval the daemon uses: hooks treat a `state.json` older than 30
/// seconds as stale and stop gating on it.
pub const MAX_SAMPLE_INTERVAL_SECS: u64 = 30;
/// Longest de-escalation dwell honored. A typo (`300000`) must not hold a level for days.
pub const MAX_DEESCALATE_SECS: u64 = 3600;
/// Most samples ever required to escalate. More would mean the level never rises.
pub const MAX_ESCALATE_SAMPLES: u32 = 30;

impl HysteresisConfig {
    /// Consecutive higher samples required before escalating, limited to a sane range.
    pub fn escalate(&self) -> u32 {
        self.escalate_samples.clamp(1, MAX_ESCALATE_SAMPLES)
    }

    /// Samples of lower pressure required before de-escalating, at this sampling interval.
    pub fn deescalate_samples(&self, sample_interval_secs: u64) -> u32 {
        let interval = sample_interval_secs.max(1);
        let secs = self.deescalate_secs.min(MAX_DEESCALATE_SECS);
        let samples = secs.div_ceil(interval).max(1);
        u32::try_from(samples).unwrap_or(u32::MAX)
    }

    /// The exit band, limited to a sane range (a typo must not pin the level forever).
    pub fn band(&self) -> f64 {
        if self.exit_band.is_nan() {
            return 0.0;
        }
        self.exit_band.clamp(0.0, 0.5)
    }
}

fn default_escalate_samples() -> u32 {
    2
}

fn default_deescalate_secs() -> u64 {
    30
}

fn default_exit_band() -> f64 {
    0.10
}

/// Tool-call gate (preToolUse / PreToolUse hooks). Advisory-only unless configured otherwise.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ToolGateConfig {
    /// Deny new subagent spawns at this pressure: `"never"` (default), `"strained"`, or `"critical"`.
    #[serde(default = "default_deny_subagents_on")]
    pub deny_subagents_on: String,
}

impl Default for ToolGateConfig {
    fn default() -> Self {
        Self {
            deny_subagents_on: default_deny_subagents_on(),
        }
    }
}

fn default_deny_subagents_on() -> String {
    "never".to_string()
}

/// Local agent work queue (`~/.guardian/agent_queue.jsonl`) — see README.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct QueueConfig {
    /// When true, blocked `beforeSubmitPrompt` may append prompt text to the queue (if extractable).
    #[serde(default = "default_false")]
    pub enqueue_on_blocked_submit: bool,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            enqueue_on_blocked_submit: false,
        }
    }
}

/// Thresholds for home-volume disk advisories (`state.json` → `disk`).
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct DiskConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_disk_warn_used_percent")]
    pub warn_used_percent: f64,
    #[serde(default = "default_disk_critical_used_percent")]
    pub critical_used_percent: f64,
}

impl Default for DiskConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            warn_used_percent: default_disk_warn_used_percent(),
            critical_used_percent: default_disk_critical_used_percent(),
        }
    }
}

fn default_disk_warn_used_percent() -> f64 {
    85.0
}

fn default_disk_critical_used_percent() -> f64 {
    93.0
}

/// Policy for `beforeSubmitPrompt` (also written to `hook_policy.json` for shell hooks).
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct PromptGateConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Pressure cutoff in strict mode (never, strained, or critical).
    #[serde(default = "default_block_on")]
    pub block_on: String,
    /// In strict mode, also block when Cursor RSS exceeds the cap under strained or critical pressure.
    #[serde(default = "default_false")]
    pub block_on_session_budget: bool,
    /// Default mode for clients without an override.
    #[serde(default = "default_prompt_gate_mode")]
    pub mode: String,
    /// Overrides keyed by cursor, claude, or codex.
    #[serde(default)]
    pub clients: std::collections::BTreeMap<String, String>,
}

impl Default for PromptGateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            block_on: default_block_on(),
            block_on_session_budget: false,
            mode: default_prompt_gate_mode(),
            clients: Default::default(),
        }
    }
}

fn default_prompt_gate_mode() -> String {
    "adaptive".to_string()
}

fn default_block_on() -> String {
    "critical".to_string()
}

/// Gates derived from aggregate Cursor RSS (`state.json` → `cursor.resident_memory_megabytes`),
/// not from `~/.cursor/projects` folder count (that field remains diagnostic-only in state).
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct SessionBudgetConfig {
    /// Block `beforeSubmitPrompt` when Cursor RSS exceeds this (MB). `0` disables RSS blocking.
    #[serde(default = "default_max_cursor_rss_mb")]
    pub max_cursor_rss_megabytes: u64,
    /// Session-start advisory when Cursor RSS exceeds this (MB).
    #[serde(default = "default_warn_cursor_rss_mb")]
    pub warn_cursor_rss_megabytes: u64,
    /// Legacy keys — ignored for gates; kept so older `config.toml` still deserializes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_active_sessions: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warn_active_sessions: Option<u32>,
}

impl Default for SessionBudgetConfig {
    fn default() -> Self {
        Self {
            max_cursor_rss_megabytes: default_max_cursor_rss_mb(),
            warn_cursor_rss_megabytes: default_warn_cursor_rss_mb(),
            max_active_sessions: None,
            warn_active_sessions: None,
        }
    }
}

fn default_max_cursor_rss_mb() -> u64 {
    8192
}

fn default_warn_cursor_rss_mb() -> u64 {
    4096
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct CursorIgnorePolicyConfig {
    #[serde(default = "default_true")]
    pub warn_once_per_path: bool,
    #[serde(default = "default_true")]
    pub before_read_enabled: bool,
}

impl Default for CursorIgnorePolicyConfig {
    fn default() -> Self {
        Self {
            warn_once_per_path: true,
            before_read_enabled: true,
        }
    }
}

fn default_sample_interval_secs() -> u64 {
    2
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ThresholdConfig {
    #[serde(default = "default_strained_cpu")]
    pub strained_cpu_percent: f64,
    #[serde(default = "default_critical_cpu")]
    pub critical_cpu_percent: f64,
    #[serde(default = "default_strained_memory")]
    pub strained_memory_gb: f64,
    #[serde(default = "default_critical_memory")]
    pub critical_memory_gb: f64,
    #[serde(default = "default_strained_swap")]
    pub strained_swap_percent: f64,
    #[serde(default = "default_critical_swap")]
    pub critical_swap_percent: f64,
    /// When set, also escalate when `available_gb / total_gb` falls below this (e.g. 0.12 = 12% free).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strained_memory_available_ratio: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critical_memory_available_ratio: Option<f64>,
    /// When true (default), swap % and CPU % can only reach `critical` if a second signal
    /// agrees (kernel memory pressure for swap, thermal pressure for CPU); alone they count as
    /// `strained`. Set false to let each threshold reach `critical` on its own.
    #[serde(default = "default_true")]
    pub corroborate_critical: bool,
}

impl Default for ThresholdConfig {
    fn default() -> Self {
        Self {
            strained_cpu_percent: default_strained_cpu(),
            critical_cpu_percent: default_critical_cpu(),
            strained_memory_gb: default_strained_memory(),
            critical_memory_gb: default_critical_memory(),
            strained_swap_percent: default_strained_swap(),
            critical_swap_percent: default_critical_swap(),
            strained_memory_available_ratio: None,
            critical_memory_available_ratio: None,
            corroborate_critical: true,
        }
    }
}

impl ThresholdConfig {
    /// The thresholds a sample must clear to *leave* a level: each numeric threshold moved
    /// `band` (a fraction) further into safe territory. Usage percentages are lowered;
    /// available-memory floors are raised.
    pub fn with_exit_band(&self, band: f64) -> Self {
        let lower = 1.0 - band;
        let raise = 1.0 + band;
        Self {
            strained_cpu_percent: self.strained_cpu_percent * lower,
            critical_cpu_percent: self.critical_cpu_percent * lower,
            strained_memory_gb: self.strained_memory_gb * raise,
            critical_memory_gb: self.critical_memory_gb * raise,
            strained_swap_percent: self.strained_swap_percent * lower,
            critical_swap_percent: self.critical_swap_percent * lower,
            strained_memory_available_ratio: self.strained_memory_available_ratio.map(|r| r * raise),
            critical_memory_available_ratio: self.critical_memory_available_ratio.map(|r| r * raise),
            corroborate_critical: self.corroborate_critical,
        }
    }
}

fn default_strained_cpu() -> f64 {
    70.0
}
fn default_critical_cpu() -> f64 {
    90.0
}
fn default_strained_memory() -> f64 {
    2.0
}
fn default_critical_memory() -> f64 {
    1.0
}
fn default_strained_swap() -> f64 {
    25.0
}
fn default_critical_swap() -> f64 {
    50.0
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct DockerConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_docker_socket")]
    pub socket_path: String,
    #[serde(default)]
    pub essential_containers: Vec<String>,
    #[serde(default = "default_true")]
    pub auto_throttle: bool,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            socket_path: default_docker_socket(),
            essential_containers: Vec::new(),
            auto_throttle: true,
        }
    }
}

fn default_docker_socket() -> String {
    let home = dirs::home_dir().unwrap_or_default();
    let docker_desktop = home.join(".docker/run/docker.sock");
    if docker_desktop.exists() {
        return docker_desktop.to_string_lossy().to_string();
    }
    "/var/run/docker.sock".to_string()
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ForkGuardConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_proc_warn_ratio")]
    pub warn_ratio: f64,
    #[serde(default = "default_proc_kill_ratio")]
    pub kill_ratio: f64,
    #[serde(default)]
    pub kill_enabled: bool,
}

impl Default for ForkGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            warn_ratio: default_proc_warn_ratio(),
            kill_ratio: default_proc_kill_ratio(),
            kill_enabled: false,
        }
    }
}

fn default_proc_warn_ratio() -> f64 {
    0.6
}
fn default_proc_kill_ratio() -> f64 {
    0.8
}

pub fn guardian_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".guardian")
}

pub fn load_config() -> GuardianConfig {
    let path = guardian_dir().join("config.toml");
    match std::fs::read_to_string(&path) {
        Ok(contents) => toml::from_str(&contents).unwrap_or_else(|e| {
            eprintln!("[guardian] config parse error: {e}, using defaults");
            GuardianConfig::default()
        }),
        Err(_) => GuardianConfig::default(),
    }
}

impl GuardianConfig {
    /// The sampling interval the daemon actually uses: 0 would spin the CPU, and a huge
    /// value would leave the daemon asleep, deaf to the edit that fixes it.
    pub fn sample_interval(&self) -> u64 {
        self.sample_interval_secs.clamp(1, MAX_SAMPLE_INTERVAL_SECS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hysteresis_defaults_apply_when_section_is_missing() {
        let cfg: GuardianConfig = toml::from_str("sample_interval_secs = 2\n").unwrap();
        assert_eq!(cfg.hysteresis.escalate_samples, 2);
        assert_eq!(cfg.hysteresis.deescalate_secs, 30);
        assert_eq!(cfg.hysteresis.deescalate_samples(cfg.sample_interval_secs), 15);
    }

    #[test]
    fn deescalate_samples_rounds_up_and_never_reaches_zero() {
        let h = HysteresisConfig {
            deescalate_secs: 5,
            ..HysteresisConfig::default()
        };
        assert_eq!(h.deescalate_samples(2), 3);
        assert_eq!(h.deescalate_samples(0), 5);
        let instant = HysteresisConfig {
            deescalate_secs: 0,
            ..HysteresisConfig::default()
        };
        assert_eq!(instant.deescalate_samples(2), 1);
    }

    #[test]
    fn runaway_timing_values_are_limited() {
        let cfg: GuardianConfig = toml::from_str(
            "sample_interval_secs = 999999999\n[hysteresis]\nescalate_samples = 4000000000\ndeescalate_secs = 300000\n",
        )
        .unwrap();
        assert_eq!(cfg.sample_interval(), MAX_SAMPLE_INTERVAL_SECS);
        assert_eq!(cfg.hysteresis.escalate(), MAX_ESCALATE_SAMPLES);
        assert_eq!(cfg.hysteresis.deescalate_samples(cfg.sample_interval()), 120);
        let zero = HysteresisConfig {
            escalate_samples: 0,
            ..HysteresisConfig::default()
        };
        assert_eq!(zero.escalate(), 1);
    }

    #[test]
    fn exit_band_is_clamped() {
        let mut h = HysteresisConfig::default();
        h.exit_band = 7.0;
        assert_eq!(h.band(), 0.5);
        h.exit_band = -1.0;
        assert_eq!(h.band(), 0.0);
        h.exit_band = f64::NAN;
        assert_eq!(h.band(), 0.0);
    }

    #[test]
    fn exit_band_moves_thresholds_toward_safety() {
        let t = ThresholdConfig::default().with_exit_band(0.10);
        assert!((t.critical_swap_percent - 45.0).abs() < 1e-9);
        assert!((t.strained_cpu_percent - 63.0).abs() < 1e-9);
        assert!((t.critical_memory_gb - 1.1).abs() < 1e-9);
        assert!((t.strained_memory_gb - 2.2).abs() < 1e-9);
    }
}
