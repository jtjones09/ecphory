// CELL-AGENT TRAIT — The atomic primitive of the cognitive immune layer
// (Spec 6 §3.2)
//
// The trait is the minimum surface every specialization implements. The
// fabric routes matching events to a cell-agent's `observe()` (under a
// per-cell-agent mutex per KING.1 fold), the cell-agent updates its
// baseline and decides whether to emit a signal, then it returns one of:
//
//   - `ObservationOutcome::Quiet` — observation absorbed; no signal
//   - `ObservationOutcome::Anomaly(_)` — deviation, low confidence
//     ("something changed" — Matzinger non-self signal)
//   - `ObservationOutcome::Damage(_)` — evidence of harm, high confidence
//     ("something is broken" — Matzinger danger signal)
//   - `ObservationOutcome::BaselineHealthy(_)` — periodic maintenance
//     emission (≥80% of output per COHEN.1)
//
// AnomalyObservation and DamageObservation are SEPARATE types per
// MATZINGER.1 — escalation rules in Step 4 weight damage heavily and
// anomaly lightly. Pure anomaly convergence does not auto-trigger
// `P53Scope::Region`; it requires at least one damage observation.

use crate::identity::{NamespaceId, VoicePrint};
use crate::node::IntentNode;
use crate::signature::LineageId;
use crate::temporal::{FabricInstant, LamportTimestamp};
use crate::tracer::TraceEvent;
use std::time::Duration;

/// Stable identifier for a cell-agent. Carries a UUID so two
/// cell-agents of the same specialization in the same region remain
/// distinct (e.g., a fresh cell-agent inheriting baseline from a
/// retiring predecessor — both observable in the cognitive map).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CellAgentId(pub uuid::Uuid);

impl CellAgentId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for CellAgentId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for CellAgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = self.0.to_string();
        write!(f, "cell:{}", &s[..8])
    }
}

/// What kind of fabric activity a cell-agent attaches to.
///
/// In v1 the fabric routes by activity kind rather than a full
/// `Predicate` closure — the six specializations have stable
/// activity profiles and a closure-based predicate would obscure the
/// dispatcher's per-region routing logic. Step 4 (aggregation) keys
/// the convergence layer off these stable patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImmunePattern {
    /// Every node creation in the watched region.
    NodeCreates,
    /// `AttestationVerified` and `AttestationFailed` events in the
    /// watched region.
    AttestationEvents,
    /// `RegionDecayThresholdCrossed` events + decay-tick reports.
    DecayEvents,
    /// `ConsensusSnapshotEmitted` events.
    ConsensusEvents,
    /// Edge formations within the watched region.
    EdgeFormation,
    /// `RegionSilent` events + the absence of activity.
    SilenceEvents,
}

/// What the cell-agent saw — either a fabric node or an out-of-band
/// trace event. A specialization may care about one or both.
#[derive(Debug, Clone)]
pub enum ObservedEvent<'a> {
    Node(&'a IntentNode),
    Trace(&'a TraceEvent),
    /// Synthetic "tick" used for time-based observers (e.g.,
    /// `SilenceObserver`) and for retune cadence. The fabric calls
    /// observe() with this when a per-cell-agent timer elapses.
    Tick,
}

/// Read-only context carried into each `observe()` call. Lets the
/// cell-agent reason about its place in the dispatch stream without
/// reaching into the fabric directly.
#[derive(Debug, Clone)]
pub struct ObservationContext {
    /// Wall-clock instant the dispatcher invoked `observe`.
    pub fabric_instant: FabricInstant,
    /// Lamport position at observation time.
    pub lamport: LamportTimestamp,
    /// LineageIds of recent anomaly / damage observations in the
    /// same region — lets a cell-agent suppress repeat firing on the
    /// same incident. Carries up to N=16 (configurable in Step 4).
    pub recent_signals: Vec<LineageId>,
}

impl Default for ObservationContext {
    fn default() -> Self {
        Self {
            fabric_instant: FabricInstant::now(),
            lamport: LamportTimestamp::new(0),
            recent_signals: Vec::new(),
        }
    }
}

/// Severity classification — kept on Anomaly + Damage observations so
/// the aggregation layer can weight signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObservationSeverity {
    /// Routine deviation; alone, never escalates.
    Low,
    /// Significant deviation; multi-cell-agent convergence triggers a
    /// `ConvergedAnomaly` per Spec 6 §5.2.1.
    Medium,
    /// Strong deviation. Always counts toward convergence.
    High,
    /// Reserved for `DamageObservation` — evidence of actual harm.
    Damage,
}

/// Anomaly: deviation from baseline. Low confidence "something changed."
#[derive(Debug, Clone)]
pub struct AnomalyObservation {
    pub observer: VoicePrint,
    pub observer_cell_agent_id: CellAgentId,
    pub region: NamespaceId,
    pub specialization: &'static str,
    pub observed_value: f64,
    pub baseline_value: f64,
    pub deviation_z_score: f64,
    pub severity: ObservationSeverity,
    pub confidence: f64,
    pub explanation: String,
    pub at: FabricInstant,
}

/// Damage: evidence of harm. High confidence "something is broken."
/// Per Matzinger fold these are weighted heavily by aggregation —
/// any damage observation among the convergent signals is sufficient
/// to trigger `P53Scope::Region` escalation.
#[derive(Debug, Clone)]
pub struct DamageObservation {
    pub observer: VoicePrint,
    pub observer_cell_agent_id: CellAgentId,
    pub region: NamespaceId,
    pub specialization: &'static str,
    pub damage_kind: &'static str,
    pub explanation: String,
    pub at: FabricInstant,
}

/// Maintenance signal — periodic emission proving the cell-agent is
/// alive and the region's baseline is steady. Per COHEN.1, ≥80% of
/// cell-agent output is `BaselineHealthy`.
#[derive(Debug, Clone)]
pub struct BaselineHealthy {
    pub observer: VoicePrint,
    pub observer_cell_agent_id: CellAgentId,
    pub region: NamespaceId,
    pub specialization: &'static str,
    pub baseline_value: f64,
    pub baseline_stddev: f64,
    pub observation_count: u64,
    pub at: FabricInstant,
}

/// Outcome of a single `observe()` call.
#[derive(Debug, Clone)]
pub enum ObservationOutcome {
    Quiet,
    Anomaly(AnomalyObservation),
    Damage(DamageObservation),
    BaselineHealthy(BaselineHealthy),
}

impl ObservationOutcome {
    /// Convenient classifier for the maintenance/defense ratio test
    /// (COHEN.1) — `BaselineHealthy` is maintenance, anomaly + damage
    /// are defense, `Quiet` is neither (no fabric node emitted).
    pub fn is_maintenance(&self) -> bool {
        matches!(self, ObservationOutcome::BaselineHealthy(_))
    }
    pub fn is_defense(&self) -> bool {
        matches!(self, ObservationOutcome::Anomaly(_) | ObservationOutcome::Damage(_))
    }
}

/// What `retune()` reports back to the fabric.
#[derive(Debug, Clone)]
pub struct RetuneReport {
    pub baseline_value: f64,
    pub baseline_stddev: f64,
    pub observation_count: u64,
    pub baseline_shifted: bool,
    pub baseline_healthy: BaselineHealthy,
}

/// Self-reported health of a cell-agent. The fabric checks this before
/// each `observe` and may replace a `Misfiring` or `Retired` agent.
#[derive(Debug, Clone)]
pub enum CellAgentHealth {
    Healthy,
    Stale {
        last_observation: FabricInstant,
    },
    Misfiring {
        false_positive_rate: f64,
    },
    Retired,
}

/// The cell-agent trait. Implementations are `Send + Sync`; the
/// dispatcher serializes per-cell-agent `observe()` calls under a
/// per-cell-agent mutex (KING.1 fold) so trait methods take `&mut self`
/// rather than wrapping their own interior mutability.
pub trait CellAgent: Send + Sync {
    fn id(&self) -> CellAgentId;
    fn voice_print(&self) -> VoicePrint;
    fn region(&self) -> &NamespaceId;
    fn specialization(&self) -> &'static str;
    fn pattern(&self) -> ImmunePattern;

    /// Invoked by the dispatcher on every event matching `pattern()`.
    fn observe(&mut self, event: ObservedEvent<'_>, ctx: &ObservationContext)
        -> ObservationOutcome;

    /// Periodic re-tuning. Called on a schedule (default: hourly per
    /// `retune_interval`). Always emits a `BaselineHealthy` so
    /// healthy operation always logs maintenance — this is the COHEN.1
    /// 80/20 mechanic.
    fn retune(&mut self) -> RetuneReport;

    /// Default cadence for `retune()`. Specializations override.
    fn retune_interval(&self) -> Duration {
        Duration::from_secs(3600)
    }

    fn health(&self) -> CellAgentHealth {
        CellAgentHealth::Healthy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_agent_id_is_unique() {
        let a = CellAgentId::new();
        let b = CellAgentId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn observation_outcome_classifier() {
        let voice = VoicePrint::from_bytes([0u8; 32]);
        let region = NamespaceId::fresh("r");
        let id = CellAgentId::new();
        let healthy = ObservationOutcome::BaselineHealthy(BaselineHealthy {
            observer: voice,
            observer_cell_agent_id: id.clone(),
            region: region.clone(),
            specialization: "Test",
            baseline_value: 1.0,
            baseline_stddev: 0.1,
            observation_count: 10,
            at: FabricInstant::now(),
        });
        assert!(healthy.is_maintenance());
        assert!(!healthy.is_defense());

        let anomaly = ObservationOutcome::Anomaly(AnomalyObservation {
            observer: voice,
            observer_cell_agent_id: id.clone(),
            region: region.clone(),
            specialization: "Test",
            observed_value: 5.0,
            baseline_value: 1.0,
            deviation_z_score: 5.0,
            severity: ObservationSeverity::High,
            confidence: 0.9,
            explanation: "test".into(),
            at: FabricInstant::now(),
        });
        assert!(!anomaly.is_maintenance());
        assert!(anomaly.is_defense());

        let damage = ObservationOutcome::Damage(DamageObservation {
            observer: voice,
            observer_cell_agent_id: id,
            region,
            specialization: "Test",
            damage_kind: "fingerprint_failed",
            explanation: "test".into(),
            at: FabricInstant::now(),
        });
        assert!(!damage.is_maintenance());
        assert!(damage.is_defense());

        let quiet = ObservationOutcome::Quiet;
        assert!(!quiet.is_maintenance());
        assert!(!quiet.is_defense());
    }
}
