//! Time spent at each pressure level, per local day (`~/.guardian/pressure_stats.json`).
//!
//! The daemon accumulates this as it samples; `guardian stats` prints it. It answers the
//! question the thresholds have to be judged by: how much of the day does this machine
//! spend at `critical`, and how often does the level flip?

use crate::classifier::PressureLevel;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;

const STATS_JSON: &str = "pressure_stats.json";
/// Days of history kept on disk.
pub const KEEP_DAYS: usize = 30;
/// `critical` should be the exception; above this share of sampled time the thresholds
/// (or the machine) need attention.
pub const CRITICAL_TARGET_PERCENT: f64 = 5.0;

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct DayStats {
    #[serde(default)]
    pub clear_secs: f64,
    #[serde(default)]
    pub strained_secs: f64,
    #[serde(default)]
    pub critical_secs: f64,
    /// Every change of the published level.
    #[serde(default)]
    pub transitions: u32,
    /// Changes that ended at `critical`.
    #[serde(default)]
    pub critical_entries: u32,
}

impl DayStats {
    pub fn total_secs(&self) -> f64 {
        self.clear_secs + self.strained_secs + self.critical_secs
    }

    fn add(&mut self, other: &DayStats) {
        self.clear_secs += other.clear_secs;
        self.strained_secs += other.strained_secs;
        self.critical_secs += other.critical_secs;
        self.transitions = self.transitions.saturating_add(other.transitions);
        self.critical_entries = self.critical_entries.saturating_add(other.critical_entries);
    }
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct PressureStats {
    /// Keyed by local date, `YYYY-MM-DD` (sorts chronologically).
    #[serde(default)]
    pub days: BTreeMap<String, DayStats>,
}

pub fn stats_path() -> std::path::PathBuf {
    crate::config::guardian_dir().join(STATS_JSON)
}

/// Today's key in the local time zone.
pub fn today_key() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

impl PressureStats {
    /// Missing or unreadable history is simply empty history.
    pub fn load() -> Self {
        fs::read_to_string(stats_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let dir = crate::config::guardian_dir();
        fs::create_dir_all(&dir)?;
        let json = serde_json::to_string(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let tmp = dir.join(format!("{STATS_JSON}.tmp"));
        let result = fs::write(&tmp, json).and_then(|()| fs::rename(&tmp, stats_path()));
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }

    pub(crate) fn record(&mut self, day: &str, level: PressureLevel, secs: f64) {
        if !secs.is_finite() || secs <= 0.0 {
            return;
        }
        let d = self.days.entry(day.to_string()).or_default();
        match level {
            PressureLevel::Clear => d.clear_secs += secs,
            PressureLevel::Strained => d.strained_secs += secs,
            PressureLevel::Critical => d.critical_secs += secs,
        }
    }

    pub(crate) fn record_transition(&mut self, day: &str, to: PressureLevel) {
        let d = self.days.entry(day.to_string()).or_default();
        d.transitions = d.transitions.saturating_add(1);
        if to == PressureLevel::Critical {
            d.critical_entries = d.critical_entries.saturating_add(1);
        }
    }

    /// Drop all but the most recent `keep` days.
    pub fn prune(&mut self, keep: usize) {
        while self.days.len() > keep {
            self.days.pop_first();
        }
    }

    /// Plain-text report of the most recent `days` days, oldest first, with a total line.
    pub fn render(&self, days: usize) -> String {
        let recent: Vec<(&String, &DayStats)> = {
            let mut v: Vec<_> = self.days.iter().rev().take(days.max(1)).collect();
            v.reverse();
            v
        };
        if recent.iter().all(|(_, d)| d.total_secs() <= 0.0) {
            return "No pressure history yet. guardiand (0.8+) records it as it samples; \
                    check back after it has run for a while.\n"
                .to_string();
        }

        let mut out = String::from(
            "Share of sampled time at each pressure level\n\
             date          clear  strained  critical   changes  to-critical   sampled\n",
        );
        let mut total = DayStats::default();
        for (day, d) in &recent {
            total.add(d);
            out.push_str(&row(day, d));
        }
        if recent.len() > 1 {
            out.push_str(&row("total", &total));
        }

        let critical_pct = percent(total.critical_secs, total.total_secs());
        out.push('\n');
        if critical_pct > CRITICAL_TARGET_PERCENT {
            out.push_str(&format!(
                "Critical {critical_pct:.1}% of sampled time, above the {CRITICAL_TARGET_PERCENT:.0}% target. \
                 Check `pressure_reasons` in ~/.guardian/state.json for what is driving it.\n"
            ));
        } else {
            out.push_str(&format!(
                "Critical {critical_pct:.1}% of sampled time, within the {CRITICAL_TARGET_PERCENT:.0}% target.\n"
            ));
        }
        out
    }
}

fn percent(part: f64, whole: f64) -> f64 {
    if whole <= 0.0 {
        0.0
    } else {
        part / whole * 100.0
    }
}

fn row(label: &str, d: &DayStats) -> String {
    let t = d.total_secs();
    format!(
        "{:<12} {:>5.0}%  {:>7.0}%  {:>7.0}%  {:>8}  {:>11}  {:>7.1}h\n",
        label,
        percent(d.clear_secs, t),
        percent(d.strained_secs, t),
        percent(d.critical_secs, t),
        d.transitions,
        d.critical_entries,
        t / 3600.0
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_time_against_the_right_level() {
        let mut s = PressureStats::default();
        s.record("2026-11-19", PressureLevel::Clear, 2.0);
        s.record("2026-11-19", PressureLevel::Strained, 4.0);
        s.record("2026-11-19", PressureLevel::Critical, 6.0);
        let d = &s.days["2026-11-19"];
        assert_eq!((d.clear_secs, d.strained_secs, d.critical_secs), (2.0, 4.0, 6.0));
        assert_eq!(d.total_secs(), 12.0);
    }

    #[test]
    fn ignores_nonsense_durations() {
        let mut s = PressureStats::default();
        s.record("d", PressureLevel::Clear, -1.0);
        s.record("d", PressureLevel::Clear, f64::NAN);
        s.record("d", PressureLevel::Clear, 0.0);
        assert!(s.days.is_empty());
    }

    #[test]
    fn counts_transitions_and_critical_entries() {
        let mut s = PressureStats::default();
        s.record_transition("d", PressureLevel::Strained);
        s.record_transition("d", PressureLevel::Critical);
        s.record_transition("d", PressureLevel::Strained);
        assert_eq!(s.days["d"].transitions, 3);
        assert_eq!(s.days["d"].critical_entries, 1);
    }

    #[test]
    fn prune_keeps_the_newest_days() {
        let mut s = PressureStats::default();
        for day in ["2026-11-17", "2026-11-18", "2026-11-19"] {
            s.record(day, PressureLevel::Clear, 1.0);
        }
        s.prune(2);
        assert_eq!(
            s.days.keys().cloned().collect::<Vec<_>>(),
            vec!["2026-11-18".to_string(), "2026-11-19".to_string()]
        );
    }

    #[test]
    fn empty_history_says_so() {
        assert!(PressureStats::default().render(7).starts_with("No pressure history"));
    }

    #[test]
    fn render_reports_shares_and_flags_a_missed_target() {
        let mut s = PressureStats::default();
        s.record("2026-11-18", PressureLevel::Clear, 3600.0);
        s.record("2026-11-19", PressureLevel::Strained, 1800.0);
        s.record("2026-11-19", PressureLevel::Critical, 1800.0);
        s.record_transition("2026-11-19", PressureLevel::Critical);
        let text = s.render(7);
        assert!(text.contains("2026-11-18"), "{text}");
        assert!(text.contains("total"), "{text}");
        // 1800 s critical of 7200 s sampled.
        assert!(text.contains("Critical 25.0% of sampled time, above the 5% target"), "{text}");
    }

    #[test]
    fn render_limits_to_the_requested_days() {
        let mut s = PressureStats::default();
        s.record("2026-11-18", PressureLevel::Critical, 3600.0);
        s.record("2026-11-19", PressureLevel::Clear, 3600.0);
        let text = s.render(1);
        assert!(!text.contains("2026-11-18"), "{text}");
        assert!(text.contains("within the 5% target"), "{text}");
    }

    #[test]
    fn round_trips_through_json_and_tolerates_missing_fields() {
        let mut s = PressureStats::default();
        s.record("d", PressureLevel::Strained, 5.0);
        let back: PressureStats = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        let sparse: PressureStats = serde_json::from_str(r#"{"days":{"d":{"clear_secs":1.0}}}"#).unwrap();
        assert_eq!(sparse.days["d"].clear_secs, 1.0);
        assert_eq!(sparse.days["d"].transitions, 0);
    }
}
