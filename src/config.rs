use serde::Deserialize;
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
}

impl Default for GuardianConfig {
    fn default() -> Self {
        Self {
            sample_interval_secs: default_sample_interval_secs(),
            thresholds: ThresholdConfig::default(),
            docker: DockerConfig::default(),
            fork_guard: ForkGuardConfig::default(),
        }
    }
}

fn default_sample_interval_secs() -> u64 {
    2
}

#[derive(Deserialize, Clone, Debug)]
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
