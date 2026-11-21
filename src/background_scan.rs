//! Keeping the disk-space index fresh without being asked.
//!
//! Finding what can be deleted takes minutes of reading the disk the first time, which is
//! the wrong thing to start once the disk is already full. So `guardiand` runs
//! `guardian disk refresh` now and then, as a separate short-lived process at background
//! priority, and `guardian disk` and the menu bar read what it saved.
//!
//! It is a separate process so that a slow scan never delays a pressure sample, and so that
//! stopping it is one signal. It only ever reads the disk and writes its own index file.

use crate::classifier::PressureLevel;
use crate::state::DiskPressureLevel;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

/// Scans never start closer together than this, whatever else says to.
pub const MIN_GAP: Duration = Duration::from_secs(10 * 60);

/// A disk that has just become tight is scanned ahead of the interval, but not if the index
/// is newer than this: usage hovering at a threshold would otherwise scan over and over.
pub const RISE_MIN_AGE: Duration = Duration::from_secs(60 * 60);

/// A scan that runs out of time with places still to measure is followed by another after
/// `MIN_GAP` instead of a whole interval, at most this many times in a row. The cap is for a
/// folder too large to ever be measured inside one scan: without it the daemon would rescan
/// every ten minutes for good.
pub const MAX_CATCH_UPS: u8 = 6;

/// No scan in the daemon's first minutes: it usually starts at login, when everything else
/// is starting too.
const STARTUP_GRACE: Duration = Duration::from_secs(5 * 60);

/// Time budget of one background scan, in seconds. It carries on where the last one
/// stopped, so a short budget costs only freshness.
const SCAN_SECONDS: &str = "120";

/// Whether to start a scan now.
///
/// - `interval_minutes`: `[disk] background_scan_minutes`; 0 turns background scans off.
/// - `disk_tight`: the disk is at `warn` or `critical`.
/// - `disk_rose`: it has reached a higher level since the last scan started.
/// - `index_age`: time since the index was last written; `None` if it never was.
/// - `since_last_start`: time since this daemon last started a scan, and the gap to leave.
/// - `catching_up`: the last scan left places unmeasured, and the cap on follow-up scans
///   has not been reached.
pub fn due(
    interval_minutes: u64,
    pressure: PressureLevel,
    disk_tight: bool,
    disk_rose: bool,
    index_age: Option<Duration>,
    since_last_start: Option<(Duration, Duration)>,
    catching_up: bool,
) -> bool {
    if interval_minutes == 0 || pressure == PressureLevel::Critical {
        return false;
    }
    if since_last_start.is_some_and(|(since, gap)| since < gap) {
        return false;
    }
    // While memory or CPU is strained, read the disk only if the list is likely to be
    // wanted soon.
    if pressure != PressureLevel::Clear && !disk_tight {
        return false;
    }
    let interval = Duration::from_secs(interval_minutes.saturating_mul(60));
    match index_age {
        None => true,
        Some(age) => {
            age >= interval
                // The disk has just become tight: have the list ready before it is asked for.
                || (disk_rose && age >= RISE_MIN_AGE)
                // The last scan did not get through everything: carry on soon.
                || (catching_up && age >= MIN_GAP)
        }
    }
}

/// How a finished scan ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Everything is measured (or another scan was already doing it).
    Done,
    /// Saved what it measured, with places left over.
    MoreToDo,
    /// Could not run: an older `guardian` without `disk refresh`, a crash, a signal.
    Failed,
}

/// Read a scan's exit status; `None` when it was ended by a signal.
pub fn outcome(code: Option<i32>) -> Outcome {
    match code {
        Some(0) => Outcome::Done,
        Some(crate::disk_targets::REFRESH_EXIT_MORE) => Outcome::MoreToDo,
        _ => Outcome::Failed,
    }
}

fn rank(level: &DiskPressureLevel) -> u8 {
    match level {
        DiskPressureLevel::Clear => 0,
        DiskPressureLevel::Warn => 1,
        DiskPressureLevel::Critical => 2,
    }
}

/// How long ago the index file was written.
fn index_age() -> Option<Duration> {
    let written = std::fs::metadata(crate::disk_targets::index_path())
        .ok()?
        .modified()
        .ok()?;
    Some(SystemTime::now().duration_since(written).unwrap_or_default())
}

/// Start `guardian disk refresh` from beside this executable. `None` when there is no
/// `guardian` there or it cannot be started.
fn start_scan() -> Option<Child> {
    let guardian = std::env::current_exe().ok()?.parent()?.join("guardian");
    if !guardian.is_file() {
        return None;
    }
    Command::new(guardian)
        .args(["disk", "refresh", "--seconds", SCAN_SECONDS])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

/// The daemon's side of background scanning: at most one scan at a time.
pub struct BackgroundScan {
    daemon_started: Instant,
    running: Option<Child>,
    last_start: Option<Instant>,
    /// The last scan could not be started or did not finish cleanly: wait a whole interval
    /// before trying again rather than retrying every few minutes.
    last_failed: bool,
    last_level: u8,
    /// The disk has got tighter and no scan has started since.
    rose: bool,
    /// Scans in a row that ended with places still to measure.
    catch_ups: u8,
    /// The running scan has been told to stop and has not been collected yet.
    stopping: bool,
}

impl BackgroundScan {
    pub fn new() -> Self {
        Self {
            daemon_started: Instant::now(),
            running: None,
            last_start: None,
            last_failed: false,
            last_level: 0,
            rose: false,
            catch_ups: 0,
            stopping: false,
        }
    }

    /// Call once per sample: collects a finished scan, stops a running one if load has
    /// become critical, and starts one when it is due.
    pub fn tick(&mut self, interval_minutes: u64, pressure: PressureLevel, level: &DiskPressureLevel) {
        if let Some(child) = &mut self.running {
            match child.try_wait() {
                Ok(None) if pressure == PressureLevel::Critical && !self.stopping => {
                    // It saves as it goes, so the next scan carries on from about here. It is
                    // collected on a later sample, not waited for: a scan stuck on a stalled
                    // disk must not hold up sampling, least of all now.
                    let _ = child.kill();
                    self.stopping = true;
                    self.catch_ups = self.catch_ups.saturating_add(1);
                    eprintln!("[guardiand] background disk scan stopped: load is critical");
                    return;
                }
                Ok(None) => return,
                Ok(Some(_)) if self.stopping => {
                    self.stopping = false;
                    self.running = None;
                }
                Ok(Some(status)) => {
                    let how = outcome(status.code());
                    self.last_failed = how == Outcome::Failed;
                    self.catch_ups = match how {
                        Outcome::MoreToDo => self.catch_ups.saturating_add(1),
                        _ => 0,
                    };
                    self.running = None;
                }
                Err(_) => {
                    self.stopping = false;
                    self.running = None;
                }
            }
        }

        let now = rank(level);
        if now > self.last_level {
            self.rose = true;
        } else if now == 0 {
            self.rose = false;
        }
        self.last_level = now;

        let gap = if self.last_failed {
            MIN_GAP.max(Duration::from_secs(interval_minutes.saturating_mul(60)))
        } else {
            MIN_GAP
        };
        let since_last_start = self.last_start.map(|at| (at.elapsed(), gap));
        if self.daemon_started.elapsed() < STARTUP_GRACE {
            return;
        }
        let catching_up = (1..=MAX_CATCH_UPS).contains(&self.catch_ups);
        if !due(
            interval_minutes,
            pressure,
            now > 0,
            self.rose,
            index_age(),
            since_last_start,
            catching_up,
        ) {
            return;
        }
        self.last_start = Some(Instant::now());
        self.rose = false;
        self.running = start_scan();
        self.last_failed = self.running.is_none();
        if self.running.is_some() {
            eprintln!(
                "[guardiand] background disk scan started{}",
                if catching_up { " (carrying on: the last one left places unmeasured)" } else { "" }
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PressureLevel::{Clear, Critical, Strained};

    const HOUR: Duration = Duration::from_secs(3600);
    const MINUTE: Duration = Duration::from_secs(60);

    #[test]
    fn scans_when_the_index_is_missing_or_older_than_the_interval() {
        assert!(due(180, Clear, false, false, None, None, false));
        assert!(due(180, Clear, false, false, Some(4 * HOUR), None, false));
        assert!(!due(180, Clear, false, false, Some(2 * HOUR), None, false));
        assert!(due(180, Clear, false, false, Some(3 * HOUR), None, false));
    }

    #[test]
    fn zero_minutes_turns_it_off() {
        assert!(!due(0, Clear, true, true, None, None, false));
        assert!(!due(0, Clear, true, true, Some(100 * HOUR), None, false));
    }

    #[test]
    fn never_at_critical_load_and_under_strain_only_when_the_disk_is_tight() {
        assert!(!due(180, Critical, true, true, None, None, false));
        assert!(!due(180, Strained, false, false, Some(9 * HOUR), None, false));
        assert!(due(180, Strained, true, false, Some(9 * HOUR), None, false));
        assert!(!due(180, Strained, true, false, Some(HOUR), None, false));
    }

    #[test]
    fn a_disk_that_just_got_tight_is_scanned_without_waiting_for_the_interval() {
        assert!(due(180, Clear, true, true, Some(HOUR), None, false));
        assert!(due(180, Strained, true, true, Some(HOUR), None, false));
        // Unless the index is recent: usage hovering at a threshold must not scan repeatedly.
        assert!(!due(180, Clear, true, true, Some(50 * MINUTE), None, false));
        // Tight for a while is not "just got tight".
        assert!(!due(180, Clear, true, false, Some(HOUR), None, false));
    }

    #[test]
    fn leaves_a_gap_after_the_last_start() {
        let just_started = Some((2 * MINUTE, MIN_GAP));
        assert!(!due(180, Clear, false, false, None, just_started, false));
        assert!(!due(180, Clear, true, true, Some(9 * HOUR), just_started, false));
        assert!(due(180, Clear, false, false, None, Some((11 * MINUTE, MIN_GAP)), false));
        // After a scan that failed, the gap is the whole interval.
        assert!(!due(180, Clear, false, false, None, Some((HOUR, 3 * HOUR)), false));
        assert!(due(180, Clear, false, false, None, Some((3 * HOUR, 3 * HOUR)), false));
    }

    #[test]
    fn a_scan_that_left_work_is_followed_up_after_the_minimum_gap() {
        let catching_up = |pressure, tight, age, since| due(180, pressure, tight, false, Some(age), since, true);
        assert!(catching_up(Clear, false, 11 * MINUTE, Some((11 * MINUTE, MIN_GAP))));
        assert!(!catching_up(Clear, false, 9 * MINUTE, Some((11 * MINUTE, MIN_GAP))));
        assert!(!catching_up(Clear, false, 11 * MINUTE, Some((9 * MINUTE, MIN_GAP))));
        // The same limits on load apply to a follow-up scan.
        assert!(!catching_up(Critical, true, HOUR, None));
        assert!(!catching_up(Strained, false, HOUR, None));
        assert!(catching_up(Strained, true, HOUR, None));
        // Without leftover work, an index eleven minutes old is left alone.
        assert!(!due(180, Clear, false, false, Some(11 * MINUTE), None, false));
        // Turned off is turned off.
        assert!(!due(0, Clear, true, false, Some(HOUR), None, true));
    }

    #[test]
    fn exit_status_says_how_a_scan_ended() {
        assert_eq!(outcome(Some(0)), Outcome::Done);
        assert_eq!(outcome(Some(crate::disk_targets::REFRESH_EXIT_MORE)), Outcome::MoreToDo);
        // An older guardian has no `disk refresh` and exits 2; a signal gives no status.
        assert_eq!(outcome(Some(2)), Outcome::Failed);
        assert_eq!(outcome(Some(1)), Outcome::Failed);
        assert_eq!(outcome(None), Outcome::Failed);
    }

    #[test]
    fn follow_up_scans_stop_after_the_cap() {
        let mut scan = BackgroundScan::new();
        let catching_up = |scan: &BackgroundScan| (1..=MAX_CATCH_UPS).contains(&scan.catch_ups);
        assert!(!catching_up(&scan));
        scan.catch_ups = 1;
        assert!(catching_up(&scan));
        scan.catch_ups = MAX_CATCH_UPS;
        assert!(catching_up(&scan));
        scan.catch_ups = MAX_CATCH_UPS + 1;
        assert!(!catching_up(&scan), "after the cap, back to the normal interval");
    }

    #[test]
    fn disk_levels_rank_in_order() {
        assert!(rank(&DiskPressureLevel::Clear) < rank(&DiskPressureLevel::Warn));
        assert!(rank(&DiskPressureLevel::Warn) < rank(&DiskPressureLevel::Critical));
    }
}
