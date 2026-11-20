use crate::config::ThresholdConfig;
use crate::sampler::memory::MemoryPressureLevel;
use crate::sampler::thermal::ThermalState;

// Variant order is severity order: `Clear < Strained < Critical`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PressureLevel {
    Clear,
    Strained,
    Critical,
}

impl std::fmt::Display for PressureLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Clear => write!(f, "clear"),
            Self::Strained => write!(f, "strained"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

pub struct ClassifierInput {
    pub cpu_percent: f64,
    pub memory_available_gb: f64,
    pub memory_total_gb: f64,
    pub swap_used_percent: f64,
    pub thermal: ThermalState,
    pub process_usage_ratio: f64,
    /// The kernel's own memory-pressure verdict (`kern.memorystatus_vm_pressure_level`).
    pub memory_pressure: MemoryPressureLevel,
}

/// A pressure level plus the conditions that produced it, in plain words, so hooks can tell
/// the user *why* they were slowed down or blocked.
#[derive(Clone, Debug, PartialEq)]
pub struct Classification {
    pub level: PressureLevel,
    pub reasons: Vec<String>,
}

fn memory_available_ratio(input: &ClassifierInput) -> f64 {
    if input.memory_total_gb <= 0.01 {
        return 1.0;
    }
    input.memory_available_gb / input.memory_total_gb
}

pub fn classify(input: &ClassifierInput, cfg: &ThresholdConfig) -> PressureLevel {
    classify_with_reasons(input, cfg).level
}

/// Classify one sample.
///
/// Two signals are noisy on their own and, with `corroborate_critical` (the default), can only
/// reach `critical` when a second signal agrees; otherwise they count as `strained`:
///
/// * **Swap used %** — macOS grows swap on demand, so "90% of the current swap file" is
///   routinely reached moments before the file is enlarged, and a small-RAM Mac sits near the
///   top of its swap file all day while the kernel reports nothing worse than `warn`. It is
///   critical only if the kernel itself reports *critical* memory pressure. (Before 0.8 a
///   kernel `warn` was enough, which made swap hovering at the threshold flip the level
///   dozens of times a day.)
/// * **CPU %** — a compile pegs every core without endangering the machine. It is critical
///   only if the machine is also heating up (thermal fair or worse).
///
/// When the corroborating signal is `Unknown` (not available on this machine) the old
/// behavior applies: the noisy signal alone is enough.
pub fn classify_with_reasons(input: &ClassifierInput, cfg: &ThresholdConfig) -> Classification {
    evaluate(input, cfg).into_classification()
}

/// Classify one sample with an exit band around the numeric thresholds.
///
/// A level is *entered* at the configured thresholds but only *left* once the numbers have
/// moved `band` (a fraction, e.g. `0.10`) past them — so a value hovering at a threshold holds
/// its level instead of flipping on every sample. `held` is the level currently published.
/// The kernel memory-pressure and thermal levels are discrete and have no band; the dwell time
/// in the daemon loop covers those.
pub fn classify_with_band(
    input: &ClassifierInput,
    cfg: &ThresholdConfig,
    held: PressureLevel,
    band: f64,
) -> Classification {
    let enter = evaluate(input, cfg);
    let enter_level = enter.level();
    if band.is_nan() || band <= 0.0 || enter_level >= held {
        return enter.into_classification();
    }

    // Below the held level by the entry thresholds: is the sample still inside its exit band?
    let exit = evaluate(input, &cfg.with_exit_band(band));
    let level = enter_level.max(exit.level().min(held));
    if level == enter_level {
        return enter.into_classification();
    }
    let reasons = match level {
        PressureLevel::Critical => exit.critical,
        // Held at strained: everything still inside a band is a cause, whichever band it is.
        _ => exit.critical.into_iter().chain(exit.strained).collect(),
    };
    Classification { level, reasons }
}

/// Causes found in one sample, split by the level each can establish.
struct Evaluation {
    critical: Vec<String>,
    strained: Vec<String>,
}

impl Evaluation {
    fn level(&self) -> PressureLevel {
        if !self.critical.is_empty() {
            PressureLevel::Critical
        } else if !self.strained.is_empty() {
            PressureLevel::Strained
        } else {
            PressureLevel::Clear
        }
    }

    fn into_classification(self) -> Classification {
        let level = self.level();
        let reasons = match level {
            PressureLevel::Critical => self.critical,
            PressureLevel::Strained => self.strained,
            PressureLevel::Clear => Vec::new(),
        };
        Classification { level, reasons }
    }
}

fn evaluate(input: &ClassifierInput, cfg: &ThresholdConfig) -> Evaluation {
    let mut critical: Vec<String> = Vec::new();
    let mut strained: Vec<String> = Vec::new();

    // --- Memory: the kernel's verdict, then our own headroom thresholds ---
    match input.memory_pressure {
        MemoryPressureLevel::Critical => {
            critical.push("macOS reports critical memory pressure".to_string())
        }
        MemoryPressureLevel::Warn => {
            strained.push("macOS reports memory pressure".to_string())
        }
        MemoryPressureLevel::Normal | MemoryPressureLevel::Unknown => {}
    }

    let ratio = memory_available_ratio(input);
    let ratio_critical = cfg
        .critical_memory_available_ratio
        .is_some_and(|r| ratio < r);
    let ratio_strained = cfg
        .strained_memory_available_ratio
        .is_some_and(|r| ratio < r);

    if input.memory_available_gb < cfg.critical_memory_gb || ratio_critical {
        critical.push(format!(
            "only {:.1} GB RAM available",
            input.memory_available_gb
        ));
    } else if input.memory_available_gb < cfg.strained_memory_gb || ratio_strained {
        strained.push(format!(
            "{:.1} GB RAM available",
            input.memory_available_gb
        ));
    }

    // --- Swap ---
    let swap_corroborated = !cfg.corroborate_critical
        || matches!(
            input.memory_pressure,
            MemoryPressureLevel::Unknown | MemoryPressureLevel::Critical
        );
    if input.swap_used_percent > cfg.critical_swap_percent {
        let reason = format!("swap {:.0}% used", input.swap_used_percent);
        if swap_corroborated {
            critical.push(reason);
        } else {
            strained.push(reason);
        }
    } else if input.swap_used_percent > cfg.strained_swap_percent {
        strained.push(format!("swap {:.0}% used", input.swap_used_percent));
    }

    // --- Thermal ---
    match input.thermal {
        ThermalState::Critical => critical.push("thermal pressure critical".to_string()),
        ThermalState::Serious => critical.push("thermal pressure heavy".to_string()),
        ThermalState::Fair => strained.push("thermal pressure moderate".to_string()),
        ThermalState::Nominal | ThermalState::Unknown => {}
    }

    // --- CPU ---
    let cpu_corroborated = !cfg.corroborate_critical
        || matches!(
            input.thermal,
            ThermalState::Unknown | ThermalState::Fair | ThermalState::Serious | ThermalState::Critical
        );
    if input.cpu_percent > cfg.critical_cpu_percent {
        let reason = format!("CPU {:.0}%", input.cpu_percent);
        if cpu_corroborated {
            critical.push(reason);
        } else {
            strained.push(reason);
        }
    } else if input.cpu_percent > cfg.strained_cpu_percent {
        strained.push(format!("CPU {:.0}%", input.cpu_percent));
    }

    // --- Process table ---
    if input.process_usage_ratio > 0.8 {
        critical.push(format!(
            "process table {:.0}% full",
            input.process_usage_ratio * 100.0
        ));
    } else if input.process_usage_ratio > 0.6 {
        strained.push(format!(
            "process table {:.0}% full",
            input.process_usage_ratio * 100.0
        ));
    }

    Evaluation { critical, strained }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThresholdConfig;
    use crate::sampler::memory::MemoryPressureLevel;
    use crate::sampler::thermal::ThermalState;

    fn defaults() -> ThresholdConfig {
        ThresholdConfig::default()
    }

    fn nominal_input() -> ClassifierInput {
        ClassifierInput {
            cpu_percent: 30.0,
            memory_available_gb: 8.0,
            memory_total_gb: 16.0,
            swap_used_percent: 5.0,
            thermal: ThermalState::Nominal,
            process_usage_ratio: 0.2,
            memory_pressure: MemoryPressureLevel::Normal,
        }
    }

    /// Thresholds with corroboration off: every signal can reach critical alone (pre-0.6 behavior).
    fn uncorroborated() -> ThresholdConfig {
        ThresholdConfig {
            corroborate_critical: false,
            ..ThresholdConfig::default()
        }
    }

    #[test]
    fn all_nominal_is_clear() {
        assert_eq!(classify(&nominal_input(), &defaults()), PressureLevel::Clear);
    }

    #[test]
    fn cpu_above_critical_threshold() {
        let mut input = nominal_input();
        input.cpu_percent = 91.0;
        assert_eq!(classify(&input, &uncorroborated()), PressureLevel::Critical);
    }

    #[test]
    fn cpu_alone_is_only_strained_when_machine_is_cool() {
        let mut input = nominal_input();
        input.cpu_percent = 99.0;
        input.thermal = ThermalState::Nominal;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Strained);
    }

    #[test]
    fn cpu_plus_heat_is_critical() {
        let mut input = nominal_input();
        input.cpu_percent = 99.0;
        input.thermal = ThermalState::Fair;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Critical);
    }

    #[test]
    fn cpu_alone_is_critical_when_thermal_is_unavailable() {
        // No thermal signal on this machine: keep the old, conservative behavior.
        let mut input = nominal_input();
        input.cpu_percent = 99.0;
        input.thermal = ThermalState::Unknown;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Critical);
    }

    #[test]
    fn cpu_above_strained_below_critical() {
        let mut input = nominal_input();
        input.cpu_percent = 75.0;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Strained);
    }

    #[test]
    fn cpu_at_exact_strained_boundary_is_clear() {
        let mut input = nominal_input();
        input.cpu_percent = 70.0; // threshold is > 70, so exactly 70 is clear
        assert_eq!(classify(&input, &defaults()), PressureLevel::Clear);
    }

    #[test]
    fn cpu_just_above_strained_boundary() {
        let mut input = nominal_input();
        input.cpu_percent = 70.1;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Strained);
    }

    #[test]
    fn memory_below_critical_threshold() {
        let mut input = nominal_input();
        input.memory_available_gb = 0.5;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Critical);
    }

    #[test]
    fn memory_between_strained_and_critical() {
        let mut input = nominal_input();
        input.memory_available_gb = 1.5;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Strained);
    }

    #[test]
    fn memory_at_exact_strained_boundary_is_clear() {
        let mut input = nominal_input();
        input.memory_available_gb = 2.0; // threshold is < 2.0, so exactly 2.0 is clear
        assert_eq!(classify(&input, &defaults()), PressureLevel::Clear);
    }

    #[test]
    fn swap_above_critical_threshold() {
        let mut input = nominal_input();
        input.swap_used_percent = 55.0;
        assert_eq!(classify(&input, &uncorroborated()), PressureLevel::Critical);
    }

    #[test]
    fn swap_between_strained_and_critical() {
        let mut input = nominal_input();
        input.swap_used_percent = 30.0;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Strained);
    }

    #[test]
    fn thermal_critical_triggers_critical() {
        let mut input = nominal_input();
        input.thermal = ThermalState::Critical;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Critical);
    }

    #[test]
    fn thermal_serious_triggers_critical() {
        let mut input = nominal_input();
        input.thermal = ThermalState::Serious;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Critical);
    }

    #[test]
    fn thermal_fair_triggers_strained() {
        let mut input = nominal_input();
        input.thermal = ThermalState::Fair;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Strained);
    }

    #[test]
    fn thermal_unknown_does_not_trigger() {
        let mut input = nominal_input();
        input.thermal = ThermalState::Unknown;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Clear);
    }

    #[test]
    fn process_ratio_above_critical_hard_threshold() {
        let mut input = nominal_input();
        input.process_usage_ratio = 0.85;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Critical);
    }

    #[test]
    fn process_ratio_above_strained_hard_threshold() {
        let mut input = nominal_input();
        input.process_usage_ratio = 0.65;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Strained);
    }

    #[test]
    fn process_ratio_at_boundary_is_clear() {
        let mut input = nominal_input();
        input.process_usage_ratio = 0.6; // threshold is > 0.6
        assert_eq!(classify(&input, &defaults()), PressureLevel::Clear);
    }

    #[test]
    fn single_critical_sensor_overrides_all_clear() {
        let mut input = nominal_input();
        input.swap_used_percent = 99.0;
        assert_eq!(classify(&input, &uncorroborated()), PressureLevel::Critical);
    }

    #[test]
    fn memory_available_ratio_can_escalate() {
        let mut cfg = defaults();
        cfg.strained_memory_available_ratio = Some(0.25);
        cfg.critical_memory_available_ratio = Some(0.10);
        let mut input = nominal_input();
        input.memory_total_gb = 16.0;
        input.memory_available_gb = 1.0; // 1/16 = 0.0625 < 0.10
        assert_eq!(classify(&input, &cfg), PressureLevel::Critical);

        let mut input2 = nominal_input();
        input2.memory_total_gb = 16.0;
        input2.memory_available_gb = 3.0; // 3/16 = 0.1875 < 0.25, not < 0.10
        assert_eq!(classify(&input2, &cfg), PressureLevel::Strained);
    }
    // --- kernel memory pressure + corroboration ---

    #[test]
    fn kernel_memory_warn_is_strained() {
        let mut input = nominal_input();
        input.memory_pressure = MemoryPressureLevel::Warn;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Strained);
    }

    #[test]
    fn kernel_memory_critical_is_critical() {
        let mut input = nominal_input();
        input.memory_pressure = MemoryPressureLevel::Critical;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Critical);
        // ...even with corroboration switched off.
        assert_eq!(classify(&input, &uncorroborated()), PressureLevel::Critical);
    }

    #[test]
    fn kernel_memory_unknown_alone_does_not_trigger() {
        let mut input = nominal_input();
        input.memory_pressure = MemoryPressureLevel::Unknown;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Clear);
    }

    #[test]
    fn swap_alone_is_only_strained_when_kernel_says_memory_is_fine() {
        let mut input = nominal_input();
        input.swap_used_percent = 99.0;
        input.memory_pressure = MemoryPressureLevel::Normal;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Strained);
    }

    #[test]
    fn swap_plus_kernel_warn_is_only_strained() {
        // An 8 GB Mac lives here all day: swap file nearly full, kernel says "warn".
        let mut input = nominal_input();
        input.swap_used_percent = 99.0;
        input.memory_pressure = MemoryPressureLevel::Warn;
        let c = classify_with_reasons(&input, &defaults());
        assert_eq!(c.level, PressureLevel::Strained);
        assert_eq!(
            c.reasons,
            vec![
                "macOS reports memory pressure".to_string(),
                "swap 99% used".to_string()
            ]
        );
    }

    #[test]
    fn swap_plus_kernel_warn_is_critical_without_corroboration() {
        let mut input = nominal_input();
        input.swap_used_percent = 99.0;
        input.memory_pressure = MemoryPressureLevel::Warn;
        assert_eq!(classify(&input, &uncorroborated()), PressureLevel::Critical);
    }

    #[test]
    fn swap_alone_is_critical_when_kernel_level_is_unavailable() {
        let mut input = nominal_input();
        input.swap_used_percent = 99.0;
        input.memory_pressure = MemoryPressureLevel::Unknown;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Critical);
    }

    #[test]
    fn low_available_memory_never_needs_corroboration() {
        let mut input = nominal_input();
        input.memory_available_gb = 0.3;
        input.memory_pressure = MemoryPressureLevel::Normal;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Critical);
    }

    #[test]
    fn thermal_never_needs_corroboration() {
        let mut input = nominal_input();
        input.thermal = ThermalState::Serious;
        assert_eq!(classify(&input, &defaults()), PressureLevel::Critical);
    }

    // --- reasons ---

    #[test]
    fn clear_has_no_reasons() {
        let c = classify_with_reasons(&nominal_input(), &defaults());
        assert_eq!(c.level, PressureLevel::Clear);
        assert!(c.reasons.is_empty());
    }

    #[test]
    fn critical_lists_only_the_critical_causes() {
        let mut input = nominal_input();
        input.thermal = ThermalState::Serious; // critical cause
        input.cpu_percent = 75.0; // strained-only cause
        let c = classify_with_reasons(&input, &defaults());
        assert_eq!(c.level, PressureLevel::Critical);
        assert_eq!(c.reasons, vec!["thermal pressure heavy".to_string()]);
    }

    #[test]
    fn corroborated_swap_is_reported_alongside_the_kernel_verdict() {
        let mut input = nominal_input();
        input.memory_pressure = MemoryPressureLevel::Critical;
        input.swap_used_percent = 99.0;
        let c = classify_with_reasons(&input, &defaults());
        assert_eq!(c.level, PressureLevel::Critical);
        assert_eq!(
            c.reasons,
            vec![
                "macOS reports critical memory pressure".to_string(),
                "swap 99% used".to_string()
            ]
        );
    }

    #[test]
    fn strained_lists_every_contributing_cause() {
        let mut input = nominal_input();
        input.cpu_percent = 99.0; // demoted: machine is cool
        input.swap_used_percent = 30.0;
        let c = classify_with_reasons(&input, &defaults());
        assert_eq!(c.level, PressureLevel::Strained);
        assert_eq!(
            c.reasons,
            vec!["swap 30% used".to_string(), "CPU 99%".to_string()]
        );
    }

    // --- exit band ---

    const BAND: f64 = 0.10;

    #[test]
    fn levels_are_ordered_by_severity() {
        assert!(PressureLevel::Clear < PressureLevel::Strained);
        assert!(PressureLevel::Strained < PressureLevel::Critical);
    }

    #[test]
    fn band_does_not_change_entry() {
        // 25% is the strained swap threshold: 24% does not enter from clear, band or no band.
        let mut input = nominal_input();
        input.swap_used_percent = 24.0;
        let c = classify_with_band(&input, &defaults(), PressureLevel::Clear, BAND);
        assert_eq!(c.level, PressureLevel::Clear);
        assert!(c.reasons.is_empty());
    }

    #[test]
    fn value_just_under_threshold_holds_the_level() {
        // Entered strained above 25%; 24% is inside the exit band (leaves below 22.5%).
        let mut input = nominal_input();
        input.swap_used_percent = 24.0;
        let c = classify_with_band(&input, &defaults(), PressureLevel::Strained, BAND);
        assert_eq!(c.level, PressureLevel::Strained);
        assert_eq!(c.reasons, vec!["swap 24% used".to_string()]);
    }

    #[test]
    fn value_past_the_band_releases_the_level() {
        let mut input = nominal_input();
        input.swap_used_percent = 22.0;
        let c = classify_with_band(&input, &defaults(), PressureLevel::Strained, BAND);
        assert_eq!(c.level, PressureLevel::Clear);
    }

    #[test]
    fn low_memory_holds_critical_inside_its_band() {
        // Critical below 1.0 GB; leaves critical only above 1.1 GB.
        let mut input = nominal_input();
        input.memory_available_gb = 1.06;
        let held = classify_with_band(&input, &defaults(), PressureLevel::Critical, BAND);
        assert_eq!(held.level, PressureLevel::Critical);
        assert_eq!(held.reasons, vec!["only 1.1 GB RAM available".to_string()]);

        input.memory_available_gb = 1.2;
        let released = classify_with_band(&input, &defaults(), PressureLevel::Critical, BAND);
        assert_eq!(released.level, PressureLevel::Strained);
    }

    #[test]
    fn band_never_raises_above_the_held_level() {
        // 1.06 GB is inside critical's exit band, but the held level is only strained.
        let mut input = nominal_input();
        input.memory_available_gb = 1.06;
        let c = classify_with_band(&input, &defaults(), PressureLevel::Strained, BAND);
        assert_eq!(c.level, PressureLevel::Strained);
    }

    #[test]
    fn leaving_critical_lands_on_strained_while_inside_the_strained_band() {
        // 2.1 GB: above strained entry (2.0) but inside its exit band (2.2).
        let mut input = nominal_input();
        input.memory_available_gb = 2.1;
        let c = classify_with_band(&input, &defaults(), PressureLevel::Critical, BAND);
        assert_eq!(c.level, PressureLevel::Strained);
        assert!(!c.reasons.is_empty());
    }

    #[test]
    fn escalation_ignores_the_band() {
        let mut input = nominal_input();
        input.thermal = ThermalState::Serious;
        let c = classify_with_band(&input, &defaults(), PressureLevel::Clear, BAND);
        assert_eq!(c.level, PressureLevel::Critical);
    }

    #[test]
    fn zero_band_matches_plain_classification() {
        let mut input = nominal_input();
        input.swap_used_percent = 24.0;
        let c = classify_with_band(&input, &defaults(), PressureLevel::Strained, 0.0);
        assert_eq!(c, classify_with_reasons(&input, &defaults()));
    }
}
