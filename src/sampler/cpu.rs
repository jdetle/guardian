use mach2::kern_return::KERN_SUCCESS;
use mach2::mach_types::host_t;
use mach2::vm_types::natural_t;
use std::mem;

const HOST_CPU_LOAD_INFO: i32 = 3;
const CPU_STATE_USER: usize = 0;
const CPU_STATE_SYSTEM: usize = 1;
const CPU_STATE_IDLE: usize = 2;
const CPU_STATE_NICE: usize = 3;
const CPU_STATE_MAX: usize = 4;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct HostCpuLoadInfo {
    ticks: [natural_t; CPU_STATE_MAX],
}

extern "C" {
    fn mach_host_self() -> host_t;
    fn host_statistics(
        host: host_t,
        flavor: i32,
        host_info: *mut HostCpuLoadInfo,
        count: *mut u32,
    ) -> i32;
}

#[derive(Clone, Copy, Default)]
pub struct CpuSnapshot {
    pub user: u64,
    pub system: u64,
    pub idle: u64,
    pub nice: u64,
}

impl CpuSnapshot {
    pub fn sample() -> Option<Self> {
        unsafe {
            let host = mach_host_self();
            let mut info = HostCpuLoadInfo::default();
            let mut count = (mem::size_of::<HostCpuLoadInfo>() / mem::size_of::<natural_t>()) as u32;

            let ret = host_statistics(host, HOST_CPU_LOAD_INFO, &mut info, &mut count);
            if ret != KERN_SUCCESS {
                return None;
            }

            Some(Self {
                user: info.ticks[CPU_STATE_USER] as u64,
                system: info.ticks[CPU_STATE_SYSTEM] as u64,
                idle: info.ticks[CPU_STATE_IDLE] as u64,
                nice: info.ticks[CPU_STATE_NICE] as u64,
            })
        }
    }

    pub fn total(&self) -> u64 {
        self.user + self.system + self.idle + self.nice
    }

    pub fn active(&self) -> u64 {
        self.user + self.system + self.nice
    }
}

pub fn cpu_usage_percent(prev: &CpuSnapshot, curr: &CpuSnapshot) -> f64 {
    let total_delta = curr.total().saturating_sub(prev.total());
    if total_delta == 0 {
        return 0.0;
    }
    let active_delta = curr.active().saturating_sub(prev.active());
    (active_delta as f64 / total_delta as f64) * 100.0
}
