use mach2::kern_return::KERN_SUCCESS;
use mach2::mach_types::host_t;
use mach2::vm_types::natural_t;
use std::mem;

const HOST_VM_INFO: i32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VmStatistics {
    free_count: natural_t,
    active_count: natural_t,
    inactive_count: natural_t,
    wire_count: natural_t,
    zero_fill_count: natural_t,
    reactivations: natural_t,
    pageins: natural_t,
    pageouts: natural_t,
    faults: natural_t,
    cow_faults: natural_t,
    lookups: natural_t,
    hits: natural_t,
    purgeable_count: natural_t,
    purges: natural_t,
    speculative_count: natural_t,
}

#[allow(clashing_extern_declarations)]
extern "C" {
    fn mach_host_self() -> host_t;
    #[link_name = "host_statistics"]
    fn host_statistics_vm(
        host: host_t,
        flavor: i32,
        host_info: *mut VmStatistics,
        count: *mut u32,
    ) -> i32;
    fn host_page_size(host: host_t, page_size: *mut u32) -> i32;
}

#[derive(Clone, Copy, Debug)]
pub struct MemoryInfo {
    pub free_bytes: u64,
    pub active_bytes: u64,
    pub inactive_bytes: u64,
    pub wired_bytes: u64,
    pub total_bytes: u64,
    pub speculative_bytes: u64,
}

impl MemoryInfo {
    pub fn sample() -> Option<Self> {
        unsafe {
            let host = mach_host_self();

            let mut page_size: u32 = 0;
            if host_page_size(host, &mut page_size) != KERN_SUCCESS {
                return None;
            }

            let mut info = VmStatistics::default();
            let mut count =
                (mem::size_of::<VmStatistics>() / mem::size_of::<natural_t>()) as u32;

            let ret = host_statistics_vm(host, HOST_VM_INFO, &mut info, &mut count);
            if ret != KERN_SUCCESS {
                return None;
            }

            let ps = page_size as u64;
            let free = info.free_count as u64 * ps;
            let active = info.active_count as u64 * ps;
            let inactive = info.inactive_count as u64 * ps;
            let wired = info.wire_count as u64 * ps;
            let speculative = info.speculative_count as u64 * ps;
            let total = total_physical_memory()?;

            Some(Self {
                free_bytes: free,
                active_bytes: active,
                inactive_bytes: inactive,
                wired_bytes: wired,
                total_bytes: total,
                speculative_bytes: speculative,
            })
        }
    }

    pub fn free_gb(&self) -> f64 {
        self.free_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
    }

    pub fn total_gb(&self) -> f64 {
        self.total_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
    }

    pub fn available_bytes(&self) -> u64 {
        self.free_bytes + self.inactive_bytes + self.speculative_bytes
    }

    pub fn available_gb(&self) -> f64 {
        self.available_bytes() as f64 / (1024.0 * 1024.0 * 1024.0)
    }
}

fn total_physical_memory() -> Option<u64> {
    let mut size: u64 = 0;
    let mut len = mem::size_of::<u64>();
    let name = b"hw.memsize\0";
    let ret = unsafe {
        libc::sysctlbyname(
            name.as_ptr() as *const libc::c_char,
            &mut size as *mut u64 as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret == 0 {
        Some(size)
    } else {
        None
    }
}

/// The kernel's own memory-pressure verdict — the same signal behind Activity Monitor's
/// memory-pressure graph. Unlike "free pages" or "swap used %", it accounts for compression,
/// purgeable memory and how hard the VM system is actually working.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryPressureLevel {
    Normal,
    Warn,
    Critical,
    /// The sysctl is unavailable or returned a value we do not know.
    Unknown,
}

impl MemoryPressureLevel {
    /// `kern.memorystatus_vm_pressure_level`: 1 = normal, 2 = warn, 4 = critical.
    pub fn from_raw(value: u32) -> Self {
        match value {
            1 => Self::Normal,
            2 => Self::Warn,
            4 => Self::Critical,
            _ => Self::Unknown,
        }
    }

    pub fn sample() -> Self {
        let mut value: u32 = 0;
        let mut len = mem::size_of::<u32>();
        let name = b"kern.memorystatus_vm_pressure_level\0";
        let ret = unsafe {
            libc::sysctlbyname(
                name.as_ptr() as *const libc::c_char,
                &mut value as *mut u32 as *mut libc::c_void,
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if ret != 0 || len != mem::size_of::<u32>() {
            return Self::Unknown;
        }
        Self::from_raw(value)
    }
}

impl std::fmt::Display for MemoryPressureLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Normal => write!(f, "normal"),
            Self::Warn => write!(f, "warn"),
            Self::Critical => write!(f, "critical"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

#[cfg(test)]
mod pressure_level_tests {
    use super::*;

    #[test]
    fn maps_kernel_values() {
        assert_eq!(MemoryPressureLevel::from_raw(1), MemoryPressureLevel::Normal);
        assert_eq!(MemoryPressureLevel::from_raw(2), MemoryPressureLevel::Warn);
        assert_eq!(MemoryPressureLevel::from_raw(4), MemoryPressureLevel::Critical);
    }

    #[test]
    fn unexpected_values_are_unknown_not_normal() {
        for v in [0u32, 3, 5, 8, u32::MAX] {
            assert_eq!(MemoryPressureLevel::from_raw(v), MemoryPressureLevel::Unknown);
        }
    }

    #[test]
    fn sampling_never_panics_and_is_known_on_macos() {
        // The sysctl exists on every supported macOS; a real machine reports a known level.
        assert_ne!(MemoryPressureLevel::sample(), MemoryPressureLevel::Unknown);
    }
}
