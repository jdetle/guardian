use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Deserialize, Clone, Debug)]
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
        }
    }
}

/// Thresholds for home-volume disk advisories (`state.json` → `disk`).
#[derive(Deserialize, Clone, Debug)]
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
    /// `never` | `strained` | `critical` — block prompt submit when pressure is at or above this band.
    #[serde(default = "default_block_on")]
    pub block_on: String,
    #[serde(default = "default_true")]
    pub block_on_session_budget: bool,
}

impl Default for PromptGateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            block_on: default_block_on(),
            block_on_session_budget: true,
        }
    }
}

fn default_block_on() -> String {
    "critical".to_string()
}

/// Gates derived from aggregate Cursor RSS (`state.json` → `cursor.resident_memory_megabytes`),
/// not from `~/.cursor/projects` folder count (that field remains diagnostic-only in state).
#[derive(Deserialize, Clone, Debug)]
pub struct SessionBudgetConfig {
    /// Block `beforeSubmitPrompt` when Cursor RSS exceeds this (MB). `0` disables RSS blocking.
    #[serde(default = "default_max_cursor_rss_mb")]
    pub max_cursor_rss_megabytes: u64,
    /// Session-start advisory when Cursor RSS exceeds this (MB).
    #[serde(default = "default_warn_cursor_rss_mb")]
    pub warn_cursor_rss_megabytes: u64,
    /// Legacy keys — ignored for gates; kept so older `config.toml` still deserializes.
    #[serde(default)]
    pub max_active_sessions: Option<u32>,
    #[serde(default)]
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
    #[serde(default)]
    pub strained_memory_available_ratio: Option<f64>,
    #[serde(default)]
    pub critical_memory_available_ratio: Option<f64>,
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

#[derive(Deserialize, Clone, Debug)]
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

#[derive(Deserialize, Clone, Debug)]
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
