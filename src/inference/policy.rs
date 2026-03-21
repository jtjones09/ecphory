// ACTION POLICY — LEARNABLE THRESHOLDS (Phase 4b)
//
// Replaces fixed constants in action selection with a mutable policy
// that adapts from Reward Prediction Error (RPE) signals.
//
// Design decisions:
// 1. ActionPolicy is a simple struct with public thresholds.
// 2. update_from_rpe() adjusts thresholds based on signal type:
//    - PhasicBurst → lower thresholds (explore more, take more actions)
//    - Dip → raise thresholds (conserve, take fewer actions)
//    - Baseline → small decay toward defaults (homeostasis)
// 3. Thresholds are clamped to valid ranges to prevent runaway drift.
// 4. Learning rate controls adaptation speed. Default 0.05.
// 5. Backward compatible: default_policy() matches Phase 3c constants.

use super::energy::RPESignal;

/// Mutable action selection policy.
///
/// Thresholds adapt from RPE signals. Replaces the fixed constants
/// from Phase 3c with learnable parameters.
#[derive(Debug, Clone)]
pub struct ActionPolicy {
    /// Free energy below which no action is needed.
    pub fe_threshold: f64,
    /// Comprehension below which RequestClarification fires.
    pub low_comprehension: f64,
    /// Comprehension above which SignalResolution can fire.
    pub high_comprehension: f64,
    /// Resolution confidence above which SignalResolution can fire.
    pub high_resolution: f64,
    /// Context edge count below which CreateEdge fires.
    pub min_context_edges: usize,
    /// How fast thresholds adapt (0.0 = frozen, 1.0 = fully reactive).
    pub learning_rate: f64,
}

/// Default threshold values — match Phase 3c constants exactly.
const DEFAULT_FE_THRESHOLD: f64 = 0.1;
const DEFAULT_LOW_COMPREHENSION: f64 = 0.4;
const DEFAULT_HIGH_COMPREHENSION: f64 = 0.7;
const DEFAULT_HIGH_RESOLUTION: f64 = 0.7;
const DEFAULT_MIN_CONTEXT_EDGES: usize = 2;
const DEFAULT_LEARNING_RATE: f64 = 0.05;

impl ActionPolicy {
    /// Create the default policy (matches Phase 3c fixed constants).
    pub fn default_policy() -> Self {
        Self {
            fe_threshold: DEFAULT_FE_THRESHOLD,
            low_comprehension: DEFAULT_LOW_COMPREHENSION,
            high_comprehension: DEFAULT_HIGH_COMPREHENSION,
            high_resolution: DEFAULT_HIGH_RESOLUTION,
            min_context_edges: DEFAULT_MIN_CONTEXT_EDGES,
            learning_rate: DEFAULT_LEARNING_RATE,
        }
    }

    /// Update thresholds based on an RPE signal.
    ///
    /// PhasicBurst (unexpected reward): the fabric is learning —
    ///   lower thresholds to take more actions (explore).
    ///
    /// Dip (unexpected loss): actions aren't helping —
    ///   raise thresholds to take fewer actions (conserve).
    ///
    /// Baseline (expected): small decay toward defaults (homeostasis).
    pub fn update_from_rpe(&mut self, signal: &RPESignal) {
        let lr = self.learning_rate;

        match signal {
            RPESignal::PhasicBurst(delta) => {
                // Positive surprise → lower thresholds (be more exploratory).
                let magnitude = delta.abs().min(1.0);
                self.fe_threshold -= lr * magnitude * DEFAULT_FE_THRESHOLD;
                self.low_comprehension -= lr * magnitude * DEFAULT_LOW_COMPREHENSION;
                self.high_comprehension -= lr * magnitude * DEFAULT_HIGH_COMPREHENSION;
                self.high_resolution -= lr * magnitude * DEFAULT_HIGH_RESOLUTION;
            }
            RPESignal::Dip(delta) => {
                // Negative surprise → raise thresholds (be more conservative).
                let magnitude = delta.abs().min(1.0);
                self.fe_threshold += lr * magnitude * DEFAULT_FE_THRESHOLD;
                self.low_comprehension += lr * magnitude * DEFAULT_LOW_COMPREHENSION;
                self.high_comprehension += lr * magnitude * DEFAULT_HIGH_COMPREHENSION;
                self.high_resolution += lr * magnitude * DEFAULT_HIGH_RESOLUTION;
            }
            RPESignal::Baseline(_) => {
                // Expected outcome → decay toward defaults (homeostasis).
                let decay = lr * 0.1; // Slow decay
                self.fe_threshold += (DEFAULT_FE_THRESHOLD - self.fe_threshold) * decay;
                self.low_comprehension += (DEFAULT_LOW_COMPREHENSION - self.low_comprehension) * decay;
                self.high_comprehension += (DEFAULT_HIGH_COMPREHENSION - self.high_comprehension) * decay;
                self.high_resolution += (DEFAULT_HIGH_RESOLUTION - self.high_resolution) * decay;
            }
        }

        self.clamp();
    }

    /// Clamp all thresholds to valid ranges.
    fn clamp(&mut self) {
        // FE threshold: [0.01, 1.0] — never zero (would suppress all actions),
        // never above 1.0 (would make everything seem fine).
        self.fe_threshold = self.fe_threshold.clamp(0.01, 1.0);

        // Comprehension thresholds: [0.1, 0.95] — reasonable operating range.
        self.low_comprehension = self.low_comprehension.clamp(0.1, 0.95);
        self.high_comprehension = self.high_comprehension.clamp(0.1, 0.95);

        // Ensure low < high for comprehension.
        if self.low_comprehension >= self.high_comprehension {
            // Push them apart: low stays at floor, high gets bumped up.
            self.low_comprehension = 0.1;
            self.high_comprehension = (self.low_comprehension + 0.1).min(0.95);
        }

        // Resolution confidence: [0.1, 0.95].
        self.high_resolution = self.high_resolution.clamp(0.1, 0.95);
    }
}

impl Default for ActionPolicy {
    fn default() -> Self {
        Self::default_policy()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_matches_constants() {
        let p = ActionPolicy::default_policy();
        assert!((p.fe_threshold - 0.1).abs() < 1e-10);
        assert!((p.low_comprehension - 0.4).abs() < 1e-10);
        assert!((p.high_comprehension - 0.7).abs() < 1e-10);
        assert!((p.high_resolution - 0.7).abs() < 1e-10);
        assert_eq!(p.min_context_edges, 2);
        assert!((p.learning_rate - 0.05).abs() < 1e-10);
    }

    #[test]
    fn phasic_burst_lowers_thresholds() {
        let mut p = ActionPolicy::default_policy();
        let original_fe = p.fe_threshold;
        let original_low = p.low_comprehension;
        p.update_from_rpe(&RPESignal::PhasicBurst(0.5));
        assert!(p.fe_threshold < original_fe, "FE threshold should decrease");
        assert!(p.low_comprehension < original_low, "Low comprehension should decrease");
    }

    #[test]
    fn dip_raises_thresholds() {
        let mut p = ActionPolicy::default_policy();
        let original_fe = p.fe_threshold;
        let original_low = p.low_comprehension;
        p.update_from_rpe(&RPESignal::Dip(-0.5));
        assert!(p.fe_threshold > original_fe, "FE threshold should increase");
        assert!(p.low_comprehension > original_low, "Low comprehension should increase");
    }

    #[test]
    fn baseline_decays_toward_defaults() {
        let mut p = ActionPolicy::default_policy();
        // Perturb away from defaults.
        p.fe_threshold = 0.5;
        p.low_comprehension = 0.8;
        let before_fe = p.fe_threshold;
        let before_low = p.low_comprehension;

        p.update_from_rpe(&RPESignal::Baseline(0.0));

        // Should move toward defaults.
        assert!((p.fe_threshold - DEFAULT_FE_THRESHOLD).abs() < (before_fe - DEFAULT_FE_THRESHOLD).abs(),
            "FE threshold should move toward default");
        assert!((p.low_comprehension - DEFAULT_LOW_COMPREHENSION).abs() < (before_low - DEFAULT_LOW_COMPREHENSION).abs(),
            "Low comprehension should move toward default");
    }

    #[test]
    fn thresholds_clamp_to_valid_range() {
        let mut p = ActionPolicy::default_policy();
        // Extreme burst — should not go below minimums.
        for _ in 0..100 {
            p.update_from_rpe(&RPESignal::PhasicBurst(1.0));
        }
        assert!(p.fe_threshold >= 0.01, "FE threshold should not go below 0.01");
        assert!(p.low_comprehension >= 0.1, "Low comprehension should not go below 0.1");
        assert!(p.low_comprehension < p.high_comprehension, "Low must stay below high");

        // Extreme dip — should not go above maximums.
        let mut p2 = ActionPolicy::default_policy();
        for _ in 0..100 {
            p2.update_from_rpe(&RPESignal::Dip(-1.0));
        }
        assert!(p2.fe_threshold <= 1.0, "FE threshold should not exceed 1.0");
        assert!(p2.high_comprehension <= 0.95, "High comprehension should not exceed 0.95");
    }
}
