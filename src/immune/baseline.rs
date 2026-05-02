// BASELINE TRACKER — Welford online mean + variance accumulator
//
// Used by every cell-agent specialization (Spec 6 §3) to maintain a
// 1-D moving baseline cheaply. Spec 5 §5.5.2 specifies Ledoit-Wolf
// shrinkage for K-dimensional covariance — that lands in Step 3 for
// the cognitive map's per-region state vectors. v1 cell-agent
// specializations track scalar metrics (rate, failure ratio,
// throughput, edge density) so 1-D Welford is the right tool here.
//
// Welford (1962) is the textbook stable online algorithm: numerically
// well-behaved, O(1) per update, no growth-with-N memory. The variance
// is the population variance unless you explicitly pull `sample_variance`
// (which divides by n-1).

use std::time::Duration;

/// Online mean + variance via Welford's algorithm. Numerically stable
/// for arbitrary observation streams.
#[derive(Debug, Clone)]
pub struct WelfordTracker {
    n: u64,
    mean: f64,
    /// Sum of squared deviations from the running mean (the "M2"
    /// quantity in Welford's algorithm).
    m2: f64,
}

impl WelfordTracker {
    pub fn new() -> Self {
        Self { n: 0, mean: 0.0, m2: 0.0 }
    }

    /// Reset to the empty state.
    pub fn reset(&mut self) {
        self.n = 0;
        self.mean = 0.0;
        self.m2 = 0.0;
    }

    /// Construct from a captured snapshot — used by Spec 6 Step 7
    /// baseline inheritance across cell-agent generations.
    pub fn from_snapshot(n: u64, mean: f64, m2: f64) -> Self {
        Self { n, mean, m2 }
    }

    /// Snapshot for `BaselineSnapshot` serialization.
    pub fn snapshot(&self) -> (u64, f64, f64) {
        (self.n, self.mean, self.m2)
    }

    /// Number of observations folded in.
    pub fn count(&self) -> u64 {
        self.n
    }

    pub fn mean(&self) -> f64 {
        self.mean
    }

    /// Population variance (`m2 / n`). Returns 0.0 for n < 1.
    pub fn variance(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.m2 / self.n as f64
        }
    }

    /// Sample variance (`m2 / (n - 1)`). Returns 0.0 for n < 2.
    pub fn sample_variance(&self) -> f64 {
        if self.n < 2 {
            0.0
        } else {
            self.m2 / (self.n - 1) as f64
        }
    }

    pub fn stddev(&self) -> f64 {
        self.variance().sqrt()
    }

    pub fn sample_stddev(&self) -> f64 {
        self.sample_variance().sqrt()
    }

    /// Fold a new observation into the running mean + M2.
    pub fn observe(&mut self, x: f64) {
        self.n += 1;
        let delta = x - self.mean;
        self.mean += delta / self.n as f64;
        let delta2 = x - self.mean;
        self.m2 += delta * delta2;
    }

    /// Z-score of `x` against the running baseline.
    ///
    /// Standard form `(x - mean) / stddev` when `stddev > 0`. When
    /// stddev is exactly 0.0 (degenerate baseline — every observation
    /// so far has been identical), returns:
    ///   - `0.0` if `x == mean` exactly (the new observation matches
    ///     the constant baseline)
    ///   - `Z_DEGENERATE_BASELINE` (signed) otherwise — operationally
    ///     "this observation is impossibly different from the constant
    ///     baseline." A finite sentinel (rather than `f64::INFINITY`)
    ///     keeps downstream comparisons (`abs() >= threshold`) clean.
    ///
    /// Callers that want to avoid false-positive triggering on a green
    /// baseline should require `count() >= warmup_n` before consulting
    /// the z-score.
    pub fn z_score(&self, x: f64) -> f64 {
        // No observations folded yet — there is no baseline to deviate
        // from, so emit a neutral 0.0. Cell-agents already gate their
        // thresholds on `count() >= warmup`, so this is also safe.
        if self.n == 0 {
            return 0.0;
        }
        let s = self.stddev();
        if s == 0.0 {
            if x == self.mean {
                0.0
            } else {
                Z_DEGENERATE_BASELINE.copysign(x - self.mean)
            }
        } else {
            (x - self.mean) / s
        }
    }
}

/// Sentinel z-score returned when stddev is 0.0 and the observation
/// differs from the mean. Large enough to dwarf any cell-agent's
/// configured threshold (5σ, 3σ, 2σ); finite to keep arithmetic safe.
pub const Z_DEGENERATE_BASELINE: f64 = 1.0e6;

impl Default for WelfordTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper to convert a wall-clock window into a per-second rate.
pub fn rate_per_second(events: u64, window: Duration) -> f64 {
    let secs = window.as_secs_f64();
    if secs <= 0.0 {
        0.0
    } else {
        events as f64 / secs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_tracker_reports_zero_stats() {
        let t = WelfordTracker::new();
        assert_eq!(t.count(), 0);
        assert_eq!(t.mean(), 0.0);
        assert_eq!(t.variance(), 0.0);
        assert_eq!(t.stddev(), 0.0);
        assert_eq!(t.z_score(42.0), 0.0);
    }

    #[test]
    fn single_observation_has_zero_variance() {
        let mut t = WelfordTracker::new();
        t.observe(7.0);
        assert_eq!(t.count(), 1);
        assert!((t.mean() - 7.0).abs() < 1e-12);
        assert_eq!(t.variance(), 0.0);
    }

    #[test]
    fn welford_matches_textbook_population_variance() {
        // mean of 2,4,4,4,5,5,7,9 = 5; population variance = 4
        let mut t = WelfordTracker::new();
        for x in [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0] {
            t.observe(x);
        }
        assert!((t.mean() - 5.0).abs() < 1e-12);
        assert!((t.variance() - 4.0).abs() < 1e-12);
        // sample variance = m2 / (n-1) = 32/7
        assert!((t.sample_variance() - 32.0 / 7.0).abs() < 1e-12);
    }

    #[test]
    fn z_score_matches_manual_computation() {
        let mut t = WelfordTracker::new();
        for x in [10.0, 12.0, 14.0, 16.0, 18.0] {
            t.observe(x);
        }
        // mean = 14; population variance = ((-4)^2 + (-2)^2 + 0 + 2^2 + 4^2)/5 = 8
        // stddev = sqrt(8) ≈ 2.828
        let z = t.z_score(20.0);
        assert!((z - (20.0 - 14.0) / 8.0_f64.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn snapshot_roundtrip_preserves_state() {
        let mut t = WelfordTracker::new();
        for x in [1.0, 2.0, 3.0, 4.0, 5.0] {
            t.observe(x);
        }
        let (n, mean, m2) = t.snapshot();
        let restored = WelfordTracker::from_snapshot(n, mean, m2);
        assert_eq!(restored.count(), t.count());
        assert!((restored.mean() - t.mean()).abs() < 1e-12);
        assert!((restored.variance() - t.variance()).abs() < 1e-12);
    }

    #[test]
    fn rate_per_second_handles_zero_window() {
        assert_eq!(rate_per_second(100, Duration::from_secs(0)), 0.0);
        assert_eq!(rate_per_second(100, Duration::from_secs(10)), 10.0);
    }

    #[test]
    fn reset_clears_state() {
        let mut t = WelfordTracker::new();
        for x in [1.0, 2.0, 3.0] {
            t.observe(x);
        }
        t.reset();
        assert_eq!(t.count(), 0);
        assert_eq!(t.mean(), 0.0);
    }
}
