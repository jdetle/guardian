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

/// Which coding-agent app a process belongs to (matched on the executable path from `ps -o comm`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentFamily {
    Cursor,
    Claude,
    Codex,
}

/// Classify an executable path. Path-based so unrelated binaries that merely start with
/// "Cursor" (e.g. macOS `CursorUIViewService`) are not counted.
pub fn classify_agent(comm: &str) -> Option<AgentFamily> {
    let comm = comm.trim();
    if comm.is_empty() {
        return None;
    }
    let basename = comm.rsplit('/').next().unwrap_or(comm);
    if comm.contains("/Cursor.app/") {
        return Some(AgentFamily::Cursor);
    }
    // The native Claude Code CLI runs as `~/.local/share/claude/versions/<version>` (the
    // `claude` on PATH is a symlink to it), so its basename is a version number.
    if comm.contains("/Claude.app/")
        || comm.contains("/claude-code/")
        || comm.contains("/claude/versions/")
        || basename == "claude"
    {
        return Some(AgentFamily::Claude);
    }
    if comm.contains("/Codex.app/") || basename == "codex" || basename.starts_with("codex-") {
        return Some(AgentFamily::Codex);
    }
    None
}

/// Resident memory (MB) summed per agent family.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AgentRss {
    pub cursor_mb: u64,
    pub claude_mb: u64,
    pub codex_mb: u64,
}

impl AgentRss {
    pub fn total_mb(&self) -> u64 {
        self.cursor_mb + self.claude_mb + self.codex_mb
    }

    /// One `ps` call per sample; best-effort (zeros on failure).
    pub fn sample() -> Self {
        let output = Command::new("ps").args(["-axo", "rss=,comm="]).output();
        let Ok(output) = output else {
            return Self::default();
        };
        if !output.status.success() {
            return Self::default();
        }
        Self::from_ps_output(&String::from_utf8_lossy(&output.stdout))
    }

    /// Parse `ps -axo rss=,comm=` output (RSS in kilobytes, then the executable path).
    pub fn from_ps_output(text: &str) -> Self {
        let (mut cursor_kb, mut claude_kb, mut codex_kb) = (0u64, 0u64, 0u64);
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut it = line.splitn(2, |c: char| c.is_whitespace());
            let rss_kb = it.next().unwrap_or("").parse::<u64>().unwrap_or(0);
            let comm = it.next().unwrap_or("");
            match classify_agent(comm) {
                Some(AgentFamily::Cursor) => cursor_kb += rss_kb,
                Some(AgentFamily::Claude) => claude_kb += rss_kb,
                Some(AgentFamily::Codex) => codex_kb += rss_kb,
                None => {}
            }
        }
        Self {
            cursor_mb: cursor_kb / 1024,
            claude_mb: claude_kb / 1024,
            codex_mb: codex_kb / 1024,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_cursor_app_processes() {
        assert_eq!(
            classify_agent("/Applications/Cursor.app/Contents/Frameworks/Cursor Helper (Renderer).app/Contents/MacOS/Cursor Helper (Renderer)"),
            Some(AgentFamily::Cursor)
        );
    }

    #[test]
    fn system_cursor_ui_service_is_not_cursor() {
        assert_eq!(
            classify_agent("/System/Library/PrivateFrameworks/TextInputUIMacHelper.framework/Versions/A/XPCServices/CursorUIViewService.xpc/Contents/MacOS/CursorUIViewService"),
            None
        );
    }

    #[test]
    fn classifies_claude_desktop_and_cli() {
        assert_eq!(
            classify_agent("/Applications/Claude.app/Contents/MacOS/Claude"),
            Some(AgentFamily::Claude)
        );
        assert_eq!(
            classify_agent("/Users/me/Library/Application Support/Claude/claude-code/2.1.0/abc/claude.app/Contents/MacOS/claude"),
            Some(AgentFamily::Claude)
        );
        assert_eq!(classify_agent("/Users/me/.local/bin/claude"), Some(AgentFamily::Claude));
        assert_eq!(classify_agent("claude"), Some(AgentFamily::Claude));
        // Native installer: the running binary is named after its version.
        assert_eq!(
            classify_agent("/Users/me/.local/share/claude/versions/2.1.251"),
            Some(AgentFamily::Claude)
        );
    }

    #[test]
    fn classifies_codex() {
        assert_eq!(
            classify_agent("/Applications/Codex.app/Contents/MacOS/Codex"),
            Some(AgentFamily::Codex)
        );
        assert_eq!(classify_agent("/opt/homebrew/bin/codex"), Some(AgentFamily::Codex));
        assert_eq!(classify_agent("codex-aarch64-apple-darwin"), Some(AgentFamily::Codex));
    }

    #[test]
    fn unrelated_processes_are_ignored() {
        assert_eq!(classify_agent("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"), None);
        assert_eq!(classify_agent("/usr/bin/claudette"), None);
        assert_eq!(classify_agent(""), None);
    }

    #[test]
    fn sums_rss_per_family_in_megabytes() {
        let ps = "  2048 /Applications/Claude.app/Contents/MacOS/Claude\n\
                  1024 /Users/me/.local/bin/claude\n\
                  4096 /Applications/Cursor.app/Contents/MacOS/Cursor\n\
                   512 /usr/sbin/cfprefsd\n\
                  garbage line here\n\
                  3072 /Applications/Codex.app/Contents/MacOS/Codex\n";
        let rss = AgentRss::from_ps_output(ps);
        assert_eq!(rss.claude_mb, 3);
        assert_eq!(rss.cursor_mb, 4);
        assert_eq!(rss.codex_mb, 3);
        assert_eq!(rss.total_mb(), 10);
    }

    #[test]
    fn empty_ps_output_is_zero() {
        assert_eq!(AgentRss::from_ps_output(""), AgentRss::default());
    }
}
