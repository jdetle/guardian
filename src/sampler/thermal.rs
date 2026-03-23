use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ThermalState {
    Nominal,
    Fair,
    Serious,
    Critical,
    Unknown,
}

impl ThermalState {
    /// Sample thermal pressure from macOS `thermal_pressure` sysctl.
    /// Returns Unknown if the API is unavailable.
    pub fn sample() -> Self {
        let mut value: i32 = 0;
        let mut len = std::mem::size_of::<i32>();
        let name = b"machdep.xcpm.thermal_pressure\0";
        let ret = unsafe {
            libc::sysctlbyname(
                name.as_ptr() as *const libc::c_char,
                &mut value as *mut i32 as *mut libc::c_void,
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };

        if ret != 0 {
            return Self::from_pmset();
        }

        match value {
            0 => Self::Nominal,
            1 => Self::Fair,
            2 => Self::Serious,
            3 => Self::Critical,
            _ => Self::Unknown,
        }
    }

    /// Fallback: parse `pmset -g therm` output
    fn from_pmset() -> Self {
        let output = Command::new("pmset")
            .args(["-g", "therm"])
            .output()
            .ok();

        let Some(output) = output else {
            return Self::Unknown;
        };

        let text = String::from_utf8_lossy(&output.stdout);
        if text.contains("CPU_Scheduler_Limit") {
            for line in text.lines() {
                if let Some(val) = line.strip_prefix("CPU_Speed_Limit") {
                    let val = val.trim().trim_start_matches('=').trim();
                    if let Ok(pct) = val.parse::<u32>() {
                        return match pct {
                            90..=100 => Self::Nominal,
                            70..=89 => Self::Fair,
                            50..=69 => Self::Serious,
                            _ => Self::Critical,
                        };
                    }
                }
            }
        }

        Self::Unknown
    }
}

impl std::fmt::Display for ThermalState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nominal => write!(f, "nominal"),
            Self::Fair => write!(f, "fair"),
            Self::Serious => write!(f, "serious"),
            Self::Critical => write!(f, "critical"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}
