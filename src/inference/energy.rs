// FREE ENERGY — ACTIVE INFERENCE (FRISTON)
//
// The mathematical core of node agency. Each node minimizes its
// free energy by taking actions that reduce prediction error.
//
// F = prediction_error + complexity
//
// prediction_error = (expected_composite - observed_composite)^2
// complexity = KL divergence from uniform prior across confidence dimensions
//
// Design decisions:
// 1. "Expected" composite is 1.0 — every node expects to be fully activated.
//    This is the default generative model. Phase 4 may learn per-node priors.
// 2. KL is computed per confidence dimension against a uniform prior.
// 3. RPE (Reward Prediction Error) signals drive learning rate modulation.
//    Phasic burst → increase learning. Dip → decrease. Baseline → steady.

use crate::confidence::ConfidenceSurface;

/// Free energy of a node given its observed state.
///
/// Lower = better aligned with expectations.
/// Higher = more "surprised" — needs to take action.
#[derive(Debug, Clone)]
pub struct FreeEnergy {
    /// Total free energy (prediction_error + complexity).
    pub total: f64,
    /// Squared difference between expected and observed composite weight.
    pub prediction_error: f64,
    /// KL divergence from uniform prior (complexity penalty).
    pub complexity: f64,
}

impl FreeEnergy {
    /// Compute free energy for a node.
    ///
    /// - `observed_composite`: the node's actual activation weight [0, 1]
    /// - `confidence`: the node's confidence surface
    /// - `expected_composite`: what the node "expects" its weight to be (default 1.0)
    pub fn compute(
        observed_composite: f64,
        confidence: &ConfidenceSurface,
        expected_composite: f64,
    ) -> Self {
        let prediction_error = (expected_composite - observed_composite).powi(2);
        let complexity = kl_from_uniform(confidence);
        FreeEnergy {
            total: prediction_error + complexity,
            prediction_error,
            complexity,
        }
    }
}

/// KL divergence of the confidence surface from a uniform prior.
///
/// For each dimension, KL[Q || P] where P is Uniform(0,1) (mean=0.5, var=0.25).
/// Using a Gaussian approximation:
///   KL[N(μ_q, σ²_q) || N(μ_p, σ²_p)] =
///     0.5 * (σ²_q/σ²_p + (μ_p - μ_q)²/σ²_p - 1 + ln(σ²_p/σ²_q))
///
/// The uniform prior has mean=0.5, variance=0.25 (max entropy for [0,1]).
fn kl_from_uniform(confidence: &ConfidenceSurface) -> f64 {
    let dims = [
        &confidence.comprehension,
        &confidence.resolution,
        &confidence.verification,
    ];

    let prior_mean = 0.5;
    let prior_var = 0.25;

    let mut total_kl = 0.0;
    for dim in &dims {
        // Guard against zero variance (certain distribution).
        let q_var = dim.variance.max(1e-10);
        let kl = 0.5 * (
            q_var / prior_var
            + (prior_mean - dim.mean).powi(2) / prior_var
            - 1.0
            + (prior_var / q_var).ln()
        );
        total_kl += kl.max(0.0); // KL is non-negative
    }

    total_kl
}

/// Reward Prediction Error signal.
///
/// Inspired by dopaminergic signaling (MacDonald et al. 2024).
/// δ = reward + γ * V(next_state) - V(current_state)
///
/// Maps to three signal types that modulate action selection.
#[derive(Debug, Clone, PartialEq)]
pub enum RPESignal {
    /// δ > threshold: unexpected reward. Increase exploration.
    PhasicBurst(f64),
    /// δ < -threshold: unexpected loss. Decrease exploration, consolidate.
    Dip(f64),
    /// |δ| ≤ threshold: as expected. Steady state.
    Baseline(f64),
}

/// Compute RPE signal.
///
/// - `reward`: immediate reward signal (e.g., free energy reduction)
/// - `current_value`: V(current) — estimated value of current state
/// - `next_value`: V(next) — estimated value after action
/// - `gamma`: discount factor [0, 1] (how much future matters)
/// - `threshold`: signal threshold for classifying burst/dip/baseline
pub fn compute_rpe(
    reward: f64,
    current_value: f64,
    next_value: f64,
    gamma: f64,
    threshold: f64,
) -> RPESignal {
    let delta = reward + gamma * next_value - current_value;

    if delta > threshold {
        RPESignal::PhasicBurst(delta)
    } else if delta < -threshold {
        RPESignal::Dip(delta)
    } else {
        RPESignal::Baseline(delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidence::{ConfidenceSurface, Distribution};

    #[test]
    fn free_energy_zero_when_perfect() {
        // Perfect observation, uniform confidence → low FE.
        let cs = ConfidenceSurface::new(); // uniform prior
        let fe = FreeEnergy::compute(1.0, &cs, 1.0);
        // Prediction error is 0, complexity near 0 (at prior).
        assert!(fe.prediction_error < 1e-10);
        assert!(fe.total < 0.1, "FE should be low at prior: {}", fe.total);
    }

    #[test]
    fn free_energy_high_when_surprised() {
        let cs = ConfidenceSurface::new();
        let fe = FreeEnergy::compute(0.0, &cs, 1.0);
        assert!(fe.prediction_error > 0.5, "Should be surprised: PE={}", fe.prediction_error);
        // Total = PE + complexity. At uniform prior, complexity ≈ 0.
        assert!(fe.total >= fe.prediction_error,
            "Total ({:.3}) should be >= PE ({:.3})", fe.total, fe.prediction_error);
    }

    #[test]
    fn complexity_increases_away_from_prior() {
        // Confident surface far from uniform → higher complexity.
        let mut cs = ConfidenceSurface::new();
        cs.comprehension = Distribution::new(0.95, 0.01);
        cs.resolution = Distribution::new(0.95, 0.01);
        cs.verification = Distribution::new(0.95, 0.01);

        let fe_confident = FreeEnergy::compute(1.0, &cs, 1.0);
        let fe_uniform = FreeEnergy::compute(1.0, &ConfidenceSurface::new(), 1.0);

        assert!(fe_confident.complexity > fe_uniform.complexity,
            "Confident ({:.3}) should have more complexity than uniform ({:.3})",
            fe_confident.complexity, fe_uniform.complexity);
    }

    #[test]
    fn rpe_phasic_burst_on_positive_surprise() {
        let signal = compute_rpe(1.0, 0.5, 0.5, 0.9, 0.1);
        match signal {
            RPESignal::PhasicBurst(d) => assert!(d > 0.0),
            _ => panic!("Expected PhasicBurst, got {:?}", signal),
        }
    }

    #[test]
    fn rpe_dip_on_negative_surprise() {
        let signal = compute_rpe(-1.0, 0.5, 0.5, 0.9, 0.1);
        match signal {
            RPESignal::Dip(d) => assert!(d < 0.0),
            _ => panic!("Expected Dip, got {:?}", signal),
        }
    }

    #[test]
    fn rpe_baseline_when_expected() {
        let signal = compute_rpe(0.5, 0.5, 0.0, 0.9, 0.1);
        match signal {
            RPESignal::Baseline(d) => assert!(d.abs() <= 0.1),
            _ => panic!("Expected Baseline, got {:?}", signal),
        }
    }

    #[test]
    fn kl_at_prior_is_near_zero() {
        let cs = ConfidenceSurface::new(); // mean=0.5, var=0.25 = uniform prior
        let kl = kl_from_uniform(&cs);
        assert!(kl < 0.01, "KL at prior should be near 0: {}", kl);
    }

    #[test]
    fn kl_increases_with_certainty() {
        let mut cs_certain = ConfidenceSurface::new();
        cs_certain.comprehension = Distribution::certain(1.0);

        let kl_certain = kl_from_uniform(&cs_certain);
        let kl_uniform = kl_from_uniform(&ConfidenceSurface::new());

        assert!(kl_certain > kl_uniform,
            "Certain ({:.3}) should have higher KL than uniform ({:.3})",
            kl_certain, kl_uniform);
    }
}
