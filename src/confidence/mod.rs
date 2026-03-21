// CONFIDENCE SURFACE
//
// Design decisions:
// 1. Three dimensions: comprehension, resolution, verification
// 2. Each dimension is a Distribution, not a float — the system maintains
//    uncertainty about its own confidence
// 3. Confidence is a STORED FIELD that contributes to composite activation weight.
//    It is NOT the whole story — other weights (resonance, recency, connectivity)
//    also contribute. The composite weight is derived, not stored.
// 4. Each dimension varies across time and the fabric's semantic dimensions.
//    For Phase 1, we represent the current state. Temporal trajectory comes in Phase 2
//    when the fabric exists to embed the node within.
//
// Open questions flagged for review:
// - How many samples in a distribution is sufficient for early prototype?
// - Should distributions be parametric (e.g., beta distribution) or non-parametric?
//   Decision: Start with a simple weighted mean + variance representation.
//   A beta distribution would be ideal (bounded [0,1], conjugate prior for Bayesian updates)
//   but adds complexity. We can migrate later.

use std::fmt;

/// A probability distribution over [0.0, 1.0].
/// Phase 1: Represented as mean + variance.
/// Future: Could become beta distribution, histogram, or full density.
#[derive(Debug, Clone, PartialEq)]
pub struct Distribution {
    /// Central tendency of the distribution
    pub mean: f64,
    /// Spread — how uncertain we are about the mean itself
    pub variance: f64,
    /// Number of observations that contributed to this distribution.
    /// More observations = more settled. Zero = prior only.
    pub observations: u64,
}

impl Distribution {
    pub fn new(mean: f64, variance: f64) -> Self {
        assert!((0.0..=1.0).contains(&mean), "Distribution mean must be in [0, 1]");
        assert!(variance >= 0.0, "Variance must be non-negative");
        Self {
            mean,
            variance,
            observations: 0,
        }
    }

    /// Maximum ignorance — we know nothing.
    /// Mean 0.5 (could go either way), high variance.
    pub fn unknown() -> Self {
        Self {
            mean: 0.5,
            variance: 0.25, // Maximum variance for [0,1] is 0.25
            observations: 0,
        }
    }

    /// Full certainty at a specific value.
    pub fn certain(value: f64) -> Self {
        assert!((0.0..=1.0).contains(&value));
        Self {
            mean: value,
            variance: 0.0,
            observations: u64::MAX, // Axiomatic certainty
        }
    }

    /// Update the distribution with a new observation.
    /// Uses incremental Bayesian-style update: more observations → less movement.
    pub fn observe(&mut self, value: f64) {
        assert!((0.0..=1.0).contains(&value));
        self.observations = self.observations.saturating_add(1);
        let n = self.observations as f64;
        // Incremental mean update
        let old_mean = self.mean;
        self.mean += (value - old_mean) / n;
        // Incremental variance update (Welford's algorithm adapted)
        self.variance = ((n - 1.0) * self.variance + (value - old_mean) * (value - self.mean)) / n;
        // Clamp to valid range
        self.mean = self.mean.clamp(0.0, 1.0);
    }

    /// Is this distribution settled enough to act on?
    /// High mean + low variance = confident and sure about it.
    pub fn is_actionable(&self, threshold: f64) -> bool {
        self.mean >= threshold && self.variance < 0.1
    }
}

impl fmt::Display for Distribution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3} ±{:.3} (n={})", self.mean, self.variance.sqrt(), self.observations)
    }
}

/// The three-dimensional confidence surface.
///
/// Each dimension answers a different question:
/// - Comprehension: "Do I understand what is wanted?"
/// - Resolution: "Can I achieve what is wanted?"
/// - Verification: "Did the outcome match what was wanted?"
///
/// This is ONE contributor to the node's composite activation weight.
/// Other contributors include semantic resonance, temporal recency,
/// contextual connectivity, etc.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfidenceSurface {
    /// How well the system understands the intent.
    /// Low → must clarify before executing. Vagueness surfaced here.
    pub comprehension: Distribution,

    /// Given understanding, can the system achieve the desired state?
    /// High comprehension + low resolution = "I understand but can't deliver."
    pub resolution: Distribution,

    /// After execution, did outcome match want?
    /// Starts unknown, collapses after verification.
    pub verification: Distribution,
}

impl ConfidenceSurface {
    /// Fresh confidence surface — nothing known yet.
    pub fn new() -> Self {
        Self {
            comprehension: Distribution::unknown(),
            resolution: Distribution::unknown(),
            verification: Distribution::unknown(),
        }
    }

    /// High comprehension, resolution and verification unknown.
    /// The common starting state when intent is clearly expressed.
    pub fn understood(comprehension: f64) -> Self {
        Self {
            comprehension: Distribution::new(comprehension, 0.05),
            resolution: Distribution::unknown(),
            verification: Distribution::unknown(),
        }
    }

    /// Should the system proceed with resolution?
    /// Only if comprehension is above threshold.
    /// This is Law 4: Vagueness is Surfaced.
    pub fn should_resolve(&self, comprehension_threshold: f64) -> bool {
        self.comprehension.is_actionable(comprehension_threshold)
    }

    /// Combined confidence signal — a single scalar summary.
    /// This is a LOSSY projection from the full surface.
    /// Used only when a single number is needed for comparison.
    /// NOT used for decision-making — use the individual dimensions.
    pub fn scalar_summary(&self) -> f64 {
        // Geometric mean — all three must be reasonably high
        // for the summary to be high
        (self.comprehension.mean * self.resolution.mean * self.verification.mean).cbrt()
    }
}

impl Default for ConfidenceSurface {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ConfidenceSurface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Confidence [ C: {} | R: {} | V: {} ]",
            self.comprehension, self.resolution, self.verification
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_distribution_is_maximally_uncertain() {
        let d = Distribution::unknown();
        assert_eq!(d.mean, 0.5);
        assert_eq!(d.variance, 0.25);
        assert_eq!(d.observations, 0);
    }

    #[test]
    fn certain_distribution_has_zero_variance() {
        let d = Distribution::certain(0.9);
        assert_eq!(d.variance, 0.0);
        assert_eq!(d.mean, 0.9);
    }

    #[test]
    fn observation_moves_distribution() {
        let mut d = Distribution::unknown();
        d.observe(1.0);
        assert!(d.mean > 0.5, "Mean should move toward observation");
        assert_eq!(d.observations, 1);
    }

    #[test]
    fn many_observations_settle_distribution() {
        let mut d = Distribution::unknown();
        for _ in 0..100 {
            d.observe(0.8);
        }
        assert!((d.mean - 0.8).abs() < 0.05, "Mean should converge to observed value");
        assert!(d.variance < 0.01, "Variance should shrink with consistent observations");
    }

    #[test]
    fn vagueness_prevents_resolution() {
        let surface = ConfidenceSurface::new();
        assert!(!surface.should_resolve(0.7), "Unknown comprehension should block resolution");
    }

    #[test]
    fn clear_comprehension_allows_resolution() {
        let surface = ConfidenceSurface::understood(0.9);
        assert!(surface.should_resolve(0.7), "High comprehension should allow resolution");
    }

    #[test]
    fn scalar_summary_requires_all_dimensions() {
        let mut surface = ConfidenceSurface::new();
        surface.comprehension = Distribution::certain(1.0);
        surface.resolution = Distribution::certain(1.0);
        surface.verification = Distribution::certain(0.0);
        assert!(surface.scalar_summary() < 0.01, "Zero in any dimension should collapse summary");
    }
}
