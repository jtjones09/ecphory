// FABRIC OBSERVABILITY — DTRACE PHILOSOPHY
//
// Probes always present. Zero cost when disabled.
// Activated without restart. (Cantrill et al. 2004)
//
// Phase 2: Trait + basic implementations.
// Phase 3: Semantic breakpoints, fabric snapshots, cost accounting.

use crate::signature::LineageId;
use crate::temporal::LamportTimestamp;

/// What kind of fabric event occurred.
#[derive(Debug, Clone)]
pub enum TraceEvent {
    /// Node was added to the fabric.
    NodeAdded { lineage_id: LineageId },
    /// Node was faded (removed) from the fabric.
    NodeFaded { lineage_id: LineageId },
    /// Node was mutated (content changed, signature recomputed).
    NodeMutated {
        lineage_id: LineageId,
        old_version: u64,
        new_version: u64,
    },
    /// Edge was added between two nodes.
    EdgeAdded {
        from: LineageId,
        to: LineageId,
        kind: String,
        weight: f64,
    },
    /// Edge was removed between two nodes.
    EdgeRemoved { from: LineageId, to: LineageId },
    /// Resonance query was performed.
    ResonanceQuery {
        query: String,
        results_count: usize,
        top_score: f64,
    },
    /// Lamport clock ticked.
    ClockTick { timestamp: LamportTimestamp },
}

/// Trait for fabric observability.
///
/// Implement this to receive trace events from the fabric.
/// The fabric calls `trace()` on every significant operation.
/// Default implementation: NoopTracer (zero cost).
pub trait FabricTracer: Send + Sync {
    fn trace(&self, event: &TraceEvent);
}

/// Does nothing. The default. Zero cost when called.
pub struct NoopTracer;

impl FabricTracer for NoopTracer {
    #[inline(always)]
    fn trace(&self, _event: &TraceEvent) {}
}

/// Prints trace events to stderr. For development and debugging.
pub struct PrintTracer;

impl FabricTracer for PrintTracer {
    fn trace(&self, event: &TraceEvent) {
        eprintln!("[FABRIC] {:?}", event);
    }
}

/// Collects trace events in a Vec. For testing.
///
/// Uses `Mutex` for interior mutability so the tracer can be shared
/// with the fabric across threads (the bridge requires `Send + Sync`).
pub struct CollectingTracer {
    events: std::sync::Mutex<Vec<TraceEvent>>,
}

impl CollectingTracer {
    pub fn new() -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Get a snapshot of all collected events.
    pub fn events(&self) -> Vec<TraceEvent> {
        self.events.lock().expect("CollectingTracer poisoned").clone()
    }

    /// How many events have been collected.
    pub fn count(&self) -> usize {
        self.events.lock().expect("CollectingTracer poisoned").len()
    }

    /// Clear all collected events.
    pub fn clear(&self) {
        self.events.lock().expect("CollectingTracer poisoned").clear();
    }
}

impl Default for CollectingTracer {
    fn default() -> Self {
        Self::new()
    }
}

impl FabricTracer for CollectingTracer {
    fn trace(&self, event: &TraceEvent) {
        self.events
            .lock()
            .expect("CollectingTracer poisoned")
            .push(event.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_tracer_does_not_panic() {
        let tracer = NoopTracer;
        tracer.trace(&TraceEvent::NodeAdded {
            lineage_id: LineageId::new(),
        });
    }

    #[test]
    fn print_tracer_does_not_panic() {
        let tracer = PrintTracer;
        tracer.trace(&TraceEvent::NodeAdded {
            lineage_id: LineageId::new(),
        });
    }

    #[test]
    fn collecting_tracer_starts_empty() {
        let tracer = CollectingTracer::new();
        assert_eq!(tracer.count(), 0);
        assert!(tracer.events().is_empty());
    }

    #[test]
    fn collecting_tracer_records_events() {
        let tracer = CollectingTracer::new();
        tracer.trace(&TraceEvent::NodeAdded {
            lineage_id: LineageId::new(),
        });
        tracer.trace(&TraceEvent::NodeFaded {
            lineage_id: LineageId::new(),
        });
        assert_eq!(tracer.count(), 2);
    }

    #[test]
    fn collecting_tracer_clear_works() {
        let tracer = CollectingTracer::new();
        tracer.trace(&TraceEvent::NodeAdded {
            lineage_id: LineageId::new(),
        });
        assert_eq!(tracer.count(), 1);
        tracer.clear();
        assert_eq!(tracer.count(), 0);
    }
}
