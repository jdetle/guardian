use crate::config::ThresholdConfig;
use crate::sampler::thermal::ThermalState;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
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
}

fn memory_available_ratio(input: &ClassifierInput) -> f64 {
    if input.memory_total_gb <= 0.01 {
        return 1.0;
    }
    input.memory_available_gb / input.memory_total_gb
}

pub fn classify(input: &ClassifierInput, cfg: &ThresholdConfig) -> PressureLevel {
    let ratio = memory_available_ratio(input);

    let ratio_critical = cfg
        .critical_memory_available_ratio
        .is_some_and(|r| ratio < r);
    let ratio_strained = cfg
        .strained_memory_available_ratio
        .is_some_and(|r| ratio < r);

    if input.cpu_percent > cfg.critical_cpu_percent
        || input.memory_available_gb < cfg.critical_memory_gb
        || ratio_critical
        || input.swap_used_percent > cfg.critical_swap_percent
        || input.thermal == ThermalState::Critical
        || input.thermal == ThermalState::Serious
        || input.process_usage_ratio > 0.8
    {
        return PressureLevel::Critical;
    }

    if input.cpu_percent > cfg.strained_cpu_percent
        || input.memory_available_gb < cfg.strained_memory_gb
        || ratio_strained
        || input.swap_used_percent > cfg.strained_swap_percent
        || input.thermal == ThermalState::Fair
        || input.process_usage_ratio > 0.6
    {
        return PressureLevel::Strained;
    }

    PressureLevel::Clear
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThresholdConfig;
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
        assert_eq!(classify(&input, &defaults()), PressureLevel::Critical);
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
        assert_eq!(classify(&input, &defaults()), PressureLevel::Critical);
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
}
