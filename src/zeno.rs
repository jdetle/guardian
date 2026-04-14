//! Zeno relaxation: each step moves percentage-based limits halfway toward 100% usage
//! (or memory free/ratio thresholds halfway toward zero), matching repeated `(x + 100) / 2`
//! on percent-style caps.

use crate::config::{
    DiskConfig, GuardianConfig, SessionBudgetConfig, ThresholdConfig,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;

const ZENO_JSON: &str = "zeno.json";

/// Upper bound for relaxing Cursor RSS caps (256 GiB).
pub const ZENO_SESSION_RSS_CAP_MB: u64 = 262_144;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ZenoState {
    /// Number of bump steps applied (each bump increments by 1).
    #[serde(default)]
    pub steps: u32,
}

pub fn zeno_path() -> std::path::PathBuf {
    crate::config::guardian_dir().join(ZENO_JSON)
}

pub fn load_zeno() -> Option<ZenoState> {
    let p = zeno_path();
    let s = fs::read_to_string(p).ok()?;
    serde_json::from_str(&s).ok()
}

pub fn load_zeno_steps() -> u32 {
    load_zeno().map(|z| z.steps).unwrap_or(0)
}

pub fn save_zeno(state: &ZenoState) -> std::io::Result<()> {
    let dir = crate::config::guardian_dir();
    fs::create_dir_all(&dir)?;
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let tmp = dir.join("zeno.json.tmp");
    let final_path = dir.join(ZENO_JSON);
    let mut f = fs::File::create(&tmp)?;
    f.write_all(json.as_bytes())?;
    f.sync_all()?;
    fs::rename(tmp, final_path)?;
    Ok(())
}

/// For thresholds where **higher numeric value = more permissive** (CPU %, swap used %, disk used %).
/// `base` is in 0–100. Result moves from `base` toward 100 by halving the remaining gap each step.
pub fn relax_percent_toward_100(base: f64, steps: u32) -> f64 {
    if steps == 0 {
        return base;
    }
    let factor = 2.0_f64.powi(steps.min(31) as i32);
    let gap = (100.0 - base) / factor;
    (100.0 - gap).clamp(0.0, 100.0)
}

/// For thresholds where **lower numeric value = more permissive** (GiB free before strain, ratio floors).
pub fn relax_toward_zero(base: f64, steps: u32) -> f64 {
    if steps == 0 {
        return base;
    }
    let factor = 2.0_f64.powi(steps.min(63) as i32);
    (base / factor).max(0.0)
}

fn relax_mb_toward_cap(base: u64, cap: u64, steps: u32) -> u64 {
    if steps == 0 || base >= cap {
        return base;
    }
    let gap = cap - base;
    let factor = 2u128.pow(steps.min(31));
    let delta = (gap as u128 * (factor - 1)) / factor;
    base.saturating_add(delta as u64).min(cap)
}

/// Apply zeno steps on top of `config.toml` — used by guardiand and the `guardian zeno status` command.
pub fn apply_zeno(cfg: &GuardianConfig, steps: u32) -> GuardianConfig {
    let mut out = cfg.clone();
    out.thresholds = apply_zeno_thresholds(&cfg.thresholds, steps);
    out.disk = apply_zeno_disk(&cfg.disk, steps);
    out.session_budget = apply_zeno_session_budget(&cfg.session_budget, steps);
    out
}

fn apply_zeno_thresholds(t: &ThresholdConfig, steps: u32) -> ThresholdConfig {
    let mut n = t.clone();
    n.strained_cpu_percent = relax_percent_toward_100(t.strained_cpu_percent, steps);
    n.critical_cpu_percent = relax_percent_toward_100(t.critical_cpu_percent, steps);
    n.strained_memory_gb = relax_toward_zero(t.strained_memory_gb, steps);
    n.critical_memory_gb = relax_toward_zero(t.critical_memory_gb, steps);
    n.strained_swap_percent = relax_percent_toward_100(t.strained_swap_percent, steps);
    n.critical_swap_percent = relax_percent_toward_100(t.critical_swap_percent, steps);
    n.strained_memory_available_ratio = t
        .strained_memory_available_ratio
        .map(|x| relax_toward_zero(x, steps));
    n.critical_memory_available_ratio = t
        .critical_memory_available_ratio
        .map(|x| relax_toward_zero(x, steps));
    n
}

fn apply_zeno_disk(d: &DiskConfig, steps: u32) -> DiskConfig {
    let mut out = d.clone();
    out.warn_used_percent = relax_percent_toward_100(d.warn_used_percent, steps);
    out.critical_used_percent = relax_percent_toward_100(d.critical_used_percent, steps);
    if out.critical_used_percent <= out.warn_used_percent {
        out.critical_used_percent = (out.warn_used_percent + 1.0).min(100.0);
    }
    out
}

fn apply_zeno_session_budget(s: &SessionBudgetConfig, steps: u32) -> SessionBudgetConfig {
    let mut out = s.clone();
    if out.warn_cursor_rss_megabytes > out.max_cursor_rss_megabytes {
        out.warn_cursor_rss_megabytes = out.max_cursor_rss_megabytes;
    }
    let cap = ZENO_SESSION_RSS_CAP_MB;
    out.max_cursor_rss_megabytes = relax_mb_toward_cap(out.max_cursor_rss_megabytes, cap, steps);
    out.warn_cursor_rss_megabytes =
        relax_mb_toward_cap(out.warn_cursor_rss_megabytes, cap, steps);
    if out.warn_cursor_rss_megabytes > out.max_cursor_rss_megabytes {
        out.warn_cursor_rss_megabytes = out.max_cursor_rss_megabytes;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_sequence_matches_halfway_to_100() {
        let x1 = relax_percent_toward_100(95.0, 1);
        assert!((x1 - 97.5).abs() < 1e-9);
        let x2 = relax_percent_toward_100(95.0, 2);
        assert!((x2 - 98.75).abs() < 1e-9);
    }

    #[test]
    fn zero_steps_is_identity() {
        assert_eq!(relax_percent_toward_100(70.0, 0), 70.0);
        assert_eq!(relax_toward_zero(2.0, 0), 2.0);
    }

    #[test]
    fn session_warn_stays_lte_max() {
        let mut sb = SessionBudgetConfig::default();
        sb.max_cursor_rss_megabytes = 8192;
        sb.warn_cursor_rss_megabytes = 9000; // pathological
        let fixed = apply_zeno_session_budget(&sb, 0);
        assert_eq!(fixed.warn_cursor_rss_megabytes, 8192);
    }
}
