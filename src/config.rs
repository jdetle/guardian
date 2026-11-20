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

const CONFIG_TOML: &str = "config.toml";
const LAST_GOOD_TOML: &str = "config.last_good.toml";

pub fn config_path() -> PathBuf {
    guardian_dir().join(CONFIG_TOML)
}

pub fn last_good_path() -> PathBuf {
    guardian_dir().join(LAST_GOOD_TOML)
}

/// Where the settings in a [`LoadedConfig`] came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigSource {
    /// `config.toml` parsed.
    File,
    /// No `config.toml`: built-in defaults, which is a normal state.
    Missing,
    /// `config.toml` is broken; the last version that parsed is in use instead.
    LastGood,
    /// `config.toml` is broken and no earlier version is available.
    Defaults,
}

/// Settings plus everything the user should be told about their `config.toml`.
#[derive(Clone, Debug)]
pub struct LoadedConfig {
    pub config: GuardianConfig,
    pub source: ConfigSource,
    /// Plain-language problems; empty when the file is clean.
    pub problems: Vec<String>,
}

/// Parse `config.toml` text.
///
/// `Err` means the file cannot be used at all (TOML syntax, wrong value type). `Ok` carries
/// the settings and any warnings: unknown keys (a misspelled setting is otherwise silently
/// ignored) and values that cannot mean what the user intended.
pub fn parse_config(text: &str) -> Result<(GuardianConfig, Vec<String>), String> {
    let value: toml::Value = toml::from_str(text).map_err(|e| one_line(&e.to_string()))?;
    let config: GuardianConfig = toml::from_str(text).map_err(|e| one_line(&e.to_string()))?;

    let mut warnings = Vec::new();
    if let Ok(schema) = toml::Value::try_from(GuardianConfig::schema_example()) {
        unknown_keys(&value, &schema, "", &mut warnings);
    }
    warnings.extend(config.validate());
    Ok((config, warnings))
}

/// Read `config.toml`, falling back to the last version that parsed.
///
/// A broken file must never silently swap the user's thresholds for the built-in defaults:
/// on a small-RAM Mac the defaults read as permanent pressure. Read-only; the daemon records
/// the last good copy with [`remember_last_good`].
pub fn load_config_report() -> LoadedConfig {
    let text = match std::fs::read_to_string(config_path()) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return LoadedConfig {
                config: GuardianConfig::default(),
                source: ConfigSource::Missing,
                problems: Vec::new(),
            }
        }
        Err(e) => return fall_back(format!("config.toml could not be read ({e})")),
    };
    match parse_config(&text) {
        Ok((config, problems)) => LoadedConfig {
            config,
            source: ConfigSource::File,
            problems,
        },
        Err(e) => fall_back(format!("config.toml has an error ({e})")),
    }
}

fn fall_back(what: String) -> LoadedConfig {
    let last_good = std::fs::read_to_string(last_good_path())
        .ok()
        .and_then(|t| parse_config(&t).ok());
    match last_good {
        Some((config, _)) => LoadedConfig {
            config,
            source: ConfigSource::LastGood,
            problems: vec![format!("{what}; using the last settings that worked")],
        },
        None => LoadedConfig {
            config: GuardianConfig::default(),
            source: ConfigSource::Defaults,
            problems: vec![format!("{what}; using built-in defaults")],
        },
    }
}

/// Settings only, for callers that do not report problems.
pub fn load_config() -> GuardianConfig {
    load_config_report().config
}

/// Keep a copy of a `config.toml` that parsed, for [`load_config_report`] to fall back to.
pub fn remember_last_good(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    // An empty file is never worth falling back to: it means "all defaults", which is what
    // a missing file means already, and it may be an editor caught mid-save.
    if text.trim().is_empty() {
        return Ok(());
    }
    if std::fs::read_to_string(path).is_ok_and(|existing| existing == text) {
        return Ok(());
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let result = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// TOML errors span several lines with a source excerpt; problems are shown on one line.
fn one_line(s: &str) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.chars().take(240).collect()
}

/// Keys present in `user` that the schema does not have, as `section.key`.
fn unknown_keys(user: &toml::Value, schema: &toml::Value, path: &str, out: &mut Vec<String>) {
    // Client names under [prompt_gate.clients] are checked by `validate`, with a better message.
    if path == "prompt_gate.clients" {
        return;
    }
    let (Some(user), Some(schema)) = (user.as_table(), schema.as_table()) else {
        return;
    };
    for (key, value) in user {
        let full = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        match schema.get(key) {
            Some(known) => unknown_keys(value, known, &full, out),
            None => {
                let hint = closest(key, schema.keys().map(String::as_str))
                    .map(|k| format!(" (did you mean `{k}`?)"))
                    .unwrap_or_default();
                out.push(format!("unknown setting `{full}` is ignored{hint}"));
            }
        }
    }
}

/// The known key within two edits of `key`, if exactly that close.
fn closest<'a>(key: &str, known: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    known
        .map(|k| (edit_distance(key, k), k))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, k)| k)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut row = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitute = prev[j] + usize::from(ca != cb);
            row.push(substitute.min(prev[j + 1] + 1).min(row[j] + 1));
        }
        prev = row;
    }
    prev[b.len()]
}

const PRESSURE_CUTOFFS: &[&str] = &["never", "strained", "critical"];
const PROMPT_MODES: &[&str] = &["adaptive", "advisory", "strict"];
const CLIENTS: &[&str] = &["cursor", "claude", "codex"];

impl GuardianConfig {
    /// A config with every optional key present, so its serialized form lists every key
    /// `config.toml` may contain. **When adding an `Option` field, set it here.**
    fn schema_example() -> Self {
        let mut c = Self::default();
        c.thresholds.strained_memory_available_ratio = Some(0.15);
        c.thresholds.critical_memory_available_ratio = Some(0.08);
        c.session_budget.max_active_sessions = Some(0);
        c.session_budget.warn_active_sessions = Some(0);
        c
    }

    /// The sampling interval the daemon actually uses: 0 would spin the CPU, and a huge
    /// value would leave the daemon asleep, deaf to the edit that fixes it.
    pub fn sample_interval(&self) -> u64 {
        self.sample_interval_secs.clamp(1, MAX_SAMPLE_INTERVAL_SECS)
    }

    /// Values that parse but cannot mean what was intended. Each is one plain sentence.
    pub fn validate(&self) -> Vec<String> {
        let mut w = Vec::new();
        let t = &self.thresholds;

        if self.sample_interval_secs == 0 {
            w.push("sample_interval_secs is 0; 1 second is used".to_string());
        }
        if self.sample_interval_secs > MAX_SAMPLE_INTERVAL_SECS {
            w.push(format!(
                "sample_interval_secs = {} is too long (hooks ignore state older than 30 seconds); {} is used",
                self.sample_interval_secs, MAX_SAMPLE_INTERVAL_SECS
            ));
        }

        for (name, v) in [
            ("thresholds.strained_cpu_percent", t.strained_cpu_percent),
            ("thresholds.critical_cpu_percent", t.critical_cpu_percent),
            ("thresholds.strained_swap_percent", t.strained_swap_percent),
            ("thresholds.critical_swap_percent", t.critical_swap_percent),
            ("disk.warn_used_percent", self.disk.warn_used_percent),
            ("disk.critical_used_percent", self.disk.critical_used_percent),
        ] {
            if !(0.0..=100.0).contains(&v) {
                w.push(format!("{name} = {v} is not a percentage between 0 and 100"));
            }
        }
        for (low_name, low, high_name, high) in [
            ("thresholds.strained_cpu_percent", t.strained_cpu_percent, "critical_cpu_percent", t.critical_cpu_percent),
            ("thresholds.strained_swap_percent", t.strained_swap_percent, "critical_swap_percent", t.critical_swap_percent),
            ("disk.warn_used_percent", self.disk.warn_used_percent, "critical_used_percent", self.disk.critical_used_percent),
            ("fork_guard.warn_ratio", self.fork_guard.warn_ratio, "kill_ratio", self.fork_guard.kill_ratio),
        ] {
            if low >= high {
                w.push(format!("{low_name} ({low}) should be below {high_name} ({high})"));
            }
        }

        // Available-memory floors run the other way: strained is the larger number.
        if t.strained_memory_gb < 0.0 || t.critical_memory_gb < 0.0 {
            w.push("thresholds: available-memory values cannot be negative".to_string());
        }
        if t.strained_memory_gb <= t.critical_memory_gb {
            w.push(format!(
                "thresholds.strained_memory_gb ({}) should be above critical_memory_gb ({}): \
                 these are GB still available",
                t.strained_memory_gb, t.critical_memory_gb
            ));
        }
        for (name, r) in [
            ("strained_memory_available_ratio", t.strained_memory_available_ratio),
            ("critical_memory_available_ratio", t.critical_memory_available_ratio),
        ] {
            if let Some(r) = r {
                if !(0.0..=1.0).contains(&r) {
                    w.push(format!("thresholds.{name} = {r} is not a fraction between 0 and 1"));
                }
            }
        }
        if let (Some(s), Some(c)) = (
            t.strained_memory_available_ratio,
            t.critical_memory_available_ratio,
        ) {
            if s <= c {
                w.push(format!(
                    "thresholds.strained_memory_available_ratio ({s}) should be above \
                     critical_memory_available_ratio ({c})"
                ));
            }
        }
        for (name, r) in [
            ("fork_guard.warn_ratio", self.fork_guard.warn_ratio),
            ("fork_guard.kill_ratio", self.fork_guard.kill_ratio),
        ] {
            if !(0.0..=1.0).contains(&r) {
                w.push(format!("{name} = {r} is not a fraction between 0 and 1"));
            }
        }

        let h = &self.hysteresis;
        if h.exit_band.is_nan() || !(0.0..=0.5).contains(&h.exit_band) {
            w.push(format!(
                "hysteresis.exit_band = {} is outside 0 to 0.5; {} is used",
                h.exit_band,
                h.band()
            ));
        }
        if h.escalate_samples == 0 {
            w.push("hysteresis.escalate_samples is 0; 1 is used".to_string());
        }
        if h.escalate_samples > MAX_ESCALATE_SAMPLES {
            w.push(format!(
                "hysteresis.escalate_samples = {} would keep the level from ever rising; {} is used",
                h.escalate_samples, MAX_ESCALATE_SAMPLES
            ));
        }
        if h.deescalate_secs > MAX_DEESCALATE_SECS {
            w.push(format!(
                "hysteresis.deescalate_secs = {} would hold a level for hours; {} (one hour) is used",
                h.deescalate_secs, MAX_DEESCALATE_SECS
            ));
        }

        // A misspelled word here does not fail: the gate just never fires.
        one_of(&mut w, "prompt_gate.mode", &self.prompt_gate.mode, PROMPT_MODES);
        one_of(&mut w, "prompt_gate.block_on", &self.prompt_gate.block_on, PRESSURE_CUTOFFS);
        one_of(
            &mut w,
            "tool_gate.deny_subagents_on",
            &self.tool_gate.deny_subagents_on,
            PRESSURE_CUTOFFS,
        );
        for (client, mode) in &self.prompt_gate.clients {
            if !CLIENTS.contains(&client.as_str()) {
                w.push(format!(
                    "prompt_gate.clients.{client} is not a known client ({}) and is ignored",
                    CLIENTS.join(", ")
                ));
            }
            one_of(&mut w, &format!("prompt_gate.clients.{client}"), mode, PROMPT_MODES);
        }

        let b = &self.session_budget;
        if b.max_cursor_rss_megabytes > 0 && b.warn_cursor_rss_megabytes > b.max_cursor_rss_megabytes {
            w.push(format!(
                "session_budget.warn_cursor_rss_megabytes ({}) is above max_cursor_rss_megabytes ({})",
                b.warn_cursor_rss_megabytes, b.max_cursor_rss_megabytes
            ));
        }
        w
    }
}

fn one_of(out: &mut Vec<String>, name: &str, value: &str, allowed: &[&str]) {
    if !allowed.contains(&value) {
        out.push(format!(
            "{name} = \"{value}\" is not one of {}; the setting has no effect",
            allowed.join(", ")
        ));
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

    // --- parse_config: problems the user must hear about ---

    /// The default config written by scripts/install.sh.
    fn installer_default_config() -> String {
        let script = include_str!("../scripts/install.sh");
        let start = script.find("<< 'TOML'\n").expect("heredoc start") + "<< 'TOML'\n".len();
        let end = script[start..].find("\nTOML\n").expect("heredoc end") + start;
        script[start..end].to_string()
    }

    #[test]
    fn installer_default_config_is_clean() {
        let (cfg, warnings) = parse_config(&installer_default_config()).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cfg.hysteresis.deescalate_secs, 30);
    }

    #[test]
    fn empty_config_is_clean_defaults() {
        let (cfg, warnings) = parse_config("").unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cfg.sample_interval_secs, 2);
    }

    #[test]
    fn every_optional_key_is_known() {
        let text = "[thresholds]\nstrained_memory_available_ratio = 0.15\n\
                    critical_memory_available_ratio = 0.08\n\
                    [session_budget]\nmax_active_sessions = 4\nwarn_active_sessions = 2\n\
                    [prompt_gate.clients]\nclaude = \"advisory\"\n";
        let (_, warnings) = parse_config(text).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn misspelled_key_is_reported_with_a_suggestion() {
        let (cfg, warnings) = parse_config("[hysteresis]\nexit_bnad = 0.3\n").unwrap();
        assert_eq!(
            warnings,
            vec!["unknown setting `hysteresis.exit_bnad` is ignored (did you mean `exit_band`?)"]
        );
        // ...and the real setting keeps its default.
        assert_eq!(cfg.hysteresis.exit_band, 0.10);
    }

    #[test]
    fn unknown_section_is_reported() {
        let (_, warnings) = parse_config("[treshholds]\ncritical_cpu_percent = 99\n").unwrap();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("`treshholds`"), "{warnings:?}");
        assert!(warnings[0].contains("did you mean `thresholds`"), "{warnings:?}");
    }

    #[test]
    fn syntax_error_is_an_error_on_one_line() {
        let err = parse_config("[thresholds\ncritical_cpu_percent = 99\n").unwrap_err();
        assert!(!err.contains('\n'), "{err}");
        assert!(!err.is_empty());
    }

    #[test]
    fn wrong_type_is_an_error() {
        assert!(parse_config("[thresholds]\ncritical_cpu_percent = \"high\"\n").is_err());
    }

    #[test]
    fn misspelled_gate_word_is_reported() {
        let (_, warnings) =
            parse_config("[tool_gate]\ndeny_subagents_on = \"critcal\"\n").unwrap();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("tool_gate.deny_subagents_on = \"critcal\""), "{warnings:?}");
    }

    #[test]
    fn unknown_client_and_mode_are_reported() {
        let (_, warnings) =
            parse_config("[prompt_gate.clients]\nclade = \"strct\"\n").unwrap();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
    }

    #[test]
    fn inverted_thresholds_are_reported() {
        let text = "[thresholds]\nstrained_cpu_percent = 95\ncritical_cpu_percent = 90\n\
                    strained_memory_gb = 0.5\ncritical_memory_gb = 1.0\n";
        let (_, warnings) = parse_config(text).unwrap();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("strained_cpu_percent (95) should be below"), "{warnings:?}");
        assert!(warnings[1].contains("should be above critical_memory_gb"), "{warnings:?}");
    }

    #[test]
    fn out_of_range_values_are_reported() {
        let text = "sample_interval_secs = 0\n[thresholds]\ncritical_swap_percent = 900\n\
                    [hysteresis]\nexit_band = 3\n";
        let (cfg, warnings) = parse_config(text).unwrap();
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert_eq!(cfg.sample_interval(), 1);
    }

    #[test]
    fn runaway_timing_values_are_reported() {
        let text = "sample_interval_secs = 999999999\n[hysteresis]\nescalate_samples = 5000\ndeescalate_secs = 300000\n";
        let (_, warnings) = parse_config(text).unwrap();
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(warnings[0].contains("30 is used"), "{warnings:?}");
    }

    #[test]
    fn edit_distance_basics() {
        assert_eq!(edit_distance("exit_band", "exit_band"), 0);
        assert_eq!(edit_distance("exit_bnad", "exit_band"), 2);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(closest("dokcer", ["docker", "disk"].into_iter()), Some("docker"));
        assert_eq!(closest("zzzzzz", ["docker", "disk"].into_iter()), None);
    }
}
