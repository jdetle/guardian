use std::mem;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XswUsage {
    xsu_total: u64,
    xsu_avail: u64,
    xsu_used: u64,
    xsu_pagesize: u32,
    xsu_encrypted: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct SwapInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
}

impl SwapInfo {
    pub fn sample() -> Option<Self> {
        let mut info = XswUsage::default();
        let mut len = mem::size_of::<XswUsage>();
        let name = b"vm.swapusage\0";
        let ret = unsafe {
            libc::sysctlbyname(
                name.as_ptr() as *const libc::c_char,
                &mut info as *mut XswUsage as *mut libc::c_void,
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if ret != 0 {
            return None;
        }
        Some(Self {
            total_bytes: info.xsu_total,
            used_bytes: info.xsu_used,
            available_bytes: info.xsu_avail,
        })
    }

    pub fn used_percent(&self) -> f64 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        (self.used_bytes as f64 / self.total_bytes as f64) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_total_returns_zero_percent() {
        let info = SwapInfo {
            total_bytes: 0,
            used_bytes: 0,
            available_bytes: 0,
        };
        assert_eq!(info.used_percent(), 0.0);
    }

    #[test]
    fn zero_total_with_nonzero_used_returns_zero() {
        let info = SwapInfo {
            total_bytes: 0,
            used_bytes: 1024,
            available_bytes: 0,
        };
        assert_eq!(info.used_percent(), 0.0);
    }

    #[test]
    fn half_used_returns_fifty_percent() {
        let info = SwapInfo {
            total_bytes: 1024,
            used_bytes: 512,
            available_bytes: 512,
        };
        assert!((info.used_percent() - 50.0).abs() < 0.001);
    }

    #[test]
    fn fully_used_returns_hundred_percent() {
        let info = SwapInfo {
            total_bytes: 4096,
            used_bytes: 4096,
            available_bytes: 0,
        };
        assert!((info.used_percent() - 100.0).abs() < 0.001);
    }
}
