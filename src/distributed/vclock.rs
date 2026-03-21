// VECTOR CLOCK — DISTRIBUTED CAUSAL ORDERING
//
// Replaces single Lamport clock for multi-replica scenarios.
// Each replica maintains its own counter; the vector tracks
// all known replica counters.
//
// Design decisions:
// 1. ReplicaId is a UUID — globally unique, no coordination needed.
// 2. Vector clock is a HashMap<ReplicaId, u64>.
// 3. Partial ordering: concurrent events are explicitly detectable.
// 4. Phase 3d: single-process simulation. Real network deferred.

use std::collections::HashMap;
use std::cmp::Ordering;
use std::fmt;

/// Unique identifier for a fabric replica.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReplicaId(uuid::Uuid);

impl ReplicaId {
    /// Create a new unique replica ID.
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// Create from a UUID (for persistence/testing).
    pub fn from_uuid(uuid: uuid::Uuid) -> Self {
        Self(uuid)
    }

    /// Get the underlying UUID.
    pub fn as_uuid(&self) -> &uuid::Uuid {
        &self.0
    }
}

impl fmt::Display for ReplicaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Replica({})", &self.0.to_string()[..8])
    }
}

/// A vector clock for distributed causal ordering.
///
/// Tracks the latest known counter for each replica.
/// Enables detecting causality (happens-before) vs concurrency.
#[derive(Debug, Clone)]
pub struct VectorClock {
    /// ReplicaId → counter mapping.
    counters: HashMap<ReplicaId, u64>,
}

impl VectorClock {
    /// Create a new empty vector clock.
    pub fn new() -> Self {
        Self {
            counters: HashMap::new(),
        }
    }

    /// Tick the clock for a specific replica.
    /// Increments that replica's counter by 1.
    pub fn tick(&mut self, replica: &ReplicaId) -> u64 {
        let counter = self.counters.entry(replica.clone()).or_insert(0);
        *counter += 1;
        *counter
    }

    /// Get the counter value for a specific replica.
    pub fn get(&self, replica: &ReplicaId) -> u64 {
        *self.counters.get(replica).unwrap_or(&0)
    }

    /// Merge with another vector clock (point-wise maximum).
    /// Used when receiving a message from another replica.
    pub fn merge(&mut self, other: &VectorClock) {
        for (replica, &count) in &other.counters {
            let entry = self.counters.entry(replica.clone()).or_insert(0);
            *entry = (*entry).max(count);
        }
    }

    /// Compare two vector clocks for causal ordering.
    ///
    /// Returns:
    /// - Some(Less) if self happened-before other
    /// - Some(Greater) if other happened-before self
    /// - Some(Equal) if identical
    /// - None if concurrent (incomparable)
    pub fn partial_cmp(&self, other: &VectorClock) -> Option<Ordering> {
        let all_replicas: std::collections::HashSet<&ReplicaId> = self.counters.keys()
            .chain(other.counters.keys())
            .collect();

        let mut has_less = false;
        let mut has_greater = false;

        for replica in all_replicas {
            let self_val = self.get(replica);
            let other_val = other.get(replica);

            if self_val < other_val {
                has_less = true;
            } else if self_val > other_val {
                has_greater = true;
            }

            // Early exit: if both less and greater found, it's concurrent.
            if has_less && has_greater {
                return None;
            }
        }

        match (has_less, has_greater) {
            (false, false) => Some(Ordering::Equal),
            (true, false) => Some(Ordering::Less),
            (false, true) => Some(Ordering::Greater),
            (true, true) => None, // Concurrent (shouldn't reach due to early exit)
        }
    }

    /// Check if this clock happened-before the other.
    pub fn happened_before(&self, other: &VectorClock) -> bool {
        self.partial_cmp(other) == Some(Ordering::Less)
    }

    /// Check if two clocks are concurrent (neither happened-before the other).
    pub fn is_concurrent_with(&self, other: &VectorClock) -> bool {
        self.partial_cmp(other).is_none()
    }

    /// Number of replicas tracked.
    pub fn replica_count(&self) -> usize {
        self.counters.len()
    }

    /// Get all replica counters.
    pub fn counters(&self) -> &HashMap<ReplicaId, u64> {
        &self.counters
    }
}

impl Default for VectorClock {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for VectorClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VC{{")?;
        let mut first = true;
        for (replica, count) in &self.counters {
            if !first { write!(f, ", ")?; }
            write!(f, "{}:{}", &replica.0.to_string()[..8], count)?;
            first = false;
        }
        write!(f, "}}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_clock_is_empty() {
        let vc = VectorClock::new();
        assert_eq!(vc.replica_count(), 0);
    }

    #[test]
    fn tick_increments_counter() {
        let mut vc = VectorClock::new();
        let r = ReplicaId::new();
        assert_eq!(vc.get(&r), 0);
        vc.tick(&r);
        assert_eq!(vc.get(&r), 1);
        vc.tick(&r);
        assert_eq!(vc.get(&r), 2);
    }

    #[test]
    fn merge_takes_pointwise_max() {
        let r1 = ReplicaId::new();
        let r2 = ReplicaId::new();

        let mut vc1 = VectorClock::new();
        vc1.tick(&r1); vc1.tick(&r1); // r1:2
        vc1.tick(&r2);                 // r2:1

        let mut vc2 = VectorClock::new();
        vc2.tick(&r1);                 // r1:1
        vc2.tick(&r2); vc2.tick(&r2); // r2:2

        vc1.merge(&vc2);
        assert_eq!(vc1.get(&r1), 2); // max(2,1)
        assert_eq!(vc1.get(&r2), 2); // max(1,2)
    }

    #[test]
    fn equal_clocks() {
        let r = ReplicaId::new();
        let mut vc1 = VectorClock::new();
        let mut vc2 = VectorClock::new();
        vc1.tick(&r);
        vc2.tick(&r);
        assert_eq!(vc1.partial_cmp(&vc2), Some(Ordering::Equal));
    }

    #[test]
    fn happened_before() {
        let r = ReplicaId::new();
        let mut vc1 = VectorClock::new();
        let mut vc2 = VectorClock::new();
        vc1.tick(&r);
        vc2.tick(&r); vc2.tick(&r);
        assert!(vc1.happened_before(&vc2));
        assert!(!vc2.happened_before(&vc1));
    }

    #[test]
    fn concurrent_clocks() {
        let r1 = ReplicaId::new();
        let r2 = ReplicaId::new();

        let mut vc1 = VectorClock::new();
        vc1.tick(&r1); // r1:1, r2:0

        let mut vc2 = VectorClock::new();
        vc2.tick(&r2); // r1:0, r2:1

        assert!(vc1.is_concurrent_with(&vc2));
        assert!(vc2.is_concurrent_with(&vc1));
        assert_eq!(vc1.partial_cmp(&vc2), None);
    }

    #[test]
    fn empty_clocks_are_equal() {
        let vc1 = VectorClock::new();
        let vc2 = VectorClock::new();
        assert_eq!(vc1.partial_cmp(&vc2), Some(Ordering::Equal));
    }

    #[test]
    fn display_format() {
        let vc = VectorClock::new();
        let s = format!("{}", vc);
        assert!(s.starts_with("VC{"));
        assert!(s.ends_with("}"));
    }

    #[test]
    fn replica_id_unique() {
        let r1 = ReplicaId::new();
        let r2 = ReplicaId::new();
        assert_ne!(r1, r2);
    }
}
