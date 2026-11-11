use std::process::Command;
use std::sync::OnceLock;

// libnotify (part of libSystem). thermald publishes the machine's thermal pressure as the
// state of this Darwin notification; reading it needs no privileges and no subprocess.
extern "C" {
    fn notify_register_check(name: *const libc::c_char, out_token: *mut libc::c_int) -> u32;
    fn notify_get_state(token: libc::c_int, state64: *mut u64) -> u32;
}

const NOTIFY_STATUS_OK: u32 = 0;
static THERMAL_NOTIFY_TOKEN: OnceLock<Option<libc::c_int>> = OnceLock::new();

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
    /// Sample thermal pressure.
    ///
    /// 1. `machdep.xcpm.thermal_pressure` — Intel Macs only.
    /// 2. The `com.apple.system.thermalpressurelevel` notification state — Apple Silicon.
    ///    Without this, Apple Silicon machines always reported `unknown` (the sysctl does not
    ///    exist there and `pmset -g therm` prints nothing on fanless models), so Guardian was
    ///    blind to heat, and spawned `pmset` on every sample for no result.
    /// 3. `pmset -g therm` as a last resort.
    ///
    /// Returns Unknown if no source is available.
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
            return match Self::from_notify() {
                Some(state) if state != Self::Unknown => state,
                _ => Self::from_pmset(),
            };
        }

        match value {
            0 => Self::Nominal,
            1 => Self::Fair,
            2 => Self::Serious,
            3 => Self::Critical,
            _ => Self::Unknown,
        }
    }

    /// `OSThermalPressureLevel` on macOS: 0 nominal, 1 moderate, 2 heavy, 3 trapping,
    /// 4 sleeping. Anything else is Unknown rather than assumed nominal.
    pub fn from_pressure_level(level: u64) -> Self {
        match level {
            0 => Self::Nominal,
            1 => Self::Fair,
            2 => Self::Serious,
            3 | 4 => Self::Critical,
            _ => Self::Unknown,
        }
    }

    /// Read the thermal pressure level thermald publishes. `None` if libnotify refuses.
    fn from_notify() -> Option<Self> {
        let token = (*THERMAL_NOTIFY_TOKEN.get_or_init(|| {
            let name = b"com.apple.system.thermalpressurelevel\0";
            let mut token: libc::c_int = 0;
            let status = unsafe {
                notify_register_check(name.as_ptr() as *const libc::c_char, &mut token)
            };
            if status == NOTIFY_STATUS_OK {
                Some(token)
            } else {
                None
            }
        }))?;

        let mut state: u64 = 0;
        let status = unsafe { notify_get_state(token, &mut state) };
        if status != NOTIFY_STATUS_OK {
            return None;
        }
        Self::interpret_notify_state(state, running_in_vm())
    }

    /// Decide whether a notification state can be trusted.
    ///
    /// libnotify returns state 0 both for "thermald says nominal" and for a name nobody has
    /// ever posted, and offers no way to tell them apart. On physical Macs thermald publishes
    /// this level (it is what `NSProcessInfo.thermalState` reads), so 0 means nominal. In a
    /// virtual machine there is no thermal management, so a 0 is treated as "no source"
    /// (`None`) rather than a reassuring "nominal". A non-zero level is always believed.
    pub fn interpret_notify_state(state: u64, in_vm: bool) -> Option<Self> {
        if state == 0 && in_vm {
            return None;
        }
        Some(Self::from_pressure_level(state))
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

/// `kern.hv_vmm_present` is 1 inside a virtual machine. Missing sysctl (older macOS) reads as
/// "not a VM".
pub fn running_in_vm() -> bool {
    let mut value: i32 = 0;
    let mut len = std::mem::size_of::<i32>();
    let name = b"kern.hv_vmm_present\0";
    let ret = unsafe {
        libc::sysctlbyname(
            name.as_ptr() as *const libc::c_char,
            &mut value as *mut i32 as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    ret == 0 && value != 0
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_os_thermal_pressure_levels() {
        assert_eq!(ThermalState::from_pressure_level(0), ThermalState::Nominal);
        assert_eq!(ThermalState::from_pressure_level(1), ThermalState::Fair);
        assert_eq!(ThermalState::from_pressure_level(2), ThermalState::Serious);
        assert_eq!(ThermalState::from_pressure_level(3), ThermalState::Critical);
        assert_eq!(ThermalState::from_pressure_level(4), ThermalState::Critical);
    }

    #[test]
    fn unexpected_levels_are_unknown_not_nominal() {
        for v in [5u64, 10, 20, 50, u64::MAX] {
            assert_eq!(ThermalState::from_pressure_level(v), ThermalState::Unknown);
        }
    }

    #[test]
    fn zero_state_is_nominal_on_hardware_but_no_source_in_a_vm() {
        // A never-posted notification also reads 0, so 0 is only believed on physical Macs.
        assert_eq!(
            ThermalState::interpret_notify_state(0, false),
            Some(ThermalState::Nominal)
        );
        assert_eq!(ThermalState::interpret_notify_state(0, true), None);
    }

    #[test]
    fn nonzero_state_is_believed_everywhere() {
        assert_eq!(
            ThermalState::interpret_notify_state(2, true),
            Some(ThermalState::Serious)
        );
        assert_eq!(
            ThermalState::interpret_notify_state(1, false),
            Some(ThermalState::Fair)
        );
    }

    #[test]
    fn physical_macs_report_a_known_thermal_state() {
        // The regression this guards: Apple Silicon used to report Unknown forever.
        // CI runners are VMs with no thermal source, so only assert on real hardware.
        if !running_in_vm() {
            assert_ne!(ThermalState::sample(), ThermalState::Unknown);
        }
    }
}
