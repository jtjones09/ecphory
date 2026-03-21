// TEMPORAL PRIMITIVES — LAW 11: TIME IS A DIMENSION
//
// Time is not metadata stamped on static objects.
// It is a fundamental dimension of node existence.
//
// Phase 2: Lamport clock for causal ordering (single process).
// Phase 3: Vector clocks for distributed consistency.
// The shovel, not the building.

use std::fmt;

/// Lamport logical clock for causal ordering.
///
/// Every fabric operation ticks the clock.
/// Timestamps are totally ordered within a single process.
/// Phase 3 replaces with vector clocks for distributed ordering.
#[derive(Debug, Clone)]
pub struct LamportClock {
    counter: u64,
}

impl LamportClock {
    pub fn new() -> Self {
        Self { counter: 0 }
    }

    /// Reconstruct a clock from a persisted value.
    /// Used by the persistence layer during deserialization.
    pub fn from_value(value: u64) -> Self {
        Self { counter: value }
    }

    /// Tick on local event. Returns the new timestamp.
    pub fn tick(&mut self) -> LamportTimestamp {
        self.counter += 1;
        LamportTimestamp(self.counter)
    }

    /// Merge with a received timestamp (max + 1).
    /// Used when synchronizing with another clock.
    pub fn receive(&mut self, other: LamportTimestamp) -> LamportTimestamp {
        self.counter = self.counter.max(other.0) + 1;
        LamportTimestamp(self.counter)
    }

    /// Current value without ticking.
    pub fn current(&self) -> LamportTimestamp {
        LamportTimestamp(self.counter)
    }
}

impl Default for LamportClock {
    fn default() -> Self {
        Self::new()
    }
}

/// An opaque timestamp from a Lamport clock.
/// Totally ordered. Copy-able. Display-able.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LamportTimestamp(u64);

impl LamportTimestamp {
    /// Create a timestamp from a raw value.
    /// Used by the persistence layer during deserialization.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// Raw value (for serialization / debugging).
    pub fn value(&self) -> u64 {
        self.0
    }
}

impl fmt::Display for LamportTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t={}", self.0)
    }
}

/// Wall-clock instant wrapper for temporal decay calculations.
///
/// BOOTSTRAP TOOL — uses std::time::Instant internally.
/// NOT used for causal ordering (that's LamportClock's job).
/// Used only for computing temporal decay weights.
///
/// Phase 3 may replace with fabric-native temporal representation.
#[derive(Debug, Clone)]
pub struct FabricInstant {
    created: std::time::Instant,
    /// Artificial age offset in seconds (for testing).
    age_offset_secs: f64,
}

impl FabricInstant {
    pub fn now() -> Self {
        Self {
            created: std::time::Instant::now(),
            age_offset_secs: 0.0,
        }
    }

    /// Create an instant that appears to be `secs` seconds old.
    /// For testing temporal decay without real sleeps.
    pub fn with_age_secs(secs: f64) -> Self {
        Self {
            created: std::time::Instant::now(),
            age_offset_secs: secs,
        }
    }

    /// Elapsed time in seconds since creation (plus any artificial offset).
    pub fn elapsed_secs(&self) -> f64 {
        self.created.elapsed().as_secs_f64() + self.age_offset_secs
    }
}

/// Exponential temporal decay.
///
/// Returns a weight in [0, 1] that decays over time.
/// decay(0, λ) = 1.0 (just happened → full weight).
/// decay(∞, λ) → 0.0 (long ago → faded).
///
/// λ controls decay rate. Use `lambda_from_half_life` for intuitive parameterization.
pub fn temporal_decay(elapsed_secs: f64, lambda: f64) -> f64 {
    (-lambda * elapsed_secs).exp()
}

/// Compute λ from a half-life in seconds.
///
/// After `half_life_secs`, the decay weight will be 0.5.
/// Example: lambda_from_half_life(3600.0) → decay to 50% after 1 hour.
pub fn lambda_from_half_life(half_life_secs: f64) -> f64 {
    assert!(half_life_secs > 0.0, "Half-life must be positive");
    2.0_f64.ln() / half_life_secs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lamport_clock_starts_at_zero() {
        let clock = LamportClock::new();
        assert_eq!(clock.current().value(), 0);
    }

    #[test]
    fn lamport_tick_increments() {
        let mut clock = LamportClock::new();
        let t1 = clock.tick();
        let t2 = clock.tick();
        let t3 = clock.tick();
        assert_eq!(t1.value(), 1);
        assert_eq!(t2.value(), 2);
        assert_eq!(t3.value(), 3);
    }

    #[test]
    fn lamport_receive_takes_max_plus_one() {
        let mut clock = LamportClock::new();
        clock.tick(); // counter = 1
        let received = clock.receive(LamportTimestamp(10));
        assert_eq!(received.value(), 11);
    }

    #[test]
    fn lamport_receive_from_lower_still_increments() {
        let mut clock = LamportClock::new();
        clock.tick(); // 1
        clock.tick(); // 2
        clock.tick(); // 3
        let received = clock.receive(LamportTimestamp(1));
        // max(3, 1) + 1 = 4
        assert_eq!(received.value(), 4);
    }

    #[test]
    fn lamport_timestamps_are_ordered() {
        let mut clock = LamportClock::new();
        let t1 = clock.tick();
        let t2 = clock.tick();
        assert!(t1 < t2);
    }

    #[test]
    fn temporal_decay_at_zero_is_one() {
        let w = temporal_decay(0.0, 0.5);
        assert!((w - 1.0).abs() < 1e-10);
    }

    #[test]
    fn temporal_decay_at_half_life_is_half() {
        let lambda = lambda_from_half_life(60.0); // 60 second half-life
        let w = temporal_decay(60.0, lambda);
        assert!((w - 0.5).abs() < 1e-10);
    }

    #[test]
    fn temporal_decay_is_monotonically_decreasing() {
        let lambda = lambda_from_half_life(100.0);
        let w1 = temporal_decay(10.0, lambda);
        let w2 = temporal_decay(50.0, lambda);
        let w3 = temporal_decay(200.0, lambda);
        assert!(w1 > w2);
        assert!(w2 > w3);
        assert!(w3 > 0.0);
    }

    #[test]
    fn lambda_from_half_life_roundtrip() {
        let half_life = 3600.0;
        let lambda = lambda_from_half_life(half_life);
        let w = temporal_decay(half_life, lambda);
        assert!((w - 0.5).abs() < 1e-10);
    }

    #[test]
    fn fabric_instant_elapsed_is_non_negative() {
        let instant = FabricInstant::now();
        assert!(instant.elapsed_secs() >= 0.0);
    }

    #[test]
    fn fabric_instant_with_age_adds_offset() {
        let instant = FabricInstant::with_age_secs(100.0);
        assert!(instant.elapsed_secs() >= 100.0);
    }

    #[test]
    fn lamport_timestamp_display() {
        let ts = LamportTimestamp(42);
        assert_eq!(format!("{}", ts), "t=42");
    }
}
