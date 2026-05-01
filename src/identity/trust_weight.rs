// TRUST WEIGHT — Behavioral trust accumulation and decay (Spec 5 §5.5.4)
//
// Per spec §5.5.4:
// - Starts at 0.0 (first contact) or 0.1 (operator-provisioned)
// - Accumulates asymptotically toward 1.0 — never reaches 1.0
// - Decays exponentially with inactivity (half-life 30 days)
// - Penalized by anomaly observations proportional to severity
// - Consumed by Spec 6's immune system; the data structure and math
//   live here in the identity primitives.
//
// Properties verified by tests:
// - Accumulation approaches 1.0 asymptotically (never reaches it).
// - Decay halves trust in 30 days of inactivity.
// - Penalty reduces trust proportional to severity.

use crate::temporal::FabricInstant;

/// Mathematical model of behavioral trust for an agent or federated peer.
///
/// "Trust is earned through observed consistency, not through credential
/// exchange." (Spec 5 §2.3.2)
#[derive(Debug, Clone)]
pub struct TrustWeight {
    /// Current trust value, in [0.0, 1.0).
    pub value: f64,
    /// Wall-clock instant of the last observation (for decay computation).
    pub last_observation: FabricInstant,
    /// How many observations have classified the subject as healthy.
    pub healthy_observation_count: u64,
    /// Total observations (healthy + anomaly).
    pub total_observation_count: u64,
}

impl TrustWeight {
    /// Initial weight for an unknown federation peer (first contact).
    pub const INITIAL_FIRST_CONTACT: f64 = 0.0;
    /// Initial weight for an agent the operator just provisioned.
    pub const INITIAL_OPERATOR_PROVISIONED: f64 = 0.1;
    /// Per-observation accumulation rate. Slow on purpose — trust earns,
    /// it doesn't get handed out.
    pub const LEARNING_RATE: f64 = 0.01;
    /// Trust half-life: weight halves after 30 days of no observations.
    pub const TRUST_HALF_LIFE_DAYS: f64 = 30.0;

    /// Construct a fresh weight for a first-contact peer.
    pub fn first_contact() -> Self {
        Self {
            value: Self::INITIAL_FIRST_CONTACT,
            last_observation: FabricInstant::now(),
            healthy_observation_count: 0,
            total_observation_count: 0,
        }
    }

    /// Construct a fresh weight for an operator-provisioned agent.
    pub fn operator_provisioned() -> Self {
        Self {
            value: Self::INITIAL_OPERATOR_PROVISIONED,
            last_observation: FabricInstant::now(),
            healthy_observation_count: 0,
            total_observation_count: 0,
        }
    }

    /// Internal: apply exponential decay since `last_observation` to `now`.
    /// Half-life is `TRUST_HALF_LIFE_DAYS`. Pure function over (value, dt).
    fn apply_decay(value: f64, elapsed_days: f64) -> f64 {
        // Standard half-life decay: weight *= exp(-ln(2) * dt / half_life).
        let factor = (-2f64.ln() * elapsed_days / Self::TRUST_HALF_LIFE_DAYS).exp();
        value * factor
    }

    /// Called after each healthy observation cycle.
    ///
    /// 1. Decay for elapsed time since last observation.
    /// 2. Accumulate toward 1.0 asymptotically: `value += rate * (1 - value)`.
    pub fn accumulate(&mut self, now: FabricInstant) {
        let elapsed_days =
            (now.elapsed_secs() - self.last_observation.elapsed_secs()).max(0.0) / 86400.0;
        self.value = Self::apply_decay(self.value, elapsed_days);
        self.value += Self::LEARNING_RATE * (1.0 - self.value);
        self.healthy_observation_count += 1;
        self.total_observation_count += 1;
        self.last_observation = now;
    }

    /// Called after an anomaly observation. Severity in [0.0, 1.0]:
    /// 0.0 → no penalty; 1.0 → trust reduced to zero.
    pub fn penalize(&mut self, severity: f64) {
        let s = severity.clamp(0.0, 1.0);
        self.value *= 1.0 - s;
        self.total_observation_count += 1;
    }

    /// Decay-only update — useful for batch retuning when no observation
    /// has happened but time has advanced.
    pub fn decay_to(&mut self, now: FabricInstant) {
        let elapsed_days =
            (now.elapsed_secs() - self.last_observation.elapsed_secs()).max(0.0) / 86400.0;
        self.value = Self::apply_decay(self.value, elapsed_days);
        self.last_observation = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FabricInstant uses real wall-clock time, but exposes
    /// `with_age_secs` to fast-forward elapsed time for tests.
    /// We craft a "now" that appears `days` later than the original by
    /// subtracting `days` from the snapshot's `last_observation` age:
    /// since `elapsed_secs(now) - elapsed_secs(last_obs)` is what we use,
    /// giving `last_observation` a NEGATIVE age (younger than now) is
    /// equivalent to giving `now` a POSITIVE offset from it.
    fn instant_now_at_offset_days(days: f64) -> FabricInstant {
        FabricInstant::with_age_secs(days * 86400.0)
    }

    #[test]
    fn first_contact_starts_at_zero() {
        let tw = TrustWeight::first_contact();
        assert_eq!(tw.value, 0.0);
    }

    #[test]
    fn operator_provisioned_starts_at_point_one() {
        let tw = TrustWeight::operator_provisioned();
        assert!((tw.value - 0.1).abs() < 1e-12);
    }

    #[test]
    fn accumulation_approaches_one_asymptotically() {
        let mut tw = TrustWeight::first_contact();
        // Run many healthy observations back-to-back at the same wall-clock.
        for _ in 0..2000 {
            tw.accumulate(FabricInstant::now());
        }
        assert!(tw.value < 1.0,
            "Trust must never reach 1.0 — always asymptotic (Spec 5 §5.5.4).");
        assert!(tw.value > 0.99,
            "After many healthy observations, trust should be near 1.0; got {}",
            tw.value);
    }

    #[test]
    fn accumulation_increments_counts() {
        let mut tw = TrustWeight::first_contact();
        for _ in 0..3 {
            tw.accumulate(FabricInstant::now());
        }
        assert_eq!(tw.healthy_observation_count, 3);
        assert_eq!(tw.total_observation_count, 3);
    }

    #[test]
    fn decay_halves_trust_in_thirty_days() {
        // Build a TrustWeight that's already at value 1.0, then decay for
        // exactly the half-life.
        let mut tw = TrustWeight::operator_provisioned();
        tw.value = 1.0;
        // last_observation is FabricInstant::now() (≈0s old).
        let later = instant_now_at_offset_days(30.0);
        tw.decay_to(later);

        assert!((tw.value - 0.5).abs() < 1e-3,
            "After 30 days (half-life), trust should be ~0.5; got {}",
            tw.value);
    }

    #[test]
    fn decay_halves_again_after_two_half_lives() {
        let mut tw = TrustWeight::operator_provisioned();
        tw.value = 1.0;
        let later = instant_now_at_offset_days(60.0);
        tw.decay_to(later);

        assert!((tw.value - 0.25).abs() < 1e-3,
            "After 60 days (two half-lives), trust should be ~0.25; got {}",
            tw.value);
    }

    #[test]
    fn penalty_reduces_trust_proportional_to_severity() {
        let mut tw = TrustWeight::operator_provisioned();
        tw.value = 0.8;

        tw.penalize(0.5);
        assert!((tw.value - 0.4).abs() < 1e-12,
            "Severity 0.5 must halve trust (1 - 0.5) * 0.8 = 0.4; got {}",
            tw.value);
    }

    #[test]
    fn penalty_severity_one_zeroes_trust() {
        let mut tw = TrustWeight::operator_provisioned();
        tw.value = 0.9;
        tw.penalize(1.0);
        assert_eq!(tw.value, 0.0);
    }

    #[test]
    fn penalty_severity_zero_is_a_noop() {
        let mut tw = TrustWeight::operator_provisioned();
        tw.value = 0.5;
        let before = tw.value;
        tw.penalize(0.0);
        assert!((tw.value - before).abs() < 1e-12);
    }

    #[test]
    fn penalty_increments_total_but_not_healthy() {
        let mut tw = TrustWeight::operator_provisioned();
        tw.penalize(0.1);
        assert_eq!(tw.total_observation_count, 1);
        assert_eq!(tw.healthy_observation_count, 0);
    }
}
