use crate::classifier::PressureLevel;
use crate::config::guardian_dir;
use crate::sampler::thermal::ThermalState;
use serde::Serialize;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

#[derive(Serialize, Clone, Debug)]
pub struct GuardianState {
    pub pressure: PressureLevel,
    pub cpu_percent: f64,
    pub memory_available_gb: f64,
    pub memory_total_gb: f64,
    pub swap_used_percent: f64,
    pub thermal_state: ThermalState,
    pub docker: DockerState,
    pub cursor: CursorState,
    pub process_count: u32,
    pub max_proc_per_uid: u32,
    pub sampled_at: String,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct DockerState {
    pub running_containers: u32,
    pub total_cpu_percent: f64,
    pub total_memory_mb: u64,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct CursorState {
    pub active_sessions: u32,
    pub process_count: u32,
    /// Sum of RSS for processes whose short name starts with `Cursor` (best-effort via `ps`).
    #[serde(default)]
    pub resident_memory_megabytes: u64,
}

pub fn state_file_path() -> PathBuf {
    guardian_dir().join("state.json")
}

pub fn write_state(state: &GuardianState) -> std::io::Result<()> {
    let dir = guardian_dir();
    fs::create_dir_all(&dir)?;

    let json = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    let tmp_path = dir.join("state.json.tmp");
    let final_path = dir.join("state.json");

    // Symlink protection: remove the target if it's a symlink
    if final_path.is_symlink() {
        eprintln!("[guardiand] WARNING: state.json is a symlink — removing it");
        fs::remove_file(&final_path)?;
    }

    let mut file = fs::File::create(&tmp_path)?;
    file.write_all(json.as_bytes())?;
    file.sync_all()?;

    fs::rename(&tmp_path, &final_path)?;
    Ok(())
}
