//! Home volume disk usage via POSIX `statvfs`.

use crate::config::DiskConfig;
use crate::state::{DiskPressureLevel, DiskState};
use libc::{c_char, statvfs};
use std::ffi::CString;
use std::path::Path;

/// Returns used percent (0–100) from total and available byte counts.
pub fn used_percent_from_bytes(total_bytes: u64, available_bytes: u64) -> f64 {
    if total_bytes == 0 {
        return 0.0;
    }
    let used = total_bytes.saturating_sub(available_bytes);
    (100.0_f64 * used as f64 / total_bytes as f64).min(100.0).max(0.0)
}

pub fn disk_level(used_percent: f64, cfg: &DiskConfig) -> DiskPressureLevel {
    if !cfg.enabled {
        return DiskPressureLevel::Clear;
    }
    let warn = cfg.warn_used_percent;
    let mut crit = cfg.critical_used_percent;
    if crit <= warn {
        crit = warn + 1.0;
    }
    if used_percent >= crit {
        DiskPressureLevel::Critical
    } else if used_percent >= warn {
        DiskPressureLevel::Warn
    } else {
        DiskPressureLevel::Clear
    }
}

fn statvfs_for_path(path: &Path) -> Option<libc::statvfs> {
    let s = path.to_str()?;
    let c = CString::new(s).ok()?;
    let mut vfs: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { statvfs(c.as_ptr() as *const c_char, &mut vfs) };
    if rc != 0 {
        return None;
    }
    Some(vfs)
}

/// Sample the volume containing `path` (typically the home directory).
pub fn sample_volume_containing(path: &Path) -> Option<(String, f64, f64, f64)> {
    let vfs = statvfs_for_path(path)?;
    let frsize = vfs.f_frsize as u64;
    let blocks = vfs.f_blocks as u64;
    let bavail = vfs.f_bavail as u64;
    if frsize == 0 || blocks == 0 {
        return None;
    }
    let total_bytes = blocks.saturating_mul(frsize);
    let available_bytes = bavail.saturating_mul(frsize);
    let used_pct = used_percent_from_bytes(total_bytes, available_bytes);
    let total_gb = total_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    let available_gb = available_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    let volume_path = path.to_string_lossy().to_string();
    Some((volume_path, total_gb, available_gb, used_pct))
}

/// Build [`DiskState`] for `state.json` from config (fail-open to clear on error).
pub fn compute_disk_state(cfg: &DiskConfig) -> DiskState {
    if !cfg.enabled {
        return DiskState::default();
    }
    let Some(home) = dirs::home_dir() else {
        return DiskState::default();
    };
    let Some((volume_path, total_gb, available_gb, used_pct)) =
        sample_volume_containing(&home)
    else {
        return DiskState::default();
    };
    let level = disk_level(used_pct, cfg);
    DiskState {
        volume_path,
        total_gb: (total_gb * 100.0).round() / 100.0,
        available_gb: (available_gb * 100.0).round() / 100.0,
        used_percent: (used_pct * 10.0).round() / 10.0,
        level,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn used_percent_quarter_free() {
        let total = 1000_u64 * 4096;
        let avail = 250_u64 * 4096;
        let pct = used_percent_from_bytes(total, avail);
        assert!((pct - 75.0).abs() < 0.0001, "got {}", pct);
    }

    #[test]
    fn used_percent_empty_total() {
        assert_eq!(used_percent_from_bytes(0, 0), 0.0);
    }

    #[test]
    fn used_percent_full() {
        let total = 100_u64 * 512;
        assert!((used_percent_from_bytes(total, 0) - 100.0).abs() < 0.0001);
    }

    #[test]
    fn disk_level_respects_thresholds() {
        let cfg = DiskConfig {
            enabled: true,
            warn_used_percent: 85.0,
            critical_used_percent: 93.0,
        };
        assert_eq!(disk_level(50.0, &cfg), DiskPressureLevel::Clear);
        assert_eq!(disk_level(85.0, &cfg), DiskPressureLevel::Warn);
        assert_eq!(disk_level(92.9, &cfg), DiskPressureLevel::Warn);
        assert_eq!(disk_level(93.0, &cfg), DiskPressureLevel::Critical);
    }

    #[test]
    fn disk_level_disabled() {
        let cfg = DiskConfig {
            enabled: false,
            warn_used_percent: 0.0,
            critical_used_percent: 100.0,
        };
        assert_eq!(disk_level(99.0, &cfg), DiskPressureLevel::Clear);
    }

    #[test]
    fn disk_level_invalid_critical_clamps_above_warn() {
        let cfg = DiskConfig {
            enabled: true,
            warn_used_percent: 90.0,
            critical_used_percent: 85.0,
        };
        // critical coerced to 91; 89% is still clear
        assert_eq!(disk_level(89.0, &cfg), DiskPressureLevel::Clear);
        assert_eq!(disk_level(90.0, &cfg), DiskPressureLevel::Warn);
        assert_eq!(disk_level(91.0, &cfg), DiskPressureLevel::Critical);
    }
}
