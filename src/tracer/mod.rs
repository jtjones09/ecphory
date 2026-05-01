// FABRIC OBSERVABILITY — DTRACE PHILOSOPHY
//
// Probes always present. Zero cost when disabled.
// Activated without restart. (Cantrill et al. 2004)
//
// Phase 2: Trait + basic implementations.
// Phase 3: Semantic breakpoints, fabric snapshots, cost accounting.

use crate::identity::{NamespaceId, VoicePrint};
use crate::signature::LineageId;
use crate::temporal::LamportTimestamp;

/// What kind of fabric event occurred.
///
/// **Spec 6 Step 1 expansion (immune-relevant events).** The
/// `Attestation*`, `ContentFingerprint*`, `SubscriptionFired`,
/// `ConsensusSnapshotEmitted`, `P53NodeTerminated`,
/// `RegionDecayThresholdCrossed`, and `RegionSilent` variants are the
/// substrate the cell-agent population (Spec 6 §3.2) consumes to
/// maintain behavioral baselines. They are added at this step as a
/// pure event-surface expansion — no behavior change in existing
/// fabric paths; cell-agents will start emitting them as they land.
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

    // ── Spec 6 immune-system event surface ─────────────────────────

    /// Per-node Ed25519 signature on a high-sensitivity node verified
    /// successfully (Spec 5 §3.3, Spec 6 §3.3.2 `AttestationObserver`).
    AttestationVerified {
        signer: VoicePrint,
        target_node: LineageId,
    },
    /// Per-node signature verification failed. Cell-agents flag this
    /// as a `DamageObservation` (Matzinger fold — high-confidence
    /// evidence of harm).
    AttestationFailed {
        signer: VoicePrint,
        target_node: LineageId,
        reason: String,
    },
    /// Content fingerprint verification on a node passed. Universal
    /// across all regions (Spec 5 §3.1) — emitted on boot-time
    /// re-check and on quarantine-trigger paths.
    ContentFingerprintVerified { lineage_id: LineageId },
    /// Content fingerprint mismatch. Per Spec 5 §3.1 + Matzinger fold
    /// this is a `DamageObservation` — evidence of corruption /
    /// tampering, not a baseline anomaly.
    ContentFingerprintFailed { lineage_id: LineageId },
    /// A subscription's pattern matched a node and the dispatch pool
    /// invoked the callback. Cell-agents track per-region firing
    /// rates to baseline observation traffic.
    SubscriptionFired {
        subscription_id: u64,
        observed_node: LineageId,
    },
    /// A `ConsensusSnapshot` node was committed to the fabric (Spec 8
    /// §3.4.3). `ConsensusObserver` baselines escalation patterns
    /// against this stream.
    ConsensusSnapshotEmitted {
        snapshot_id: LineageId,
        target: LineageId,
        finalized_count: usize,
    },
    /// `P53Scope::Node` self-termination (Spec 8 §8.4.1, Cohen I.3
    /// fold — routine maintenance, not an alert).
    P53NodeTerminated { node: LineageId },
    /// `DecayObserver` detected a region-level decay-throughput
    /// excursion (Spec 6 §3.3.3). Carries the observed rate so the
    /// observer / Cognitive Map can consume it directly.
    RegionDecayThresholdCrossed {
        region: NamespaceId,
        observed_rate: f64,
        baseline_rate: f64,
    },
    /// `SilenceObserver` detected an unexpected quiet window — a
    /// region went `silent_secs` seconds without a write while its
    /// baseline expected activity within `expected_within_secs`
    /// (Spec 6 §3.3.6).
    RegionSilent {
        region: NamespaceId,
        silent_secs: f64,
        expected_within_secs: f64,
    },
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

    // ── Spec 6 Step 1 — immune-relevant events ──

    #[test]
    fn immune_event_variants_construct_and_trace() {
        // The Spec 6 expansion is a pure event-surface addition. This
        // test confirms each new variant constructs without panic and
        // that NoopTracer / CollectingTracer accept them — Step 2's
        // cell-agents will consume them via subscribing tracers.
        use crate::identity::{generate_agent_keypair, NamespaceId};
        let agent = generate_agent_keypair();
        let node = LineageId::new();
        let region = NamespaceId::fresh("test-region");

        let events = vec![
            TraceEvent::AttestationVerified {
                signer: agent.voice_print(),
                target_node: node.clone(),
            },
            TraceEvent::AttestationFailed {
                signer: agent.voice_print(),
                target_node: node.clone(),
                reason: "invalid signature".into(),
            },
            TraceEvent::ContentFingerprintVerified {
                lineage_id: node.clone(),
            },
            TraceEvent::ContentFingerprintFailed {
                lineage_id: node.clone(),
            },
            TraceEvent::SubscriptionFired {
                subscription_id: 1,
                observed_node: node.clone(),
            },
            TraceEvent::ConsensusSnapshotEmitted {
                snapshot_id: LineageId::new(),
                target: node.clone(),
                finalized_count: 3,
            },
            TraceEvent::P53NodeTerminated { node: node.clone() },
            TraceEvent::RegionDecayThresholdCrossed {
                region: region.clone(),
                observed_rate: 0.85,
                baseline_rate: 0.30,
            },
            TraceEvent::RegionSilent {
                region: region.clone(),
                silent_secs: 600.0,
                expected_within_secs: 60.0,
            },
        ];

        let collecting = CollectingTracer::new();
        for e in &events {
            // NoopTracer must accept every variant.
            NoopTracer.trace(e);
            // CollectingTracer must record every variant.
            collecting.trace(e);
        }
        assert_eq!(collecting.count(), events.len());
    }

    #[test]
    fn print_tracer_handles_immune_events() {
        // Smoke test that PrintTracer's Debug formatting works on
        // every new variant.
        use crate::identity::{generate_agent_keypair, NamespaceId};
        let agent = generate_agent_keypair();
        PrintTracer.trace(&TraceEvent::AttestationFailed {
            signer: agent.voice_print(),
            target_node: LineageId::new(),
            reason: "test".into(),
        });
        PrintTracer.trace(&TraceEvent::RegionSilent {
            region: NamespaceId::fresh("r"),
            silent_secs: 1.0,
            expected_within_secs: 1.0,
        });
    }
}
