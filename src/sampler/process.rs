use std::mem;
use std::process::Command;

#[derive(Clone, Debug)]
pub struct ProcessInfo {
    pub total_count: u32,
    pub max_proc_per_uid: u32,
    pub cursor_process_count: u32,
}

impl ProcessInfo {
    pub fn sample() -> Option<Self> {
        let total = count_user_processes()?;
        let max = max_proc_per_uid()?;
        let cursor = count_processes_by_name("Cursor");

        Some(Self {
            total_count: total,
            max_proc_per_uid: max,
            cursor_process_count: cursor,
        })
    }

    pub fn usage_ratio(&self) -> f64 {
        if self.max_proc_per_uid == 0 {
            return 0.0;
        }
        self.total_count as f64 / self.max_proc_per_uid as f64
    }
}

fn count_user_processes() -> Option<u32> {
    let uid = unsafe { libc::getuid() };
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_UID, uid as i32];
    let mut size: usize = 0;

    let ret = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            4,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret != 0 || size == 0 {
        return None;
    }

    // kinfo_proc is 648 bytes on macOS arm64 / 648 on x86_64
    const KINFO_PROC_SIZE: usize = 648;
    let count = size / KINFO_PROC_SIZE;
    Some(count as u32)
}

fn max_proc_per_uid() -> Option<u32> {
    let mut value: i32 = 0;
    let mut len = mem::size_of::<i32>();
    let name = b"kern.maxprocperuid\0";
    let ret = unsafe {
        libc::sysctlbyname(
            name.as_ptr() as *const libc::c_char,
            &mut value as *mut i32 as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret == 0 {
        Some(value as u32)
    } else {
        None
    }
}

/// Count processes matching a name prefix using `ps` command.
/// More reliable than parsing raw kinfo_proc structs.
/// Sum resident memory for macOS `ps` lines where the command starts with `Cursor` (kilobytes → megabytes).
pub fn cursor_rss_megabytes() -> u64 {
    let output = Command::new("ps").args(["-axo", "rss,ucomm"]).output();
    let Ok(output) = output else {
        return 0;
    };
    if !output.status.success() {
        return 0;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut kb_total: u64 = 0;
    for line in text.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.splitn(2, |c: char| c.is_whitespace());
        let rss_part = it.next().unwrap_or("");
        let comm_part = it.next().unwrap_or("").trim_start();
        if !comm_part.starts_with("Cursor") {
            continue;
        }
        kb_total += rss_part.parse::<u64>().unwrap_or(0);
    }
    kb_total / 1024
}

fn count_processes_by_name(name_prefix: &str) -> u32 {
    let output = Command::new("ps")
        .args(["-u", &unsafe { libc::getuid() }.to_string(), "-o", "comm="])
        .output()
        .ok();

    let Some(output) = output else { return 0 };
    let text = String::from_utf8_lossy(&output.stdout);

    text.lines()
        .filter(|line| {
            let basename = line.rsplit('/').next().unwrap_or(line).trim();
            basename.starts_with(name_prefix)
        })
        .count() as u32
}
