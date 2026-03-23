use crate::config::guardian_dir;
use std::fs;

/// Stamp this platform as validated. Writes a validation record to
/// ~/.guardian/validations.json with the machine fingerprint.
pub fn stamp_platform() {
    let fingerprint = PlatformFingerprint::collect();
    eprintln!("[guardiand] platform: {} {} {}", fingerprint.chip, fingerprint.os_version, fingerprint.arch);

    let path = guardian_dir().join("validations.json");
    let mut records: Vec<ValidationRecord> = if path.exists() {
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let key = fingerprint.key();
    let now = chrono::Utc::now().to_rfc3339();

    if let Some(existing) = records.iter_mut().find(|r| r.key == key) {
        existing.last_validated = now.clone();
        existing.validation_count += 1;
        existing.daemon_version = env!("CARGO_PKG_VERSION").to_string();
    } else {
        records.push(ValidationRecord {
            key: key.clone(),
            fingerprint: fingerprint.clone(),
            first_validated: now.clone(),
            last_validated: now,
            validation_count: 1,
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            chaos_tested: false,
            chaos_level_passed: 0,
        });
    }

    if let Ok(json) = serde_json::to_string_pretty(&records) {
        let _ = fs::write(&path, json);
    }
}

pub fn mark_chaos_tested(level: u32) {
    let path = guardian_dir().join("validations.json");
    let fingerprint = PlatformFingerprint::collect();
    let key = fingerprint.key();

    let mut records: Vec<ValidationRecord> = if path.exists() {
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    if let Some(existing) = records.iter_mut().find(|r| r.key == key) {
        existing.chaos_tested = true;
        if level > existing.chaos_level_passed {
            existing.chaos_level_passed = level;
        }
    }

    if let Ok(json) = serde_json::to_string_pretty(&records) {
        let _ = fs::write(&path, json);
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PlatformFingerprint {
    pub chip: String,
    pub arch: String,
    pub os_version: String,
    pub os_build: String,
    pub physical_cpus: u32,
    pub logical_cpus: u32,
    pub perf_cores: u32,
    pub efficiency_cores: u32,
    pub memory_gb: u32,
    pub max_proc_per_uid: u32,
    pub model_identifier: String,
    pub kernel_version: String,
    pub docker_arch: String,
}

impl PlatformFingerprint {
    pub fn collect() -> Self {
        Self {
            chip: sysctl_string("machdep.cpu.brand_string"),
            arch: std::env::consts::ARCH.to_string(),
            os_version: run_cmd("sw_vers", &["-productVersion"]),
            os_build: run_cmd("sw_vers", &["-buildVersion"]),
            physical_cpus: sysctl_u32("hw.physicalcpu"),
            logical_cpus: sysctl_u32("hw.logicalcpu"),
            perf_cores: sysctl_u32("hw.perflevel0.physicalcpu"),
            efficiency_cores: sysctl_u32("hw.perflevel1.physicalcpu"),
            memory_gb: (sysctl_u64("hw.memsize") / (1024 * 1024 * 1024)) as u32,
            max_proc_per_uid: sysctl_u32("kern.maxprocperuid"),
            model_identifier: sysctl_string("hw.model"),
            kernel_version: sysctl_string("kern.osrelease"),
            docker_arch: run_cmd("docker", &["version", "--format", "{{.Server.Arch}}"]),
        }
    }

    pub fn key(&self) -> String {
        format!(
            "{}_{}_{}_{}gb",
            self.model_identifier, self.os_build, self.arch, self.memory_gb
        )
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ValidationRecord {
    key: String,
    fingerprint: PlatformFingerprint,
    first_validated: String,
    last_validated: String,
    validation_count: u64,
    daemon_version: String,
    chaos_tested: bool,
    chaos_level_passed: u32,
}

fn sysctl_string(name: &str) -> String {
    let name_c = format!("{name}\0");
    let mut buf = vec![0u8; 256];
    let mut len = buf.len();
    let ret = unsafe {
        libc::sysctlbyname(
            name_c.as_ptr() as *const libc::c_char,
            buf.as_mut_ptr() as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret != 0 || len == 0 {
        return "unknown".to_string();
    }
    String::from_utf8_lossy(&buf[..len.saturating_sub(1)])
        .trim()
        .to_string()
}

fn sysctl_u32(name: &str) -> u32 {
    let name_c = format!("{name}\0");
    let mut value: u32 = 0;
    let mut len = std::mem::size_of::<u32>();
    let ret = unsafe {
        libc::sysctlbyname(
            name_c.as_ptr() as *const libc::c_char,
            &mut value as *mut u32 as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret == 0 { value } else { 0 }
}

fn sysctl_u64(name: &str) -> u64 {
    let name_c = format!("{name}\0");
    let mut value: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let ret = unsafe {
        libc::sysctlbyname(
            name_c.as_ptr() as *const libc::c_char,
            &mut value as *mut u64 as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret == 0 { value } else { 0 }
}

fn run_cmd(program: &str, args: &[&str]) -> String {
    std::process::Command::new(program)
        .args(args)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
